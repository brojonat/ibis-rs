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
