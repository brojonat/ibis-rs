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
    "argmin": lambda t: t.timestamp_col.argmin(t.id),
    "argmax": lambda t: t.double_col.argmax(t.id),
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
    "argmin": lambda t, w: t.double_col.argmin(t.int_col, where=w),
    "argmax": lambda t, w: t.double_col.argmax(t.int_col, where=w),
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

# ids are "<null>-<filtered>-<method>": the key is NULL where int_col is 0 or
# 9, and with "null" the value at the answer's row is NULL too.
for null_id, null_result in {"null": True, "non-null": False}.items():
    for filtered_id, filtered in {"filtered": True, "unfiltered": False}.items():
        for method in ("argmin", "argmax"):

            def build(t, method=method, filtered=filtered, null_result=null_result):
                t = t.mutate(by_col=_.int_col.nullif(0).nullif(9), val_col=10 * _.int_col)
                if filtered:
                    where = _.int_col != (1 if method == "argmin" else 8)
                    sol = 20 if method == "argmin" else 70
                else:
                    where = None
                    sol = 10 if method == "argmin" else 80
                if null_result:
                    t = t.mutate(val_col=_.val_col.nullif(sol))
                return t.aggregate(tmp=getattr(t.val_col, method)("by_col", where=where))

            r.add(f"test_argmin_argmax[{null_id}-{filtered_id}-{method}]", table(build))

# test_corr_cov runs on batting; ids are "<cond>-<fn>". DuckDB has no sample
# correlation, so "corr_samp" has no golden and is not here.
CORR_COV = {
    "covar_pop": lambda t, w: t.G.cov(t.RBI, where=w, how="pop"),
    "covar_samp": lambda t, w: t.G.cov(t.RBI, where=w, how="sample"),
    "corr_pop": lambda t, w: t.G.corr(t.RBI, where=w, how="pop"),
    "covar_pop_bool": lambda t, w: (t.G > 34.0).cov(t.G <= 34.0, where=w, how="pop"),
    "corr_pop_bool": lambda t, w: (t.G > 34.0).corr(t.G <= 34.0, where=w, how="pop"),
}
for cid, cond in {"no_cond": lambda t: None, "cond": lambda t: t.yearID.isin([2009, 2015])}.items():
    for fid, fn in CORR_COV.items():
        r.add(
            f"test_corr_cov[{cid}-{fid}]",
            tables(lambda c, fn=fn, cond=cond: c.batting.aggregate(tmp=fn(c.batting, cond(c.batting)))),
        )

# ids are "<cond>-<quantile|multi-quantile>"
QUANTILES = {
    "quantile": lambda t, w: t.double_col.quantile(0.5, where=w),
    "multi-quantile": lambda t, w: t.double_col.quantile([0.5], where=w),
}
for cid, cond in CONDS.items():
    for qid, q in QUANTILES.items():
        r.add(f"test_quantile[{cid}-{qid}]", table(lambda t, q=q, cond=cond: t.aggregate(tmp=q(t, cond(t)))))
r.add(
    "test_string_quantile[quantile]",
    table(lambda t: (lambda s: s.aggregate(tmp=s.col.quantile(0.5)))(t.select(col=ibis.literal("a")).limit(5))),
)
r.add("test_date_quantile", table(lambda t: t.aggregate(tmp=t.timestamp_col.date().quantile(0.5))))

# ids are "<cond>-<sep>". Every string of a group is the same one, so the
# result does not depend on the order they are joined in.
SEPS = {"const": lambda: ":", "expr": lambda: ibis.literal(":") + ":"}
GROUP_CONCAT_CONDS = {
    "no_cond": lambda t: None,
    "is_in": lambda t: t.string_col.isin(["1", "7"]),
    "not_in": lambda t: t.string_col.notin(["1", "7"]),
}
for cid, cond in GROUP_CONCAT_CONDS.items():
    for sid, sep in SEPS.items():
        r.add(
            f"test_group_concat[{cid}-{sid}]",
            table(
                lambda t, cond=cond, sep=sep: (
                    t.group_by("bigint_col")
                    .aggregate(tmp=lambda s: s.string_col.group_concat(sep(), where=cond(s)))
                    .order_by("bigint_col")
                ),
                ordered=True,
            ),
        )
for filtered in (False, True):

    def ordered_concat(t, filtered=filtered):
        cond = (_.id % 13 == 0) if filtered else None
        t = t.filter(_.bigint_col == 10)
        return t.aggregate(tmp=t.id.cast("str").group_concat(":", where=cond, order_by=_.id.desc()))

    r.add(f"test_group_concat_ordered[{filtered}]", table(ordered_concat))


def group_concat_over_window(c):
    t = c.mem("group_concat_window")
    w = dict(group_by="pk", order_by="id", rows=(0, None))
    return t.mutate(test=t.s.group_concat(sep="|").over(**w)).order_by("id")


r.add("test_group_concat_over_window", tables(group_concat_over_window, ordered=True))
