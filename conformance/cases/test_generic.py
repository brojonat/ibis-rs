# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

"""ibis/backends/tests/test_generic.py

Deviations from the Ibis source, all to keep goldens small or deterministic:
- cases whose Ibis test only checks one column of a wide frame select that
  column (plus `id`) at the end;
- tests with several assertions become `name#1`, `name#2`, ...;
- negative slices in test_static_table_slice are not ported.
"""

from __future__ import annotations

import operator
from itertools import permutations

import ibis
import ibis.selectors as s
import numpy as np
from ibis import _

from cases import Registry, column, scalar, table, tables

r = Registry("test_generic")

r.params(
    "test_scalar_fill_null_nullif",
    {
        "na_fill_null": scalar(lambda: ibis.null().fill_null(5)),
        "non_na_fill_null": scalar(lambda: ibis.literal(5).fill_null(10)),
        "nullif_null": scalar(lambda: ibis.literal(5).nullif(5)),
        "nullif_not_null": scalar(lambda: ibis.literal(10).nullif(5)),
    },
)


def _isna(col, value, filt):
    def build(t):
        tbl = t.select(**{col: value})
        return tbl.filter(filt(tbl[col]))

    return table(build)


r.params(
    "test_isna",
    {
        "nan_col": _isna("nan_col", ibis.literal(np.nan), operator.methodcaller("isnan")),
        "none_col": _isna("none_col", ibis.null().cast("float64"), operator.methodcaller("isnull")),
    },
)


def _column_fill_null(t):
    tbl = t.mutate(missing=ibis.literal(None).cast("float64"))
    return tbl.mutate(missing=tbl.missing.fill_null(0.0))


r.add("test_column_fill_null[None]", table(_column_fill_null))

r.params(
    "test_coalesce",
    {
        "generic": scalar(lambda: ibis.coalesce(5, None, 4)),
        "null_start_end": scalar(lambda: ibis.coalesce(ibis.null(), 4, ibis.null())),
        "non_null_last": scalar(lambda: ibis.coalesce(ibis.null(), ibis.null(), 3.14)),
    },
)

ISIN = [
    ("int_col", [1, 2, 3]),
    ("int_col", (1, 2, 3)),
    ("string_col", ["1", "2", "3"]),
    ("string_col", ("1", "2", "3")),
    ("int_col", {1}),
    ("int_col", frozenset({1})),
]
for i, (col, elements) in enumerate(ISIN):

    def _isin(t, col=col, elements=elements):
        st = t.order_by("id")
        return st.select("id", st[col].isin(elements).name("tmp")).order_by("id")

    def _notin(t, col=col, elements=elements):
        st = t.order_by("id")
        return st.select("id", st[col].notin(elements).name("tmp")).order_by("id")

    r.add(f"test_isin[{col}-elements{i}]", table(_isin, ordered=True))
    r.add(f"test_notin[{col}-elements{i}]", table(_notin, ordered=True))

FILTER = {
    "no_op": lambda t: t["bool_col"],
    "negate": lambda t: ~t["bool_col"],
    "and": lambda t: t.bool_col & t.bool_col,
    "or": lambda t: t.bool_col | t.bool_col,
    "xor": lambda t: t.bool_col ^ t.bool_col,
}
for pid, pred in FILTER.items():

    def _filter(t, pred=pred):
        st = t.order_by("id")
        return st.filter(pred(st)).order_by("id")

    r.add(f"test_filter[{pid}]", table(_filter, ordered=True))

r.add(
    "test_case_where",
    table(
        lambda t: t.mutate(
            new_col=ibis.cases((t["int_col"] == 1, 20), (t["int_col"] == 0, 10), else_=0).cast("int64")
        )
    ),
)


def _select_filter_mutate(t):
    t = t.mutate(float_col=ibis.cases((t["bool_col"], t["float_col"]), else_=np.nan))
    t = t.select(t.columns)
    t = t.filter(~t["float_col"].isnan())
    t = t.mutate(float_col=t["float_col"].cast("float64"))
    return t.select("id", "float_col")  # the Ibis test compares float_col only


r.add("test_select_filter_mutate", table(_select_filter_mutate))


def _nullable(t):
    return t.mutate(
        int_col=t.int_col.nullif(1),
        double_col=t.double_col.nullif(3.0),
        string_col=t.string_col.nullif("2"),
    ).select("id", "int_col", "double_col", "string_col")


r.params(
    "test_table_fill_null_mapping",
    {
        "int": table(lambda t: _nullable(t).fill_null({"int_col": 20})),
        "double-int-str": table(lambda t: _nullable(t).fill_null({"double_col": -1, "string_col": "missing"})),
        "double-str": table(lambda t: _nullable(t).fill_null({"double_col": -1.5, "string_col": "missing"})),
        "empty": table(lambda t: _nullable(t).fill_null({})),
    },
)
r.add("test_table_fill_null_scalar#1", table(lambda t: _nullable(t)[["int_col", "double_col"]].fill_null(0)))
r.add("test_table_fill_null_scalar#2", table(lambda t: _nullable(t)[["string_col"]].fill_null("missing")))

DROP_NULL_SUBSETS = {
    "none": None,
    "empty": [],
    "single": "col_1",
    "one-and-two": ["col_1", "col_2"],
    "one-and-three": ["col_1", "col_3"],
}
for sid, subset in DROP_NULL_SUBSETS.items():
    for how in ["any", "all"]:

        def _drop_null(t, subset=subset, how=how):
            is_two = t.int_col == 2
            is_four = t.int_col == 4
            tbl = t.mutate(
                col_1=is_two.ifelse(ibis.null(), t.float_col),
                col_2=is_four.ifelse(ibis.null(), t.float_col),
                col_3=(is_two | is_four).ifelse(ibis.null(), t.float_col),
            ).select("id", "col_1", "col_2", "col_3")
            return tbl.drop_null(subset, how=how).order_by("id")

        r.add(f"test_drop_null_table[{sid}-{how}]", table(_drop_null, ordered=True))

ORDER_BY = [
    ("id-df_kwargs0", "id"),
    ("key1-df_kwargs1", _.id),
    ("<lambda>-df_kwargs2", lambda _: _.id),
    ("key3-df_kwargs3", ibis.desc("id")),
    ("key4-df_kwargs4", ["id", "int_col"]),
    ("key5-df_kwargs5", ["id", ibis.desc("int_col")]),
]
for pid, key in ORDER_BY:
    r.add(f"test_order_by[{pid}]", table(lambda t, key=key: t.filter(_.id < 100).order_by(key), ordered=True))

for op in ["desc", "asc"]:

    def _nulls_default(c, op=op):
        t = c.mem("order_nulls")
        return t.order_by(getattr(t["b"], op)())

    def _nulls_first(c, op=op):
        t = c.mem("order_nulls")
        return t.order_by(getattr(t["b"], op)(nulls_first=True))

    r.add(f"test_order_by_nulls_default[{op}]", tables(_nulls_default, ordered=True))
    r.add(f"test_order_by_nulls[{op}]", tables(_nulls_first, ordered=True))

TWO_COLS = {
    "asc-desc-ff": ("asc", False, "desc", False),
    "asc-desc-tt": ("asc", True, "desc", True),
    "asc-desc-tf": ("asc", True, "desc", False),
    "asc-asc-tt": ("asc", True, "asc", True),
    "asc-asc-tf": ("asc", True, "asc", False),
}
for pid, (op1, nf1, op2, nf2) in TWO_COLS.items():

    def _two_cols(c, op1=op1, nf1=nf1, op2=op2, nf2=nf2):
        t = c.mem("order_two_cols")
        return t.order_by(getattr(t["col1"], op1)(nulls_first=nf1), getattr(t["col2"], op2)(nulls_first=nf2))

    r.add(f"test_order_by_two_cols_nulls[{pid}]", tables(_two_cols, ordered=True))

r.params(
    "test_isin_notin",
    {
        "isin": table(lambda t: t.filter(_.string_col.isin([]))),
        "notin": table(lambda t: t.filter(_.string_col.notin([]))),
        "isin_non_empty": table(lambda t: t.filter((_.string_col.length() * 1).isin([1]))),
        "notin_non_empty": table(lambda t: t.filter((_.string_col.length() * 1).notin([1]))),
    },
)

r.params(
    "test_isin_notin_column_expr",
    {
        "isin_col": table(lambda t: t.filter(_.string_col.isin(_.string_col)).order_by("id"), ordered=True),
        "isin_expr": table(
            lambda t: t.filter((_.bigint_col + 1).isin(_.string_col.length() + 1)).order_by("id"), ordered=True
        ),
        "notin_col": table(lambda t: t.filter(_.string_col.notin(_.string_col)).order_by("id"), ordered=True),
        "notin_expr": table(
            lambda t: t.filter((_.bigint_col + 1).notin(_.string_col.length() + 1)).order_by("id"), ordered=True
        ),
    },
)

r.params(
    "test_logical_negation_literal",
    {
        "true_noop": scalar(lambda: ibis.literal(True)),
        "false_noop": scalar(lambda: ibis.literal(False)),
        "true_invert": scalar(lambda: ~ibis.literal(True)),
        "false_invert": scalar(lambda: ~ibis.literal(False)),
    },
)
r.params(
    "test_logical_negation_column",
    {
        "identity": column(lambda t: t["bool_col"]),
        "invert": column(lambda t: ~t["bool_col"]),
    },
)

for pid, dtype, expected in [("int64-0-1", "int64", 1), ("float64-0.0-1.0", "float64", 1.0)]:
    r.add(f"test_zero_ifnull_literals[{pid}]#1", scalar(lambda dtype=dtype: ibis.null().cast(dtype).fill_null(0)))
    r.add(
        f"test_zero_ifnull_literals[{pid}]#2",
        scalar(lambda dtype=dtype, expected=expected: ibis.literal(expected, type=dtype).fill_null(0)),
    )

r.add("test_zero_ifnull_column", column(lambda t: t.int_col.nullif(1).fill_null(0)))
r.add("test_select_filter", table(lambda t: t.select("int_col", "string_col").filter(t.string_col == "4")))
r.add(
    "test_select_filter_select",
    table(lambda t: t.select("int_col", "string_col").filter(t.string_col == "4").select("int_col")),
)
r.add("test_between", column(lambda t: t.double_col.between(5, 10)))


def _uncorrelated_subquery(c):
    batting = c.batting
    subset_batting = batting.filter(batting.yearID <= 2000)
    return batting.filter(_.yearID == subset_batting.yearID.max())["playerID", "yearID"]


r.add("test_uncorrelated_subquery", tables(_uncorrelated_subquery))

for method_name in ["any", "notany"]:

    def _exists(c, method_name=method_name):
        years = [1980, 1981]
        batting = c.batting.filter(c.batting.yearID.isin([1871, *years]))
        awards_players = c.awards_players.filter(c.awards_players.yearID.isin(years))
        method = operator.methodcaller(method_name)
        return batting.filter(method(batting.yearID == awards_players.yearID))

    r.add(f"test_exists[{method_name}]", tables(_exists))


def _isin_uncorrelated_simple(c):
    u1, a = c.mem("ids_123"), c.mem("ids_12")
    return u1.mutate(in_a=u1["id"].isin(a["id"])).order_by("id")


r.add("test_isin_uncorrelated_simple", tables(_isin_uncorrelated_simple, ordered=True))
r.add(
    "test_isin_uncorrelated",
    tables(
        lambda c: c.batting.select(
            "playerID", "yearID", has_year_id=c.batting.yearID.isin(c.awards_players.yearID)
        ).order_by(["playerID", "yearID"]),
        ordered=True,
    ),
)
r.add(
    "test_isin_uncorrelated_filter",
    tables(
        lambda c: c.batting.select("playerID", "yearID")
        .filter(c.batting.yearID.isin(c.awards_players.yearID))
        .order_by(["playerID", "yearID"]),
        ordered=True,
    ),
)

r.add(
    "test_select_distinct_order_by", table(lambda t: t.select("int_col").distinct().order_by("int_col"), ordered=True)
)
r.add(
    "test_select_distinct_order_by_expr",
    table(lambda t: t.select("int_col").distinct().order_by(-_.int_col), ordered=True),
)

for ops in permutations(("select", "distinct", "filter", "order_by")):
    if ops.index("select") > ops.index("distinct"):
        continue

    def _commute(t, ops=ops):
        expr = t.select("int_col", "float_col", b=t.id % 33)
        for op in ops:
            if op == "select":
                expr = expr.select("int_col", "b")
            elif op == "distinct":
                expr = expr.distinct()
            elif op == "filter":
                expr = expr.filter(expr.int_col > 5)
            elif op == "order_by":
                expr = expr.order_by(-expr.int_col, expr.b)
        return expr

    r.add(f"test_select_distinct_filter_order_by_commute[{'-'.join(ops)}]", table(_commute, ordered=True))

r.params(
    "test_cast",
    {
        "int_to_float": scalar(lambda: ibis.literal(0, type="int").cast("float")),
        "float_to_int": scalar(lambda: ibis.literal(0.0, type="float").cast("int")),
        "string_to_int": scalar(lambda: ibis.literal("0", type="string").cast("int")),
        "string_to_float": scalar(lambda: ibis.literal("0", type="string").cast("float")),
    },
)

SLICES = {
    "[:0]": slice(None, 0),
    "[:]": slice(None, None),
    "[0:0]": slice(0, 0),
    "[0:]": slice(0, None),
    "[:2]": slice(None, 2),
    "[0:2]": slice(0, 2),
    "[3:0]": slice(3, 0),
    "[3:]": slice(3, None),
    "[3:2]": slice(3, 2),
    "[3:4]": slice(3, 4),
}
for pid, slc in SLICES.items():
    r.add(f"test_static_table_slice[{pid}]", table(lambda t, slc=slc: t[slc].aggregate(n=_.count())))


def _isnull_equality(c):
    t = c.mem("isnull_eq")
    return t.mutate(out=t.x.isnull() == t.y.isnull()).order_by("z").select("out")


r.add("test_isnull_equality", tables(_isnull_equality, ordered=True))
r.add("test_topk_counts_null", tables(lambda c: c.mem("topk").x.topk(10).filter(_.x.isnull())[[1]]))
r.add("test_null_isin_null_is_null", tables(lambda c: c.mem("x_one").select(tmp=_.x.isin([None]))))
r.add(
    "test_value_counts_on_tables",
    table(lambda t: t[["bigint_col", "int_col"]].value_counts().order_by(s.all()), ordered=True),
)
r.add("test_order_by_preservation", tables(lambda c: c.mem("id_col").order_by("id").select("col").distinct()))

# Later sort keys take precedence; earlier ones break ties. Both tests compare
# the whole frame in Ibis; a few columns are enough to pin the order.
r.add(
    "test_subsequent_overlapping_order_by#1",
    table(lambda t: t.order_by(ibis.desc("id")).order_by("id").select("id"), ordered=True),
)
r.add(
    "test_subsequent_overlapping_order_by#2",
    table(lambda t: t.order_by(ibis.desc("id")).order_by("id").order_by(ibis.desc("id")).select("id"), ordered=True),
)
r.add(
    "test_select_sort_sort",
    table(
        lambda t: t.order_by(t.year, t.id.desc()).order_by(t.bool_col).select("bool_col", "year", "id"), ordered=True
    ),
)

for dtype in [
    "bool", "bytes", "str", "int", "float", "int8", "int16", "int32", "int64", "float32", "float64", "timestamp",
    "date",
]:  # fmt: skip
    r.add(f"test_literal_na[{dtype}]", scalar(lambda dtype=dtype: ibis.literal(None, type=dtype)))

# --- more of test_generic.py --------------------------------------------------
import decimal  # noqa: E402

r.add("test_null_literal#1", scalar(lambda: ibis.null()))
r.add("test_null_literal#2", scalar(lambda: ibis.null().cast(str).upper()))
r.add("test_null_literal_typed#1", scalar(lambda: ibis.null(bool)))
r.add("test_null_literal_typed#2", scalar(lambda: ~ibis.null(bool)))
r.add("test_null_literal_typed#3", scalar(lambda: ibis.null(bool).cast(str).upper()))
r.add("test_boolean_literal", scalar(lambda: ibis.literal(False, type="boolean")))
r.add("test_identical_to", column(lambda t: t.tinyint_col.identical_to(t.double_col)))
# the Ibis test compares the whole frame; the ids say which rows are kept
r.add(
    "test_filter_with_window_op",
    table(lambda t: t.filter(lambda t: t["id"].mean().over(ibis.window(group_by=t.id)) > 3).select("id")),
)
r.add("test_mutate_rename", table(lambda t: t.select("id", "bool_col", "string_col").mutate(dupe_col=_.bool_col)))
r.add("test_int_column", table(lambda t: t.mutate(x=ibis.literal(1)).select("x")))
r.add("test_int_scalar", column(lambda t: t.int_col.min()))
r.add(
    "test_select_distinct_order_by_alias",
    tables(lambda c: c.mem("xy_dups").select(y="x", x="y").distinct().order_by("x", "y"), ordered=True),
)
r.add("test_select_mutate_with_dict", table(lambda t: t.mutate({"a": 1.0}).select("a").limit(1)))
r.add("test_select_scalar", table(lambda t: t.select(y=ibis.literal(1)).limit(3)))
r.add(
    "test_select_sort_sort_deferred",
    table(
        lambda t: t.order_by(t.tinyint_col, t.bool_col, t.id)
        .order_by(_.bool_col.asc(), (_.tinyint_col + 1).desc())
        .select("bool_col", "tinyint_col", "id"),
        ordered=True,
    ),
)


def _predictable_aliases(c):
    t = c.mem("penguin")
    sub1 = t.inner_join(t.view(), "island").mutate(island_right=lambda t: t.island)
    sub2 = t.inner_join(t.view(), "sex").mutate(sex_right=lambda t: t.sex)
    return ibis.union(sub1, sub2)


r.add("test_union_generates_predictable_aliases", tables(_predictable_aliases))
r.add(
    "test_comparison_with_decimal_literal",
    tables(lambda c: c.mem("decimal_ints").filter(_.numbers == decimal.Decimal(2))),
)
r.params(
    "test_cast",
    {
        "array": scalar(lambda: ibis.literal([0, 1, 2], type="array<int>").cast("array<string>")),
        "struct": scalar(
            lambda: ibis.literal({"a": 0, "b": "1"}, type="struct<a: int, b: string>").cast("struct<a: string, b: int>")
        ),
    },
)
# a timestamp tried as an integer is NULL on DuckDB, which the Ibis test marks wrong there
r.params(
    "test_try_cast",
    {
        "0-float-0.0": scalar(lambda: ibis.literal(0).try_cast("float")),
        "0.0-int-0": scalar(lambda: ibis.literal(0.0).try_cast("int")),
        "0-int-0": scalar(lambda: ibis.literal("0").try_cast("int")),
        "0.0-float-0.0": scalar(lambda: ibis.literal("0.0").try_cast("float")),
    },
)
r.params(
    "test_try_cast_null",
    {
        "a-int": scalar(lambda: ibis.literal("a").try_cast("int")),
        "2023-01-01 00:00:00-int": scalar(lambda: ibis.timestamp("2023-01-01 00:00:00").try_cast("int")),
    },
)
r.add(
    "test_try_cast_table",
    tables(lambda c: c.mem("try_cast_ab").try_cast({"a": "int", "b": "float"}).order_by("a"), ordered=True),
)
r.params(
    "test_try_cast_func",
    {
        "string-to-float": scalar(lambda: ibis.literal("a").try_cast("float")),
        "datetime-to-float": scalar(lambda: ibis.timestamp("2023-01-01 00:00:00").try_cast("float")),
    },
)
# the Ibis test reads the first ten rows in the order of id
r.add("test_hexdigest", column(lambda t: t.string_col.hexdigest()))
