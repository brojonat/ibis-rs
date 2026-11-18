// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! ibis/backends/tests/test_map.py

use ibis_core::prelude::*;
use ibis_core::{Expr, Value};

use super::array::NO_ARRAYS;
use super::{Case, c, column, scalar, tables};

fn strings() -> DataType {
    DataType::array(DataType::String)
}

fn string_map() -> DataType {
    DataType::map(DataType::String, DataType::String)
}

/// A map literal (Ibis `ibis.literal({...})`).
fn map_of<V: Into<Value>>(entries: impl IntoIterator<Item = (&'static str, V)>) -> Expr {
    lit(Value::Map(entries.into_iter().map(|(k, v)| (k.into(), v.into())).collect()))
}

fn string_array(values: &[&str]) -> Expr {
    lit_as(values.to_vec(), strings())
}

/// The maps of the null tests, by id.
fn null_map(id: &str) -> Expr {
    match id {
        "null_values" => map(string_array(&["a", "b"]), null(strings())),
        "null_keys" => map(null(strings()), string_array(&["c", "d"])),
        "null_both" => map(null(strings()), null(strings())),
        other => unreachable!("no map {other}"),
    }
}

/// The two keys, or two values, that Ibis builds a map from, per type.
fn pair(kind: &str, as_values: bool) -> [Expr; 2] {
    let a_struct = |n: i64| struct_([("a", lit(n))]);
    match kind {
        "string" => [lit("a"), lit("b")],
        "int" => [lit(1), lit(2)],
        "bool" => [lit(true), lit(false)],
        "float" => [lit(1.0), lit(2.0)],
        "timestamp" => [timestamp("2021-01-01"), timestamp("2021-01-02")],
        "date" if as_values => [date_from_ymd(2021, 1, 1), date_from_ymd(2022, 2, 2)],
        "date" => [date_from_ymd(1, 2, 3), date_from_ymd(4, 5, 6)],
        "array" => [lit(vec![1, 2]), lit(vec![3, 4])],
        "struct" => [a_struct(1), a_struct(2)],
        other => unreachable!("no kind {other}"),
    }
}

const KINDS: [&str; 8] = ["string", "int", "bool", "float", "timestamp", "date", "array", "struct"];

pub fn register(v: &mut Vec<Case>) {
    let start = v.len();
    for id in ["null_values", "null_keys", "null_both"] {
        v.push(scalar(&format!("test_map_nulls[{id}]"), move || Ok(null_map(id))));
    }
    for id in ["null_keys", "null_both"] {
        v.push(scalar(&format!("test_map_keys_nulls[{id}]"), move || Ok(null_map(id).keys())));
    }
    v.push(scalar("test_map_values_nulls[null_values]", || Ok(map(lit(vec!["a", "b"]), null(strings())).values())));
    v.push(scalar("test_map_values_nulls[null_both]", || Ok(null_map("null_both").values())));
    v.push(scalar("test_map_values_nulls[null_map]", || Ok(null(string_map()).values())));

    let get_contains: [(&str, fn() -> (Expr, Expr)); 5] = [
        ("non_null_map_null_key", || (map(lit(vec!["a", "b"]), string_array(&["c", "d"])), null(DataType::String))),
        ("null_both_non_null_key", || (null_map("null_both"), lit("a"))),
        ("null_both_null_key", || (null_map("null_both"), null(DataType::String))),
        ("null_map_non_null_key", || (null(string_map()), lit("a"))),
        ("null_map_null_key", || (null(string_map()), null(DataType::String))),
    ];
    for (id, build) in get_contains {
        v.push(scalar(&format!("test_map_get_contains_nulls[get-{id}]"), move || {
            let (m, key) = build();
            Ok(m.get(key))
        }));
        v.push(scalar(&format!("test_map_get_contains_nulls[contains-{id}]"), move || {
            let (m, key) = build();
            Ok(m.contains(key))
        }));
    }

    let ab = || lit_as(Value::Map(vec![("a".into(), "b".into())]), string_map());
    let no_map = || null(string_map());
    v.push(scalar("test_map_merge_nulls[null_and_non_null]", move || Ok(no_map() + ab())));
    v.push(scalar("test_map_merge_nulls[non_null_and_null]", move || Ok(ab() + no_map())));
    v.push(scalar("test_map_merge_nulls[null_and_null]", move || Ok(no_map() + no_map())));

    v.push(tables("test_map_table", |fx| fx.mem("map")));
    v.push(
        tables("test_column_map_values", |fx| {
            fx.mem("map")?.select([c("idx"), c("kv").values().name("vals")])?.order_by(["idx"])
        })
        .ordered(),
    );
    v.push(
        tables("test_column_map_merge", |fx| {
            let d = map(lit(vec!["d"]), lit_as(vec![1], DataType::array(DataType::Int64)));
            fx.mem("map")?.select([c("idx"), (c("kv") + d).name("merged")])?.order_by(["idx"])
        })
        .ordered(),
    );
    v.push(scalar("test_literal_map_keys", || Ok(map_of([("1", "a"), ("2", "b")]).keys())));
    v.push(scalar("test_literal_map_values", || Ok(map_of([("1", "a"), ("2", "b")]).values())));

    let a1_b2 = || map_of([("a", 1), ("b", 2)]);
    v.push(scalar("test_scalar_isin_literal_map_keys#1", move || Ok(a1_b2().keys().contains("a"))));
    v.push(scalar("test_scalar_isin_literal_map_keys#2", move || Ok(a1_b2().keys().contains("c"))));
    v.push(scalar("test_map_scalar_contains_key_scalar#1", move || Ok(a1_b2().contains("a"))));
    v.push(scalar("test_map_scalar_contains_key_scalar#2", move || Ok(a1_b2().contains("c"))));
    v.push(column("test_map_scalar_contains_key_column", |_| {
        Ok(map_of([("1", "a"), ("3", "c")]).contains(c("string_col")))
    }));

    let column_map = || map(array([c("string_col")]), array([c("int_col")]));
    v.push(column("test_map_column_contains_key_scalar", move |_| Ok(column_map().contains("1"))));
    v.push(column("test_map_column_contains_key_column", move |_| Ok(column_map().contains(c("string_col")))));
    v.push(scalar("test_literal_map_merge", || Ok(map_of([("a", 0), ("b", 2)]) + map_of([("a", 1), ("c", 3)]))));
    v.push(column("test_literal_map_getitem_broadcast", |_| Ok(map_of([("1", "a"), ("2", "b")]).get(c("string_col")))));

    for values in KINDS {
        for keys in KINDS {
            v.push(tables(&format!("test_map_get_all_types[{keys}-{values}]"), move |fx| {
                let (ks, vs) = (pair(keys, false), pair(values, true));
                let m = map(array(ks.clone()), array(vs));
                let [k1, k2] = ks;
                fx.mem("x_one")?.select([m.clone().get(k1).name("first"), m.get(k2).name("second")])
            }));
        }
    }
    for keys in KINDS {
        v.push(tables(&format!("test_map_contains_all_types[{keys}]"), move |fx| {
            let ks = pair(keys, false);
            let m = map(array(ks.clone()), array(ks.clone()));
            let [k1, k2] = ks;
            fx.mem("x_one")?.select([m.clone().contains(k1).name("first"), m.contains(k2).name("second")])
        }));
    }
    v.push(column("test_literal_map_get_broadcast", |_| {
        Ok(map_of([("1", "a"), ("2", "b")]).get_or(c("string_col"), "default"))
    }));
    v.push(scalar("test_map_construct_dict[string]", || Ok(map(lit(vec!["a", "b"]), lit(vec![1, 2])))));
    v.push(scalar("test_map_construct_dict[int]", || Ok(map(lit(vec!["a", "b"]), lit(vec!["1", "2"])))));
    v.push(column("test_map_construct_array_column", move |_| Ok(column_map())));

    let thousands = || map_of([("A", 1000), ("B", 2000)]);
    v.push(scalar("test_map_get_with_compatible_value_smaller", move || Ok(thousands().get_or("C", 3))));
    v.push(scalar("test_map_get_with_compatible_value_bigger", || Ok(map_of([("A", 1), ("B", 2)]).get_or("C", 3000))));
    v.push(scalar("test_map_get_with_incompatible_value_different_kind", move || Ok(thousands().get_or("C", 3.0))));
    let int16_map = move || thousands().cast(DataType::map(DataType::String, DataType::Int16));
    for id in ["None", "null_value1"] {
        v.push(scalar(&format!("test_map_get_with_null_on_not_nullable[{id}]"), move || Ok(int16_map().get("C"))));
    }
    let nulls = || map_of([("A", Value::Null), ("B", Value::Null)]);
    for id in ["none", "null"] {
        v.push(scalar(&format!("test_map_get_with_null_on_null_type_with_null[{id}]"), move || Ok(nulls().get("C"))));
    }
    v.push(scalar("test_map_get_with_null_on_null_type_with_non_null", move || Ok(nulls().get_or("C", 1))));
    v.push(scalar("test_map_length", || Ok(map_of([("a", "A"), ("b", "B")]).length())));
    v.push(tables("test_map_keys_unnest", |fx| fx.mem("map")?.select([c("kv").keys().unnest().name("tmp")])));

    let null_value = || map(lit(vec!["a"]), lit_as(vec![Value::Null], strings()));
    v.push(scalar("test_map_contains_null#1", move || Ok(null_value().contains("a"))));
    v.push(scalar("test_map_contains_null#2", move || Ok(null_value().contains("b"))));

    for case in &mut v[start..] {
        case.notimpl.extend(NO_ARRAYS);
    }
}
