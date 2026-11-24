# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

"""ibis/backends/tests/test_temporal.py

Ported so far: field extraction, truncation, literals and constructors,
strftime with a literal format, timestamps and dates moved by intervals. Not
yet: time zones, the `time` type, deltas and buckets, and the two differences
of test_temporal_binop (their result is an interval). Also left out, because
Python Ibis on DuckDB can't produce a golden:
test_timestamp_extract_epoch_seconds (its result doesn't fit the int32 Ibis
declares) and truncation to "ns".

Tests with several assertions become `name#1`, `name#2`.
"""

from __future__ import annotations

import ibis
import ibis.expr.datatypes as dt
import pandas as pd

from cases import Registry, column, scalar, table

r = Registry("test_temporal")

for attr in ["year", "month", "day"]:
    r.add(f"test_date_extract[date-{attr}]", column(lambda t, attr=attr: getattr(t.timestamp_col.date(), attr)()))
    r.add(
        f"test_date_extract[cast-{attr}]", column(lambda t, attr=attr: getattr(t.timestamp_col.cast("date"), attr)())
    )

for attr in ["year", "month", "day", "day_of_year", "quarter", "hour", "minute", "second"]:
    r.add(f"test_timestamp_extract[{attr}]", column(lambda t, attr=attr: getattr(t.timestamp_col, attr)()))

VALUE = "2015-09-01 14:48:05.359"
r.params(
    "test_timestamp_extract_literal",
    {
        "year": scalar(lambda: ibis.timestamp(VALUE).year()),
        "month": scalar(lambda: ibis.timestamp(VALUE).month()),
        "day": scalar(lambda: ibis.timestamp(VALUE).day()),
        "hour": scalar(lambda: ibis.timestamp(VALUE).hour()),
        "minute": scalar(lambda: ibis.timestamp(VALUE).minute()),
        "second": scalar(lambda: ibis.timestamp(VALUE).second()),
        "millisecond": scalar(lambda: ibis.timestamp(VALUE).millisecond()),
        "day_of_week_index": scalar(lambda: ibis.timestamp(VALUE).day_of_week.index()),
        "day_of_week_full_name": scalar(lambda: ibis.timestamp(VALUE).day_of_week.full_name()),
    },
)

r.add("test_timestamp_extract_microseconds", column(lambda t: t.timestamp_col.microsecond()))
r.add("test_timestamp_extract_milliseconds", column(lambda t: t.timestamp_col.millisecond()))
r.add("test_timestamp_extract_week_of_year", column(lambda t: t.timestamp_col.week_of_year()))

TRUNCATE = {
    "year": "Y", "quarter": "Q", "month": "M", "W-W": "W", "D-D": "D", "h-h": "h", "m-min": "m", "s-s": "s",
    "ms-ms": "ms", "us-us": "us",
}  # fmt: skip
for pid, unit in TRUNCATE.items():
    r.add(f"test_timestamp_truncate[{pid}]", column(lambda t, unit=unit: t.timestamp_col.truncate(unit)))
for unit in ["Y", "Q", "M", "D", "W"]:
    r.add(f"test_date_truncate[{unit}]", column(lambda t, unit=unit: t.timestamp_col.date().truncate(unit)))

r.add(
    "test_strftime[literal_format_str]", column(lambda t: t.timestamp_col.strftime("%Y%m%d")),
)

DAYS = {
    "sunday": "2017-01-01", "monday": "2017-01-02", "tuesday": "2017-01-03", "wednesday": "2017-01-04",
    "thursday": "2017-01-05", "friday": "2017-01-06", "saturday": "2017-01-07",
}  # fmt: skip
for pid, date in DAYS.items():
    r.add(
        f"test_day_of_week_scalar[{pid}]#1",
        scalar(lambda date=date: ibis.literal(date).cast(dt.date).day_of_week.index()),
    )
    r.add(
        f"test_day_of_week_scalar[{pid}]#2",
        scalar(lambda date=date: ibis.literal(date).cast(dt.date).day_of_week.full_name()),
    )

r.add("test_day_of_week_column#1", column(lambda t: t.timestamp_col.day_of_week.index()))
r.add("test_day_of_week_column#2", column(lambda t: t.timestamp_col.day_of_week.full_name()))
r.params(
    "test_day_of_week_column_group_by",
    {
        "day_of_week_index": table(
            lambda t: t.group_by("string_col").aggregate(
                day_of_week_result=t.timestamp_col.day_of_week.index().count()
            )
        ),
        "day_of_week_full_name": table(
            lambda t: t.group_by("string_col").aggregate(
                day_of_week_result=t.timestamp_col.day_of_week.full_name().length().sum()
            )
        ),
    },
)

r.add("test_date_literal", scalar(lambda: ibis.date(2022, 2, 4)))
r.add("test_timestamp_literal", scalar(lambda: ibis.timestamp(2022, 2, 4, 16, 20, 0)))
r.add(
    "test_date_column_from_ymd",
    table(
        lambda t: t.select(
            "id", ibis.date(t.timestamp_col.year(), t.timestamp_col.month(), t.timestamp_col.day()).name("timestamp_col")
        )
    ),
)


def _from_ymdhms(t):
    c = t.timestamp_col
    return t.select(
        "id", ibis.timestamp(c.year(), c.month(), c.day(), c.hour(), c.minute(), c.second()).name("timestamp_col")
    )


r.add("test_timestamp_column_from_ymdhms", table(_from_ymdhms))
r.add("test_date_scalar_from_iso", scalar(lambda: ibis.date(ibis.literal("2022-02-24"))))
r.add(
    "test_date_column_from_iso",
    column(lambda t: ibis.date(t.year.cast("string") + "-" + t.month.cast("string").lpad(2, "0") + "-13")),
)

# The second half of each id is the pandas class Ibis's test builds the
# expected column with.
INTERVAL_UNITS = {
    "Y-DateOffset": "Y", "Q-DateOffset": "Q", "M-DateOffset": "M", "W-DateOffset": "W", "D-DateOffset": "D",
    "h-Timedelta": "h", "m-Timedelta": "m", "s-Timedelta": "s", "ms-Timedelta": "ms", "us-Timedelta": "us",
}  # fmt: skip
for pid, unit in INTERVAL_UNITS.items():
    r.add(
        f"test_integer_to_interval_timestamp[{pid}]",
        column(lambda t, unit=unit: t.timestamp_col + t.int_col.as_interval(unit)),
    )


def _date_col(t):
    month = t.date_string_col[:2]
    day = t.date_string_col[3:5]
    year = t.date_string_col[6:8]
    return ("20" + year + "-" + month + "-" + day).cast("date")


for unit in ["Y", "Q", "M", "W", "D"]:
    r.add(
        f"test_integer_to_interval_date[{unit}]", column(lambda t, unit=unit: _date_col(t) + t.int_col.as_interval(unit))
    )

r.params(
    "test_temporal_binop",
    {
        "timestamp-add-interval": column(lambda t: t.timestamp_col + ibis.interval(days=4)),
        "timestamp-add-interval-binop": column(
            lambda t: t.timestamp_col + (ibis.interval(days=4) - ibis.interval(days=2))
        ),
        "timestamp-add-interval-binop-different-units": column(
            lambda t: t.timestamp_col + (ibis.interval(days=4) + ibis.interval(hours=2))
        ),
        "timestamp-subtract-interval": column(lambda t: t.timestamp_col - ibis.interval(days=17)),
        "date-add-interval": column(lambda t: t.timestamp_col.date() + ibis.interval(days=4)),
        "date-subtract-interval": column(lambda t: t.timestamp_col.date() - ibis.interval(days=14)),
    },
)

TIMEDELTAS = {
    "large-days": "36500D", "weeks": "5W", "three-days": "3D", "two-hours": "2h", "three-minutes": "3m",
    "ten-seconds": "10s",
}  # fmt: skip
for pid, delta in TIMEDELTAS.items():
    r.add(
        f"test_temporal_binop_pandas_timedelta[{pid}-plus]",
        column(lambda t, delta=delta: t.timestamp_col + pd.Timedelta(delta)),
    )
    r.add(
        f"test_temporal_binop_pandas_timedelta[{pid}-minus]",
        column(lambda t, delta=delta: t.timestamp_col - pd.Timedelta(delta)),
    )

r.add(
    "test_interval_add_cast_scalar",
    column(lambda t: t.timestamp_col.date() + ibis.literal(10).cast("interval('D')")),
)
r.add(
    "test_interval_add_cast_column",
    table(lambda t: t.select("id", (t.timestamp_col.date() + t.bigint_col.cast("interval('D')")).name("tmp"))),
)
