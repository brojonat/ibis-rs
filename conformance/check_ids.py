# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "ibis-framework[duckdb,sqlite,postgres]==12.*", "pyarrow", "pandas", "numpy", "pytest", "python-dateutil",
#   "hypothesis", "pytest-mock", "pytest-xdist", "filelock", "pytest-timeout", "pytest-snapshot", "pytest-randomly",
#   "requests", "rich", "packaging", "tomli",
# ]
# ///
"""Check that every conformance case id is a real Ibis pytest id.

Collects Ibis's own backend tests with pytest and compares: case
`test_x[params]` (ignoring a trailing `#n`, our marker for the nth assertion
of one test) must exist in Ibis as `test_x[duckdb-params]` or `test_x[params]`
in the module the case is registered under.

stdout: one line per unknown id. Exit 1 if there are any.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import ibis

sys.path.insert(0, str(Path(__file__).parent))
import cases  # noqa: E402


def collect(module: str) -> set[str]:
    root = os.path.dirname(os.path.dirname(ibis.__file__))
    p = subprocess.run(
        [sys.executable, "-m", "pytest", "--collect-only", "-q", "-p", "no:randomly",
         f"{root}/ibis/backends/tests/{module}.py"],
        capture_output=True, text=True, cwd=root,
    )  # fmt: skip
    ids = {line.split("::", 1)[1] for line in p.stdout.splitlines() if "::" in line}
    if not ids:
        raise SystemExit(f"pytest collected nothing for {module}:\n{p.stdout[-800:]}{p.stderr[-800:]}")
    # ids of every backend are collected; tests that take no backend have none in their id
    return {i.replace("[duckdb-", "[").replace("[duckdb]", "") for i in ids}


def main() -> int:
    by_module: dict[str, list[str]] = {}
    for case_id, case in cases.load().items():
        by_module.setdefault(case.module, []).append(case_id)
    bad = 0
    for module, ids in sorted(by_module.items()):
        real = collect(module)
        for case_id in ids:
            if case_id.split("#", 1)[0] not in real:
                print(f"{module}: {case_id}")
                bad += 1
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
