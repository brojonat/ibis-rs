# Changelog

All notable changes to this project are documented here. Format: [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Added
- Arrays, structs and maps. `DataType::{Array, Struct, Map}` with Arrow conversion and `FromStr` for Ibis's type spellings (`"map<string, array<int64>>".parse()`); `Value::{Array, Struct, Map}`, and `lit(vec![1, 2, 3])` is an array literal.
- Expressions over them, named as in Ibis: `array`, `struct_`, `map`, `range`; on arrays `at`, `slice`, `concat` (and `+`), `repeat` (and `*`), `contains`, `index`, `remove`, `unique`, `sort`, `union`, `intersect`, `flatten`, `zip`, `mins`/`maxs`/`sums`/`means`/`anys`/`alls`/`modes`, `map`/`filter` and their `_with_index` forms taking Rust closures, `unnest`; on structs `field`; on maps `get`, `get_or`, `contains`, `keys`, `values`, `length`, `+`. `collect` aggregates a column into an array.
- `Table::unnest(column, offset, keep_empty)`, `Table::unpack(columns)` and `Table::lift(column)`.
- Support in ibis-duckdb, ibis-postgres (arrays; structs and maps as JSONB), ibis-clickhouse (`Array`, named `Tuple`, `Map`), ibis-trino (`ARRAY`, `ROW`, `MAP`) and ibis-spark (`ARRAY`, `STRUCT`, `MAP`): expressions, results, `create_table` and `table`. SQLite, MySQL and Impala refuse the types; Polars does not support them yet.
- `ibis_sql::json`: nested values to and from JSON, the form several engines exchange them in.
- `Dialect` hooks: `nested_literal`, `array_lambda`, `unnest`, `unnest_in_select`, `in_subquery_needle`. The default `type_name` spells nested types as DuckDB does.
- `Backend` methods for tables and views, as on an Ibis connection: `create_empty_table`, `drop_table`, `insert` (append or overwrite, from Arrow data), `insert_from` (from a query), `rename_table`, `create_view`, `drop_view`. They have default bodies (`Unsupported`, or built on the others), so a backend outside this repository still compiles. All nine backends implement them; Polars has no views yet.
- `ibis_sql::ddl`: the statements behind those methods, shared by the SQL backends through the `SqlClient` trait and the `sql_client_methods!` macro. `Dialect::rename_table` and `Dialect::truncate_table` for the engines that spell those differently.
- `ibis_core::convert::conform_to`: Arrow data matched to a table's columns by name and cast to its types.
- Conformance: 25 of Ibis's tests of the connection API (`test_client`, and the two of that kind in `test_struct` and `test_map`) run as procedures against each backend (`crates/ibis-conformance/src/client.rs`).
- Conformance: `test_array`, `test_struct` and `test_map` ported, 311 cases. `conformance/memtables.json` takes any Ibis type for a column and gained Ibis's `array_types`, `struct` and `map` tables.

- ibis-polars: arrays, structs and maps. An array is a Polars list, a struct a struct, a map a list of `{key, value}` structs; tables, literals and results carry all three. Operations Polars has an expression for become that expression; the others (`zip`, map lookups and merges, `index`, `remove`, `unique`, `union`, `intersect`, `flatten`, `modes`, repeat) are computed row by row in Rust, so they work on columns as well as literals. `map` and `filter` run inside `list.eval`; a function that reads a column there is `Unsupported`. `unnest` (in a select, inside an expression, or `Table::unnest`) explodes the frame; `collect` gathers a group into a list. This supersedes "Polars does not support them yet" above.
- ibis-polars: views (`create_view`, `drop_view`): a named query, planned again each time it is read.
- `Backend::create_table_with(name, schema, data, CreateOptions { overwrite, temp })`: a table from a schema and data, failing on an existing table unless `overwrite`, temporary with `temp` (not on Trino, Spark, Impala or ClickHouse). `Backend::create_table_from(name, query, options)` creates one from a query, which may read the table it replaces.
- A `Field` with `nullable: false` is created `NOT NULL`, and `Backend::table` reports it on DuckDB, SQLite, Postgres, MySQL, ClickHouse and Trino.
- `Backend::upsert(name, data, on)` and `upsert_from(name, query, on)`: `MERGE INTO` on DuckDB and Postgres, `Unsupported` elsewhere (as in Ibis).
- `Backend::sql(query)`: a query in the engine's own SQL as a table expression (`RelOp::Sql`, `Table::sql`), on every backend.
- ibis-polars: `Backend::sql`, through Polars's own SQL (`polars-sql`, the `sql` feature of the `polars` crate; a new dependency, with `sqlparser` behind it). The query sees the connection's tables and views.
- `ibis_sql::ddl`: `create_table`, `create_table_statement`, `upsert_from`, `sql`; `SqlClient::after_create`. `Dialect` hooks `column_type`, `temporary`, `create_table_suffix`, `merge`.
- `ibis_core::scratch_name`: a name for a table or view the library creates and drops again.
- Conformance: 18 more of Ibis's client tests (create options, temporary tables, nullability, `sql`, upsert, inserts by column name, a table replaced by a query on itself): 43 in all.
- `Table::alias(name)` and `Backend::sql_over(table, query)` (Ibis `t.alias(name)` and `t.sql(query)`): SQL in the engine's dialect that reads the aliased expressions of `table` as tables of those names. `RelOp::Alias`; `RelOp::Sql` carries the views it reads, compiled as common table expressions; `Table::sql_over` and `Table::aliases` for backends. On every backend, Polars included.
- `Table::as_scalar()` (Ibis `t.as_scalar()`): a one-column table as a value.
- `SqlClient::query_schema`: how a backend reads the columns of a query's result. The default is the short-lived view `sql` already used; ibis-trino runs the query for no rows instead, so a query with an unnamed column works there.
- `ibis_sql::sql_query`: the text of a SQL query after the common table expressions it reads.
- Reductions: `mode`, `median`, `approx_median`, `approx_nunique`, `kurtosis` and `kurtosis_pop`, `bit_and`, `bit_or`, `bit_xor`, `arbitrary`, and `first`/`last` as reductions (the first and last value that is not NULL). Each backend has the ones its engine has, as in Ibis; the rest are `Unsupported`. `approx_nunique` counts exactly on Postgres, SQLite and MySQL, which have no approximate count, and on Polars. ibis-sqlite registers aggregates for `mode`, the bitwise three, `first` and `last`. ibis-polars enables the `mode`, `moment` and `bitwise` features of `polars`.
- Reductions of more than one argument: `argmin(key)` and `argmax(key)`, `cov(other)` and `cov_pop(other)`, `corr(other)` and `corr_pop(other)`. `AggOptions` (further arguments, an order, `distinct`, `include_null`) is what a reduction carries besides its argument and condition; `Dialect::reduction` is where a dialect writes one. `argmin`/`argmax` everywhere but MySQL and Impala; covariance and correlation on DuckDB, Postgres, Trino, Spark, ClickHouse and Polars (each engine names the sample's correlation, the population's, or both: the other is `Unsupported`, as in Ibis). ibis-polars enables the `cov` feature of `polars`.
- Quantiles: `quantile(q)`, `quantiles([q, ...])` (an array of them), `approx_quantile(q)`, `approx_quantiles([...])` (`AggFunc::Quantile`, `AggFunc::ApproxQuantile`). Of numbers a quantile is interpolated and a float; of text and dates it is one of the values. Exact on DuckDB, Postgres, ClickHouse, Spark and Polars (one fraction at a time there); approximate on those and Trino (Postgres and Polars answer exactly). Not on SQLite, MySQL or Impala.
- Conformance: 10 more ids of `test_aggregation` (`test_quantile`, `test_string_quantile[quantile]`, `test_date_quantile` with goldens; `test_approx_quantile` as procedures). Memtable `approx_quantile_x`.
- `group_concat(sep)` (`AggFunc::GroupConcat`): the strings of a group joined into one, NULL for none. `Expr::order_by(keys)` gives a reduction the order it reads its rows in (Ibis `order_by=`): used by `group_concat` here, and by `first`, `last` and `collect` once they take one. `STRING_AGG` on DuckDB and Postgres, `GROUP_CONCAT` on SQLite, MySQL and Impala, joined arrays on Trino, Spark and ClickHouse, `str.join` on Polars. Ordered on DuckDB, Postgres, MySQL, Trino and Polars. Over a window everywhere but Polars, Impala and MySQL (ClickHouse, Trino and Spark included, which Ibis marks).
- Conformance: 9 more ids of `test_aggregation` (`test_group_concat`, `test_group_concat_ordered`, `test_group_concat_over_window`). Memtable `group_concat_window`.
- `ExprCompiler::standard_reduction`, `by_key`, `percentile`, `fractions`, `agg_order`, `string_agg` and `unordered`, for a dialect's `reduction`.
- Conformance: 26 more ids of `test_aggregation` (`argmin`/`argmax` 16, `test_corr_cov` 10).
- `ExprCompiler::call_agg`, for a dialect's `agg`.
- `Table::value_counts(expr)` and `Table::topk(expr, k)` (Ibis `col.value_counts()`, `col.topk(k)`), with Ibis's column names (`<name>_count`).
- Conformance: 13 more ids of `test_aggregation` (a filter before a grouping, a `CASE` inside a reduction, grouping by an expression and by a constant, `value_counts`, `topk`, the sum of no rows, default names of unnamed reductions), passing on all nine backends. Memtables `grouped_case` and `project_names`.
- Conformance: 27 more ids of `test_aggregation`: 20 cases with goldens (`mode`, `notany`, `notall`, kurtosis, `approx_nunique`, the bitwise reductions, the medians) and 7 procedures for the reductions whose answer the engine chooses (`first`, `last`, `arbitrary`, `approx_median`; `crates/ibis-conformance/src/reductions.rs`). The Python-UDF ids are marked not applicable; `conformance/not_applicable.json` now also takes the id of one parameter (`module::test[param]`).
- Conformance: `test_dot_sql` ported, 15 tests as procedures (`crates/ibis-conformance/src/dot_sql.rs`), passing on all nine backends (one expected failure on Polars, as in Ibis). Its 44 `transpile` ids and the repr test are marked not applicable: ibis-rs passes SQL on as written and has no translator between dialects.
- `Expr::include_null()` and `Expr::distinct()` on a reduction (Ibis `include_null=True`, `distinct=True`), and `Expr::order_by` is now read by `first`, `last` and `collect`: `col("x").collect().distinct().order_by([col("x").desc()])`. DuckDB, Postgres, Trino and Polars take all three; SQLite takes an order and NULLs for `first`/`last` (new registered aggregates `ibis_first_null`, `ibis_last_null`); Spark takes NULLs for `first`/`last` and `distinct` for `collect`; ClickHouse takes `distinct` for `collect`. The rest is `Unsupported`, as in Ibis.
- `nunique()` (Ibis `t.nunique()`, `AggFunc::CountDistinctStar`): the number of distinct rows, with `filter_where` for a condition. `Dialect::count_distinct_rows` is the hook; not on SQLite.
- An ordered `first` or `last` in a projection, or under `.over(w)`, is `FIRST_VALUE`/`LAST_VALUE` over a window in that order (after the window's own), as Ibis compiles it.
- `ExprCompiler::ordered`: `[DISTINCT ]arg[ ORDER BY ...]`, for a dialect's `reduction`.
- Conformance: the last 36 ids of `test_aggregation`, all procedures (`test_first_last` with NULLs kept, `test_first_last_ordered`, `test_first_last_ordered_in_mutate`, `test_collect`, `test_count_distinct_star`, `test_corr_cov[*-corr_samp]`). Beyond Ibis's marks: Postgres keeps NULLs in `first`/`last`, SQLite reads them in an order, Spark keeps NULLs under a condition, MySQL counts distinct rows under a condition, Polars does an ordered `first` in a projection.

### Changed
- **Breaking for backend implementations:** `Backend::create_table_with` is the required method; `create_table` and `create_empty_table` are provided on top of it (and still replace an existing table). The eight SQL backends get it from `sql_client_methods!`.
- `create_table` now creates a `NOT NULL` column for an Arrow field that is not nullable. `ibis_sql::create_table_sql` writes `NOT NULL` and the dialect's storage clause.
- The schema of every relation derived from a table is nullable in all its fields: `NOT NULL` describes a stored column, not what a query returns.
- MySQL and Postgres `table` find a temporary table (MySQL through `SHOW COLUMNS`, Postgres in the connection's temporary schema).
- `Expr::contains` takes an expression or a plain value (`contains("a")`, `contains(1)`, `contains(col("k"))`) instead of a `&str`, and means substring, element or key by the type it is called on. `length` and `repeat` likewise serve strings, arrays and maps. Existing calls compile unchanged.
- `Dialect::default_type_name` returns `Unsupported` for nested types; `Dialect::type_name` is where an engine names them.
- Conformance: backends run side by side, one thread each, for loading fixtures, the Ibis cases and TPC-H. Outcomes are reported in the same order as before. The runner does the Ibis cases and TPC-H at the same time.
- A `Table` that is only a SQL query (`con.sql(q)` with nothing built on it) compiles to the query itself, not `SELECT * FROM (q)`: its `ORDER BY` holds on engines that drop the order of a subquery.
- **Breaking:** `Expr::Agg` has an `options: AggOptions` field; `AggFunc` has nine more variants (`ArgMin`, `ArgMax`, `CovSamp`, `CovPop`, `CorrSamp`, `CorrPop`, `Quantile`, `ApproxQuantile`, `GroupConcat`).
- ibis-mysql sets `group_concat_max_len` to its maximum on the connection: at the default, `GROUP_CONCAT` cuts its result at 1,024 bytes with a warning and no error.
- **Breaking:** `Expr::first()` and `Expr::last()` are reductions (`AggFunc::First`, `AggFunc::Last`), as in Ibis. `first().over(w)` is still `FIRST_VALUE` over the window, so windowed uses are unchanged. `AggFunc` has twelve new variants, which a `match` on it outside this repository must cover.
- `Table::order_by` refuses a column bound to another table (`agg.order_by([t.col("year")])`), as Ibis does, instead of leaving the error to the backend.
- ibis-sql: a group key that reads no column (`group_by([lit("a").name("key")])`) is written as its position in the select list. Postgres refused `GROUP BY 'a'`.
- `filter_where` on `notany()` and `notall()` restricts the reduction they negate (it did nothing before).
- **Breaking:** `RelOp::Sql` has a `views` field. `ibis_sql::ddl::sql` takes the table the query is run over (`None` for `Backend::sql`).
- ibis-sqlite: a view column that is an expression has no declared type; `table` (and so `sql`) now gives it the type of its value in the first row, as Ibis does, instead of string.
- CI runs the suite once instead of twice: `mise run unit` (new: unit tests and doctests, no containers) and then `mise run site`, whose conformance run fails the job on any failing case. Images are pulled while the code compiles.
- **Breaking:** `AggFunc::CountDistinctStar` is a new variant.
- ibis-sqlite, ibis-mysql and ibis-impala report `collect` as `Unsupported` instead of sending `ARRAY_AGG` to an engine that has no arrays.
- ibis-postgres: `first`, `last` and `arbitrary` of a boolean column no longer cast it to an integer (the cast is for the sums and bitwise reductions).

## [ibis-sql 0.1.3, ibis-hiveserver2 0.1.0, ibis-spark 0.1.0, ibis-impala 0.1.0, ibis-rs 0.1.4] - 2026-11-16

### Added
- `ibis-hiveserver2`: a HiveServer2 client with no dependencies. The Thrift binary protocol and the part of `TCLIService` a query needs are written by hand; it speaks to a plain socket with or without the SASL `PLAIN` handshake (no TLS, Kerberos or HTTP transport). `examples/hs2.rs` runs statements from the command line.
- `ibis-spark`: Spark SQL backend, over the Spark Thrift server. Floats are written with an exponent (a bare `1.5` is a decimal), division by zero follows IEEE 754 whatever the ANSI setting, zone-less timestamps are `TIMESTAMP_NTZ`, and POSIX classes in a literal regular expression (`[[:digit:]]`) are rewritten for Java's regex engine. Passes all 551 Ibis cases and the 22 TPC-H queries.
- `ibis-impala`: Impala backend. Impala has no `FILTER (WHERE ...)`, `INTERSECT ALL`, `EXCEPT ALL` or `NTH_VALUE`, refuses `OFFSET` without `ORDER BY`, and its `NTILE` sizes buckets its own way; all but `NTH_VALUE` are rewritten. `create_table` sends the rows as one string per statement, split and cast on the server. Passes 548 of the 551 Ibis cases (the other 3 are marked for Impala in Ibis itself) and the 22 TPC-H queries.
- `ibis-rs`: `spark` and `impala` features; `spark://` and `impala://` URLs in `connect`.
- `ibis-sql`: `Dialect::offset_needs_order_by` (a query that skips rows without ordering them is ordered by its first column) and `strftime_to_java_format`.
- `compose.yaml`: Spark 4.3.0 (Thrift server) and Impala 4.5.3 (metastore, statestore, catalog, daemon) services.

### Fixed
- CI: the step that puts the prebuilt `libduckdb.so` on the loader path looked where the library is only after a fresh download. With a restored build cache it found nothing and the site build could not start (the run after the one that introduced it).

### Changed
- `ibis-sql`: the `INTERSECT ALL` / `EXCEPT ALL` emulation numbers rows with an `ORDER BY` (Impala will not number them without one) and quotes its helper column the dialect's way.

## [ibis-sql 0.1.2, ibis-duckdb 0.1.1, ibis-mysql 0.1.0, ibis-trino 0.1.0, ibis-rs 0.1.3] - 2026-11-16

### Added
- `ibis-mysql`: MySQL backend (`mysql` crate, text protocol). MySQL has no boolean type, NaN or infinity, `FILTER (WHERE ...)`, `NULLS FIRST`/`LAST` or `FULL OUTER JOIN`, and its `CAST` has its own type names; each is rewritten. Passes 531 of the 551 Ibis cases (the other 20 need NaN or infinity) and the 22 TPC-H queries.
- `ibis-trino`: Trino backend over the HTTP protocol. Floats are written with an exponent (a bare `1.5` is a decimal there), and `create_table` sends rows as one JSON array per column. Passes all 551 Ibis cases and the 22 TPC-H queries.
- `ibis-rs`: `mysql` and `trino` features; `mysql://`, `trino://` and `trinos://` URLs in `connect`.
- `ibis-sql`: `Dialect::cast_type_name` (type names inside `CAST`), `Dialect::sort_key` (how an `ORDER BY` key and its NULL placement are spelled), `Dialect::supports_agg_filter` (without it the condition moves into the argument as a `CASE`), `Dialect::supports_full_outer_join` (without it the join becomes a left join plus the unmatched right rows) and `strftime_to_date_format`.
- `compose.yaml`: MySQL 8.4 and Trino 484 services.
- `ibis-duckdb`: `bundled` is now a feature, on by default. Without it the crate links a prebuilt DuckDB (downloaded by the `duckdb` crate, or `DUCKDB_LIB_DIR`) instead of compiling it. `ibis-rs`: `duckdb-linked` feature for the same; `duckdb` still compiles from source.

### Changed
- `ibis-sql`: a literal `nth` index compiles to a literal (`NTH_VALUE(x, 4)`, not `NTH_VALUE(x, 3 + 1)`).
- CI links the prebuilt DuckDB and caches rustc outputs with sccache; the mise cargo tasks take their flags from `IBIS_CARGO_FLAGS` (default `--workspace --all-features`).
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
