// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! ibis/backends/tests/test_array.py

use ibis_core::prelude::*;
use ibis_core::{Expr, Result};

use super::{Case, c, column, scalar, tables};

/// Engines with no array type (Ibis `NO_ARRAY_SUPPORT_MARKS`).
pub const NO_ARRAYS: [&str; 3] = ["impala", "mysql", "sqlite"];

pub fn register(v: &mut Vec<Case>) {
    let start = v.len();
    v.push(column("test_array_column", |_| Ok(array([c("double_col"), c("double_col"), lit(5.0), lit(6.0)]))));
    v.push(scalar("test_array_scalar", || Ok(array([lit(1.0), lit(2.0), lit(3.0)]))));
    v.push(scalar("test_array_repeat", || Ok(array([lit(1.0), lit(2.0)]) * 2)));
    v.push(tables("test_array_repeat_column", |fx| fx.mem("arr_x_f64")?.select([(c("x") * 2).name("tmp")])));
    v.push(scalar("test_array_concat", || Ok(lit(vec![1, 2, 3]) + lit(vec![2, 1]))));
    v.push(scalar("test_array_concat_variadic", || {
        Ok(lit(vec![1, 2, 3]).concat([lit(vec![2, 1]), lit(vec![2, 1]), lit(vec![2, 1])]))
    }));
    v.push(scalar("test_array_concat_some_empty", || Ok(lit(Vec::<i64>::new()).concat([lit(vec![2, 1])]))));
    v.push(scalar("test_array_radd_concat", || Ok(lit(vec![1]) + lit(vec![2]))));
    v.push(scalar("test_array_concat_scalar[<lambda>0]", || Ok(lit(vec![1, 2, 3]) + lit(vec![3, 4]))));
    v.push(scalar("test_array_concat_scalar[<lambda>1]", || Ok(lit(vec![3, 4]) + lit(vec![1, 2, 3]))));

    let max_int32 = || array([lit(i64::from(i32::MAX))]);
    let no_array = || null(DataType::array(DataType::Int32));
    v.push(scalar("test_array_concat_with_null[left]", move || Ok(max_int32() + no_array())));
    v.push(scalar("test_array_concat_with_null[right]", move || Ok(no_array() + max_int32())));
    v.push(tables("test_array_concat_with_null_non_constant", |fx| {
        fx.mem("arr_a_null")?.select([(c("a") + c("a")).name("tmp")])
    }));
    v.push(scalar("test_array_length", || Ok(lit(vec![1, 2, 3]).length())));
    v.push(scalar("test_list_literal", || Ok(lit(vec![1, 2, 3]))));
    v.push(scalar("test_np_array_literal", || Ok(lit(vec![1, 2, 3]))));
    for i in 0..3 {
        v.push(scalar(&format!("test_array_index[{i}]"), move || Ok(lit(vec![1, 2, 3]).at(i))));
    }
    v.push(tables("test_array_discovery", |fx| fx.mem("array_types")));

    let slices = [
        ("1-3", Some(1), Some(3)),
        ("1-1", Some(1), Some(1)),
        ("2-3", Some(2), Some(3)),
        ("2-5", Some(2), Some(5)),
        ("None-3", None, Some(3)),
        ("None-None", None, None),
        ("3-None", Some(3), None),
        ("-3-None", Some(-3), None),
        ("-3--1", Some(-3), Some(-1)),
        ("nulls", None, Some(-3)),
    ];
    for (id, start, stop) in slices {
        v.push(tables(&format!("test_array_slice[{id}]"), move |fx| {
            fx.mem("array_types")?.select([c("y").slice(start, stop).name("sliced")])
        }));
    }

    unnest(v);
    lambdas(v);
    searches(v);
    zips_and_ranges(v);
    table_unnest(v);
    reductions(v);

    for case in &mut v[start..] {
        case.notimpl.extend(NO_ARRAYS);
    }
}

fn int_array() -> DataType {
    DataType::array(DataType::Int64)
}

fn array_types(id: &str, f: impl Fn(Table) -> Result<Table> + 'static) -> Case {
    mem(id, "array_types", f)
}

/// A case over one table of conformance/memtables.json.
fn mem(id: &str, table: &'static str, f: impl Fn(Table) -> Result<Table> + 'static) -> Case {
    tables(id, move |fx| f(fx.mem(table)?))
}

fn unnest(v: &mut Vec<Case>) {
    v.push(array_types("test_unnest_simple", |t| {
        t.select([c("x").cast(DataType::array(DataType::Float64)).unnest().name("tmp")])
    }));
    v.push(
        array_types("test_unnest_complex", |t| {
            t.select(["grouper", "x"])?
                .mutate([c("x").unnest().name("x")])?
                .group_by(["grouper"])
                .agg([c("x").count().name("count_flat")])?
                .order_by(["grouper"])
        })
        .ordered(),
    );
    v.push(
        array_types("test_unnest_idempotent", |t| {
            t.select([c("scalar_column"), c("x").cast(int_array()).unnest().name("x")])?
                .group_by(["scalar_column"])
                .agg([c("x").collect().name("x")])?
                .mutate([c("x").sort().name("x")])?
                .order_by(["scalar_column"])
        })
        .ordered(),
    );
    v.push(
        array_types("test_unnest_no_nulls", |t| {
            t.select([c("scalar_column"), c("x").cast(int_array()).unnest().name("y")])?
                .filter([c("y").notnull()])?
                .group_by(["scalar_column"])
                .agg([c("y").collect().name("x")])?
                .mutate([c("x").sort().name("x")])?
                .order_by(["scalar_column"])
        })
        .ordered(),
    );
    v.push(array_types("test_unnest_default_name", |t| {
        t.select([(c("x").cast(int_array()) + array([lit(1)]).cast(int_array())).unnest().name("x")])
    }));
    v.push(mem("test_unnest_struct", "arr_struct_a", |t| t.select([c("value").unnest().name("tmp")])));
    v.push(mem("test_unnest_struct_with_multiple_fields", "arr_struct_ab", |t| {
        t.select([c("value").unnest().name("tmp")])
    }));
    v.push(mem("test_array_of_struct_unnest", "jobs", |t| {
        t.select([c("steps").unnest().field("status").name("status")])
    }));
    v.push(mem("test_unnest_empty_array", "arr_strings", |t| t.select([c("arr").unnest().name("tmp")])));
    v.push(mem("test_unnest_range", "x_one", |t| {
        t.select([range(0, 2, 1).unnest().name("x")])?.mutate([lit(1.0).name("y")])
    }));
    for n in [-2, 0, 2] {
        v.push(mem(&format!("test_range_single_argument_unnest[{n}]"), "x_one", move |t| {
            t.select([range(0, n, 1).unnest().name("tmp")])
        }));
    }
}

fn lambdas(v: &mut Vec<Case>) {
    for (nulls, table) in [("nulls", "arr_int8_nulls"), ("no_nulls", "arr_int8")] {
        for how in ["lambda", "partial", "deferred"] {
            v.push(mem(&format!("test_array_map[{how}-{nulls}]"), table, |t| {
                t.select([c("a").map(|x| x + 1).name("a")])
            }));
            v.push(mem(&format!("test_array_filter[{how}-{nulls}]"), table, |t| {
                t.select([c("a").filter(|x| x.gt(1)).name("a")])
            }));
        }
        for how in ["lambda", "partial"] {
            v.push(mem(&format!("test_array_map_with_index[{how}-{nulls}]"), table, |t| {
                t.select([c("a").map_with_index(|x, i| x + 1 + i).name("a")])
            }));
        }
        v.push(mem(&format!("test_array_filter_with_index[lambda-{nulls}]"), table, |t| {
            t.select([c("a").filter_with_index(|x, i| (x + (i.clone() - i)).gt(1)).name("a")])
        }));
        v.push(mem(&format!("test_array_filter_with_index[partial-{nulls}]"), table, |t| {
            t.select([c("a").filter_with_index(|x, i| x.gt(lit(1) + i * lit(0))).name("a")])
        }));
    }
    for (nulls, table) in [("nulls", "arr_int8_idx_nulls"), ("no_nulls", "arr_int8_idx")] {
        for how in ["lambda", "partial"] {
            v.push(mem(&format!("test_array_filter_with_index_lambda[{how}-{nulls}]"), table, |t| {
                t.select([c("a").filter_with_index(|_, i| (i % 2).eq(0)).name("a")])
            }));
        }
    }
    v.push(mem("test_array_map_with_conflicting_names", "arr_x_int8", |t| {
        t.select([c("x").map(|x| x + 1).name("a")])?.select([c("a").filter(|a| a.gt(2)).name("b")])
    }));
    let words = || array([lit("abc"), lit("xyz")]);
    v.push(scalar("test_complex_array_map#1", move || Ok(words().map(|token| token.upper()))));
    v.push(scalar("test_complex_array_map#2", move || {
        Ok(words().map(|token| token.clone().cases([(lit("abc"), lit("ABC"))], Some(token))))
    }));
}

fn searches(v: &mut Vec<Case>) {
    v.push(array_types("test_array_contains[x-1]", |t| t.select([c("x").contains(1).name("tmp")])));
    v.push(array_types("test_array_contains[y-a]", |t| t.select([c("y").contains("a").name("tmp")])));
    for (id, table) in [("some-empty", "arr_position_some_empty"), ("none-empty", "arr_position_none_empty")] {
        v.push(mem(&format!("test_array_position[{id}]"), table, |t| t.select([c("a").index(42).name("tmp")])));
    }
    for (id, table) in [
        ("including-empty-array", "arr_remove_empty"),
        ("all-non-empty-arrays", "arr_remove_non_empty"),
        ("nested-null", "arr_remove_nested_null"),
        ("non-nested-null", "arr_remove_null"),
    ] {
        v.push(mem(&format!("test_array_remove[{id}]"), table, |t| t.select([c("a").remove(2).name("tmp")])));
    }
    for (id, table) in [("null", "arr_unique_null"), ("not_null", "arr_unique_not_null")] {
        v.push(mem(&format!("test_array_unique[{id}]"), table, |t| t.select([c("a").unique().sort().name("tmp")])));
    }
    for (id, table) in [("empty", "arr_sort_empty"), ("nonempty", "arr_sort_nonempty")] {
        v.push(
            mem(&format!("test_array_sort[{id}]"), table, |t| t.mutate([c("a").sort().name("a")])?.order_by(["id"]))
                .ordered(),
        );
    }
    for (id, table) in [("including-empty-array", "arr_union_empty"), ("all-non-empty-arrays", "arr_union_non_empty")] {
        v.push(mem(&format!("test_array_union[{id}]"), table, |t| t.select([c("a").union(c("b")).sort().name("tmp")])));
    }
    for (id, table) in [("nulls", "arr_intersect_nulls"), ("no_nulls", "arr_intersect_no_nulls")] {
        v.push(
            mem(&format!("test_array_intersect[{id}]"), table, |t| {
                t.select([c("c"), c("a").intersect(c("b")).sort().name("d")])?.order_by(["c"])?.drop(&["c"])
            })
            .ordered(),
        );
    }
    for column in ["empty", "happy", "nulls_only", "mixed_nulls"] {
        let table: &'static str = match column {
            "empty" => "flatten_empty",
            "happy" => "flatten_happy",
            "nulls_only" => "flatten_nulls_only",
            _ => "flatten_mixed_nulls",
        };
        v.push(
            mem(&format!("test_array_flatten[{column}]"), table, move |t| {
                t.select([c("id"), c(column).flatten().name("flat")])?.order_by(["id"])
            })
            .ordered(),
        );
    }
    v.push(mem("test_flatten", "arr_nested", |t| t.select([c("arr").flatten().name("result")])));
}

fn zips_and_ranges(v: &mut Vec<Case>) {
    for n in [1, 5] {
        v.push(
            array_types(&format!("test_zip[{n}]"), move |t| {
                let others = std::iter::repeat_n(c("x"), n);
                t.select([c("scalar_column").name("id"), c("x").zip(others).name("zipped")])?.order_by(["id"])
            })
            .ordered(),
        );
    }
    let a = || lit_as(vec![1, 2, 3], int_array());
    let b = || null(int_array());
    v.push(scalar("test_zip_null[non-null-zip-null]", move || Ok(a().zip([b()]))));
    v.push(scalar("test_zip_null[null-zip-non-null]", move || Ok(b().zip([a()]))));
    v.push(scalar("test_zip_null[null-zip-null]", move || Ok(b().zip([b()]))));
    v.push(mem("test_zip_unnest_lift", "arr_pair", |t| {
        t.mutate([c("array1").zip([c("array2")]).name("zipped")])?
            .mutate([c("zipped").unnest().name("unnest")])?
            .lift("unnest")
    }));

    for n in [-2, 0, 2] {
        v.push(scalar(&format!("test_range_single_argument[{n}]"), move || Ok(range(0, n, 1))));
    }
    for step in [-2, -1, 1, 2] {
        for stop in [-7, 0, 7] {
            for start in [-7, 0, 7] {
                v.push(scalar(&format!("test_range_start_stop_step[{step}-{stop}-{start}]"), move || {
                    Ok(range(start, stop, step))
                }));
            }
        }
    }
    for start in [-7, 0, 7] {
        for stop in [-7, 0, 7] {
            v.push(scalar(&format!("test_range_start_stop_step_zero[{start}-{stop}]"), move || {
                Ok(range(start, stop, 0))
            }));
        }
    }
    v.push(scalar("test_array_literal_with_exprs[int-int]", || Ok(array([lit(1), lit(2)]))));
    v.push(scalar("test_array_literal_with_exprs[float-int]", || Ok(array([lit(1.0), lit(2)]))));
    v.push(scalar("test_array_literal_with_exprs[float-float]", || Ok(array([lit(1.0), lit(2.0)]))));
    v.push(scalar("test_array_literal_with_exprs[int-float]", || Ok(array([lit(1), lit(2.0)]))));
    v.push(scalar("test_array_literal_with_exprs[int-float-exprs]", || Ok(array([lit(1), lit(2.0)]))));
    v.push(scalar("test_array_literal_with_exprs[array]", || Ok(array([lit(vec![1]), lit(vec![2])]))));
}

fn table_unnest(v: &mut Vec<Case>) {
    for how in ["string", "lambda", "deferred"] {
        v.push(array_types(&format!("test_table_unnest[{how}]"), |t| t.unnest("y", None, false)));
    }
    v.push(
        array_types("test_table_unnest_with_offset", |t| {
            t.unnest("y", Some("idx"), false)?.select(["idx", "y"])?.order_by(["idx", "y"])
        })
        .ordered(),
    );
    v.push(mem("test_table_unnest_with_keep_empty", "arr_keep_empty", |t| t.unnest("y", None, true)?.select(["y"])));
    v.push(array_types("test_table_unnest_column_expr", |t| {
        let plural = c("y").map(|v| v.cast(DataType::String) + lit("'s")).name("plural");
        t.unnest(plural, None, false)?.select(["plural"])
    }));
    v.push(
        mem("test_table_unnest_array_of_struct_of_array", "arr_struct_of_arr", |t| {
            t.unnest("a", None, false)?.select([c("a").field("x").unnest().name("x")])?.order_by(["x"])
        })
        .ordered(),
    );
}

fn reductions(v: &mut Vec<Case>) {
    let numeric: [(&str, fn(Expr) -> Expr); 5] = [
        ("sums", Expr::sums),
        ("mins", Expr::mins),
        ("maxs", Expr::maxs),
        ("means", Expr::means),
        ("modes", Expr::modes),
    ];
    for (nulls, table) in [("nulls", "arr_agg_nulls"), ("no-nulls", "arr_agg_no_nulls")] {
        for (name, agg) in numeric {
            v.push(
                mem(&format!("test_array_agg_numeric[{nulls}-{name}]"), table, move |t| {
                    t.mutate([agg(c("x")).name("y")])?.order_by(["id"])
                })
                .ordered(),
            );
        }
    }
    let boolean: [(&str, fn(Expr) -> Expr); 2] = [("anys", Expr::anys), ("alls", Expr::alls)];
    for (nulls, table) in [("nulls", "arr_bool_nulls"), ("no-nulls", "arr_bool_no_nulls")] {
        for (name, agg) in boolean {
            v.push(
                mem(&format!("test_array_agg_bool[{nulls}-{name}]"), table, move |t| {
                    t.mutate([agg(c("x")).name("y")])?.order_by(["id"])
                })
                .ordered(),
            );
        }
    }
}
