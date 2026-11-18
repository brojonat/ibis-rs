# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

from __future__ import annotations

import ibis

from cases import Registry, scalar, table, tables

r = Registry("test_struct")

STRUCT_TYPE = "struct<a: int64, b: string, c: float64>"

r.params(
    "test_single_field",
    {
        field: tables(
            lambda c, field=field: c.mem("struct").select(field=lambda t: t.abc[field]).order_by("field"),
            ordered=True,
        )
        for field in "abc"
    },
)
r.add("test_all_fields", tables(lambda c: c.mem("struct").select("abc")))
r.add("test_literal", scalar(lambda: ibis.struct(dict(a=1, b="2", c=3.0), type=STRUCT_TYPE)))
r.params(
    "test_null_literal",
    {field: scalar(lambda field=field: ibis.null().cast(STRUCT_TYPE)[field]) for field in "abc"},
)
r.add(
    "test_struct_column",
    table(lambda t: t.select(s=ibis.struct(dict(a=t.string_col, b=1, c=t.bigint_col)))),
)
# the collected values are sorted: `collect` promises no order
r.add(
    "test_collect_into_struct",
    table(
        lambda t: t.filter(t.string_col.isin(("0", "1")))
        .group_by(group="string_col")
        .agg(val=lambda t: ibis.struct(dict(key=t.bigint_col.collect().sort().cast("!array<int64>"))))
    ),
)
r.add(
    "test_field_access_after_case",
    scalar(lambda: ibis.cases((True, ibis.struct({"a": 3})), else_=ibis.struct({"a": 4})).a),
)


def _isin_struct(c):
    needle1 = ibis.struct({"x": 1, "y": 2})
    needle2 = ibis.struct({"x": 2, "y": 3})
    haystack_t = c.mem("xs_ys")
    haystack = ibis.struct({"x": haystack_t.xs, "y": haystack_t.ys})
    return c.mem("x_one").select(tmp=needle1.isin(haystack) | needle2.isin(haystack))


r.add("test_isin_struct", tables(_isin_struct))


def _unpacked(order):
    def build(c):
        t = c.mem("struct").filter(lambda t: t.abc.a.notnull())
        t1 = t.mutate(a=1)
        t2 = t1.select("a", "abc")
        t3 = t1.select("abc", "a")
        t4 = t2.select("abc", "a")
        return [t1, t2, t3, t4][order].unpack("abc")

    return tables(build)


for n in range(4):
    r.add(f"test_field_overwrite_always_prefers_unpacked#{n + 1}", _unpacked(n))
