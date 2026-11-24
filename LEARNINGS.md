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

## An unbounded query plan can take the whole machine down
- **What:** The first Polars conformance run used 25 GB; the kernel OOM killer fired and took
  the terminal session (and unrelated processes) with it. The cause was `isin` over a
  subquery translated as one OR per value, thousands deep.
- **Why surprising:** The SQL backends push such plans to an engine that streams. An
  in-process engine materializes whatever plan it is handed, inside the test process.
- **Avoid:** Run new backends under a cap: `prlimit --as=8000000000 timeout 300 target/debug/ibis-conformance --backend polars`.
  The runner prints results only at the end, so to find a crashing case run one test
  function per process (filter by id) and collect.

## Polars is not SQL: where the semantics differ
- **What:** `sum` of no rows is 0 (SQL: NULL); `any`/`all` of no rows are false/true (SQL:
  NULL); `int32 + int32` stays `int32` and overflows (Ibis: `int64`); `%` takes the
  divisor's sign (SQL engines: the dividend's); `rank` gives NULL for NULLs; padding does not
  truncate; a regex that does not match extracts NULL (SQL engines: empty string).
- **Avoid:** Translate each op to Ibis's meaning and cast to the Ibis dtype
  (`crates/ibis-polars/src/expr.rs`). Use max/min of booleans for any/all.

## Polars window functions
- **What:** `over` orders by one direction for all keys, packs the keys into a struct (needs
  the `dtype-struct` feature, and key names must be unique), and has no frames.
- **Avoid:** Alias window keys (`__w0`, `__p0`); compute frames as differences of prefix
  sums, and ranks from "does any key change from the previous row" in window order.

## A cross join plus a filter is cheap in Polars; a join on a constant key is not
- **What:** TPC-H Q9 joins on `(a == b) & (c == d)`. Treated as one opaque predicate it became
  a join on a constant key and ran out of memory. Polars's optimizer turns `cross_join` +
  `filter` into a real join, but cannot see through a constant key.
- **Avoid:** Split AND-ed conditions first, so each equality becomes a join key.

## Polars feature flags are not self-consistent
- **What:** With `default-features = false`, Polars 0.55 fails to compile for several
  feature sets: `dtype-datetime` needs `timezones`, `range` needs `dtype-array`,
  `string_to_integer` needs `dtype-u128`. Multi-key window ordering panics at run time
  without `dtype-struct`.
- **Avoid:** The working set is in the workspace `Cargo.toml`. Add features one at a time
  and expect a 4-minute rebuild each.

## `cargo build -p crate` rebuilds DuckDB
- **What:** Selecting packages with `-p` resolves a different feature set for shared
  dependencies than `--workspace`, so bundled DuckDB (10+ minutes) compiles again.
- **Avoid:** Build and test with `--workspace` (`mise run test`, `mise run lint`).

## mise tasks run under `sh`, which is dash on Ubuntu
- **What:** `2> >(tee ...)` (process substitution) in `mise run conformance` worked locally,
  where `sh` is bash, and failed in CI with "Syntax error: redirection unexpected".
- **Avoid:** Keep task scripts POSIX, or set `shell = "bash -c"` on the task that needs bash.

## A memory cap on `cargo test` also caps the doctest linker
- **What:** `ulimit -v 12000000; cargo test` failed with "ld.lld: failed to open ...: Cannot
  allocate memory". Doctests are compiled and linked when they run, inside the limit, and
  linking against DuckDB and Polars maps more than 12 GB of address space.
- **Avoid:** Build first (`--no-run`), run `--lib --bins --tests --examples` under the cap,
  and run `--doc` outside it. That is what `mise run test` does.

## Database images pass a naive healthcheck before they are ready
- **What:** The Postgres and ClickHouse images start a temporary server to run their init
  scripts, then restart. A healthcheck over the unix socket or localhost passes during that
  phase, `docker compose up --wait` returns, and the first connection is refused.
- **Avoid:** Check the way a client connects: `pg_isready -h 127.0.0.1` for Postgres, and
  the container's own address (`hostname -i`) for ClickHouse, whose temporary server only
  listens on localhost.

## ClickHouse is closer to standard SQL than its reputation, given the right settings
- **What:** With `join_use_nulls`, `aggregate_functions_null_for_empty`, `cast_keep_nullable`,
  `union_default_mode=DISTINCT` and `prefer_column_name_to_alias` set per request, and every
  type spelled `Nullable(...)`, the default SQL passed 568 of 573 on the first run. What was
  left: `round` on floats rounds half to even; `DATE_TRUNC` stops at seconds
  (`toStartOfMillisecond`); `cume_dist` exists only in lower case while the other window
  functions take either; `x IN (NULL)` is false (Ibis marks that notyet too).
- **Also:** string literals read backslash escapes, so a regex like `\d+` must be written
  `'\\d+'`; function names are case-sensitive except the standard SQL ones.

## pytest ids: the backend is usually the first parameter, not always
- **What:** Ibis's ids look like `test_x[duckdb-param]`, but `test_time_literal_sql[0-duckdb]`
  has it last, and `test_dot_sql` ids contain other backends' names as dialect parameters
  (`[duckdb-postgres]`). Filtering out every id that mentions another backend dropped 42
  real ids.
- **Avoid:** Take the backend from the first parameter, else the last; leave the rest alone
  (conformance/site.py `collect`).

## A new crate can rebuild DuckDB
- **What:** Adding `ibis-clickhouse` to the workspace made bundled DuckDB compile again
  (12 minutes). The cause was not confirmed; the likely one is `ureq`, which the new crate
  uses and which was already in the lockfile, being built with a different feature set.
- **Avoid:** Add a new crate's dependencies once, before iterating on its code, and expect
  one long build.

## A new dependency rebuilt DuckDB again (and why it is cheap to find out first)
- **What:** Adding `ibis-mysql` and `ibis-trino` recompiled bundled DuckDB (11 minutes), as
  `ibis-clickhouse` had. A feature diff showed crates elsewhere in the graph gaining
  features with the `mysql` crate (`crc32fast`, `getrandom`, ...), which is the likely
  trigger; which one DuckDB's build depends on was not pinned down.
- **Avoid:** Before adding a dependency, diff `cargo tree --workspace --all-features -e
  features --prefix none -f '{p} {f}' | sort -u` with and without it (a `git worktree` of
  HEAD gives the "without"). If shared crates change features, add every new dependency
  in one go and start the build before writing the code that uses them. To type-check new
  crates meanwhile, `cargo check -p new-crate --target-dir target/check` does not wait
  for the build lock.

## `target/` reached 182 GB
- **What:** Full debug info for Polars, DuckDB and arrow, linked into every test binary and
  kept for every feature combination ever built.
- **Avoid:** The workspace sets `debug = "line-tables-only"` for our crates and none for
  dependencies; a clean rebuild is 9 GB. `cargo clean` is the only thing that removes
  stale feature combinations.

## Trino: a big `VALUES` list is the wrong way to load data
- **What:** `INSERT ... VALUES` with 4,000 rows per statement loaded the fixtures at 1,300
  rows a second, and the third consecutive test run killed the server with
  `OutOfMemoryError` on a 3 GB heap.
- **Why surprising:** The data is 20 MB. Trino plans one expression per cell and keeps every
  finished query and task, with text and plan, for 15 minutes; the plans filled the heap.
- **Avoid:** Send columns, not cells: `SELECT CAST(c0 AS ...) FROM UNNEST(CAST(JSON_PARSE('[...]')
  AS ARRAY(VARCHAR)), ...) AS v (c0, ...)`. One literal per column, and twice as fast.
- **And:** that alone still leaked 100 MB of live heap per test run (600 queries each).
  `docker/trino/config.properties` sets `query.max-history`, `query.min-expire-age` and
  `task.info.max-age` so the server forgets within 30 seconds; live heap then stays near
  450 MB (it still creeps about 10 MB a run; cause not found). Measure with
  `docker exec <container> jcmd 1 GC.run` then `GC.heap_info`: "used" before a collection
  says nothing.
- **Also:** the image sizes its heap at 80% of the memory it sees, so a container memory
  limit gets it OOM-killed; `docker/trino/jvm.config` sets a fixed 2 GB heap instead. A
  bind-mounted file edited with `sed -i` is a new inode the running container does not
  see: `docker compose up --force-recreate`. Without
  the `X-Trino-Client-Capabilities: PARAMETRIC_DATETIME` header, timestamps come back cut to
  milliseconds. `FILTER` is not supported on window aggregates. Identifiers fold to lower
  case, so the harness gives fixture columns their created names back (`bind` in
  `crates/ibis-conformance/src/lib.rs`).

## MySQL differences that needed a hook, not a function override
- **What:** `CAST` only takes `SIGNED`, `UNSIGNED`, `CHAR`, `DOUBLE`, `DECIMAL`, `DATE`,
  `DATETIME` (hence `Dialect::cast_type_name`); there is no `NULLS FIRST`/`LAST`
  (`Dialect::sort_key`), no `FILTER (WHERE ...)`, no `FULL OUTER JOIN`; `NTH_VALUE` takes
  only a literal position; `ROUND` and `CAST(... AS SIGNED)` of a DOUBLE round half to even;
  `1.5` is an exact decimal (write `1.5e0`); `x / 0` is NULL and there is no NaN or infinity.
- **Also:** string comparison follows the column collation, and the default is
  case-insensitive. The test server runs with `utf8mb4_bin` (compose.yaml), the counterpart
  of Postgres's `--no-locale`, and the backend sets the connection collation to match.

## CI: a partly restored cache breaks the DuckDB build
- **What:** The first push that changed `Cargo.lock` failed in `mise run lint` after 90
  seconds: `libduckdb-sys` build script, "unpacking bundled DuckDB sources: Directory not empty".
- **Why surprising:** Nothing in the change touched DuckDB. `Swatinem/rust-cache` restored the
  previous lockfile's cache ("full match: false"), which keeps `target/duckdb-sources/<hash>/`
  in pruned form; the changed dependency graph reran the build script, which will not unpack
  into a directory that exists.
- **Avoid:** CI no longer compiles DuckDB (it links the prebuilt library). If it ever does
  again: `rm -rf target/duckdb-sources` after restoring the cache.

## The `duckdb` crate downloads a prebuilt library when `bundled` is off
- **What:** A cold CI run took 30 minutes, 13 of them compiling DuckDB's C++. Without the
  `bundled` feature, `libduckdb-sys` downloads the pinned `libduckdb.so` into
  `target/duckdb-download/` and links it: lint went from minutes to seconds.
- **Why it took plumbing:** `--all-features` turns on every feature of every workspace
  crate, so no feature can be "off in CI" while tasks pass it; and a dependency edge with
  default features turns `bundled` back on. Hence `default-features = false` on the
  workspace's `ibis-duckdb` entry, a `bundled-duckdb` default feature on the conformance
  crate, and `IBIS_CARGO_FLAGS` for the mise tasks.
- **Avoid:** `cargo test` and `cargo run` put the library on the loader path; a binary run
  directly (`target/debug/ibis-conformance`) needs `LD_LIBRARY_PATH`. `cargo run` does not
  take `--workspace`, so the task cannot simply go through it.
- **And where the library is depends on the cache:** the download lands in
  `target/duckdb-download/` and is copied into the build script's output directory
  (`target/debug/build/libduckdb-sys-*/out/`). `rust-cache` keeps the second and drops the
  first, and a cached build script does not run again. The CI step looked only in
  `duckdb-download`, so it worked on the run that downloaded and set
  `LD_LIBRARY_PATH=.` on the next one: `mise run site` failed with "cannot open shared
  object file" one push after the change went green. Look under all of `target/`, and
  fail the step when nothing is found.


## Spark and Impala are one protocol and two very different loaders
- **What:** Both answer SQL over HiveServer2, so one client (`ibis-hiveserver2`, about 500
  lines of hand-written Thrift) serves both. They differ at the socket: the Spark Thrift
  server expects the SASL `PLAIN` handshake even with authentication off
  (`hive.server2.authentication=NONE`), and an unauthenticated Impala expects none.
  Talking the wrong one gets a closed connection, not an error message.
- **Loading:** Spark takes megabyte `INSERT ... VALUES` statements without complaint.
  Impala plans an expression per cell and keeps finished queries' plans: 100,000 rows
  sent as `VALUES` took the daemon from 0.7 to 4.3 GB and then every statement failed
  with "dropped due to backpressure" (the process memory limit, reported as an RPC
  queue problem). What works is one string literal per statement, cut into rows by a
  join against the numbers 1 to 10,000 and into cells by `SPLIT_PART`.
- **The trap inside that:** Impala pastes a subquery's expressions into every use of them,
  so the row extraction (a scan of the whole literal) ran once per cell, twice with
  `NULLIF`: 17 seconds per statement. A `GROUP BY` on (row number, row text) in the
  subquery materializes it: 0.3 seconds.
- **Avoid:** Assuming `VALUES` is the portable way to load test data. It was wrong for
  Trino too (see above), for the same reason.

## Impala in compose: two things the quickstart images do not tell you
- **Network name:** the metastore client resolves the server's address back to a name and
  gets `<container>.<network>`. Compose's default network is `<project>_default`, and an
  underscore is not legal in a URI host: `impalad` dies at startup with
  `URISyntaxException`. The Impala services have a network of their own with a plain name.
- **Volume ownership:** a new named volume is root-owned unless the mount point exists in
  the image, in which case it takes that directory's owner. The services run as uid 1000,
  so the warehouse is mounted at `/var/lib/hive` (which the metastore image owns) in all
  three containers, and `hive-site.xml` points the warehouse there.
- **Memory:** `-mem_limit` covers the embedded JVM's heap as well. 2 GB with `-Xmx1g` left
  too little; it is 4 GB, of which about 1 GB is used.

## Spark behaviours that needed handling
- **ANSI mode** is on by default since Spark 4: `x / 0` raises. Off, it is NULL. Ibis wants
  IEEE 754, so the dialect writes the `CASE` itself (as Postgres's does).
- **`'it''s'` is `its`:** adjacent string literals concatenate, so a quote is escaped with a
  backslash, and backslashes are doubled. Impala reads literals the same way.
- **Java regex has no POSIX classes:** `[[:digit:]]` is not an error, it is the set of
  characters `:digt`. Ibis's own test swaps the pattern for Java backends; here the
  dialect rewrites `[:name:]` to `\p{Name}` in literal patterns.
- **The JVM's resident memory was twice its heap** until `MALLOC_ARENA_MAX=2`
  (3.3 GB and climbing became 2.3 GB and flat). The retention settings in compose.yaml
  matter for the same reason as Trino's: an INSERT's plan holds its rows.

## CI time is the suite's CPU, and the runner has four cores
- **What:** With nine backends CI went from 6 to 17.5 minutes: the suite ran twice (once
  under `cargo test`, once for the site), one backend after another, about 6.5 minutes a
  pass. Running it once with a thread per backend brought the job to 7 minutes.
- **What did not help:** running the Ibis cases and TPC-H at the same time took a loaded
  16-core machine from 241 s to 183 s and the runner from 238 s to about the same. Nine
  engines on four cores are already CPU-bound; more concurrency moves nothing.
- **Avoid:** judging a CI speedup locally. The next real lever is less work per runner
  (two jobs with the backends split between them, results merged for the site), or a
  cheaper harness (it is compiled unoptimized: `opt-level = 1` cut its CPU on Postgres
  from 20 s to 7 s).
- **Also:** image pulls started in the background at the top of the job (`nohup docker
  compose pull &`) overlap with checkout, cache restore and compile; `mise run up` then
  waits only for what is left. That step varies from 65 to 100 s run to run.

## Arrays, structs and maps: every engine draws the line somewhere else

Porting test_array, test_struct and test_map took one pass per engine, and
each had one fact that shaped everything else:

- **Postgres has no arrays of arrays and no anonymous struct or map.** A
  multidimensional array is rectangular and cannot hold a NULL sub-array, so
  `array<array<T>>` is refused. Structs and maps are `JSONB` (Ibis does this
  for maps; its `ROW`-based structs cannot be stored in a table). Two JSONB
  columns look alike, so `create_table` writes the Ibis type into a column
  comment and `table` reads it back. Map keys are the *text of a value of the
  key type*: looking up the literal `1.0` in a float-keyed map needs
  `CAST(CAST(1.0 AS DOUBLE PRECISION) AS TEXT)`, or it looks for `'1.0'` where
  the key is `'1'`.
- **ClickHouse cannot hold a NULL array, tuple or map.** A NULL array is
  stored and returned as `[]`. Ibis marks those tests `notyet` and so do we;
  no query rewrite gets the NULL back. `map[key]` only takes some key types;
  `mapValues(m)[indexOf(mapKeys(m), k)]` takes any.
- **Trino and Spark take no set-returning function inside an expression in
  SELECT.** A projection with `unnest` is compiled as a lateral join and the
  unnest replaced by the join's column (`Dialect::unnest_in_select`). Trino's
  `UNNEST` of an array of rows spreads the fields over columns: unnest
  `TRANSFORM(arr, e -> ARRAY[e])` and take `v[1]`.
- **Spark over Thrift returns nested values as display text** (`[1,2,null]`,
  strings unescaped, map keys unquoted), which cannot be parsed back reliably.
  `execute` wraps the query and asks for `TO_JSON(NAMED_STRUCT('v', col))`;
  inserts go the other way through `FROM_JSON(text, 'v <type>').v`. Wrapping
  in a struct is what makes both work on every Spark version.
- **Under ANSI mode Spark raises** on an index out of range and on a missing
  map key. `CASE WHEN p < SIZE(x) THEN x[p] END` and a `MAP_KEYS` check keep
  Ibis's NULL. `MAP_CONCAT` raises on a shared key unless
  `spark.sql.mapKeyDedupPolicy=LAST_WIN`, which `connect` sets.

Smaller traps: DuckDB rejects `x -> ...` lambdas now (`LAMBDA x: ...`), and
its lambda position counts from 1. Trino's `SEQUENCE(0, -1)` counts *down*
(two elements), so "positions of an empty array" needs
`SLICE(SEQUENCE(0, n), 1, n)`. Trino reads a struct on the left of
`IN (subquery)` as a row of columns: wrap it in `ROW(...)`
(`Dialect::in_subquery_needle`).

How to avoid the ditch: before spelling an operation for a new engine, find
out whether its containers can be NULL, can nest, and can be produced by a
function in SELECT; and how the driver hands them back. JSON at the boundary
(`ibis_sql::json`) is the dependable transport.

## One method name, three types: resolve it before compiling

`length`, `contains` and `repeat` (and `+`, `*`) are one method on strings,
arrays and maps in Ibis. The builder does not know types, so the IR keeps the
shared name and `Func::resolve(first argument type)` turns it into
`ArrayLength`, `MapContains`, ... when the type is known. Every compiler calls
it before looking at the function; a dialect hook that matches `Func::Length`
for strings then never sees an array. `expr::nested_binary` does the same for
the operators. Forgetting the call in a new compiler gives `LENGTH(array)`
or a string repeat of an array, not an error.

## Lambda parameters are typed late and named by depth

`arr.map(|x| ...)` builds the body from a placeholder `Expr::Param`, which
has no type until the array's element type is known. `Expr::lambda_body`
fills the types in; compilers must compile *that*, not the stored body, or
`dtype` of anything inside the lambda is `null`. Parameters are renamed
`ibis_x<depth>` after the closure runs (innermost is 1): unique names would
change the SQL text from run to run, and a fixed name would capture across
nested lambdas.

## `mise run goldens <filter>` with a broad filter rewrites every golden

The filter is an id substring: `test_` matches everything, and a regenerated
parquet file differs in bytes even when its rows do not (34 files of
unordered results did). Use a filter that names the test function, or
`git checkout` the files whose module you did not touch before committing.

## Tests of the connection API are procedures, not goldens

Everything ported before test_client was "build an expression, compare the
result with Python Ibis's". Ibis's client tests create, fill, rename and drop
tables: there is nothing for Python to pre-compute. They live in
`crates/ibis-conformance/src/client.rs` as functions over a `&dyn Backend`
that return an error when a check does not hold, with ids
`client::<module>::<pytest id>` so the site can file them under the right
Ibis module (`client_cases` in conformance/site.py) even though they have no
Python twin. Each makes its own uniquely named tables and drops them when
its `Temp` guard goes out of scope, so they can run on the same connection,
on the same thread, right after the expression cases.

A type's text form has to survive what a table can hold: `struct<left join:
int64>` (a field name with a space, from Ibis's test_keyword_fields) went
into a Postgres column comment and did not parse back until the type parser
read field names up to the colon rather than as one word.

## Polars: `list.eval` sees the list and nothing else

Inside `list.eval` (and so inside a lambda's body, or any per-element trick)
a named column is an error: "named columns are not allowed in `eval`". That
rules out a Polars expression for anything that pairs a list with another
column per row: zipping two arrays, looking a column's value up in a map,
the position of a column's value in an array. Those run in `rowwise`
(`crates/ibis-polars/src/nested.rs`): a `map_many` closure that turns each
row's arguments into `Value`s, computes the answer as DuckDB gives it, and
builds the output series. It is slower than a kernel and it is correct on
columns. Before replacing one with a Polars expression, check it against a
column argument, not only the literals most Ibis cases use. Two more traps
in the same file: `concat_list` of lists splices them into one list rather
than nesting them (so an array of arrays is built row by row), and
`lit(Series)` is a column of the series's length, which only behaves as a
scalar after `.first()`.

## NOT NULL belongs to stored columns, not to query results

`Field::nullable` reaches Arrow: `rows_to_batch` refuses a NULL in a field
that is not nullable. A `NOT NULL` column read through an outer join is NULL
for the unmatched rows, so a derived relation that kept its input's fields
would fail to build its own result. `Table::wrap` therefore resets every
field of a derived schema to nullable, and only `Table::named` (what
`Backend::table` returns) keeps what the catalog says. ClickHouse reports an
array, tuple or map column as nullable although its type is not `Nullable`:
it takes a NULL on insert and stores the empty value, and `insert` conforms
data to the table's schema before sending it.

## Temporary tables are not where the catalog queries look

`Backend::table` reads a table's columns from the catalog, and a temporary
table is elsewhere in each engine: Postgres keeps it in a per-connection
schema (`pg_my_temp_schema()`), not `current_schema()`; MySQL does not list
it in `information_schema` at all (`SHOW COLUMNS FROM` finds it); SQLite's
`pragma_table_info` and DuckDB's `DESCRIBE` resolve the name the way a query
does and need nothing. ClickHouse's temporary tables last a session, and
each HTTP request is its own unless it carries a `session_id`; a session
also takes one request at a time, which the conformance run (tests on one
connection, sometimes from two threads) would trip over. It is marked as
having none.

## SQL as text: where each engine's reading of a query's columns breaks

`Backend::sql` needs the columns of a query it has not run. The portable way
is a view created and dropped again, read like a table. Two engines needed
something else:

- SQLite gives a view column that is an expression no declared type
  (`pragma_table_info` returns `''`), which read as string: `COUNT(*)` came
  back as text. Ibis asks `typeof()` of the first row for those columns, and
  so does ibis-sqlite now. An empty result still reads as string.
- Trino refuses a view with an unnamed column (`SELECT 'a', 1 AS x`:
  `MISSING_COLUMN_NAME`), where a plain query names it `_col0`. Its REST
  response carries the columns of any result, so ibis-trino runs
  `SELECT * FROM (query) LIMIT 0` and reads them there
  (`SqlClient::query_schema`).

Two more things that looked harmless:

- `SELECT * FROM (query ORDER BY x) AS t0` keeps the order on most engines
  and not on Trino. A `Table` that is only a SQL query now compiles to the
  query itself; anything built on it is still a derived table, and should
  order explicitly.
- A query given to `sql_over` may open with its own `WITH`, so the aliased
  tables cannot be prepended as `WITH a AS (...) ` to it. They go around it:
  `WITH a AS (...) SELECT * FROM (query) AS ibis_sql`. Every engine here
  accepts a `WITH` inside a derived table.

## Reductions: one name, a different function in every engine

Adding `mode`, `median`, `first`/`last` and friends, what was not obvious:

- `default_agg` in ibis-sql refuses these instead of guessing a name: each
  dialect lists the ones it has (`Dialect::agg`). A guessed `MEDIAN(x)`
  would have reached the engine and failed there with its error, not ours.
- Postgres and Trino have no `FIRST`/`LAST` aggregate. `(ARRAY_AGG(x))[1]`
  and `ELEMENT_AT(ARRAY_AGG(x), -1)` do it; Trino's `ELEMENT_AT` returns
  NULL past the end, where `[n]` raises.
- ClickHouse's `any` and `anyLast` skip NULLs; DuckDB's `FIRST` does not, so
  `first`, `last` (and `collect`) add `x IS NOT NULL` to the aggregate's
  filter in the compiler, for every dialect.
- The median of an even count interpolates in DuckDB, Postgres
  (`PERCENTILE_CONT`), Spark (`PERCENTILE`) and Polars. ClickHouse's
  `median` is approximate and `quantileExact` picks one of the two middle
  values: `quantileExactInclusive(0.5)` is the one that interpolates.
- MySQL's `BIT_AND`/`BIT_OR`/`BIT_XOR` return `BIGINT UNSIGNED`: cast to
  `SIGNED`, or the value comes back as a type the column never had.
- SQLite has none of mode, bitwise reductions, first or last. Ibis registers
  Python functions there; ibis-sqlite registers Rust ones
  (`polyfill.rs`, `ibis_mode`, `ibis_bit_and`, ...).
- `Expr::first()` used to build `FIRST_VALUE` and needed `.over(w)`. It is a
  reduction now, and `over` turns it back into `FIRST_VALUE`: the first
  value of a window frame may be NULL, the first value of a group may not.
- A test whose right answer depends on the engine (which value is first in
  rows with no order, an approximate median) cannot have a golden: those
  are procedures with Ibis's own assertion (`reductions.rs`).

## `GROUP BY 'a'`: a constant key needs its position, not its text

`t.group_by([lit("a").name("key")])` compiled to `GROUP BY 'a'`, which every
engine here takes except Postgres ("non-integer constant in GROUP BY"). An
integer there is a position in the select list, so a key that reads no
column is now written as its position (`GROUP BY 1`), which all nine accept.
Dropping the key from `GROUP BY` instead would be wrong: with no `GROUP BY`
an aggregate of no rows returns one row, not none.

## argmin and a NULL argument: three engines, three spellings

`x.argmin(key)` is `x` in the row with the smallest key, and that `x` may be
NULL (Ibis tests it). The obvious function drops such rows on two engines:

- DuckDB's `ARG_MIN(x, key)` passes over rows where `x` is NULL. Ibis writes
  `FIRST(x ORDER BY key)` instead, and so does ibis-duckdb.
- ClickHouse's `argMin(x, key)` does the same; wrapped as
  `tupleElement(argMin(tuple(x), key), 1)` the NULL is a value inside a
  tuple and survives.
- Postgres has no such aggregate: `(ARRAY_AGG(x ORDER BY key))[1]`.
- Trino and Spark have `MIN_BY`, which keeps the NULL.

In every form the key's NULLs must sort last (`NULLS LAST`, also descending
for `argmax`), or a NULL key wins.

Pearson's correlation is one number for a sample and a population, but
DuckDB, Postgres and Trino say theirs is the population's and ClickHouse
that its is the sample's; Ibis refuses the other on each, and so does
ibis-rs. The consequence for the suite: `corr_samp` has no golden (DuckDB
cannot compute one), so those two ids need a procedure.

## Quantiles: which of two middle values, and what the function accepts

- An exact quantile of numbers interpolates (`PERCENTILE_CONT`); of text or
  dates it must be one of the values (`PERCENTILE_DISC`), and of two middle
  values that is the lower. ClickHouse's `quantileExact` picks the higher:
  `quantileExactLow` matches. Its numeric `quantileExactInclusive`
  interpolates like the others.
- ClickHouse's quantile functions take no `Date32` (which is what a date is
  there, for its range): the quantile is taken of `toInt32(date)`, days
  since 1970, and turned back with `toDate32`.
- ClickHouse writes the fractions as parameters, `quantiles(0.25, 0.75)(x)`,
  so they must be literals (`ExprCompiler::fractions`).
- DuckDB's `APPROX_QUANTILE(x, [0.25, 0.75])` finds no overload: the list is
  of decimals and it wants `FLOAT[]`; a single decimal fraction is cast for
  it. It also answers in the column's type, where Ibis says float.
- Ids of stacked `parametrize` decorators read bottom-up: for
  `@parametrize("filtered") @parametrize("multi") def test_approx_quantile`
  the id `[True-False]` is multi=True, filtered=False.

## group_concat: an empty group, and MySQL's silent 1,024 bytes

- MySQL's `GROUP_CONCAT` cuts its result at `group_concat_max_len`, 1,024
  bytes by default, and reports that as a warning the client never sees:
  the test compared 730 joined strings and got the first 512. ibis-mysql
  raises the limit when it connects (with its other session settings).
- No strings in a group must give NULL. `STRING_AGG` and `GROUP_CONCAT` do;
  joining an array does not: Spark's `ARRAY_JOIN(COLLECT_LIST(x), sep)` and
  ClickHouse's `arrayStringConcat(groupArray(x), sep)` give `''` and need
  the empty array turned into NULL first. Trino's `ARRAY_AGG` of no rows is
  already NULL.
- MySQL's separator is syntax (`SEPARATOR ':'`), so it must be a literal.
- Ibis marks the windowed form unsupported on ClickHouse, Trino and PySpark;
  the array forms above work over a window on all three, so those marks
  were dropped here (the harness reports an unexpected pass as a failure,
  which is how it showed).

## Reductions with an order, NULLs kept, or distinct values

`first`, `last` and `collect` take `order_by`, `include_null` and `distinct`.
What was not obvious:

- Polars removes a sort whose result feeds `implode(maintain_order=false)`:
  the optimizer is told the order does not matter and believes it. An
  ordered `collect` came back unsorted, but only when nothing stood between
  the sort and the `implode`: with a filter in between the sort survived, so
  one of the sixteen `test_collect` variants failed. Pass `true` when there
  is an order.
- Postgres refuses `ARRAY_AGG(x ORDER BY k) OVER ()` ("aggregate ORDER BY is
  not implemented for window functions"), so an ordered `first` in a
  projection cannot be the reduction with `OVER ()` after it. Ibis compiles
  it to `FIRST_VALUE(x) OVER (ORDER BY k ROWS BETWEEN UNBOUNDED PRECEDING AND
  UNBOUNDED FOLLOWING)` on every backend, and so does ibis-sql. That form
  does not pass over NULLs; neither does Ibis's.
- SQLite 3.44 and later take `ORDER BY` inside any aggregate call, registered
  ones included: `ibis_first(x ORDER BY k)` works with the SQLite that
  rusqlite bundles. Ibis marks it unsupported. A build against an older
  system SQLite would get a syntax error from the engine.
- Postgres has no `FIRST`, and `(ARRAY_AGG(x ORDER BY k))[1]` keeps NULLs as
  it stands: `include_null` costs nothing there, where Ibis refuses it.
- Without `FILTER`, a condition is folded into the argument
  (`CASE WHEN cond THEN x END`). With `include_null` that turns every row
  failing the condition into a NULL that counts, so the compiler refuses the
  combination on such dialects instead of returning a wrong value.
- A count of distinct rows is `COUNT(DISTINCT (a, b))` on DuckDB and
  Postgres, `ROW(a, b)` on Trino, `STRUCT(a, b)` on Spark, and several
  arguments (`COUNT(DISTINCT a, b)`) on MySQL and Impala. They do not agree
  on NULLs: for the rows `(1, NULL), (1, NULL), (2, 3), (NULL, NULL)`
  Postgres and Spark's `STRUCT` count 3, MySQL and Spark's several-argument
  form count 1 (a row with any NULL is not counted). So the fold above is
  right only for the several-argument form, and `Dialect::count_distinct_rows`
  is handed the condition rather than columns already folded: the default
  (a tuple) refuses it.
- Dialect `reduction` hooks that read `options.args[0]` before looking at
  the function panic once a reduction with options and no further argument
  exists (`first().include_null()`). Read it inside the arm that needs it.

## Intervals: compile the movement, not the interval

A timestamp plus an interval looked like it needed an interval value in
every engine. It does not, and several engines have none:

- SQLite has no interval type at all (`DATETIME(ts, '+4 day')`), and MySQL
  and Impala take `INTERVAL n UNIT` only as the right side of one addition.
  Ibis refuses `ts + (interval_a - interval_b)` on all three.
- `Expr::shifts()` flattens an interval expression into steps (count, unit,
  forward or back), and the compiler applies them one after the other:
  `((ts + INTERVAL '4' DAY) - INTERVAL '2' DAY)`. Each dialect spells one
  step (`Dialect::date_shift`) and sums of intervals work everywhere.
  Normalizing weeks to 7 days and quarters to 3 months in `shifts()` removed
  the two units engines disagree on most (Postgres has no quarter: `INTERVAL
  '1 quarter'` is invalid input; Ibis's Trino backend has no week).
- A date plus an interval is a timestamp in DuckDB, Postgres and Impala and
  a date in Ibis: the shift is wrapped in `CAST(... AS DATE)`.
- Postgres does not refuse `ts + INTERVAL '1' MILLISECOND`: it adds one
  second. `MILLISECOND` is no interval field there, so the statement is
  `INTERVAL '1'` (a second) with `millisecond` as the column's alias. The
  unit goes inside the quotes (`INTERVAL '1 millisecond'`), where it takes
  any unit down to the microsecond.
- For a count that is not a literal, `count * INTERVAL '1' UNIT` works on
  DuckDB, Postgres and Spark; ClickHouse has `toIntervalDay(count)`, Trino
  `DATE_ADD('day', count, ts)`, SQLite `PRINTF('%+d day', count)` as the
  modifier (`'+' || count` gives `'+-3 day'` for a negative count, which
  SQLite answers with NULL).
- SQLite's `'floor'` modifier (3.46) is what keeps January 31 plus one month
  in February (`2023-02-28`); without it the answer is `2023-03-03`.
- Polars: `dt().offset_by()` takes a string expression (`"4d"`, or
  `concat_str([count, lit("mo")])` for a column), so years and months work
  there although `duration()` has neither. Ibis marks them unsupported.
- An interval literal is `Value::Int` with an interval `DataType`: no new
  `Value` variant until a column of intervals has to be read back. Until
  then an interval outside date arithmetic is `Unsupported` at compile time.
- The conformance filter matches ids, not modules: `test_temporal` selected
  only the 18 ids that contain that text.

## Seconds since 1970: the engine's own function answers in the session's zone

A number cast to a timestamp, `as_timestamp` and `epoch_seconds` each have
an obvious function in every engine, and most of those are wrong for a
timestamp without a time zone. Checked with the session set to another zone:

- MySQL `FROM_UNIXTIME(0)` is `1970-01-01 05:00:00` at `+05:00`, and
  `UNIX_TIMESTAMP` reads its argument the same way. Postgres
  `CAST(TO_TIMESTAMP(0) AS TIMESTAMP)` and Trino
  `CAST(FROM_UNIXTIME(0) AS TIMESTAMP)` are `09:00:00` in Tokyo, and Trino's
  `TO_UNIXTIME(TIMESTAMP '1970-01-01 09:00:00')` is 0 there.
- The goldens are UTC wall-clock times (Python Ibis on DuckDB, on a machine
  in US/Pacific, still gave `1970-01-01 00:00:00` for 0), so the test
  containers, all in UTC, would have passed with the obvious functions and
  a user's session would not.
- What does not depend on the session: arithmetic from a literal. The count
  is turned into microseconds once, in core (`epoch_micros`), and each
  dialect adds them to `1970-01-01 00:00:00` its own way
  (`Dialect::timestamp_from_micros`); `epoch_seconds` is the difference from
  that literal (`TIMESTAMPDIFF`, `DATE_DIFF`), or `EXTRACT(EPOCH FROM ts)`
  where the engine reads a zoneless timestamp as it is written (Postgres).
- SQLite's `'unixepoch', 'subsec'` keeps three digits. The seconds go
  through `STRFTIME` and the six digits after them are written with
  `PRINTF('%06d', ...)`; the same trick keeps the fraction of a string cast
  to a timestamp, which `DATETIME(x)` silently dropped.
- Polars casts `"2023-01-07 13:20:05.561"` to a datetime as NULL;
  `str.to_datetime` with no format works it out.

## delta: truncate both ends, then any difference function will do

Ibis's `delta` counts the boundaries of a unit between two moments (23:59:59
to 01:58:00 is two hours). Engines split into those that count boundaries
(DuckDB and ClickHouse `DATE_DIFF`) and those that count whole units elapsed
(MySQL and Spark `TIMESTAMPDIFF`: one hour for that pair), and Postgres has
neither. Cutting both ends back to the start of the unit first
(`delta_ends`) makes the two kinds agree, so each dialect only names a
difference function (`Dialect::date_diff`). Ibis leaves the timestamp delta
out on MySQL, SQLite, Polars and Impala and expects Spark's to fail.

It was checked beyond Ibis's two tests with a throwaway procedure: one pair
of timestamps a year and two months apart, every unit from years to
microseconds, on all nine backends. Worth doing again when a function has
many units and the suite tests two of them.

## ClickHouse `Date32` holds the year 1 and the year 9999 now

Ibis marks `test_date_scalar` for dates outside 1900 to 2299 on ClickHouse.
With ClickHouse 26.10 `toDate32('0001-07-17')` and `toDate32('9999-01-02')`
come back unchanged, so those cases pass. An unexpected pass on a newer
engine is worth one direct query before the mark is dropped.

## bucket: a feature built from three others needs no dialect code

Ibis's `bucket` is `time_bucket` on DuckDB, `date_bin` on Postgres,
`toStartOfInterval` on ClickHouse, and missing on Trino, Spark, MySQL,
SQLite and Impala. Here it is one expression in core
(`ibis_core::expr::bucket_expr`): seconds since 1970 from `delta`, plus
`microsecond`, rounded down to the width with `floordiv`, and back through
`as_timestamp`. All nine backends passed the seven tests on the first run,
because each piece was already tested on each of them. Before writing a
dialect hook, check whether the operation is arithmetic on ones that exist.

Two things to remember: the function is intercepted before its arguments are
compiled (they are intervals, which do not compile outside date arithmetic),
and the seconds must come from `delta` (int64), not `epoch_seconds`, which is
int32 as in Ibis and ends in 2038.

## Parsing by a pattern: `[pyspark_format]` is not a test of parsing

`test_string_as_timestamp` and `test_string_as_date` have two ids each: a C
pattern (`%m/%d/%y`) and Java's (`MM/dd/yy`). Ibis hands the pattern to the
engine as written, so each id passes only where the engine speaks that
pattern language, and the marks say so. ibis-rs takes C patterns everywhere
and translates (the helpers `strftime` already used read as well as they
write), so the Java ids are marked not applicable instead of ported as
expected failures on eight backends.

## Time zones: the zone is in the type, the value stays UTC

`Value::Timestamp` is microseconds since 1970 and a zoneless timestamp is
read as UTC everywhere in this project. That made the first zone tests
almost free:

- `timestamp_col.cast("timestamp('UTC')")` and
  `ibis.timestamp(...).cast(timestamp('PST8PDT'))` do not change the
  moment, so on a SQL backend the cast compiles to its operand and the
  schema carries the zone. Rows are turned into Arrow with the schema's
  zone as a label (`with_timezone_opt`), so `16:20` comes back as
  `08:20-08:00` for PST8PDT with no engine involved. Ibis marks this test
  for MySQL, Spark, Impala and SQLite; all nine pass.
- What this model cannot answer is anything that reads a wall clock other
  than UTC's: `hour()`, `date()`, `strftime`, a cast back to no zone. Those
  would silently answer in UTC, so the compiler refuses them for a zone
  that is not UTC (`DataType::foreign_zone`). Wrong answers that look right
  are the cost of a shortcut like this one; the refusal is what makes it
  safe to ship before `AT TIME ZONE` is written for eight dialects.
- Polars has real zones, and one sharp edge: it will not compare
  `Datetime('us', 'UTC')` with `Datetime('us')`. Cast the zoneless side to
  the other's type first (a zoneless value cast to a zone is read as UTC
  there, as here).
- `now()`: `CURRENT_TIMESTAMP` is a moment with the session's zone on most
  engines and a wall clock in it on others. Asking for UTC outright
  (`NOW() AT TIME ZONE 'UTC'`, `UTC_TIMESTAMP(6)`, `now64(6, 'UTC')`,
  `STRFTIME(..., 'now')`, DuckDB's `MAKE_TIMESTAMP(EPOCH_US(NOW()))`, Spark's
  `CONVERT_TIMEZONE(CURRENT_TIMEZONE(), 'UTC', LOCALTIMESTAMP())`) gives the
  same value everywhere. The test compares it with the host's clock to
  within ten minutes, which a wall clock from a session in another zone
  would not meet (the test containers are all in UTC, so it has not been
  seen to fail).
- `test_timestamp_comparison_filter[eq]` matches no row at all (no row has
  exactly that timestamp). It is Ibis's test, ported as it is; it passes on
  an engine that returns nothing for every `=`.

## The schema check is a tenth backend, and a one-backend summary hides it

Commit d44dced went out with `as_timestamp(format)` typed `timestamp` where
Ibis says `timestamp('UTC')`. Every backend returned the right rows, all
nine summaries read "pass", and CI failed: the runner also compares the type
ibis-rs gives each case with the type in the golden, and reports that as a
backend called `schema`. The scratch scripts used to check a batch printed
the line of the backend they were run for and nothing else, so the one
`schema fail` among 1,139 outcomes was never on screen.

- Run `mise run conformance` (it exits non-zero on any failure, the schema's
  included), or make any narrower script print every status that is not
  `pass` or `xfail`, whatever backend it belongs to.
- A new function's type is part of porting it: read the type off the Ibis
  expression (`expr.type()`) when writing the case, not only the SQL.
- Waiting for CI before the next push is what kept this to one red build:
  the following slice was ready and was held back when the run came in red.

## Bits, random numbers and decimal literals

- Bitwise operators are spelled four ways (operators; `bitAnd`; `BITWISE_AND`;
  `BITAND`) and typed three. MySQL's answer in `BIGINT UNSIGNED`: `~2` is
  18446744073709551613 unless it is cast to `SIGNED`, and that cast reads
  the same bits back as -3. A shift is as wide as what is shifted on Trino,
  ClickHouse and Impala, where a literal `3` is a TINYINT: Ibis marks
  `3 << int_col` wrong on Impala for that. Casting the left side to BIGINT
  first makes it Ibis's int64 everywhere.
- Postgres has no `bigint << bigint`; the count must be an `integer`.
- SQLite has `&`, `|`, `<<`, `>>` and `~` but no XOR: `(a | b) - (a & b)`.
- Polars has no shift expression. Multiplying and floor-dividing by `2^n` is
  exact in int64 (and floor division is the arithmetic shift of a negative
  number), up to the bit that overflows.
- SQLite's `RANDOM()` is any int64. `0.5 + RANDOM() / 2^64` can round to
  exactly 1.0; masking 53 bits and dividing by 2^53 cannot.
- Impala's `RAND()` starts from the same seed in every query, so a seed from
  the clock is what makes two queries differ (Ibis does the same).
- A test with a random answer has no golden: it is a procedure
  (`crates/ibis-conformance/src/random.rs`) that checks what the Ibis test
  checks, the range and that rows differ.
- An expression that mixes a column with a reduction of it (`histogram`:
  `(x - min(x)) / width`) is a projection whose reductions become windows
  `OVER ()`. The conformance harness's `column` shape aggregates anything
  that contains a reduction, so such a case is written as a `table`.
- A decimal literal written as its digits (`1.1`) takes whatever type the
  engine reads digits as: DECIMAL(2,1) on DuckDB, a double on others. The
  values compare equal, so only the type gives it away; the literal is now a
  cast to its decimal type.
- Ibis's `dt.decimal` has no precision or scale and each engine fills in its
  own (18 and 3 on DuckDB, 10 and 0 on MySQL, where 1.1 becomes 1).
  `DataType::Decimal` always has both, so that one case differs from Ibis in
  type: it carries a `notimpl` for the `schema` check, the first one.
- A memtable column of decimals: PyArrow will not build a decimal array from
  floats (convert through `Decimal(str(v))`), and Python's sqlite3 will not
  bind a `Decimal` (hand it a float). SQLite keeps `DECIMAL(38, 9)` as the
  declared type, which is the only place the type survives there.

## Strings: whitespace, accents, an emoji

Ibis's `test_string_methods_accents_and_emoji` runs fourteen methods over
seven strings: `"AbC\t"`, `"\n123\n   "`, `"abc, 123"`, `"123"`, `"aBc"`, a
snake emoji and `"ÉéÈèêç"`. Every earlier string case used ASCII digits, and
all of the following had gone unseen.

- `strip` means all whitespace (space, tab, newline, carriage return,
  vertical tab, form feed), not `TRIM(x)`, which takes off spaces. Every
  engine takes a set of characters in some form (`TRIM(x, set)`,
  `TRIM(BOTH set FROM x)`, `LTRIM(x, set)`), except MySQL, whose `TRIM`
  takes off one whole string: there it is `REGEXP_REPLACE`. (Ibis nests six
  `TRIM`s on MySQL, which leaves `" \t x"` with a space.)
- `lpad`/`rpad` in Ibis never shorten a string; SQL's `LPAD(x, n, fill)` cuts
  it to `n`. `LPAD(x, GREATEST(LENGTH(x), n), fill)` is the Ibis meaning.
- Length in characters: ClickHouse's plain functions count bytes
  (`lengthUTF8`, `upperUTF8`, `leftPadUTF8` are the others); Impala counts
  bytes unless the session has `SET UTF8_MODE=true`, and its `LPAD`/`RPAD`
  count bytes even then; SQLite's `UPPER`/`LOWER` know ASCII only.
- Postgres takes `UPPER`/`LOWER` from the database's `LC_CTYPE`. The test
  database was made with `--no-locale` (for bytewise ordering), which is also
  "letters are ASCII". `--lc-collate=C --lc-ctype=C.UTF-8` keeps the ordering
  and knows the letters. `initdb` runs once per volume: changing the
  arguments does nothing until the volume is removed.
- ibis-clickhouse read a response by dropping empty lines. In a result of one
  string column an empty line is a row holding `''`: `capitalize('')` came
  back as no rows. Strip the one trailing newline and keep every line.
- A table with a tab and newlines in its values loaded correctly on all nine
  backends through each one's existing loader (the SQL-text loaders escape
  through their dialect's `quote_str`).

Other things from the same batch:

- DuckDB 2 answers `''` for `ARRAY_TO_STRING([], ',')`; DuckDB 1 answered
  NULL, which is what Ibis's `test_empty_array_string_join` asserts and what
  Ibis compiles every other backend to give. When the golden engine has
  moved away from what the test states, the test is the spec: the case is a
  procedure with the test's own expectation, and ibis-duckdb is compiled to
  give NULL like the rest.
- Spark's `SPLIT` takes a regular expression. A delimiter is wrapped in
  `\Q...\E`, and the limit is `-1` so trailing empty strings are kept.
- ClickHouse's `splitByRegexp` and Polars's `replace_all` take one pattern
  for the whole column; a pattern per row is not expressible (Ibis marks
  both too).
- Polars has no regular-expression split. Replacing every match with U+0000
  and splitting on that literal gives the same pieces, for any string
  without that character in it.
- `CONCAT_WS` skips NULL arguments and `||` does not; Ibis's `join` is the
  first and its `concat`/`+` the second, so `"abc".concat(None)` is NULL.
- Postgres has `LEVENSHTEIN` only with `CREATE EXTENSION fuzzystrmatch`
  (shipped in the standard image, `docker/postgres/init.sql` creates it).
- `scratchpad/multi.sh` starts the runner once per filter, and each start
  loads every fixture: about a second on DuckDB, a minute and a half on
  Impala. Eleven filters on Impala is a quarter of an hour; give Impala one
  broad filter.

## try_cast, hashes and a window in a filter

- A cast that answers NULL instead of failing exists on DuckDB, Trino and
  Spark (`TRY_CAST`), ClickHouse (`accurateCastOrNull`) and Polars (any cast
  that is not `strict_cast`). Impala's plain `CAST` already answers NULL.
  Postgres raises, and MySQL and SQLite answer 0 for text that is no number,
  so `try_cast` is `Unsupported` there rather than a cast that lies.
- "Cannot be cast" is the engine's judgement. A timestamp tried as an integer
  is NULL on DuckDB, its seconds since 1970 on ClickHouse and Impala, its
  microseconds on Polars, and a type error on Trino and Spark. Ibis marks all
  of those against DuckDB's answer, and so do the cases.
- `try_cast` carries its type as a string literal argument (`Func::TryCast`
  with `[x, 'int64']`), like the unit of `truncate`: a `Func` has no place
  for a `DataType`, and the type parses back from Ibis's spelling.
  `Expr::dtype` types it before `func_dtype`, which only sees types.
- A hash is not comparable between engines, so `test_hash` is a procedure:
  an int64, never NULL, the same twice. DuckDB's `HASH` is unsigned (read the
  bits as signed through `BIT`); Trino hashes bytes to bytes
  (`FROM_BIG_ENDIAN_64(XXHASH64(TO_UTF8(...)))`); Polars's is a u64, reduced
  modulo 2^63 because a cast of a u64 above the int64 range is NULL.
- `Expr::hash` on Polars needs the crate's `row_hash` feature, and the Rust
  compiler's hint for the missing method is `std::hash::Hash`, which is
  beside the point.
- `typeof` is a reserved word in Rust: the method is `typeof_`.
- Ibis lets a window function into `filter`. The window is over the rows
  before the filter, so it cannot be a `WHERE`: `Table::filter` computes each
  such predicate as a column (`mutate`), filters on the columns, and selects
  the original ones again. No backend needed to change.
- A conformance `column` case whose function returns a literal is run as an
  aggregate (a literal is a scalar to Ibis, as a reduction is). A test that
  reads a literal as a column of the table is a `table` case.
