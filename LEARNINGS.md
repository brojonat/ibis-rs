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
