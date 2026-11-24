# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

"""ibis/backends/tests/test_param.py

Each Ibis test runs an expression with a parameter and a value for it, and
expects what the expression gives with the value written in. The Python side
here is that expectation; the Rust side builds the expression with a
parameter and binds the value.
"""

from __future__ import annotations

import datetime
from collections import OrderedDict

import ibis
from ibis import _

from cases import Registry, column, scalar, table

r = Registry("test_param")
L = ibis.literal

for col, raw in [("double_col", 0.0), ("double_col", 10.1), ("float_col", 1.1), ("float_col", 2.2)]:
    r.add(
        f"test_floating_scalar_parameter[{col}-{raw}]",
        column(lambda t, col=col, raw=raw: t[col] + L(raw, type="float64")),
    )
for start, end in [("2009-03-01", "2010-07-03"), ("2014-12-01", "2017-01-05")]:
    r.add(
        f"test_date_scalar_parameter[{start}-{end}]",
        column(lambda t, start=start, end=end: t.timestamp_col.date().between(ibis.date(start), ibis.date(end))),
    )
# the Ibis test compiles this and no more
r.add(
    "test_timestamp_accepts_date_literals",
    table(lambda t: t.select("id", param=L("2009-03-01").cast("timestamp"))),
)
r.add("test_scalar_param_array", scalar(lambda: L([1, 2, 3], type="array<int64>").length()))
r.add(
    "test_scalar_param_struct",
    scalar(lambda: L(dict(a=1, b="abc", c=3.0), type="struct<a: int64, b: string, c: float64>")["a"]),
)
r.add(
    "test_scalar_param_map",
    scalar(lambda: L({"a": "ghi", "b": "def", "c": "abc"}, type="map<string, string>")["b"]),
)
SCALARS = {
    "string": ("0", "string", "string_col"),
    "int": (0, "int64", "int_col"),
    "double": (0.0, "float64", "double_col"),
    "bool": (True, "bool", "bool_col"),
    "string_timestamp": ("2009-01-20 01:02:03", "timestamp", "timestamp_col"),
    "date_timestamp": (datetime.date(2009, 1, 20), "timestamp", "timestamp_col"),
    "datetime_timestamp": (datetime.datetime(2009, 1, 20, 1, 2, 3), "timestamp", "timestamp_col"),
}
# the Ibis test compares whole rows; the ids say which rows are kept
for pid, (value, dtype, col) in SCALARS.items():
    r.add(
        f"test_scalar_param[{pid}]",
        table(lambda t, value=value, dtype=dtype, col=col: t.filter(_[col] == L(value, type=dtype)).select("id")),
    )


def _date_col(t):
    ds = t.date_string_col
    return (("20" + ds[6:8]) + "-" + ds[:2] + "-" + ds[3:5]).cast("date")


for pid, value in {
    "string": "2009-01-20",
    "date": datetime.date(2009, 1, 20),
    "datetime": datetime.datetime(2009, 1, 20),
}.items():
    r.add(
        f"test_scalar_param_date[{pid}]",
        table(
            lambda t, value=value: t.mutate(date_col=_date_col(t)).filter(_.date_col == L(value, type="date")).select("id")
        ),
    )
r.add(
    "test_scalar_param_nested",
    scalar(
        lambda: L(
            OrderedDict([("x", [OrderedDict([("y", [1.0, 2.0, 3.0])])])]),
            type="struct<x: array<struct<y: array<double>>>>",
        )
    ),
)
