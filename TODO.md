# TODO

North star: Ibis parity, measured by ported Ibis backend test ids passing per backend.
The target shape is Ibis's backend support matrix: https://ibis-project.org/backends/support/matrix (which backend supports which operation).
Current: 551 Ibis cases + 22 TPC-H queries. Polars 551, DuckDB 551, Postgres 551, Trino 551, ClickHouse 550 (1 marked notyet in Ibis too), MySQL 531 (20 need NaN or infinity), SQLite 530 (21 need NaN).
Coverage of the Ibis suite: 561 of 1,899 applicable test ids (30%); per module at https://brojonat.github.io/ibis-rs/

## Done
- [x] Spike: DataFusion Unparser across Postgres/DuckDB/SQLite (spikes/unparser)
- [x] Design doc, workspace scaffold, mise front door
- [x] ibis-core: datatypes, schema, IR, Table/Expr API, type inference, Backend trait
- [x] ibis-sql: Select-merging compiler + Dialect trait
- [x] ibis-duckdb, ibis-sqlite, ibis-postgres backends
- [x] Bound column refs (`t.col`), correlated subqueries, EXISTS, scalar subqueries, joins with general predicates
- [x] Order-preserving wrap: ordering carried in the IR and hoisted over nested selects
- [x] Conformance harness: per-module cases, parquet goldens, multi-table and in-memory fixtures
- [x] Output dtype parity (`schema` pseudo-backend)
- [x] Case ids verified against `pytest --collect-only` (`mise run check-ids`)
- [x] Ported: test_generic, test_numeric, test_string, test_aggregation (part), test_temporal (tier 1), test_join, test_window, test_set_ops, test_conditionals
- [x] ibis-polars backend (IR -> LazyFrame): 551/551 and 22/22; default backend of the facade
- [x] Facade crate `ibis-rs`: feature per backend, `connect(url)`
- [x] TPC-H: Ibis's 22 queries on every backend, benchmark against Python Ibis (`mise run bench`)
- [x] Backends in containers: `compose.yaml`, `mise run up` / `down`; the runner skips a backend that is down (`IBIS_TESTING_STRICT` makes that an error)
- [x] ibis-clickhouse backend (HTTP): 550/551 + 1 xfail, 22/22
- [x] Parity site (`mise run site`, conformance/site.py), published to GitHub Pages by CI
- [x] CI: lint + test on every backend + site (.github/workflows/ci.yml)
- [x] `mise run test` under a memory cap
- [x] Published 2026-11-15: ibis-sql 0.1.1, ibis-clickhouse 0.1.0, ibis-rs 0.1.2 (the `clickhouse` feature)
- [x] ibis-mysql backend: 531/551 + 20 xfail, 22/22. Full outer joins are emulated (Ibis's own MySQL backend has none)
- [x] ibis-trino backend (HTTP): 551/551, 22/22

## Next: parity
- [ ] test_temporal tier 2: intervals and date arithmetic, deltas, buckets, time zones, the `time` type, strftime with a column format
- [ ] Rest of test_aggregation: grouped reductions, mode, argmin/argmax, approx, collect
- [ ] Rest of test_string: translate, split, join, find_in_set, capitalize, slices
- [ ] Nested types (array, struct, map) and test_array / test_struct / test_map (303 ids, the largest unported block)
- [ ] test_client (203 ids): create/drop/insert/list through the `Backend` trait; decide which apply
- [ ] TPC-DS (100 ids): Ibis's queries, like TPC-H
- [ ] `distinct(on=..., keep=...)`, `GroupedTable.order_by().mutate()`, negative slices
- [ ] Timestamp scale in `DataType` (the schema check normalizes `timestamp(6)` away today)
- [ ] SQLite `least`/`greatest`: scalar MIN/MAX return NULL if any argument is NULL; Ibis skips NULLs
- [ ] Float literals are read as DECIMAL by DuckDB and NUMERIC by Postgres; emit typed literals
- [ ] Ordering is lost through a wrap when the sort column is dropped or a LIMIT sits between
- [ ] Review `conformance/not_applicable.json`: the module-level calls (307 ids) were made without reading every test

## Next: backends
One compose service + one crate each; start in `EXPERIMENTAL` (README: "Adding things").
- [x] Published 2026-11-16: ibis-sql 0.1.2, ibis-duckdb 0.1.1, ibis-mysql 0.1.0, ibis-trino 0.1.0, ibis-rs 0.1.3 (the `mysql`, `trino` and `duckdb-linked` features)
- [ ] MySQL: TLS (the `mysql` crate is built without it), `caching_sha2_password` over plain TCP is untested (the test server has an empty root password)
- [ ] Trino: a load path that is not SQL text (`create_table` sends rows inside INSERT statements); the test server's memory catalog loses its tables on restart
- [ ] Spark (Spark Connect container) and Impala (needs a metastore and storage: start from Ibis's `compose.yaml`)
- [ ] Cloud-only backends (BigQuery, Snowflake): credentials from environment variables, tests skipped without them
- [ ] ClickHouse: the native protocol or Arrow format instead of tab-separated text for large results
- [ ] Add MySQL, ClickHouse and Trino to `mise run bench` and to the Python Ibis cross-check in gen_goldens.py
- [ ] `x IN (NULL)` on ClickHouse is false, not NULL (the one xfail)

## Next: parity site
- [ ] Per-backend operation matrix on the same site, shaped like Ibis's support matrix, generated from the cases and `notimpl` marks
- [ ] Rust line links are best-effort when a case id is built with `format!`; give cases a `file!()`/`line!()` at registration
- [ ] Trim page weight (6.6 MB with seven backends; the generated queries are most of it)

## Next: Polars
- [ ] Arrow in and out through the C Data Interface. Today `create_table` and results go through rows of `Value`, which is slow and memory-hungry on large tables
- [ ] `polars://path`: scan a directory of parquet/CSV files lazily instead of holding tables in memory
- [ ] Add Polars to `mise run bench` (Rust side, and Python Ibis's Polars backend as the baseline)
- [ ] Not supported yet (reported as `Error::Unsupported`): RANGE window frames, min/max over frames wider than 256 rows, std/var over a frame, outer joins on non-equality conditions, correlated scalar subqueries outside a filter or correlated by a non-equality, uncorrelated EXISTS

## Next: performance
- [ ] Cache `Rel::deref_map()`; it is recomputed recursively, so join-chain queries take 1-3 ms to compile (TPC-H Q5, Q7, Q8, Q9)
- [ ] Benchmark at sf=1; add primary keys and indexes to the Postgres TPC-H tables (Q20 takes 24 s without)

## Next: release automation
Plan: lockstep versions (one `workspace.package.version`), hand-written CHANGELOG, tag-triggered publish.
`vX.Y.Z` tag -> CI gate -> `cargo publish --workspace` -> GitHub release with that version's CHANGELOG section.
Steps are in dependency order.
- [x] Repo is public (2026-11-15)
- [x] crates.io names checked 2026-11-14: ibis-core, ibis-sql, ibis-duckdb, ibis-sqlite, ibis-postgres and ibis-rs are free. `ibis` is taken (a 1.5 KB "WIP" placeholder published 2026-09-27, no repository), so the facade needs another name (`ibis-rs`) or a request to that owner. Names are held only once published
- [x] Licensing: library crates `MIT OR Apache-2.0`, conformance suite Apache-2.0 (adapted from Ibis), LICENSE files + NOTICE
- [ ] Ask the Ibis maintainers about the name before publishing (`ibis-rs`, a crate called `ibis`); the Apache license grants no trademark rights
- [x] Manifests: `repository`, `readme`, `keywords`, `categories` in each crate
- [ ] `rust-version` (MSRV) in each crate; needs older toolchains to verify
- [x] Internal deps carry versions; `cargo publish --workspace --dry-run` passes
- [x] ibis-duckdb: `bundled` is an opt-out default feature; ibis-rs has `duckdb-linked`
- [ ] The same for ibis-sqlite; add `duckdb-linked` to the docs.rs features if docs.rs builds can download the library
- [ ] CI no longer builds bundled DuckDB (same DuckDB commit as the prebuilt library, but the static build is only exercised locally): a weekly job with the default flags
- [ ] Pin the toolchain (`rust-toolchain.toml`) and set the MSRV
- [x] CI workflow (`.github/workflows/ci.yml`) on push and PR: `mise run lint`, `mise run up`, `mise run test`, `mise run site`
- [ ] Versions are uneven (ibis-rs 0.1.3, ibis-sql 0.1.2, ibis-duckdb 0.1.1, the rest 0.1.0): realign at the next minor release
- [ ] `mise run release <version>`: bump the workspace version and internal dep versions, rename `[Unreleased]` in CHANGELOG.md to `[X.Y.Z] - date`, commit, tag. Pushing stays manual
- [ ] Release workflow (`.github/workflows/release.yml`) on `v*` tags: check tag == workspace version, run the gate, `cargo publish --workspace`, `gh release create` with the CHANGELOG section as the body
- [x] First publish: 0.1.0 of the five library crates is on crates.io (2026-11-14), tagged `v0.1.0`
- [x] Facade `ibis-rs` published (0.1.0 without a default backend, 0.1.1 with Polars as the default)
- [ ] Configure trusted publishing per crate (repo, workflow file `release.yml`, environment `release`) so CI needs no stored token
- [ ] After 0.1.0 is on crates.io: `cargo-semver-checks` job in CI
- [ ] When there is a user-facing binary: `dist` (cargo-dist) for cross-platform binaries and installers on the same tags
- [ ] Alternative to the tag flow if release PRs are preferred: release-plz (needs a PAT or GitHub App, and its generated changelog turned off)

## Later
- [ ] `connect` with a registry, so third-party backends can add URL schemes
- [ ] ibis-datafusion backend: IR -> LogicalPlan; arrow 59 vs 60 needs C Data Interface hop
- [ ] More SQL backends: BigQuery, Snowflake, MSSQL, Oracle (dialect + driver each)
- [ ] Upstream DataFusion unparser bug reports from spikes/unparser/FINDINGS.md
- [ ] Selectors (s.numeric(), s.matches(...)), deferred-style sugar, typed derive layer
- [ ] UDF escape hatch
