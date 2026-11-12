//! Query cases built with the DataFusion DataFrame API.
//!
//! Each case is the kind of thing an ibis-style API would emit. `ordered`
//! means the case has a total ORDER BY and row order must match; otherwise
//! rows are compared as a multiset.

use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, IntervalMonthDayNano};
use datafusion::common::ScalarValue;
use datafusion::error::Result;
use datafusion::functions_aggregate::expr_fn::{
    avg, count, count_distinct, max, median, min, stddev, sum, var_sample,
};
use datafusion::functions_aggregate::count::count_all;
use datafusion::functions_aggregate::{average::avg_udaf, sum::sum_udaf};
use datafusion::functions_window::expr_fn::{dense_rank, lag, rank, row_number};
use datafusion::logical_expr::expr::WindowFunction;
use datafusion::logical_expr::{
    ExprFunctionExt, WindowFrame, WindowFrameBound, WindowFrameUnits, in_subquery,
    scalar_subquery, when,
};
use datafusion::prelude::*;

pub struct Tables {
    pub o: DataFrame,
    pub c: DataFrame,
}

pub struct Case {
    pub name: &'static str,
    pub ordered: bool,
    pub build: fn(&Tables) -> Result<DataFrame>,
}

const fn case(name: &'static str, build: fn(&Tables) -> Result<DataFrame>) -> Case {
    Case { name, ordered: false, build }
}
const fn ordered(name: &'static str, build: fn(&Tables) -> Result<DataFrame>) -> Case {
    Case { name, ordered: true, build }
}

fn asc(e: Expr) -> datafusion::logical_expr::SortExpr {
    e.sort(true, false)
}

pub fn all() -> Vec<Case> {
    vec![
        // --- projection / filter -------------------------------------------
        case("select_columns", |t| t.o.clone().select_columns(&["id", "amount"])),
        case("filter_gt", |t| t.o.clone().filter(col("amount").gt(lit(50.0)))),
        case("filter_and_or_null", |t| {
            t.o.clone().filter(
                col("amount")
                    .gt(lit(20.0))
                    .and(col("status").eq(lit("shipped")))
                    .or(col("customer_id").is_null()),
            )
        }),
        case("filter_not_between", |t| {
            t.o.clone().filter(col("quantity").between(lit(0), lit(3)).not())
        }),
        case("in_list", |t| {
            t.o.clone()
                .filter(col("status").in_list(vec![lit("shipped"), lit("returned")], false))
        }),
        case("bool_column", |t| t.o.clone().filter(col("is_gift"))?.select_columns(&["id", "is_gift"])),
        case("distinct", |t| t.o.clone().select_columns(&["status"])?.distinct()),
        // --- arithmetic / casts --------------------------------------------
        case("arithmetic", |t| {
            t.o.clone().select(vec![
                col("id"),
                (col("amount") * col("quantity")).alias("total"),
                (col("amount") - lit(1)).alias("minus_one"),
                (-col("quantity")).alias("neg"),
            ])
        }),
        case("int_division", |t| {
            t.o.clone().select(vec![col("id"), (col("quantity") / lit(3)).alias("q")])
        }),
        case("float_division", |t| {
            t.o.clone().select(vec![col("id"), (col("amount") / lit(4.0)).alias("q")])
        }),
        case("modulo_negative", |t| {
            t.o.clone().select(vec![col("id"), (col("quantity") % lit(3)).alias("m")])
        }),
        case("cast_float_to_int", |t| {
            t.o.clone().select(vec![col("id"), cast(col("amount"), DataType::Int64).alias("a")])
        }),
        case("cast_int_to_string", |t| {
            t.o.clone().select(vec![col("id"), cast(col("quantity"), DataType::Utf8).alias("s")])
        }),
        case("math_funcs", |t| {
            t.o.clone().select(vec![
                col("id"),
                round(vec![col("amount"), lit(1)]).alias("r"),
                abs(col("amount")).alias("a"),
                ceil(col("amount")).alias("c"),
                floor(col("amount")).alias("f"),
                sqrt(abs(col("amount"))).alias("s"),
                power(col("quantity"), lit(2)).alias("p"),
            ])
        }),
        case("ln_exp", |t| {
            t.o.clone().filter(col("amount").gt(lit(0.0)))?.select(vec![
                col("id"),
                ln(col("amount")).alias("l"),
                exp(lit(1.0)).alias("e"),
            ])
        }),
        // --- conditionals ---------------------------------------------------
        case("case_when", |t| {
            t.o.clone().select(vec![
                col("id"),
                when(col("amount").gt(lit(100.0)), lit("big"))
                    .when(col("amount").gt(lit(0.0)), lit("small"))
                    .otherwise(lit("neg_or_null"))?
                    .alias("size"),
            ])
        }),
        case("coalesce", |t| {
            t.o.clone().select(vec![col("id"), coalesce(vec![col("amount"), lit(0.0)]).alias("a")])
        }),
        case("nullif", |t| {
            t.o.clone().select(vec![col("id"), nullif(col("quantity"), lit(0)).alias("q")])
        }),
        // --- strings --------------------------------------------------------
        case("string_case_length", |t| {
            t.c.clone().select(vec![
                col("id"),
                upper(col("name")).alias("u"),
                lower(col("name")).alias("l"),
                character_length(col("name")).alias("n"),
            ])
        }),
        case("string_concat_nulls", |t| {
            t.c.clone().select(vec![
                col("id"),
                concat(vec![col("name"), lit("-"), col("region")]).alias("c"),
            ])
        }),
        case("string_concat_op", |t| {
            // `||` operator; NULL-propagating in standard SQL
            t.c.clone().select(vec![col("id"), binary_expr(col("name"), datafusion::logical_expr::Operator::StringConcat, col("region")).alias("c")])
        }),
        case("substr", |t| {
            t.c.clone().select(vec![col("id"), substr(col("name"), lit(2)).alias("s")])
        }),
        case("substring_len", |t| {
            t.c.clone().select(vec![col("id"), substring(col("name"), lit(2), lit(3)).alias("s")])
        }),
        case("trim_replace", |t| {
            t.c.clone().select(vec![
                col("id"),
                btrim(vec![col("name")]).alias("t"),
                replace(col("name"), lit("a"), lit("A")).alias("r"),
            ])
        }),
        case("like", |t| t.c.clone().filter(col("name").like(lit("%a%")))),
        case("ilike", |t| t.c.clone().filter(col("name").ilike(lit("%A%")))),
        case("starts_with", |t| t.c.clone().filter(starts_with(col("name"), lit("A")))),
        case("regexp_like", |t| {
            t.c.clone().filter(regexp_like(col("name"), lit("^[A-E]"), None))
        }),
        case("strpos", |t| {
            t.c.clone().select(vec![col("id"), strpos(col("name"), lit("a")).alias("p")])
        }),
        // --- aggregates -----------------------------------------------------
        case("agg_basic", |t| {
            t.o.clone().aggregate(
                vec![],
                vec![
                    count(col("amount")).alias("n"),
                    count_all().alias("n_all"),
                    sum(col("amount")).alias("s"),
                    avg(col("amount")).alias("a"),
                    min(col("amount")).alias("lo"),
                    max(col("amount")).alias("hi"),
                ],
            )
        }),
        case("agg_int_avg", |t| {
            t.o.clone().aggregate(vec![], vec![avg(col("quantity")).alias("a"), sum(col("quantity")).alias("s")])
        }),
        case("count_distinct", |t| {
            t.o.clone().aggregate(vec![], vec![count_distinct(col("customer_id")).alias("n")])
        }),
        case("group_by", |t| {
            t.o.clone().aggregate(
                vec![col("status")],
                vec![sum(col("amount")).alias("s"), count_all().alias("n")],
            )
        }),
        case("group_by_having", |t| {
            t.o.clone()
                .aggregate(vec![col("customer_id")], vec![count_all().alias("n")])?
                .filter(col("n").gt(lit(4)))
        }),
        case("group_by_expr", |t| {
            t.o.clone().aggregate(vec![lower(col("status")).alias("st")], vec![count_all().alias("n")])
        }),
        case("stddev_var", |t| {
            t.o.clone().aggregate(
                vec![],
                vec![stddev(col("amount")).alias("sd"), var_sample(col("amount")).alias("v")],
            )
        }),
        case("median", |t| t.o.clone().aggregate(vec![], vec![median(col("amount")).alias("m")])),
        case("nested_agg", |t| {
            t.o.clone()
                .aggregate(vec![col("customer_id")], vec![sum(col("amount")).alias("total")])?
                .aggregate(vec![], vec![avg(col("total")).alias("avg_total"), max(col("total")).alias("max_total")])
        }),
        // --- sorting --------------------------------------------------------
        ordered("sort_limit_offset", |t| {
            t.o.clone()
                .filter(col("amount").is_not_null())?
                .sort(vec![col("amount").sort(false, false), asc(col("id"))])?
                .limit(2, Some(5))
        }),
        ordered("sort_nulls_first", |t| {
            t.o.clone()
                .select_columns(&["id", "amount"])?
                .sort(vec![col("amount").sort(true, true), asc(col("id"))])
        }),
        ordered("sort_nulls_default_desc", |t| {
            t.o.clone()
                .select_columns(&["id", "amount"])?
                .sort(vec![col("amount").sort(false, false), asc(col("id"))])
        }),
        // --- joins ----------------------------------------------------------
        case("inner_join", |t| {
            t.o.clone()
                .join(t.c.clone(), JoinType::Inner, &["customer_id"], &["id"], None)?
                .select(vec![col("orders.id"), col("customers.name"), col("amount")])
        }),
        case("left_join", |t| {
            t.o.clone()
                .join(t.c.clone(), JoinType::Left, &["customer_id"], &["id"], None)?
                .select(vec![col("orders.id"), col("customers.name"), col("customers.region")])
        }),
        case("join_then_agg", |t| {
            t.o.clone()
                .join(t.c.clone(), JoinType::Inner, &["customer_id"], &["id"], None)?
                .aggregate(vec![col("region")], vec![sum(col("amount")).alias("s")])
        }),
        case("self_join", |t| {
            let a = t.o.clone().alias("a")?;
            let b = t.o.clone().alias("b")?;
            a.join_on(
                b,
                JoinType::Inner,
                vec![col("a.customer_id").eq(col("b.customer_id")), col("a.id").lt(col("b.id"))],
            )?
            .aggregate(vec![], vec![count_all().alias("pairs")])
        }),
        case("anti_join", |t| {
            t.c.clone()
                .join(t.o.clone(), JoinType::LeftAnti, &["id"], &["customer_id"], None)
        }),
        // --- set ops --------------------------------------------------------
        case("union_all", |t| {
            t.o.clone().select_columns(&["id"])?.union(t.c.clone().select_columns(&["id"])?)
        }),
        case("union_distinct", |t| {
            t.o.clone()
                .select_columns(&["customer_id"])?
                .union_distinct(t.c.clone().select(vec![col("id").alias("customer_id")])?)
        }),
        // --- windows --------------------------------------------------------
        case("window_row_number", |t| {
            t.o.clone().select(vec![
                col("id"),
                row_number()
                    .partition_by(vec![col("customer_id")])
                    .order_by(vec![asc(col("created_at")), asc(col("id"))])
                    .build()?
                    .alias("rn"),
            ])
        }),
        case("window_rank", |t| {
            t.o.clone().select(vec![
                col("id"),
                rank().order_by(vec![asc(col("status"))]).build()?.alias("r"),
                dense_rank().order_by(vec![asc(col("status"))]).build()?.alias("dr"),
            ])
        }),
        case("window_lag", |t| {
            t.o.clone().select(vec![
                col("id"),
                lag(col("amount"), Some(1), None)
                    .order_by(vec![asc(col("id"))])
                    .build()?
                    .alias("prev"),
            ])
        }),
        case("window_cumsum", |t| {
            t.o.clone().select(vec![
                col("id"),
                Expr::from(WindowFunction::new(sum_udaf(), vec![col("quantity")]))
                    .partition_by(vec![col("status")])
                    .order_by(vec![asc(col("id"))])
                    .build()?
                    .alias("cum"),
            ])
        }),
        case("window_moving_avg", |t| {
            t.o.clone().select(vec![
                col("id"),
                Expr::from(WindowFunction::new(avg_udaf(), vec![col("quantity")]))
                    .order_by(vec![asc(col("id"))])
                    .window_frame(WindowFrame::new_bounds(
                        WindowFrameUnits::Rows,
                        WindowFrameBound::Preceding(ScalarValue::UInt64(Some(2))),
                        WindowFrameBound::CurrentRow,
                    ))
                    .build()?
                    .alias("ma"),
            ])
        }),
        case("window_partition_total", |t| {
            t.o.clone().select(vec![
                col("id"),
                Expr::from(WindowFunction::new(sum_udaf(), vec![col("amount")])).partition_by(vec![col("status")]).build()?.alias("tot"),
            ])
        }),
        case("filter_on_window", |t| {
            // "latest order per customer": the QUALIFY pattern
            t.o.clone()
                .select(vec![
                    col("id"),
                    col("customer_id"),
                    row_number()
                        .partition_by(vec![col("customer_id")])
                        .order_by(vec![col("created_at").sort(false, false), asc(col("id"))])
                        .build()?
                        .alias("rn"),
                ])?
                .filter(col("rn").eq(lit(1)))?
                .select_columns(&["id", "customer_id"])
        }),
        // --- temporal -------------------------------------------------------
        case("date_trunc_month", |t| {
            t.o.clone()
                .select(vec![col("id"), date_trunc(lit("month"), col("created_at")).alias("m")])
        }),
        case("extract_parts", |t| {
            t.o.clone().select(vec![
                col("id"),
                date_part(lit("year"), col("created_at")).alias("y"),
                date_part(lit("month"), col("created_at")).alias("mo"),
                date_part(lit("dow"), col("created_at")).alias("dow"),
            ])
        }),
        case("timestamp_plus_interval", |t| {
            let week = ScalarValue::IntervalMonthDayNano(Some(IntervalMonthDayNano::new(0, 7, 0)));
            t.o.clone().select(vec![col("id"), (col("created_at") + lit(week)).alias("ts")])
        }),
        case("filter_timestamp_literal", |t| {
            t.o.clone().filter(
                col("created_at").gt(cast(lit("2024-01-01 00:00:00"), DataType::Timestamp(
                    datafusion::arrow::datatypes::TimeUnit::Nanosecond,
                    None,
                ))),
            )
        }),
        case("date_column", |t| {
            t.c.clone().filter(col("signup").lt(cast(lit("2024-01-01"), DataType::Date32)))
        }),
        // --- subqueries -----------------------------------------------------
        case("in_subquery", |t| {
            let west = t.c.clone().filter(col("region").eq(lit("west")))?.select_columns(&["id"])?;
            t.o.clone()
                .filter(in_subquery(col("customer_id"), Arc::new(west.into_unoptimized_plan())))
        }),
        case("scalar_subquery", |t| {
            let mean = t.o.clone().aggregate(vec![], vec![avg(col("amount"))])?;
            t.o.clone()
                .filter(col("amount").gt(scalar_subquery(Arc::new(mean.into_unoptimized_plan()))))
        }),
    ]
}
