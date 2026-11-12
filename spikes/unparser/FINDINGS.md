# Spike: DataFusion 55.2 Unparser as a multi-backend SQL compiler

`mise run run` (raw plans) / `mise run run -- --optimized`. 62 cases built with
the DataFusion DataFrame API, unparsed per dialect, executed on the real engine,
and compared by column name to DataFusion's own result (floats to 6 sig figs).

| target | raw plan ok | optimized plan ok |
|---|---|---|
| datafusion (round-trip) | 61/62 | 61/62 |
| postgres 18 | 56/62 | 55/62 |
| duckdb 1.4 | 55/62 | 54/62 |
| sqlite 3 | 47/62 | 47/62 |

## Unparser bugs (wrong or invalid SQL)
- nested aggregate (agg over agg) drops the inner aggregate entirely (raw plan).
- filter over a window projection (QUALIFY pattern) references the inner table
  qualifier from the outer query: fails on all three engines, both plan modes.
- optimized plans: Sort fetch pushdown is emitted as `LIMIT skip+fetch OFFSET skip`,
  so you get 7 rows, not 5. **Silent wrong answer**, even in a DataFusion round-trip.
- optimized plans: CSE introduces `__common_expr_1` subqueries with broken qualifiers.
- empty projection (`SELECT FROM ...`) for anti joins / count over join (DuckDB, SQLite reject).
- aggregate output column order differs from the plan schema (`SELECT aggs..., groups...`).

## No function-name translation (dialect hooks exist but are mostly unused)
DataFusion names are printed verbatim: `var`, `btrim` (duckdb/sqlite), `regexp_like`,
`starts_with`, `strpos`, `stddev` (sqlite), `median` (postgres), `ILIKE`, `INTERVAL`
and `date_part` on sqlite. `Dialect::scalar_function_to_sql_overrides` is the seam.

## Semantic differences (no printer can fix these; an IR contract must decide)
- CAST(float AS BIGINT): DataFusion/SQLite truncate, Postgres/DuckDB round.
- ORDER BY on strings: Postgres uses locale collation ("Shipped" sorts with "shipped").
- LIKE is case-insensitive on SQLite.
- DuckDB `~` is full-match; optimized `regexp_like` -> `~` returns 0 rows. **Silent.**
- SQLite date_trunc('month') unparses to strftime('%Y-%m') -> '2023-02', not a timestamp.
- round(151.95, 1): 152.0 vs 151.9 (decimal vs binary rounding).

## Harness caveats
- sqlite3 CSV mode ignores -nullvalue; "" is mapped to NULL (fixture has no empty
  strings), and a single-NULL-column row prints as a blank line that the CSV reader
  skips (the sqlite union_distinct DIFF is this artifact).
- anti_join oracle result is empty (every customer has orders), so Postgres "ok" is vacuous.
- DataFusion API footgun: `sum(x).partition_by(..)` is an error and `avg(x).order_by(..)`
  becomes an ordered aggregate, not a window; build `WindowFunction::new(avg_udaf(), ..)`.

## Verdict
Good scaffolding, not a semantic layer. Postgres/DuckDB coverage is ~90% for free,
but the failures include silent wrong answers, and semantic choices (cast rounding,
collation, LIKE case, regex anchoring) need an owning IR + per-backend compilers +
a conformance suite: i.e., what Ibis is.
