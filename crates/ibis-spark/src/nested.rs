//! Arrays, structs and maps on Spark: `ARRAY`, `STRUCT`, `MAP`.
//!
//! Spelled with functions Spark has had since 2.4 where there is a choice,
//! and so that nothing raises under ANSI mode (the default from Spark 4):
//! an index out of range and a missing map key are NULL, as in Ibis.

use ibis_core::{DataType, Expr, Func, Lambda, Result, Value};
use ibis_sql::{Dialect, ExprCompiler, Unnest};

use crate::dialect::{SparkDialect, quote_str, unsupported};

pub(crate) fn type_name(d: &SparkDialect, t: &DataType) -> Result<Option<String>> {
    Ok(match t {
        DataType::Array(item) => Some(format!("ARRAY<{}>", d.type_name(item)?)),
        DataType::Struct(fields) => {
            let fields = fields
                .iter()
                .map(|(name, t)| Ok(format!("{}: {}", d.quote_ident(name), d.type_name(t)?)))
                .collect::<Result<Vec<_>>>()?;
            Some(format!("STRUCT<{}>", fields.join(", ")))
        }
        DataType::Map(key, value) => Some(format!("MAP<{}, {}>", d.type_name(key)?, d.type_name(value)?)),
        _ => None,
    })
}

/// The comma-separated parts of a type's argument list, not splitting inside
/// brackets or backquotes.
fn top_level(args: &str) -> Vec<&str> {
    let (mut parts, mut depth, mut quoted, mut start) = (vec![], 0, false, 0);
    for (i, ch) in args.char_indices() {
        match ch {
            '`' => quoted = !quoted,
            '<' | '(' if !quoted => depth += 1,
            '>' | ')' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                parts.push(args[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(args[start..].trim());
    parts
}

/// `array<bigint>`, `map<string,bigint>` or `struct<a:double,b:string>` as an
/// Ibis type; `None` for any other type name. `scalar` reads the rest.
pub(crate) fn parse_type(name: &str, scalar: &dyn Fn(&str) -> DataType) -> Option<DataType> {
    let lower = name.to_ascii_lowercase();
    let inner = |prefix: &str| lower.starts_with(prefix).then(|| name[prefix.len()..].strip_suffix('>')).flatten();
    let parse = |t: &str| parse_type(t, scalar).unwrap_or_else(|| scalar(t));
    if let Some(item) = inner("array<") {
        return Some(DataType::array(parse(item)));
    }
    if let Some(kv) = inner("map<")
        && let [key, value] = top_level(kv)[..]
    {
        return Some(DataType::map(parse(key), parse(value)));
    }
    let fields = inner("struct<")?;
    Some(DataType::Struct(
        top_level(fields)
            .into_iter()
            .filter(|field| !field.is_empty())
            .filter_map(|field| {
                let (name, t) = match field.strip_prefix('`') {
                    Some(rest) => rest.split_once('`').map(|(n, t)| (n.replace("``", "`"), t))?,
                    None => field.split_once(':').map(|(n, t)| (n.trim().to_string(), t))?,
                };
                Some((name, parse(t.trim_start_matches(':').trim())))
            })
            .collect(),
    ))
}

fn typed(d: &SparkDialect, x: String, t: &DataType) -> Result<String> {
    Ok(format!("CAST({x} AS {})", d.type_name(t)?))
}

pub(crate) fn literal(d: &SparkDialect, c: &ExprCompiler, v: &Value, t: &DataType) -> Result<Option<String>> {
    let parts = c.literal_parts(v, t)?;
    let body = match v {
        Value::Array(_) => format!("ARRAY({})", parts.join(", ")),
        Value::Struct(fields) => {
            let pairs: Vec<String> =
                fields.iter().zip(&parts).map(|((name, _), v)| format!("{}, {v}", quote_str(name))).collect();
            format!("NAMED_STRUCT({})", pairs.join(", "))
        }
        Value::Map(entries) => {
            let (keys, values) = parts.split_at(entries.len());
            let pairs: Vec<String> = keys.iter().zip(values).map(|(k, v)| format!("{k}, {v}")).collect();
            format!("MAP({})", pairs.join(", "))
        }
        _ => return Ok(None),
    };
    typed(d, body, t).map(Some)
}

/// The elements of `x` that are not NULL: what a reduction sees.
fn present(x: &str) -> String {
    format!("FILTER({x}, ibis_e -> ibis_e IS NOT NULL)")
}

/// `SIZE` of a NULL is -1 on some settings; this is NULL on all of them.
fn size(x: &str) -> String {
    format!("CASE WHEN {x} IS NOT NULL THEN CAST(SIZE({x}) AS BIGINT) END")
}

pub(crate) fn func(d: &SparkDialect, c: &ExprCompiler, f: Func, args: &[Expr]) -> Result<Option<String>> {
    use Func::*;
    if !matches!(
        f,
        Array
            | ArrayLength
            | ArrayIndex
            | ArrayConcat
            | ArrayRepeat
            | ArraySlice
            | ArrayContains
            | ArrayPosition
            | ArrayRemove
            | ArrayDistinct
            | ArraySort
            | ArrayUnion
            | ArrayIntersect
            | ArrayFlatten
            | ArrayMin
            | ArrayMax
            | ArraySum
            | ArrayMean
            | ArrayAny
            | ArrayAll
            | ArrayMode
            | ArrayZip
            | Range
            | StructPack
            | StructField
            | Map
            | MapGet
            | MapContains
            | MapLength
            | MapKeys
            | MapValues
            | MapMerge
    ) {
        return Ok(None);
    }
    let a = args.iter().map(|x| c.sql(x)).collect::<Result<Vec<_>>>()?;
    let ts = args.iter().map(|x| c.dtype(x)).collect::<Result<Vec<_>>>()?;
    let out = c.dtype(&Expr::Func(f, args.to_vec()))?;
    let x = a.first().cloned().unwrap_or_default();
    let item = ts.first().and_then(|t| t.item()).cloned().unwrap_or(DataType::Null);
    let open = |e: &Expr| matches!(e, Expr::Literal(Value::Null, _));
    let element = |i: usize| -> Result<String> {
        if item == DataType::Null { Ok(a[i].clone()) } else { typed(d, a[i].clone(), &item) }
    };
    let key = |i: usize| -> Result<String> {
        match &ts[0] {
            DataType::Map(k, _) if **k != DataType::Null => typed(d, a[i].clone(), k),
            _ => Ok(a[i].clone()),
        }
    };
    Ok(Some(match f {
        Array => typed(d, format!("ARRAY({})", a.join(", ")), &out)?,
        ArrayLength | MapLength => size(&x),
        // x[i] counts from 0 and raises out of range under ANSI: only reached in range
        ArrayIndex => {
            let position = match ExprCompiler::lit_int(&args[1]) {
                Some(i) if i >= 0 => i.to_string(),
                Some(i) => format!("(SIZE({x}) + {i})"),
                None => format!("IF({i} >= 0, {i}, SIZE({x}) + {i})", i = a[1]),
            };
            format!("CASE WHEN {position} >= 0 AND {position} < SIZE({x}) THEN {x}[{position}] END")
        }
        ArrayConcat => {
            let cast = a.iter().map(|arr| typed(d, arr.clone(), &out)).collect::<Result<Vec<_>>>()?;
            format!("CONCAT({})", cast.join(", "))
        }
        ArrayRepeat => format!("FLATTEN(ARRAY_REPEAT({x}, {}))", a[1]),
        ArraySlice => {
            let n = format!("SIZE({x})");
            let to_position = |e: &Expr, sql: &str| match ExprCompiler::lit_int(e) {
                Some(i) if i >= 0 => i.to_string(),
                Some(i) => format!("GREATEST({n} + {i}, 0)"),
                None => format!("IF({sql} < 0, GREATEST({n} + {sql}, 0), {sql})"),
            };
            let start =
                if open(&args[1]) { "0".to_string() } else { format!("LEAST({n}, {})", to_position(&args[1], &a[1])) };
            let stop = if open(&args[2]) { n.clone() } else { to_position(&args[2], &a[2]) };
            format!("SLICE({x}, {start} + 1, GREATEST({stop} - {start}, 0))")
        }
        ArrayContains => format!("IF({x} IS NULL, NULL, COALESCE(ARRAY_CONTAINS({x}, {}), FALSE))", element(1)?),
        ArrayPosition => format!("(ARRAY_POSITION({x}, {}) - 1)", element(1)?),
        ArrayRemove => format!("FILTER({x}, ibis_e -> ibis_e IS NULL OR ibis_e <> {})", element(1)?),
        ArrayDistinct => format!("ARRAY_DISTINCT({x})"),
        ArraySort => format!("ARRAY_SORT({x})"),
        ArrayUnion => format!("ARRAY_UNION({x}, {})", a[1]),
        ArrayIntersect => format!("ARRAY_INTERSECT({x}, {})", a[1]),
        ArrayFlatten => format!("FLATTEN({x})"),
        // both skip NULL elements; true is the larger boolean
        ArrayMin | ArrayAll => format!("ARRAY_MIN({x})"),
        ArrayMax | ArrayAny => format!("ARRAY_MAX({x})"),
        ArraySum | ArrayMean => {
            let state = d.type_name(if f == ArraySum { &out } else { &DataType::Float64 })?;
            let kept = present(&x);
            let sum = format!(
                "AGGREGATE({kept}, CAST(NULL AS {state}), (ibis_s, ibis_e) -> CAST(COALESCE(ibis_s, 0) + ibis_e AS \
                 {state}))"
            );
            if f == ArraySum { sum } else { format!("({sum} / SIZE({kept}))") }
        }
        ArrayMode => return unsupported("modes"),
        ArrayZip => {
            let any_null: Vec<String> = a.iter().map(|arr| format!("{arr} IS NULL")).collect();
            let zipped = typed(d, format!("ARRAYS_ZIP({})", a.join(", ")), &out)?;
            format!("CASE WHEN {} THEN NULL ELSE {zipped} END", any_null.join(" OR "))
        }
        Range => {
            let (start, stop, step) = (&a[0], &a[1], &a[2]);
            let made =
                typed(d, format!("FILTER(SEQUENCE({start}, {stop}, {step}), ibis_e -> ibis_e <> {stop})"), &out)?;
            let empty = typed(d, "ARRAY()".into(), &out)?;
            format!("CASE WHEN {step} <> 0 AND SIGN({step}) = SIGN({stop} - {start}) THEN {made} ELSE {empty} END")
        }
        StructPack => {
            let pairs: Vec<String> = a.chunks(2).map(|pair| format!("{}, {}", pair[0], pair[1])).collect();
            typed(d, format!("NAMED_STRUCT({})", pairs.join(", ")), &out)?
        }
        StructField => format!("{x}[{}]", a[1]),
        Map => typed(d, format!("MAP_FROM_ARRAYS({x}, {})", a[1]), &out)?,
        MapGet => {
            let k = key(1)?;
            let default = if open(&args[2]) { "NULL".to_string() } else { a[2].clone() };
            let found = if open(&args[2]) { format!("{x}[{k}]") } else { format!("COALESCE({x}[{k}], {default})") };
            let value = format!(
                "CASE WHEN {x} IS NULL OR {k} IS NULL THEN NULL WHEN ARRAY_CONTAINS(MAP_KEYS({x}), {k}) THEN {found} \
                 ELSE {default} END"
            );
            if out == DataType::Null { value } else { typed(d, value, &out)? }
        }
        MapContains => format!("ARRAY_CONTAINS(MAP_KEYS({x}), {})", key(1)?),
        MapKeys => format!("MAP_KEYS({x})"),
        MapValues => format!("MAP_VALUES({x})"),
        MapMerge => format!("MAP_CONCAT({x}, {})", a[1]),
        _ => return Ok(None),
    }))
}

/// `TRANSFORM(arr, (x, i) -> body)`; the position counts from 0.
pub(crate) fn array_lambda(
    c: &ExprCompiler,
    kind: Lambda,
    arg: &Expr,
    param: &str,
    index: Option<&str>,
    body: &Expr,
) -> Result<String> {
    let function = match kind {
        Lambda::Map => "TRANSFORM",
        Lambda::Filter => "FILTER",
    };
    let params = match index {
        Some(i) => format!("({param}, {i})"),
        None => param.to_string(),
    };
    Ok(format!("{function}({}, {params} -> {})", c.sql(arg)?, c.sql(body)?))
}

pub(crate) fn unnest(array: &str, alias: &str, with_offset: bool, keep_empty: bool) -> Unnest {
    let outer = if keep_empty { " OUTER" } else { "" };
    let (function, columns) = if with_offset { ("POSEXPLODE", "i, v") } else { ("EXPLODE", "v") };
    Unnest {
        join: format!("LATERAL VIEW{outer} {function}({array}) {alias} AS {columns}"),
        value: format!("{alias}.v"),
        offset: format!("{alias}.i"),
    }
}
