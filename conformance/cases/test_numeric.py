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
