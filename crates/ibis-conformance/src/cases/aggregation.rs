// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! ibis/backends/tests/test_aggregation.py

use ibis_core::prelude::*;

use super::{Case, c, table, tables};

pub fn register(v: &mut Vec<Case>) {
    let aggs: Vec<(&str, fn() -> Expr)> = vec![
        ("mean", || c("double_col").mean()),
        ("min", || c("double_col").min()),
        ("max", || c("double_col").max()),
        ("complex_sum", || (c("double_col") + 5).sum()),
        ("timestamp_max", || c("timestamp_col").max()),
        // int_col % 3 so there are no ties for most common value
        ("mode", || (c("int_col") % 3).mode()),
        ("argmin", || c("timestamp_col").argmin("id")),
        ("argmax", || c("double_col").argmax("id")),
    ];
    for (id, f) in &aggs {
        let f = *f;
        v.push(table(&format!("test_aggregate[{id}]"), move |t| t.aggregate([f().name("tmp")])).notimpl(missing(id)));
    }
    for (id, f) in &aggs {
        let f = *f;
        let grouped =
            table(&format!("test_aggregate_grouped[{id}]"), move |t| t.group_by(["bigint_col"]).agg([f().name("tmp")]));
        v.push(grouped.notimpl(missing(id)));
    }

    let reductions: Vec<(&str, fn() -> Expr)> = vec![
        ("count", || c("bool_col").count()),
        ("nunique", || c("bool_col").nunique()),
        ("any", || c("bool_col").any()),
        ("any_negate", || c("bool_col").any()),
        ("all", || c("bool_col").all()),
        ("all_negate", || c("bool_col").all()),
        ("sum", || c("double_col").sum()),
        ("bool_sum", || c("int_col").gt(0).sum()),
        ("mean", || c("double_col").mean()),
        ("min", || c("double_col").min()),
        ("max", || c("double_col").max()),
        ("std", || c("double_col").std()),
        ("var", || c("double_col").var()),
        ("std_pop", || c("double_col").std_pop()),
        ("var_pop", || c("double_col").var_pop()),
        ("count_star", count),
        ("notany", || c("bool_col").notany()),
        ("notall", || c("bool_col").notall()),
        ("mode", || (c("int_col") % 3).mode()),
        ("kurtosis_samp", || c("double_col").kurtosis()),
        ("approx_nunique", || c("string_col").approx_nunique()),
        ("bit_and", || c("bigint_col").bit_and()),
        ("bit_or", || c("bigint_col").bit_or()),
        ("bit_xor", || c("bigint_col").bit_xor()),
        ("argmin", || c("double_col").argmin("int_col")),
        ("argmax", || c("double_col").argmax("int_col")),
    ];
    let conds: Vec<(&str, fn() -> Option<Expr>)> =
        vec![("no_cond", || None), ("is_in", || Some(c("string_col").isin([lit("1"), lit("7")])))];
    for (cid, cond) in &conds {
        for (rid, red) in &reductions {
            let (cond, red, negate) = (*cond, *red, rid.ends_with("_negate"));
            let case = table(&format!("test_reduction_ops[{cid}-{rid}]"), move |t| {
                let mut e = red();
                if let Some(w) = cond() {
                    e = e.filter_where(w);
                }
                if negate {
                    e = !e;
                }
                t.filter([c("id").lt(1550)])?.aggregate([e.name("tmp")])
            });
            v.push(case.notimpl(missing(rid)));
        }
    }

    v.push(
        table("test_median", |t| t.aggregate([c("double_col").median().name("tmp")]))
            .notimpl(&["impala", "mysql", "sqlite", "trino"]),
    );
    v.push(
        table("test_string_quantile[median]", |t| {
            t.select([lit("a").name("col")])?.limit(5, 0).aggregate([c("col").median().name("tmp")])
        })
        // Ibis marks ClickHouse, PySpark and Polars too: no median of strings there
        .notimpl(&["impala", "mysql", "sqlite", "trino", "clickhouse", "spark", "polars"]),
    );

    // ids are "<null>-<filtered>-<method>": the key is NULL where int_col is 0
    // or 9, and with "null" the value at the answer's row is NULL too
    for (null_id, null_result) in [("null", true), ("non-null", false)] {
        for (filtered_id, filtered) in [("filtered", true), ("unfiltered", false)] {
            for method in ["argmin", "argmax"] {
                let id = format!("test_argmin_argmax[{null_id}-{filtered_id}-{method}]");
                let case = table(&id, move |t| {
                    let smallest = method == "argmin";
                    let by_col = c("int_col").nullif(0).nullif(9).name("by_col");
                    let mut t = t.mutate([by_col, (lit(10) * c("int_col")).name("val_col")])?;
                    let (filter, sol) = match (filtered, smallest) {
                        (true, true) => (Some(c("int_col").ne(1)), 20),
                        (true, false) => (Some(c("int_col").ne(8)), 70),
                        (false, true) => (None, 10),
                        (false, false) => (None, 80),
                    };
                    if null_result {
                        t = t.mutate([c("val_col").nullif(sol).name("val_col")])?;
                    }
                    let mut e = if smallest { c("val_col").argmin("by_col") } else { c("val_col").argmax("by_col") };
                    if let Some(filter) = filter {
                        e = e.filter_where(filter);
                    }
                    t.aggregate([e.name("tmp")])
                });
                v.push(case.notimpl(missing(method)));
            }
        }
    }

    // on batting; ids are "<cond>-<fn>". "corr_samp" has no golden: DuckDB,
    // which the goldens come from, has no sample correlation.
    let corr_cov: Vec<(&str, fn() -> Expr)> = vec![
        ("covar_pop", || c("G").cov_pop("RBI")),
        ("covar_samp", || c("G").cov("RBI")),
        ("corr_pop", || c("G").corr_pop("RBI")),
        ("covar_pop_bool", || c("G").gt(34.0).cov_pop(c("G").le(34.0))),
        ("corr_pop_bool", || c("G").gt(34.0).corr_pop(c("G").le(34.0))),
    ];
    for (cid, cond) in [("no_cond", false), ("cond", true)] {
        for (fid, f) in &corr_cov {
            let f = *f;
            let case = tables(&format!("test_corr_cov[{cid}-{fid}]"), move |fx| {
                let mut e = f();
                if cond {
                    e = e.filter_where(c("yearID").isin([lit(2009), lit(2015)]));
                }
                fx.batting.aggregate([e.name("tmp")])
            });
            // Ibis marks ClickHouse for the population's correlation: it names only the sample's
            let none: &[&str] = match fid.starts_with("corr") {
                true => &["impala", "mysql", "sqlite", "clickhouse"],
                false => &["impala", "mysql", "sqlite"],
            };
            v.push(case.notimpl(none));
        }
    }

    // ids are "<cond>-<quantile|multi-quantile>"
    for (cid, cond) in &conds {
        for (qid, many) in [("quantile", false), ("multi-quantile", true)] {
            let cond = *cond;
            let case = table(&format!("test_quantile[{cid}-{qid}]"), move |t| {
                let mut e = if many { c("double_col").quantiles([0.5]) } else { c("double_col").quantile(0.5) };
                if let Some(w) = cond() {
                    e = e.filter_where(w);
                }
                t.aggregate([e.name("tmp")])
            });
            // Trino has approximate quantiles only; Polars takes one fraction at a time
            let none: &[&str] = match many {
                true => &["impala", "mysql", "sqlite", "trino", "polars"],
                false => &["impala", "mysql", "sqlite", "trino"],
            };
            v.push(case.notimpl(none));
        }
    }
    v.push(
        table("test_string_quantile[quantile]", |t| {
            t.select([lit("a").name("col")])?.limit(5, 0).aggregate([c("col").quantile(0.5).name("tmp")])
        })
        .notimpl(&["impala", "mysql", "sqlite", "trino", "clickhouse", "spark", "polars"]),
    );
    v.push(
        table("test_date_quantile", |t| t.aggregate([c("timestamp_col").date().quantile(0.5).name("tmp")]))
            .notimpl(&["impala", "mysql", "sqlite", "trino", "spark"]),
    );

    v.push(table("test_binds_are_cast", |t| {
        let urgent = c("string_col").cases([(lit("1-URGENT"), lit(1))], Some(lit(0)));
        t.aggregate([urgent.sum().name("high_line_count")])
    }));
    v.push(table("test_filter", |t| {
        t.filter([c("string_col").eq(lit("1"))])?
            .mutate([lit_as(1, DataType::Int64).name("x")])?
            .group_by(["x"])
            .agg([c("double_col").sum().name("sum")])
    }));
    v.push(
        tables("test_grouped_case", |fx| {
            let case = cases([(c("value").lt(25), c("value"))], None);
            fx.mem("grouped_case")?
                .group_by([c("key").name("k")])
                .agg([case.max().name("mx")])?
                .drop_null(Some(&["k"]), How::Any)?
                .order_by(["k"])
        })
        .ordered(),
    );
    v.push(table("test_value_counts_on_expr", |t| t.value_counts(c("bigint_col") + 1)));
    v.push(
        tables("test_group_by_expr", |fx| {
            fx.mem("project_names")?
                .group_by([c("project_name").length().name("n")])
                .agg([c("project_name").nunique().name("c")])?
                .order_by([desc(c("n"))])
        })
        .ordered(),
    );
    let scalars: [(&str, fn() -> Expr); 2] = [("string", || lit("a")), ("null", || null(DataType::String))];
    for (id, value) in scalars {
        v.push(table(&format!("test_group_by_scalar[{id}]"), move |t| {
            t.group_by([value().name("key")]).agg([count().name("n")])
        }));
    }
    // the test's two results: a count, and the sum of no rows
    v.push(tables("test_empty_sum#1", |fx| fx.mem("x_one")?.aggregate([count().name("tmp")])));
    v.push(tables("test_empty_sum#2", |fx| {
        fx.mem("x_one")?.aggregate([c("x").sum().filter_where(c("x").gt(1)).name("tmp")])
    }));
}

/// The backends Ibis marks as having no such reduction.
fn missing(reduction: &str) -> &'static [&'static str] {
    match reduction {
        "mode" => &["clickhouse", "impala", "mysql", "spark", "trino"],
        "kurtosis_samp" => &["clickhouse", "impala", "mysql", "postgres", "spark", "sqlite"],
        "bit_and" | "bit_or" | "bit_xor" => &["impala", "spark"],
        "argmin" | "argmax" => &["impala", "mysql"],
        _ => &[],
    }
}
