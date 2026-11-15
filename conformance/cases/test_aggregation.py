# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

from __future__ import annotations

from cases import Registry, table

r = Registry("test_aggregation")
register = r.params

AGGREGATE = {
    "mean": lambda t: t.double_col.mean(),
    "min": lambda t: t.double_col.min(),
    "max": lambda t: t.double_col.max(),
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

