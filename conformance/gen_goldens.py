# /// script
# requires-python = ">=3.11"
# dependencies = ["ibis-framework[duckdb,sqlite,postgres]==12.*", "pyarrow", "pandas", "numpy"]
# ///
"""Run conformance/cases on Python Ibis + DuckDB and write the goldens.

Writes:
  conformance/golden/<module>/<case id>.parquet   expected result (typed)
  conformance/golden/index.json                  per-case metadata (diffable)

For each case the index also records whether Python Ibis *itself* returns the
same rows on SQLite and Postgres as it does on DuckDB ("ibis": {"sqlite":
"same" | "differs" | "error: ..."}). That is the bar for cross-backend
consistency: ibis-rs aims to match the DuckDB golden on every backend, and
this shows where Python Ibis doesn't.

Usage: gen_goldens.py [ID_SUBSTRING]
Env:   IBIS_TESTING_DATA           (default: ci/ibis-testing-data)
       IBIS_TESTING_POSTGRES_URL   libpq keywords; Postgres check skipped if unset
"""

from __future__ import annotations

import json
import math
import os
import sys
from pathlib import Path

import ibis
import ibis.expr.types as ir
import pyarrow as pa
import pyarrow.parquet as pq

sys.path.insert(0, str(Path(__file__).parent))
import cases  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
GOLDEN = ROOT / "conformance" / "golden"
DATA = Path(os.environ.get("IBIS_TESTING_DATA", ROOT / "ci" / "ibis-testing-data"))
TABLES = ["functional_alltypes", "batting", "awards_players"]
MEMTABLES = json.loads((ROOT / "conformance" / "memtables.json").read_text())
ARROW = {"int64": pa.int64(), "string": pa.string(), "float64": pa.float64(), "boolean": pa.bool_()}


class Fixtures:
    """The fixture tables of one connection."""

    def __init__(self, con) -> None:
        self.con = con
        self.alltypes = con.table("functional_alltypes")
        self.batting = con.table("batting")
        self.awards_players = con.table("awards_players")

    def mem(self, name: str) -> ir.Table:
        return self.con.table(name)


def memtable(name: str) -> pa.Table:
    spec = MEMTABLES[name]
    cols = list(zip(*spec["rows"])) if spec["rows"] else [[] for _ in spec["columns"]]
    return pa.table({n: pa.array(list(c), type=ARROW[t]) for (n, t), c in zip(spec["columns"], cols)})


def sql_type(t: pa.DataType, backend: str) -> str:
    """Column type for a native CREATE TABLE (names Ibis's backends read back)."""
    if pa.types.is_boolean(t):
        return "BOOLEAN"
    if pa.types.is_int8(t):
        return "TINYINT" if backend == "sqlite" else "SMALLINT"
    if pa.types.is_int16(t):
        return "SMALLINT"
    if pa.types.is_int32(t):
        return "INTEGER"
    if pa.types.is_integer(t):
        return "BIGINT"
    if pa.types.is_float32(t):
        return "REAL"
    if pa.types.is_floating(t):
        return "DOUBLE PRECISION" if backend == "postgres" else "DOUBLE"
    if pa.types.is_timestamp(t):
        return "TIMESTAMP"
    if pa.types.is_date(t):
        return "DATE"
    return "TEXT"


def create(con, name: str, data: pa.Table) -> None:
    """Load a table natively: Ibis 12's create_table(obj=...) emits a malformed
    DROP statement with current sqlglot."""
    if con.name == "duckdb":
        con.con.register("_src", data)
        con.con.execute(f'CREATE OR REPLACE TABLE "{name}" AS SELECT * FROM _src')
        con.con.unregister("_src")
        return
    cols = ", ".join(f'"{f.name}" {sql_type(f.type, con.name)}' for f in data.schema)
    rows = [tuple(r.values()) for r in data.to_pylist()]
    if con.name == "sqlite":
        # timestamps as ISO text, like Ibis's own SQLite fixtures
        rows = [tuple(v.isoformat(sep=" ") if hasattr(v, "isoformat") else v for v in r) for r in rows]
        con.con.execute(f'DROP TABLE IF EXISTS "{name}"')
        con.con.execute(f'CREATE TABLE "{name}" ({cols})')
        con.con.executemany(f'INSERT INTO "{name}" VALUES ({", ".join("?" * len(data.schema))})', rows)
        con.con.commit()
    else:
        with con.con.cursor() as cur:
            cur.execute(f'DROP TABLE IF EXISTS "{name}"')
            cur.execute(f'CREATE TABLE "{name}" ({cols})')
            with cur.copy(f'COPY "{name}" FROM STDIN') as copy:
                for r in rows:
                    copy.write_row(r)
        con.con.commit()


def load(con) -> Fixtures:
    for name in TABLES:
        create(con, name, pq.read_table(DATA / "parquet" / f"{name}.parquet"))
    for name in MEMTABLES:
        create(con, name, memtable(name))
    return Fixtures(con)


def to_table(c: Fixtures, case: cases.Case) -> ir.Table:
    t = c.alltypes
    if case.kind == "tables":
        return case.fn(c)
    if case.kind == "scalar":
        return t.select(tmp=case.fn()).limit(1)
    out = case.fn(t)
    if case.kind == "table":
        return out
    if isinstance(out, ir.Scalar):
        return t.aggregate(tmp=out)
    return t.select("id", tmp=out)


def canon(tbl: pa.Table, ordered: bool) -> list[tuple]:
    def cell(v):
        if isinstance(v, float) and math.isnan(v):
            return ("nan",)
        if isinstance(v, bool) or v is None or isinstance(v, (int, float, str)):
            return v
        if hasattr(v, "as_tuple"):  # Decimal
            return float(v)
        return str(v)

    rows = [tuple(cell(v) for v in row.values()) for row in tbl.to_pylist()]
    if not ordered:
        rows.sort(key=lambda r: tuple((v is None, str(type(v).__name__ in ("int", "float", "bool")), repr(v)) for v in r))
    return rows


def same(a: list[tuple], b: list[tuple], ordered: bool) -> bool:
    def eq(x, y) -> bool:
        if isinstance(x, bool) or isinstance(y, bool):
            # SQLite has no boolean; Ibis returns ints there
            return x is not None and y is not None and int(x) == int(y)
        if isinstance(x, (int, float)) and isinstance(y, (int, float)):
            return x == y or abs(x - y) <= 1e-9 + 1e-6 * max(abs(x), abs(y))
        return x == y

    if len(a) != len(b):
        return False
    if not ordered:
        # sort keys can differ by float noise; compare by numeric-tolerant sort
        key = lambda r: tuple((v is None, round(v, 6) if isinstance(v, float) else (int(v) if isinstance(v, bool) else v)) if not isinstance(v, (str, tuple)) else (False, str(v)) for v in r)  # noqa: E731
        try:
            a, b = sorted(a, key=key), sorted(b, key=key)
        except TypeError:
            pass
    return all(len(r) == len(s) and all(eq(x, y) for x, y in zip(r, s)) for r, s in zip(a, b))


def others() -> dict[str, Fixtures]:
    out = {"sqlite": load(ibis.sqlite.connect())}
    url = os.environ.get("IBIS_TESTING_POSTGRES_URL")
    if url:
        kw = dict(p.split("=", 1) for p in url.split())
        out["postgres"] = load(
            ibis.postgres.connect(
                host=kw.get("host"), port=int(kw.get("port", 5432)), user=kw.get("user"), database="ibis_python"
            )
        )
    return out


def main() -> int:
    flt = sys.argv[1] if len(sys.argv) > 1 else ""
    all_cases = cases.load()
    duck = load(ibis.duckdb.connect())
    other = others()

    index_path = GOLDEN / "index.json"
    index = json.loads(index_path.read_text()) if index_path.exists() else {}
    # drop goldens whose case no longer exists
    for stale in [k for k in index if k != "_meta" and k not in all_cases]:
        f = index.pop(stale).get("file")
        if f:
            (GOLDEN / f).unlink(missing_ok=True)
    failures = 0
    for case_id, case in sorted(all_cases.items()):
        if flt not in case_id:
            continue
        try:
            expr = to_table(duck, case)
            result = expr.to_pyarrow()
        except Exception as e:  # noqa: BLE001 - record and keep going
            print(f"{case_id}: {type(e).__name__}: {e}", file=sys.stderr)
            index[case_id] = {"error": f"{type(e).__name__}: {e}"}
            failures += 1
            continue
        want = canon(result, case.ordered)
        agree = {}
        for name, fx in other.items():
            try:
                got = canon(to_table(fx, case).to_pyarrow(), case.ordered)
                agree[name] = "same" if same(want, got, case.ordered) else "differs"
            except Exception as e:  # noqa: BLE001
                agree[name] = f"error: {type(e).__name__}"
        rel = Path(case.module) / f"{case_id.replace('/', '_')}.parquet"
        (GOLDEN / rel).parent.mkdir(parents=True, exist_ok=True)
        pq.write_table(result, GOLDEN / rel, compression="zstd")
        index[case_id] = {
            "file": str(rel),
            "kind": case.kind,
            "ordered": case.ordered,
            "columns": list(expr.schema().names),
            "dtypes": [str(d.copy(nullable=True)) for d in expr.schema().types],
            "rows": result.num_rows,
            "ibis": agree,
        }
    index["_meta"] = {"ibis": ibis.__version__, "backend": "duckdb"}
    index_path.write_text(json.dumps(dict(sorted(index.items())), indent=1) + "\n")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
