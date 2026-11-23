// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! The tests of ibis/backends/tests/test_aggregation.py whose answer is the
//! engine's to choose (the first value of rows that have no order, some
//! value, an approximate median): Ibis checks a property of the result, and
//! so do these. The reductions with one right answer are cases with a
//! golden result (`cases/aggregation.rs`).

use ibis_core::prelude::*;
use ibis_core::{Result, Value};

use crate::Target;
use crate::client::{ClientCase, case, check, rows};

fn test(id: &str, run: impl Fn(&dyn Backend, &Target) -> Result<()> + Send + Sync + 'static) -> ClientCase {
    let mut c = case(id, run);
    c.module = "test_aggregation";
    c
}

/// The one value of a query of one row and one column.
fn value(b: &dyn Backend, t: &Table) -> Result<Value> {
    let got = rows(b, t)?;
    check(got.len() == 1 && got[0].len() == 1, || format!("want one value, got {got:?}"))?;
    Ok(got[0][0].clone())
}

/// A column that is NULL or one value (after the filter, if there is one),
/// so a reduction that picks a value can only pick that one: the column
/// `new`, the filter, and the value.
fn one_value(target: &Target, filtered: bool) -> Result<(Table, Option<Expr>)> {
    let int_col = || col("int_col");
    let (new, filter) = match filtered {
        true => (int_col().cases([(lit(3), lit(30)), (lit(4), lit(40))], None), Some(int_col().eq(3))),
        false => (int_col().eq(3).ifelse(lit(30), null(DataType::Int8)), None),
    };
    Ok((target.fx.alltypes.mutate([new.name("new")])?, filter))
}

fn check_picks_30(b: &dyn Backend, target: &Target, filtered: bool, pick: fn(Expr) -> Expr) -> Result<()> {
    let (t, filter) = one_value(target, filtered)?;
    let mut picked = pick(col("new"));
    if let Some(filter) = filter {
        picked = picked.filter_where(filter);
    }
    let got = value(b, &t.aggregate([picked.name("tmp")])?)?;
    check(got == Value::Int(30), || format!("want 30, got {got:?}"))
}

pub(crate) fn cases() -> Vec<ClientCase> {
    let mut v = vec![];

    // ids are include_null-filtered-method; `include_null=True` is not ported yet
    for filtered in [false, true] {
        for (method, pick) in [("first", Expr::first as fn(Expr) -> Expr), ("last", Expr::last)] {
            let filtered_id = if filtered { "True" } else { "False" };
            let run = move |b: &dyn Backend, target: &Target| check_picks_30(b, target, filtered, pick);
            v.push(test(&format!("test_first_last[False-{filtered_id}-{method}]"), run).notimpl(&["impala", "mysql"]));
        }
        let filtered_id = if filtered { "True" } else { "False" };
        let run = move |b: &dyn Backend, target: &Target| check_picks_30(b, target, filtered, Expr::arbitrary);
        v.push(test(&format!("test_arbitrary[{filtered_id}]"), run).notimpl(&["impala", "mysql"]));
    }

    v.push(
        test("test_approx_median", |b, target| {
            let t = target.fx.alltypes.aggregate([col("double_col").approx_median().name("tmp")])?;
            let dtype = &t.schema().fields[0].dtype;
            check(*dtype == DataType::Float64, || format!("the type is {dtype}"))?;
            let got = value(b, &t)?;
            check(matches!(got, Value::Float(_)), || format!("want a float, got {got:?}"))
        })
        .notimpl(&["mysql", "sqlite"]),
    );

    // ids are multi-filtered. An approximation: Ibis allows it 10 either way.
    for many in [false, true] {
        for filtered in [false, true] {
            let id = |b: bool| if b { "True" } else { "False" };
            let run = move |b: &dyn Backend, target: &Target| {
                let t = target.fx.mem("approx_quantile_x")?;
                let mut e = if many { col("x").approx_quantiles([0.25, 0.75]) } else { col("x").approx_quantile(0.25) };
                if filtered {
                    e = e.filter_where(col("x").le(100));
                }
                let got = value(b, &t.aggregate([e.name("tmp")])?)?;
                let want: &[f64] = match (many, filtered) {
                    (true, true) => &[25.0, 75.0],
                    (true, false) => &[50.0, 150.0],
                    (false, true) => &[25.0],
                    (false, false) => &[50.0],
                };
                let floats = match (&got, many) {
                    (Value::Array(items), true) => items.clone(),
                    (one, false) => vec![one.clone()],
                    _ => vec![],
                };
                let near = |(v, w): (&Value, &f64)| matches!(v, Value::Float(f) if (f - w).abs() <= 10.0);
                check(floats.len() == want.len() && floats.iter().zip(want).all(near), || {
                    format!("want floats within 10 of {want:?}, got {got:?}")
                })
            };
            let case = test(&format!("test_approx_quantile[{}-{}]", id(many), id(filtered)), run);
            // Ibis marks Polars for several fractions at once
            let none: &[&str] =
                if many { &["impala", "mysql", "sqlite", "polars"] } else { &["impala", "mysql", "sqlite"] };
            v.push(case.notimpl(none));
        }
    }

    // Every value of string_col occurs as often as every other, so which
    // three are on top is the engine's choice: Ibis checks the counts.
    v.push(test("test_topk_op", |b, target| {
        let t = target.fx.alltypes.order_by(["string_col"])?;
        let top = t.topk(col("string_col"), 3)?;
        let each = value(b, &target.fx.alltypes.filter([col("string_col").eq(lit("0"))])?.aggregate([count()])?)?;
        let got = rows(b, &top)?;
        let counts: Vec<&Value> = got.iter().map(|row| &row[1]).collect();
        check(counts == [&each; 3], || format!("want three counts of {each:?}, got {got:?}"))
    }));
    v.push(test("test_topk_filter_op[string_col_filter_top3]", |b, target| {
        let t = target.fx.alltypes.order_by(["string_col"])?;
        let top = t.topk(col("string_col"), 3)?;
        let kept = t.join(&top, [("string_col", "string_col")], JoinKind::Semi)?;
        let got = value(b, &kept.aggregate([count()])?)?;
        let each = value(b, &target.fx.alltypes.filter([col("string_col").eq(lit("0"))])?.aggregate([count()])?)?;
        let want = match each {
            Value::Int(n) => Value::Int(3 * n),
            other => other,
        };
        check(got == want, || format!("want {want:?} rows, got {got:?}"))
    }));

    v.push(test("test_agg_sort", |_, target| {
        let t = &target.fx.alltypes;
        let query = t.aggregate([count().name("count")])?;
        // `year` is a column of the table, not of its one-row aggregate
        let sorted = query.order_by([t.col("year")]);
        check(sorted.is_err(), || "ordering an aggregate by a column it does not have is not an error".to_string())
    }));

    v.push(test("test_agg_name_in_output_column", |b, target| {
        let query = target.fx.alltypes.aggregate([col("int_col").min(), col("int_col").max()])?;
        let names: Vec<String> = query.schema().names().map(str::to_lowercase).collect();
        check(names.len() == 2 && names[0].contains("min") && names[1].contains("max"), || format!("{names:?}"))?;
        check(rows(b, &query)?.len() == 1, || "want one row".to_string())
    }));

    v
}
