# /// script
# requires-python = ">=3.11"
# dependencies = ["ibis-framework[duckdb]==12.*", "pyarrow"]
# ///
"""Run conformance/cases.py on Python Ibis + DuckDB and write the goldens.

Writes:
  conformance/golden/<module>/<case id>.parquet   expected result (typed)
  conformance/golden/index.json                  per-case metadata (diffable)

Usage: gen_goldens.py [ID_SUBSTRING]
Env:   IBIS_TESTING_DATA  (default: ci/ibis-testing-data)
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import ibis
import ibis.expr.types as ir
import pyarrow.parquet as pq

sys.path.insert(0, str(Path(__file__).parent))
from cases import CASES, module_of  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
GOLDEN = ROOT / "conformance" / "golden"
DATA = Path(os.environ.get("IBIS_TESTING_DATA", ROOT / "ci" / "ibis-testing-data"))


def to_table(t: ir.Table, case) -> ir.Table:
    out = case.fn(t)
    if case.kind == "table":
        return out
    if isinstance(out, ir.Scalar):
        return t.aggregate(tmp=out)
    return t.select("id", tmp=out)


def safe_name(case_id: str) -> str:
    return case_id.replace("/", "_")


def main() -> int:
    flt = sys.argv[1] if len(sys.argv) > 1 else ""
    con = ibis.duckdb.connect()
    t = con.read_parquet(DATA / "parquet" / "functional_alltypes.parquet", table_name="functional_alltypes")

    index_path = GOLDEN / "index.json"
    index = json.loads(index_path.read_text()) if index_path.exists() else {}
    failures = 0
    for case_id, case in sorted(CASES.items()):
        if flt not in case_id:
            continue
        try:
            expr = to_table(t, case)
            result = expr.to_pyarrow()
        except Exception as e:  # noqa: BLE001 - record and keep going
            print(f"{case_id}: {type(e).__name__}: {e}", file=sys.stderr)
            index[case_id] = {"error": f"{type(e).__name__}: {e}"}
            failures += 1
            continue
        rel = Path(module_of(case_id)) / f"{safe_name(case_id)}.parquet"
        (GOLDEN / rel).parent.mkdir(parents=True, exist_ok=True)
        pq.write_table(result, GOLDEN / rel, compression="zstd")
        index[case_id] = {
            "file": str(rel),
            "kind": case.kind,
            "ordered": case.ordered,
            "columns": list(expr.schema().names),
            "dtypes": [str(d) for d in expr.schema().types],
            "rows": result.num_rows,
        }
    index["_meta"] = {"ibis": ibis.__version__, "backend": "duckdb"}
    index_path.write_text(json.dumps(dict(sorted(index.items())), indent=1) + "\n")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
