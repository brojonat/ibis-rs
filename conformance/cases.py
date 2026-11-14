"""Ibis expressions ported verbatim from ibis/backends/tests, keyed by pytest id.

Each entry is the *Ibis side* of a conformance case. gen_goldens.py runs it on
Python Ibis + DuckDB and stores the result as the golden. The Rust side of the
same id lives in crates/ibis-conformance/src/cases.rs.

Shapes (mirroring how the Ibis tests execute them):
- column(fn): fn(t) returns a column expression. Run as
  `t.select("id", tmp=expr)` so rows can be compared without relying on order;
  reductions run as `t.aggregate(tmp=expr)`.
- table(fn): fn(t) returns a table expression, compared as a multiset of rows
  (or in order with ordered=True).
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Callable

import ibis


@dataclass(frozen=True)
class Case:
    kind: str  # "column" | "table"
    fn: Callable
    ordered: bool = False


def column(fn: Callable) -> Case:
    return Case("column", fn)


def table(fn: Callable, ordered: bool = False) -> Case:
    return Case("table", fn, ordered)


CASES: dict[str, Case] = {}


def register(test: str, params: dict[str, Case]) -> None:
    for pid, case in params.items():
        CASES[f"{test}[{pid}]"] = case


# --- ibis/backends/tests/test_string.py -------------------------------------

register(
    "test_string",
    {
        "contains": column(lambda t: t.string_col.contains("6")),
        "like": column(lambda t: t.string_col.like("6%")),
        "complex_like_escape": column(lambda t: t.string_col.like("6^%")),
        "complex_like_escape_match": column(lambda t: t.string_col.like("6^%%")),
        "ilike": column(lambda t: t.string_col.ilike("6%")),
        "rlike": column(lambda t: t.string_col.rlike("|".join(map(str, range(10))))),
        "re_search_substring": column(lambda t: ("a" + t.string_col + "a").re_search(r"\d+")),
        "re_search": column(lambda t: t.string_col.re_search(r"\d+")),
        "re_search_posix": column(lambda t: t.string_col.re_search(r"[[:digit:]]+")),
        "re_extract": column(lambda t: ("xyz" + t.string_col + "abcd").re_extract(r"(\d+)", 0)),
        "re_extract_group": column(lambda t: ("xyz" + t.string_col + "abcd").re_extract(r"(\d+)abc", 1)),
        "re_extract_posix": column(lambda t: t.string_col.re_extract(r"([[:digit:]]+)", 1)),
        "re_extract_whole_group": column(lambda t: (t.string_col + "1").re_extract(r"\d(\d+)", 0)),
        "re_extract_group_1": column(lambda t: t.date_string_col.re_extract(r"(\d+)\D(\d+)\D(\d+)", 1)),
        "re_extract_group_2": column(lambda t: t.date_string_col.re_extract(r"(\d+)\D(\d+)\D(\d+)", 2)),
        "re_extract_group_3": column(lambda t: t.date_string_col.re_extract(r"(\d+)\D(\d+)\D(\d+)", 3)),
        "re_extract_group_at_beginning": column(lambda t: t.date_string_col.re_extract(r"^(\d+)", 1)),
        "re_extract_group_at_end": column(lambda t: t.date_string_col.re_extract(r"(\d+)$", 1)),
        "re_replace_posix": column(lambda t: t.string_col.re_replace(r"[[:digit:]]+", "a")),
        "re_replace": column(lambda t: t.string_col.re_replace(r"\d+", "a")),
        "repeat_method": column(lambda t: t.string_col.repeat(2)),
        "repeat_left": column(lambda t: 2 * t.string_col),
        "repeat_right": column(lambda t: t.string_col * 2),
        "find": column(lambda t: t.string_col.find("a")),
        "find_start": column(lambda t: t.date_string_col.find("13", 3)),
        "lpad": column(lambda t: t.string_col.lpad(10, "a")),
        "rpad": column(lambda t: t.string_col.rpad(10, "a")),
        "lower": column(lambda t: t.string_col.lower()),
        "upper": column(lambda t: t.string_col.upper()),
        "reverse": column(lambda t: t.string_col.reverse()),
        "ascii_str": column(lambda t: t.string_col.ascii_str()),
        "length": column(lambda t: t.string_col.length()),
        "startswith": column(
            lambda t: t.int_col.cases((1, "abcd"), (2, "ABCD"), else_="dabc").startswith("abc")
        ),
        "endswith": column(
            lambda t: t.int_col.cases((1, "abcd"), (2, "ABCD"), else_="dabc").endswith("bcd")
        ),
        "startswith-simple": column(lambda t: t.date_string_col.startswith("2010-01")),
        "endswith-simple": column(lambda t: t.date_string_col.endswith("/10")),
        "strip": column(lambda t: t.string_col.strip()),
        "lstrip": column(lambda t: t.string_col.lstrip()),
        "rstrip": column(lambda t: t.string_col.rstrip()),
        "concat_columns": column(lambda t: t.string_col + t.date_string_col),
        "concat_column_scalar": column(lambda t: t.string_col + "a"),
        "concat_scalar_column": column(lambda t: "a" + t.string_col),
        "replace": column(lambda t: t.string_col.replace("1", "42")),
    },
)

# test_substring applies each function to `alltypes.date_string_col`.
register(
    "test_substring",
    {
        "substr": column(lambda t: t.date_string_col.substr(2, 3)),
        "substr-start-only": column(lambda t: t.date_string_col.substr(2)),
        "left": column(lambda t: t.date_string_col.left(2)),
        "right": column(lambda t: t.date_string_col.right(2)),
    },
)

# --- ibis/backends/tests/test_numeric.py ------------------------------------

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

CASES["test_mod"] = column(lambda t: t.smallint_col % (t.smallint_col + 1))
CASES["test_floating_mod"] = column(lambda t: t.double_col % (t.smallint_col + 1))

# --- ibis/backends/tests/test_aggregation.py --------------------------------

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

        CASES[f"test_reduction_ops[{cid}-{rid}]"] = table(build)


def module_of(case_id: str) -> str:
    """Which Ibis test module a case id came from (for golden file layout)."""
    test = case_id.split("[", 1)[0]
    if test in {"test_string", "test_substring"}:
        return "test_string"
    if test.startswith(("test_aggregate", "test_reduction")):
        return "test_aggregation"
    return "test_numeric"
