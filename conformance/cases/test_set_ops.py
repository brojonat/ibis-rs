# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

"""ibis/backends/tests/test_set_ops.py

The `union_subsets` fixture shuffles each table's columns (random.seed(89)):
set operations match columns by name, not position. The shuffled orders are
written out so the Rust side can use the same ones.
"""

from __future__ import annotations

import ibis
from ibis import _

from cases import Registry, table

r = Registry("test_set_ops")

COLS_A = [
    "id", "timestamp_col", "date_string_col", "float_col", "year", "smallint_col", "double_col", "month",
    "bigint_col", "tinyint_col", "int_col", "string_col", "bool_col",
]  # fmt: skip
COLS_B = [
    "bigint_col", "float_col", "timestamp_col", "date_string_col", "year", "int_col", "month", "smallint_col", "id",
    "double_col", "tinyint_col", "string_col", "bool_col",
]  # fmt: skip
COLS_C = [
    "string_col", "int_col", "id", "date_string_col", "bigint_col", "tinyint_col", "smallint_col", "timestamp_col",
    "double_col", "float_col", "month", "bool_col", "year",
]  # fmt: skip


def _subsets(t):
    a = t.filter((_.id >= 5200) & (_.id <= 5210))[COLS_A]
    b = t.filter((_.id >= 5205) & (_.id <= 5215))[COLS_B]
    c = t.filter((_.id >= 5213) & (_.id <= 5220))[COLS_C]
    return a, b, c


for pid, distinct in [("all", False), ("distinct", True)]:

    def _union(t, distinct=distinct):
        a, b, _c = _subsets(t)
        return ibis.union(a, b, distinct=distinct).order_by("id")

    def _intersect(t, distinct=distinct):
        a = t.filter((_.id >= 5200) & (_.id <= 5210))
        b = t.filter((_.id >= 5205) & (_.id <= 5215))
        c = t.filter((_.id >= 5195) & (_.id <= 5208))
        return ibis.intersect(a, b, c, distinct=distinct).order_by("id")

    def _difference(t, distinct=distinct):
        a = t.filter((_.id >= 5200) & (_.id <= 5210))
        b = t.filter((_.id >= 5205) & (_.id <= 5215))
        c = t.filter((_.id >= 5195) & (_.id <= 5202))
        return ibis.difference(a, b, c, distinct=distinct).order_by("id")

    # ids repeat in the "all" union, so the order isn't total there
    r.add(f"test_union[{pid}]", table(_union, ordered=distinct))
    r.add(f"test_intersect[{pid}]", table(_intersect, ordered=True))
    r.add(f"test_difference[{pid}]", table(_difference, ordered=True))


def _union_mixed_distinct(t):
    a, b, c = _subsets(t)
    return a.union(b, distinct=True).union(c, distinct=False).order_by("id")


r.add("test_union_mixed_distinct", table(_union_mixed_distinct))

for ordered in [False, True]:
    for distinct in [True, False]:

        def _top_level_union(t, ordered=ordered, distinct=distinct):
            t1 = t.select(a="bigint_col").filter(lambda t: t.a == 10).distinct()
            t2 = t.select(a="bigint_col").filter(lambda t: t.a == 20).distinct()
            if ordered:
                t1 = t1.order_by("a")
                t2 = t2.order_by("a")
            return t1.union(t2, distinct=distinct).limit(2)

        r.add(f"test_top_level_union[{ordered}-{distinct}]", table(_top_level_union))

        for opname in ["intersect", "difference"]:

            def _top_level(t, ordered=ordered, distinct=distinct, opname=opname):
                t1 = t.select(a="bigint_col").filter(lambda t: (t.a == 10) | (t.a == 20)).distinct()
                t2 = t.select(a="bigint_col").filter(lambda t: (t.a == 20) | (t.a == 30)).distinct()
                if ordered:
                    t1 = t1.order_by("a")
                    t2 = t2.order_by("a")
                return getattr(t1, opname)(t2, distinct=distinct).limit(2)

            r.add(f"test_top_level_intersect_difference[{ordered}-{opname}-{distinct}]", table(_top_level))
