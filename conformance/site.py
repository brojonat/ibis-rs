# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "ibis-framework[duckdb,sqlite,postgres]==12.*", "pyarrow", "pandas", "numpy", "pytest", "python-dateutil",
#   "hypothesis", "pytest-mock", "pytest-xdist", "filelock", "pytest-timeout", "pytest-snapshot", "pytest-randomly",
#   "requests", "rich", "packaging", "tomli",
# ]
# ///
"""Build the parity site: how much of Ibis's backend test suite ibis-rs covers.

Reads
  Ibis's own test ids         pytest --collect-only on the installed Ibis
  the ported cases            conformance/cases (Python) and crates/ibis-conformance/src (Rust)
  conformance/not_applicable.json   Ibis tests that cannot apply to a Rust port, with the reason
  the last conformance run    logs/conformance.jsonl (`mise run conformance`)
Writes
  OUT/index.html, OUT/<module>.html, OUT/style.css
  OUT/parity.json             the same data, for scripts

Usage: site.py [--results logs/conformance.jsonl] [--out site]
"""

from __future__ import annotations

import argparse
import ast
import inspect
import json
import re
import subprocess
import sys
import tomllib
from html import escape
from importlib.metadata import entry_points
from pathlib import Path

import ibis

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))
import cases  # noqa: E402

IBIS_ROOT = Path(ibis.__file__).resolve().parent.parent
IBIS_TESTS = "ibis/backends/tests"
IBIS_URL = f"https://github.com/ibis-project/ibis/blob/{ibis.__version__}"
TPCH = "tpc/h/test_queries"
# column order; "schema" is the engine-independent check of output names and types
BACKENDS = ["polars", "duckdb", "postgres", "sqlite", "mysql", "clickhouse", "trino", "spark", "impala", "schema"]
PROBLEMS = {"fail", "error", "xpass", "no_golden"}


def collect() -> dict[str, dict[str, list[str]]]:
    """Ibis's test ids as module -> test function -> ids, for one backend.

    Most tests are parametrized over every backend; the DuckDB variant stands
    for all of them, with the backend taken out of the id.
    """
    p = subprocess.run(
        [sys.executable, "-m", "pytest", "--collect-only", "-q", "-p", "no:randomly", f"{IBIS_ROOT}/{IBIS_TESTS}"],
        capture_output=True, text=True, cwd=IBIS_ROOT,
    )  # fmt: skip
    backends = {e.name for e in entry_points(group="ibis.backends")}
    out: dict[str, dict[str, list[str]]] = {}
    for line in p.stdout.splitlines():
        if "::" not in line or not line.startswith(IBIS_TESTS):
            continue
        path, rest = line.split("::", 1)
        module = path.removeprefix(IBIS_TESTS + "/").removesuffix(".py")
        name = re.match(r"(?:\w+::)*\w+", rest).group(0)
        params = rest[len(name) + 1 : -1].split("-") if len(rest) > len(name) else []
        # the backend is the first parameter, or now and then the last
        at = next((i for i in (0, -1) if params and params[i] in backends), None)
        if at is not None and params.pop(at) != "duckdb":
            continue
        test_id = name + (f"[{'-'.join(params)}]" if params else "")
        ids = out.setdefault(module, {}).setdefault(name, [])
        if test_id not in ids:
            ids.append(test_id)
    if not out:
        raise SystemExit(f"pytest collected nothing:\n{p.stdout[-800:]}{p.stderr[-800:]}")
    return out


def test_lines(module: str) -> dict[str, int]:
    """Line of each test function in an Ibis test module."""
    tree = ast.parse((IBIS_ROOT / IBIS_TESTS / f"{module}.py").read_text())
    lines: dict[str, int] = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef) and node.name.startswith("test"):
            lines.setdefault(node.name, node.lineno)
    return lines


def rust_line(lines: list[str], case_id: str) -> int | None:
    """Best guess at where a case is defined in its Rust file.

    Ids are often built (`format!("test_x[{id}]")`), so look for the literal
    id, then for the parameter after the first mention of the test, then for
    the test.
    """

    def find(pattern: str, start: int = 0) -> int | None:
        rx = re.compile(pattern)
        return next((i for i in range(start, len(lines)) if rx.search(lines[i])), None)

    base = case_id.split("#")[0]
    name, _, params = base.partition("[")
    for literal in (case_id, base):
        if (i := find(re.escape(f'"{literal}"'))) is not None:
            return i + 1
    at = find(rf"\b{re.escape(name)}\b")
    if params:
        quoted = re.escape(f'"{params[:-1]}"')
        i = find(quoted, at or 0)
        if i is None:
            i = find(quoted)
        if i is not None:
            return i + 1
    return None if at is None else at + 1


def ported(repo: str) -> dict[str, dict[str, list[dict]]]:
    """Our cases as module -> Ibis id -> [case], with links to both sides."""
    out: dict[str, dict[str, list[dict]]] = {}
    rust_files: dict[str, list[str]] = {}
    for case_id, case in cases.load().items():
        py = None
        code = getattr(inspect.unwrap(case.fn), "__code__", None)
        if code is not None:
            py = f"{repo}/blob/main/{Path(code.co_filename).resolve().relative_to(ROOT)}#L{code.co_firstlineno}"
        rel = f"crates/ibis-conformance/src/cases/{case.module.removeprefix('test_')}.rs"
        if rel not in rust_files:
            rust_files[rel] = (ROOT / rel).read_text().splitlines()
        line = rust_line(rust_files[rel], case_id)
        rs = f"{repo}/blob/main/{rel}" + (f"#L{line}" if line else "")
        out.setdefault(case.module, {}).setdefault(case_id.split("#")[0], []).append(
            {"id": case_id, "python": py, "rust": rs, "results": {}}
        )
    return out


def tpch_cases(repo: str, ids: list[str]) -> dict[str, list[dict]]:
    rel = "crates/ibis-conformance/src/tpch.rs"
    lines = (ROOT / rel).read_text().splitlines()
    out = {}
    for case_id in ids:
        n = case_id.rsplit("_", 1)[1]
        line = next((i + 1 for i, text in enumerate(lines) if f"fn h{n}(" in text), None)
        rs = f"{repo}/blob/main/{rel}" + (f"#L{line}" if line else "")
        out[case_id.removeprefix("tpch::")] = [{"id": case_id, "python": None, "rust": rs, "results": {}}]
    return out


def build(results_path: Path) -> dict:
    repo = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["repository"]
    not_applicable: dict[str, str] = json.loads((HERE / "not_applicable.json").read_text())

    results: dict[str, dict[str, dict]] = {}
    for line in results_path.read_text().splitlines():
        o = json.loads(line)
        results.setdefault(o["id"], {})[o["backend"]] = {"status": o["status"], "detail": o["detail"], "query": o["sql"]}

    mine = ported(repo)
    mine[TPCH] = tpch_cases(repo, sorted(i for i in results if i.startswith("tpch::")))
    for by_id in mine.values():
        for group in by_id.values():
            for case in group:
                case["results"] = results.get(case["id"], {})
    backends = [b for b in BACKENDS if any(b in r for r in results.values())]

    modules = []
    for module, tests in sorted(collect().items()):
        lines = test_lines(module)
        known = mine.pop(module, {})
        out_tests = []
        for name, ids in tests.items():
            reason = not_applicable.get(f"{module}::{name}") or not_applicable.get(module)
            rows = []
            for test_id in ids:
                group = known.pop(test_id, [])
                state = "ported" if group else "na" if reason else "missing"
                rows.append({"id": test_id, "state": state, "cases": group})
            line = lines.get(name.split("::")[-1])
            url = f"{IBIS_URL}/{IBIS_TESTS}/{module}.py" + (f"#L{line}" if line else "")
            out_tests.append({"name": name, "url": url, "not_applicable": reason, "ids": rows})
        for stray in known:
            print(f"site: {module}: {stray} is not an Ibis test id (mise run check-ids)", file=sys.stderr)
        modules.append({"module": module, "url": f"{IBIS_URL}/{IBIS_TESTS}/{module}.py", "tests": out_tests})
    for module in mine:
        print(f"site: no Ibis test module {module}", file=sys.stderr)

    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, cwd=ROOT)
    return {
        "ibis_version": ibis.__version__,
        "repository": repo,
        "commit": commit.stdout.strip(),
        "backends": backends,
        "modules": modules,
    }


def counts(module: dict, backends: list[str]) -> dict:
    rows = [row for test in module["tests"] for row in test["ids"]]
    group = [case for row in rows for case in row["cases"]]
    return {
        "ids": len(rows),
        "na": sum(row["state"] == "na" for row in rows),
        "ported": sum(row["state"] == "ported" for row in rows),
        "cases": len(group),
        "pass": {b: sum(c["results"].get(b, {}).get("status") == "pass" for c in group) for b in backends},
        "ran": {b: sum(b in c["results"] for c in group) for b in backends},
    }


# ---------------------------------------------------------------- HTML

STYLE = """
:root {
  --bg: #fbfbfa; --fg: #1c1d1f; --muted: #62676e; --line: #dfe1e4; --panel: #f1f2f3; --link: #1558b0;
  --ok-bg: #d9f2e1; --ok-fg: #0d5a2b; --warn-bg: #fdeec2; --warn-fg: #6b4a00; --bad-bg: #fbd9d6; --bad-fg: #8c1d13;
  --off-bg: #e9eaec; --off-fg: #555a61; --bar: #2f8f52;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #15171a; --fg: #e6e7e9; --muted: #9aa0a8; --line: #2e3238; --panel: #1e2125; --link: #7fb2f5;
    --ok-bg: #153d25; --ok-fg: #9be0b3; --warn-bg: #46360a; --warn-fg: #f3d589; --bad-bg: #4c1b17; --bad-fg: #f5aaa3;
    --off-bg: #2a2e33; --off-fg: #a3a9b1; --bar: #4fb873;
  }
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg); font: 15px/1.5 system-ui, sans-serif; }
main { max-width: 1100px; margin: 0 auto; padding: 24px 16px 64px; }
h1 { font-size: 1.6rem; line-height: 1.25; margin: 0 0 8px; }
h1 code { font-size: inherit; }
h2 { font-size: 1.15rem; margin: 32px 0 8px; }
p { max-width: 75ch; }
a { color: var(--link); }
code, pre { font: 13px/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
.muted { color: var(--muted); }
.tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(140px, 1fr)); gap: 12px; margin: 20px 0; }
.tile { background: var(--panel); border: 1px solid var(--line); border-radius: 8px; padding: 12px 14px; }
.tile b { display: block; font-size: 1.5rem; font-variant-numeric: tabular-nums; }
.tile span { color: var(--muted); font-size: 0.85rem; }
.scroll { overflow-x: auto; }
table { border-collapse: collapse; width: 100%; font-variant-numeric: tabular-nums; }
th, td { text-align: left; padding: 5px 10px 5px 0; border-bottom: 1px solid var(--line); vertical-align: top; }
th { font-size: 0.8rem; color: var(--muted); font-weight: 600; white-space: nowrap; }
td.n, th.n { text-align: right; white-space: nowrap; }
.bar { display: inline-block; width: 110px; height: 8px; background: var(--off-bg); border-radius: 4px; overflow: hidden; vertical-align: middle; margin-right: 8px; }
.bar i { display: block; height: 100%; background: var(--bar); }
.chip { display: inline-block; padding: 0 7px; border-radius: 10px; font-size: 0.78rem; white-space: nowrap; }
.pass { background: var(--ok-bg); color: var(--ok-fg); }
.xfail, .unsupported { background: var(--warn-bg); color: var(--warn-fg); }
.fail, .error, .xpass, .no_golden { background: var(--bad-bg); color: var(--bad-fg); }
.missing, .na, .none { background: var(--off-bg); color: var(--off-fg); }
details.test { border: 1px solid var(--line); border-radius: 8px; margin: 8px 0; padding: 0 12px; background: var(--panel); }
details.test > summary { cursor: pointer; padding: 8px 0; display: flex; flex-wrap: wrap; gap: 4px 12px; align-items: baseline; }
details.test > summary code { font-weight: 600; }
details.test[open] { padding-bottom: 8px; }
details.q summary { cursor: pointer; color: var(--muted); font-size: 0.8rem; }
pre { background: var(--bg); border: 1px solid var(--line); border-radius: 6px; padding: 8px; overflow-x: auto; margin: 4px 0 8px; white-space: pre-wrap; }
td.id code { overflow-wrap: anywhere; }
footer { margin-top: 40px; color: var(--muted); font-size: 0.85rem; }
"""

LABEL = {"schema": "types"}


def page(title: str, body: str) -> str:
    return (
        '<!doctype html>\n<html lang="en"><head><meta charset="utf-8">'
        '<meta name="viewport" content="width=device-width, initial-scale=1">'
        f'<title>{escape(title)}</title><link rel="stylesheet" href="style.css"></head>\n'
        f"<body><main>\n{body}\n</main></body></html>\n"
    )


def slug(module: str) -> str:
    return module.replace("/", "-") + ".html"


def percent(part: int, whole: int) -> str:
    return f"{100 * part / whole:.0f}%" if whole else "–"


def bar(part: int, whole: int) -> str:
    width = 100 * part / whole if whole else 0
    return f'<span class="bar"><i style="width:{width:.1f}%"></i></span>'


def footer(data: dict) -> str:
    repo = escape(data["repository"])
    return (
        f'<footer>Generated by <a href="{repo}/blob/main/conformance/site.py">conformance/site.py</a> '
        f'from <a href="{repo}">ibis-rs</a> at <code>{escape(data["commit"])}</code>, against '
        f'<a href="https://github.com/ibis-project/ibis">Ibis</a> {escape(data["ibis_version"])}. '
        'Data: <a href="parity.json">parity.json</a>. '
        "ibis-rs is an independent project, not affiliated with the Ibis project.</footer>"
    )


def index(data: dict) -> str:
    backends = data["backends"]
    per = {m["module"]: counts(m, backends) for m in data["modules"]}
    total = {k: sum(c[k] for c in per.values()) for k in ("ids", "na", "ported", "cases")}
    applicable = total["ids"] - total["na"]
    tiles = [
        (f"{total['ported']}", f"of {applicable} applicable Ibis test ids ported ({percent(total['ported'], applicable)})"),
        (f"{total['cases']}", "cases checked against Python Ibis's results"),
    ]
    for b in backends:
        passed, ran = sum(c["pass"][b] for c in per.values()), sum(c["ran"][b] for c in per.values())
        tiles.append((f"{passed}/{ran}", f"pass on {LABEL.get(b, b)}"))
    rows = []
    for m in data["modules"]:
        c = per[m["module"]]
        app = c["ids"] - c["na"]
        cells = "".join(
            f'<td class="n">{c["pass"][b]}/{c["ran"][b]}</td>' if c["ran"][b] else '<td class="n muted">–</td>'
            for b in backends
        )
        coverage = (
            f"{bar(c['ported'], app)}{c['ported']}/{app}" if app else '<span class="muted">not applicable</span>'
        )
        rows.append(
            f'<tr><td><a href="{slug(m["module"])}"><code>{escape(m["module"])}</code></a></td>'
            f'<td class="n">{c["ids"]}</td><td class="n">{c["na"] or ""}</td><td>{coverage}</td>{cells}</tr>'
        )
    heads = "".join(f'<th class="n">{escape(LABEL.get(b, b))}</th>' for b in backends)
    body = f"""<h1>ibis-rs parity with the Ibis test suite</h1>
<p><a href="{escape(data["repository"])}">ibis-rs</a> is a Rust port of <a href="https://ibis-project.org">Ibis</a>.
Its target is Ibis's own backend test suite: each case here is an Ibis test, ported under the same pytest id,
and its expected result is what Python Ibis {escape(data["ibis_version"])} returns for the same expression.
This page shows how much of that suite is ported and how each backend does on it.</p>
<div class="tiles">{"".join(f'<div class="tile"><b>{escape(n)}</b><span>{escape(t)}</span></div>' for n, t in tiles)}</div>
<h2>By Ibis test module</h2>
<div class="scroll"><table>
<tr><th>module</th><th class="n">Ibis ids</th><th class="n">n/a</th><th>ported</th>{heads}</tr>
{"".join(rows)}
</table></div>
<h2>Reading this</h2>
<p><b>Ibis ids</b> counts the pytest ids of a module for one backend (Ibis runs each test once per backend).
<b>n/a</b> are tests that cannot apply to a Rust library, such as Python UDFs or pandas output; each module page
gives the reason. <b>Ported</b> is out of the rest. The backend columns count passing cases out of cases run;
one Ibis id can be several cases when the test makes several assertions. <b>types</b> checks the output column
names and types that ibis-rs infers against Ibis's, without running a query.</p>
<p>A case that is expected to fail on a backend (<span class="chip xfail">xfail</span>) mirrors a
<code>notimpl</code> mark in Ibis's own suite, or a limit of the engine: SQLite cannot store NaN.</p>
{footer(data)}"""
    return page("ibis-rs parity with Ibis", body)


def chip(status: str | None) -> str:
    if status is None:
        return '<span class="chip none">–</span>'
    return f'<span class="chip {escape(status)}">{escape(status)}</span>'


def worst(statuses: list[str | None]) -> str | None:
    present = [s for s in statuses if s is not None]
    if not present:
        return None
    for s in present:
        if s in PROBLEMS:
            return s
    return next((s for s in present if s != "pass"), "pass")


def module_page(data: dict, m: dict) -> str:
    backends = data["backends"]
    c = counts(m, backends)
    app = c["ids"] - c["na"]
    heads = "".join(f"<th>{escape(LABEL.get(b, b))}</th>" for b in backends)
    blocks = []
    for test in m["tests"]:
        done = sum(row["state"] == "ported" for row in test["ids"])
        rows = []
        for row in test["ids"]:
            group = row["cases"]
            if not group:
                state = "not applicable" if row["state"] == "na" else "not ported"
                rows.append(
                    f'<tr><td class="id"><code>{escape(row["id"])}</code></td>'
                    f'<td><span class="chip {row["state"]}">{state}</span></td>'
                    f'<td colspan="{len(backends)}"></td></tr>'
                )
                continue
            links = []
            for case in group:
                tag = f" {escape(case['id'].partition('#')[2])}" if len(group) > 1 else ""
                if case["python"]:
                    links.append(f'<a href="{escape(case["python"])}">python{tag}</a>')
                links.append(f'<a href="{escape(case["rust"])}">rust{tag}</a>')
            cells = "".join(
                f"<td>{chip(worst([case['results'].get(b, {}).get('status') for case in group]))}</td>"
                for b in backends
            )
            queries = []
            for case in group:
                for b in backends:
                    r = case["results"].get(b, {})
                    if r.get("query") or r.get("detail"):
                        note = f"{escape(str(r['detail']))}\n\n" if r.get("detail") else ""
                        queries.append(
                            f'<div class="muted">{escape(b)}{" · " + escape(case["id"]) if len(group) > 1 else ""}</div>'
                            f"<pre>{note}{escape(r.get('query') or '')}</pre>"
                        )
            extra = f'<details class="q"><summary>queries</summary>{"".join(queries)}</details>' if queries else ""
            rows.append(
                f'<tr><td class="id"><code>{escape(row["id"])}</code>{extra}</td>'
                f"<td>{' · '.join(links)}</td>{cells}</tr>"
            )
        if test["not_applicable"] and not done:
            status = '<span class="chip na">not applicable</span>'
        else:
            status = f'<span class="muted">{done}/{len(test["ids"])} ported</span>'
        reason = f"<p>{escape(test['not_applicable'])}</p>" if test["not_applicable"] else ""
        blocks.append(
            f'<details class="test"{" open" if done else ""}><summary><code>{escape(test["name"])}</code>{status}'
            f'<a href="{escape(test["url"])}">Ibis source</a></summary>{reason}'
            f'<div class="scroll"><table><tr><th>Ibis test id</th><th>port</th>{heads}</tr>{"".join(rows)}</table></div>'
            "</details>"
        )
    summary = (
        f"{c['ported']} of {app} applicable ids ported ({percent(c['ported'], app)})"
        if app
        else "No test in this module applies to a Rust port"
    )
    body = f"""<p><a href="index.html">← all modules</a></p>
<h1><code>{escape(m["module"])}</code></h1>
<p>{summary}; {c["ids"]} ids in <a href="{escape(m["url"])}">the Ibis module</a>{f", {c['na']} not applicable" if c["na"] and app else ""}.
Tests with ported cases are open; the rest are collapsed.</p>
{"".join(blocks)}
{footer(data)}"""
    return page(f"{m['module']} · ibis-rs parity", body)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--results", type=Path, default=ROOT / "logs" / "conformance.jsonl")
    ap.add_argument("--out", type=Path, default=ROOT / "site")
    args = ap.parse_args()
    if not args.results.exists():
        print(f"site: {args.results} missing; run `mise run conformance` first", file=sys.stderr)
        return 1
    data = build(args.results)
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "style.css").write_text(STYLE.strip() + "\n")
    (args.out / "parity.json").write_text(json.dumps(data, indent=1) + "\n")
    (args.out / "index.html").write_text(index(data))
    for m in data["modules"]:
        (args.out / slug(m["module"])).write_text(module_page(data, m))
    return 0


if __name__ == "__main__":
    sys.exit(main())
