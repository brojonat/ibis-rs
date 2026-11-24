// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! The tests of ibis/backends/tests/test_numeric.py whose answer is drawn
//! at random, so there is no golden result to compare with: what is
//! checked is what the Ibis test checks. The other numeric tests are
//! cases with a golden (`cases/numeric.rs`).

use ibis_core::prelude::*;
use ibis_core::{Result, Value};

use crate::Target;
use crate::client::{ClientCase, case, check, rows};

fn test(id: &str, run: impl Fn(&dyn Backend, &Target) -> Result<()> + Send + Sync + 'static) -> ClientCase {
    let mut c = case(id, run);
    c.module = "test_numeric";
    c
}

pub(crate) fn cases() -> Vec<ClientCase> {
    // Polars has no random number to give a row (as in Ibis)
    let no_random: &[&str] = &["polars"];
    let mut v = vec![];

    v.push(
        test("test_random", |b, target| {
            let got = rows(b, &target.fx.alltypes.select([random().name("tmp")])?.limit(1, 0))?;
            let unit = matches!(got[..], [ref row] if matches!(row[..], [Value::Float(x)] if (0.0..=1.0).contains(&x)));
            check(unit, || format!("want a float between 0 and 1, got {got:?}"))
        })
        .notimpl(no_random),
    );
    v.push(
        test("test_random_different_per_row", |b, target| {
            let got = rows(b, &target.fx.alltypes.select([col("int_col"), random().name("rand_col")])?)?;
            let mut drawn: Vec<u64> = got
                .iter()
                .filter_map(|row| match row[1] {
                    Value::Float(x) => Some(x.to_bits()),
                    _ => None,
                })
                .collect();
            drawn.sort_unstable();
            drawn.dedup();
            check(drawn.len() > 1, || format!("{} rows, {} different numbers", got.len(), drawn.len()))
        })
        .notimpl(no_random),
    );

    v
}
