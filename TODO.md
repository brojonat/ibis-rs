# TODO

North star: Ibis parity, measured by ported Ibis backend test ids passing per backend.

## Now
- [x] Spike: DataFusion Unparser across Postgres/DuckDB/SQLite (spikes/unparser)
- [x] Design doc, workspace scaffold, mise front door
- [ ] ibis-core: datatypes, schema, IR, Table/Expr API, type inference, Backend trait
- [ ] ibis-sql: Select-merging compiler + Dialect trait
- [ ] ibis-duckdb backend
- [ ] ibis-sqlite backend (results coerced to Ibis schema)
- [ ] Conformance: cases.py + gen_goldens.py + Rust runner, first slice of test_string / test_numeric / test_aggregation

## Next
- [ ] Parity report: collect all Ibis test ids (`pytest --collect-only` on ibis/backends/tests) vs ported ids
- [ ] Port more: test_generic, test_temporal, test_window, test_join, test_set_ops, test_conditionals
- [ ] Output dtype parity (compare result Arrow types to Ibis dtypes, not just values)
- [ ] ibis-postgres backend (throwaway cluster via scripts like spikes/unparser/scripts/pg.sh)
- [ ] ibis-datafusion backend: IR -> LogicalPlan; arrow 59 vs 60 needs C Data Interface hop
- [ ] ibis-polars backend: IR -> LazyFrame (proves the IR isn't SQL-shaped)
- [ ] Facade crate `ibis` with cargo features + `connect(url, &registry)`
- [ ] Joins with general predicates (side-bound column refs), collision renaming parity
- [ ] Order-preserving wrap: hoist ORDER BY when wrapping a sorted Select

## Later
- [ ] More SQL backends: BigQuery, Snowflake, Trino, ClickHouse (dialect + driver each)
- [ ] Upstream DataFusion unparser bug reports from spikes/unparser/FINDINGS.md
- [ ] Selectors (s.numeric(), s.matches(...)), deferred-style sugar, typed derive layer
- [ ] UDF escape hatch
