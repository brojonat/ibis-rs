// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! ibis/backends/tests/test_window.py

use ibis_core::prelude::*;
use ibis_core::{Expr, SortKey};

use super::{Case, c, table, tables};

type Fun = fn(WindowSpec) -> Expr;
type Win = fn() -> WindowSpec;

/// `t.mutate(val=f(window)).select("id", "val")`
fn val(id: &str, f: Fun, window: Win) -> Case {
    table(id, move |t| t.mutate([f(window()).name("val")])?.select(["id", "val"]))
}

/// No ordering of its own: it comes from the window passed to `.over`.
fn unordered() -> Vec<SortKey> {
    vec![]
}

fn by_string_col() -> WindowSpec {
    WindowSpec::new().partition_by(["string_col"]).order_by(["id"])
}

pub fn register(v: &mut Vec<Case>) {
    // ibis.window(following=0, group_by=[t.string_col], order_by=[t.id])
    let expanding: Win = || by_string_col().rows(None, Some(0));
    let expanding_cases: [(&str, Fun); 24] = [
        ("lag", |w| c("float_col").lag(None).over(w)),
        ("lead", |w| c("float_col").lead(None).over(w)),
        ("rank", |w| c("id").rank().over(w)),
        ("dense_rank", |w| c("id").dense_rank().over(w)),
        ("percent_rank", |w| c("id").percent_rank().over(w)),
        ("cume_dist", |w| c("id").cume_dist().over(w)),
        ("ntile", |w| c("float_col").ntile(7).over(w)),
        ("first", |w| c("float_col").first().over(w)),
        ("last", |w| c("float_col").last().over(w)),
        ("nth", |w| c("double_col").nth(3).over(w)),
        ("row_number", |w| row_number().over(w)),
        ("cumsum", |w| c("double_col").cumsum(unordered()).over(w)),
        ("cummean", |w| c("double_col").cummean(unordered()).over(w)),
        ("cummin", |w| c("float_col").cummin(unordered()).over(w)),
        ("cummax", |w| c("float_col").cummax(unordered()).over(w)),
        ("cumany", |w| c("double_col").eq(0).any().over(w)),
        ("cumnotany", |w| c("double_col").eq(0).notany().over(w)),
        ("cumall", |w| c("double_col").eq(0).all().over(w)),
        ("cumnotall", |w| c("double_col").eq(0).notall().over(w)),
        ("sum", |w| c("double_col").sum().over(w)),
        ("mean", |w| c("double_col").mean().over(w)),
        ("min", |w| c("float_col").min().over(w)),
        ("max", |w| c("float_col").max().over(w)),
        ("count", |w| c("double_col").count().over(w)),
    ];
    for (id, f) in expanding_cases {
        let case = val(&format!("test_grouped_bounded_expanding_window[{id}]"), f, expanding);
        // Impala has no NTH_VALUE
        v.push(if id == "nth" { case.notimpl(&["impala"]) } else { case });
    }

    let mean: Fun = |w| c("double_col").mean().over(w);
    v.push(val("test_ungrouped_bounded_expanding_window[mean]", mean, || {
        WindowSpec::new().order_by(["id"]).rows(None, Some(0))
    }));

    // preceding=0, following=2 and preceding=None, following=(0, 2) are the same frame
    for id in ["zero-two", "none-zero-two"] {
        v.push(val(
            &format!("test_grouped_bounded_following_window[{id}]"),
            |w| c("id").mean().over(w),
            || by_string_col().rows(Some(0), Some(2)),
        ));
    }

    let sum: Fun = |w| c("double_col").sum().over(w);
    let preceding: [(&str, Win); 5] = [
        ("preceding-2-following-0", || by_string_col().rows(Some(2), Some(0))),
        ("preceding-2-following-0-tuple", || by_string_col().rows(Some(2), Some(0))),
        ("trailing-2", || by_string_col().rows(Some(2), Some(0))),
        ("large-preceding-999-following-0", || by_string_col().rows(Some(999), Some(0))),
        ("large-preceding-1000-following-0", || by_string_col().rows(Some(1000), Some(0))),
    ];
    for (id, window) in preceding {
        v.push(val(&format!("test_grouped_bounded_preceding_window[{id}]"), sum, window));
    }

    v.push(val("test_grouped_unbounded_window[mean]", mean, || WindowSpec::new().partition_by(["string_col"])));

    let methods: [(&str, fn(Expr) -> Expr); 3] = [("sum", Expr::sum), ("min", Expr::min), ("mean", Expr::mean)];
    for (id, method) in methods {
        v.push(table(&format!("test_simple_ungrouped_unbound_following_window[{id}]"), move |t| {
            let t = t.filter([t.col("double_col").lt(50)])?.order_by(["id"])?;
            // ibis.window(rows=(0, None), order_by=t.id)
            let w = WindowSpec::new().order_by([t.col("id")]).rows(Some(0), None);
            t.select([c("id"), method(t.col("double_col")).over(w).name("double_col")])
        }));
    }

    let ordered: Win = || WindowSpec::new().order_by(["id"]);
    v.push(val("test_ungrouped_unbounded_window[ordered-mean]", mean, ordered));
    v.push(val("test_ungrouped_unbounded_window[unordered-mean]", mean, WindowSpec::new));
    v.push(val("test_ungrouped_unbounded_window[ordered-lag]", |w| c("float_col").lag(None).over(w), ordered));
    v.push(val("test_ungrouped_unbounded_window[ordered-lead]", |w| c("float_col").lead(None).over(w), ordered));

    v.push(table("test_percent_rank_whole_table_no_order_by", |t| {
        t.mutate([c("id").percent_rank().name("val")])?.select(["id", "val"])
    }));
    // t.group_by("month").order_by("id").mutate(...): the window is spelled out
    v.push(table("test_grouped_ordered_window_coalesce", |t| {
        let w = WindowSpec::new().partition_by(["month"]).order_by(["id"]);
        t.mutate([coalesce([c("bigint_col").lag(None).over(w), lit(0)]).name("lagged_value")])?
            .select(["id", "lagged_value"])
    }));
    v.push(table("test_mutate_window_filter", |t| {
        let win = WindowSpec::new().order_by([t.col("id")]);
        Ok(t.mutate([t.col("int_col").lead(None).over(win).name("next_int")])?
            .filter([c("int_col").eq(1)])?
            .select(["int_col", "next_int"])?
            .limit(3, 0))
    }));
    v.push(tables("test_first_last", |fx| {
        let t = fx.mem("win")?;
        let w = || WindowSpec::new().partition_by(["g"]).order_by(["x", "y"]).rows(Some(1), Some(0));
        t.mutate([
            c("x").first().over(w()).name("x_first"),
            c("x").last().over(w()).name("x_last"),
            c("y").first().over(w()).name("y_first"),
            c("y").last().over(w()).name("y_last"),
        ])
    }));
    v.push(tables("test_windowed_order_by_sequence_is_preserved", |fx| {
        let t = fx.mem("bool_nulls")?;
        let window = WindowSpec::new().order_by([c("bool_col").isnull().asc(), c("bool_col").asc()]);
        t.select([c("bool_col").rank().over(window).name("rank"), c("bool_col")])
    }));
    v.push(table("test_over_with_scalar_group_by", |t| {
        t.select([c("tinyint_col").sum().over(WindowSpec::new().partition_by([lit("string_col")])).name("x")])
    }));
}
