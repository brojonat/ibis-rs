# Changelog

All notable changes to this project are documented here. Format: [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Added
- `ibis-mysql`: MySQL backend (`mysql` crate, text protocol). MySQL has no boolean type, NaN or infinity, `FILTER (WHERE ...)`, `NULLS FIRST`/`LAST` or `FULL OUTER JOIN`, and its `CAST` has its own type names; each is rewritten. Passes 531 of the 551 Ibis cases (the other 20 need NaN or infinity) and the 22 TPC-H queries. Not on crates.io yet.
- `ibis-trino`: Trino backend over the HTTP protocol. Floats are written with an exponent (a bare `1.5` is a decimal there), and `create_table` sends rows as one JSON array per column. Passes all 551 Ibis cases and the 22 TPC-H queries. Not on crates.io yet.
- `ibis-rs`: `mysql` and `trino` features; `mysql://`, `trino://` and `trinos://` URLs in `connect`.
- `ibis-sql`: `Dialect::cast_type_name` (type names inside `CAST`), `Dialect::sort_key` (how an `ORDER BY` key and its NULL placement are spelled), `Dialect::supports_agg_filter` (without it the condition moves into the argument as a `CASE`), `Dialect::supports_full_outer_join` (without it the join becomes a left join plus the unmatched right rows) and `strftime_to_date_format`.
- `compose.yaml`: MySQL 8.4 and Trino 484 services.

### Changed
- `ibis-sql`: a literal `nth` index compiles to a literal (`NTH_VALUE(x, 4)`, not `NTH_VALUE(x, 3 + 1)`).
- Dev builds keep line tables only (`[profile.dev] debug = "line-tables-only"`, none for dependencies): `target/` had grown past 180 GB.

## [ibis-sql 0.1.1, ibis-clickhouse 0.1.0, ibis-rs 0.1.2] - 2026-11-15

### Added
- `ibis-clickhouse`: ClickHouse backend over the HTTP interface. Every type is `Nullable`, each request carries the settings that give standard SQL semantics (NULLs from outer joins, NULL for aggregates of no rows, `UNION` meaning `UNION DISTINCT`), and floats round half away from zero. Passes 550 of the 551 Ibis cases (the other is marked `notyet` for ClickHouse in Ibis itself) and the 22 TPC-H queries.
- `ibis-rs`: `clickhouse` feature and `clickhouse://` / `clickhouses://` URLs in `connect`.
- `ibis-sql`: `Dialect::analytic`, a hook for how a dialect spells ranking and offset window functions.
- `compose.yaml`: one container per server backend (Postgres, ClickHouse). `mise run up [service...]` and `mise run down`.
- Parity site: `conformance/site.py` and `mise run site` build a static site showing every Ibis backend test id, whether it is ported (with links to the Ibis source and both sides of our port), and the result and query on each backend. `conformance/not_applicable.json` lists the Ibis tests that cannot apply to a Rust port.
- CI (`.github/workflows/ci.yml`): lint, test on every backend, build the parity site and publish it to GitHub Pages from `main`.
- Conformance: a backend whose server is down is skipped with a note (`IBIS_TESTING_STRICT`, or naming it with `--backend`, makes that an error). An `EXPERIMENTAL` tier for backends still being ported: reported, not gating.

### Changed
- Postgres for tests runs from `compose.yaml` (TCP, localhost:54330) instead of host binaries. `mise run pg-start` / `pg-stop` are replaced by `mise run up` / `down`.
- `mise run test` runs the test binaries under a 12 GB address-space cap.
- `bench/tpch_data.py` is idempotent (each database is left alone if already loaded) and no longer needs `psql`.
- `mise run conformance` builds with the same flags as `mise run test`.

### Removed
- `scripts/pg.sh`.

## [ibis-rs 0.1.1, ibis-polars 0.1.0] - 2026-11-15

### Added
- `ibis-polars`: Polars backend. The IR is translated to a Polars `LazyFrame` (no SQL) with casts to the Ibis output types. `EXISTS` and correlated scalar subqueries are rewritten as joins; window frames, ranks and `ntile` are computed from prefix sums and row positions. Passes all 551 Ibis cases and the 22 TPC-H queries. `Polars::register` takes a `DataFrame`; `Polars::lazy` returns the query as a `LazyFrame`.
- `ibis-rs`: `polars` feature and `connect("polars://")`.
- Conformance: Polars as a fourth backend; an `unsupported` status for backends still being built out (`PARTIAL`), which does not fail the run.

### Changed
- `ibis-rs`: `polars` is now the default feature (0.1.0 had no default backend).

## [ibis-rs 0.1.0] - 2026-11-14

### Added
- `ibis-rs` 0.1.0: the facade crate. Re-exports the API, one cargo feature per backend (`duckdb`, `sqlite`, `postgres`; none on by default), and `connect(url)` returning a `Box<dyn Backend>`.

## [0.1.0] - 2026-11-14

First release on crates.io: `ibis-core`, `ibis-sql`, `ibis-duckdb`, `ibis-postgres`, `ibis-sqlite`.

### Added
- `LICENSE-MIT`, `LICENSE-APACHE` (linked into each crate so packages ship them) and `NOTICE` with the Ibis attribution.
- `ibis-postgres`: Postgres backend (`postgres` crate, binary protocol including NUMERIC, `COPY` for table loads, server error detail surfaced). `scripts/pg.sh` and `mise run pg-start` / `pg-stop` run a throwaway local cluster.
- Bound column references: `t.col("x")` (Ibis `t.x`) next to unbound `col("x")` (Ibis `_.x`). Correlated subqueries, `EXISTS` (`.any()` over another table), scalar subqueries, `isin_col` / `notin_col`.
- Joins: all six kinds, general predicates, self joins through `view()`, Ibis's `_right` collision renaming. Set operations: `union` / `intersect` / `difference` with `distinct`.
- Table API: `mutate`, `drop`, `relocate`, `distinct`, `fill_null`, `drop_null`, `offset`, `head`.
- Expressions: `isin` / `notin`, `cases`, `coalesce`, `least` / `greatest`, `clip`, `isnan` / `isinf`, trigonometry, `log`, `radians` / `degrees`, xor; temporal extraction, `truncate`, `strftime`, date and timestamp constructors; window functions (`rank`, `dense_rank`, `percent_rank`, `cume_dist`, `ntile`, `lag`, `lead`, `first`, `last`, `nth`, cumulative aggregates, `over`).
- Conformance: 551 Ibis cases (was 108) across nine modules, one file per Ibis test module on each side; parquet goldens with an index; multi-table and in-memory fixtures; `schema` pseudo-backend comparing output names and types to Ibis's; a record of whether Python Ibis itself agrees across backends; a gate on goldens with no Rust port; `mise run check-ids`.
- TPC-H: Ibis's 22 queries ported and passing on DuckDB, Postgres and SQLite; `mise run bench` compares compile and execute time with Python Ibis.
- `ibis-core`: Ibis datatypes/schema with Arrow mapping; IR (`Rel`/`RelOp`, `Expr` with closed op enums documenting Ibis semantics); `Table`/`Expr` builder API with eager schema validation and type inference; `Backend` trait; Arrow <-> row conversions that conform results to the Ibis output schema.
- `ibis-sql`: IR -> SQL compiler (merge-or-wrap `Select` lowering) and the `Dialect` trait with per-op override hooks.
- `ibis-duckdb`: DuckDB backend (bundled, Arrow-native results).
- `ibis-sqlite`: SQLite backend with Rust polyfills (regex, padding, reverse, math, variance/stddev aggregates), LIKE -> GLOB for case-sensitive LIKE, declared-type round-tripping.
- Conformance suite: `conformance/cases.py` (108 ids ported from Ibis's test_string, test_numeric, test_aggregation), `gen_goldens.py` (goldens from Ibis 12.0.0 on DuckDB), `ibis-conformance` runner + `cargo test` gate. 108/108 pass on DuckDB and SQLite.
- `mise` tasks: setup, build, test, lint, conformance, goldens, dev.

- Spike `spikes/unparser`: DataFusion 55 Unparser conformance across Postgres 18, DuckDB, SQLite (62 cases; findings in `spikes/unparser/FINDINGS.md`).
- Design doc (`docs/design.md`): owned IR, backend plugins, Ibis-test-suite parity as the north star.
- Cargo workspace scaffold, `mise.toml` front door, `install-skills.sh`.

### Changed
- License: library crates are now `MIT OR Apache-2.0` (was Apache-2.0). `ibis-conformance` and `conformance/` stay Apache-2.0 because their cases are adapted from Ibis; see `NOTICE`. `ibis-conformance` is `publish = false`.
- Integer typing follows Ibis: integer literals take the smallest integer type, `+ - * %` widen by value bounds, `//` is always `int64`.
- Division follows IEEE 754 on every backend: `x / 0` is infinity and `0 / 0` is NaN (NULL on SQLite, which has no NaN). Postgres raised an error before.
- A sorted table stays sorted through later `filter` / `select` / `distinct`; a later `order_by` takes precedence and earlier keys break ties.
- A window with an ordering and no frame covers the whole partition, as in Ibis. SQL's default would make it a running aggregate.
- Set operations match columns by name, not position.
- `Dialect::agg` receives the `FILTER` and `OVER` clauses as a tail to place after the aggregate call.
- Every FROM source is aliased and column references are qualified.

### Fixed
- Ordering was dropped whenever the compiler nested a query.
- Windowed aggregates on Postgres and SQLite put `OVER` after the dialect's cast wrapper, which is invalid SQL.
- Joining a table to a table derived from itself resolved left-side columns to the right side.
- `INTERSECT ALL` / `EXCEPT ALL` on SQLite (unsupported natively; emulated with `ROW_NUMBER`).
- `drop_null` with an empty subset and `how = all` kept rows; it drops every row, as Ibis does.
