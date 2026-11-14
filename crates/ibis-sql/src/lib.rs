//! ibis-sql: compile the ibis-rs IR to SQL.
//!
//! SQL backends implement [`Dialect`], overriding only the ops their engine
//! spells differently, and call [`compile`].

pub mod compiler;
pub mod dialect;

pub use compiler::{ExprCompiler, compile, create_table_sql, quote_str};
pub use dialect::{Dialect, GenericDialect, default_type_name};

#[cfg(test)]
mod tests {
    use ibis_core::prelude::*;

    use super::*;

    fn t() -> Table {
        Table::named("t", Schema::of(&[("a", DataType::Int64), ("s", DataType::String)]))
    }

    fn sql(t: &Table) -> String {
        compile(&GenericDialect, t).unwrap()
    }

    #[test]
    fn filter_project_merge_into_one_select() {
        let q = t().filter([col("a").gt(1)]).unwrap().select([col("s").upper().name("u")]).unwrap();
        assert_eq!(sql(&q), r#"SELECT UPPER("s") AS "u" FROM "t" AS t0 WHERE ("a" > 1)"#);
    }

    #[test]
    fn filter_after_aggregate_wraps() {
        let q = t().group_by(["s"]).agg([count().name("n")]).unwrap().filter([col("n").gt(1)]).unwrap();
        assert_eq!(
            sql(&q),
            r#"SELECT * FROM (SELECT "s" AS "s", COUNT(*) AS "n" FROM "t" AS t0 GROUP BY "s") AS t1 WHERE ("n" > 1)"#
        );
    }

    #[test]
    fn order_then_limit_stay_together() {
        let q = t().order_by([desc("a")]).unwrap().limit(5, 2);
        assert_eq!(sql(&q), r#"SELECT * FROM "t" AS t0 ORDER BY "a" DESC NULLS LAST LIMIT 5 OFFSET 2"#);
    }

    #[test]
    fn ibis_semantics_compiled() {
        let q = t()
            .select([
                (col("a") / 2).name("div"),
                row_number().over(WindowSpec::new().order_by(["a"])).name("rn"),
                col("a").mean().name("broadcast"),
            ])
            .unwrap();
        assert_eq!(
            sql(&q),
            r#"SELECT (CAST("a" AS DOUBLE) / 2) AS "div", (ROW_NUMBER() OVER (ORDER BY "a" ASC NULLS LAST) - 1) AS "rn", AVG("a") OVER () AS "broadcast" FROM "t" AS t0"#
        );
    }

    fn u() -> Table {
        Table::named("u", Schema::of(&[("a", DataType::Int64), ("b", DataType::Float64)]))
    }

    #[test]
    fn correlated_exists_from_any_over_another_table() {
        let (t, u) = (t(), u());
        let q = t.filter([u.col("a").eq(t.col("a")).any()]).unwrap();
        assert_eq!(sql(&q), r#"SELECT * FROM "t" AS t0 WHERE (EXISTS (SELECT * FROM "u" AS t1 WHERE ("a" = t0."a")))"#);
    }

    #[test]
    fn reduction_in_filter_is_a_scalar_subquery() {
        let (t, u) = (t(), u());
        let q = t.filter([t.col("a").gt(u.filter([u.col("a").eq(t.col("a"))]).unwrap().col("b").mean())]).unwrap();
        assert_eq!(
            sql(&q),
            r#"SELECT * FROM "t" AS t0 WHERE ("a" > (SELECT AVG("b") AS "_" FROM "u" AS t1 WHERE ("a" = t0."a")))"#
        );
    }

    #[test]
    fn self_join_through_view_keeps_sides_apart() {
        let t = t();
        let v = t.view();
        let j = t.join(&v, [t.col("a").eq(v.col("a"))], JoinKind::Inner).unwrap();
        let q = j.select([t.col("s"), v.col("s").name("s2")]).unwrap();
        assert_eq!(
            sql(&q),
            r#"SELECT "s" AS "s", "s_right" AS "s2" FROM (SELECT t0."a" AS "a", t0."s" AS "s", t1."s" AS "s_right" FROM "t" AS t0 INNER JOIN "t" AS t1 ON (t0."a" = t1."a")) AS t2"#
        );
    }
}
