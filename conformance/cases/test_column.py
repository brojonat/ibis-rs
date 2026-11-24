# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

"""ibis/backends/tests/test_column.py"""

from __future__ import annotations

from cases import Registry, table

r = Registry("test_column")

for column in ["string_col", "double_col", "date_string_col", "timestamp_col"]:
    r.add(f"test_distinct_column[{column}]", table(lambda t, column=column: t[[column]].distinct()))
