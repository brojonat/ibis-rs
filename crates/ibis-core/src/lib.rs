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
    AggFunc, AggOptions, Analytic, AsTimestamp, BinaryOp, Bound, Expr, Frame, Func, IntoOperand, Lambda, Shift,
    SortKey, UnaryOp, WindowSpec,
};
pub use table::{DerefMap, GroupedTable, How, JoinKind, JoinPred, Rel, RelOp, SetKind, Side, Table};
pub use value::Value;

/// Everything needed to write queries.
pub mod prelude {
    pub use crate::backend::{Backend, CreateOptions};
    pub use crate::datatypes::{DataType, IntervalUnit, Schema};
    pub use crate::expr::{
        Expr, WindowSpec, array, asc, cases, coalesce, col, count, date, date_from_ymd, desc, e, greatest, interval,
        least, lit, lit_as, map, now, null, nunique, param, pi, random, range, row_number, struct_, timestamp,
        timestamp_from_ymdhms, today,
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
        // a string has characters at positions; a struct has not
        assert_eq!(t(col("name").at(0)), "string");
        assert!(col("st").at(0).dtype(&s).is_err());
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
    fn bits_have_ibis_types() {
        let t = alltypes();
        let dtype = |e: Expr| t.select([e.name("x")]).map(|t| t.schema().fields[0].dtype.to_string());
        // the wider of the two integers; a shift is an int64 whatever is shifted
        assert_eq!(dtype(col("int_col").bitwise_and(3)).unwrap(), "int32");
        assert_eq!(dtype(lit(4).bitwise_xor(2)).unwrap(), "int8");
        assert_eq!(dtype(lit(4).lshift(2)).unwrap(), "int64");
        assert_eq!(dtype(col("int_col").bitwise_not()).unwrap(), "int32");
        assert!(dtype(col("double_col").bitwise_or(1)).is_err());
        assert_eq!(dtype(random()).unwrap(), "float64");
        // a histogram is arithmetic on the column and its range
        assert_eq!(dtype(col("int_col").histogram(10)).unwrap(), "int64");
    }

    #[test]
    fn strings_take_the_forms_of_arrays() {
        let t = alltypes();
        let dtype = |e: Expr| t.select([e.name("x")]).map(|t| t.schema().fields[0].dtype.to_string());
        let s = || col("string_col");
        assert_eq!(dtype(s().split("/")).unwrap(), "array<string>");
        assert_eq!(dtype(s().re_split(r"\d+")).unwrap(), "array<string>");
        assert_eq!(dtype(s().levenshtein("kitten")).unwrap(), "int64");
        assert_eq!(dtype(s().find_in_set(["a", "b"])).unwrap(), "int64");
        assert_eq!(dtype(s().translate("01", "ab")).unwrap(), "string");
        assert_eq!(dtype(s().substitute([("a", "b")], None)).unwrap(), "string");
        assert!(dtype(col("int_col").split("/")).is_err());
        // Python's s[2], s[1:3] and s.concat(..) are the array functions, read as substrings
        assert_eq!(dtype(s().at(2)).unwrap(), "string");
        assert_eq!(dtype(s().slice(Some(1), Some(3))).unwrap(), "string");
        assert_eq!(dtype(s().concat([lit("a"), null(DataType::Null)])).unwrap(), "string");
        let form = |e: Expr| match e {
            Expr::Func(f, args) => {
                let types: Vec<DataType> = args.iter().map(|a| a.dtype(t.schema()).unwrap()).collect();
                crate::expr::string_form(f, &args, &types).map(|e| e.to_string())
            }
            _ => None,
        };
        assert_eq!(form(s().at(-2)).unwrap(), "Substring(string_col, -2, 1)");
        assert_eq!(form(s().slice(Some(1), Some(3))).unwrap(), "Substring(string_col, 1, Sub(3, 1))");
        assert_eq!(form(s().slice(None, None)).unwrap(), "string_col");
        // sep.join(strings) is a join of strings; with one array either way round, of the array
        assert_eq!(form(lit("-").join([s(), s()])), None);
        assert_eq!(dtype(lit("-").join([s(), s()])).unwrap(), "string");
        let arr = || array([lit("a"), lit("b")]);
        assert_eq!(form(lit(",").join([arr()])), form(arr().join([lit(",")])));
        assert_eq!(dtype(arr().join([lit(",")])).unwrap(), "string");
        // an array is still an array
        assert_eq!(form(arr().at(0)), None);
    }

    #[test]
    fn a_window_in_a_filter_is_computed_first() {
        let t = alltypes();
        let dtype = |e: Expr| t.select([e.name("x")]).map(|t| t.schema().fields[0].dtype.to_string());
        assert_eq!(dtype(col("int_col").identical_to(col("double_col"))).unwrap(), "boolean");
        assert_eq!(dtype(col("string_col").try_cast(DataType::Int64)).unwrap(), "int64");
        assert_eq!(dtype(col("int_col").typeof_()).unwrap(), "string");
        assert_eq!(dtype(col("double_col").hash()).unwrap(), "int64");
        assert_eq!(dtype(col("string_col").hexdigest()).unwrap(), "string");
        assert!(dtype(col("int_col").hexdigest()).is_err());
        // the filtered table has the columns it had, and the window is under the filter
        let mean = col("int_col").mean().over(WindowSpec::new().partition_by(["id"]));
        let kept = t.filter([mean.gt(3), col("id").gt(0)]).unwrap();
        assert_eq!(kept.schema(), t.schema());
        assert!(t.filter([col("int_col").mean().over(WindowSpec::new())]).is_err(), "a predicate is boolean");
    }

    #[test]
    fn a_slice_from_the_end_numbers_the_rows() {
        let t = alltypes();
        let op = |t: &Table| format!("{:?}", t.rel().op).split_whitespace().next().unwrap_or("").to_string();
        // from the front: a limit and an offset
        assert_eq!(op(&t.slice(Some(3), Some(5)).unwrap()), "Limit");
        assert_eq!(op(&t.slice(Some(3), None).unwrap()), "Limit");
        // empty whatever the table holds
        assert_eq!(op(&t.slice(Some(-3), Some(-5)).unwrap()), "Limit");
        // from the back: numbered, filtered, and the columns it had
        let last = t.order_by(["id"]).unwrap().slice(Some(-3), None).unwrap();
        assert_eq!(op(&last), "Project");
        assert_eq!(last.schema(), t.schema());
        assert_eq!(t.slice(None, Some(-2)).unwrap().schema(), t.schema());
        assert!(t.sample(1.5).is_err());
        assert_eq!(t.sample(0.5).unwrap().schema(), t.schema());
    }

    #[test]
    fn a_parameter_takes_its_value_when_bound() {
        let t = alltypes();
        let p = param(DataType::Date);
        assert_eq!(p.dtype(t.schema()).unwrap(), DataType::Date);
        let open = t.filter([col("int_col").gt(0)]).unwrap().select([col("id"), p.clone().name("day")]).unwrap();
        let has_param = |t: &Table| format!("{:?}", t.rel().op).contains("_param_");
        assert!(has_param(&open));
        // the value is cast to the parameter's type, and the table is the same table otherwise
        let bound = open.bind([(p.clone(), lit("2009-03-01"))]).unwrap();
        assert!(!has_param(&bound));
        assert!(format!("{:?}", bound.rel().op).contains("Cast("), "{:?}", bound.rel().op);
        assert_eq!((bound.rel().id, bound.schema()), (open.rel().id, open.schema()));
        // a value for something that is not a parameter is a mistake
        assert!(open.bind([(col("id"), lit(1))]).is_err());
        // two parameters are two parameters
        assert_ne!(param(DataType::Int64), param(DataType::Int64));
    }

    #[test]
    fn epochs_and_deltas_have_ibis_types() {
        use IntervalUnit::*;
        let t = alltypes();
        let dtype = |e: Expr| t.select([e.name("x")]).map(|t| t.schema().fields[0].dtype.to_string());
        let ts = || col("timestamp_col");
        assert_eq!(dtype(ts().iso_year()).unwrap(), "int32");
        assert_eq!(dtype(ts().epoch_seconds()).unwrap(), "int32");
        assert_eq!(dtype(ts().date().epoch_days()).unwrap(), "int64");
        assert_eq!(dtype(ts().delta(timestamp("2020-01-01 00:00:00"), Hour)).unwrap(), "int64");
        // a date and a timestamp have no delta
        assert!(dtype(ts().delta(date("2020-01-01"), Day)).is_err());
        assert_eq!(dtype(col("int_col").as_timestamp(Millisecond)).unwrap(), "timestamp");
        assert!(dtype(col("string_col").as_timestamp(Second)).is_err());

        // a count of any unit becomes a count of microseconds; a float keeps its fraction
        let micros = |unit, t: DataType| ibis_core_micros(&col("x"), &t, unit);
        assert_eq!(micros(Second, DataType::Int32), "Mul(Cast(x, int64), 1000000)");
        assert_eq!(micros(Nanosecond, DataType::Int64), "FloorDiv(Cast(x, int64), 1000)");
        assert_eq!(micros(Second, DataType::Float64), "Floor(Mul(Cast(x, float64), 1000000))");
        assert!(crate::expr::epoch_micros(&col("x"), &DataType::Int64, Day).is_err());
        // the ends of a delta are cut back to the start of the unit
        let (unit, start, end) = crate::expr::delta_ends(&[col("b"), col("a"), lit("h")]).unwrap();
        assert_eq!(
            (unit, start.to_string(), end.to_string()),
            (Hour, r#"Truncate(a, "h")"#.into(), r#"Truncate(b, "h")"#.into())
        );
    }

    #[test]
    fn buckets_and_formats() {
        use IntervalUnit::*;
        let t = alltypes();
        let dtype = |e: Expr| t.select([e.name("x")]).map(|t| t.schema().fields[0].dtype.to_string());
        let ts = || col("timestamp_col");
        assert_eq!(dtype(ts().bucket(interval(5, Minute))).unwrap(), "timestamp");
        assert!(dtype(col("int_col").bucket(interval(5, Minute))).is_err());
        // one method, as in Ibis: a unit for a number, a pattern for a string
        assert_eq!(dtype(col("string_col").as_timestamp("%m/%d/%y")).unwrap(), "timestamp('UTC')");
        assert_eq!(dtype(col("string_col").as_date("%m/%d/%y")).unwrap(), "date");
        assert!(dtype(col("int_col").as_date("%m/%d/%y")).is_err());

        // a bucket is the microseconds since 1970 rounded down to the width, the offset aside
        let bucket = |width: Expr, offset: Option<Expr>| {
            let args: Vec<Expr> = [ts(), width].into_iter().chain(offset).collect();
            crate::expr::bucket_expr(&args).map(|e| e.to_string())
        };
        let plain = bucket(interval(1, Week), None).unwrap();
        assert!(plain.contains("604800000000") && !plain.contains("Sub("), "{plain}");
        let offset = bucket(interval(5, Minute), Some(interval(-2, Minute))).unwrap();
        assert!(offset.contains("300000000") && offset.contains("-120000000"), "{offset}");
        // a month has no fixed length, and the width is a literal
        assert!(bucket(interval(1, Month), None).is_err());
        assert!(bucket(col("int_col").as_interval(Day), None).is_err());
        assert!(bucket(interval(0, Day), None).is_err());

        let parsed = |s, f| crate::convert::strptime(s, f).map(crate::convert::format_timestamp);
        assert_eq!(parsed("11/01/10", "%m/%d/%y").unwrap(), "2010-11-01 00:00:00");
        assert_eq!(parsed("2010-11-01 13:05:09", "%Y-%m-%d %H:%M:%S").unwrap(), "2010-11-01 13:05:09");
        assert_eq!(parsed("November", "%m/%d/%y"), None);
    }

    fn ibis_core_micros(x: &Expr, t: &DataType, unit: IntervalUnit) -> String {
        crate::expr::epoch_micros(x, t, unit).unwrap().to_string()
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
