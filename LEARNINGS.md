# Learnings

Hard-won knowledge. Each entry: what happened, why it was surprising, how to avoid it.

## DataFusion's Unparser is a plan printer, not a semantic translator
- **What:** Unparsing DataFusion plans to Postgres/DuckDB/SQLite passed ~90% of a 62-case
  spike, but failures included silent wrong answers: optimized plans print a pushed-down
  sort fetch as `LIMIT skip+fetch OFFSET skip`; `regexp_like` on DuckDB becomes `~`, which
  is a *full* match there. Nested aggregates get dropped in raw plans.
- **Why surprising:** It round-trips through DataFusion fine in most cases, so it looks done.
- **Avoid:** Don't use it as the semantics layer. Own the IR; verify every backend against
  goldens (see docs/design.md). Details: spikes/unparser/FINDINGS.md.

## Engines disagree on "simple" things
- **What:** CAST(float AS BIGINT) truncates (DataFusion, SQLite) vs rounds (Postgres, DuckDB);
  Postgres sorts strings with locale collation; SQLite LIKE ignores ASCII case.
- **Avoid:** Every IR op needs a defined meaning (Ibis's) and per-dialect SQL that produces it.

## DataFusion DataFrame API: aggregates aren't windows
- **What:** `sum(x).partition_by(..)` errors; `avg(x).order_by(..)` silently becomes an
  ordered-set aggregate, not a window.
- **Avoid:** `Expr::from(WindowFunction::new(avg_udaf(), vec![x])).order_by(..).build()`.

## Arrow version skew between drivers
- **What:** duckdb-rs 1.20000 links arrow 60; DataFusion 55 links arrow 59. Their
  `RecordBatch` types are different Rust types.
- **Avoid:** Pin one arrow in `[workspace.dependencies]`; drivers on another version cross
  via the Arrow C Data Interface (FFI), never via Rust types.

## sqlite3 CLI in CSV mode ignores `-nullvalue`
- **What:** NULLs print as empty fields; a row whose only column is NULL prints as a blank
  line that CSV readers skip.
- **Avoid:** Don't diff through CLIs; use drivers. (Only mattered for the spike.)

## Postgres NOTICEs go to stderr
- **What:** `DROP TABLE IF EXISTS` emits a NOTICE that broke "stderr non-empty = error".
- **Avoid:** `PGOPTIONS='-c client_min_messages=warning'`.

## Bundled SQLite has no math functions
- **What:** rusqlite's `bundled` SQLite lacks `CEIL`, `FLOOR`, `LN`, `POWER`, ... (not built
  with `SQLITE_ENABLE_MATH_FUNCTIONS`).
- **Avoid:** The SQLite plugin registers Rust polyfills (`crates/ibis-sqlite/src/polyfill.rs`)
  so the default SQL runs unchanged. Prefer polyfills over dialect rewrites when the
  *meaning* matches and only the function is missing.

## Compile regexes once per statement, not per row
- **What:** The SQLite regex polyfills called `Regex::new` per row; the SQLite conformance
  run took 122s.
- **Avoid:** `Context::get_or_create_aux` caches the compiled pattern for the statement.

## Debug-built DuckDB/SQLite are painfully slow
- **What:** Bundled C/C++ engines compiled at opt-level 0 dominate test time.
- **Avoid:** `[profile.dev.package."*"] opt-level = 2` in the workspace Cargo.toml. One slow
  rebuild, then fast dev loops; our own crates still compile in debug.

## Aggregate FILTER must attach to the call, not to a wrapped expression
- **What:** SQLite's `any` is `(MAX(x) <> 0)`; appending `FILTER (WHERE ...)` after the
  dialect output produced `(MAX(x) <> 0) FILTER (...)`, a syntax error.
- **Avoid:** Dialect `agg` hooks receive the filter clause and place it after the call.

## SQLite: declared types vs CAST types
- **What:** Declaring columns `TIMESTAMP`/`BOOLEAN` round-trips Ibis types via
  `pragma_table_info`, but `CAST(x AS TIMESTAMP)` uses NUMERIC affinity and turns
  `'2010-01-01 00:00:00'` into `2010`.
- **Avoid:** Declared names for DDL; affinity names (`INTEGER`/`REAL`/`TEXT`) or
  `DATE()`/`DATETIME()` in CAST overrides.

## SQLite LIKE ignores ASCII case
- **Avoid:** Compile Ibis `like` to `GLOB` (case-sensitive) with a pattern translation;
  `ilike` to `LOWER(x) LIKE LOWER(p)`.

## `cargo new` inside a git repo creates a nested repo
- **What:** `cargo new` in a directory without a parent `.git` (at the time) runs `git init`;
  later `git add -A` at the root fails: "does not have a commit checked out".
- **Avoid:** `cargo new --vcs none` for crates/spikes inside a repo.

## Comparing values alone misses type divergences
- **What:** 108 cases passed on values while `lit(1)` was typed `int64` (Ibis: `int8`),
  `smallint + 1` was `int64` (Ibis: `int32`) and `//` kept the operand type (Ibis: `int64`).
- **Why surprising:** Arrow conversion coerces results to the inferred schema, so a wrong
  inferred type still produces "equal" values.
- **Avoid:** The `schema` pseudo-backend compares inferred names and dtypes to the ones
  Ibis recorded in `conformance/golden/index.json`. Keep it green for every new op.

## Nesting a sorted SELECT silently drops the order
- **What:** `t.order_by(x).filter(...)` compiled to `SELECT ... FROM (SELECT ... ORDER BY x)`.
  No engine guarantees that an inner ORDER BY survives the outer query.
- **Avoid:** Ordering is IR state on `Select` (`order`, `passthrough`), hoisted to the
  outermost query by `wrap()`. In `Aggregate`, wrap first and clear the order after, or
  order + limit under a group-by breaks.

## An ordered window without a frame is a running aggregate in SQL
- **What:** `SUM(x) OVER (ORDER BY y)` defaults to `RANGE UNBOUNDED PRECEDING .. CURRENT ROW`.
  Ibis means the whole partition.
- **Avoid:** Emit `ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING` explicitly;
  ranking functions and lag/lead take no frame.

## Dialect wrappers around aggregates break OVER as well as FILTER
- **What:** Postgres casts integer sums (`CAST(SUM(x) AS BIGINT)`); appending `OVER (...)`
  to that is a syntax error. Same shape as the earlier FILTER learning.
- **Avoid:** `Dialect::agg` takes one `tail` (FILTER + OVER) and places it after the call.

## A table joined to a table derived from itself
- **What:** In `a.join(a.filter(..))` the right side's deref map also contains `a`'s
  columns, so `a.col("x")` resolved to the right side.
- **Avoid:** Scope lookup matches the exact relation id first and only then falls back to
  derived relations (`RelCompiler::field`).

## Division by zero: three engines, three answers
- **What:** DuckDB gives NULL or inf depending on settings, Postgres raises, SQLite gives
  NULL. Ibis's DuckDB results are IEEE (inf, NaN). Python Ibis itself raises on Postgres.
- **Avoid:** Postgres and SQLite dialects emulate IEEE with CASE. SQLite has no NaN at all
  (it stores NULL), so NaN cases are `notimpl(&["sqlite"])`, as in Ibis.

## Float literals are not floats in DuckDB or Postgres
- **What:** DuckDB reads `5.556 * 57.29577951308232` as DECIMAL arithmetic and overflows;
  Postgres reads float literals as NUMERIC.
- **Avoid:** Use native functions (`DEGREES`, `RADIANS`) rather than literal formulas;
  keep formulas for SQLite only. Typed literals are a TODO.

## Postgres specifics
- **What:** No BOOLEAN -> BIGINT cast (cast to INTEGER first). The `postgres` crate's
  `Display` for an error is just "db error".
- **Avoid:** `pg_err` formats the server's message, detail and hint. Run the cluster with
  `--no-locale`, or string ordering differs from DuckDB and the goldens.

## SQLite specifics
- **What:** No `INTERSECT ALL` / `EXCEPT ALL`; OFFSET needs a LIMIT (`LIMIT -1 OFFSET n`);
  scalar `MIN(a, b)` / `MAX(a, b)` return NULL if any argument is NULL.
- **Avoid:** Emulate the ALL set ops with `ROW_NUMBER() OVER (PARTITION BY all columns)`.

## Python Ibis 12 pitfalls when generating goldens
- **What:** `create_table(name, obj=...)` emits a malformed DROP with current sqlglot on
  SQLite and Postgres. `to_pyarrow()` fails on some Postgres results ("Rescaling Decimal
  value would cause data loss") and on TPC-H decimals.
- **Avoid:** `gen_goldens.py` loads fixtures with native drivers and reads results with
  `to_pandas()`. Some "error" entries in the Python Ibis cross-check are these conversion
  failures, not wrong answers.

## Not every Ibis test has a usable DuckDB golden
- **What:** A few Ibis cases have no single right answer on DuckDB (epoch seconds as a
  different type per backend, `ns` truncation).
- **Avoid:** Leave them out rather than encode a DuckDB quirk as the spec. Notes at the top
  of each `conformance/cases/test_*.py` list the departures from the Ibis source.

## `pytest --collect-only -m duckdb` misses tests
- **What:** Tests that take no backend fixture have no backend marker, so filtering by
  marker dropped real ids.
- **Avoid:** `check_ids.py` collects everything and normalizes the backend out of the id.

## TPC-H goldens are per backend
- **What:** SQLite stores decimals as floats, so its results differ in type and in the last
  digits from DuckDB's and Postgres's.
- **Avoid:** `conformance/golden/tpch/<backend>/`. Build the SQLite database with Python's
  `sqlite3` and declared column types (DuckDB's sqlite writer declares dates VARCHAR), and
  create Postgres databases with `psql` (`CREATE DATABASE` cannot run in the transaction
  DuckDB's `postgres_execute` opens).

## What retriggers the DuckDB build
- **What:** Bundled DuckDB takes 10-13 minutes to compile, and compiles again when a
  dependency's feature set changes (adding the `postgres` crate did it), for clippy, and
  for `--release`. With all 16 cores it was killed for memory.
- **Avoid:** `CARGO_BUILD_JOBS=4`. Batch dependency changes. Expect the first
  `mise run lint` and `mise run bench` after such a change to be slow.

## mise sets the test environment
- **What:** `IBIS_TESTING_POSTGRES_URL` and `IBIS_TESTING_DATA` come from `mise.toml`;
  a bare `cargo test` fails on the Postgres target.
- **Avoid:** `mise run test`, or `mise exec -- cargo test ...` for a single test.
