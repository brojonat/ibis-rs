# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

"""ibis/backends/tests/test_window.py

Deviations from the Ibis source:
- the Ibis tests compare only the computed `val` column (indexed by `id`), so
  cases end in `.select("id", "val")` instead of returning the whole frame;
- not ported: UDF windows, and cases whose result depends on physical row
  order (lag/lead/ntile over a window with no ORDER BY, cumsum over ties).
"""

from __future__ import annotations

import ibis

from cases import Registry, table, tables

r = Registry("test_window")


def _val(fn, window):
    return table(lambda t: t.mutate(val=fn(t, window(t))).select("id", "val"))


def _expanding(t):
    return ibis.window(following=0, group_by=[t.string_col], order_by=[t.id])


r.params(
    "test_grouped_bounded_expanding_window",
    {
        pid: _val(fn, _expanding)
        for pid, fn in {
            "lag": lambda t, win: t.float_col.lag().over(win),
            "lead": lambda t, win: t.float_col.lead().over(win),
            "rank": lambda t, win: t.id.rank().over(win),
            "dense_rank": lambda t, win: t.id.dense_rank().over(win),
            "percent_rank": lambda t, win: t.id.percent_rank().over(win),
            "cume_dist": lambda t, win: t.id.cume_dist().over(win),
            "ntile": lambda t, win: t.float_col.ntile(7).over(win),
            "first": lambda t, win: t.float_col.first().over(win),
            "last": lambda t, win: t.float_col.last().over(win),
            "nth": lambda t, win: t.double_col.nth(3).over(win),
            "row_number": lambda _, win: ibis.row_number().over(win),
            "cumsum": lambda t, win: t.double_col.cumsum().over(win),
            "cummean": lambda t, win: t.double_col.cummean().over(win),
            "cummin": lambda t, win: t.float_col.cummin().over(win),
            "cummax": lambda t, win: t.float_col.cummax().over(win),
            "cumany": lambda t, win: (t.double_col == 0).any().over(win),
            "cumnotany": lambda t, win: (t.double_col == 0).notany().over(win),
            "cumall": lambda t, win: (t.double_col == 0).all().over(win),
            "cumnotall": lambda t, win: (t.double_col == 0).notall().over(win),
            "sum": lambda t, win: t.double_col.sum().over(win),
            "mean": lambda t, win: t.double_col.mean().over(win),
            "min": lambda t, win: t.float_col.min().over(win),
            "max": lambda t, win: t.float_col.max().over(win),
            "count": lambda t, win: t.double_col.count().over(win),
        }.items()
    },
)

r.add(
    "test_ungrouped_bounded_expanding_window[mean]",
    _val(lambda t, win: t.double_col.mean().over(win), lambda t: ibis.window(following=0, order_by=[t.id])),
)

for pid, (preceding, following) in {"zero-two": (0, 2), "none-zero-two": (None, (0, 2))}.items():
    r.add(
        f"test_grouped_bounded_following_window[{pid}]",
        _val(
            lambda t, win: t.id.mean().over(win),
            lambda t, preceding=preceding, following=following: ibis.window(
                preceding=preceding, following=following, group_by=[t.string_col], order_by=[t.id]
            ),
        ),
    )

PRECEDING = {
    "preceding-2-following-0": lambda t: ibis.window(
        preceding=2, following=0, group_by=[t.string_col], order_by=[t.id]
    ),
    "preceding-2-following-0-tuple": lambda t: ibis.window(
        preceding=(2, 0), following=None, group_by=[t.string_col], order_by=[t.id]
    ),
    "trailing-2": lambda t: ibis.trailing_window(preceding=2, group_by=[t.string_col], order_by=[t.id]),
    "large-preceding-999-following-0": lambda t: ibis.window(
        preceding=999, following=0, group_by=[t.string_col], order_by=[t.id]
    ),
    "large-preceding-1000-following-0": lambda t: ibis.window(
        preceding=1000, following=0, group_by=[t.string_col], order_by=[t.id]
    ),
}
for pid, window in PRECEDING.items():
    r.add(f"test_grouped_bounded_preceding_window[{pid}]", _val(lambda t, win: t.double_col.sum().over(win), window))

r.add(
    "test_grouped_unbounded_window[mean]",
    _val(lambda t, win: t.double_col.mean().over(win), lambda t: ibis.window(group_by=[t.string_col])),
)

for method in ["sum", "min", "mean"]:

    def _unbound_following(t, method=method):
        t = t.filter(t.double_col < 50).order_by("id")
        w = ibis.window(rows=(0, None), order_by=t.id)
        return t.select("id", double_col=getattr(t.double_col, method)().over(w))

    r.add(f"test_simple_ungrouped_unbound_following_window[{method}]", table(_unbound_following))

r.params(
    "test_ungrouped_unbounded_window",
    {
        "ordered-mean": _val(lambda t, win: t.double_col.mean().over(win), lambda t: ibis.window(order_by=[t.id])),
        "unordered-mean": _val(lambda t, win: t.double_col.mean().over(win), lambda t: ibis.window(order_by=None)),
        "ordered-lag": _val(lambda t, win: t.float_col.lag().over(win), lambda t: ibis.window(order_by=[t.id])),
        "ordered-lead": _val(lambda t, win: t.float_col.lead().over(win), lambda t: ibis.window(order_by=[t.id])),
    },
)

r.add(
    "test_percent_rank_whole_table_no_order_by",
    table(lambda t: t.mutate(val=lambda t: t.id.percent_rank()).select("id", "val")),
)
r.add(
    "test_grouped_ordered_window_coalesce",
    table(
        lambda t: t.group_by("month").order_by("id").mutate(lagged_value=ibis.coalesce(t.bigint_col.lag(), 0))[
            ["id", "lagged_value"]
        ]
    ),
)


def _mutate_window_filter(t):
    win = ibis.window(order_by=[t.id])
    return (
        t.mutate(next_int=t.int_col.lead().over(win))
        .filter(lambda t: t.int_col == 1)
        .select("int_col", "next_int")
        .limit(3)
    )


r.add("test_mutate_window_filter", table(_mutate_window_filter))


def _first_last(c):
    t = c.mem("win")
    w = ibis.window(group_by=t.g, order_by=[t.x, t.y], preceding=1, following=0)
    return t.mutate(
        x_first=t.x.first().over(w),
        x_last=t.x.last().over(w),
        y_first=t.y.first().over(w),
        y_last=t.y.last().over(w),
    )


r.add("test_first_last", tables(_first_last))


def _order_by_sequence(c):
    t = c.mem("bool_nulls")
    window = ibis.window(order_by=[ibis.asc(t["bool_col"].isnull()), ibis.asc(t["bool_col"])])
    return t.select(rank=t["bool_col"].rank().over(window), bool_col=t["bool_col"])


r.add("test_windowed_order_by_sequence_is_preserved", tables(_order_by_sequence))
r.add(
    "test_over_with_scalar_group_by",
    table(lambda t: t.select(x=t.tinyint_col.sum().over(group_by=ibis.literal("string_col")))),
)
