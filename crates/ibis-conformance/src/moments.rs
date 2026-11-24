// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! The tests of ibis/backends/tests/test_temporal.py that Python Ibis on
//! DuckDB cannot make a golden result for (it has no microsecond or
//! nanosecond `as_timestamp`, and its `epoch_seconds` overflows the type
//! Ibis declares). What they expect is plain arithmetic, done here. The
//! other temporal tests are cases with a golden (`cases/temporal.rs`).

use ibis_core::convert::parse_timestamp;
use ibis_core::prelude::*;
use ibis_core::{Result, Value};

use crate::Target;
use crate::client::{ClientCase, case, check, rows};

fn test(id: &str, run: impl Fn(&dyn Backend, &Target) -> Result<()> + Send + Sync + 'static) -> ClientCase {
    let mut c = case(id, run);
    c.module = "test_temporal";
    c
}

pub(crate) fn cases() -> Vec<ClientCase> {
    use IntervalUnit::*;
    let mut v = vec![];

    // A moment as a count of each unit since 1970, and back: the moment
    // with the unit's precision, to the microsecond a timestamp keeps.
    let units = [
        ("s", Second, 1_000_000, 1),
        ("ms", Millisecond, 1_000, 1),
        ("us", Microsecond, 1, 1),
        ("ns", Nanosecond, 1, 1_000),
    ];
    for (id, unit, micros_per_unit, units_per_micro) in units {
        let run = move |b: &dyn Backend, target: &Target| {
            let moment = parse_timestamp("2018-04-13 09:54:11.872832").expect("a timestamp");
            let count = moment / micros_per_unit * units_per_micro;
            let expr = lit(count).as_timestamp(unit);
            let got = rows(b, &target.fx.alltypes.select([expr.name("tmp")])?.limit(1, 0))?;
            let want = Value::Timestamp(moment / micros_per_unit * micros_per_unit);
            check(got == [[want.clone()]], || format!("want {want:?}, got {got:?}"))
        };
        v.push(test(&format!("test_integer_to_timestamp[{id}]"), run));
    }

    v.push(test("test_timestamp_extract_epoch_seconds", |b, target| {
        let ts = || col("timestamp_col");
        let got = rows(b, &target.fx.alltypes.select([ts(), ts().epoch_seconds().name("tmp")])?)?;
        let wrong = got.iter().find(|row| match (&row[0], &row[1]) {
            (Value::Timestamp(micros), Value::Int(seconds)) => micros.div_euclid(1_000_000) != *seconds,
            _ => true,
        });
        check(wrong.is_none() && !got.is_empty(), || format!("{} rows, one of them {wrong:?}", got.len()))
    }));

    v
}
