// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! ibis/backends/tests/test_temporal.py

use ibis_core::prelude::*;

use super::{Case, c, column, scalar, table};

const VALUE: &str = "2015-09-01 14:48:05.359";

type Field = fn(Expr) -> Expr;

pub fn register(v: &mut Vec<Case>) {
    let ymd: [(&str, Field); 3] = [("year", Expr::year), ("month", Expr::month), ("day", Expr::day)];
    for (attr, f) in ymd {
        // .date() and .cast("date") are the same expression here
        for how in ["date", "cast"] {
            v.push(column(&format!("test_date_extract[{how}-{attr}]"), move |_| Ok(f(c("timestamp_col").date()))));
        }
    }

    let fields: [(&str, Field); 8] = [
        ("year", Expr::year),
        ("month", Expr::month),
        ("day", Expr::day),
        ("day_of_year", Expr::day_of_year),
        ("quarter", Expr::quarter),
        ("hour", Expr::hour),
        ("minute", Expr::minute),
        ("second", Expr::second),
    ];
    for (attr, f) in fields {
        v.push(column(&format!("test_timestamp_extract[{attr}]"), move |_| Ok(f(c("timestamp_col")))));
    }

    let literal: [(&str, Field); 9] = [
        ("year", Expr::year),
        ("month", Expr::month),
        ("day", Expr::day),
        ("hour", Expr::hour),
        ("minute", Expr::minute),
        ("second", Expr::second),
        ("millisecond", Expr::millisecond),
        ("day_of_week_index", Expr::day_of_week_index),
        ("day_of_week_full_name", Expr::day_of_week_name),
    ];
    for (id, f) in literal {
        v.push(scalar(&format!("test_timestamp_extract_literal[{id}]"), move || Ok(f(timestamp(VALUE)))));
    }

    v.push(column("test_timestamp_extract_microseconds", |_| Ok(c("timestamp_col").microsecond())));
    v.push(column("test_timestamp_extract_milliseconds", |_| Ok(c("timestamp_col").millisecond())));
    v.push(column("test_timestamp_extract_week_of_year", |_| Ok(c("timestamp_col").week_of_year())));

    let truncate = [
        ("year", "Y"),
        ("quarter", "Q"),
        ("month", "M"),
        ("W-W", "W"),
        ("D-D", "D"),
        ("h-h", "h"),
        ("m-min", "m"),
        ("s-s", "s"),
        ("ms-ms", "ms"),
        ("us-us", "us"),
    ];
    for (id, unit) in truncate {
        v.push(column(&format!("test_timestamp_truncate[{id}]"), move |_| Ok(c("timestamp_col").truncate(unit))));
    }
    for unit in ["Y", "Q", "M", "D", "W"] {
        v.push(column(&format!("test_date_truncate[{unit}]"), move |_| Ok(c("timestamp_col").date().truncate(unit))));
    }

    v.push(column("test_strftime[literal_format_str]", |_| Ok(c("timestamp_col").strftime("%Y%m%d"))));

    let days = [
        ("sunday", "2017-01-01"),
        ("monday", "2017-01-02"),
        ("tuesday", "2017-01-03"),
        ("wednesday", "2017-01-04"),
        ("thursday", "2017-01-05"),
        ("friday", "2017-01-06"),
        ("saturday", "2017-01-07"),
    ];
    for (id, day) in days {
        v.push(scalar(&format!("test_day_of_week_scalar[{id}]#1"), move || {
            Ok(lit(day).cast(DataType::Date).day_of_week_index())
        }));
        v.push(scalar(&format!("test_day_of_week_scalar[{id}]#2"), move || {
            Ok(lit(day).cast(DataType::Date).day_of_week_name())
        }));
    }

    v.push(column("test_day_of_week_column#1", |_| Ok(c("timestamp_col").day_of_week_index())));
    v.push(column("test_day_of_week_column#2", |_| Ok(c("timestamp_col").day_of_week_name())));
    v.push(table("test_day_of_week_column_group_by[day_of_week_index]", |t| {
        t.group_by(["string_col"]).agg([c("timestamp_col").day_of_week_index().count().name("day_of_week_result")])
    }));
    v.push(table("test_day_of_week_column_group_by[day_of_week_full_name]", |t| {
        t.group_by(["string_col"]).agg([c("timestamp_col")
            .day_of_week_name()
            .length()
            .sum()
            .name("day_of_week_result")])
    }));

    v.push(scalar("test_date_literal", || Ok(date_from_ymd(lit(2022), lit(2), lit(4)))));
    v.push(scalar("test_timestamp_literal", || Ok(timestamp_from_ymdhms([2022, 2, 4, 16, 20, 0].map(lit)))));
    v.push(table("test_date_column_from_ymd", |t| {
        let ts = || c("timestamp_col");
        t.select([c("id"), date_from_ymd(ts().year(), ts().month(), ts().day()).name("timestamp_col")])
    }));
    v.push(table("test_timestamp_column_from_ymdhms", |t| {
        let ts = || c("timestamp_col");
        t.select([
            c("id"),
            timestamp_from_ymdhms([ts().year(), ts().month(), ts().day(), ts().hour(), ts().minute(), ts().second()])
                .name("timestamp_col"),
        ])
    }));
    v.push(scalar("test_date_scalar_from_iso", || Ok(lit("2022-02-24").date())));
    v.push(column("test_date_column_from_iso", |_| {
        Ok((c("year").cast(DataType::String) + lit("-") + c("month").cast(DataType::String).lpad(2, "0") + lit("-13"))
            .date())
    }));

    intervals(v);
    epochs_and_edges(v);
    buckets_and_formats(v);
}

/// Timestamps cut into buckets of a fixed width; strings read by a format.
fn buckets_and_formats(v: &mut Vec<Case>) {
    use IntervalUnit::*;
    let widths = [
        ("milliseconds", 50, Millisecond),
        ("seconds", 2, Second),
        ("minutes", 5, Minute),
        ("hours", 2, Hour),
        ("days", 2, Day),
    ];
    for (id, n, unit) in widths {
        v.push(column(&format!("test_timestamp_bucket[{id}]"), move |_| {
            Ok(c("timestamp_col").bucket(interval(n, unit)))
        }));
    }
    for (id, minutes) in [("pos", 2), ("neg", -2)] {
        v.push(column(&format!("test_timestamp_bucket_offset[{id}]"), move |_| {
            Ok(c("timestamp_col").bucket_offset(interval(5, Minute), interval(minutes, Minute)))
        }));
    }

    // date_string_col is mm/dd/yy; the pattern is C's on every backend
    v.push(column("test_string_as_timestamp[mysql_format]", |_| Ok(c("date_string_col").as_timestamp("%m/%d/%y"))));
    v.push(column("test_string_as_date[mysql_format]", |_| Ok(c("date_string_col").as_date("%m/%d/%y"))));
}

/// ISO years, numbers as timestamps, literals far from now, deltas.
fn epochs_and_edges(v: &mut Vec<Case>) {
    let naive = || DataType::Timestamp { tz: None };

    v.push(column("test_extract_iso_year[timestamp]", |_| Ok(c("timestamp_col").iso_year())));
    v.push(column("test_extract_iso_year[date]", |_| Ok(c("timestamp_col").date().iso_year())));
    v.push(scalar("test_iso_year_does_not_match_date_year", || Ok(date("2022-01-01").iso_year())));
    v.push(scalar("test_timestamp_extract_milliseconds_with_big_value", || {
        Ok(timestamp("2021-01-01 01:30:59.333456").millisecond())
    }));

    // a number cast to a timestamp counts seconds since 1970
    v.push(column("test_integer_cast_to_timestamp_column", move |_| Ok(c("int_col").cast(naive()))));
    v.push(table("test_integer_cast_to_timestamp_scalar", move |t| {
        t.aggregate([c("int_col").min().cast(naive()).name("tmp")])
    }));
    // the ids are Ibis's types; a timestamp here has one precision, the microsecond
    for dtype in ["timestamp", "timestamp(1)", "timestamp(3)", "timestamp(6)"] {
        v.push(scalar(&format!("test_subsecond_cast_to_timestamp[{dtype}]"), move || {
            Ok(lit("1699109238.5").cast(DataType::Float64).cast(dtype.parse()?))
        }));
    }

    v.push(scalar("test_big_timestamp", || Ok(timestamp("2419-10-11 10:10:25"))));
    v.push(scalar("test_large_timestamp", || Ok(timestamp("4567-01-01 00:00:00"))));
    for (id, text) in [("ms", "2023-01-07 13:20:05.561"), ("us", "2023-01-07 13:20:05.561021")] {
        v.push(scalar(&format!("test_timestamp_precision_output[{id}]"), move || Ok(lit(text).cast(naive()))));
    }

    // Ibis's test builds the literal from a string and from a Python date
    let dates = [("simple", "2017-12-31"), ("large", "9999-01-02"), ("small", "0001-07-17"), ("medium", "2150-01-01")];
    for (id, value) in dates {
        for how in ["identity", "fromstring"] {
            v.push(scalar(&format!("test_date_scalar[{how}-{id}]"), move || Ok(date(value))));
        }
    }

    v.push(scalar("test_simple_unix_date_offset", || Ok(date("2023-04-07").epoch_days())));
    v.push(scalar("test_delta[date]", || Ok(date("1992-10-01").delta(date("1992-09-30"), IntervalUnit::Day))));
    v.push(scalar("test_delta[timestamp]", || {
        Ok(timestamp("1992-10-01 01:58:00").delta(timestamp("1992-09-30 23:59:59"), IntervalUnit::Hour))
    }));

    let built = || {
        let two = |e: Expr| e.cast(DataType::String).lpad(2, "0");
        (c("year").cast(DataType::String) + lit("-") + two(c("month")) + lit("-") + two(c("int_col") + 1))
            .cast(DataType::Date)
    };
    v.push(column("test_timestamp_date_comparison[column_date]", move |_| Ok(built().eq(date("2010-11-01")))));
    v.push(column("test_timestamp_date_comparison[date_column]", move |_| Ok(date("2010-11-01").eq(built()))));
}

/// Timestamps and dates moved by intervals. The differences of
/// test_temporal_binop (two timestamps, two dates) are not here: their
/// result is an interval, which no column holds yet.
fn intervals(v: &mut Vec<Case>) {
    use IntervalUnit::*;

    // the second half of each id is the pandas class Ibis's test compares with
    let units = [
        ("Y-DateOffset", Year),
        ("Q-DateOffset", Quarter),
        ("M-DateOffset", Month),
        ("W-DateOffset", Week),
        ("D-DateOffset", Day),
        ("h-Timedelta", Hour),
        ("m-Timedelta", Minute),
        ("s-Timedelta", Second),
        ("ms-Timedelta", Millisecond),
        ("us-Timedelta", Microsecond),
    ];
    for (id, unit) in units {
        let case = column(&format!("test_integer_to_interval_timestamp[{id}]"), move |_| {
            Ok(c("timestamp_col") + c("int_col").as_interval(unit))
        });
        // SQLite's modifiers and Trino's DATE_ADD stop at the millisecond
        let none: &[&str] = if unit == Microsecond { &["sqlite", "trino"] } else { &[] };
        v.push(case.notimpl(none));
    }

    for (id, unit) in [("Y", Year), ("Q", Quarter), ("M", Month), ("W", Week), ("D", Day)] {
        v.push(column(&format!("test_integer_to_interval_date[{id}]"), move |_| {
            // date_string_col is mm/dd/yy
            let part = |start: i64| c("date_string_col").substr(start, Some(lit(2)));
            let date = (lit("20") + part(6) + lit("-") + part(0) + lit("-") + part(3)).cast(DataType::Date);
            Ok(date + c("int_col").as_interval(unit))
        }));
    }

    let ts = || c("timestamp_col");
    let binops: [(&str, fn() -> Expr); 6] = [
        ("timestamp-add-interval", || c("timestamp_col") + interval(4, Day)),
        ("timestamp-add-interval-binop", || c("timestamp_col") + (interval(4, Day) - interval(2, Day))),
        ("timestamp-add-interval-binop-different-units", || {
            c("timestamp_col") + (interval(4, Day) + interval(2, Hour))
        }),
        ("timestamp-subtract-interval", || c("timestamp_col") - interval(17, Day)),
        ("date-add-interval", || c("timestamp_col").date() + interval(4, Day)),
        ("date-subtract-interval", || c("timestamp_col").date() - interval(14, Day)),
    ];
    for (id, f) in binops {
        v.push(column(&format!("test_temporal_binop[{id}]"), move |_| Ok(f())));
    }

    // Ibis's test passes pandas Timedeltas: 5 weeks is 35 days either way
    let deltas = [
        ("large-days", 36500, Day),
        ("weeks", 5, Week),
        ("three-days", 3, Day),
        ("two-hours", 2, Hour),
        ("three-minutes", 3, Minute),
        ("ten-seconds", 10, Second),
    ];
    for (id, n, unit) in deltas {
        v.push(column(&format!("test_temporal_binop_pandas_timedelta[{id}-plus]"), move |_| {
            Ok(ts() + interval(n, unit))
        }));
        v.push(column(&format!("test_temporal_binop_pandas_timedelta[{id}-minus]"), move |_| {
            Ok(ts() - interval(n, unit))
        }));
    }

    v.push(column("test_interval_add_cast_scalar", |_| {
        Ok(c("timestamp_col").date() + lit(10).cast(DataType::Interval(Day)))
    }));
    v.push(table("test_interval_add_cast_column", |t| {
        let moved = c("timestamp_col").date() + c("bigint_col").cast(DataType::Interval(Day));
        t.select([c("id"), moved.name("tmp")])
    }));
}
