# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

from __future__ import annotations

import ibis

from cases import Registry, column, scalar, tables

r = Registry("test_array")

r.add(
    "test_array_column",
    column(lambda t: ibis.array([t.double_col, t.double_col, 5.0, ibis.literal(6.0)])),
)
r.add("test_array_scalar", scalar(lambda: ibis.array([1.0, 2.0, 3.0])))
r.add("test_array_repeat", scalar(lambda: ibis.array([1.0, 2.0]) * 2))
r.add("test_array_repeat_column", tables(lambda c: c.mem("arr_x_f64").select(tmp=lambda t: t.x * 2)))
r.add("test_array_concat", scalar(lambda: ibis.literal([1, 2, 3]) + ibis.literal([2, 1])))
r.add(
    "test_array_concat_variadic",
    scalar(lambda: ibis.literal([1, 2, 3]).concat(ibis.literal([2, 1]), ibis.literal([2, 1]), ibis.literal([2, 1]))),
)
r.add("test_array_concat_some_empty", scalar(lambda: ibis.literal([]).concat(ibis.literal([2, 1]))))
r.add("test_array_radd_concat", scalar(lambda: [1] + ibis.literal([2])))
r.params(
    "test_array_concat_scalar",
    {
        "<lambda>0": scalar(lambda: ibis.literal([1, 2, 3]) + ibis.literal([3, 4])),
        "<lambda>1": scalar(lambda: ibis.literal([3, 4]) + ibis.literal([1, 2, 3])),
    },
)


def _max_int32():
    return ibis.array([2**31 - 1])


r.params(
    "test_array_concat_with_null",
    {
        "left": scalar(lambda: _max_int32() + ibis.null(_max_int32().type())),
        "right": scalar(lambda: ibis.null(_max_int32().type()) + _max_int32()),
    },
)
r.add(
    "test_array_concat_with_null_non_constant",
    tables(lambda c: c.mem("arr_a_null").select(tmp=lambda t: t.a + t.a)),
)
r.add("test_array_length", scalar(lambda: ibis.literal([1, 2, 3]).length()))
r.add("test_list_literal", scalar(lambda: ibis.literal([1, 2, 3])))
r.add("test_np_array_literal", scalar(lambda: ibis.literal([1, 2, 3])))
r.params("test_array_index", {str(i): scalar(lambda i=i: ibis.literal([1, 2, 3])[i]) for i in range(3)})
r.add("test_array_discovery", tables(lambda c: c.mem("array_types")))


def _slice(start, stop):
    return tables(lambda c: c.mem("array_types").select(sliced=lambda t: t.y[start:stop]))


r.params(
    "test_array_slice",
    {
        "1-3": _slice(1, 3),
        "1-1": _slice(1, 1),
        "2-3": _slice(2, 3),
        "2-5": _slice(2, 5),
        "None-3": _slice(None, 3),
        "None-None": _slice(None, None),
        "3-None": _slice(3, None),
        "-3-None": _slice(-3, None),
        "-3--1": _slice(-3, -1),
        "nulls": _slice(None, -3),
    },
)


def _array_types(fn, ordered=False):
    return tables(lambda c: fn(c.mem("array_types")), ordered=ordered)


def _mem(name, fn, ordered=False):
    return tables(lambda c: fn(c.mem(name)), ordered=ordered)


INT_ARRAY = "!array<int64>"

r.add(
    "test_unnest_simple",
    _array_types(lambda t: t.select(tmp=t.x.cast("!array<float64>").unnest())),
)
r.add(
    "test_unnest_complex",
    _array_types(
        lambda t: t.select(["grouper", "x"])
        .mutate(x=lambda t: t.x.unnest())
        .group_by("grouper")
        .aggregate(count_flat=lambda t: t.x.count())
        .order_by("grouper"),
        ordered=True,
    ),
)
r.add(
    "test_unnest_idempotent",
    _array_types(
        lambda t: t.select(["scalar_column", t.x.cast(INT_ARRAY).unnest().name("x")])
        .group_by("scalar_column")
        .aggregate(x=lambda t: t.x.collect())
        .mutate(x=lambda t: t.x.sort())
        .order_by("scalar_column"),
        ordered=True,
    ),
)
# Ibis compares in the order `collect` happened to keep; sorted here, as nothing promises that order
r.add(
    "test_unnest_no_nulls",
    _array_types(
        lambda t: t.select(["scalar_column", t.x.cast(INT_ARRAY).unnest().name("y")])
        .filter(lambda t: t.y.notnull())
        .group_by("scalar_column")
        .aggregate(x=lambda t: t.y.collect())
        .mutate(x=lambda t: t.x.sort())
        .order_by("scalar_column"),
        ordered=True,
    ),
)
r.add(
    "test_unnest_default_name",
    _array_types(lambda t: t.select(x=(t.x.cast(INT_ARRAY) + ibis.array([1]).cast(INT_ARRAY)).unnest())),
)

for nulls, table in [("nulls", "arr_int8_nulls"), ("no_nulls", "arr_int8")]:
    for how in ["lambda", "partial", "deferred"]:
        r.add(f"test_array_map[{how}-{nulls}]", _mem(table, lambda t: t.select(a=t.a.map(lambda x: x + 1))))
        r.add(f"test_array_filter[{how}-{nulls}]", _mem(table, lambda t: t.select(a=t.a.filter(lambda x: x > 1))))
    for how in ["lambda", "partial"]:
        r.add(
            f"test_array_map_with_index[{how}-{nulls}]",
            _mem(table, lambda t: t.select(a=t.a.map(lambda x, i: x + 1 + i))),
        )
    r.add(
        f"test_array_filter_with_index[lambda-{nulls}]",
        _mem(table, lambda t: t.select(a=t.a.filter(lambda x, i: x + (i - i) > 1))),
    )
    r.add(
        f"test_array_filter_with_index[partial-{nulls}]",
        _mem(table, lambda t: t.select(a=t.a.filter(lambda x, i: x > 1 + (i * 0)))),
    )
for nulls, table in [("nulls", "arr_int8_idx_nulls"), ("no_nulls", "arr_int8_idx")]:
    for how in ["lambda", "partial"]:
        r.add(
            f"test_array_filter_with_index_lambda[{how}-{nulls}]",
            _mem(table, lambda t: t.select(a=t.a.filter(lambda _, i: i % 2 == 0))),
        )

r.params(
    "test_array_contains",
    {
        "x-1": _array_types(lambda t: t.select(tmp=t.x.contains(1))),
        "y-a": _array_types(lambda t: t.select(tmp=t.y.contains("a"))),
    },
)
r.params(
    "test_array_position",
    {
        "some-empty": _mem("arr_position_some_empty", lambda t: t.select(tmp=t.a.index(42))),
        "none-empty": _mem("arr_position_none_empty", lambda t: t.select(tmp=t.a.index(42))),
    },
)
r.params(
    "test_array_remove",
    {
        "including-empty-array": _mem("arr_remove_empty", lambda t: t.select(tmp=t.a.remove(2))),
        "all-non-empty-arrays": _mem("arr_remove_non_empty", lambda t: t.select(tmp=t.a.remove(2))),
        "nested-null": _mem("arr_remove_nested_null", lambda t: t.select(tmp=t.a.remove(2))),
        "non-nested-null": _mem("arr_remove_null", lambda t: t.select(tmp=t.a.remove(2))),
    },
)
# `unique`, `union` and `intersect` promise no order: the results are sorted to compare them
r.params(
    "test_array_unique",
    {
        "null": _mem("arr_unique_null", lambda t: t.select(tmp=t.a.unique().sort())),
        "not_null": _mem("arr_unique_not_null", lambda t: t.select(tmp=t.a.unique().sort())),
    },
)
r.params(
    "test_array_sort",
    {
        "empty": _mem("arr_sort_empty", lambda t: t.mutate(a=t.a.sort()).order_by("id"), ordered=True),
        "nonempty": _mem("arr_sort_nonempty", lambda t: t.mutate(a=t.a.sort()).order_by("id"), ordered=True),
    },
)
r.params(
    "test_array_union",
    {
        "including-empty-array": _mem("arr_union_empty", lambda t: t.select(tmp=t.a.union(t.b).sort())),
        "all-non-empty-arrays": _mem("arr_union_non_empty", lambda t: t.select(tmp=t.a.union(t.b).sort())),
    },
)


def _intersect(t):
    return t.select("c", d=t.a.intersect(t.b).sort()).order_by("c").drop("c")


r.params(
    "test_array_intersect",
    {
        "nulls": _mem("arr_intersect_nulls", _intersect, ordered=True),
        "no_nulls": _mem("arr_intersect_no_nulls", _intersect, ordered=True),
    },
)
r.add("test_unnest_struct", _mem("arr_struct_a", lambda t: t.select(tmp=t.value.unnest())))
r.add("test_unnest_struct_with_multiple_fields", _mem("arr_struct_ab", lambda t: t.select(tmp=t.value.unnest())))
r.params(
    "test_zip",
    {
        str(n): _array_types(
            lambda t, n=n: t.select(id="scalar_column", zipped=t.x.zip(*([t.x] * n))).order_by("id"), ordered=True
        )
        for n in (1, 5)
    },
)


def _zip_null(fn):
    a = ibis.literal([1, 2, 3], type="array<int64>")
    b = ibis.literal(None, type="array<int64>")
    return scalar(lambda: fn(a, b))


r.params(
    "test_zip_null",
    {
        "non-null-zip-null": _zip_null(lambda a, b: a.zip(b)),
        "null-zip-non-null": _zip_null(lambda a, b: b.zip(a)),
        "null-zip-null": _zip_null(lambda _, b: b.zip(b)),
    },
)
# Ibis unnests the first row only (`limit(1)`), whichever that is; every row here
r.add("test_array_of_struct_unnest", _mem("jobs", lambda t: t.select(status=t.steps.unnest().status)))
r.params(
    "test_array_flatten",
    {
        column: _mem(
            f"flatten_{column}",
            lambda t, column=column: t.select("id", flat=t[column].flatten()).order_by("id"),
            ordered=True,
        )
        for column in ["empty", "happy", "nulls_only", "mixed_nulls"]
    },
)
r.params("test_range_single_argument", {str(n): scalar(lambda n=n: ibis.range(n)) for n in (-2, 0, 2)})
r.params(
    "test_range_single_argument_unnest",
    {str(n): _mem("x_one", lambda t, n=n: t.select(tmp=ibis.range(n).unnest())) for n in (-2, 0, 2)},
)
r.params(
    "test_range_start_stop_step",
    {
        f"{step}-{stop}-{start}": scalar(lambda start=start, stop=stop, step=step: ibis.range(start, stop, step))
        for step in (-2, -1, 1, 2)
        for stop in (-7, 0, 7)
        for start in (-7, 0, 7)
    },
)
r.params(
    "test_range_start_stop_step_zero",
    {
        f"{start}-{stop}": scalar(lambda start=start, stop=stop: ibis.range(start, stop, 0))
        for start in (-7, 0, 7)
        for stop in (-7, 0, 7)
    },
)
r.add("test_unnest_empty_array", _mem("arr_strings", lambda t: t.select(tmp=t.arr.unnest())))
r.add(
    "test_array_map_with_conflicting_names",
    _mem(
        "arr_x_int8",
        lambda t: t.select(a=t.x.map(lambda x: x + 1)).select(b=lambda t: t.a.filter(lambda a: a > 2)),
    ),
)


def _words():
    return ibis.array(["abc", "xyz"])


r.add("test_complex_array_map#1", scalar(lambda: _words().map(lambda token: token.upper())))
r.add("test_complex_array_map#2", scalar(lambda: _words().map(lambda token: token.substitute({"abc": "ABC"}))))
r.add("test_unnest_range", _mem("x_one", lambda t: t.select(x=ibis.range(2).unnest()).mutate(y=1.0)))
r.params(
    "test_array_literal_with_exprs",
    {
        "int-int": scalar(lambda: ibis.array([1, ibis.literal(2)])),
        "float-int": scalar(lambda: ibis.array([1.0, ibis.literal(2)])),
        "float-float": scalar(lambda: ibis.array([1.0, ibis.literal(2.0)])),
        "int-float": scalar(lambda: ibis.array([1, ibis.literal(2.0)])),
        "int-float-exprs": scalar(lambda: ibis.array([ibis.literal(1), ibis.literal(2.0)])),
        "array": scalar(lambda: ibis.array([[1], ibis.literal([2])])),
    },
)


def _zip_unnest_lift(t):
    zipped = t.mutate(zipped=t.array1.zip(t.array2))
    unnested = zipped.mutate(unnest=zipped.zipped.unnest())
    return unnested["unnest"].lift()


r.add("test_zip_unnest_lift", _mem("arr_pair", _zip_unnest_lift))
r.params(
    "test_table_unnest",
    {how: _array_types(lambda t: t.unnest("y")) for how in ["string", "lambda", "deferred"]},
)
r.add(
    "test_table_unnest_with_offset",
    _array_types(lambda t: t.unnest("y", offset="idx")[["idx", "y"]].order_by("idx", "y"), ordered=True),
)
r.add("test_table_unnest_with_keep_empty", _mem("arr_keep_empty", lambda t: t.unnest("y", keep_empty=True)[["y"]]))
r.add(
    "test_table_unnest_column_expr",
    _array_types(lambda t: t.unnest(t.y.map(lambda v: v.cast("str") + "'s").name("plural"))[["plural"]]),
)
r.add(
    "test_table_unnest_array_of_struct_of_array",
    _mem(
        "arr_struct_of_arr",
        lambda t: t.unnest("a").a.x.unnest().name("x").as_table().order_by("x"),
        ordered=True,
    ),
)

for nulls, table in [("nulls", "arr_agg_nulls"), ("no-nulls", "arr_agg_no_nulls")]:
    for agg in ["sums", "mins", "maxs", "means", "modes"]:
        r.add(
            f"test_array_agg_numeric[{nulls}-{agg}]",
            _mem(table, lambda t, agg=agg: t.mutate(y=getattr(t.x, agg)()).order_by("id"), ordered=True),
        )
for nulls, table in [("nulls", "arr_bool_nulls"), ("no-nulls", "arr_bool_no_nulls")]:
    for agg in ["anys", "alls"]:
        r.add(
            f"test_array_agg_bool[{nulls}-{agg}]",
            _mem(table, lambda t, agg=agg: t.mutate(y=getattr(t.x, agg)()).order_by("id"), ordered=True),
        )
r.add("test_flatten", _mem("arr_nested", lambda t: t.select(result=t.arr.flatten())))
