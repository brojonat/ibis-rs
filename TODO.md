# TODO

North star: Ibis parity, measured by ported Ibis backend test ids passing per backend.
Current: 551 Ibis cases + 22 TPC-H queries. DuckDB 551, Postgres 551, SQLite 530 (21 need NaN).

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
- [x] TPC-H: Ibis's 22 queries on all three backends, benchmark against Python Ibis (`mise run bench`)

## Next: parity
- [ ] Parity report: all Ibis test ids vs ported ids, per module (check_ids.py already collects them)
- [ ] test_temporal tier 2: intervals and date arithmetic, deltas, buckets, time zones, the `time` type, strftime with a column format
- [ ] Rest of test_aggregation: grouped reductions, mode, argmin/argmax, approx, collect
- [ ] Rest of test_string: translate, split, join, find_in_set, capitalize, slices
- [ ] Nested types (array, struct, map) and test_array / test_struct / test_map
- [ ] `distinct(on=..., keep=...)`, `GroupedTable.order_by().mutate()`, negative slices
- [ ] Timestamp scale in `DataType` (the schema check normalizes `timestamp(6)` away today)
- [ ] SQLite `least`/`greatest`: scalar MIN/MAX return NULL if any argument is NULL; Ibis skips NULLs
- [ ] Float literals are read as DECIMAL by DuckDB and NUMERIC by Postgres; emit typed literals
- [ ] Ordering is lost through a wrap when the sort column is dropped or a LIMIT sits between

## Next: performance
- [ ] Cache `Rel::deref_map()`; it is recomputed recursively, so join-chain queries take 1-3 ms to compile (TPC-H Q5, Q7, Q8, Q9)
- [ ] Benchmark at sf=1; add primary keys and indexes to the Postgres TPC-H tables (Q20 takes 24 s without)

## Next: release automation
Plan: lockstep versions (one `workspace.package.version`), hand-written CHANGELOG, tag-triggered publish.
`vX.Y.Z` tag -> CI gate -> `cargo publish --workspace` -> GitHub release with that version's CHANGELOG section.
Steps are in dependency order.
- [ ] Decide when the repo goes public (publishing to crates.io makes the source public either way)
- [x] crates.io names checked 2026-11-14: ibis-core, ibis-sql, ibis-duckdb, ibis-sqlite, ibis-postgres and ibis-rs are free. `ibis` is taken (a 1.5 KB "WIP" placeholder published 2026-09-27, no repository), so the facade needs another name (`ibis-rs`) or a request to that owner. Names are held only once published
- [x] Licensing: library crates `MIT OR Apache-2.0`, conformance suite Apache-2.0 (adapted from Ibis), LICENSE files + NOTICE
- [ ] Ask the Ibis maintainers about the name before publishing (`ibis-rs`, a crate called `ibis`); the Apache license grants no trademark rights
- [x] Manifests: `repository`, `readme`, `keywords`, `categories` in each crate
- [ ] `rust-version` (MSRV) in each crate; needs older toolchains to verify
- [x] Internal deps carry versions; `cargo publish --workspace --dry-run` passes
- [ ] Features: make `bundled` an opt-out default feature of ibis-duckdb and ibis-sqlite, so users can link a system library; check the docs.rs build of ibis-duckdb fits its time limit
- [ ] Pin the toolchain (`rust-toolchain.toml`) and set the MSRV
- [ ] CI workflow (`.github/workflows/ci.yml`) on push and PR: `mise run lint`, `mise run test`. Needs Postgres binaries on PATH for scripts/pg.sh, mise + uv, a cache for `ci/` data, and `Swatinem/rust-cache` (cold DuckDB build is 10+ min). Private-repo Actions minutes are limited
- [ ] `mise run release <version>`: bump the workspace version and internal dep versions, rename `[Unreleased]` in CHANGELOG.md to `[X.Y.Z] - date`, commit, tag. Pushing stays manual
- [ ] Release workflow (`.github/workflows/release.yml`) on `v*` tags: check tag == workspace version, run the gate, `cargo publish --workspace`, `gh release create` with the CHANGELOG section as the body
- [x] First publish: 0.1.0 of the five library crates is on crates.io (2026-11-14), tagged `v0.1.0`
- [ ] Publish the facade crate `ibis-rs` (one dependency, a cargo feature per backend) before someone else takes the name
- [ ] Configure trusted publishing per crate (repo, workflow file `release.yml`, environment `release`) so CI needs no stored token
- [ ] After 0.1.0 is on crates.io: `cargo-semver-checks` job in CI
- [ ] When there is a user-facing binary: `dist` (cargo-dist) for cross-platform binaries and installers on the same tags
- [ ] Alternative to the tag flow if release PRs are preferred: release-plz (needs a PAT or GitHub App, and its generated changelog turned off)

## Later
- [ ] Facade crate (`ibis-rs`; `ibis` is taken on crates.io) with cargo features + `connect(url, &registry)`
- [ ] ibis-datafusion backend: IR -> LogicalPlan; arrow 59 vs 60 needs C Data Interface hop
- [ ] ibis-polars backend: IR -> LazyFrame (proves the IR isn't SQL-shaped)
- [ ] More SQL backends: BigQuery, Snowflake, Trino, ClickHouse, MySQL (dialect + driver each)
- [ ] Upstream DataFusion unparser bug reports from spikes/unparser/FINDINGS.md
- [ ] Selectors (s.numeric(), s.matches(...)), deferred-style sugar, typed derive layer
- [ ] UDF escape hatch
