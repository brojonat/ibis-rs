//! Arrays, structs and maps on Postgres.
//!
//! An array is a Postgres array. Postgres has no anonymous struct or map
//! type, so both are `JSONB`: a struct is an object of its fields, a map an
//! object whose keys are the text of the map's keys. Values come out of the
//! JSON through a cast to the type Ibis knows they have. Arrays of arrays
//! are not supported: a Postgres multidimensional array is rectangular.

use ibis_core::{DataType, Error, Expr, Func, Lambda, Result, Value};
use ibis_sql::json::json_key;
use ibis_sql::{Dialect, ExprCompiler, quote_str};

use crate::PostgresDialect;

fn unsupported<T>(op: impl Into<String>) -> Result<T> {
    Err(Error::Unsupported { backend: "postgres".into(), op: op.into() })
}

fn is_json(t: &DataType) -> bool {
    matches!(t, DataType::Struct(_) | DataType::Map(..))
}

pub(crate) fn type_name(d: &PostgresDialect, t: &DataType) -> Result<Option<String>> {
    Ok(match t {
        DataType::Array(item) if matches!(**item, DataType::Array(_)) => {
            return unsupported(format!("the type {t} (a multidimensional array is not an array of arrays)"));
        }
        // an untyped empty array
        DataType::Array(item) if **item == DataType::Null => Some("TEXT[]".into()),
        DataType::Array(item) => Some(format!("{}[]", d.type_name(item)?)),
        DataType::Struct(_) | DataType::Map(..) => Some("JSONB".into()),
        _ => None,
    })
}

/// The value of type `t` held in the JSONB expression `j`.
fn from_jsonb(d: &PostgresDialect, j: &str, t: &DataType) -> Result<String> {
    Ok(match t {
        DataType::Struct(_) | DataType::Map(..) => j.to_string(),
        DataType::Array(item) => format!(
            "CASE WHEN JSONB_TYPEOF({j}) = 'array' THEN ARRAY(SELECT {} FROM JSONB_ARRAY_ELEMENTS({j}) WITH \
             ORDINALITY AS _e(_v, _n) ORDER BY _e._n) END",
            from_jsonb(d, "_e._v", item)?
        ),
        DataType::Null => format!("({j} #>> '{{}}')"),
        scalar => format!("CAST({j} #>> '{{}}' AS {})", d.type_name(scalar)?),
    })
}

/// A key to look up in a map of type `map`, as the text it has in the JSON
/// object: the text of a value of the map's key type, which `1.0` written
/// as a literal is not.
fn key_text(c: &ExprCompiler, map: &DataType, key: &Expr) -> Result<String> {
    let k = c.sql(key)?;
    Ok(match map {
        _ if c.dtype(key)? == DataType::String => k,
        DataType::Map(kt, _) if **kt != DataType::Null => format!("CAST(CAST({k} AS {}) AS TEXT)", c.d.type_name(kt)?),
        _ => format!("CAST({k} AS TEXT)"),
    })
}

pub(crate) fn literal(c: &ExprCompiler, v: &Value, t: &DataType) -> Result<Option<String>> {
    Ok(match v {
        Value::Array(_) => Some(format!("CAST(ARRAY[{}] AS {})", c.literal_parts(v, t)?.join(", "), c.d.type_name(t)?)),
        Value::Struct(fields) => {
            let parts = c.literal_parts(v, t)?;
            let pairs: Vec<String> =
                fields.iter().zip(parts).map(|((name, _), v)| format!("{}, {v}", quote_str(name))).collect();
            Some(format!("JSONB_BUILD_OBJECT({})", pairs.join(", ")))
        }
        Value::Map(entries) if entries.is_empty() => Some("CAST('{}' AS JSONB)".into()),
        Value::Map(entries) => {
            let parts = c.literal_parts(v, t)?;
            let values = &parts[entries.len()..];
            let pairs: Vec<String> =
                entries.iter().zip(values).map(|((k, _), v)| format!("{}, {v}", quote_str(&json_key(k)))).collect();
            Some(format!("JSONB_BUILD_OBJECT({})", pairs.join(", ")))
        }
        _ => None,
    })
}

/// `CASE WHEN a IS NULL OR ... THEN NULL ELSE body END`
fn guard(args: &[&str], body: String) -> String {
    let any: Vec<String> = args.iter().map(|a| format!("{a} IS NULL")).collect();
    format!("CASE WHEN {} THEN NULL ELSE {body} END", any.join(" OR "))
}

/// A reduction over the elements of the array `x`.
fn reduce(x: &str, call: &str) -> String {
    format!("(SELECT {call} FROM UNNEST({x}) AS _t(_e))")
}

pub(crate) fn func(d: &PostgresDialect, c: &ExprCompiler, f: Func, args: &[Expr]) -> Result<Option<String>> {
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
    let position = |e: &Expr, sql: &str| match ExprCompiler::lit_int(e) {
        Some(i) if i >= 0 => (i + 1).to_string(),
        Some(i) => format!("CARDINALITY({x}) + {}", i + 1),
        None => format!("CASE WHEN {sql} < 0 THEN CARDINALITY({x}) + {sql} + 1 ELSE {sql} + 1 END"),
    };
    Ok(Some(match f {
        Array => format!("CAST(ARRAY[{}] AS {})", a.join(", "), d.type_name(&out)?),
        ArrayLength => format!("CAST(CARDINALITY({x}) AS BIGINT)"),
        ArrayIndex => format!("({x})[{}]", position(&args[1], &a[1])),
        ArrayConcat => {
            let t = d.type_name(&out)?;
            let cast: Vec<String> = a.iter().map(|arr| format!("CAST({arr} AS {t})")).collect();
            let joined = cast.iter().skip(1).fold(cast[0].clone(), |acc, y| format!("ARRAY_CAT({acc}, {y})"));
            guard(&a.iter().map(String::as_str).collect::<Vec<_>>(), joined)
        }
        ArrayRepeat => guard(
            &[&x],
            format!(
                "ARRAY(SELECT ({x})[_i % CARDINALITY({x}) + 1] FROM GENERATE_SERIES(0, CARDINALITY({x}) * {} - 1) AS \
                 _i)",
                a[1]
            ),
        ),
        ArraySlice => {
            let n = format!("CARDINALITY({x})");
            let to_position = |e: &Expr, sql: &str| match ExprCompiler::lit_int(e) {
                Some(i) if i >= 0 => i.to_string(),
                Some(i) => format!("GREATEST({n} + {i}, 0)"),
                None => format!("CASE WHEN {sql} < 0 THEN GREATEST({n} + {sql}, 0) ELSE {sql} END"),
            };
            let start = if open(&args[1]) { "0".to_string() } else { to_position(&args[1], &a[1]) };
            let stop = if open(&args[2]) { n.clone() } else { to_position(&args[2], &a[2]) };
            format!("({x})[LEAST({n}, {start}) + 1:{stop}]")
        }
        ArrayContains => format!("({x} @> ARRAY[CAST({} AS {})])", a[1], d.type_name(&item)?),
        ArrayPosition => guard(
            &[&x, &a[1]],
            format!("CAST(COALESCE(ARRAY_POSITION({x}, CAST({} AS {})), 0) - 1 AS BIGINT)", a[1], d.type_name(&item)?),
        ),
        ArrayRemove => format!("ARRAY_REMOVE({x}, CAST({} AS {}))", a[1], d.type_name(&item)?),
        ArrayDistinct => guard(&[&x], format!("ARRAY(SELECT DISTINCT UNNEST({x}))")),
        ArraySort => guard(&[&x], format!("ARRAY(SELECT _e FROM UNNEST({x}) AS _t(_e) ORDER BY _e NULLS LAST)")),
        ArrayUnion => guard(&[&x, &a[1]], format!("ARRAY(SELECT UNNEST({x}) UNION SELECT UNNEST({}))", a[1])),
        ArrayIntersect => guard(&[&x, &a[1]], format!("ARRAY(SELECT UNNEST({x}) INTERSECT SELECT UNNEST({}))", a[1])),
        ArrayFlatten => return unsupported("flatten (no arrays of arrays)"),
        ArrayMin => reduce(&x, "MIN(_e)"),
        ArrayMax => reduce(&x, "MAX(_e)"),
        ArraySum => format!("CAST({} AS {})", reduce(&x, "SUM(_e)"), d.type_name(&out)?),
        ArrayMean => format!("CAST({} AS {})", reduce(&x, "AVG(_e)"), d.type_name(&out)?),
        ArrayAny => reduce(&x, "BOOL_OR(_e)"),
        ArrayAll => reduce(&x, "BOOL_AND(_e)"),
        ArrayMode => reduce(&x, "MODE() WITHIN GROUP (ORDER BY _e)"),
        ArrayZip => {
            let lens: Vec<String> = a.iter().map(|arr| format!("CARDINALITY({arr})")).collect();
            let longest = if lens.len() == 1 { lens[0].clone() } else { format!("GREATEST({})", lens.join(", ")) };
            let fields: Vec<String> =
                a.iter().enumerate().map(|(i, arr)| format!("'f{}', ({arr})[_i]", i + 1)).collect();
            guard(
                &a.iter().map(String::as_str).collect::<Vec<_>>(),
                format!(
                    "ARRAY(SELECT JSONB_BUILD_OBJECT({}) FROM GENERATE_SERIES(1, {longest}) AS _i)",
                    fields.join(", ")
                ),
            )
        }
        Range => {
            let t = d.type_name(&out)?;
            let (start, stop, step) = (&a[0], &a[1], &a[2]);
            format!(
                "CASE WHEN {step} <> 0 AND SIGN({step}) = SIGN({stop} - {start}) THEN CAST(ARRAY_REMOVE(ARRAY(SELECT * \
                 FROM GENERATE_SERIES({start}, {stop}, {step})), {stop}) AS {t}) ELSE CAST(ARRAY[] AS {t}) END"
            )
        }
        StructPack => {
            let pairs: Vec<String> = a.chunks(2).map(|pair| format!("{}, {}", pair[0], pair[1])).collect();
            format!("JSONB_BUILD_OBJECT({})", pairs.join(", "))
        }
        StructField => from_jsonb(d, &format!("({x} -> {})", a[1]), &out)?,
        Map => guard(
            &[&x, &a[1]],
            format!(
                "COALESCE((SELECT JSONB_OBJECT_AGG(CAST(_kv._k AS TEXT), _kv._v) FROM UNNEST({x}, {}) AS _kv(_k, _v)), \
                 CAST('{{}}' AS JSONB))",
                a[1]
            ),
        ),
        MapGet => {
            let key = key_text(c, &ts[0], &args[1])?;
            let value_type = match &ts[0] {
                DataType::Map(_, v) => (**v).clone(),
                _ => DataType::Null,
            };
            let found = match value_type {
                DataType::Null => "NULL".to_string(),
                ref v => from_jsonb(d, &format!("({x} -> {key})"), v)?,
            };
            let value = match (open(&args[2]), found.as_str()) {
                (true, _) => found,
                (false, "NULL") => a[2].clone(),
                (false, _) => format!("COALESCE({found}, {})", a[2]),
            };
            let value = if is_json(&out) || out == DataType::Null {
                value
            } else {
                format!("CAST({value} AS {})", d.type_name(&out)?)
            };
            guard(&[&x, &key], value)
        }
        MapContains => format!("JSONB_EXISTS({x}, {})", key_text(c, &ts[0], &args[1])?),
        MapLength => guard(&[&x], format!("(SELECT COUNT(*) FROM JSONB_OBJECT_KEYS({x}))")),
        MapKeys => {
            let key = match out.item() {
                Some(DataType::String) | None => "_kv._k".to_string(),
                Some(k) => format!("CAST(_kv._k AS {})", d.type_name(k)?),
            };
            guard(&[&x], format!("ARRAY(SELECT {key} FROM JSONB_EACH({x}) AS _kv(_k, _v))"))
        }
        MapValues => {
            let value = from_jsonb(d, "_kv._v", out.item().unwrap_or(&DataType::Null))?;
            guard(&[&x], format!("ARRAY(SELECT {value} FROM JSONB_EACH({x}) AS _kv(_k, _v))"))
        }
        MapMerge => format!("({x} || {})", a[1]),
        _ => return Ok(None),
    }))
}

/// No lambdas: a subquery over the unnested array, with the parameters as
/// its columns. `WITH ORDINALITY` counts from 1.
pub(crate) fn array_lambda(
    c: &ExprCompiler,
    kind: Lambda,
    arg: &Expr,
    param: &str,
    index: Option<&str>,
    body: &Expr,
) -> Result<String> {
    let x = c.sql(arg)?;
    let n = index.unwrap_or("ibis_n");
    let body = match index {
        Some(i) => body.clone().replace_param(i, &(Expr::Param(i.to_string(), DataType::Int64) - 1)),
        None => body.clone(),
    };
    let body = c.sql(&body)?;
    let select = match kind {
        Lambda::Map => format!("SELECT {body} FROM UNNEST({x}) WITH ORDINALITY AS _t({param}, {n}) ORDER BY {n}"),
        Lambda::Filter => {
            format!("SELECT {param} FROM UNNEST({x}) WITH ORDINALITY AS _t({param}, {n}) WHERE {body} ORDER BY {n}")
        }
    };
    Ok(guard(&[&x], format!("ARRAY({select})")))
}
