# ibis-rs

[![ci](https://github.com/brojonat/ibis-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/brojonat/ibis-rs/actions/workflows/ci.yml)
[Parity with the Ibis test suite](https://brojonat.github.io/ibis-rs/)

A portable dataframe API for Rust, modeled on Python's
[Ibis](https://ibis-project.org): write a query once against a lazy
`Table`/`Expr` API, then compile and run it on any backend. Develop against
SQLite, switch to Postgres, MySQL, DuckDB, ClickHouse, Trino, Spark or Impala by changing the connection. Backends are
plugins: a core plus one crate per engine.

**North star: Ibis parity, measured by Ibis's own backend test suite.** Each
conformance case is a ported Ibis test (same pytest id), and its expected result
comes from running the real Python Ibis expression. The shape to grow into is
Ibis's [backend support matrix](https://ibis-project.org/backends/support/matrix).
The [parity site](https://brojonat.github.io/ibis-rs/) lists every Ibis backend
test, whether it is ported, and how each backend does on it: 561 of 1,899
applicable test ids so far. See [docs/design.md](docs/design.md).

Status: pre-alpha. Nine backends, 551 ported Ibis test cases and Ibis's 22
TPC-H queries:

| backend | Ibis cases (551) | TPC-H (22) |
|---|---|---|
| Polars (default) | 551 | 22 |
| DuckDB | 551 | 22 |
| Postgres | 551 | 22 |
| SQLite | 530 (21 expected failures: they need NaN, which SQLite cannot store) | 22 |
| MySQL | 531 (20 expected failures: they need NaN or infinity, which MySQL does not have) | 22 |
| ClickHouse | 550 (1 expected failure, as in Ibis: `x IN (NULL)` is false, not NULL) | 22 |
| Trino | 551 | 22 |
| Spark SQL | 551 | 22 |
| Impala | 548 (3 expected failures, as in Ibis: no `NTH_VALUE`, and `IN (subquery)` only in `WHERE`) | 22 |

Spark and Impala are in this repository but not in a crates.io release yet.

Output column names and types match Ibis's on all 551. Ported so far:
`test_generic`, `test_numeric`, `test_string`, `test_aggregation`,
`test_temporal` (first tier), `test_join`, `test_window`, `test_set_ops`,
`test_conditionals`. What's next is in [TODO.md](TODO.md).

## Usage

```toml
[dependencies]
ibis-rs = "0.1"                                                   # Polars, in process
ibis-rs = { version = "0.1", features = ["sqlite", "postgres"] }  # more engines: duckdb, sqlite, postgres, mysql, clickhouse, trino (spark, impala: not released yet)
```

```rust
use ibis_rs::prelude::*;

// "polars://", "duckdb://", "sqlite://app.db", "postgres://user@host/db",
// "mysql://user:pass@host/db", "clickhouse://user:pass@host:8123/db",
// "trino://user@host:8080/catalog/schema", "spark://user@host:10000/db", "impala://host:21050/db": same code, different engine
let con = ibis_rs::connect(&std::env::var("DATABASE_URL")?)?;
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

`ibis-rs` is the front door: it re-exports the API and puts each backend behind
a cargo feature. (`duckdb` also has a `duckdb-linked` variant, which links a
prebuilt DuckDB instead of compiling it.) Polars is the default: it runs in process, needs no server
and no C++ toolchain, and holds its tables in memory (`create_table` from Arrow
batches, or `ibis_rs::polars::Polars::register` for a Polars `DataFrame`).
`connect` returns a `Box<dyn Backend>`; the concrete types
(`ibis_rs::polars::Polars`, `ibis_rs::sqlite::Sqlite`, `ibis_rs::duckdb::DuckDb`,
`ibis_rs::postgres::Postgres`, `ibis_rs::mysql::MySql`, `ibis_rs::clickhouse::ClickHouse`,
`ibis_rs::trino::Trino`) add engine-specific methods such as `raw_sql` or
`Polars::lazy`, which hands back the Polars `LazyFrame` for a query. The crates
behind it (`ibis-core`, `ibis-sql`, `ibis-polars`, `ibis-duckdb`, `ibis-sqlite`,
`ibis-postgres`, `ibis-mysql`, `ibis-clickhouse`, `ibis-trino`, `ibis-spark`, `ibis-impala`) can also be used directly; a compiled example of that is
`cargo run -p ibis-duckdb --example readme` (after `mise run setup`).

Column references come in two forms, as in Ibis: `col("x")` is unbound (Ibis's
`_.x`) and resolves against whatever table it is used on; `t.col("x")` is bound
to `t` (Ibis's `t.x`), which is what joins, correlated subqueries and
`EXISTS` need.

## Requirements

- Rust (edition 2024) and [mise](https://mise.jdx.dev/).
- Docker with the compose plugin. Server backends run as containers defined in
  `compose.yaml` (Postgres on localhost:54330, MySQL on 13306, ClickHouse on
  18123, Trino on 18080, the Spark Thrift server on 20000, Impala on 21050),
  holding throwaway test data. Nothing else needs installing per backend. All
  of them together use about 10 GB of memory: Trino 3.4, Spark 2.3, Impala's
  four containers 2, the rest under 1 each. Start only the ones you are
  working on if that is too much.
- `uv`, for the Python side (goldens, benchmark, id check). mise installs it.

Run tests through mise. `IBIS_TESTING_DATA` and the backend URLs
(`IBIS_TESTING_<BACKEND>_URL`, one per server backend) are set in
`mise.toml`; a bare `cargo test` fails without them.

A backend whose container is not running is skipped with a note on stderr, so
`mise run test` works with whatever is up. `mise run setup` starts Postgres;
`mise run up` starts everything; `mise run up clickhouse` starts one. CI sets
`IBIS_TESTING_STRICT=1`, which turns a skipped backend into a failure.

## Layout

| path | what |
|---|---|
| `crates/ibis-rs` | the crate users depend on: re-exports, a feature per backend, `connect(url)` |
| `crates/ibis-core` | datatypes, schema, IR (`Rel`, `Expr`), `Table`/`Expr` API, `Backend` trait |
| `crates/ibis-sql` | IR → SQL compiler; the `Dialect` trait is the SQL plugin seam |
| `crates/ibis-polars` | Polars backend: the IR becomes a `LazyFrame`, no SQL; subqueries are rewritten as joins |
| `crates/ibis-duckdb` | DuckDB backend (bundled DuckDB, Arrow-native results) |
| `crates/ibis-postgres` | Postgres backend (`postgres` crate, binary protocol, `COPY` for loads) |
| `crates/ibis-sqlite` | SQLite backend (bundled SQLite, Rust polyfills for missing functions) |
| `crates/ibis-mysql` | MySQL backend (`mysql` crate, text protocol; no boolean, NaN, `FILTER` or `FULL JOIN`, each rewritten) |
| `crates/ibis-clickhouse` | ClickHouse backend (HTTP interface, tab-separated results, every type `Nullable`) |
| `crates/ibis-trino` | Trino backend (HTTP protocol, JSON pages; identifiers fold to lower case) |
| `crates/ibis-hiveserver2` | HiveServer2 client (Thrift binary protocol written by hand, no dependencies): what Spark and Impala are reached over |
| `crates/ibis-spark` | Spark SQL backend, over the Spark Thrift server (not the cluster master, not Spark Connect) |
| `crates/ibis-impala` | Impala backend (no `FILTER`, `INTERSECT ALL` or `NTH_VALUE`; `OFFSET` needs an `ORDER BY`) |
| `crates/ibis-conformance` | Rust side of the suite: `src/cases/<module>.rs`, `src/tpch.rs`, the runner, `tpch-bench` |
| `conformance/` | Python side: `cases/test_<module>.py` (the Ibis expressions), `gen_goldens.py`, `check_ids.py`, `memtables.json`, `golden/` (parquet + `index.json`), `site.py` and `not_applicable.json` (the parity site) |
| `compose.yaml`, `docker/` | one container per server backend (four for Impala: metastore, statestore, catalog, daemon) |
| `.github/workflows/ci.yml` | lint, test on every backend (DuckDB linked prebuilt), build the parity site and publish it to GitHub Pages from `main` |
| `bench/` | TPC-H data generation, the Python Ibis baseline, the report script |
| `scripts/` | `download-data.sh` (Ibis testing data) |
| `spikes/unparser` | spike: DataFusion's Unparser as a multi-backend compiler ([findings](spikes/unparser/FINDINGS.md)) |

## Commands

`mise tasks` lists everything. The main ones:

| task | does |
|---|---|
| `mise run setup` | start the Postgres container, download Ibis testing data, generate TPC-H sf=0.01 (DuckDB, SQLite, Postgres; MySQL, ClickHouse, Trino, Spark and Impala load it on first use) |
| `mise run up [service...]` / `down` | start backend containers and wait until healthy (all of them if none is named) / stop them and delete their data |
| `mise run build` | `cargo build --workspace` |
| `mise run test` | unit tests + conformance + TPC-H on every backend that is up, under a 12 GB memory cap |
| `mise run lint` | `cargo fmt --check` + `cargo clippy -D warnings` |
| `mise run conformance` | run the runner: JSONL to `logs/conformance.jsonl`, matrix to stderr and `logs/conformance.txt` |
| `mise run site` | run conformance, then build the parity site into `site/` (open `site/index.html`) |
| `mise run goldens` | regenerate `conformance/golden/` with Python Ibis |
| `mise run check-ids` | check every case id is a real Ibis pytest id |
| `mise run bench` | TPC-H: Python Ibis vs ibis-rs, report in `logs/bench.md` (about 10 minutes) |
| `mise run dev` | rerun conformance on every file change, tee to `logs/dev.log` |

The first build compiles DuckDB from source (10+ minutes) and Polars (about 5).
To skip the DuckDB compile, build without `ibis-duckdb`'s `bundled` feature: the
`duckdb` crate then downloads a prebuilt `libduckdb.so` and links that. CI does
this through `IBIS_CARGO_FLAGS` (see `.github/workflows/ci.yml`), which replaces
the `--workspace --all-features` the mise tasks otherwise pass to cargo; a
binary started outside cargo then needs the library's directory
(`target/duckdb-download/<target>/<version>`) in `LD_LIBRARY_PATH`.
Dev builds keep line tables only, not full debug info: with it, `target/` grew
past 180 GB. It is about 9 GB now; `cargo clean` when it creeps up.
On a machine short of memory, `CARGO_BUILD_JOBS=4`. Build with `--workspace`:
`cargo build -p one-crate` picks a different feature set and compiles DuckDB
all over again.

### Conformance runner

```
ibis-conformance [--backend clickhouse,duckdb,impala,mysql,polars,postgres,spark,sqlite,trino] [ID_SUBSTRING]
```

stdout: one JSON object per (case, backend):
`{"id": "test_string[upper]", "backend": "sqlite", "status": "pass|fail|error|xfail|xpass|no_golden|unsupported", "detail": ...}`.
Besides the seven engines there is a `schema` pseudo-backend: it checks each
case's output column names and inferred types against Ibis's, without running
anything. stderr: a matrix of non-passing cases, per-backend totals, how often
Python Ibis itself agrees with its own DuckDB result on SQLite and Postgres, and
any golden that has no Rust port. Exit status is non-zero if any case fails that
isn't marked `notimpl` for that backend, or a marked case unexpectedly passes.
Without `--backend`, a backend that is not running is skipped; one named with
`--backend` must be up.

How a case is checked:

- The golden is Python Ibis 12.0.0 running the expression on DuckDB. Every
  backend must match that one result.
- Rows compare as a multiset unless the case is `ordered`. Floats compare with a
  tolerance (1e-9 absolute + 1e-6 relative).
- `notimpl` mirrors Ibis's `pytest.mark.notimpl`: the case must fail on that
  backend, and passing is reported as `xpass`.
- `unsupported` is for a backend still being built out (listed in `PARTIAL`;
  today only Polars): it said it cannot run the op yet. That does not fail the
  run; a wrong answer or any other error does.
- TPC-H goldens are per backend, from Python Ibis on the same engine and data.
  Polars, MySQL, ClickHouse, Trino, Spark and Impala share DuckDB's: they
  load the same parquet files.
- A backend listed in `EXPERIMENTAL` (none today) is run and reported, but its
  failures do not fail the run and `cargo test` leaves it out. That is how a
  new backend lands before it passes everything.

### Parity site

`mise run site` writes `site/`: an index of Ibis's backend test modules with how
many test ids are ported, and a page per module with one row per Ibis test id,
linked to the Ibis source, our Python case and our Rust case, with the result
and the generated query on each backend. `site/parity.json` has the same data.
Ibis tests that cannot apply to a Rust library (Python UDFs, pandas output,
snapshots of Ibis's SQL text) are listed in `conformance/not_applicable.json`
with the reason, and are not counted as missing. CI publishes the site from
`main` to <https://brojonat.github.io/ibis-rs/>.

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
  executes SQL and returns Arrow (`ibis-clickhouse` and `ibis-trino` are the
  smallest examples; `ibis-mysql` shows an engine far from the defaults, and
  `ibis-spark` one reached over HiveServer2: an engine that speaks it needs
  only a dialect on top of `ibis-hiveserver2`).
  Add a service to `compose.yaml` and `IBIS_TESTING_<BACKEND>_URL` to
  `mise.toml`; add the backend to `ALL_BACKENDS`, `EXPERIMENTAL`, `SERVERS`,
  `connect()` and `tpch_backend()` in `crates/ibis-conformance/src/lib.rs`, a
  feature and URL scheme to `crates/ibis-rs`, and a column to `BACKENDS` in
  `conformance/site.py`. Run `mise run up <backend>` and
  `mise run conformance --backend <backend>`, fix what fails, and take it out
  of `EXPERIMENTAL` when it passes. Optional: the fixture loaders in
  `conformance/gen_goldens.py` (the Python Ibis cross-check) and
  `bench/tpch_data.py`.

## License

The library crates are licensed under either of [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option. The conformance suite
(`conformance/`, `crates/ibis-conformance/`) is Apache-2.0 only: its cases are
adapted from Ibis's Apache-2.0 test suite. Details in [NOTICE](NOTICE).

ibis-rs is an independent project, not affiliated with the Ibis project.
