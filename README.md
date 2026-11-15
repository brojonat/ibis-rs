# ibis-rs

A portable dataframe API for Rust, modeled on Python's
[Ibis](https://ibis-project.org): write a query once against a lazy
`Table`/`Expr` API, then compile and run it on any backend. Develop against
SQLite, switch to Postgres or DuckDB by changing the connection. Backends are
plugins: a core plus one crate per engine.

**North star: Ibis parity, measured by Ibis's own backend test suite.** Each
conformance case is a ported Ibis test (same pytest id), and its expected result
comes from running the real Python Ibis expression. See
[docs/design.md](docs/design.md).

Status: pre-alpha, unpublished. Three backends (DuckDB, Postgres, SQLite), 551
ported Ibis test cases and Ibis's 22 TPC-H queries:

| backend | Ibis cases (551) | TPC-H (22) |
|---|---|---|
| DuckDB | 551 | 22 |
| Postgres | 551 | 22 |
| SQLite | 530 (21 expected failures: they need NaN, which SQLite cannot store) | 22 |

Output column names and types match Ibis's on all 551. Ported so far:
`test_generic`, `test_numeric`, `test_string`, `test_aggregation`,
`test_temporal` (first tier), `test_join`, `test_window`, `test_set_ops`,
`test_conditionals`. What's next is in [TODO.md](TODO.md).

## Usage

```rust
use ibis_core::prelude::*;

let con = ibis_duckdb::DuckDb::open_in_memory()?;
con.raw_sql("CREATE TABLE functional_alltypes AS FROM 'ci/ibis-testing-data/parquet/functional_alltypes.parquet'")?;
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

Swap `ibis_duckdb::DuckDb` for `ibis_sqlite::Sqlite` or
`ibis_postgres::Postgres::connect(params)` and the same `expr` runs there. This
example is compiled: `cargo run -p ibis-duckdb --example readme` (after
`mise run setup`).

Column references come in two forms, as in Ibis: `col("x")` is unbound (Ibis's
`_.x`) and resolves against whatever table it is used on; `t.col("x")` is bound
to `t` (Ibis's `t.x`), which is what joins, correlated subqueries and
`EXISTS` need.

## Requirements

- Rust (edition 2024) and [mise](https://mise.jdx.dev/).
- Postgres binaries on `PATH` (`initdb`, `pg_ctl`, `psql`). `mise run pg-start`
  creates a throwaway cluster in `.pg/` (unix socket, port 54330, C locale).
- `uv`, for the Python side (goldens, benchmark, id check). mise installs it.

Run tests through mise. `IBIS_TESTING_POSTGRES_URL` and `IBIS_TESTING_DATA` are
set in `mise.toml`; a bare `cargo test` fails without them.

## Layout

| path | what |
|---|---|
| `crates/ibis-core` | datatypes, schema, IR (`Rel`, `Expr`), `Table`/`Expr` API, `Backend` trait |
| `crates/ibis-sql` | IR → SQL compiler; the `Dialect` trait is the SQL plugin seam |
| `crates/ibis-duckdb` | DuckDB backend (bundled DuckDB, Arrow-native results) |
| `crates/ibis-postgres` | Postgres backend (`postgres` crate, binary protocol, `COPY` for loads) |
| `crates/ibis-sqlite` | SQLite backend (bundled SQLite, Rust polyfills for missing functions) |
| `crates/ibis-conformance` | Rust side of the suite: `src/cases/<module>.rs`, `src/tpch.rs`, the runner, `tpch-bench` |
| `conformance/` | Python side: `cases/test_<module>.py` (the Ibis expressions), `gen_goldens.py`, `check_ids.py`, `memtables.json`, `golden/` (parquet + `index.json`) |
| `bench/` | TPC-H data generation, the Python Ibis baseline, the report script |
| `scripts/` | `pg.sh` (local Postgres), `download-data.sh` (Ibis testing data) |
| `spikes/unparser` | spike: DataFusion's Unparser as a multi-backend compiler ([findings](spikes/unparser/FINDINGS.md)) |

## Commands

`mise tasks` lists everything. The main ones:

| task | does |
|---|---|
| `mise run setup` | start Postgres, download Ibis testing data, generate TPC-H sf=0.01 for all three engines |
| `mise run build` | `cargo build --workspace` |
| `mise run test` | unit tests + conformance + TPC-H on all backends |
| `mise run lint` | `cargo fmt --check` + `cargo clippy -D warnings` |
| `mise run conformance` | run the runner: JSONL to `logs/conformance.jsonl`, matrix to stderr and `logs/conformance.txt` |
| `mise run goldens` | regenerate `conformance/golden/` with Python Ibis |
| `mise run check-ids` | check every case id is a real Ibis pytest id |
| `mise run bench` | TPC-H: Python Ibis vs ibis-rs, report in `logs/bench.md` (about 10 minutes) |
| `mise run dev` | rerun conformance on every file change, tee to `logs/dev.log` |
| `mise run pg-start` / `pg-stop` | the local test Postgres |

The first build compiles DuckDB from source (10+ minutes). On a machine short
of memory, `CARGO_BUILD_JOBS=4`.

### Conformance runner

```
ibis-conformance [--backend duckdb,postgres,sqlite] [ID_SUBSTRING]
```

stdout: one JSON object per (case, backend):
`{"id": "test_string[upper]", "backend": "sqlite", "status": "pass|fail|error|xfail|xpass|no_golden", "detail": ...}`.
Besides the three engines there is a `schema` pseudo-backend: it checks each
case's output column names and inferred types against Ibis's, without running
anything. stderr: a matrix of non-passing cases, per-backend totals, how often
Python Ibis itself agrees with its own DuckDB result on SQLite and Postgres, and
any golden that has no Rust port. Exit status is non-zero if any case fails that
isn't marked `notimpl` for that backend, or a marked case unexpectedly passes.

How a case is checked:

- The golden is Python Ibis 12.0.0 running the expression on DuckDB. Every
  backend must match that one result.
- Rows compare as a multiset unless the case is `ordered`. Floats compare with a
  tolerance (1e-9 absolute + 1e-6 relative).
- `notimpl` mirrors Ibis's `pytest.mark.notimpl`: the case must fail on that
  backend, and passing is reported as `xpass`.

### Benchmark

`mise run bench` runs Ibis's 22 TPC-H queries (taken from Ibis's own test
suite) in Python Ibis and in ibis-rs against identical databases, and checks
row counts agree. At sf=0.01, minimum of 3 runs, totals over the 22 queries:

| backend | compile, Python → Rust | execute, Python → Rust |
|---|---|---|
| DuckDB | 114 ms → 8.4 ms | 330 ms → 170 ms |
| Postgres | 108 ms → 10 ms | 24.7 s → 24.4 s |
| SQLite | 117 ms → 8.7 ms | 118.7 s → 33.0 s |

Execute time is mostly the engine's. Postgres is dominated by Q20 (24 s, no
indexes on the tables); the SQLite gap comes from Q21 and Q4, where the two
libraries emit different SQL.

## Adding things

- **A test case:** add the Ibis expression to `conformance/cases/test_<module>.py`
  under its real pytest id, port it to
  `crates/ibis-conformance/src/cases/<module>.rs` under the same id, then
  `mise run goldens` and `mise run conformance <id>`. `mise run check-ids`
  confirms the id exists in Ibis.
- **An op:** add the IR variant in `ibis-core` with its type inference and API
  method, the default SQL in `ibis-sql`, and dialect overrides where engines
  differ. Port the Ibis tests that cover it.
- **A SQL backend:** a crate with a `Dialect` impl plus a `Backend` impl that
  executes SQL and returns Arrow. Add it to `targets()` in
  `crates/ibis-conformance/src/lib.rs`, to the fixture loaders in
  `conformance/gen_goldens.py` (for the Python Ibis cross-check), and to
  `bench/tpch_data.py`.
