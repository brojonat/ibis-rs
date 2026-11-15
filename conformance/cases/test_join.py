# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

"""ibis/backends/tests/test_join.py

Deviations from the Ibis source:
- test_mutating_join / test_filtering_join compare the join result itself,
  not the pyarrow post-processing the Ibis test applies to it;
- tests that only assert a row count or non-emptiness (and so tolerate
  arbitrary rows from `limit`) end in a count, which is deterministic.
"""

from __future__ import annotations

import ibis
from ibis import _

from cases import Registry, tables

r = Registry("test_join")


def _left_right(c):
    left = c.batting.filter(c.batting.yearID == 2015)
    right = c.awards_players.filter(c.awards_players.lgID == "NL").drop("yearID", "lgID")
    return left, right


def _join(how):
    def build(c):
        left, right = _left_right(c)
        return left.join(right, ["playerID"], how=how)

    return tables(build)


r.params("test_mutating_join", {how: _join(how) for how in ["inner", "left", "right", "outer"]})
r.params("test_filtering_join", {how: _join(how) for how in ["semi", "anti"]})


def _join_then_filter(c):
    left = c.batting.filter(c.batting.yearID == 2015)
    year = left.yearID.name("year")
    left = left.select(year, "RBI")
    right = c.awards_players.filter(c.awards_players.lgID == "NL")
    expr = left.join(right, left.year == right.yearID)
    return expr.filter([expr.RBI == 9])


r.add("test_join_then_filter_no_column_overlap", tables(_join_then_filter))


def _mutate_then_join(c):
    left = c.batting.mutate(year=c.batting.yearID).filter(lambda t: t.year == 2015)
    left = left["year", "RBI"]
    right = c.awards_players
    return left.join(right, left.year == right.yearID).aggregate(n=_.count())


r.add("test_mutate_then_join_no_column_overlap", tables(_mutate_then_join))

TRIVIAL = {
    "true": True,
    "true-literal": ibis.literal(True),
    "true-list": [True],
    "true-literal-list": [ibis.literal(True)],
    "true-true-list": [True, True],
    "true-true-literal-list": [ibis.literal(True), ibis.literal(True)],
    "true-true-const-expr-list": [True, ibis.literal(True)],
    "true-true-expr-const-list": [ibis.literal(True), True],
    "false": False,
    "false-literal": ibis.literal(False),
    "false-list": [False],
    "false-literal-list": [ibis.literal(False)],
    "false-false-list": [False, False],
    "false-false-literal-list": [ibis.literal(False), ibis.literal(False)],
    "false-false-const-expr-list": [False, ibis.literal(False)],
    "false-false-expr-const-list": [ibis.literal(False), False],
}
for how in ["inner", "left", "right", "outer"]:
    for pid, predicate in TRIVIAL.items():

        def _trivial(c, how=how, predicate=predicate):
            base = c.awards_players.limit(5)
            left = base.select(left_key="playerID")
            right = base.select(right_key="playerID")
            return left.join(right, predicate, how=how).aggregate(n=_.count())

        r.add(f"test_join_with_trivial_predicate[{how}-{pid}]", tables(_trivial))

NULLABILITY = {
    "left-xy": ("left", True),
    "left-x": ("left", False),
    "right-xy": ("right", True),
    "right-x": ("right", False),
    "outer-xy": ("outer", True),
    "outer-x": ("outer", False),
}
for pid, (how, renamed) in NULLABILITY.items():

    def _nullability(c, how=how, renamed=renamed):
        left = c.mem("win").select(x=lambda t: t.x).filter(lambda t: t.x.isin((1, 2)))
        if renamed:
            right = left.filter(lambda t: t.x == 1).select(y=lambda t: t.x)
            return left.join(right, [("x", "y")], how=how)
        right = left.filter(lambda t: t.x == 1)
        return left.join(right, "x", how=how)

    r.add(f"test_outer_join_nullability[{pid}]", tables(_nullability))

r.add("test_join_conflicting_columns", tables(lambda c: c.mem("xyz1").join(c.mem("xyz2"), "x")))
