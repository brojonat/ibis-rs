# TODO

North star: Ibis parity, measured by ported Ibis backend test ids passing per backend.

## Now
- [x] Spike: DataFusion Unparser across Postgres/DuckDB/SQLite (spikes/unparser)
- [x] Design doc, workspace scaffold, mise front door
- [x] ibis-core: datatypes, schema, IR, Table/Expr API, type inference, Backend trait
- [x] ibis-sql: Select-merging compiler + Dialect trait
- [x] ibis-duckdb backend
- [x] ibis-sqlite backend (polyfills + results coerced to Ibis schema)
- [x] Conformance: cases.py + gen_goldens.py + Rust runner; 108 cases from test_string / test_numeric / test_aggregation pass on duckdb + sqlite
- [ ] Verify ported ids match `pytest --collect-only` exactly (esp. stacked parametrize like `test_reduction_ops[no_cond-count]`)

## Next
- [ ] Parity report: collect all Ibis test ids (`pytest --collect-only` on ibis/backends/tests) vs ported ids
- [ ] Port the rest of the slice's test functions: test_string (translate, split, join, find_in_set, capitalize...), test_substring slices, test_numeric literals/round/clip, test_aggregation grouped/mode/argmin
- [ ] Port more modules: test_generic, test_temporal, test_window, test_join, test_set_ops, test_conditionals
- [ ] Rel-level conformance cases (joins, set ops, windows, order+limit) — current slice is mostly column exprs and reductions
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
