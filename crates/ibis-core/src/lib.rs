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
pub use expr::{
    AggFunc, Analytic, BinaryOp, Bound, Expr, Frame, Func, IntoOperand, Lambda, SortKey, UnaryOp, WindowSpec,
};
pub use table::{DerefMap, GroupedTable, How, JoinKind, JoinPred, Rel, RelOp, SetKind, Side, Table};
pub use value::Value;

/// Everything needed to write queries.
pub mod prelude {
    pub use crate::backend::Backend;
    pub use crate::datatypes::{DataType, Schema};
    pub use crate::expr::{
        Expr, WindowSpec, array, asc, cases, coalesce, col, count, date, date_from_ymd, desc, e, greatest, least, lit,
        lit_as, map, null, pi, range, row_number, struct_, timestamp, timestamp_from_ymdhms,
    };
    pub use crate::table::{How, JoinKind, Table};
    pub use crate::value::Value;
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
    fn nested_types_round_trip_through_their_ibis_spelling() {
        for text in [
            "array<int64>",
            "array<array<string>>",
            "map<string, int64>",
            "struct<a: float64, b: string, c: array<int8>>",
            "decimal(10, 2)",
            "timestamp('UTC')",
        ] {
            assert_eq!(text.parse::<DataType>().unwrap().to_string(), text);
        }
        assert_eq!("!array<!int8>".parse::<DataType>().unwrap(), DataType::array(DataType::Int8));
        assert_eq!("timestamp(6)".parse::<DataType>().unwrap(), DataType::Timestamp { tz: None });
        assert!("array<int64".parse::<DataType>().is_err());
        assert!("list<int64>".parse::<DataType>().is_err());
    }

    #[test]
    fn nested_ops_are_typed_like_ibis() {
        let s = Schema::of(&[
            ("xs", DataType::array(DataType::Int64)),
            ("m", DataType::map(DataType::String, DataType::Int64)),
            ("st", DataType::structure(&[("a", DataType::Float64), ("b", DataType::String)])),
            ("name", DataType::String),
        ]);
        let t = |e: Expr| e.dtype(&s).unwrap().to_string();
        assert_eq!(t(lit(vec![1, 2, 3])), "array<int8>");
        assert_eq!(t(array([lit(1.5), col("xs").at(0)])), "array<float64>");
        // one name, three types
        assert_eq!(t(col("xs").length()), "int64");
        assert_eq!(t(col("m").length()), "int64");
        assert_eq!(t(col("name").length()), "int32");
        assert_eq!(t(col("xs").contains(1)), "boolean");
        assert_eq!(t(col("m").contains("k")), "boolean");
        assert_eq!(t(col("xs").at(-1)), "int64");
        assert_eq!(t(col("xs") + lit(vec![1])), "array<int64>");
        assert_eq!(t(col("xs") * 2), "array<int64>");
        assert_eq!(t(col("xs").means()), "float64");
        assert_eq!(t(col("xs").zip([array([col("name")])])), "array<struct<f1: int64, f2: string>>");
        assert_eq!(t(col("st").field("b")), "string");
        assert_eq!(t(struct_([("x", col("xs")), ("n", lit(1))])), "struct<x: array<int64>, n: int8>");
        assert_eq!(t(col("m").get("k")), "int64");
        assert_eq!(t(col("m").get_or("k", 1.5)), "float64");
        assert_eq!(t(col("m").keys()), "array<string>");
        assert_eq!(t(map(col("m").keys(), col("xs"))), "map<string, int64>");
        assert_eq!(t(range(0, 300, 1)), "array<int16>");

        let err = col("st").field("nope").dtype(&s).unwrap_err().to_string();
        assert!(err.contains("no field \"nope\""), "{err}");
        assert!(col("name").at(0).dtype(&s).is_err());
    }

    #[test]
    fn nested_values_round_trip_through_arrow() {
        use crate::convert::{batch_to_rows, rows_to_batch};
        let schema = Schema::of(&[
            ("xs", DataType::array(DataType::array(DataType::Int64))),
            ("m", DataType::map(DataType::String, DataType::Float64)),
            ("st", DataType::structure(&[("a", DataType::Int64), ("b", DataType::array(DataType::String))])),
        ]);
        let rows = vec![
            vec![
                Value::Array(vec![vec![1, 2].into(), Value::Null, Value::Array(vec![])]),
                Value::Map(vec![("a".into(), 1.5.into()), ("b".into(), Value::Null)]),
                Value::Struct(vec![("a".into(), 1.into()), ("b".into(), vec![Value::from("x"), Value::Null].into())]),
            ],
            vec![Value::Null, Value::Null, Value::Null],
            vec![
                Value::Array(vec![]),
                Value::Map(vec![]),
                Value::Struct(vec![("a".into(), Value::Null), ("b".into(), Value::Null)]),
            ],
        ];
        let batch = rows_to_batch(&schema, &rows).unwrap();
        assert_eq!(Schema::from_arrow(&batch.schema()).unwrap(), schema);
        assert_eq!(batch_to_rows(&batch).unwrap(), rows);
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
