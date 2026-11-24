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

/// `nulls`: the reduction keeps NULLs, so with no order to read the rows
/// in it may as well pick one.
fn check_picks_30(b: &dyn Backend, target: &Target, filtered: bool, nulls: bool, pick: fn(Expr) -> Expr) -> Result<()> {
    let (t, filter) = one_value(target, filtered)?;
    let mut picked = pick(col("new"));
    if nulls {
        picked = picked.include_null();
    }
    if let Some(filter) = filter {
        picked = picked.filter_where(filter);
    }
    let got = value(b, &t.aggregate([picked.name("tmp")])?)?;
    check(got == Value::Int(30) || (nulls && got == Value::Null), || format!("want 30, got {got:?}"))
}

/// `collect` of string_col with "3" made NULL, checked against the rows.
fn check_collect(b: &dyn Backend, target: &Target, [distinct, filtered, ordered, nulls]: [bool; 4]) -> Result<()> {
    let t = target.fx.alltypes.mutate([col("string_col").nullif(lit("3")).name("x")])?;
    let mut e = col("x").collect();
    if filtered {
        e = e.filter_where((col("id") % 13).eq(0));
    }
    if nulls {
        e = e.include_null();
    }
    if distinct {
        e = e.distinct();
    }
    if ordered {
        e = e.order_by([col("x").desc()]);
    }
    let Value::Array(mut got) = value(b, &t.aggregate([e.name("tmp")])?)? else {
        return check(false, || "want an array".to_string());
    };

    let mut want = vec![];
    for row in rows(b, &t.select([col("x"), col("id")])?)? {
        let passes = !filtered || matches!(row[1], Value::Int(id) if id % 13 == 0);
        let counts = nulls || row[0] != Value::Null;
        if passes && counts && !(distinct && want.contains(&row[0])) {
            want.push(row[0].clone());
        }
    }
    // largest first and NULLs last: the order asked for, where one is
    let text = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        _ => None,
    };
    want.sort_by_key(|v| std::cmp::Reverse(text(v)));
    if !ordered {
        got.sort_by_key(|v| std::cmp::Reverse(text(v)));
    }
    check(got == want, || {
        format!("want {} values {:?}..., got {} {:?}...", want.len(), want.first(), got.len(), got.first())
    })
}

/// Pearson's coefficient of G and RBI from sums the engine computes: what
/// `corr` should give, whichever of the two formulas the engine names.
fn check_corr(b: &dyn Backend, target: &Target, cond: bool) -> Result<()> {
    let filter = || col("yearID").isin([lit(2009), lit(2015)]);
    let mut e = col("G").corr("RBI");
    if cond {
        e = e.filter_where(filter());
    }
    let got = value(b, &target.fx.batting.aggregate([e.name("tmp")])?)?;

    let (x, y) = (|| col("G").cast(DataType::Float64), || col("RBI").cast(DataType::Float64));
    let mut both = target.fx.batting.filter([col("G").notnull(), col("RBI").notnull()])?;
    if cond {
        both = both.filter([filter()])?;
    }
    let sums = both.aggregate([
        count().name("n"),
        x().sum().name("sx"),
        y().sum().name("sy"),
        (x() * y()).sum().name("sxy"),
        (x() * x()).sum().name("sxx"),
        (y() * y()).sum().name("syy"),
    ])?;
    let sums: Vec<f64> = rows(b, &sums)?[0]
        .iter()
        .map(|v| match v {
            Value::Int(i) => *i as f64,
            Value::Float(f) => *f,
            _ => f64::NAN,
        })
        .collect();
    let [n, sx, sy, sxy, sxx, syy] = sums[..] else { return check(false, || "want six sums".to_string()) };
    let want = (n * sxy - sx * sy) / ((n * sxx - sx * sx) * (n * syy - sy * sy)).sqrt();
    check(matches!(got, Value::Float(r) if (r - want).abs() < 1e-6), || format!("want {want}, got {got:?}"))
}

pub(crate) fn cases() -> Vec<ClientCase> {
    let mut v = vec![];

    let id = |b: bool| if b { "True" } else { "False" };
    let methods = [("first", Expr::first as fn(Expr) -> Expr), ("last", Expr::last)];

    // ids are include_null-filtered-method
    for filtered in [false, true] {
        for (method, pick) in methods {
            for nulls in [false, true] {
                let run = move |b: &dyn Backend, target: &Target| check_picks_30(b, target, filtered, nulls, pick);
                let case = test(&format!("test_first_last[{}-{}-{method}]", id(nulls), id(filtered)), run);
                // ClickHouse's `any` and `anyLast` pass over NULLs. Ibis marks
                // Postgres too, and Spark with a condition; both do it here.
                let none: &[&str] = if nulls { &["impala", "mysql", "clickhouse"] } else { &["impala", "mysql"] };
                v.push(case.notimpl(none));
            }
        }
        let run = move |b: &dyn Backend, target: &Target| check_picks_30(b, target, filtered, false, Expr::arbitrary);
        v.push(test(&format!("test_arbitrary[{}]", id(filtered)), run).notimpl(&["impala", "mysql"]));
    }

    // int_col runs 0 to 9; read largest first, `new` is NULL at both ends
    for (nulls_id, nulls) in [("exclude-null", false), ("include-null", true)] {
        for (filtered_id, filtered) in [("not-filtered", false), ("filtered", true)] {
            for (method, pick) in methods {
                let last = method == "last";
                let run = move |b: &dyn Backend, target: &Target| {
                    let int_col = || col("int_col");
                    let t = target.fx.alltypes.mutate([int_col().nullif(0).nullif(9).name("new")])?;
                    let mut e = pick(col("new")).order_by([int_col().desc()]);
                    if filtered {
                        e = e.filter_where(int_col().ne(if last { 1 } else { 8 }));
                    }
                    if nulls {
                        e = e.include_null();
                    }
                    let got = value(b, &t.aggregate([e.name("tmp")])?)?;
                    let want = match (nulls, filtered, last) {
                        (true, _, _) => Value::Null,
                        (_, true, true) => Value::Int(2),
                        (_, true, false) => Value::Int(7),
                        (_, false, true) => Value::Int(1),
                        (_, false, false) => Value::Int(8),
                    };
                    check(got == want, || format!("want {want:?}, got {got:?}"))
                };
                let case = test(&format!("test_first_last_ordered[{nulls_id}-{filtered_id}-{method}]"), run);
                // no aggregate of ClickHouse or Spark reads its rows in an order
                v.push(case.notimpl(&["impala", "mysql", "clickhouse", "spark"]));
            }
        }
    }

    // In a projection the reduction is a window function over all the
    // rows (https://github.com/ibis-project/ibis/issues/11656).
    let in_mutate: [(&str, fn() -> Expr, i64); 4] = [
        ("first_asc", || col("val").first().order_by(["ob"]), 4),
        ("last_asc", || col("val").last().order_by(["ob"]), 5),
        ("first_desc", || col("val").first().order_by([col("ob").desc()]), 5),
        ("last_desc", || col("val").last().order_by([col("ob").desc()]), 4),
    ];
    for (method, pick, want) in in_mutate {
        let run = move |b: &dyn Backend, target: &Target| {
            let (val, ob) = (|| col("val"), || col("ob"));
            let t = target
                .fx
                .alltypes
                .select([col("tinyint_col").name("a"), col("int_col").name("val"), col("bigint_col").name("ob")])?
                .filter([(val().eq(4) & ob().eq(40)) | (val().eq(5) & ob().eq(50))])?;
            let got = rows(b, &t.mutate([pick().name("new")])?.limit(10, 0).select([col("new")])?)?;
            check(got == vec![vec![Value::Int(want)]; 10], || format!("want ten rows of {want}, got {got:?}"))
        };
        v.push(test(&format!("test_first_last_ordered_in_mutate[{method}]"), run));
    }

    // ids are distinct-filtered-ordered-include_null
    for flags in 0..16u8 {
        let [distinct, filtered, ordered, nulls] = [8, 4, 2, 1].map(|bit| flags & bit == 0);
        let run =
            move |b: &dyn Backend, target: &Target| check_collect(b, target, [distinct, filtered, ordered, nulls]);
        let case = test(&format!("test_collect[{}-{}-{}-{}]", id(distinct), id(filtered), id(ordered), id(nulls)), run);
        // ClickHouse and Spark gather values in no order and pass over NULLs
        let none: &[&str] = match ordered || nulls {
            true => &["impala", "mysql", "sqlite", "clickhouse", "spark"],
            false => &["impala", "mysql", "sqlite"],
        };
        v.push(case.notimpl(none));
    }

    // The distinct rows of three columns, against DISTINCT and a count.
    // Ibis marks MySQL with a condition; it does it here.
    for (cond_id, cond) in [("no_cond", false), ("cond", true)] {
        let run = move |b: &dyn Backend, target: &Target| {
            let t = target.fx.alltypes.select(["int_col", "double_col", "string_col"])?;
            let filter = || col("string_col").isin([lit("1"), lit("7")]);
            let (e, kept) = match cond {
                true => (nunique().filter_where(filter()), t.filter([filter()])?),
                false => (nunique(), t.clone()),
            };
            let got = value(b, &t.aggregate([e.name("tmp")])?)?;
            let want = value(b, &kept.distinct().aggregate([count()])?)?;
            check(got == want, || format!("want {want:?}, got {got:?}"))
        };
        v.push(test(&format!("test_count_distinct_star[{cond_id}]"), run).notimpl(&["sqlite"]));
    }

    // The engines that name the sample's correlation; the population's has
    // a golden result (`cases/aggregation.rs`). Ids are cond-function.
    for (cond_id, cond) in [("no_cond", false), ("cond", true)] {
        let run = move |b: &dyn Backend, target: &Target| check_corr(b, target, cond);
        let case = test(&format!("test_corr_cov[{cond_id}-corr_samp]"), run);
        v.push(case.notimpl(&["duckdb", "postgres", "trino", "impala", "mysql", "sqlite"]));
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
