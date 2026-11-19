//! Arrays, structs and maps.
//!
//! An array is a Polars list, a struct a struct, and a map (Polars has no
//! map type) a list of `{key, value}` structs.
//!
//! What Polars has an expression for is translated to it. The rest (zipping
//! arrays, looking a key up in a map, ...) is computed row by row in Rust
//! ([`rowwise`]): slower than a Polars kernel, but it works on columns, where
//! `list.eval` cannot see anything but the list.

use ibis_core::{DataType, Expr, Func, Lambda, Result, Value};
use polars::prelude as pl;

use crate::expr::{Tx, lit_int, lit_str, literal, null};
use crate::{pl_err, series, to_pl, unsupported, value};

/// `f` applied to the argument values of each row.
fn rowwise(
    args: &[pl::Expr],
    types: Vec<DataType>,
    out: DataType,
    f: impl Fn(&[Value]) -> Value + Send + Sync + 'static,
) -> pl::Expr {
    let pl_out = to_pl(&out);
    let run = move |cols: &mut [pl::Column]| -> Result<pl::Column> {
        // a literal arrives as one value, whatever the height of the frame
        let len = cols.iter().map(|c| c.len()).find(|n| *n != 1).unwrap_or(1);
        let vals = cols
            .iter()
            .zip(&types)
            .map(|(c, t)| c.as_materialized_series().iter().map(|av| value(av, t)).collect::<Result<Vec<_>>>())
            .collect::<Result<Vec<_>>>()?;
        let mut row = vec![Value::Null; cols.len()];
        let rows: Vec<Value> = (0..len)
            .map(|i| {
                for (cell, col) in row.iter_mut().zip(&vals) {
                    *cell = col[if col.len() == 1 { 0 } else { i }].clone();
                }
                f(&row)
            })
            .collect();
        Ok(series(cols[0].name(), &out, &rows)?.into())
    };
    args[0].clone().map_many(
        move |cols| run(cols).map_err(|e| pl::PolarsError::ComputeError(e.to_string().into())),
        &args[1..],
        move |_, fields| Ok(pl::Field::new(fields[0].name().clone(), pl_out.clone())),
    )
}

/// Equality of two elements; an integer equals the float of the same value.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(i), Value::Float(f)) | (Value::Float(f), Value::Int(i)) => *i as f64 == *f,
        _ => a == b,
    }
}

/// The elements of `items` without repeats; NULL counts as a value.
fn distinct(items: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = vec![];
    for v in items {
        if !out.iter().any(|seen| same(seen, v)) {
            out.push(v.clone());
        }
    }
    out
}

/// One nested function on the values of a row, as DuckDB answers it.
fn eval(f: Func, a: &[Value]) -> Value {
    use Value::{Array, Bool, Int, Map, Null, Struct};
    match (f, a) {
        (Func::Array, items) => Array(items.to_vec()),
        (Func::ArrayRepeat, [Array(items), Int(n)]) => {
            Array((0..(*n).max(0)).flat_map(|_| items.iter().cloned()).collect())
        }
        (Func::ArrayPosition, [Array(items), v]) if !v.is_null() => {
            Int(items.iter().position(|e| same(e, v)).map_or(-1, |i| i as i64))
        }
        // nothing equals NULL, and DuckDB's filter keeps what it knows differs
        (Func::ArrayRemove, [Array(_), Null]) => Array(vec![]),
        (Func::ArrayRemove, [Array(items), v]) => Array(items.iter().filter(|e| !same(e, v)).cloned().collect()),
        (Func::ArrayDistinct, [Array(items)]) => Array(distinct(items)),
        (Func::ArrayUnion, [Array(l), Array(r)]) => Array(distinct(&l.iter().chain(r).cloned().collect::<Vec<_>>())),
        (Func::ArrayIntersect, [Array(l), Array(r)]) => {
            Array(l.iter().filter(|e| !e.is_null() && r.iter().any(|o| same(o, e))).cloned().collect())
        }
        (Func::ArrayIntersect, [Array(_), Null]) => Array(vec![]),
        (Func::ArrayFlatten, [Array(lists)]) => Array(
            lists
                .iter()
                .flat_map(|l| match l {
                    Array(items) => items.clone(),
                    _ => vec![],
                })
                .collect(),
        ),
        (Func::ArrayMode, [Array(items)]) => {
            let present: Vec<&Value> = items.iter().filter(|v| !v.is_null()).collect();
            let count = |v: &Value| present.iter().filter(|o| same(o, v)).count();
            // the first of the most frequent
            present.iter().rev().max_by_key(|v| count(v)).map_or(Null, |v| (*v).clone())
        }
        (Func::ArrayZip, arrays) if arrays.iter().all(|v| matches!(v, Array(_))) => {
            let items = |v: &Value| match v {
                Array(items) => items.clone(),
                _ => vec![],
            };
            let arrays: Vec<Vec<Value>> = arrays.iter().map(items).collect();
            let len = arrays.iter().map(Vec::len).max().unwrap_or(0);
            let row = |i: usize| {
                let field =
                    |(n, arr): (usize, &Vec<Value>)| (format!("f{}", n + 1), arr.get(i).cloned().unwrap_or(Null));
                Struct(arrays.iter().enumerate().map(field).collect())
            };
            Array((0..len).map(row).collect())
        }
        (Func::Range, [Int(start), Int(stop), Int(step)]) => {
            let mut out = vec![];
            let mut i = *start;
            while *step != 0 && (if *step > 0 { i < *stop } else { i > *stop }) {
                out.push(Int(i));
                i += step;
            }
            Array(out)
        }
        (Func::Map, [Array(keys), Array(values)]) => Map(keys.iter().cloned().zip(values.iter().cloned()).collect()),
        (Func::MapGet, [Map(entries), key, default]) => {
            match entries.iter().find(|(k, _)| !key.is_null() && same(k, key)) {
                Some((_, v)) if !v.is_null() => v.clone(),
                _ => default.clone(),
            }
        }
        (Func::MapContains, [Map(entries), key]) if !key.is_null() => Bool(entries.iter().any(|(k, _)| same(k, key))),
        (Func::MapMerge, [Map(left), Map(right)]) => {
            let mut out = left.clone();
            for (k, v) in right {
                match out.iter_mut().find(|(seen, _)| same(seen, k)) {
                    Some(entry) => entry.1 = v.clone(),
                    None => out.push((k.clone(), v.clone())),
                }
            }
            Map(out)
        }
        // a NULL array, map or argument
        _ => Null,
    }
}

impl Tx<'_> {
    pub(crate) fn nested(&self, e: &Expr, f: Func, args: &[Expr], a: &[pl::Expr]) -> Result<pl::Expr> {
        let out = self.dtype(e)?;
        let t = to_pl(&out);
        let x = a.first().cloned().unwrap_or_else(null);
        let by_row = || -> Result<pl::Expr> {
            let types = args.iter().map(|a| self.dtype(a)).collect::<Result<Vec<_>>>()?;
            Ok(rowwise(a, types, out.clone(), move |row| eval(f, row)))
        };
        let field = |name: &str| pl::element().struct_().field_by_name(name);
        let int = |e: pl::Expr| e.cast(pl::DataType::Int64);
        use Func::*;
        Ok(match f {
            Array if a.is_empty() => literal(&Value::Array(vec![]), &out)?,
            // `concat_list` would splice arrays into one another
            Array if matches!(out.item(), Some(DataType::Array(_) | DataType::Map(..))) => by_row()?,
            Array => {
                let item = to_pl(out.item().unwrap_or(&DataType::Null));
                let items: Vec<pl::Expr> = a.iter().map(|e| e.clone().cast(item.clone())).collect();
                pl::concat_list(items).map_err(pl_err)?
            }
            ArrayLength | MapLength => x.list().len().cast(t),
            ArrayIndex => x.list().get(int(a[1].clone()), true),
            ArrayConcat => {
                let lists: Vec<pl::Expr> = a.iter().map(|e| e.clone().cast(t.clone())).collect();
                pl::concat_list(lists).map_err(pl_err)?
            }
            ArraySlice => {
                let len = || int(x.clone().list().len());
                // a position counted from the end, then clamped to the array
                let position = |i: usize| {
                    if matches!(&args[i], Expr::Literal(Value::Null, _)) {
                        return if i == 1 { pl::lit(0i64) } else { len() };
                    }
                    let p = int(a[i].clone());
                    let p = pl::when(p.clone().lt(pl::lit(0))).then(len() + p.clone()).otherwise(p);
                    p.clip(pl::lit(0i64), len())
                };
                let (start, stop) = (position(1), position(2));
                let n = pl::when(stop.clone().gt(start.clone())).then(stop - start.clone()).otherwise(pl::lit(0i64));
                x.list().slice(start, n)
            }
            ArrayContains => x.list().contains(a[1].clone(), false),
            ArraySort => x.list().sort(pl::SortOptions::default().with_nulls_last(true)),
            ArrayMin => x.list().min().cast(t),
            ArrayMax => x.list().max().cast(t),
            ArrayMean => x.list().mean().cast(t),
            ArraySum | ArrayAny | ArrayAll => {
                // SQL: nothing to reduce is NULL, where Polars sums to 0
                let present = x.list().drop_nulls();
                let reduced = match f {
                    ArraySum => present.clone().list().sum(),
                    ArrayAny => present.clone().list().max(),
                    _ => present.clone().list().min(),
                };
                pl::when(present.list().len().eq(pl::lit(0))).then(null()).otherwise(reduced).cast(t)
            }
            Range => match lit_int(&args[2]) {
                Some(0) | None => by_row()?,
                Some(step) => pl::int_ranges(int(x), int(a[1].clone()), pl::lit(step), pl::DataType::Int64).cast(t),
            },
            StructPack => {
                let fields = args.chunks(2).zip(a.chunks(2)).map(|(pair, compiled)| match lit_str(&pair[0]) {
                    Some(name) => Ok(compiled[1].clone().alias(name)),
                    None => unsupported("a struct whose field names are not literals"),
                });
                pl::as_struct(fields.collect::<Result<Vec<_>>>()?).cast(t)
            }
            StructField => match lit_str(&args[1]) {
                Some(name) => x.struct_().field_by_name(name),
                None => return unsupported("a struct field that is not named by a literal"),
            },
            MapKeys => x.list().eval(field("key")),
            MapValues => x.list().eval(field("value")),
            Unnest => return unsupported("unnest inside another expression"),
            _ => by_row()?,
        })
    }

    /// `arr.map(...)` and `arr.filter(...)`: the body runs inside the list,
    /// where Polars lets it see the element and nothing else.
    pub(crate) fn lambda(&self, e: &Expr) -> Result<pl::Expr> {
        let Expr::ArrayLambda { kind, arg, param, index, .. } = e else {
            return unsupported(format!("{e} as a lambda"));
        };
        let body = e.lambda_body(self.schema)?;
        if body.has(&|e| matches!(e, Expr::Column(_) | Expr::Field { .. })) {
            return unsupported("array map and filter with a function that refers to a column");
        }
        let inside = Tx { lambda: Some((param.as_str(), index.as_deref())), ..*self };
        let result = inside.expr(&body)?;
        let array = self.expr(arg)?;
        Ok(match kind {
            Lambda::Map => array.list().eval(result.cast(to_pl(&body.dtype(self.schema)?))),
            Lambda::Filter => array.list().eval(pl::element().filter(result)),
        })
    }

    /// A lambda parameter, inside `list.eval`.
    pub(crate) fn param(&self, name: &str) -> Result<pl::Expr> {
        match self.lambda {
            Some((element, _)) if element == name => Ok(pl::element()),
            Some((_, Some(index))) if index == name => Ok(pl::int_range(pl::lit(0), pl::len(), 1, pl::DataType::Int64)),
            _ => unsupported("a lambda that uses the parameter of the lambda around it"),
        }
    }
}
