"""Join Python Ibis and ibis-rs TPC-H timings into one report.

Usage: compare.py PYTHON.jsonl RUST.jsonl [--json]

Reads the JSONL written by bench/tpch_ibis.py and tpch-bench. Prints a
markdown table per backend (or, with --json, one joined JSON object per
(backend, query)). Ratios are python / rust: above 1 means ibis-rs is faster.
"""

from __future__ import annotations

import json
import sys


def load(path: str) -> dict[tuple[str, str], dict]:
    with open(path) as f:
        rows = [json.loads(line) for line in f if line.strip()]
    return {(r["backend"], r["query"]): r for r in rows}


def ms(x: float | None) -> str:
    return "-" if x is None else f"{x * 1000:.2f}"


def ratio(a: float | None, b: float | None) -> str:
    return "-" if not a or not b else f"{a / b:.1f}x"


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if len(args) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    py, rs = load(args[0]), load(args[1])
    keys = sorted(set(py) | set(rs))
    if "--json" in sys.argv:
        for k in keys:
            print(json.dumps({"backend": k[0], "query": k[1], "python": py.get(k), "rust": rs.get(k)}))
        return 0
    for backend in sorted({k[0] for k in keys}):
        print(f"\n### {backend}\n")
        print("| query | rows py/rs | compile ms py | compile ms rs | compile ratio | execute ms py | execute ms rs | execute ratio |")
        print("|---|---|---|---|---|---|---|---|")
        tot = {"pc": 0.0, "rc": 0.0, "pe": 0.0, "re": 0.0}
        for k in [k for k in keys if k[0] == backend]:
            p, r = py.get(k, {}), rs.get(k, {})
            if p.get("error") or r.get("error"):
                print(f"| {k[1]} | error | | | | | | |")
                continue
            pc, rc, pe, re_ = p.get("compile_s"), r.get("compile_s"), p.get("execute_s"), r.get("execute_s")
            for n, v in zip(tot, (pc, rc, pe, re_)):
                tot[n] += v or 0
            print(
                f"| {k[1]} | {p.get('rows')}/{r.get('rows')} | {ms(pc)} | {ms(rc)} | {ratio(pc, rc)} "
                f"| {ms(pe)} | {ms(re_)} | {ratio(pe, re_)} |"
            )
        print(
            f"| **total** | | {ms(tot['pc'])} | {ms(tot['rc'])} | {ratio(tot['pc'], tot['rc'])} "
            f"| {ms(tot['pe'])} | {ms(tot['re'])} | {ratio(tot['pe'], tot['re'])} |"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
