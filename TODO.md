# TODO

North star: Ibis parity, measured by ported Ibis backend test ids passing per backend.
The target shape is Ibis's backend support matrix: https://ibis-project.org/backends/support/matrix (which backend supports which operation).
Current: 1,291 Ibis tests (1,162 expression cases, 129 procedures) + 22 TPC-H queries. DuckDB 1,289, Polars 1,261, Trino 1,261, Postgres 1,252, Spark 1,238, ClickHouse 1,197, SQLite 881, Impala 862, MySQL 842; everything else is an expected failure with its reason in the case file (README has the table).
Coverage of the Ibis suite: 1,288 of 1,722 applicable test ids (75%); per module at https://brojonat.github.io/ibis-rs/

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
- [x] Arrays, structs and maps: `DataType`/`Value`, the Ibis operations, lambdas, unnest, collect; test_array, test_struct and test_map ported (311 cases) on DuckDB, Postgres, ClickHouse, Trino and Spark
- [x] Polars: arrays, structs and maps (all 311 cases), unnest, collect, views. Lists, structs, and maps as lists of `{key, value}` structs; `crates/ibis-polars/src/nested.rs` has the split between Polars expressions and the row-by-row fallback

## Next: parity
- [ ] test_temporal tier 2 (11 ids left of 163 applicable)
  - [x] Intervals: `DataType::Interval`, `interval(n, unit)`, `as_interval`, a timestamp or a date moved by one (35 ids)
  - [ ] Intervals as values: a result column of intervals (`Value`, Arrow, each backend's reader), `test_temporal_binop[timestamp-subtract-timestamp]` and `[date-subtract-date]`, `test_interval_literal`; comparisons and columns of intervals
  - [x] `iso_year`, `epoch_seconds`, `epoch_days`, `delta` of dates and timestamps, numbers as timestamps (`cast`, `as_timestamp(unit)`), date and timestamp literals far from now, a date compared with a built one (32 ids; the 4 SQL-text snapshot ids are not applicable)
  - [x] `bucket` (7 ids); strings to timestamps and dates by a C pattern (2 ids; the two ids that pass Java's pattern through are not applicable)
  - [ ] `bucket` of months and years (no fixed width: needs calendar arithmetic)
  - [ ] The `time` type: `test_delta[time]`, `test_string_as_time`, `test_time_literal`, `test_extract_time_from_timestamp`
  - [ ] A timestamp finer than a microsecond: `test_timestamp_truncate[ns-ns]`, `test_timestamp_precision_output[ns]`
  - [x] Time zones as part of the type (casts, comparisons, results), `now` and `today` (18 ids)
  - [ ] The wall clock of a zone other than UTC on the SQL backends (`AT TIME ZONE` and its relatives): fields, dates, formats, a cast back to no zone. Today `Unsupported`
  - [ ] Reading a column that the engine stores with a zone (`timestamptz`): not looked at
  - [ ] strftime with a column format (1 id)
  - [ ] ibis-polars: `floordiv` goes through float64, so an int64 above 2^53 loses its last digits (a bucket of a timestamp after the year 2255)
- [x] test_aggregation: all 162 applicable ids
  - [x] Reductions of one argument: mode, median, approx_median, approx_nunique, kurtosis, bit_and/or/xor, arbitrary, first/last, notany/notall (27 ids; the Python-UDF ids are not applicable)
  - [x] `AggOptions` on `Expr::Agg` (further arguments, order, distinct, include_null) and `Dialect::reduction`; argmin/argmax and corr/cov on it (26 ids)
  - [x] quantile, quantiles, approx_quantile, approx_quantiles (10 ids)
  - [x] group_concat with a separator, a condition, an order, over a window (9 ids); `Expr::order_by` for a reduction's order
  - [x] first/last with `order_by` and `include_null` (16 ids), collect with `order_by`, `distinct`, `include_null` (16 ids), test_count_distinct_star (2 ids), `test_corr_cov[*-corr_samp]` (2 ids; a procedure, since DuckDB has no sample correlation to make a golden from)
  - [ ] ClickHouse and Spark: reductions that read their rows in an order (`first`, `last`, `collect`, `group_concat` with `order_by`). Both can sort an array of `(key, value)` pairs; Ibis does not either
  - [ ] An ordered `first`/`last` in a projection is `FIRST_VALUE` over an ordered window, which does not pass over NULLs (as in Ibis); `include_null` is not looked at there
  - [ ] A count of distinct rows on SQLite (one argument to `COUNT(DISTINCT ...)`), and on MySQL and Impala for rows with a NULL (they are not counted there)
  - [x] topk, value_counts, test_filter, test_grouped_case, test_group_by_expr, test_group_by_scalar, test_binds_are_cast, test_empty_sum, test_agg_sort, test_agg_name_in_output_column (13 ids)
  - [ ] Engines that have a reduction Ibis does not use there (Spark `mode`, `bit_and`, kurtosis; ClickHouse `kurtSamp`): today `Unsupported`, as in Ibis
  - [ ] MySQL `BIT_AND` of no rows is all ones, not NULL
- [x] test_numeric: 154 of 159 applicable ids. Bitwise operators (`bitwise_and`/`or`/`xor`/`not`, `lshift`, `rshift`), `random()`, `histogram(nbins)`, typed and decimal literals, `log` with any base, decimal columns in memtables (48 ids)
  - [ ] The five left have no DuckDB result to compare with: `test_numeric_literal[float16]` (no such type there or in ibis-rs), `test_decimal_literal[decimal-big]` (precision 76), `[decimal-infinity+]`, `[decimal-infinity-]`, `[decimal-NaN]` (not decimals on DuckDB; a decimal literal is an f64 in ibis-rs, so a `Value::Decimal` comes first)
  - [ ] A decimal with no precision or scale (Ibis `dt.decimal`): `DataType::Decimal` always has both, so `test_decimal_literal[default]` differs from Ibis in its type (the one expected failure of the schema check)
  - [ ] `histogram` with a bin width or a base (Ibis `binwidth=`, `base=`)
  - [ ] ibis-polars: `random()` (needs a source of random numbers per row), `lshift`/`rshift` past bit 62 (they multiply and divide by a power of two)
  - [ ] Shifts by a negative count or by 64 and more differ between engines; untested
- [x] test_string: 116 of 127 applicable ids. `translate`, `find_in_set`, `split`, `re_split`, `join`, `levenshtein`, `substitute`, slices and negative indexes, strip of all whitespace, pads that do not cut, the tests on accents and an emoji (69 ids)
  - [ ] `test_parse_url` (10 ids): `protocol`, `authority`, `userinfo`, `host`, `file`, `path`, `query`, `fragment` of a URL. Ibis has none of them on DuckDB, so no goldens: procedures, on the engines with URL functions (ClickHouse, Trino, Impala, Spark) and a Rust function for SQLite
  - [ ] `test_string_methods_accents_and_emoji[find_in_set_w_comma]`: Ibis's docstring says an item with a comma is never found; DuckDB (and ibis-rs, which compares) finds it
  - [ ] `translate` and `levenshtein` on MySQL and Polars; `split` by a delimiter that is not a literal on Spark and Polars; a pattern per row for `re_split` on ClickHouse and Polars
  - [ ] ibis-clickhouse: `SUBSTRING` (so `substr`, `capitalize`, `left`) counts bytes; `substringUTF8` counts characters
  - [ ] `join` on SQLite before 3.44 (no `CONCAT_WS`); `strip` takes off Unicode whitespace on Polars and the six ASCII characters elsewhere
- [ ] test_generic (66 ids left of 233 applicable)
  - [x] Literals of NULL and booleans, `identical_to`, a window in `filter`, `try_cast`, `typeof_`, `hash`, `hexdigest`, casts of arrays and structs, and the plain select, sort and union tests (34 ids)
  - [ ] Table slices with negative bounds (`test_static_table_slice`, `test_dynamic_table_slice*`: 13 ids), `sample` (6), `order_by(random())` (1)
  - [ ] `distinct(on=..., keep=...)` (7 ids; six read Ibis's `diamonds` table, which is not among the fixtures), `pivot_longer`/`pivot_wider` (6)
  - [ ] `info` and `describe` (7 ids)
  - [ ] `ibis.memtable` (19 ids; with test_client slice 4), `test_named_literal` (a table from a value, with no table under it)
  - [ ] `hashbytes` (binary digests; no golden, DuckDB has none in Ibis), `hexdigest` and `hashbytes` with another algorithm than SHA-256, and on SQLite and Polars (needs a SHA-256 in Rust: a dependency)
  - [ ] `try_cast` on Postgres, MySQL and SQLite (a test of the text before the cast would do for numbers)
  - [ ] The error tests (`test_table_fill_null_invalid`, `test_drop_null_invalid`), `test_column_fill_null[nan_col]` (SQLite only), `test_literal_na[time]`, `test_interactive` (to mark not applicable)
- [ ] Rest of test_array: `ibis.range` over timestamps (10 ids; needs an interval type), `test_repr_timestamp_array` (a repr test: not applicable)
- [ ] Nested types, loose ends: several `unnest`s in one projection on Trino and Spark (a join each today: a cross product, not DuckDB's side-by-side), a JSON type (Postgres `jsonb` columns that ibis-rs did not create come back as strings), Postgres arrays of arrays as JSONB
- [ ] test_client (203 ids), in slices. These test the connection API, not expressions: they run as procedural tests per backend (`crates/ibis-conformance/src/client.rs`, ids `client::<module>::<pytest id>`, no goldens)
  - [x] Slice 1, table lifecycle: `Backend::{drop_table, insert, insert_from, rename_table, create_view, drop_view, create_empty_table}`; shared DDL text in `ibis_sql::ddl` (`SqlClient`, `sql_client_methods!`); each backend's loader split from its `create_table` so `insert` reuses it. 25 ids ported, passing on all nine backends (Polars: views unsupported)
  - [x] Slice 1, the rest: test_insert_using_col_name_not_position (3), test_overwrite (`Backend::create_table_from`), views on Polars
  - [x] Slice 2: `Backend::create_table_with` with `CreateOptions { overwrite, temp }` (now the one method a backend implements to create a table), `NOT NULL` columns written and read back, `upsert`/`upsert_from` (`MERGE INTO`: DuckDB and Postgres), `Backend::sql` (`RelOp::Sql`). 18 ids ported; 43 of test_client's 83 applicable ids are still missing
  - [ ] Left from slices 1 and 2: test_insert_into_table_missing_columns (insert only the columns given, so a column's `DEFAULT` applies; needs a statement method on `Backend` for the test's DDL), test_create_table_timestamp (needs the timestamp scale; applies to Trino only)
  - [x] `con.sql` on Polars, through `polars-sql` (the Polars project's own crate, the `sql` feature of `polars`)
  - [ ] `upsert` where there is no `MERGE`: MySQL (`INSERT ... ON DUPLICATE KEY UPDATE`) and SQLite (`ON CONFLICT DO UPDATE`) have their own forms, both needing a unique index on the column; Ibis does not use them either
  - [ ] ClickHouse temporary tables: need a `session_id` on every request, and a session takes one request at a time
  - [ ] Slice 3: catalogs and databases (`list_catalogs`, `list_databases`, `create_database`, tables addressed by database)
  - [ ] Slice 4: `ibis.memtable` (a relation holding its data; backends register it on execute), test_in_memory_table and friends
  - [x] Marked not applicable in conformance/not_applicable.json (now keyed by `module::test` as well as by module): the default/set/get backend options, `__array__`, interactive repr and mimebundle, `has_operation`, memtable bookkeeping through a mock, the suite's own fixture test
  - [ ] Still to decide: `test_from_connection` (wrapping a driver connection: `Postgres::new(client)` exists), `test_json_to_pyarrow` (needs a JSON type), `test_schema_with_caching`, `test_close_connection`, `test_connect_url`
- [x] test_dot_sql: `Table::alias`, `Backend::sql_over` (Ibis `t.sql`), `Table::as_scalar`. 15 ids ported as procedures (`crates/ibis-conformance/src/dot_sql.rs`), passing on all nine backends; 45 ids not applicable (sqlglot transpiling between dialects, a repr test)
  - [ ] `test_order_by_no_projection` reads awards_players, not Ibis's astronauts table (not among the fixtures): load astronauts and use it
  - [ ] An alias inside a subquery expression (`ScalarSubquery`, `InSubquery`) is not found by `Table::aliases`
  - [ ] `con.sql(query, schema)` on the `Backend` trait for an explicit schema (today: `Table::sql(query, schema)` directly)
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
- [x] ibis-spark (Spark Thrift server) 551/551, 22/22; ibis-impala 548/551 (3 xfail), 22/22; both over `ibis-hiveserver2`
- [x] Published 2026-11-16: ibis-sql 0.1.3, ibis-hiveserver2 0.1.0, ibis-spark 0.1.0, ibis-impala 0.1.0, ibis-rs 0.1.4 (the `spark` and `impala` features)
- [ ] ibis-hiveserver2: TLS, Kerberos, and the HTTP transport (today: a plain socket, with or without SASL PLAIN)
- [ ] Spark: Spark Connect as a second way in (Databricks and Spark 4 clusters often expose that and no Thrift server)
- [ ] Impala: `IN (subquery)` in a select list (2 xfail; a left join against the distinct subquery would do it), `NTH_VALUE` (1 xfail), `LENGTH` and friends count bytes on an Impala before 4.1 (later ones get `UTF8_MODE` from the connection)
- [ ] Spark and Impala: a load path that is not SQL text, as for Trino
- [ ] `rows()` and `insert_text()` are the same code in ibis-spark and ibis-impala (and `insert_text` in ibis-trino): share them once a fourth copy threatens
- [ ] Cloud-only backends (BigQuery, Snowflake): credentials from environment variables, tests skipped without them
- [ ] ClickHouse: the native protocol or Arrow format instead of tab-separated text for large results
- [ ] Add MySQL, ClickHouse, Trino, Spark and Impala to `mise run bench` and to the Python Ibis cross-check in gen_goldens.py
- [ ] `x IN (NULL)` on ClickHouse is false, not NULL (the one xfail)

## Next: parity site
- [ ] Per-backend operation matrix on the same site, shaped like Ibis's support matrix, generated from the cases and `notimpl` marks
- [ ] Rust line links are best-effort when a case id is built with `format!`; give cases a `file!()`/`line!()` at registration
- [ ] Trim page weight (6.6 MB with seven backends, more with nine; the generated queries are most of it)

## Next: Polars
- [ ] Arrow in and out through the C Data Interface. Today `create_table` and results go through rows of `Value`, which is slow and memory-hungry on large tables
- [ ] `polars://path`: scan a directory of parquet/CSV files lazily instead of holding tables in memory
- [ ] Add Polars to `mise run bench` (Rust side, and Python Ibis's Polars backend as the baseline)
- [ ] Nested ops that run row by row in Rust (`nested.rs`: zip, map get/contains/merge, `index`, `remove`, `unique`, `union`, `intersect`, `flatten`, `modes`, repeat, arrays of arrays) could be Polars expressions where one gives DuckDB's answer; `map`/`filter` with a function that reads a column needs the frame exploded and regrouped
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
- [ ] Versions are uneven (ibis-rs 0.1.4, ibis-sql 0.1.3, ibis-duckdb 0.1.1, the rest 0.1.0): realign at the next minor release
- [ ] CI is 7 minutes, 4 of them the conformance run, CPU-bound on a 4-core runner. Splitting the backends over two jobs (results merged for the site) would roughly halve that, and also eases memory: the containers take about 10 GB of the runner's 16
- [ ] Spark on versions before 3.4: `TIMESTAMP_NTZ`, `LIMIT ... OFFSET`, `STARTSWITH`/`CONTAINS`/`ILIKE` need older spellings; test against an older image
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
