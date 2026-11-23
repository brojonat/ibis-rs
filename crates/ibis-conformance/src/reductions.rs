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

    v
}
