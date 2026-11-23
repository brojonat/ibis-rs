// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! Ibis's tests of SQL given as text (ibis/backends/tests/test_dot_sql.py):
//! `con.sql(query)`, `table.sql(query)` and `table.alias(name)`.
//!
//! Ibis writes each query once, in DuckDB's SQL, and has sqlglot translate
//! it for the backend. ibis-rs passes SQL on as written, so here [`native`]
//! does by hand the little these queries need.

use std::collections::HashMap;

use ibis_core::convert::batch_to_rows;
use ibis_core::prelude::*;
use ibis_core::{Result, Value};

use crate::Target;
use crate::client::{ClientCase, case, check};

fn test(id: &str, run: impl Fn(&dyn Backend, &Target) -> Result<()> + Send + Sync + 'static) -> ClientCase {
    let mut c = case(id, run);
    c.module = "test_dot_sql";
    c
}

/// A query written in DuckDB's SQL, in the SQL of the backend: how names
/// are quoted, and what two of the types are called.
fn native(b: &dyn Backend, sql: &str) -> String {
    match b.name() {
        "mysql" => sql.replace('"', "`").replace("AS BIGINT)", "AS SIGNED)"),
        "spark" | "impala" => sql.replace('"', "`"),
        "postgres" => sql.replace("AS DOUBLE)", "AS DOUBLE PRECISION)"),
        _ => sql.to_string(),
    }
}

fn rows(b: &dyn Backend, t: &Table) -> Result<Vec<Vec<Value>>> {
    let mut rows = vec![];
    for batch in b.execute(t)? {
        rows.extend(batch_to_rows(&batch)?);
    }
    Ok(rows)
}

/// Rows in an order of their own, for results the query does not order.
fn sorted(mut rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
    rows.sort_by_cached_key(|row| format!("{row:?}"));
    rows
}

/// Equal, with floats equal to nine digits (Ibis compares these results
/// with `pytest.approx` or pandas's `assert_series_equal`).
fn close(got: &[Vec<Value>], want: &[Vec<Value>]) -> bool {
    let same = |a: &Value, b: &Value| match (a, b) {
        (Value::Float(x), Value::Float(y)) => (x - y).abs() <= 1e-9 * x.abs().max(y.abs()),
        _ => a == b,
    };
    got.len() == want.len()
        && got.iter().zip(want).all(|(g, w)| g.len() == w.len() && g.iter().zip(w).all(|(a, b)| same(a, b)))
}

fn check_close(got: &[Vec<Value>], want: &[Vec<Value>]) -> Result<()> {
    check(close(got, want), || {
        let show = |rows: &[Vec<Value>]| format!("{} rows, the first {:?}", rows.len(), &rows[..rows.len().min(5)]);
        format!("want {}; got {}", show(want), show(got))
    })
}

const S_NEW_COL: &str = r#"SELECT "string_col" AS "s", "double_col" + 1.0 AS "new_col" FROM "functional_alltypes""#;

const S_NEW_COL_CAST: &str = r#"
    SELECT
      "string_col" AS "s",
      "double_col" + CAST(1.0 AS DOUBLE) AS "new_col"
    FROM "functional_alltypes""#;

const ABC: &str = r#"SELECT * FROM (SELECT 'abc' "ts") "x""#;

const X_FROM_ALIAS: &str = r#"WITH "x" AS (SELECT * FROM "alias") SELECT * FROM "x""#;

/// What `S_NEW_COL` selects, as an expression.
fn s_new_col(target: &Target) -> Result<Table> {
    target.fx.alltypes.select([col("string_col").name("s"), (col("double_col") + lit(1.0)).name("new_col")])
}

/// `ts` is the one column, and 'abc' its one value.
fn check_abc(b: &dyn Backend, t: &Table) -> Result<()> {
    let names: Vec<String> = t.schema().names().map(|n| n.to_lowercase()).collect();
    check(names == ["ts"], || format!("the columns are {names:?}, not [\"ts\"]"))?;
    let got = rows(b, t)?;
    check(got == [[Value::from("abc")]], || format!("want one row of 'abc', got {got:?}"))
}

pub(crate) fn cases() -> Vec<ClientCase> {
    let mut v = vec![];

    for id in ["implicit_schema", "explicit_schema"] {
        v.push(test(&format!("test_con_dot_sql[{id}]"), move |b, target| {
            let query = native(b, S_NEW_COL);
            let t = match id {
                // the schema is given, so the backend is not asked for it
                "explicit_schema" => {
                    Table::sql(&query, Schema::of(&[("s", DataType::String), ("new_col", DataType::Float64)]))
                }
                _ => b.sql(&query)?,
            };
            let yas = |t: &Table| -> Result<Vec<Vec<Value>>> {
                let t = t.group_by(["s"]).agg([col("new_col").max().name("yas")])?;
                rows(b, &t.order_by(["yas"])?.select(["yas"])?)
            };
            check_close(&yas(&t)?, &yas(&s_new_col(target)?)?)
        }));
    }

    v.push(test("test_table_dot_sql", |b, target| {
        let t = b
            .sql_over(&target.fx.alltypes, &native(b, S_NEW_COL_CAST))?
            .group_by(["s"]) // a column from the SQL
            .agg([col("new_col").mean().name("fancy_af")])?
            .alias("awesome_t"); // a name for the aggregate
        let t = b.sql_over(&t, &native(b, r#"SELECT "fancy_af" AS "yas" FROM "awesome_t""#))?.order_by(["yas"])?;
        let want = s_new_col(target)?.group_by(["s"]).agg([col("new_col").mean().name("yas")])?;
        check_close(&rows(b, &t)?, &rows(b, &want.order_by(["yas"])?.select(["yas"])?)?)
    }));

    v.push(test("test_table_dot_sql_with_join", |b, target| {
        let join = r#"
            SELECT
              "l"."fancy_af" AS "yas",
              "r"."s" AS "s"
            FROM "awesome_t" AS "l"
            LEFT JOIN "ft2" AS "r"
            ON "l"."s" = "r"."s""#;
        let t = b
            .sql_over(&target.fx.alltypes, &native(b, S_NEW_COL_CAST))?
            .alias("ft2")
            .group_by(["s"])
            .agg([col("new_col").mean().name("fancy_af")])?
            .alias("awesome_t");
        let t = b.sql_over(&t, &native(b, join))?.order_by(["s", "yas"])?;
        // each row of the table, with the mean of its group
        let ft = s_new_col(target)?;
        let means = rows(b, &ft.group_by(["s"]).agg([col("new_col").mean().name("yas")])?)?;
        let means: HashMap<String, Value> =
            means.into_iter().map(|row| (format!("{:?}", row[0]), row[1].clone())).collect();
        let mut want = rows(b, &ft.select(["s"])?.order_by(["s"])?)?;
        for row in &mut want {
            row.insert(0, means[&format!("{:?}", row[0])].clone());
        }
        check_close(&rows(b, &t)?, &want)
    }));

    v.push(test("test_dot_sql_alias_with_params", |b, target| {
        let x = target.fx.alltypes.select([(col("string_col") + lit(" abc")).name("x")])?;
        check_close(&sorted(rows(b, &x.alias("foo"))?), &sorted(rows(b, &x)?))
    }));

    v.push(test("test_dot_sql_reuse_alias_with_different_types", |b, target| {
        for column in ["string_col", "bigint_col"] {
            let x = target.fx.alltypes.select([col(column).name("x")])?;
            let aliased = x.alias("foo");
            check(aliased.schema() == x.schema(), || format!("{column}: {} became {}", x.schema(), aliased.schema()))?;
            check_close(&sorted(rows(b, &aliased)?), &sorted(rows(b, &x)?))?;
        }
        Ok(())
    }));

    // Ibis's test reads the astronauts table, which is not among the
    // fixtures here: the same query shape over awards_players. The SQL has
    // an ORDER BY and `con.sql` adds no projection of its own, so the rows
    // come back in that order.
    let order = test("test_order_by_no_projection", |b, target| {
        let expr = target
            .fx
            .awards_players
            .group_by(["playerID"])
            .agg([count().name("nbr_awards")])?
            .order_by([desc(col("nbr_awards"))])?;
        let top = |t: &Table| -> Result<Vec<Vec<Value>>> {
            Ok(rows(b, t)?.into_iter().take(2).map(|r| r[1..].to_vec()).collect())
        };
        let got = top(&b.sql(&b.compile(&expr)?)?)?;
        check_close(&got, &top(&expr)?)
    });
    // Ibis marks Polars too: there is no SQL to compile an expression to
    v.push(order.notimpl(&["polars"]));

    v.push(test("test_dot_sql_limit", |b, _| {
        let t = b.sql(&native(b, ABC))?.limit(1, 0);
        check_abc(b, &t)
    }));

    v.push(test("test_cte", |b, target| {
        let query = r#"SELECT "string_col", CAST(COUNT(*) AS BIGINT) "n" FROM "ft" GROUP BY "string_col""#;
        let t = b.sql_over(&target.fx.alltypes.alias("ft"), &native(b, query))?;
        let want = target.fx.alltypes.group_by(["string_col"]).agg([count().name("n")])?;
        check_close(&sorted(rows(b, &t)?), &sorted(rows(b, &want)?))
    }));

    v.push(test("test_bare_minimum", |b, target| {
        let query = r#"SELECT COUNT(*) AS "n" FROM "functional_alltypes""#;
        let got = rows(b, &b.sql_over(&target.fx.alltypes, &native(b, query))?)?;
        let want = rows(b, &target.fx.alltypes.aggregate([count().name("n")])?)?;
        check_close(&got, &want)
    }));

    v.push(test("test_embedded_cte", |b, target| {
        let query = r#"WITH "x" AS (SELECT * FROM "functional_alltypes") SELECT * FROM "x""#;
        let got = rows(b, &b.sql_over(&target.fx.alltypes, &native(b, query))?.head(1))?;
        check(got.len() == 1, || format!("want 1 row, got {}", got.len()))
    }));

    v.push(test("test_embedded_cte_with_alias_simple", |b, _| {
        let t = b.sql(&native(b, ABC))?.alias("alias");
        check_abc(b, &b.sql_over(&t, &native(b, X_FROM_ALIAS))?.head(1))
    }));

    v.push(test("test_embedded_cte_with_alias_nested", |b, _| {
        let second = r#"WITH "second_alias" as (SELECT * FROM (SELECT 'abc' "ts") "x") SELECT * FROM "second_alias""#;
        let t = b.sql(&native(b, second))?.alias("alias");
        check_abc(b, &b.sql_over(&t, &native(b, X_FROM_ALIAS))?.head(1))
    }));

    v.push(test("test_unnamed_columns", |b, _| {
        let t = b.sql(&native(b, r#"SELECT 'a', 1 AS "col42""#))?;
        let fields = &t.schema().fields;
        check(fields.len() == 2, || format!("want 2 columns, got {}", t.schema()))?;
        check(!fields[0].name.is_empty(), || "the first column has no name".to_string())?;
        check(fields[1].name == "col42", || format!("the second column is {:?}", fields[1].name))?;
        check(fields[0].dtype == DataType::String, || format!("the first column is {}", fields[0].dtype))?;
        check(fields[1].dtype.is_integer(), || format!("the second column is {}", fields[1].dtype))
    }));

    v.push(test("test_scalar_dot_sql", |b, _| {
        let scalar = b.sql(&native(b, r#"SELECT 1 AS "a""#))?.as_scalar()?;
        let dtype = scalar.dtype(&Schema::new(vec![]))?;
        check(dtype.is_numeric(), || format!("the scalar is {dtype}"))
    }));

    v
}
