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

    v
}
