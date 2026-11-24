// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! ibis/backends/tests/test_param.py
//!
//! Each case builds its expression with a parameter and binds a value to
//! it; the golden is the expression with the value written in.

use ibis_core::Value;
use ibis_core::prelude::*;

use super::{Case, NO_NESTED, c, table};

pub fn register(v: &mut Vec<Case>) {
    use DataType as T;
    let ts = || T::Timestamp { tz: None };

    for (col, raw) in [("double_col", 0.0), ("double_col", 10.1), ("float_col", 1.1), ("float_col", 2.2)] {
        v.push(table(&format!("test_floating_scalar_parameter[{col}-{raw:?}]"), move |t| {
            let value = param(T::Float64);
            t.select([c("id"), (c(col) + value.clone()).name("tmp")])?.bind([(value, lit(raw))])
        }));
    }
    for (start, end) in [("2009-03-01", "2010-07-03"), ("2014-12-01", "2017-01-05")] {
        v.push(table(&format!("test_date_scalar_parameter[{start}-{end}]"), move |t| {
            let (from, to) = (param(T::Date), param(T::Date));
            let within = c("timestamp_col").date().between(from.clone(), to.clone());
            t.select([c("id"), within.name("tmp")])?.bind([(from, lit(start)), (to, lit(end))])
        }));
    }
    v.push(table("test_timestamp_accepts_date_literals", move |t| {
        let p = param(ts());
        t.select([c("id"), p.clone().name("param")])?.bind([(p, lit("2009-03-01"))])
    }));

    // a value with no column in it: one row of the table carries it
    let one = |e: Expr, values: Vec<(Expr, Expr)>| {
        move |t: &Table| t.select([e.clone().name("tmp")])?.limit(1, 0).bind(values.clone())
    };
    let numbers = param(T::array(T::Int64));
    v.push(
        table("test_scalar_param_array", one(numbers.clone().length(), vec![(numbers, lit(vec![1, 2, 3]))]))
            .notimpl(&NO_NESTED),
    );
    let record = param(T::structure(&[("a", T::Int64), ("b", T::String), ("c", T::Float64)]));
    let fields = Value::Struct(vec![
        ("a".into(), Value::Int(1)),
        ("b".into(), Value::String("abc".into())),
        ("c".into(), Value::Float(3.0)),
    ]);
    v.push(
        table("test_scalar_param_struct", one(record.clone().field("a"), vec![(record, lit(fields))]))
            .notimpl(&NO_NESTED),
    );
    let lookup = param(T::map(T::String, T::String));
    let entries = Value::Map(
        [("a", "ghi"), ("b", "def"), ("c", "abc")]
            .into_iter()
            .map(|(k, x)| (Value::String(k.into()), Value::String(x.into())))
            .collect(),
    );
    v.push(
        table("test_scalar_param_map", one(lookup.clone().get("b"), vec![(lookup, lit(entries))])).notimpl(&NO_NESTED),
    );

    // the Ibis test compares whole rows; the ids say which rows are kept
    let moment = |s: &str| lit(s).cast(T::Timestamp { tz: None });
    let scalars: [(&str, T, &str, Expr); 7] = [
        ("string", T::String, "string_col", lit("0")),
        ("int", T::Int64, "int_col", lit(0)),
        ("double", T::Float64, "double_col", lit(0.0)),
        ("bool", T::Boolean, "bool_col", lit(true)),
        ("string_timestamp", ts(), "timestamp_col", lit("2009-01-20 01:02:03")),
        ("date_timestamp", ts(), "timestamp_col", date("2009-01-20")),
        ("datetime_timestamp", ts(), "timestamp_col", moment("2009-01-20 01:02:03")),
    ];
    for (id, dtype, col, value) in scalars {
        v.push(table(&format!("test_scalar_param[{id}]"), move |t| {
            let p = param(dtype.clone());
            t.filter([c(col).eq(p.clone())])?.select(["id"])?.bind([(p, value.clone())])
        }));
    }
    let dates: [(&str, Expr); 3] =
        [("string", lit("2009-01-20")), ("date", date("2009-01-20")), ("datetime", moment("2009-01-20 00:00:00"))];
    for (id, value) in dates {
        v.push(table(&format!("test_scalar_param_date[{id}]"), move |t| {
            let p = param(T::Date);
            let ds = || c("date_string_col");
            let (month, day, year) =
                (ds().slice(None, Some(2)), ds().slice(Some(3), Some(5)), lit("20") + ds().slice(Some(6), Some(8)));
            let date_col = (year + lit("-") + month + lit("-") + day).cast(T::Date);
            t.mutate([date_col.name("date_col")])?
                .filter([c("date_col").eq(p.clone())])?
                .select(["id"])?
                .bind([(p, value.clone())])
        }));
    }

    let inner = T::structure(&[("y", T::array(T::Float64))]);
    let nested = param(T::structure(&[("x", T::array(inner))]));
    let ys = Value::Array(vec![Value::Float(1.0), Value::Float(2.0), Value::Float(3.0)]);
    let value = Value::Struct(vec![("x".into(), Value::Array(vec![Value::Struct(vec![("y".into(), ys)])]))]);
    v.push(table("test_scalar_param_nested", one(nested.clone(), vec![(nested, lit(value))])).notimpl(&NO_NESTED));
}
