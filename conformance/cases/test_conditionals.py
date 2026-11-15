# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

"""ibis/backends/tests/test_conditionals.py

test_ibis_case_still_works (the deprecated `.case().when()` builder) is not
ported: it builds the same expressions as the `cases` tests.
"""

from __future__ import annotations

import ibis

from cases import Registry, column, scalar, table, tables

r = Registry("test_conditionals")

r.add(
    "test_ifelse_select",
    table(lambda t: t.select("int_col", ibis.ifelse(t["int_col"] == 0, 42, -1).cast("int64").name("where_col"))),
)
r.add("test_ifelse_column", column(lambda t: ibis.ifelse(t["int_col"] == 0, 42, -1).cast("int64")))
r.add(
    "test_substitute",
    table(
        lambda t: t.string_col.nullif("1")
        .substitute({None: "400"})
        .name("subs")
        .value_counts()
        .filter(lambda t: t.subs == "400")
    ),
)
r.params(
    "test_value_cases_scalar",
    {
        "one_kwarg": scalar(lambda: ibis.literal(1).cases((1, "one"), (2, "two"), else_="other")),
        "fallthrough": scalar(lambda: ibis.literal(5).cases((1, "one"), (2, "two"))),
    },
)
r.add(
    "test_value_cases_column",
    tables(
        lambda c: c.batting.select(
            tmp=c.batting.RBI.cases((5, "five"), (4, "four"), (3, "three"), else_="could be good?")
        )
    ),
)
r.add("test_ibis_cases_scalar", scalar(lambda: ibis.literal(5).cases((5, "five"), (4, "four"))))
r.add(
    "test_ibis_cases_column",
    tables(
        lambda c: c.batting.select(
            tmp=ibis.cases(
                (c.batting.RBI < 5, "really bad team"), (c.batting.teamID == "PH1", "ph1 team"), else_=c.batting.teamID
            )
        )
    ),
)
# CASE x WHEN NULL never gets hit
r.add("test_value_cases_null", scalar(lambda: ibis.literal(5).nullif(5).cases((None, "oops"), else_="expected")))
