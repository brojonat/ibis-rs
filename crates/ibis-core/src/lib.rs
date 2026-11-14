//! ibis-core: the backend-agnostic half of ibis-rs.
//!
//! Build a query with [`Table`] and [`Expr`]; it becomes an IR tree
//! ([`Rel`]/[`Expr`]) that a [`Backend`] plugin compiles and executes.

pub mod backend;
pub mod convert;
pub mod datatypes;
pub mod error;
pub mod expr;
pub mod table;
pub mod value;

pub use backend::Backend;
pub use datatypes::{DataType, Field, Schema};
pub use error::{Error, Result};
pub use expr::{AggFunc, Analytic, BinaryOp, Bound, Expr, Frame, Func, SortKey, UnaryOp, WindowSpec};
pub use table::{DerefMap, GroupedTable, JoinKind, JoinPred, Rel, RelOp, SetKind, Side, Table};
pub use value::Value;

/// Everything needed to write queries.
pub mod prelude {
    pub use crate::backend::Backend;
    pub use crate::datatypes::{DataType, Schema};
    pub use crate::expr::{Expr, WindowSpec, asc, cases, coalesce, col, count, date, desc, lit, null, row_number};
    pub use crate::table::{JoinKind, Table};
}

#[cfg(test)]
mod tests {
    use super::prelude::*;

    fn alltypes() -> Table {
        Table::named(
            "functional_alltypes",
            Schema::of(&[
                ("id", DataType::Int32),
                ("int_col", DataType::Int32),
                ("double_col", DataType::Float64),
                ("string_col", DataType::String),
                ("timestamp_col", DataType::Timestamp { tz: None }),
            ]),
        )
    }

    #[test]
    fn schema_flows_through_ops() {
        let t = alltypes()
            .filter([col("int_col").gt(2)])
            .unwrap()
            .mutate([(col("int_col") / 2).name("half")])
            .unwrap()
            .group_by(["string_col"])
            .agg([col("half").mean().name("avg"), count().name("n")])
            .unwrap();
        assert_eq!(t.schema().to_string(), "string_col: string, avg: float64, n: int64");
    }

    #[test]
    fn unknown_column_is_an_error_at_build_time() {
        let err = alltypes().filter([col("nope").gt(1)]).unwrap_err();
        assert!(err.to_string().contains("no column \"nope\""), "{err}");
    }

    #[test]
    fn ibis_semantics_in_types() {
        let s = alltypes().schema().clone();
        assert_eq!((col("int_col") / 2).dtype(&s).unwrap(), DataType::Float64);
        assert_eq!(col("double_col").ceil().dtype(&s).unwrap(), DataType::Int64);
        assert_eq!((col("string_col") + lit("a")).dtype(&s).unwrap(), DataType::String);
        assert_eq!(col("string_col").length().dtype(&s).unwrap(), DataType::Int32);
    }

    #[test]
    fn join_renames_collisions() {
        let t = alltypes();
        let j = t.join(&t, [("id", "id")], JoinKind::Inner).unwrap();
        let names: Vec<_> = j.schema().names().collect();
        assert_eq!(names[..5], ["id", "int_col", "double_col", "string_col", "timestamp_col"]);
        assert_eq!(names[5..], ["int_col_right", "double_col_right", "string_col_right", "timestamp_col_right"]);
    }

    #[test]
    fn mutate_replaces_in_place() {
        let t = alltypes().mutate([(col("int_col") + 1).name("int_col")]).unwrap();
        assert_eq!(
            t.schema().names().collect::<Vec<_>>(),
            ["id", "int_col", "double_col", "string_col", "timestamp_col"]
        );
    }
}
