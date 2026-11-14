# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "ibis-framework[duckdb,sqlite,postgres]==12.*",
#   "pyarrow",
#   "pytest",
#   "python-dateutil",
# ]
# ///
"""Run Ibis's own TPC-H queries on Python Ibis and time them.

The query expressions are imported from Ibis's test suite
(ibis/backends/tests/tpc/h/test_queries.py), so they are exactly the
expressions Ibis tests itself with.

stdout: one JSON object per (backend, query):
  {"impl": "python-ibis", "backend": "duckdb", "query": "h01", "sf": "0.01",
   "rows": 4, "build_s": ..., "compile_s": ..., "execute_s": ..., "error": null}
  Timings are the minimum over --repeat runs.

Usage: tpch_ibis.py [--sf 0.01] [--backend duckdb,sqlite,postgres] [--repeat 3]
                    [--goldens DIR] [QUERY_SUBSTRING]
  --goldens DIR   also write each result to DIR/<query>.parquet (from the first backend)
Env: IBIS_TESTING_POSTGRES_URL  libpq keyword string, e.g. "host=/x/.pg port=54330 user=postgres"
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path

import ibis
import pyarrow as pa
import pyarrow.parquet as pq

ROOT = Path(__file__).resolve().parent.parent
TABLES = ["customer", "lineitem", "nation", "orders", "part", "partsupp", "region", "supplier"]


def tag(sf: str) -> str:
    return "sf" + sf.replace(".", "_")


def unwrap(f):
    """The undecorated query function behind Ibis's @tpc_test wrapper."""
    if hasattr(f, "__wrapped__"):
        return f.__wrapped__
    # `@tpc_test` used without arguments (test_10): the original function is
    # captured in the decorator's closure.
    for cell in f.__closure__ or ():
        if callable(cell.cell_contents) and getattr(cell.cell_contents, "__name__", "").startswith("test_"):
            return cell.cell_contents
    raise ValueError(f"can't unwrap {f.__name__}")


def queries():
    from ibis.backends.tests.tpc.h import test_queries as tq

    for n in range(1, 23):
        yield f"h{n:02d}", unwrap(getattr(tq, f"test_{n:02d}"))


def connect(backend: str, sf: str):
    d = ROOT / "ci" / "tpch" / tag(sf)
    if backend == "duckdb":
        return ibis.duckdb.connect(str(d / "tpch.duckdb"), read_only=True)
    if backend == "sqlite":
        return ibis.sqlite.connect(str(d / "tpch.sqlite"))
    if backend == "postgres":
        url = os.environ["IBIS_TESTING_POSTGRES_URL"]
        kw = dict(p.split("=", 1) for p in url.split())
        return ibis.postgres.connect(
            host=kw.get("host"), port=int(kw.get("port", 5432)), user=kw.get("user"), database=f"tpch_{tag(sf)}"
        )
    raise SystemExit(f"unknown backend {backend!r}")


def timed(f, repeat: int):
    best, out = float("inf"), None
    for _ in range(repeat):
        t0 = time.perf_counter()
        out = f()
        best = min(best, time.perf_counter() - t0)
    return best, out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--sf", default="0.01")
    ap.add_argument("--backend", default="duckdb,sqlite,postgres")
    ap.add_argument("--repeat", type=int, default=3)
    ap.add_argument("--goldens", type=Path)
    ap.add_argument("filter", nargs="?", default="")
    args = ap.parse_args()

    failed = False
    for i, backend in enumerate(args.backend.split(",")):
        con = connect(backend, args.sf)
        tables = {t: con.table(t) for t in TABLES}
        for name, fn in queries():
            if args.filter not in name:
                continue
            rec = {"impl": "python-ibis", "backend": backend, "query": name, "sf": args.sf}
            try:
                params = fn.__code__.co_varnames[: fn.__code__.co_argcount]
                build_s, expr = timed(lambda: fn(**{p: tables[p] for p in params}), args.repeat)
                compile_s, _ = timed(lambda: con.compile(expr), args.repeat)
                # to_pandas, as Ibis's own TPC-H tests do: to_pyarrow fails on
                # several queries ("Rescaling Decimal value would cause data loss").
                execute_s, result = timed(lambda: con.to_pandas(expr), args.repeat)
                rec |= {"rows": len(result), "build_s": build_s, "compile_s": compile_s, "execute_s": execute_s}
                if args.goldens and i == 0:
                    args.goldens.mkdir(parents=True, exist_ok=True)
                    pq.write_table(pa.Table.from_pandas(result, preserve_index=False), args.goldens / f"{name}.parquet")
            except Exception as e:  # report and keep going: one query shouldn't sink the run
                failed = True
                rec["error"] = f"{type(e).__name__}: {e}"[:500]
                print(f"{backend} {name}: {rec['error']}", file=sys.stderr)
            print(json.dumps(rec), flush=True)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
