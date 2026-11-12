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
