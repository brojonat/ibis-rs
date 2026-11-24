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

pub use backend::{Backend, CreateOptions, scratch_name};
pub use datatypes::{DataType, Field, IntervalUnit, Schema};
pub use error::{Error, Result};
pub use expr::{
    AggFunc, AggOptions, Analytic, BinaryOp, Bound, Expr, Frame, Func, IntoOperand, Lambda, Shift, SortKey, UnaryOp,
    WindowSpec,
};
pub use table::{DerefMap, GroupedTable, How, JoinKind, JoinPred, Rel, RelOp, SetKind, Side, Table};
pub use value::Value;

/// Everything needed to write queries.
pub mod prelude {
    pub use crate::backend::{Backend, CreateOptions};
    pub use crate::datatypes::{DataType, IntervalUnit, Schema};
    pub use crate::expr::{
        Expr, WindowSpec, array, asc, cases, coalesce, col, count, date, date_from_ymd, desc, e, greatest, interval,
        least, lit, lit_as, map, null, nunique, pi, range, row_number, struct_, timestamp, timestamp_from_ymdhms,
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
            "struct<left join: int64, select: string>",
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
    fn not_null_is_a_fact_about_a_stored_column_only() {
        let mut schema = Schema::of(&[("a", DataType::Int64), ("b", DataType::Int64)]);
        schema.fields[0].nullable = false;
        let t = Table::named("t", schema);
        assert!(!t.schema().fields[0].nullable);
        // what a query returns may be NULL: an outer join pads the column
        for derived in [t.filter([col("a").gt(1)]).unwrap(), t.select([col("a")]).unwrap(), t.limit(1, 0)] {
            assert!(derived.schema().fields.iter().all(|f| f.nullable), "{:?}", derived.schema());
        }
    }

    #[test]
    fn aliases_are_found_through_the_expression() {
        let t = Table::named("t", Schema::of(&[("a", DataType::Int64)]));
        let inner = t.alias("inner");
        let outer = inner.filter([col("a").gt(1)]).unwrap().alias("outer");
        let names = |t: &Table| t.aliases().unwrap().into_iter().map(|(name, _)| name).collect::<Vec<_>>();
        assert_eq!(names(&outer), ["inner", "outer"]);
        // the same table on both sides of a join is one alias; two tables under one name are an error
        assert_eq!(names(&inner.join(&inner.view(), [("a", "a")], JoinKind::Inner).unwrap()), ["inner"]);
        assert!(inner.join(&t.limit(1, 0).alias("inner"), [("a", "a")], JoinKind::Inner).unwrap().aliases().is_err());
        // a column of the table is still a column of its alias
        assert!(inner.select([t.col("a")]).is_ok());
        assert!(t.as_scalar().is_ok());
        assert!(t.mutate([col("a").name("b")]).unwrap().as_scalar().is_err());
    }

    #[test]
    fn reductions_have_ibis_types() {
        let t = alltypes();
        let dtype = |e: Expr| t.aggregate([e.name("x")]).unwrap().schema().fields[0].dtype.to_string();
        assert_eq!(dtype(col("int_col").median()), "float64");
        assert_eq!(dtype(col("string_col").median()), "string");
        assert_eq!(dtype(col("int_col").mode()), "int32");
        assert_eq!(dtype(col("int_col").bit_xor()), "int32");
        assert_eq!(dtype(col("string_col").approx_nunique()), "int64");
        assert_eq!(dtype(col("int_col").kurtosis()), "float64");
        assert!(t.aggregate([col("double_col").bit_and().name("x")]).is_err());
        assert_eq!(dtype(col("string_col").argmin("id")), "string");
        assert_eq!(dtype(col("int_col").gt(1).corr(col("double_col"))), "float64");
        assert!(t.aggregate([col("string_col").cov("id").name("x")]).is_err());
        assert_eq!(dtype(col("int_col").quantile(0.5)), "float64");
        assert_eq!(dtype(col("string_col").quantile(0.5)), "string");
        assert_eq!(dtype(col("int_col").approx_quantiles([0.25, 0.75])), "array<float64>");
        // the key of `argmin` is bound like any column, and is in the default name
        let by_key = t.aggregate([col("int_col").argmin(t.col("id"))]).unwrap();
        assert_eq!(by_key.schema().to_string(), "ArgMin(int_col, id): int32");
        // the condition of `notany` is the condition of the `any` it negates
        let filtered = col("int_col").gt(0).notany().filter_where(col("id").lt(5));
        assert!(format!("{filtered:?}").contains("filter: Some"), "{filtered:?}");
        // over a window, `first` is the first value of the frame
        let w = WindowSpec::new().order_by([asc(col("id"))]);
        assert!(format!("{:?}", col("int_col").first().over(w)).contains("FirstValue"));
        // ... read in the reduction's own order, after the window's
        let w = WindowSpec::new().order_by([asc(col("id"))]);
        let ordered = format!("{:?}", col("int_col").last().order_by([col("year").desc()]).over(w));
        assert!(ordered.contains("LastValue") && ordered.contains("year"), "{ordered}");
        assert_eq!(dtype(nunique()), "int64");
        assert_eq!(dtype(col("string_col").collect().distinct().include_null()), "array<string>");
        let kept = format!("{:?}", col("string_col").collect().distinct().include_null());
        assert!(kept.contains("distinct: true, include_null: true"), "{kept}");
    }

    #[test]
    fn intervals_move_timestamps_and_dates() {
        use IntervalUnit::*;
        let t = alltypes();
        let dtype = |e: Expr| t.select([e.name("x")]).map(|t| t.schema().fields[0].dtype.to_string());
        let (ts, date) = (|| col("timestamp_col"), || col("timestamp_col").date());
        assert_eq!(dtype(ts() + interval(4, Day)).unwrap(), "timestamp");
        assert_eq!(dtype(interval(1, Hour) + ts()).unwrap(), "timestamp");
        assert_eq!(dtype(date() - col("int_col").as_interval(Month)).unwrap(), "date");
        // a date takes days and longer
        assert!(dtype(date() + interval(2, Hour)).is_err());
        // differences are intervals; a sum of intervals counts the finer unit
        assert_eq!(dtype(ts() - ts()).unwrap(), "interval('s')");
        assert_eq!(dtype(date() - date()).unwrap(), "interval('D')");
        assert_eq!(dtype(interval(4, Day) + interval(2, Hour)).unwrap(), "interval('h')");
        assert_eq!("interval('ms')".parse::<DataType>().unwrap(), DataType::Interval(Millisecond));

        // weeks are days and quarters months; a difference turns its right side back
        let steps = (interval(2, Week) - col("int_col").as_interval(Quarter) * 2).shifts().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!((steps[0].literal(), steps[0].unit, steps[0].back), (Some(14), Day, false));
        assert_eq!((steps[1].literal(), steps[1].unit, steps[1].back), (None, Month, true));
        assert!(col("int_col").shifts().is_err());
    }

    #[test]
    fn value_counts_and_order_by_follow_ibis() {
        let t = alltypes();
        let counts = t.value_counts(col("int_col") + 1).unwrap();
        assert_eq!(counts.schema().to_string(), "Add(int_col, 1): int64, Add(int_col, 1)_count: int64");
        assert_eq!(
            t.topk(col("string_col"), 3).unwrap().schema().to_string(),
            "string_col: string, string_col_count: int64"
        );
        // an aggregate has no `id` to sort by, though the table it reads does
        let n = t.aggregate([count().name("n")]).unwrap();
        assert!(n.order_by([t.col("id")]).is_err());
        assert!(t.filter([col("id").gt(1)]).unwrap().order_by([t.col("id")]).is_ok());
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
