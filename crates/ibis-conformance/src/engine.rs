// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! The tests of ibis/backends/tests/test_generic.py whose answer is the
//! engine's own (a hash, the name of a type), so there is no golden result
//! to compare with: what is checked is what the Ibis test checks. The other
//! generic tests are cases with a golden (`cases/generic.rs`).

use ibis_core::prelude::*;
use ibis_core::{Error, Result, Value};

use crate::Target;
use crate::client::{ClientCase, case, check, rows};

fn test(id: &str, run: impl Fn(&dyn Backend, &Target) -> Result<()> + Send + Sync + 'static) -> ClientCase {
    let mut c = case(id, run);
    c.module = "test_generic";
    c
}

pub(crate) fn cases() -> Vec<ClientCase> {
    let mut v = vec![];

    // The name each engine gives a boolean. Where there is no `typeof`
    // the Ibis test passes without asking, and so does this one.
    v.push(test("test_null_literal_typed_typeof", |b, target| {
        let want = match b.name() {
            "duckdb" | "impala" => "BOOLEAN",
            "postgres" | "trino" | "spark" => "boolean",
            // the type of a value, and the value is NULL
            "sqlite" => "null",
            "clickhouse" => "Nullable(Bool)",
            _ => "",
        };
        let t = target.fx.alltypes.select([null(DataType::Boolean).typeof_().name("tmp")])?.limit(1, 0);
        match rows(b, &t) {
            Err(Error::Unsupported { .. }) => Ok(()),
            Err(e) => Err(e),
            Ok(got) => check(got == [[Value::String(want.into())]], || format!("want {want:?}, got {got:?}")),
        }
    }));
    // MySQL and Polars have no name to give (as in Ibis)
    v.push(
        test("test_typeof", |b, target| {
            let got = rows(b, &target.fx.alltypes.select([lit(1).typeof_().name("tmp")])?.limit(1, 0))?;
            let named = matches!(got[..], [ref row] if matches!(row[..], [Value::String(ref s)] if !s.is_empty()));
            check(named, || format!("want the name of a type, got {got:?}"))
        })
        .notimpl(&["mysql", "polars"]),
    );

    // A hash is an int64, never NULL, and the same every time it is asked for.
    for dtype in ["smallint", "int", "bigint", "float", "double", "string"] {
        v.push(test(&format!("test_hash[{dtype}]"), move |b, target| {
            let t = &target.fx.alltypes;
            let hashed =
                t.select([col("id"), col(&format!("{dtype}_col")).hash().name("tmp")])?.order_by(["id"])?.limit(20, 0);
            let (first, second) = (rows(b, &hashed)?, rows(b, &hashed)?);
            let ints = first.len() == 20 && first.iter().all(|row| matches!(row[1], Value::Int(_)));
            check(ints && first == second, || format!("want 20 int64s, twice the same; got {first:?} and {second:?}"))
        }));
    }

    // Polars has no random number to give a row (as in Ibis)
    let no_random: &[&str] = &["polars"];
    v.push(
        test("test_order_by_random", |b, target| {
            let shuffled = target.fx.alltypes.filter([col("id").lt(100)])?.order_by([random()])?.limit(5, 0);
            let (first, second) = (rows(b, &shuffled)?, rows(b, &shuffled)?);
            check(first.len() == 5 && second.len() == 5 && first != second, || {
                format!("want five rows, and five others the second time; got {} and {}", first.len(), second.len())
            })
        })
        .notimpl(no_random),
    );
    // A sample has the table's columns and no more than its rows. (Ibis's
    // `method="block"` and `seed=` are not ported: `sample` draws by row.)
    for (id, filtered) in [("subquery-row", true), ("table-row", false)] {
        let run = move |b: &dyn Backend, target: &Target| {
            let t = target.fx.alltypes.clone();
            let t = if filtered { t.filter([col("int_col").ge(2)])? } else { t };
            let total = rows(b, &t.aggregate([count().name("n")])?)?;
            let sample = t.sample(0.1)?;
            check(sample.schema() == t.schema(), || format!("the sample's columns are {}", sample.schema()))?;
            let drawn = rows(b, &sample)?.len() as i64;
            check(matches!(total[..], [ref row] if matches!(row[..], [Value::Int(n)] if drawn <= n)), || {
                format!("{drawn} rows drawn from {total:?}")
            })
        };
        v.push(test(&format!("test_sample[{id}]"), run).notimpl(no_random));
    }
    v.push(
        test("test_sample_memtable", |b, target| {
            let t = target.fx.mem("c1_dups")?;
            let sample = t.sample(0.5)?;
            check(sample.schema() == t.schema(), || format!("the sample's columns are {}", sample.schema()))?;
            let drawn = rows(b, &sample)?.len();
            check(drawn <= 5, || format!("{drawn} rows drawn from 5"))
        })
        .notimpl(no_random),
    );

    v
}
