# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

from __future__ import annotations

import ibis
import ibis.expr.datatypes as dt

from cases import Registry, column, scalar, tables

r = Registry("test_map")


def _no_array():
    return ibis.literal(None, type="array<string>")


def _no_map():
    return ibis.literal(None, type="map<string, string>")


def _no_string():
    return ibis.literal(None, type="string")


def _strings(*values):
    return ibis.literal(list(values), type="array<string>")


NULL_MAPS = {
    "null_values": lambda: ibis.map(_strings("a", "b"), _no_array()),
    "null_keys": lambda: ibis.map(_no_array(), _strings("c", "d")),
    "null_both": lambda: ibis.map(_no_array(), _no_array()),
}

r.params("test_map_nulls", {k: scalar(lambda f=f: f()) for k, f in NULL_MAPS.items()})
r.params(
    "test_map_keys_nulls",
    {k: scalar(lambda k=k: NULL_MAPS[k]().keys()) for k in ["null_keys", "null_both"]},
)
r.params(
    "test_map_values_nulls",
    {
        "null_values": scalar(lambda: ibis.map(ibis.literal(["a", "b"]), _no_array()).values()),
        "null_both": scalar(lambda: NULL_MAPS["null_both"]().values()),
        "null_map": scalar(lambda: _no_map().values()),
    },
)

GET_CONTAINS = {
    "non_null_map_null_key": lambda: (ibis.map(ibis.literal(["a", "b"]), _strings("c", "d")), _no_string()),
    "null_both_non_null_key": lambda: (NULL_MAPS["null_both"](), "a"),
    "null_both_null_key": lambda: (NULL_MAPS["null_both"](), _no_string()),
    "null_map_non_null_key": lambda: (_no_map(), "a"),
    "null_map_null_key": lambda: (_no_map(), _no_string()),
}


def _get_contains(method, case):
    def build():
        map_, key = GET_CONTAINS[case]()
        return getattr(map_, method)(key)

    return scalar(build)


r.params(
    "test_map_get_contains_nulls",
    {f"{method}-{case}": _get_contains(method, case) for method in ["get", "contains"] for case in GET_CONTAINS},
)


def _ab():
    return ibis.literal({"a": "b"}, type="map<string, string>")


r.params(
    "test_map_merge_nulls",
    {
        "null_and_non_null": scalar(lambda: _no_map() + _ab()),
        "non_null_and_null": scalar(lambda: _ab() + _no_map()),
        "null_and_null": scalar(lambda: _no_map() + _no_map()),
    },
)
r.add("test_map_table", tables(lambda c: c.mem("map")))
r.add(
    "test_column_map_values",
    tables(lambda c: c.mem("map").select("idx", vals=lambda t: t.kv.values()).order_by("idx"), ordered=True),
)
r.add(
    "test_column_map_merge",
    tables(
        lambda c: c.mem("map")
        .select("idx", merged=lambda t: t.kv + ibis.map(ibis.literal(["d"]), ibis.literal([1], type="array<int64>")))
        .order_by("idx"),
        ordered=True,
    ),
)
r.add("test_literal_map_keys", scalar(lambda: ibis.literal({"1": "a", "2": "b"}).keys()))
r.add("test_literal_map_values", scalar(lambda: ibis.literal({"1": "a", "2": "b"}).values()))


def _a1_b2():
    return ibis.literal({"a": 1, "b": 2})


r.add("test_scalar_isin_literal_map_keys#1", scalar(lambda: ibis.literal("a").isin(_a1_b2().keys())))
r.add("test_scalar_isin_literal_map_keys#2", scalar(lambda: ibis.literal("c").isin(_a1_b2().keys())))
r.add("test_map_scalar_contains_key_scalar#1", scalar(lambda: _a1_b2().contains(ibis.literal("a"))))
r.add("test_map_scalar_contains_key_scalar#2", scalar(lambda: _a1_b2().contains(ibis.literal("c"))))
r.add(
    "test_map_scalar_contains_key_column",
    column(lambda t: ibis.literal({"1": "a", "3": "c"}).contains(t.string_col)),
)


def _column_map(t):
    return ibis.map(ibis.array([t.string_col]), ibis.array([t.int_col]))


r.add("test_map_column_contains_key_scalar", column(lambda t: _column_map(t).contains("1")))
r.add("test_map_column_contains_key_column", column(lambda t: _column_map(t).contains(t.string_col)))
r.add(
    "test_literal_map_merge",
    scalar(lambda: ibis.literal({"a": 0, "b": 2}) + ibis.literal({"a": 1, "c": 3})),
)
r.add(
    "test_literal_map_getitem_broadcast",
    column(lambda t: ibis.literal({"1": "a", "2": "b"})[t.string_col]),
)

# The two keys (and two values) Ibis builds a map from, per type.
KEYS = {
    "string": lambda: ["a", "b"],
    "int": lambda: [1, 2],
    "bool": lambda: [True, False],
    "float": lambda: [1.0, 2.0],
    "timestamp": lambda: [ibis.timestamp("2021-01-01"), ibis.timestamp("2021-01-02")],
    "date": lambda: [ibis.date(1, 2, 3), ibis.date(4, 5, 6)],
    "array": lambda: [[1, 2], [3, 4]],
    "struct": lambda: [ibis.struct(dict(a=1)), ibis.struct(dict(a=2))],
}
VALUES = dict(KEYS, date=lambda: [ibis.date(2021, 1, 1), ibis.date(2022, 2, 2)])


def _get_all_types(keys, values):
    """Ibis looks up each key in turn; here both lookups are one row."""

    def build(c):
        ks, vs = KEYS[keys](), VALUES[values]()
        m = ibis.map(ibis.array(ks), ibis.array(vs))
        return c.mem("x_one").select(first=m[ks[0]], second=m[ks[1]])

    return tables(build)


def _contains_all_types(keys):
    def build(c):
        ks = KEYS[keys]()
        a = ibis.array(ks)
        m = ibis.map(a, a)
        return c.mem("x_one").select(first=m.contains(ks[0]), second=m.contains(ks[1]))

    return tables(build)


r.params(
    "test_map_get_all_types",
    {f"{keys}-{values}": _get_all_types(keys, values) for values in VALUES for keys in KEYS},
)
r.params("test_map_contains_all_types", {keys: _contains_all_types(keys) for keys in KEYS})
r.add(
    "test_literal_map_get_broadcast",
    column(lambda t: ibis.literal({"1": "a", "2": "b"}).get(t.string_col, "default")),
)
r.params(
    "test_map_construct_dict",
    {
        "string": scalar(lambda: ibis.map(["a", "b"], [1, 2])),
        "int": scalar(lambda: ibis.map(["a", "b"], ["1", "2"])),
    },
)
r.add("test_map_construct_array_column", column(_column_map))
r.add(
    "test_map_get_with_compatible_value_smaller",
    scalar(lambda: ibis.literal({"A": 1000, "B": 2000}).get("C", 3)),
)
r.add(
    "test_map_get_with_compatible_value_bigger",
    scalar(lambda: ibis.literal({"A": 1, "B": 2}).get("C", 3000)),
)
r.add(
    "test_map_get_with_incompatible_value_different_kind",
    scalar(lambda: ibis.literal({"A": 1000, "B": 2000}).get("C", 3.0)),
)


def _int16_map():
    return ibis.literal({"A": 1000, "B": 2000}).cast(dt.Map(dt.string, dt.Int16(nullable=False)))


r.params(
    "test_map_get_with_null_on_not_nullable",
    {
        "None": scalar(lambda: _int16_map().get("C", None)),
        "null_value1": scalar(lambda: _int16_map().get("C", ibis.null())),
    },
)
r.params(
    "test_map_get_with_null_on_null_type_with_null",
    {
        "none": scalar(lambda: ibis.literal({"A": None, "B": None}).get("C", None)),
        "null": scalar(lambda: ibis.literal({"A": None, "B": None}).get("C", ibis.null())),
    },
)
r.add(
    "test_map_get_with_null_on_null_type_with_non_null",
    scalar(lambda: ibis.literal({"A": None, "B": None}).get("C", 1)),
)
r.add("test_map_length", scalar(lambda: ibis.literal(dict(a="A", b="B")).length()))
r.add("test_map_keys_unnest", tables(lambda c: c.mem("map").select(tmp=lambda t: t.kv.keys().unnest())))


def _null_value_map():
    return ibis.map(["a"], ibis.literal([None], type="array<string>"))


r.add("test_map_contains_null#1", scalar(lambda: _null_value_map().contains("a")))
r.add("test_map_contains_null#2", scalar(lambda: _null_value_map().contains("b")))
