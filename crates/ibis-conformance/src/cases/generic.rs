// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! ibis/backends/tests/test_generic.py

use ibis_core::prelude::*;
use ibis_core::{Expr, Result, SortKey};

use super::{Case, c, column, scalar, table, tables};

fn untyped_null() -> Expr {
    null(DataType::Null)
}

/// `alltypes` with NULLs introduced into three columns.
fn nullable(t: &Table) -> Result<Table> {
    t.mutate([
        c("int_col").nullif(1).name("int_col"),
        c("double_col").nullif(3.0).name("double_col"),
        c("string_col").nullif(lit("2")).name("string_col"),
    ])?
    .select(["id", "int_col", "double_col", "string_col"])
}

/// All orderings of the four ops with `select` before `distinct`.
fn commute_orders() -> Vec<Vec<&'static str>> {
    fn go(rest: &[&'static str], acc: &mut Vec<&'static str>, out: &mut Vec<Vec<&'static str>>) {
        if rest.is_empty() {
            out.push(acc.clone());
        }
        for (i, op) in rest.iter().enumerate() {
            let mut next = rest.to_vec();
            next.remove(i);
            acc.push(op);
            go(&next, acc, out);
            acc.pop();
        }
    }
    let mut out = vec![];
    go(&["select", "distinct", "filter", "order_by"], &mut vec![], &mut out);
    let pos = |ops: &[&str], op: &str| ops.iter().position(|o| *o == op).unwrap();
    out.retain(|ops| pos(ops, "select") < pos(ops, "distinct"));
    out
}

pub fn register(v: &mut Vec<Case>) {
    let p = |test: &str, id: &str| format!("{test}[{id}]");

    v.push(scalar(&p("test_scalar_fill_null_nullif", "na_fill_null"), || Ok(untyped_null().fill_null(5))));
    v.push(scalar(&p("test_scalar_fill_null_nullif", "non_na_fill_null"), || Ok(lit(5).fill_null(10))));
    v.push(scalar(&p("test_scalar_fill_null_nullif", "nullif_null"), || Ok(lit(5).nullif(5))));
    v.push(scalar(&p("test_scalar_fill_null_nullif", "nullif_not_null"), || Ok(lit(10).nullif(5))));

    // SQLite has no NaN (it stores NULL) and neither has MySQL; Ibis marks
    // it notimpl on both.
    v.push(
        table(&p("test_isna", "nan_col"), |t| {
            t.select([lit(f64::NAN).name("nan_col")])?.filter([c("nan_col").isnan()])
        })
        .notimpl(&["sqlite", "mysql"]),
    );
    v.push(table(&p("test_isna", "none_col"), |t| {
        t.select([untyped_null().cast(DataType::Float64).name("none_col")])?.filter([c("none_col").isnull()])
    }));

    v.push(table("test_column_fill_null[None]", |t| {
        t.mutate([untyped_null().cast(DataType::Float64).name("missing")])?
            .mutate([c("missing").fill_null(0.0).name("missing")])
    }));

    v.push(scalar(&p("test_coalesce", "generic"), || Ok(coalesce([lit(5), untyped_null(), lit(4)]))));
    v.push(scalar(&p("test_coalesce", "null_start_end"), || Ok(coalesce([untyped_null(), lit(4), untyped_null()]))));
    #[allow(clippy::approx_constant)] // 3.14 is the value in the Ibis test
    v.push(scalar(&p("test_coalesce", "non_null_last"), || Ok(coalesce([untyped_null(), untyped_null(), lit(3.14)]))));

    let isin: [(&str, fn() -> Vec<Expr>); 6] = [
        ("int_col", || [1, 2, 3].map(lit).to_vec()),
        ("int_col", || [1, 2, 3].map(lit).to_vec()),
        ("string_col", || ["1", "2", "3"].map(lit).to_vec()),
        ("string_col", || ["1", "2", "3"].map(lit).to_vec()),
        ("int_col", || vec![lit(1)]),
        ("int_col", || vec![lit(1)]),
    ];
    for (i, (name, elements)) in isin.into_iter().enumerate() {
        v.push(
            table(&format!("test_isin[{name}-elements{i}]"), move |t| {
                let st = t.order_by(["id"])?;
                st.select([c("id"), st.col(name).isin(elements()).name("tmp")])?.order_by(["id"])
            })
            .ordered(),
        );
        v.push(
            table(&format!("test_notin[{name}-elements{i}]"), move |t| {
                let st = t.order_by(["id"])?;
                st.select([c("id"), st.col(name).notin(elements()).name("tmp")])?.order_by(["id"])
            })
            .ordered(),
        );
    }

    let filters: [(&str, fn(&Table) -> Expr); 5] = [
        ("no_op", |t| t.col("bool_col")),
        ("negate", |t| !t.col("bool_col")),
        ("and", |t| t.col("bool_col") & t.col("bool_col")),
        ("or", |t| t.col("bool_col") | t.col("bool_col")),
        ("xor", |t| t.col("bool_col") ^ t.col("bool_col")),
    ];
    for (id, pred) in filters {
        v.push(
            table(&p("test_filter", id), move |t| {
                let st = t.order_by(["id"])?;
                st.filter([pred(&st)])?.order_by(["id"])
            })
            .ordered(),
        );
    }

    v.push(table("test_case_where", |t| {
        t.mutate([cases([(c("int_col").eq(1), lit(20)), (c("int_col").eq(0), lit(10))], Some(lit(0)))
            .cast(DataType::Int64)
            .name("new_col")])
    }));

    v.push(
        table("test_select_filter_mutate", |t| {
            let t = t.mutate([cases([(c("bool_col"), c("float_col"))], Some(lit(f64::NAN))).name("float_col")])?;
            let t = t.select(t.schema().names().map(c).collect::<Vec<_>>())?;
            let t = t.filter([!c("float_col").isnan()])?;
            let t = t.mutate([c("float_col").cast(DataType::Float64).name("float_col")])?;
            t.select(["id", "float_col"])
        })
        .notimpl(&["sqlite", "mysql"]),
    );

    v.push(table(&p("test_table_fill_null_mapping", "int"), |t| {
        nullable(t)?.fill_null([("int_col", Value::from(20))])
    }));
    v.push(table(&p("test_table_fill_null_mapping", "double-int-str"), |t| {
        nullable(t)?.fill_null([("double_col", Value::from(-1)), ("string_col", Value::from("missing"))])
    }));
    v.push(table(&p("test_table_fill_null_mapping", "double-str"), |t| {
        nullable(t)?.fill_null([("double_col", Value::from(-1.5)), ("string_col", Value::from("missing"))])
    }));
    v.push(table(&p("test_table_fill_null_mapping", "empty"), |t| nullable(t)?.fill_null(Vec::<(&str, Value)>::new())));
    v.push(table("test_table_fill_null_scalar#1", |t| {
        nullable(t)?.select(["int_col", "double_col"])?.fill_null_with(0)
    }));
    v.push(table("test_table_fill_null_scalar#2", |t| nullable(t)?.select(["string_col"])?.fill_null_with("missing")));

    let subsets: [(&str, Option<&'static [&'static str]>); 5] = [
        ("none", None),
        ("empty", Some(&[])),
        ("single", Some(&["col_1"])),
        ("one-and-two", Some(&["col_1", "col_2"])),
        ("one-and-three", Some(&["col_1", "col_3"])),
    ];
    for (sid, subset) in subsets {
        for (hid, how) in [("any", How::Any), ("all", How::All)] {
            v.push(
                table(&format!("test_drop_null_table[{sid}-{hid}]"), move |t| {
                    let is_two = || c("int_col").eq(2);
                    let is_four = || c("int_col").eq(4);
                    let tbl = t
                        .mutate([
                            is_two().ifelse(untyped_null(), c("float_col")).name("col_1"),
                            is_four().ifelse(untyped_null(), c("float_col")).name("col_2"),
                            (is_two() | is_four()).ifelse(untyped_null(), c("float_col")).name("col_3"),
                        ])?
                        .select(["id", "col_1", "col_2", "col_3"])?;
                    tbl.drop_null(subset, how)?.order_by(["id"])
                })
                .ordered(),
            );
        }
    }

    let order_by: [(&str, fn() -> Vec<SortKey>); 6] = [
        ("id-df_kwargs0", || vec![asc("id")]),
        ("key1-df_kwargs1", || vec![c("id").asc()]),
        ("<lambda>-df_kwargs2", || vec![c("id").asc()]),
        ("key3-df_kwargs3", || vec![desc("id")]),
        ("key4-df_kwargs4", || vec![asc("id"), asc("int_col")]),
        ("key5-df_kwargs5", || vec![asc("id"), desc("int_col")]),
    ];
    for (id, keys) in order_by {
        v.push(table(&p("test_order_by", id), move |t| t.filter([c("id").lt(100)])?.order_by(keys())).ordered());
    }

    let by_b: [(&str, fn() -> SortKey); 2] = [("desc", || desc("b")), ("asc", || asc("b"))];
    for (op, key) in by_b {
        v.push(
            tables(&p("test_order_by_nulls_default", op), move |fx| fx.mem("order_nulls")?.order_by([key()])).ordered(),
        );
        v.push(
            tables(&p("test_order_by_nulls", op), move |fx| fx.mem("order_nulls")?.order_by([key().nulls_first()]))
                .ordered(),
        );
    }

    let two_cols: [(&str, bool, bool, bool); 5] = [
        // (id, nulls_first of col1, col2 descending, nulls_first of col2)
        ("asc-desc-ff", false, true, false),
        ("asc-desc-tt", true, true, true),
        ("asc-desc-tf", true, true, false),
        ("asc-asc-tt", true, false, true),
        ("asc-asc-tf", true, false, false),
    ];
    for (id, nf1, desc2, nf2) in two_cols {
        v.push(
            tables(&p("test_order_by_two_cols_nulls", id), move |fx| {
                let mut k1 = asc("col1");
                k1.nulls_first = nf1;
                let mut k2 = if desc2 { desc("col2") } else { asc("col2") };
                k2.nulls_first = nf2;
                fx.mem("order_two_cols")?.order_by([k1, k2])
            })
            .ordered(),
        );
    }

    v.push(table(&p("test_isin_notin", "isin"), |t| t.filter([c("string_col").isin(Vec::<Expr>::new())])));
    v.push(table(&p("test_isin_notin", "notin"), |t| t.filter([c("string_col").notin(Vec::<Expr>::new())])));
    v.push(table(&p("test_isin_notin", "isin_non_empty"), |t| {
        t.filter([(c("string_col").length() * 1).isin([lit(1)])])
    }));
    v.push(table(&p("test_isin_notin", "notin_non_empty"), |t| {
        t.filter([(c("string_col").length() * 1).notin([lit(1)])])
    }));

    let col_expr = "test_isin_notin_column_expr";
    v.push(
        table(&p(col_expr, "isin_col"), |t| t.filter([c("string_col").isin_col(c("string_col"))])?.order_by(["id"]))
            .ordered(),
    );
    v.push(
        table(&p(col_expr, "isin_expr"), |t| {
            t.filter([(c("bigint_col") + 1).isin_col(c("string_col").length() + 1)])?.order_by(["id"])
        })
        .ordered(),
    );
    v.push(
        table(&p(col_expr, "notin_col"), |t| t.filter([c("string_col").notin_col(c("string_col"))])?.order_by(["id"]))
            .ordered(),
    );
    v.push(
        table(&p(col_expr, "notin_expr"), |t| {
            t.filter([(c("bigint_col") + 1).notin_col(c("string_col").length() + 1)])?.order_by(["id"])
        })
        .ordered(),
    );

    v.push(scalar(&p("test_logical_negation_literal", "true_noop"), || Ok(lit(true))));
    v.push(scalar(&p("test_logical_negation_literal", "false_noop"), || Ok(lit(false))));
    v.push(scalar(&p("test_logical_negation_literal", "true_invert"), || Ok(!lit(true))));
    v.push(scalar(&p("test_logical_negation_literal", "false_invert"), || Ok(!lit(false))));
    v.push(column(&p("test_logical_negation_column", "identity"), |t| Ok(t.col("bool_col"))));
    v.push(column(&p("test_logical_negation_column", "invert"), |t| Ok(!t.col("bool_col"))));

    v.push(scalar("test_zero_ifnull_literals[int64-0-1]#1", || Ok(untyped_null().cast(DataType::Int64).fill_null(0))));
    v.push(scalar("test_zero_ifnull_literals[int64-0-1]#2", || Ok(lit_as(1, DataType::Int64).fill_null(0))));
    v.push(scalar("test_zero_ifnull_literals[float64-0.0-1.0]#1", || {
        Ok(untyped_null().cast(DataType::Float64).fill_null(0))
    }));
    v.push(scalar("test_zero_ifnull_literals[float64-0.0-1.0]#2", || Ok(lit_as(1.0, DataType::Float64).fill_null(0))));

    v.push(column("test_zero_ifnull_column", |_| Ok(c("int_col").nullif(1).fill_null(0))));
    v.push(table("test_select_filter", |t| {
        t.select(["int_col", "string_col"])?.filter([t.col("string_col").eq(lit("4"))])
    }));
    v.push(table("test_select_filter_select", |t| {
        t.select(["int_col", "string_col"])?.filter([t.col("string_col").eq(lit("4"))])?.select(["int_col"])
    }));
    v.push(column("test_between", |_| Ok(c("double_col").between(5, 10))));

    v.push(tables("test_uncorrelated_subquery", |fx| {
        let batting = &fx.batting;
        let subset_batting = batting.filter([batting.col("yearID").le(2000)])?;
        batting.filter([c("yearID").eq(subset_batting.col("yearID").max())])?.select(["playerID", "yearID"])
    }));

    for (method, negate) in [("any", false), ("notany", true)] {
        v.push(tables(&p("test_exists", method), move |fx| {
            let batting = fx.batting.filter([fx.batting.col("yearID").isin([1871, 1980, 1981].map(lit))])?;
            let awards_players =
                fx.awards_players.filter([fx.awards_players.col("yearID").isin([1980, 1981].map(lit))])?;
            let cond = batting.col("yearID").eq(awards_players.col("yearID"));
            batting.filter([if negate { cond.notany() } else { cond.any() }])
        }));
    }

    v.push(
        tables("test_isin_uncorrelated_simple", |fx| {
            let (u1, a) = (fx.mem("ids_123")?, fx.mem("ids_12")?);
            u1.mutate([u1.col("id").isin_col(a.col("id")).name("in_a")])?.order_by(["id"])
        })
        .ordered()
        // Impala takes IN (subquery) in WHERE only; marked notyet in Ibis too
        .notimpl(&["impala"]),
    );
    v.push(
        tables("test_isin_uncorrelated", |fx| {
            fx.batting
                .select([
                    c("playerID"),
                    c("yearID"),
                    fx.batting.col("yearID").isin_col(fx.awards_players.col("yearID")).name("has_year_id"),
                ])?
                .order_by(["playerID", "yearID"])
        })
        .ordered()
        .notimpl(&["impala"]),
    );
    v.push(
        tables("test_isin_uncorrelated_filter", |fx| {
            fx.batting
                .select(["playerID", "yearID"])?
                .filter([fx.batting.col("yearID").isin_col(fx.awards_players.col("yearID"))])?
                .order_by(["playerID", "yearID"])
        })
        .ordered(),
    );

    v.push(
        table("test_select_distinct_order_by", |t| t.select(["int_col"])?.distinct().order_by(["int_col"])).ordered(),
    );
    v.push(
        table("test_select_distinct_order_by_expr", |t| t.select(["int_col"])?.distinct().order_by([-c("int_col")]))
            .ordered(),
    );

    for ops in commute_orders() {
        let id = format!("test_select_distinct_filter_order_by_commute[{}]", ops.join("-"));
        v.push(
            table(&id, move |t| {
                let mut expr = t.select([c("int_col"), c("float_col"), (t.col("id") % 33).name("b")])?;
                for op in &ops {
                    expr = match *op {
                        "select" => expr.select(["int_col", "b"])?,
                        "distinct" => expr.distinct(),
                        "filter" => expr.filter([expr.col("int_col").gt(5)])?,
                        _ => expr.order_by([(-expr.col("int_col")).asc(), expr.col("b").asc()])?,
                    };
                }
                Ok(expr)
            })
            .ordered(),
        );
    }

    v.push(scalar(&p("test_cast", "int_to_float"), || Ok(lit_as(0, DataType::Int64).cast(DataType::Float64))));
    v.push(scalar(&p("test_cast", "float_to_int"), || Ok(lit_as(0.0, DataType::Float64).cast(DataType::Int64))));
    v.push(scalar(&p("test_cast", "string_to_int"), || Ok(lit("0").cast(DataType::Int64))));
    v.push(scalar(&p("test_cast", "string_to_float"), || Ok(lit("0").cast(DataType::Float64))));

    // Python slices as (limit, offset); None = no limit.
    let slices: [(&str, Option<u64>, u64); 10] = [
        ("[:0]", Some(0), 0),
        ("[:]", None, 0),
        ("[0:0]", Some(0), 0),
        ("[0:]", None, 0),
        ("[:2]", Some(2), 0),
        ("[0:2]", Some(2), 0),
        ("[3:0]", Some(0), 3),
        ("[3:]", None, 3),
        ("[3:2]", Some(0), 3),
        ("[3:4]", Some(1), 3),
    ];
    for (id, limit, offset) in slices {
        v.push(table(&p("test_static_table_slice", id), move |t| {
            let rows = match limit {
                Some(n) => t.limit(n, offset),
                None => t.offset(offset),
            };
            rows.aggregate([count().name("n")])
        }));
    }

    v.push(
        tables("test_isnull_equality", |fx| {
            fx.mem("isnull_eq")?
                .mutate([c("x").isnull().eq(c("y").isnull()).name("out")])?
                .order_by(["z"])?
                .select(["out"])
        })
        .ordered(),
    );
    // x.topk(10): group, count as "<col>_count", most frequent first
    v.push(tables("test_topk_counts_null", |fx| {
        fx.mem("topk")?
            .group_by(["x"])
            .agg([count().name("x_count")])?
            .order_by([desc("x_count")])?
            .limit(10, 0)
            .filter([c("x").isnull()])?
            .select(["x_count"])
    }));
    // Ibis marks this notyet on ClickHouse: it returns false for `x IN (NULL)`.
    v.push(
        tables("test_null_isin_null_is_null", |fx| {
            fx.mem("x_one")?.select([c("x").isin([untyped_null()]).name("tmp")])
        })
        .notimpl(&["clickhouse"]),
    );
    // t.value_counts(): group by every column, count as "<cols joined by _>_count"
    v.push(
        table("test_value_counts_on_tables", |t| {
            t.select(["bigint_col", "int_col"])?
                .group_by(["bigint_col", "int_col"])
                .agg([count().name("bigint_col_int_col_count")])?
                .order_by(["bigint_col", "int_col", "bigint_col_int_col_count"])
        })
        .ordered(),
    );
    v.push(tables("test_order_by_preservation", |fx| {
        Ok(fx.mem("id_col")?.order_by(["id"])?.select(["col"])?.distinct())
    }));

    v.push(
        table("test_subsequent_overlapping_order_by#1", |t| t.order_by([desc("id")])?.order_by(["id"])?.select(["id"]))
            .ordered(),
    );
    v.push(
        table("test_subsequent_overlapping_order_by#2", |t| {
            t.order_by([desc("id")])?.order_by(["id"])?.order_by([desc("id")])?.select(["id"])
        })
        .ordered(),
    );
    v.push(
        table("test_select_sort_sort", |t| {
            t.order_by([t.col("year").asc(), t.col("id").desc()])?
                .order_by([t.col("bool_col")])?
                .select(["bool_col", "year", "id"])
        })
        .ordered(),
    );

    let na: [(&str, DataType); 13] = [
        ("bool", DataType::Boolean),
        ("bytes", DataType::Binary),
        ("str", DataType::String),
        ("int", DataType::Int64),
        ("float", DataType::Float64),
        ("int8", DataType::Int8),
        ("int16", DataType::Int16),
        ("int32", DataType::Int32),
        ("int64", DataType::Int64),
        ("float32", DataType::Float32),
        ("float64", DataType::Float64),
        ("timestamp", DataType::Timestamp { tz: None }),
        ("date", DataType::Date),
    ];
    for (id, dtype) in na {
        v.push(scalar(&p("test_literal_na", id), move || Ok(null(dtype.clone()))));
    }

    more(v);
}

/// More of test_generic.py, in the order of the Python file.
fn more(v: &mut Vec<Case>) {
    use DataType as T;
    let no_nested = super::NO_NESTED;
    v.push(scalar("test_null_literal#1", || Ok(null(T::Null))));
    v.push(scalar("test_null_literal#2", || Ok(null(T::Null).cast(T::String).upper())));
    v.push(scalar("test_null_literal_typed#1", || Ok(null(T::Boolean))));
    v.push(scalar("test_null_literal_typed#2", || Ok(!null(T::Boolean))));
    v.push(scalar("test_null_literal_typed#3", || Ok(null(T::Boolean).cast(T::String).upper())));
    v.push(scalar("test_boolean_literal", || Ok(lit_as(false, T::Boolean))));
    v.push(column("test_identical_to", |_| Ok(c("tinyint_col").identical_to(c("double_col")))));
    v.push(table("test_filter_with_window_op", |t| {
        let mean = c("id").mean().over(WindowSpec::new().partition_by(["id"]));
        t.filter([mean.gt(3)])?.select(["id"])
    }));
    v.push(table("test_mutate_rename", |t| {
        t.select(["id", "bool_col", "string_col"])?.mutate([c("bool_col").name("dupe_col")])
    }));
    v.push(table("test_int_column", |t| t.mutate([lit(1).name("x")])?.select(["x"])));
    v.push(column("test_int_scalar", |_| Ok(c("int_col").min())));
    v.push(
        tables("test_select_distinct_order_by_alias", |fx| {
            fx.mem("xy_dups")?.select([c("x").name("y"), c("y").name("x")])?.distinct().order_by(["x", "y"])
        })
        .ordered(),
    );
    v.push(table("test_select_mutate_with_dict", |t| Ok(t.mutate([lit(1.0).name("a")])?.select(["a"])?.limit(1, 0))));
    v.push(table("test_select_scalar", |t| Ok(t.select([lit(1).name("y")])?.limit(3, 0))));
    v.push(
        table("test_select_sort_sort_deferred", |t| {
            t.order_by(["tinyint_col", "bool_col", "id"])?
                .order_by([c("bool_col").asc(), (c("tinyint_col") + 1).desc()])?
                .select(["bool_col", "tinyint_col", "id"])
        })
        .ordered(),
    );
    v.push(tables("test_union_generates_predictable_aliases", |fx| {
        let t = fx.mem("penguin")?;
        let sub1 = t.inner_join(&t.view(), ["island"])?.mutate([c("island").name("island_right")])?;
        let sub2 = t.inner_join(&t.view(), ["sex"])?.mutate([c("sex").name("sex_right")])?;
        sub1.union(&sub2, false)
    }));
    v.push(tables("test_comparison_with_decimal_literal", |fx| {
        let two = lit_as(2, T::Decimal { precision: 38, scale: 9 });
        fx.mem("decimal_ints")?.filter([c("numbers").eq(two)])
    }));
    v.push(
        scalar("test_cast[array]", || Ok(lit_as(vec![0, 1, 2], T::array(T::Int64)).cast(T::array(T::String))))
            .notimpl(&no_nested),
    );
    v.push(
        scalar("test_cast[struct]", || {
            let from = T::structure(&[("a", T::Int64), ("b", T::String)]);
            let value = Value::Struct(vec![("a".into(), Value::Int(0)), ("b".into(), Value::String("1".into()))]);
            Ok(lit_as(value, from).cast(T::structure(&[("a", T::String), ("b", T::Int64)])))
        })
        .notimpl(&no_nested),
    );

    // Postgres, MySQL and SQLite have no cast that gives NULL instead of failing (as in Ibis)
    let no_try: &[&str] = &["postgres", "mysql", "sqlite"];
    let tries: [(&str, fn() -> Expr); 8] = [
        ("test_try_cast[0-float-0.0]", || lit(0).try_cast(T::Float64)),
        ("test_try_cast[0.0-int-0]", || lit(0.0).try_cast(T::Int64)),
        ("test_try_cast[0-int-0]", || lit("0").try_cast(T::Int64)),
        ("test_try_cast[0.0-float-0.0]", || lit("0.0").try_cast(T::Float64)),
        ("test_try_cast_null[a-int]", || lit("a").try_cast(T::Int64)),
        ("test_try_cast_null[2023-01-01 00:00:00-int]", || timestamp("2023-01-01 00:00:00").try_cast(T::Int64)),
        ("test_try_cast_func[string-to-float]", || lit("a").try_cast(T::Float64)),
        ("test_try_cast_func[datetime-to-float]", || timestamp("2023-01-01 00:00:00").try_cast(T::Float64)),
    ];
    // A timestamp tried as a number is NULL on DuckDB, where the goldens are
    // from. These engines count its seconds or refuse (Ibis marks them too).
    let no_null: &[&str] = &["polars", "clickhouse", "trino", "spark", "impala"];
    for (id, f) in tries {
        let case = scalar(id, move || Ok(f())).notimpl(no_try);
        v.push(if id.contains("2023") || id.contains("datetime") { case.notimpl(no_null) } else { case });
    }
    v.push(
        tables("test_try_cast_table", |fx| {
            fx.mem("try_cast_ab")?
                .mutate([c("a").try_cast(T::Int64).name("a"), c("b").try_cast(T::Float64).name("b")])?
                .order_by(["a"])
        })
        .ordered()
        .notimpl(no_try),
    );
    // Slices with a bound counted from the last row; the Ibis tests count the rows.
    let p = |test: &str, id: &str| format!("{test}[{id}]");
    let static_slices = [("[-3:0]", -3, 0), ("[-3:-3]", -3, -3), ("[-3:-4]", -3, -4), ("[-3:-5]", -3, -5)];
    for (id, start, stop) in static_slices {
        v.push(table(&p("test_static_table_slice", id), move |t| {
            t.slice(Some(start), Some(stop))?.aggregate([count().name("n")])
        }));
    }
    let dynamic_slices: [(&str, Option<i64>, Option<i64>); 8] = [
        ("[:-2]", None, Some(-2)),
        ("[0:-2]", Some(0), Some(-2)),
        ("[-3:]", Some(-3), None),
        ("[-3:-2]", Some(-3), Some(-2)),
        ("[-4000:7000]", Some(-4000), Some(7000)),
        ("[-3:2]", Some(-3), Some(2)),
        ("[3:-2]", Some(3), Some(-2)),
        ("[3:-4]", Some(3), Some(-4)),
    ];
    for (id, start, stop) in dynamic_slices {
        v.push(table(&p("test_dynamic_table_slice", id), move |t| {
            t.slice(start, stop)?.aggregate([count().name("n")])
        }));
    }
    v.push(
        table("test_dynamic_table_slice_with_computed_offset", |t| {
            t.select(["id"])?.order_by(["id"])?.slice(Some(-10), None)
        })
        .ordered(),
    );
    // no SHA-256 in SQLite or Polars (as in Ibis)
    v.push(column("test_hexdigest", |_| Ok(c("string_col").hexdigest())).notimpl(&["sqlite", "polars"]));
}
