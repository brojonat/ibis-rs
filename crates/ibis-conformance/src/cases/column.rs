// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! ibis/backends/tests/test_column.py

use super::{Case, table};

pub fn register(v: &mut Vec<Case>) {
    for column in ["string_col", "double_col", "date_string_col", "timestamp_col"] {
        v.push(table(&format!("test_distinct_column[{column}]"), move |t| Ok(t.select([column])?.distinct())));
    }
}
