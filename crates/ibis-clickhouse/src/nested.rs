//! Arrays, structs and maps on ClickHouse: `Array`, a named `Tuple`, `Map`.
//!
//! None of the three can be NULL as a whole: a NULL array is stored and
//! returned as an empty one, a NULL struct as one of NULL fields. Map keys
//! cannot be NULL either.

use ibis_core::{DataType, Expr, Func, Lambda, Result, Value};
use ibis_sql::{Dialect, ExprCompiler, Unnest};

use crate::dialect::{ClickHouseDialect, quote_str, unsupported};

/// `t` without the `Nullable` wrapper `type_name` puts around scalars: the
/// only form a map key takes.
fn bare(d: &ClickHouseDialect, t: &DataType) -> Result<String> {
    let name = d.type_name(t)?;
    Ok(name.strip_prefix("Nullable(").and_then(|s| s.strip_suffix(')')).map_or(name.clone(), String::from))
}

pub(crate) fn type_name(d: &ClickHouseDialect, t: &DataType) -> Result<Option<String>> {
    Ok(match t {
        DataType::Array(item) => Some(format!("Array({})", d.type_name(item)?)),
        DataType::Struct(fields) => {
            let fields = fields
                .iter()
                .map(|(name, t)| Ok(format!("{} {}", d.quote_ident(name), d.type_name(t)?)))
                .collect::<Result<Vec<_>>>()?;
            Some(format!("Tuple({})", fields.join(", ")))
        }
        DataType::Map(key, value) => Some(format!("Map({}, {})", bare(d, key)?, d.type_name(value)?)),
        _ => None,
    })
}

fn typed(d: &ClickHouseDialect, x: String, t: &DataType) -> Result<String> {
    Ok(format!("CAST({x} AS {})", d.type_name(t)?))
}

pub(crate) fn literal(d: &ClickHouseDialect, c: &ExprCompiler, v: &Value, t: &DataType) -> Result<Option<String>> {
    let parts = c.literal_parts(v, t)?;
    let body = match v {
        Value::Array(_) => format!("[{}]", parts.join(", ")),
        Value::Struct(_) => format!("tuple({})", parts.join(", ")),
        Value::Map(entries) => {
            let (keys, values) = parts.split_at(entries.len());
            let pairs: Vec<String> = keys.iter().zip(values).map(|(k, v)| format!("{k}, {v}")).collect();
            format!("map({})", pairs.join(", "))
        }
        _ => return Ok(None),
    };
    typed(d, body, t).map(Some)
}

/// The elements of `x` that are not NULL: what a reduction sees.
fn present(x: &str) -> String {
    format!("arrayFilter(_x -> _x IS NOT NULL, {x})")
}

pub(crate) fn func(d: &ClickHouseDialect, c: &ExprCompiler, f: Func, args: &[Expr]) -> Result<Option<String>> {
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
            | Unnest
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
    let open = |e: &Expr| matches!(e, Expr::Literal(Value::Null, _));
    let distinct = |x: &str| format!("arrayConcat(arrayDistinct({x}), if(countEqual({x}, NULL) > 0, [NULL], []))");
    let reduce = |agg: &str| typed(d, format!("arrayReduce('{agg}OrNull', {})", present(&x)), &out);
    // a key as the map's key type, which is never Nullable
    let key = |i: usize| -> Result<String> {
        Ok(match &ts[0] {
            DataType::Map(k, _) if **k != DataType::Null => format!("CAST({} AS {})", a[i], bare(d, k)?),
            _ => a[i].clone(),
        })
    };
    Ok(Some(match f {
        Array => typed(d, format!("[{}]", a.join(", ")), &out)?,
        ArrayLength | MapLength => typed(d, format!("length({x})"), &out)?,
        ArrayIndex => match ExprCompiler::lit_int(&args[1]) {
            Some(i) if i >= 0 => format!("{x}[{}]", i + 1),
            Some(i) => format!("{x}[{i}]"),
            None => format!("{x}[if({i} >= 0, {i} + 1, {i})]", i = a[1]),
        },
        ArrayConcat => typed(d, format!("arrayConcat({})", a.join(", ")), &out)?,
        ArrayRepeat => format!("arrayFlatten(arrayMap(_x -> {x}, range({})))", a[1]),
        ArraySlice => {
            // arraySlice(arr, offset from 1 (negative from the end), length (negative: up to that from the end))
            let (start, stop) = (ExprCompiler::lit_int(&args[1]), ExprCompiler::lit_int(&args[2]));
            let offset = match start {
                _ if open(&args[1]) => "1".to_string(),
                Some(i) if i >= 0 => (i + 1).to_string(),
                Some(i) => i.to_string(),
                None => format!("if({i} >= 0, {i} + 1, {i})", i = a[1]),
            };
            match (open(&args[2]), start, stop) {
                (true, ..) => format!("arraySlice({x}, {offset})"),
                (false, _, Some(stop)) if stop < 0 => format!("arraySlice({x}, {offset}, {stop})"),
                (false, _, Some(stop)) if open(&args[1]) => format!("arraySlice({x}, 1, {stop})"),
                (false, Some(start), Some(stop)) if start >= 0 => {
                    format!("arraySlice({x}, {offset}, {})", (stop - start).max(0))
                }
                (false, Some(start), Some(stop)) => {
                    format!("arraySlice({x}, {offset}, greatest(0, {stop} - (length({x}) + {start})))")
                }
                _ => return unsupported("an array slice with bounds that are not literals"),
            }
        }
        ArrayContains => format!("has({x}, {})", a[1]),
        ArrayPosition => typed(d, format!("indexOf({x}, {}) - 1", a[1]), &out)?,
        ArrayRemove => format!("arrayFilter(_x -> _x != {v} OR (_x IS NULL AND {v} IS NOT NULL), {x})", v = a[1]),
        ArrayDistinct => distinct(&x),
        ArraySort => format!("arraySort({x})"),
        ArrayUnion => distinct(&format!("arrayConcat({x}, {})", a[1])),
        ArrayIntersect => format!("arrayIntersect({x}, {})", a[1]),
        ArrayFlatten => format!("arrayFlatten({x})"),
        ArrayMin | ArrayAll => reduce("min")?,
        ArrayMax | ArrayAny => reduce("max")?,
        ArraySum => reduce("sum")?,
        ArrayMean => reduce("avg")?,
        ArrayMode => return unsupported("modes"),
        ArrayZip => typed(d, format!("arrayZip({})", a.join(", ")), &out)?,
        Range => {
            let (start, stop, step) = (&a[0], &a[1], &a[2]);
            // range() takes a step above zero: count down as a count up, mirrored
            let up = format!("range({start}, {stop}, {step})");
            let down = format!("arrayMap(_x -> -_x, range(-({start}), -({stop}), -({step})))");
            typed(d, format!("multiIf({step} > 0, {up}, {step} < 0, {down}, [])"), &out)?
        }
        Unnest => format!("arrayJoin({x})"),
        StructPack => {
            let values: Vec<&str> = a.chunks(2).map(|pair| pair[1].as_str()).collect();
            typed(d, format!("tuple({})", values.join(", ")), &out)?
        }
        StructField => {
            let name = ExprCompiler::lit_str(&args[1]).unwrap_or_default();
            let position = match &ts[0] {
                DataType::Struct(fields) => fields.iter().position(|(n, _)| n == name),
                _ => None,
            };
            match position {
                Some(i) => format!("tupleElement({x}, {})", i + 1),
                None => format!("tupleElement({x}, {})", quote_str(name)),
            }
        }
        Map => {
            let keys = match &out {
                DataType::Map(k, _) if **k != DataType::Null => format!("CAST({x} AS Array({}))", bare(d, k)?),
                _ => x.clone(),
            };
            typed(d, format!("mapFromArrays({keys}, {})", a[1]), &out)?
        }
        MapGet => {
            let k = key(1)?;
            let default = if open(&args[2]) { "NULL".to_string() } else { a[2].clone() };
            // `map[key]` takes only some key types; any key can be found among the keys
            let simple = matches!(&ts[0], DataType::Map(kt, _) if **kt == DataType::String || kt.is_integer());
            let found =
                if simple { format!("{x}[{k}]") } else { format!("mapValues({x})[indexOf(mapKeys({x}), {k})]") };
            let value = format!("if(mapContains({x}, {k}), {found}, {default})");
            if out == DataType::Null { value } else { typed(d, value, &out)? }
        }
        MapContains => format!("if({} IS NULL, NULL, mapContains({x}, {}))", a[1], key(1)?),
        MapKeys => typed(d, format!("mapKeys({x})"), &out)?,
        MapValues => format!("mapValues({x})"),
        MapMerge => format!("mapUpdate({x}, {})", a[1]),
        _ => return Ok(None),
    }))
}

/// `arrayMap((x, i) -> body, arr, range(0, length(arr)))`: the positions are
/// a second array, already counted from 0.
pub(crate) fn array_lambda(
    c: &ExprCompiler,
    kind: Lambda,
    arg: &Expr,
    param: &str,
    index: Option<&str>,
    body: &Expr,
) -> Result<String> {
    let x = c.sql(arg)?;
    let function = match kind {
        Lambda::Map => "arrayMap",
        Lambda::Filter => "arrayFilter",
    };
    let body = c.sql(body)?;
    Ok(match index {
        Some(i) => format!("{function}(({param}, {i}) -> {body}, {x}, range(0, length({x})))"),
        None => format!("{function}({param} -> {body}, {x})"),
    })
}

pub(crate) fn unnest(array: &str, alias: &str, with_offset: bool, keep_empty: bool) -> Unnest {
    let left = if keep_empty { "LEFT " } else { "" };
    let positions = if with_offset { format!(", arrayEnumerate({array}) AS {alias}_i") } else { String::new() };
    Unnest {
        join: format!("{left}ARRAY JOIN {array} AS {alias}_v{positions}"),
        value: format!("{alias}_v"),
        offset: format!("{alias}_i - 1"),
    }
}
