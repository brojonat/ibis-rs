//! ibis/backends/tests/test_conditionals.py

use ibis_core::prelude::*;

use super::{Case, c, column, scalar, table, tables};

pub fn register(v: &mut Vec<Case>) {
    v.push(table("test_ifelse_select", |t| {
        t.select([c("int_col"), t.col("int_col").eq(0).ifelse(42, -1).cast(DataType::Int64).name("where_col")])
    }));
    v.push(column("test_ifelse_column", |t| Ok(t.col("int_col").eq(0).ifelse(42, -1).cast(DataType::Int64))));
    // x.substitute({None: "400"}): NULLs become "400", other values stay.
    // value_counts(): group by the column, count as "<name>_count".
    v.push(table("test_substitute", |t| {
        let x = || c("string_col").nullif(lit("1"));
        t.select([cases([(x().isnull(), lit("400"))], Some(x())).name("subs")])?
            .group_by(["subs"])
            .agg([count().name("subs_count")])?
            .filter([c("subs").eq(lit("400"))])
    }));
    v.push(scalar("test_value_cases_scalar[one_kwarg]", || {
        Ok(lit(1).cases([(lit(1), lit("one")), (lit(2), lit("two"))], Some(lit("other"))))
    }));
    v.push(scalar("test_value_cases_scalar[fallthrough]", || {
        Ok(lit(5).cases([(lit(1), lit("one")), (lit(2), lit("two"))], None))
    }));
    v.push(tables("test_value_cases_column", |fx| {
        fx.batting.select([fx
            .batting
            .col("RBI")
            .cases([(lit(5), lit("five")), (lit(4), lit("four")), (lit(3), lit("three"))], Some(lit("could be good?")))
            .name("tmp")])
    }));
    v.push(scalar("test_ibis_cases_scalar", || Ok(lit(5).cases([(lit(5), lit("five")), (lit(4), lit("four"))], None))));
    v.push(tables("test_ibis_cases_column", |fx| {
        let t = &fx.batting;
        t.select([cases(
            [(t.col("RBI").lt(5), lit("really bad team")), (t.col("teamID").eq(lit("PH1")), lit("ph1 team"))],
            Some(t.col("teamID")),
        )
        .name("tmp")])
    }));
    // CASE x WHEN NULL never gets hit
    v.push(scalar("test_value_cases_null", || {
        Ok(lit(5).nullif(5).cases([(null(DataType::Null), lit("oops"))], Some(lit("expected"))))
    }));
}
