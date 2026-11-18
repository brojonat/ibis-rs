// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! ibis/backends/tests/test_struct.py

use ibis_core::Value;
use ibis_core::prelude::*;

use super::array::NO_ARRAYS;
use super::{Case, c, scalar, table, tables};

pub fn register(v: &mut Vec<Case>) {
    let start = v.len();
    let struct_type =
        || DataType::structure(&[("a", DataType::Int64), ("b", DataType::String), ("c", DataType::Float64)]);
    for field in ["a", "b", "c"] {
        v.push(
            tables(&format!("test_single_field[{field}]"), move |fx| {
                fx.mem("struct")?.select([c("abc").field(field).name("field")])?.order_by(["field"])
            })
            .ordered(),
        );
    }
    v.push(tables("test_all_fields", |fx| fx.mem("struct")?.select(["abc"])));
    v.push(scalar("test_literal", move || {
        let value = Value::Struct(vec![("a".into(), 1.into()), ("b".into(), "2".into()), ("c".into(), 3.0.into())]);
        Ok(lit_as(value, struct_type()))
    }));
    for field in ["a", "b", "c"] {
        v.push(scalar(&format!("test_null_literal[{field}]"), move || {
            Ok(null(DataType::Null).cast(struct_type()).field(field))
        }));
    }
    v.push(table("test_struct_column", |t| {
        t.select([struct_([("a", c("string_col")), ("b", lit(1)), ("c", c("bigint_col"))]).name("s")])
    }));
    v.push(table("test_collect_into_struct", |t| {
        let key = c("bigint_col").collect().sort().cast(DataType::array(DataType::Int64));
        t.filter([c("string_col").isin([lit("0"), lit("1")])])?
            .group_by([c("string_col").name("group")])
            .agg([struct_([("key", key)]).name("val")])
    }));
    v.push(scalar("test_field_access_after_case", || {
        let s = |n: i64| struct_([("a", lit(n))]);
        Ok(cases([(lit(true), s(3))], Some(s(4))).field("a"))
    }));
    v.push(tables("test_isin_struct", |fx| {
        let needle = |x: i64, y: i64| struct_([("x", lit(x)), ("y", lit(y))]);
        let haystack_t = fx.mem("xs_ys")?;
        let haystack = || struct_([("x", haystack_t.col("xs")), ("y", haystack_t.col("ys"))]);
        let both = needle(1, 2).isin_col(haystack()) | needle(2, 3).isin_col(haystack());
        fx.mem("x_one")?.select([both.name("tmp")])
    }));
    for n in 0..4 {
        v.push(tables(&format!("test_field_overwrite_always_prefers_unpacked#{}", n + 1), move |fx| {
            let t = fx.mem("struct")?.filter([c("abc").field("a").notnull()])?;
            let t1 = t.mutate([lit(1).name("a")])?;
            let t2 = t1.select(["a", "abc"])?;
            let t3 = t1.select(["abc", "a"])?;
            let t4 = t2.select(["abc", "a"])?;
            [t1, t2, t3, t4][n].unpack(&["abc"])
        }));
    }

    for case in &mut v[start..] {
        case.notimpl.extend(NO_ARRAYS);
    }
}
