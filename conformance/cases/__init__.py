"""Ibis expressions ported from ibis/backends/tests, keyed by pytest id.

Each entry is the *Ibis side* of a conformance case. gen_goldens.py runs it on
Python Ibis + DuckDB and stores the result as the golden. The Rust side of the
same id lives in crates/ibis-conformance/src/cases/<module>.rs.

One file per Ibis test module; importing this package registers them all.

Shapes (mirroring how the Ibis tests execute them):
- column(fn): fn(t) returns a column expression over functional_alltypes. Run
  as `t.select("id", tmp=expr)` so rows can be compared without relying on
  order; reductions run as `t.aggregate(tmp=expr)`.
- scalar(fn): fn() returns an expression with no columns (Ibis
  `con.execute(expr)`). Run as `t.select(tmp=expr).limit(1)`.
- table(fn): fn(t) returns a table expression over functional_alltypes.
- tables(fn): fn(c) returns a table expression; `c` has the fixture tables
  (`c.alltypes`, `c.batting`, `c.awards_players`) and the small in-memory
  tables of conformance/memtables.json (`c.mem("name")`, Ibis `ibis.memtable`).

Table results are compared as a multiset of rows unless ordered=True, which is
only set when the expression ends in a total ordering.
"""

from __future__ import annotations

import importlib
from dataclasses import dataclass
from typing import Callable

MODULES = [
    "test_string",
    "test_numeric",
    "test_aggregation",
]


@dataclass(frozen=True)
class Case:
    kind: str  # "column" | "scalar" | "table" | "tables"
    fn: Callable
    ordered: bool = False
    module: str = ""


def column(fn: Callable) -> Case:
    return Case("column", fn)


def scalar(fn: Callable) -> Case:
    return Case("scalar", fn)


def table(fn: Callable, ordered: bool = False) -> Case:
    return Case("table", fn, ordered)


def tables(fn: Callable, ordered: bool = False) -> Case:
    return Case("tables", fn, ordered)


CASES: dict[str, Case] = {}


class Registry:
    """Registers cases for one Ibis test module."""

    def __init__(self, module: str) -> None:
        self.module = module

    def add(self, case_id: str, case: Case) -> None:
        if case_id in CASES:
            raise ValueError(f"duplicate case id {case_id}")
        CASES[case_id] = Case(case.kind, case.fn, case.ordered, self.module)

    def params(self, test: str, params: dict[str, Case]) -> None:
        for pid, case in params.items():
            self.add(f"{test}[{pid}]", case)


def load() -> dict[str, Case]:
    for m in MODULES:
        importlib.import_module(f"cases.{m}")
    return CASES
