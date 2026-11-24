// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! The tests of ibis/backends/tests/test_string.py that Python Ibis on
//! DuckDB does not give the answer the test asks for, so there is no golden
//! result to compare with: what is checked is what the Ibis test checks.
//! The other string tests are cases with a golden (`cases/string.rs`).

use ibis_core::prelude::*;
use ibis_core::{Result, Value};

use crate::Target;
use crate::cases::NO_NESTED;
use crate::client::{ClientCase, case, check, rows};

fn test(id: &str, run: impl Fn(&dyn Backend, &Target) -> Result<()> + Send + Sync + 'static) -> ClientCase {
    let mut c = case(id, run);
    c.module = "test_string";
    c
}

pub(crate) fn cases() -> Vec<ClientCase> {
    let mut v = vec![];

    // An empty array joins to NULL: every Ibis backend is compiled to say
    // so, and DuckDB 2 answers '' where DuckDB 1 answered NULL.
    v.push(
        test("test_empty_array_string_join", |b, target| {
            let t = target.fx.mem("arr_empty_abc")?.select([col("arr").join([lit(",")]).name("tmp")])?;
            let mut got = rows(b, &t)?;
            got.sort_by_key(|row| !row[0].is_null());
            let want = [[Value::Null], [Value::String("a,b,c".into())]];
            check(got == want, || format!("want NULL and 'a,b,c', got {got:?}"))
        })
        .notimpl(&NO_NESTED),
    );

    v
}
