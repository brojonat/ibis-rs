# ibis-rs design

## North star

**Parity with [Ibis](https://ibis-project.org), measured by Ibis's own backend
test suite.** Ibis's tests live in `ibis/backends/tests/test_*.py` and are
parametrized with stable ids (`test_string[upper]`, `test_substring[substr]`).
Every conformance case in this repo carries the id of the Ibis test it ports,
and its expected result is produced by running *the real Python Ibis
expression* on DuckDB. "How close are we to Ibis?" is therefore a number:
ported-and-passing ids / total Ibis ids, per backend.

## Why this exists

Ibis gives Python one lazy dataframe API that compiles (via sqlglot) to 20+
backends. Rust has the parts (DataFusion, sqlparser-rs, Polars, ADBC, PRQL,
`sqlglot-rust`) but nothing that composes them into a portable dataframe API
with pluggable backends and an explicit semantics contract.

## What the spike taught us

`spikes/unparser/` built 62 queries with DataFusion's DataFrame API, unparsed
them with DataFusion's `Unparser` for Postgres/DuckDB/SQLite, ran them on the
real engines and diffed against DataFusion's own answer (details in
`spikes/unparser/FINDINGS.md`). Postgres and DuckDB pass ~90% for free, but:

1. **Unparser bugs**: nested aggregates dropped, window-then-filter emits invalid
   SQL, optimized plans emit `LIMIT skip+fetch` (silent wrong answer).
2. **No function translation**: DataFusion names are printed verbatim.
3. **Real semantic differences** no SQL printer can fix: float→int cast
   (truncate vs round), string collation in ORDER BY, case-insensitive LIKE on
   SQLite, full-match `~` on DuckDB.

Point 3 is the important one. The hard part of Ibis is not printing SQL; it's
owning the *meaning* of each operation and making every backend honor it.

## Architecture

```
your code ──► Table / Expr API ──► IR (plain Rust enums) ──► backend plugin ──► Arrow RecordBatches
               (ibis-core)           (ibis-core)              compile + execute
```

### IR (intermediate representation)

The IR is a tree of plain enums: `Rel` (relational nodes: `Table`, `Project`,
`Filter`, `Aggregate`, `Sort`, `Limit`, `Distinct`, `Join`, `SetOp`) and `Expr`
(columns, literals, casts, binary/unary ops, scalar functions, aggregates,
windows, CASE). Building an expression never touches a database. Backends walk
the tree.

**We own the IR; we do not reuse DataFusion's `LogicalPlan`.** Reusing it would
import DataFusion's function semantics, type coercion, optimizer rewrites and
release churn (v55 at time of writing) into every backend. The IR *is* the
semantics contract: each node documents what it means (in Ibis terms), and the
conformance suite checks each backend honors it. DataFusion is welcome as a
*backend* that our IR lowers into.

Ops are closed enums, not trait objects: adding an op is a deliberate change
that every compiler must consider, and `match` exhaustiveness helps find them.
UDFs, when they come, get their own escape-hatch variant.

### Semantics: Ibis is the spec

Where engines disagree, we do what Ibis does, because Ibis's results are the
goldens. Examples:

- `int / int` is true division returning `float64` (Ibis `__truediv__`).
- `row_number()` is **0-based** in Ibis; compilers emit `ROW_NUMBER() - 1`.
- `substr(start, length)` uses 0-based `start` in Ibis.
- `count()` on a table counts rows; `col.count()` counts non-nulls.

### Backend plugins

A backend is a crate that implements `ibis_core::Backend`:

```rust
pub trait Backend: Send + Sync {
    fn name(&self) -> &str;
    fn compile(&self, t: &Table) -> Result<String>;          // inspectable, like ibis.to_sql
    fn execute(&self, t: &Table) -> Result<Vec<RecordBatch>>;
    fn table(&self, name: &str) -> Result<Table>;            // bind a table, with schema
    fn create_table(&self, name: &str, data: &[RecordBatch]) -> Result<()>;
    fn list_tables(&self) -> Result<Vec<String>>;
}
```

SQL backends share `ibis-sql`'s compiler and implement only a `Dialect` trait:
identifier quoting, type names, and per-op overrides. A dialect override
handles the ops it cares about and delegates the rest to the default
(`ibis_sql::default_scalar`, ...), the Rust equivalent of Ibis backends
subclassing the sqlglot compiler and overriding `visit_*` methods.

Non-SQL backends (DataFusion `LogicalPlan`, Polars `LazyFrame`) implement
`Backend` directly. The Polars backend is a useful test that the IR isn't
secretly SQL-shaped.

**Plugins are compile-time crates, selected with cargo features.** Rust has no
stable ABI, so runtime-loaded plugins in the style of pluggy aren't worth it.
The registry of plugins is an explicit value, never global state. If dynamic
loading is ever needed, ADBC drivers are C-ABI shared libraries.

### SQL generation

`ibis-sql` lowers `Rel` to a small `Select` struct (projections, from, where,
group by, order by, limit/offset, distinct) and merges an op into the current
`Select` only when that's semantics-preserving; otherwise it wraps the current
query as a derived table (`FROM (...) AS tN`). Correctness first, pretty SQL
later. Each `Select` has exactly one source except joins, so column references
are unqualified outside joins.

### Data in, data out: Arrow

Results are Arrow `RecordBatch`es. Each backend converts its native results
to the *Ibis output schema* the IR computed (e.g. SQLite's 0/1 becomes
`boolean`, text timestamps become `timestamp`), the same way Ibis
post-processes results. Polars users convert via the Arrow C Data Interface.

The workspace pins one arrow version. Drivers that link another version
(DataFusion 55 uses arrow 59; duckdb-rs uses arrow 60) need an FFI hop.

### API shape

Ibis-flavored, with Polars-style ergonomics where Rust requires it:

```rust
let t = con.table("functional_alltypes")?;
let expr = t
    .filter([col("int_col").gt(lit(2))])?
    .group_by(["string_col"])
    .agg([col("double_col").mean().name("avg"), count().name("n")])?
    .order_by([desc("avg")])?
    .limit(10, 0);
println!("{}", con.compile(&expr)?);
let batches = con.execute(&expr)?;
```

- `col("x")` stands in for Ibis's `_.x` and is resolved against the table when
  an op is applied. Schema errors surface at that point (`?`), like Ibis
  raising at expression construction.
- Methods are flat on `Expr` (`.upper()`, `.substr()`, `.mean()`), matching
  Ibis, so that porting Ibis tests is a mechanical translation.
- Arithmetic uses operator overloading. Comparisons are methods (`.gt()`,
  `.eq()`) because Rust's comparison traits must return `bool`.
- The API is synchronous, like Ibis. Async-native drivers (Postgres) block
  internally. Revisit if it hurts.

## Conformance suite

```
conformance/
  cases.py          # Ibis expressions, keyed by Ibis test id (ported from ibis/backends/tests)
  gen_goldens.py    # runs cases.py on Python Ibis + DuckDB, writes golden/*.json
  golden/*.json     # expected results: columns, ibis dtypes, rows (human-diffable)
crates/ibis-conformance/
  src/cases.rs      # the same ids, written against the Rust API, with notimpl markers
  src/main.rs       # runs every case on every backend: JSONL on stdout, matrix on stderr
```

- Column-expression cases run as `t.select("id", tmp=expr)` (or
  `t.aggregate(tmp=expr)` for reductions) and compare as a multiset of rows,
  so we don't depend on backend row order.
- Floats compare with a relative tolerance; temporal values compare as
  canonical strings.
- `notimpl: &["sqlite"]` markers mirror Ibis's `pytest.mark.notimpl`. A marked
  case that unexpectedly passes is reported as `xpass` so markers don't rot.
- Fixture data is Ibis's own: `ibis-project/testing-data` parquet files.

## Open questions

- Join API: name-pair equi-joins first; general predicates need column
  references bound to a specific side (Ibis binds fields to table objects).
- Type parity: we compare values today; Ibis output dtypes should match too.
- Collation: Ibis doesn't fully normalize string ordering across backends
  either. Decide per op, guided by what the Ibis tests assert.
- Async API.
