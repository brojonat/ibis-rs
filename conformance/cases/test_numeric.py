# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

from __future__ import annotations

from cases import Registry, column

r = Registry("test_numeric")
register = r.params

register(
    "test_simple_math_functions_columns",
    {
        "abs-neg": column(lambda t: (-t.double_col).abs()),
        "abs": column(lambda t: t.double_col.abs()),
        "ceil": column(lambda t: t.double_col.ceil()),
        "floor": column(lambda t: t.double_col.floor()),
        "sign": column(lambda t: t.double_col.sign()),
        "sign-negative": column(lambda t: (-t.double_col).sign()),
    },
)

register(
    "test_complex_math_functions_columns",
    {
        "sqrt": column(lambda t: t.double_col.add(1).sqrt()),
        "exp": column(lambda t: t.double_col.add(1).exp()),
        "log2": column(lambda t: t.double_col.add(1).log2()),
        "ln": column(lambda t: t.double_col.add(1).ln()),
        "log10": column(lambda t: t.double_col.add(1).log10()),
    },
)

# test_binary_arithmetic_operations: op(double_col, smallint_col + 1)
register(
    "test_binary_arithmetic_operations",
    {
        "add": column(lambda t: t.double_col + (t.smallint_col + 1)),
        "sub": column(lambda t: t.double_col - (t.smallint_col + 1)),
        "mul": column(lambda t: t.double_col * (t.smallint_col + 1)),
        "truediv": column(lambda t: t.double_col / (t.smallint_col + 1)),
        "floordiv": column(lambda t: t.double_col // (t.smallint_col + 1)),
        "pow": column(lambda t: t.double_col ** (t.smallint_col + 1)),
    },
)

r.add("test_mod", column(lambda t: t.smallint_col % (t.smallint_col + 1)))
r.add("test_floating_mod", column(lambda t: t.double_col % (t.smallint_col + 1)))

# --- more of test_numeric.py -------------------------------------------------
# test_trig_functions_columns / test_cotangent scale double_col by its max,
# which the Ibis test reads from the data: 90.9.
import ibis  # noqa: E402
import numpy as np  # noqa: E402
from ibis import _  # noqa: E402

from cases import scalar, table  # noqa: E402

L = ibis.literal
DC_MAX = 90.9

OPERANDS = {
    "float-column": column(lambda t: t.float_col),
    "double-column": column(lambda t: t.double_col),
    "float-literal": scalar(lambda: ibis.literal(1.3)),
    "nan-literal": scalar(lambda: ibis.literal(np.nan)),
    "inf-literal": scalar(lambda: ibis.literal(np.inf)),
    "-inf-literal": scalar(lambda: ibis.literal(-np.inf)),
}
for fn in ["isnan", "isinf"]:
    for oid, operand in OPERANDS.items():
        if operand.kind == "column":
            case = column(lambda t, fn=fn, operand=operand: getattr(operand.fn(t), fn)())
        else:
            case = scalar(lambda fn=fn, operand=operand: getattr(operand.fn(), fn)())
        r.add(f"test_isnan_isinf[{fn}-{oid}]", case)

MATH_LITERALS = {
    "abs-neg": lambda: L(-5).abs(),
    "abs": lambda: L(5).abs(),
    "least": lambda: ibis.least(L(10), L(1)),
    "greatest": lambda: ibis.greatest(L(10), L(1)),
    "round": lambda: L(5.5).round(),
    "round-digits": lambda: L(5.556).round(2),
    "ceil": lambda: L(5.556).ceil(),
    "floor": lambda: L(5.556).floor(),
    "exp": lambda: L(5.556).exp(),
    "sign-pos": lambda: L(5.556).sign(),
    "sign-neg": lambda: L(-5.556).sign(),
    "sign-zero": lambda: L(0).sign(),
    "sqrt": lambda: L(5.556).sqrt(),
    "log-base": lambda: L(5.556).log(2),
    "ln0": lambda: L(5.556).ln(),
    "ln1": lambda: L(5.556).ln(),
    "log2": lambda: L(5.556).log2(),
}
# Ibis lists these four three times each (pytest numbers the duplicate ids)
for i in range(3):
    MATH_LITERALS[f"log10_{i}"] = lambda: L(5.556).log10()
    MATH_LITERALS[f"radians{i}"] = lambda: L(5.556).radians()
    MATH_LITERALS[f"degrees{i}"] = lambda: L(5.556).degrees()
    MATH_LITERALS[f"mod{i}"] = lambda: L(11) % 3
register("test_math_functions_literals", {k: scalar(f) for k, f in MATH_LITERALS.items()})

register(
    "test_trig_functions_literals",
    {
        "acos": scalar(lambda: L(0.0).acos()),
        "asin": scalar(lambda: L(0.0).asin()),
        "atan": scalar(lambda: L(0.0).atan()),
        "atan2": scalar(lambda: L(0.0).atan2(1.0)),
        "cos": scalar(lambda: L(0.0).cos()),
        "cot": scalar(lambda: L(1.0).cot()),
        "sin": scalar(lambda: L(0.0).sin()),
        "tan": scalar(lambda: L(0.0).tan()),
    },
)

TRIG_COLUMNS = {
    "acos": _.dc.acos(),
    "asin": _.dc.asin(),
    "atan": _.dc.atan(),
    "atan2": _.dc.atan2(_.dc),
    "cos": _.dc.cos(),
    "sin": _.dc.sin(),
    "tan": _.dc.tan(),
}
register(
    "test_trig_functions_columns",
    {
        k: table(lambda t, e=e: t.mutate(dc=_.double_col / DC_MAX).select(tmp=e))
        for k, e in TRIG_COLUMNS.items()
    },
)
r.add("test_cotangent", table(lambda t: t.select(tmp=(0.5 + _.double_col / DC_MAX).cot())))
r.add("test_floor_divide_precedence", scalar(lambda: ibis.literal(16) // (4 / ibis.literal(2))))

register(
    "test_backend_specific_numerics",
    {
        "round": column(lambda t: t.double_col.round()),
        "round-with-param": column(lambda t: t.double_col.add(0.05).round(3)),
        "least-all-columns": column(lambda t: ibis.least(t.bigint_col, t.int_col)),
        "least-scalar": column(lambda t: ibis.least(t.bigint_col, t.int_col, -2)),
        "greatest-all-columns": column(lambda t: ibis.greatest(t.bigint_col, t.int_col)),
        "greatest-scalar": column(lambda t: ibis.greatest(t.bigint_col, t.int_col, -2)),
    },
)
r.add("test_integer_truediv", scalar(lambda: 1 / ibis.literal(2)))

for denominator in [0, 0.0]:
    for col in ["tinyint_col", "smallint_col", "int_col", "bigint_col", "float_col", "double_col"]:
        r.add(
            f"test_divide_by_zero[{col}-{denominator}]",
            column(lambda t, col=col, denominator=denominator: t[col] / denominator),
        )

register(
    "test_clip",
    {
        k: column(lambda t, f=f: f(t.int_col))
        for k, f in {
            "lower-int": lambda x: x.clip(lower=0),
            "lower-float": lambda x: x.clip(lower=0.0),
            "upper-int": lambda x: x.clip(upper=0),
            "lower-upper-expr": lambda x: x.clip(lower=x - 1, upper=x + 1),
            "lower-upper-int": lambda x: x.clip(lower=0, upper=1),
            "lower-upper-float": lambda x: x.clip(lower=0, upper=1.0),
            "null-lower": lambda x: x.nullif(1).clip(lower=0),
            "null-upper": lambda x: x.nullif(1).clip(upper=0),
        }.items()
    },
)
register("test_constants", {"pi": scalar(lambda: ibis.pi), "e": scalar(lambda: ibis.e)})

# --- the rest of test_numeric.py ---------------------------------------------
import decimal  # noqa: E402
import operator  # noqa: E402

import ibis.expr.datatypes as dt  # noqa: E402

from cases import tables  # noqa: E402

# float16 is left out: DuckDB has no such type and neither has ibis-rs
register(
    "test_numeric_literal",
    {
        name: scalar(lambda name=name: ibis.literal(1, type=name))
        for name in ["int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64", "float32", "float64"]
    },
)
# decimal-big is past what DuckDB holds; infinity and NaN are not decimals there
register(
    "test_decimal_literal",
    {
        "default": scalar(lambda: ibis.literal(decimal.Decimal("1.1"), type=dt.decimal)),
        "decimal-small": scalar(lambda: ibis.literal(decimal.Decimal("1.1"), type=dt.Decimal(38, 9))),
    },
)
register(
    "test_complex_math_functions_columns",
    {
        "log2-explicit": column(lambda t: t.double_col.add(1).log(2)),
        "log_base_bigint": column(lambda t: (t.double_col + 1).log(ibis.greatest(9_000, t.bigint_col))),
    },
)
r.add("test_histogram#1", column(lambda t: t.int_col.histogram(nbins=10)))
r.add(
    "test_histogram#2",
    tables(
        lambda c: c.mem("range100")
        .select(bin=_.value.histogram(nbins=10))
        .value_counts()
        .aggregate(tmp=_.bin_count.nunique())
    ),
)

BITWISE = {"and_": operator.and_, "or_": operator.or_, "xor": operator.xor}
SHIFTS = {"lshift": operator.lshift, "rshift": operator.rshift}
SIDES = {
    "col_col": (lambda t: t.int_col, lambda t: t.int_col),
    "scalar_col": (lambda t: 3, lambda t: t.int_col),
    "col_scalar": (lambda t: t.int_col, lambda t: 3),
}
for sid, (left, right) in SIDES.items():
    for oid, op in BITWISE.items():
        r.add(
            f"test_bitwise_columns[{sid}-{oid}]",
            column(lambda t, op=op, left=left, right=right: op(left(t), right(t))),
        )
for oid, op in SHIFTS.items():
    for sid, (left, right) in SIDES.items():
        r.add(
            f"test_bitwise_shift[{oid}_{sid}]",
            column(lambda t, op=op, left=left, right=right: op(left(t), right(t))),
        )
for oid, op in {**BITWISE, **SHIFTS}.items():
    r.add(f"test_bitwise_scalars[int_col-{oid}]", scalar(lambda op=op: op(4, L(2))))
    r.add(f"test_bitwise_scalars[col_int-{oid}]", scalar(lambda op=op: op(L(4), 2)))
r.add("test_bitwise_not_scalar", scalar(lambda: ~L(2)))
r.add("test_bitwise_not_col", column(lambda t: ~t.int_col))
r.add("test_column_round_is_integer", tables(lambda c: c.mem("x_floats").select(tmp=_.x.round().cast(int))))
r.add("test_scalar_round_is_integer", scalar(lambda: ibis.literal(1.2).round().cast(int)))
# the Ibis test checks the schema the table comes back with
register(
    "test_memtable_decimal",
    {"ints": tables(lambda c: c.mem("decimal_ints")), "decimals": tables(lambda c: c.mem("decimal_decimals"))},
)
