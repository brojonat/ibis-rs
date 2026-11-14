# ibis-rs

A portable dataframe API for Rust, modeled on Python's
[Ibis](https://ibis-project.org): write a query once against a lazy
`Table`/`Expr` API, then compile and run it on any backend (DuckDB, SQLite,
Postgres, DataFusion, Polars, ...). Backends are plugins: a core plus one crate
per engine.

**North star: Ibis parity, measured by Ibis's own backend test suite.** Each
conformance case is a ported Ibis test (same id), and its expected result comes
from running the real Python Ibis expression. See [docs/design.md](docs/design.md).

Status: pre-alpha. Working: core IR + API, SQL compiler, DuckDB and SQLite
backends, conformance runner. **108 ported Ibis test cases** (from
`test_string`, `test_numeric`, `test_aggregation`) pass on both backends. Not
yet: Postgres, DataFusion, Polars backends; most of the Ibis op surface. See
[TODO.md](TODO.md).

## Usage

```rust
use ibis_core::prelude::*;

let con = ibis_duckdb::DuckDb::open_in_memory()?;
let t = con.table("functional_alltypes")?;

let expr = t
    .filter([col("int_col").gt(lit(2))])?
    .group_by(["string_col"])
    .agg([col("double_col").mean().name("avg"), count().name("n")])?
    .order_by([desc("avg")])?
    .limit(10, 0);

println!("{}", con.compile(&expr)?);          // the SQL, like ibis.to_sql()
let batches = con.execute(&expr)?;            // Vec<arrow RecordBatch>
```

Swap `ibis_duckdb::DuckDb` for `ibis_sqlite::Sqlite` and the same `expr` runs
there. This example is compiled: `cargo run -p ibis-duckdb --example readme`
(after `mise run setup`).

## Layout

| path | what |
|---|---|
| `crates/ibis-core` | datatypes, schema, IR (`Rel`, `Expr`), `Table`/`Expr` API, `Backend` trait |
| `crates/ibis-sql` | IR → SQL compiler; `Dialect` trait is the SQL plugin seam |
| `crates/ibis-duckdb` | DuckDB backend (bundled DuckDB, arrow-native results) |
| `crates/ibis-sqlite` | SQLite backend (results converted to the Ibis schema) |
| `crates/ibis-conformance` | runs ported Ibis test cases on every backend |
| `conformance/` | Python side: `cases.py` (Ibis expressions), `gen_goldens.py`, `golden/*.json` |
| `spikes/unparser` | spike: DataFusion's Unparser as a multi-backend compiler ([findings](spikes/unparser/FINDINGS.md)) |

## Commands

`mise tasks` lists everything. The main ones:

| task | does |
|---|---|
| `mise run setup` | download Ibis testing data (parquet) into `ci/ibis-testing-data/` |
| `mise run build` | `cargo build --workspace` |
| `mise run test` | unit tests + conformance on all backends |
| `mise run lint` | `cargo fmt --check` + `cargo clippy` |
| `mise run conformance` | run the conformance runner: JSONL to `logs/conformance.jsonl`, matrix to stderr |
| `mise run goldens` | regenerate `conformance/golden/*.json` with Python Ibis (needs `uv`) |
| `mise run dev` | rerun conformance on every file change, tee to `logs/` |

### Conformance runner

```
ibis-conformance [--backend duckdb,sqlite] [ID_SUBSTRING]
```

stdout: one JSON object per (case, backend):
`{"id": "test_string[upper]", "backend": "sqlite", "status": "pass|fail|error|xfail|xpass|no_golden", "detail": ...}`.
stderr: a pass matrix and per-backend parity counts. Exit status is non-zero
if any case fails that isn't marked `notimpl` for that backend, or a marked case
unexpectedly passes.

## Adding things

- **An op:** add the IR variant in `ibis-core`, type inference, the API method,
  the default SQL in `ibis-sql`, dialect overrides where engines differ, then
  port the matching Ibis test ids to `conformance/cases.py` and
  `crates/ibis-conformance/src/cases.rs` and run `mise run goldens`.
- **A SQL backend:** a crate with a `Dialect` impl plus a `Backend` impl that
  executes SQL and returns Arrow. Add it to the conformance runner.
