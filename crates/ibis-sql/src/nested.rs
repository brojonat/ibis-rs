//! Arrays, structs and maps: literals and functions as DuckDB spells them.
//! Engines agree on little here, so most dialects override most of this.

use ibis_core::{DataType, Expr, Func, Lambda, Result, Value};

use crate::compiler::{ExprCompiler, quote_str, unsupported};

/// Whether a value of this type can be written without saying its type:
/// false when part of it is untyped, as in an empty array literal.
fn fully_typed(t: &DataType) -> bool {
    match t {
        DataType::Null => false,
        DataType::Array(item) => fully_typed(item),
        DataType::Map(k, v) => fully_typed(k) && fully_typed(v),
        DataType::Struct(fields) => fields.iter().all(|(_, t)| fully_typed(t)),
        _ => true,
    }
}

impl ExprCompiler<'_> {
    /// `CAST(x AS type)` when the type can be named, else `x`.
    pub fn typed(&self, x: String, t: &DataType) -> Result<String> {
        Ok(if fully_typed(t) { format!("CAST({x} AS {})", self.d.cast_type_name(t)?) } else { x })
    }

    /// The parts of a nested literal, each compiled as a literal of its own:
    /// the elements of an array, the values of a struct's fields, or the
    /// keys then the values of a map.
    pub fn literal_parts(&self, v: &Value, t: &DataType) -> Result<Vec<String>> {
        let null = DataType::Null;
        match (v, t) {
            (Value::Array(items), _) => {
                let item = t.item().unwrap_or(&null);
                items.iter().map(|v| self.literal(v, item)).collect()
            }
            (Value::Struct(fields), _) => {
                fields.iter().map(|(name, v)| self.literal(v, t.field(name).unwrap_or(&null))).collect()
            }
            (Value::Map(entries), _) => {
                let (kt, vt) = match t {
                    DataType::Map(k, v) => (&**k, &**v),
                    _ => (&null, &null),
                };
                let keys = entries.iter().map(|(k, _)| self.literal(k, kt));
                keys.chain(entries.iter().map(|(_, v)| self.literal(v, vt))).collect()
            }
            _ => Ok(vec![]),
        }
    }

    pub(crate) fn nested_literal(&self, v: &Value, t: &DataType) -> Result<String> {
        let parts = self.literal_parts(v, t)?;
        let body = match v {
            Value::Array(_) => format!("[{}]", parts.join(", ")),
            Value::Struct(fields) => {
                let pairs: Vec<String> =
                    fields.iter().zip(&parts).map(|((name, _), v)| format!("{}: {v}", quote_str(name))).collect();
                format!("{{{}}}", pairs.join(", "))
            }
            Value::Map(_) => {
                let (keys, values) = parts.split_at(parts.len() / 2);
                format!("MAP([{}], [{}])", keys.join(", "), values.join(", "))
            }
            _ => return unsupported(self.d, format!("literal {v:?}")),
        };
        self.typed(body, t)
    }

    /// An array position as DuckDB counts: from 1, negative from the end.
    fn one_based(&self, e: &Expr) -> Result<String> {
        Ok(match Self::lit_int(e) {
            Some(i) if i >= 0 => (i + 1).to_string(),
            Some(i) => i.to_string(),
            None => {
                let i = self.sql(e)?;
                format!("CASE WHEN {i} >= 0 THEN {i} + 1 ELSE {i} END")
            }
        })
    }

    /// `CASE WHEN a IS NULL OR b IS NULL ... THEN NULL ELSE body END`
    pub fn null_if_any_null(&self, args: &[String], body: String) -> String {
        let any: Vec<String> = args.iter().map(|a| format!("{a} IS NULL")).collect();
        format!("CASE WHEN {} THEN NULL ELSE {body} END", any.join(" OR "))
    }

    pub(crate) fn nested_func(&self, f: Func, args: &[Expr], a: &[String]) -> Result<String> {
        let x = a.first().cloned().unwrap_or_default();
        let open = |e: &Expr| matches!(e, Expr::Literal(Value::Null, _));
        use Func::*;
        Ok(match f {
            Array => {
                let t = self.dtype(&Expr::Func(f, args.to_vec()))?;
                self.typed(format!("[{}]", a.join(", ")), &t)?
            }
            ArrayLength => format!("LEN({x})"),
            ArrayIndex => format!("LIST_EXTRACT({x}, {})", self.one_based(&args[1])?),
            ArrayConcat => {
                let joined = a.iter().skip(1).fold(x.clone(), |acc, y| format!("LIST_CONCAT({acc}, {y})"));
                self.null_if_any_null(a, joined)
            }
            ArrayRepeat => format!("FLATTEN(LIST_APPLY(RANGE({}), LAMBDA _x: {x}))", a[1]),
            ArraySlice => {
                let start = if open(&args[1]) { "1".to_string() } else { self.one_based(&args[1])? };
                let stop = match Self::lit_int(&args[2]) {
                    _ if open(&args[2]) => format!("LEN({x})"),
                    Some(i) if i >= 0 => i.to_string(),
                    Some(i) => (i - 1).to_string(),
                    None => format!("CASE WHEN {i} < 0 THEN {i} - 1 ELSE {i} END", i = a[2]),
                };
                format!("LIST_SLICE({x}, {start}, {stop})")
            }
            ArrayContains => format!("LIST_CONTAINS({x}, {})", a[1]),
            ArrayPosition => self.null_if_any_null(a, format!("COALESCE(LIST_INDEXOF({x}, {}), 0) - 1", a[1])),
            ArrayRemove => {
                format!("LIST_FILTER({x}, LAMBDA _x: _x <> {v} OR (_x IS NULL AND {v} IS NOT NULL))", v = a[1])
            }
            ArrayDistinct => self.distinct(&x),
            ArraySort => format!("LIST_SORT({x})"),
            ArrayUnion => self.null_if_any_null(a, self.distinct(&format!("LIST_CONCAT({x}, {})", a[1]))),
            ArrayIntersect => format!("LIST_FILTER({x}, LAMBDA _x: LIST_CONTAINS({}, _x))", a[1]),
            ArrayFlatten => format!("FLATTEN({x})"),
            Unnest => format!("UNNEST({x})"),
            ArrayMode => format!("LIST_MODE({x})"),
            ArrayMin => format!("LIST_MIN({x})"),
            ArrayMax => format!("LIST_MAX({x})"),
            ArraySum => format!("LIST_SUM({x})"),
            ArrayMean => format!("LIST_AVG({x})"),
            ArrayAny => format!("LIST_BOOL_OR({x})"),
            ArrayAll => format!("LIST_BOOL_AND({x})"),
            ArrayZip => {
                let lens: Vec<String> = a.iter().map(|arr| format!("LEN({arr})")).collect();
                let fields: Vec<String> =
                    a.iter().enumerate().map(|(i, arr)| format!("'f{}': {arr}[_i]", i + 1)).collect();
                let zipped = format!(
                    "LIST_APPLY(RANGE(1, GREATEST({}) + 1), LAMBDA _i: {{{}}})",
                    lens.join(", "),
                    fields.join(", ")
                );
                self.null_if_any_null(a, zipped)
            }
            Range => format!("RANGE({x}, {}, {})", a[1], a[2]),
            StructPack => {
                let fields: Vec<String> = a.chunks(2).map(|pair| format!("{}: {}", pair[0], pair[1])).collect();
                format!("{{{}}}", fields.join(", "))
            }
            StructField => format!("STRUCT_EXTRACT({x}, {})", a[1]),
            Map => self.null_if_any_null(a, format!("MAP({x}, {})", a[1])),
            MapGet => {
                let found =
                    format!("LIST_EXTRACT(CASE WHEN {k} IS NULL THEN NULL ELSE ELEMENT_AT({x}, {k}) END, 1)", k = a[1]);
                let value = if open(&args[2]) { found } else { format!("COALESCE({found}, {})", a[2]) };
                self.null_if_any_null(&a[..1], value)
            }
            MapContains => self.null_if_any_null(
                &a[..1],
                format!("(LEN(CASE WHEN {k} IS NULL THEN NULL ELSE ELEMENT_AT({x}, {k}) END) <> 0)", k = a[1]),
            ),
            MapLength => format!("CARDINALITY({x})"),
            MapKeys => format!("MAP_KEYS({x})"),
            MapValues => format!("MAP_VALUES({x})"),
            MapMerge => self.null_if_any_null(a, format!("MAP_CONCAT({x}, {})", a[1])),
            other => return unsupported(self.d, format!("{other:?}")),
        })
    }

    /// DuckDB's lambda, whose position argument counts from 1.
    pub(crate) fn default_array_lambda(
        &self,
        kind: Lambda,
        arg: &Expr,
        param: &str,
        index: Option<&str>,
        body: Expr,
    ) -> Result<String> {
        let (mut params, mut body) = (param.to_string(), body);
        if let Some(i) = index {
            params = format!("{params}, {i}");
            body = body.replace_param(i, &(Expr::Param(i.to_string(), DataType::Int64) - 1));
        }
        let function = match kind {
            Lambda::Map => "LIST_APPLY",
            Lambda::Filter => "LIST_FILTER",
        };
        Ok(format!("{function}({}, LAMBDA {params}: {})", self.sql(arg)?, self.sql(&body)?))
    }

    /// LIST_DISTINCT drops NULL elements; one is put back if there were any.
    fn distinct(&self, x: &str) -> String {
        format!(
            "CASE WHEN {x} IS NULL THEN NULL ELSE LIST_DISTINCT({x}) || CASE WHEN \
             LIST_BOOL_OR(LIST_APPLY({x}, LAMBDA _x: _x IS NULL)) THEN [NULL] ELSE [] END END"
        )
    }
}
