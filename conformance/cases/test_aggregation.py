# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

from __future__ import annotations

import ibis

from ibis import _

from cases import Registry, table, tables

r = Registry("test_aggregation")
register = r.params

AGGREGATE = {
    "mean": lambda t: t.double_col.mean(),
    "min": lambda t: t.double_col.min(),
    "max": lambda t: t.double_col.max(),
    # int_col % 3 so there are no ties for most common value
    "mode": lambda t: (t.int_col % 3).mode(),
    "complex_sum": lambda t: (t.double_col + 5).sum(),
    "timestamp_max": lambda t: t.timestamp_col.max(),
}
register("test_aggregate", {k: table(lambda t, f=f: t.aggregate(tmp=f(t))) for k, f in AGGREGATE.items()})
register(
    "test_aggregate_grouped",
    {k: table(lambda t, f=f: t.group_by("bigint_col").aggregate(tmp=f(t))) for k, f in AGGREGATE.items()},
)

# test_reduction_ops runs on alltypes.filter(id < 1550); ids are "<cond>-<op>".
REDUCTIONS = {
    "count": lambda t, w: t.bool_col.count(where=w),
    "nunique": lambda t, w: t.bool_col.nunique(where=w),
    "any": lambda t, w: t.bool_col.any(where=w),
    "any_negate": lambda t, w: ~t.bool_col.any(where=w),
    "all": lambda t, w: t.bool_col.all(where=w),
    "all_negate": lambda t, w: ~t.bool_col.all(where=w),
    "sum": lambda t, w: t.double_col.sum(where=w),
    "bool_sum": lambda t, w: (t.int_col > 0).sum(where=w),
    "mean": lambda t, w: t.double_col.mean(where=w),
    "min": lambda t, w: t.double_col.min(where=w),
    "max": lambda t, w: t.double_col.max(where=w),
    "std": lambda t, w: t.double_col.std(how="sample", where=w),
    "var": lambda t, w: t.double_col.var(how="sample", where=w),
    "std_pop": lambda t, w: t.double_col.std(how="pop", where=w),
    "var_pop": lambda t, w: t.double_col.var(how="pop", where=w),
    "count_star": lambda t, w: t.count(where=w),
    "notany": lambda t, w: t.bool_col.notany(where=w),
    "notall": lambda t, w: t.bool_col.notall(where=w),
    "mode": lambda t, w: (t.int_col % 3).mode(where=w),
    "kurtosis_samp": lambda t, w: t.double_col.kurtosis(how="sample", where=w),
    "approx_nunique": lambda t, w: t.string_col.approx_nunique(where=w),
    "bit_and": lambda t, w: t.bigint_col.bit_and(where=w),
    "bit_or": lambda t, w: t.bigint_col.bit_or(where=w),
    "bit_xor": lambda t, w: t.bigint_col.bit_xor(where=w),
}
CONDS = {
    "no_cond": lambda t: None,
    "is_in": lambda t: t.string_col.isin(["1", "7"]),
}
for cid, cond in CONDS.items():
    for rid, red in REDUCTIONS.items():

        def build(t, red=red, cond=cond):
            t = t.filter(t.id < 1550)
            return t.aggregate(tmp=red(t, cond(t)))

        r.add(f"test_reduction_ops[{cid}-{rid}]", table(build))

r.add("test_median", table(lambda t: t.aggregate(tmp=t.double_col.median())))
r.add(
    "test_string_quantile[median]",
    table(lambda t: (lambda s: s.aggregate(tmp=s.col.median()))(t.select(col=ibis.literal("a")).limit(5))),
)

r.add(
    "test_binds_are_cast",
    table(lambda t: t.aggregate(high_line_count=t.string_col.cases(("1-URGENT", 1), else_=0).sum())),
)
r.add(
    "test_filter",
    table(
        lambda t: (
            t.filter(_.string_col == "1")
            .mutate(x=ibis.literal(1, "int64"))
            .group_by(_.x)
            .aggregate(sum=_.double_col.sum())
        )
    ),
)


def grouped_case(c):
    t = c.mem("grouped_case")
    case_expr = ibis.cases((t.value < 25, t.value), else_=ibis.null())
    return t.group_by(k="key").aggregate(mx=case_expr.max()).drop_null("k").order_by("k")


r.add("test_grouped_case", tables(grouped_case, ordered=True))
r.add("test_value_counts_on_expr", table(lambda t: t.bigint_col.add(1).value_counts()))
r.add(
    "test_group_by_expr",
    tables(
        lambda c: (
            c.mem("project_names")
            .group_by(n=_.project_name.length())
            .aggregate(c=_.project_name.nunique())
            .order_by(_.n.desc())
        ),
        ordered=True,
    ),
)
for vid, value in {"string": lambda: ibis.literal("a"), "null": lambda: ibis.null("str")}.items():
    r.add(
        f"test_group_by_scalar[{vid}]",
        table(lambda t, value=value: t.group_by(key=value()).agg(n=lambda t: t.count())),
    )
# the test's two results: a count, and the sum of no rows
r.add("test_empty_sum#1", tables(lambda c: c.mem("x_one").aggregate(tmp=_.count())))
r.add("test_empty_sum#2", tables(lambda c: (lambda t: t.aggregate(tmp=t.x.sum(where=t.x > 1)))(c.mem("x_one"))))
