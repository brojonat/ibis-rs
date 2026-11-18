//! Arrays, structs and maps on Trino: `ARRAY`, `ROW`, `MAP`.

use ibis_core::{DataType, Expr, Func, Lambda, Result, Value};
use ibis_sql::{Dialect, ExprCompiler, Unnest};

use crate::dialect::{TrinoDialect, unsupported};

pub(crate) fn type_name(d: &TrinoDialect, t: &DataType) -> Result<Option<String>> {
    Ok(match t {
        DataType::Array(item) => Some(format!("ARRAY({})", d.type_name(item)?)),
        DataType::Struct(fields) => {
            let fields = fields
                .iter()
                .map(|(name, t)| Ok(format!("{} {}", d.quote_ident(name), d.type_name(t)?)))
                .collect::<Result<Vec<_>>>()?;
            Some(format!("ROW({})", fields.join(", ")))
        }
        DataType::Map(key, value) => Some(format!("MAP({}, {})", d.type_name(key)?, d.type_name(value)?)),
        _ => None,
    })
}

/// The comma-separated parts of a type's argument list, not splitting inside
/// parentheses or quotes.
fn top_level(args: &str) -> Vec<&str> {
    let (mut parts, mut depth, mut quoted, mut start) = (vec![], 0, false, 0);
    for (i, ch) in args.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
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

/// `array(bigint)`, `map(varchar, bigint)` or `row(a double, "b c" varchar)`
/// as an Ibis type; `None` for any other type name. `scalar` reads the rest.
pub(crate) fn parse_type(name: &str, scalar: &dyn Fn(&str) -> DataType) -> Option<DataType> {
    let lower = name.to_ascii_lowercase();
    let inner = |prefix: &str| lower.starts_with(prefix).then(|| name[prefix.len()..].strip_suffix(')')).flatten();
    let parse = |t: &str| parse_type(t, scalar).unwrap_or_else(|| scalar(t));
    if let Some(item) = inner("array(") {
        return Some(DataType::array(parse(item)));
    }
    if let Some(kv) = inner("map(")
        && let [key, value] = top_level(kv)[..]
    {
        return Some(DataType::map(parse(key), parse(value)));
    }
    let fields = inner("row(")?;
    Some(DataType::Struct(
        top_level(fields)
            .into_iter()
            .enumerate()
            .map(|(i, field)| {
                let named = match field.strip_prefix('"') {
                    Some(rest) => rest.split_once('"').map(|(n, t)| (n.replace("\"\"", "\""), t.trim())),
                    None => {
                        field.split_once(' ').filter(|(n, _)| !n.contains('(')).map(|(n, t)| (n.to_string(), t.trim()))
                    }
                };
                match named {
                    Some((name, t)) => (name, parse(t)),
                    None => (format!("f{}", i + 1), parse(field)),
                }
            })
            .collect(),
    ))
}

fn typed(d: &TrinoDialect, x: String, t: &DataType) -> Result<String> {
    Ok(format!("CAST({x} AS {})", d.type_name(t)?))
}

pub(crate) fn literal(d: &TrinoDialect, c: &ExprCompiler, v: &Value, t: &DataType) -> Result<Option<String>> {
    let parts = c.literal_parts(v, t)?;
    let body = match v {
        Value::Array(_) => format!("ARRAY[{}]", parts.join(", ")),
        Value::Struct(_) => format!("ROW({})", parts.join(", ")),
        Value::Map(entries) => {
            let (keys, values) = parts.split_at(entries.len());
            format!("MAP(ARRAY[{}], ARRAY[{}])", keys.join(", "), values.join(", "))
        }
        _ => return Ok(None),
    };
    typed(d, body, t).map(Some)
}

/// The elements of `x` that are not NULL: what a reduction sees.
fn present(x: &str) -> String {
    format!("FILTER({x}, ibis_e -> ibis_e IS NOT NULL)")
}

/// The positions of the array `x`, from 0. `SEQUENCE(0, -1)` would count
/// down, so one position too many is made and cut off.
fn positions(x: &str) -> String {
    format!("SLICE(SEQUENCE(0, CARDINALITY({x})), 1, CARDINALITY({x}))")
}

pub(crate) fn func(d: &TrinoDialect, c: &ExprCompiler, f: Func, args: &[Expr]) -> Result<Option<String>> {
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
    // a value to compare with the elements, as their type
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
        Array => typed(d, format!("ARRAY[{}]", a.join(", ")), &out)?,
        ArrayLength | MapLength => format!("CARDINALITY({x})"),
        // ELEMENT_AT counts from 1, or from the end below zero, and is NULL out of range
        ArrayIndex => match ExprCompiler::lit_int(&args[1]) {
            Some(i) if i >= 0 => format!("ELEMENT_AT({x}, {})", i + 1),
            Some(i) => format!("ELEMENT_AT({x}, {i})"),
            None => format!("ELEMENT_AT({x}, IF({i} >= 0, {i} + 1, {i}))", i = a[1]),
        },
        ArrayConcat => {
            let cast = a.iter().map(|arr| typed(d, arr.clone(), &out)).collect::<Result<Vec<_>>>()?;
            format!("CONCAT({})", cast.join(", "))
        }
        ArrayRepeat => format!("FLATTEN(REPEAT({x}, {}))", a[1]),
        ArraySlice => {
            let n = format!("CARDINALITY({x})");
            let to_position = |e: &Expr, sql: &str| match ExprCompiler::lit_int(e) {
                Some(i) if i >= 0 => i.to_string(),
                Some(i) => format!("({n} + GREATEST({i}, -{n}))"),
                None => format!("IF({sql} < 0, {n} + GREATEST({sql}, -{n}), {sql})"),
            };
            let start =
                if open(&args[1]) { "0".to_string() } else { format!("LEAST({n}, {})", to_position(&args[1], &a[1])) };
            let stop = if open(&args[2]) { n.clone() } else { to_position(&args[2], &a[2]) };
            format!("SLICE({x}, {start} + 1, GREATEST({stop} - {start}, 0))")
        }
        ArrayContains => format!("IF({x} IS NOT NULL, COALESCE(CONTAINS({x}, {}), FALSE), NULL)", element(1)?),
        ArrayPosition => format!("(ARRAY_POSITION({x}, {}) - 1)", element(1)?),
        ArrayRemove => format!("FILTER({x}, ibis_e -> ibis_e IS NULL OR ibis_e <> {})", element(1)?),
        ArrayDistinct => format!("ARRAY_DISTINCT({x})"),
        ArraySort => format!("ARRAY_SORT({x})"),
        ArrayUnion => format!("ARRAY_UNION({x}, {})", a[1]),
        ArrayIntersect => format!("ARRAY_INTERSECT({x}, {})", a[1]),
        ArrayFlatten => format!("FLATTEN({x})"),
        ArrayMin => format!("ARRAY_MIN({})", present(&x)),
        ArrayMax => format!("ARRAY_MAX({})", present(&x)),
        ArraySum | ArrayMean => {
            let state = d.type_name(if f == ArraySum { &out } else { &DataType::Float64 })?;
            let kept = present(&x);
            let sum = format!(
                "REDUCE({kept}, CAST(NULL AS {state}), (ibis_s, ibis_e) -> COALESCE(ibis_s, 0) + ibis_e, ibis_s -> ibis_s)"
            );
            if f == ArraySum { sum } else { format!("({sum} / CARDINALITY({kept}))") }
        }
        ArrayAny => format!("ANY_MATCH(NULLIF({}, ARRAY[]), ibis_e -> ibis_e)", present(&x)),
        ArrayAll => format!("ALL_MATCH(NULLIF({}, ARRAY[]), ibis_e -> ibis_e)", present(&x)),
        ArrayMode => return unsupported("modes"),
        ArrayZip => {
            // ZIP takes five arrays at most; this takes any number
            let lens: Vec<String> = a.iter().map(|arr| format!("CARDINALITY({arr})")).collect();
            let longest = if lens.len() == 1 { lens[0].clone() } else { format!("GREATEST({})", lens.join(", ")) };
            let fields: Vec<String> = a.iter().map(|arr| format!("ELEMENT_AT({arr}, ibis_n + 1)")).collect();
            let row = typed(d, format!("ROW({})", fields.join(", ")), out.item().unwrap_or(&DataType::Null))?;
            let any_null: Vec<String> = a.iter().map(|arr| format!("{arr} IS NULL")).collect();
            format!(
                "IF({}, NULL, TRANSFORM(SLICE(SEQUENCE(0, {longest}), 1, {longest}), ibis_n -> {row}))",
                any_null.join(" OR ")
            )
        }
        Range => {
            let (start, stop, step) = (&a[0], &a[1], &a[2]);
            let made = typed(d, format!("ARRAY_REMOVE(SEQUENCE({start}, {stop}, {step}), {stop})"), &out)?;
            let empty = typed(d, "ARRAY[]".into(), &out)?;
            format!("IF({step} <> 0 AND SIGN({step}) = SIGN({stop} - {start}), {made}, {empty})")
        }
        StructPack => {
            let values: Vec<&str> = a.chunks(2).map(|pair| pair[1].as_str()).collect();
            typed(d, format!("ROW({})", values.join(", ")), &out)?
        }
        StructField => {
            let name = ExprCompiler::lit_str(&args[1]).unwrap_or_default();
            format!("({x}).{}", d.quote_ident(name))
        }
        Map => typed(d, format!("MAP({x}, {})", a[1]), &out)?,
        MapGet => {
            let found = format!("ELEMENT_AT({x}, {})", key(1)?);
            let value = if open(&args[2]) { found } else { format!("COALESCE({found}, {})", a[2]) };
            if out == DataType::Null { value } else { typed(d, value, &out)? }
        }
        MapContains => format!("CONTAINS(MAP_KEYS({x}), {})", key(1)?),
        MapKeys => format!("MAP_KEYS({x})"),
        MapValues => format!("MAP_VALUES({x})"),
        MapMerge => format!("MAP_CONCAT({x}, {})", a[1]),
        _ => return Ok(None),
    }))
}

/// `TRANSFORM` and `FILTER` take the element alone. With its position, the
/// array is zipped with its positions; a filter then carries the element
/// along with the verdict, and keeps the elements whose verdict is true.
pub(crate) fn array_lambda(
    d: &TrinoDialect,
    c: &ExprCompiler,
    kind: Lambda,
    arg: &Expr,
    param: &str,
    index: Option<&str>,
    body: &Expr,
) -> Result<String> {
    let x = c.sql(arg)?;
    let body = c.sql(body)?;
    Ok(match (kind, index) {
        (Lambda::Map, None) => format!("TRANSFORM({x}, {param} -> {body})"),
        (Lambda::Filter, None) => format!("FILTER({x}, {param} -> {body})"),
        (Lambda::Map, Some(i)) => format!("ZIP_WITH({x}, {}, ({param}, {i}) -> {body})", positions(&x)),
        (Lambda::Filter, Some(i)) => {
            let item = c.dtype(arg)?.item().cloned().unwrap_or(DataType::Null);
            let verdict = format!("CAST(ROW({body}, {param}) AS ROW(keep BOOLEAN, value {}))", d.type_name(&item)?);
            format!(
                "TRANSFORM(FILTER(ZIP_WITH({x}, {}, ({param}, {i}) -> {verdict}), ibis_r -> ibis_r.keep), ibis_r -> \
                 ibis_r.value)",
                positions(&x)
            )
        }
    })
}

/// `UNNEST` spreads the fields of a row over columns of their own, so an
/// array of rows is unnested as an array of one-element arrays.
pub(crate) fn unnest(array: &str, item: &DataType, alias: &str, with_offset: bool, keep_empty: bool) -> Unnest {
    let rows = matches!(item, DataType::Struct(_));
    let source = if rows { format!("TRANSFORM({array}, ibis_e -> ARRAY[ibis_e])") } else { array.to_string() };
    let (ordinality, columns) = if with_offset { (" WITH ORDINALITY", "(v, i)") } else { ("", "(v)") };
    let table = format!("UNNEST({source}){ordinality} AS {alias}{columns}");
    Unnest {
        join: if keep_empty { format!("LEFT JOIN {table} ON TRUE") } else { format!("CROSS JOIN {table}") },
        value: if rows { format!("{alias}.v[1]") } else { format!("{alias}.v") },
        offset: format!("{alias}.i - 1"),
    }
}
