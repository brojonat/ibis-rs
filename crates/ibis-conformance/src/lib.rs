//! Conformance runner: every ported Ibis case, on every backend, compared to
//! goldens produced by Python Ibis (conformance/golden).

pub mod cases;
pub mod tpch;

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use arrow_array::RecordBatch;
use ibis_core::convert::{batch_to_rows, rows_to_batch};
use ibis_core::prelude::*;
use ibis_core::{Error, Field, Result, Value};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde_json::{Value as Json, json};

use crate::cases::{Case, Fx};

pub const FIXTURES: [&str; 3] = ["functional_alltypes", "batting", "awards_players"];

/// Paths the runner needs, resolved from the repo root.
pub struct Paths {
    pub golden: PathBuf,
    pub memtables: PathBuf,
    pub testing_data: PathBuf,
    /// TPC-H databases at scale factor 0.01 (bench/tpch_data.py).
    pub tpch: PathBuf,
}

impl Paths {
    pub fn from_repo_root(root: &Path) -> Self {
        let testing_data = std::env::var_os("IBIS_TESTING_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("ci/ibis-testing-data"));
        Paths {
            golden: root.join("conformance/golden"),
            memtables: root.join("conformance/memtables.json"),
            testing_data,
            tpch: root.join("ci/tpch/sf0_01"),
        }
    }

    /// Repo root, from this crate's manifest dir.
    pub fn default_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }
}

pub fn read_parquet(path: &Path) -> Result<Vec<RecordBatch>> {
    let f = File::open(path).map_err(|e| Error::Other(format!("open {}: {e}", path.display())))?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(f)
        .and_then(|b| b.build())
        .map_err(|e| Error::Other(format!("read {}: {e}", path.display())))?;
    Ok(reader.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// The fixture tables, as Arrow data: the three Ibis parquet files plus the
/// in-memory tables of conformance/memtables.json.
pub struct FixtureData {
    tables: Vec<(String, Vec<RecordBatch>)>,
    memtables: Vec<String>,
}

/// A column type in memtables.json, spelled as Ibis spells it.
fn dtype_of(name: &str) -> Result<DataType> {
    name.parse().map_err(|e| Error::Other(format!("memtables.json: {e}")))
}

/// A cell of memtables.json as a value of `dtype`. An array is a JSON array,
/// a struct an object, and a map an array of `[key, value]` pairs.
fn json_value(v: &Json, dtype: &DataType) -> Result<Value> {
    let bad = || Error::Other(format!("memtables.json: {v} is not a value of type {dtype}"));
    Ok(match (v, dtype) {
        (Json::Null, _) => Value::Null,
        (Json::Bool(b), _) => Value::Bool(*b),
        (Json::Number(n), DataType::Float32 | DataType::Float64) => Value::Float(n.as_f64().ok_or_else(bad)?),
        (Json::Number(n), _) if n.is_i64() => Value::Int(n.as_i64().unwrap()),
        (Json::Number(n), _) => Value::Float(n.as_f64().ok_or_else(bad)?),
        (Json::String(s), _) => Value::String(s.clone()),
        (Json::Array(items), DataType::Array(item)) => {
            Value::Array(items.iter().map(|v| json_value(v, item)).collect::<Result<_>>()?)
        }
        (Json::Array(entries), DataType::Map(key, value)) => Value::Map(
            entries
                .iter()
                .map(|entry| match entry.as_array().map(Vec::as_slice) {
                    Some([k, v]) => Ok((json_value(k, key)?, json_value(v, value)?)),
                    _ => Err(bad()),
                })
                .collect::<Result<_>>()?,
        ),
        (Json::Object(object), DataType::Struct(fields)) => Value::Struct(
            fields
                .iter()
                .map(|(name, t)| Ok((name.clone(), json_value(object.get(name).unwrap_or(&Json::Null), t)?)))
                .collect::<Result<_>>()?,
        ),
        _ => return Err(bad()),
    })
}

impl FixtureData {
    pub fn read(paths: &Paths) -> Result<FixtureData> {
        let mut tables = vec![];
        for name in FIXTURES {
            tables.push((name.to_string(), read_parquet(&paths.testing_data.join(format!("parquet/{name}.parquet")))?));
        }
        let p = paths.memtables.clone();
        let text = std::fs::read_to_string(&p).map_err(|e| Error::Other(format!("read {}: {e}", p.display())))?;
        let spec: BTreeMap<String, Json> =
            serde_json::from_str(&text).map_err(|e| Error::Other(format!("parse {}: {e}", p.display())))?;
        let bad = |name: &str| Error::Other(format!("memtables.json: malformed table {name:?}"));
        let mut memtables = vec![];
        for (name, t) in &spec {
            let cols = t["columns"].as_array().ok_or_else(|| bad(name))?;
            let fields = cols
                .iter()
                .map(|c| {
                    Ok((c[0].as_str().ok_or_else(|| bad(name))?, dtype_of(c[1].as_str().ok_or_else(|| bad(name))?)?))
                })
                .collect::<Result<Vec<_>>>()?;
            let rows = t["rows"]
                .as_array()
                .ok_or_else(|| bad(name))?
                .iter()
                .map(|r| {
                    let cells = r.as_array().ok_or_else(|| bad(name))?;
                    cells.iter().zip(&fields).map(|(v, (_, t))| json_value(v, t)).collect()
                })
                .collect::<Result<Vec<Vec<Value>>>>()?;
            tables.push((name.clone(), vec![rows_to_batch(&Schema::of(&fields), &rows)?]));
            memtables.push(name.clone());
        }
        Ok(FixtureData { tables, memtables })
    }

    /// `skip` names the memtables this backend has no column types for.
    fn fx(&self, backend: &str, skip: &[&str], table: impl Fn(&str) -> Result<Table>) -> Result<Fx> {
        let mem = self.memtables.iter().filter(|n| !skip.contains(&n.as_str()));
        Ok(Fx {
            alltypes: table("functional_alltypes")?,
            batting: table("batting")?,
            awards_players: table("awards_players")?,
            mem: mem.map(|n| Ok((n.clone(), table(n)?))).collect::<Result<_>>()?,
            backend: backend.to_string(),
            skipped: skip.iter().map(|s| s.to_string()).collect(),
        })
    }

    /// Fixtures with the schemas Python Ibis saw, attached to no backend:
    /// what type inference is checked against.
    pub fn unbound(&self) -> Result<Fx> {
        self.fx(SCHEMA, &[], |name| {
            let (_, batches) = self.tables.iter().find(|(n, _)| n == name).expect("known fixture");
            Ok(Table::named(name, Schema::from_arrow(&batches[0].schema())?))
        })
    }

    /// Create every fixture table on `b` and return them as the backend sees them.
    ///
    /// A table with arrays, structs or maps is left out on a backend that
    /// says it has no such types (SQLite, MySQL and Impala, as in Ibis); the
    /// cases that use it then fail for want of the table, which their
    /// `notimpl` marks expect.
    pub fn load(&self, b: &dyn Backend) -> Result<Fx> {
        let mut skipped = vec![];
        // tables created without their arrays of arrays (Postgres has none)
        let mut flat: BTreeMap<&str, Vec<RecordBatch>> = BTreeMap::new();
        for (name, batches) in &self.tables {
            let nested = batches[0].schema().fields().iter().any(|f| f.data_type().is_nested());
            match b.create_table(name, batches) {
                Err(Error::Unsupported { .. }) if nested => {
                    let without = without_arrays_of_arrays(batches)?;
                    match without.as_deref().map(|batches| b.create_table(name, batches)) {
                        Some(Ok(())) => drop(flat.insert(name.as_str(), without.expect("just created"))),
                        Some(Err(Error::Unsupported { .. })) | None => skipped.push(name.as_str()),
                        Some(other) => other?,
                    }
                }
                other => other?,
            }
        }
        self.fx(b.name(), &skipped, |name| {
            let (_, batches) = self.tables.iter().find(|(n, _)| n == name).expect("known fixture");
            let created = flat.get(name).unwrap_or(batches);
            bind(b, name, &Schema::from_arrow(&created[0].schema())?)
        })
    }
}

/// `batches` without the columns that are arrays of arrays; `None` if there
/// are no such columns, or no others.
fn without_arrays_of_arrays(batches: &[RecordBatch]) -> Result<Option<Vec<RecordBatch>>> {
    let schema = Schema::from_arrow(&batches[0].schema())?;
    let keep: Vec<usize> = (0..schema.len())
        .filter(|i| !matches!(&schema.fields[*i].dtype, DataType::Array(item) if matches!(**item, DataType::Array(_))))
        .collect();
    if keep.len() == schema.len() || keep.is_empty() {
        return Ok(None);
    }
    Ok(Some(batches.iter().map(|b| b.project(&keep)).collect::<std::result::Result<_, _>>()?))
}

/// `b`'s table `name`, under the column names it was created with (`created`).
/// Trino folds identifiers to lower case and resolves them case-insensitively,
/// so its `playerid` is given back the name the cases use, `playerID`.
fn bind(b: &dyn Backend, name: &str, created: &Schema) -> Result<Table> {
    let t = b.table(name)?;
    let fields = &t.schema().fields;
    let folded = fields.len() == created.len()
        && fields.iter().zip(&created.fields).all(|(f, c)| f.name.eq_ignore_ascii_case(&c.name))
        && fields.iter().zip(&created.fields).any(|(f, c)| f.name != c.name);
    if !folded {
        return Ok(t);
    }
    let renamed = fields.iter().zip(&created.fields).map(|(f, c)| Field::new(&c.name, f.dtype.clone())).collect();
    Ok(Table::named(name, Schema::new(renamed)))
}

/// A backend under test and its fixture tables.
pub struct Target {
    pub backend: Box<dyn Backend>,
    pub fx: Fx,
}

/// One result per item, in order, each computed on a thread of its own.
/// The backends are separate engines, most of them separate servers: run one
/// after another, the suite spends its time waiting on one of them at a time.
fn each<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    std::thread::scope(|scope| {
        let workers: Vec<_> = items.iter().map(|item| scope.spawn(|| f(item))).collect();
        workers.into_iter().map(|w| w.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))).collect()
    })
}

pub fn targets(data: &FixtureData, names: &[&str]) -> Result<Vec<Target>> {
    each(names, |n| {
        let backend: Box<dyn Backend> = match *n {
            "duckdb" => Box::new(ibis_duckdb::DuckDb::open_in_memory()?),
            "sqlite" => Box::new(ibis_sqlite::Sqlite::open_in_memory()?),
            "polars" => Box::new(ibis_polars::Polars::new()),
            server if SERVERS.contains(&server) => connect(server, &server_url(server)?)?,
            other => return Err(Error::Other(format!("unknown backend {other:?}"))),
        };
        let fx = data.load(backend.as_ref())?;
        Ok(Target { backend, fx })
    })
    .into_iter()
    .collect()
}

pub const ALL_BACKENDS: [&str; 9] =
    ["clickhouse", "duckdb", "impala", "mysql", "polars", "postgres", "spark", "sqlite", "trino"];

/// Backends that are a server to connect to, each a service in compose.yaml.
const SERVERS: [&str; 6] = ["clickhouse", "impala", "mysql", "postgres", "spark", "trino"];

/// Backends whose port is in progress. They are run and reported like the
/// rest, but their failures do not fail the run and `cargo test` leaves them
/// out, so a backend can land before it passes everything.
pub const EXPERIMENTAL: [&str; 0] = [];

/// The backends every case must pass on (or be marked `notimpl` for).
pub fn gated() -> Vec<&'static str> {
    ALL_BACKENDS.into_iter().filter(|b| !EXPERIMENTAL.contains(b)).collect()
}

/// Where the test instance of a server backend is: `IBIS_TESTING_<NAME>_URL`.
fn server_url(name: &str) -> Result<String> {
    let var = format!("IBIS_TESTING_{}_URL", name.to_uppercase());
    std::env::var(&var).map_err(|_| {
        Error::Other(format!("{name}: set {var} (mise.toml does; `mise run up {name}` starts the server)"))
    })
}

fn connect(name: &str, url: &str) -> Result<Box<dyn Backend>> {
    Ok(match name {
        "clickhouse" => Box::new(ibis_clickhouse::ClickHouse::connect(url)?),
        "impala" => Box::new(ibis_impala::Impala::connect(url)?),
        "mysql" => Box::new(ibis_mysql::MySql::connect(url)?),
        "postgres" => Box::new(ibis_postgres::Postgres::connect(url)?),
        "spark" => Box::new(ibis_spark::Spark::connect(url)?),
        "trino" => Box::new(ibis_trino::Trino::connect(url)?),
        other => return Err(Error::Other(format!("{other:?} is not a server backend"))),
    })
}

/// Why `name` cannot be tested right now, if it cannot: its server is not
/// running. In-process backends are always available.
fn unavailable(name: &str) -> Option<String> {
    if !SERVERS.contains(&name) {
        return None;
    }
    server_url(name).and_then(|url| connect(name, &url)).err().map(|e| e.to_string())
}

/// The backends in `names` that are up. One that is not is skipped with a
/// note on stderr, so the suite runs with whichever containers are started
/// (`mise run up <backend>`). With `strict`, or `IBIS_TESTING_STRICT` set (CI
/// does), a backend that is down is an error instead.
pub fn available<'a>(names: &[&'a str], strict: bool) -> Result<Vec<&'a str>> {
    let strict = strict || std::env::var_os("IBIS_TESTING_STRICT").is_some_and(|v| !v.is_empty());
    let mut up = vec![];
    for name in names {
        match unavailable(name) {
            None => up.push(*name),
            Some(why) if strict => return Err(Error::Other(format!("{name} is not available: {why}"))),
            Some(why) => eprintln!("{name}: skipped, not available (`mise run up {name}`): {why}"),
        }
    }
    Ok(up)
}

/// Backends still being built out. When one of these says it does not support
/// an op (`Error::Unsupported`), the case is reported as `unsupported` and
/// does not fail the run; a wrong answer or any other error still does.
pub const PARTIAL: [&str; 1] = ["polars"];

/// Name of the pseudo-backend that checks output column names and types
/// against Ibis's, independent of any engine.
pub const SCHEMA: &str = "schema";

#[derive(Clone, Debug, PartialEq)]
enum Cell {
    Null,
    Num(f64),
    Bool(bool),
    Str(String),
    Ts(i64),
    /// An array's elements, a struct's field values in order, or a map's
    /// entries as `[key, value]` pairs sorted by key.
    List(Vec<Cell>),
}

impl Cell {
    fn from_value(v: Value) -> Cell {
        match v {
            Value::Null => Cell::Null,
            Value::Bool(b) => Cell::Bool(b),
            Value::Int(i) => Cell::Num(i as f64),
            Value::Float(f) => Cell::Num(f),
            Value::String(s) => Cell::Str(s),
            Value::Binary(b) => Cell::Str(format!("{b:?}")),
            // goldens from pandas may carry dates as timestamps
            Value::Date(d) => Cell::Ts(d as i64 * 86_400_000_000),
            Value::Timestamp(t) => Cell::Ts(t),
            Value::Array(items) => Cell::List(items.into_iter().map(Cell::from_value).collect()),
            Value::Struct(fields) => Cell::List(fields.into_iter().map(|(_, v)| Cell::from_value(v)).collect()),
            Value::Map(entries) => {
                let mut entries: Vec<Cell> = entries
                    .into_iter()
                    .map(|(k, v)| Cell::List(vec![Cell::from_value(k), Cell::from_value(v)]))
                    .collect();
                entries.sort_by_cached_key(Cell::key);
                Cell::List(entries)
            }
        }
    }

    /// Sort key; floats rounded so near-equal values sort together.
    fn key(&self) -> String {
        match self {
            Cell::Null => "\u{0}".into(),
            Cell::Num(f) if f.is_nan() => "NaN".into(),
            Cell::Num(f) => format!("{f:+.6e}"),
            Cell::List(items) => format!("[{}]", items.iter().map(Cell::key).collect::<Vec<_>>().join(",")),
            other => format!("{other:?}"),
        }
    }

    fn approx_eq(&self, other: &Cell) -> bool {
        match (self, other) {
            (Cell::Num(a), Cell::Num(b)) => {
                (a.is_nan() && b.is_nan()) || a == b || (a - b).abs() <= 1e-9 + 1e-6 * a.abs().max(b.abs())
            }
            (Cell::List(a), Cell::List(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.approx_eq(y)),
            (a, b) => a == b,
        }
    }
}

fn canon(batches: &[RecordBatch], ordered: bool) -> Result<Vec<Vec<Cell>>> {
    let mut rows = vec![];
    for b in batches {
        for r in batch_to_rows(b)? {
            rows.push(r.into_iter().map(Cell::from_value).collect::<Vec<_>>());
        }
    }
    if !ordered {
        rows.sort_by_cached_key(|r| r.iter().map(Cell::key).collect::<Vec<_>>());
    }
    Ok(rows)
}

/// None if equal, else a short description of the first difference.
fn diff(want: &[Vec<Cell>], got: &[Vec<Cell>]) -> Option<Json> {
    if want.len() != got.len() {
        return Some(json!({"want_rows": want.len(), "got_rows": got.len()}));
    }
    for (i, (w, g)) in want.iter().zip(got).enumerate() {
        if w.len() != g.len() || w.iter().zip(g).any(|(a, b)| !a.approx_eq(b)) {
            return Some(json!({"row": i, "want": format!("{w:?}"), "got": format!("{g:?}")}));
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Pass,
    /// Marked notimpl and didn't pass: expected.
    Xfail,
    /// Marked notimpl but passed: remove the mark.
    Xpass,
    /// Wrong result.
    Fail,
    /// Build, compile or execution error.
    Error,
    /// No golden for this id (Ibis itself failed, or goldens not regenerated).
    NoGolden,
    /// A backend in [`PARTIAL`] reported the op as not supported yet.
    Unsupported,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Xfail => "xfail",
            Status::Xpass => "xpass",
            Status::Fail => "fail",
            Status::Error => "error",
            Status::NoGolden => "no_golden",
            Status::Unsupported => "unsupported",
        }
    }
    /// Whether this outcome should fail the suite.
    pub fn is_problem(self) -> bool {
        matches!(self, Status::Xpass | Status::Fail | Status::Error | Status::NoGolden)
    }
}

pub struct Outcome {
    pub id: String,
    pub backend: String,
    pub status: Status,
    pub sql: Option<String>,
    pub detail: Json,
}

impl Outcome {
    pub fn to_json(&self) -> Json {
        json!({"id": self.id, "backend": self.backend, "status": self.status.as_str(), "sql": self.sql, "detail": self.detail})
    }
}

fn load_index(paths: &Paths) -> Result<BTreeMap<String, Json>> {
    let p = paths.golden.join("index.json");
    let text = std::fs::read_to_string(&p).map_err(|e| Error::Other(format!("read {}: {e}", p.display())))?;
    serde_json::from_str(&text).map_err(|e| Error::Other(format!("parse {}: {e}", p.display())))
}

/// Column names and Ibis types of the case's output, against Ibis's.
fn check_schema(case: &Case, fx: &Fx, meta: &Json) -> Outcome {
    let mut out =
        Outcome { id: case.id.clone(), backend: SCHEMA.into(), status: Status::Pass, sql: None, detail: Json::Null };
    let strs = |k: &str| -> Vec<String> {
        meta[k].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
    };
    // Ibis timestamps carry a scale ("timestamp(6)"); ours don't yet.
    let unscaled = |t: String| match t.strip_prefix("timestamp(") {
        Some(rest) if rest.len() == 2 && rest.ends_with(')') => "timestamp".to_string(),
        _ => t,
    };
    // nor nullability: Ibis writes a type that cannot be NULL as `!int64`
    let nullable = |t: String| t.replace('!', "");
    let want: Vec<(String, String)> =
        strs("columns").into_iter().zip(strs("dtypes").into_iter().map(unscaled).map(nullable)).collect();
    let passed = match case.build(fx) {
        Ok(t) => {
            let got: Vec<(String, String)> =
                t.schema().fields.iter().map(|f| (f.name.clone(), f.dtype.to_string())).collect();
            if got != want {
                out.status = Status::Fail;
                out.detail = json!({"want": format!("{want:?}"), "got": format!("{got:?}")});
            }
            got == want
        }
        Err(e) => {
            out.status = Status::Error;
            out.detail = json!(e.to_string());
            false
        }
    };
    if case.notimpl.contains(&SCHEMA) {
        out.status = if passed { Status::Xpass } else { Status::Xfail };
    }
    out
}

fn error_status(backend: &str, e: &Error) -> Status {
    if matches!(e, Error::Unsupported { .. }) && PARTIAL.contains(&backend) {
        Status::Unsupported
    } else {
        Status::Error
    }
}

fn run_one(case: &Case, target: &Target, meta: &Json, paths: &Paths) -> Outcome {
    let backend = target.backend.as_ref();
    let mut out = Outcome {
        id: case.id.clone(),
        backend: backend.name().into(),
        status: Status::Pass,
        sql: None,
        detail: Json::Null,
    };
    let Some(file) = meta.get("file").and_then(Json::as_str) else {
        out.status = Status::NoGolden;
        return out;
    };
    let result = (|| -> Result<Option<Json>> {
        let expr = case.build(&target.fx)?;
        out.sql = Some(backend.compile(&expr)?);
        let got = backend.execute(&expr)?;
        let want = read_parquet(&paths.golden.join(file))?;
        Ok(diff(&canon(&want, case.ordered)?, &canon(&got, case.ordered)?))
    })();
    let passed = match result {
        Ok(None) => true,
        Ok(Some(d)) => {
            out.status = Status::Fail;
            out.detail = d;
            false
        }
        Err(e) => {
            out.status = error_status(backend.name(), &e);
            out.detail = json!(e.to_string());
            false
        }
    };
    if !passed {
        // context: does Python Ibis itself match its DuckDB result on this backend?
        if let Some(py) = meta.get("ibis").and_then(|m| m.get(backend.name())) {
            out.detail = json!({"problem": out.detail, "python_ibis": py});
        }
    }
    if case.notimpl.contains(&backend.name()) {
        out.status = if passed { Status::Xpass } else { Status::Xfail };
    }
    out
}

/// Run every case whose id contains `filter`: the schema check once, then on
/// every target.
pub fn run(paths: &Paths, data: &FixtureData, targets: &[Target], filter: &str) -> Result<Vec<Outcome>> {
    let index = load_index(paths)?;
    let unbound = data.unbound()?;
    let golden = |case: &Case| index.get(&case.id).filter(|m| m.get("file").is_some());
    // Each target runs the cases on its own thread, with its own copy of them
    // (a case holds closures, which do not cross threads).
    let mut per_target: Vec<_> = each(targets, |t| {
        let cases = cases::all();
        let run = |case: &Case| golden(case).map(|meta| run_one(case, t, meta, paths));
        cases.iter().filter(|c| c.id.contains(filter)).filter_map(run).collect::<Vec<_>>()
    })
    .into_iter()
    .map(Vec::into_iter)
    .collect();
    // in case order: the schema check, then each target's outcome
    let mut outcomes = vec![];
    for case in cases::all().iter().filter(|c| c.id.contains(filter)) {
        let Some(meta) = golden(case) else {
            outcomes.push(Outcome {
                id: case.id.clone(),
                backend: SCHEMA.into(),
                status: Status::NoGolden,
                sql: None,
                detail: index.get(&case.id).and_then(|m| m.get("error")).cloned().unwrap_or(Json::Null),
            });
            continue;
        };
        outcomes.push(check_schema(case, &unbound, meta));
        outcomes.extend(per_target.iter_mut().filter_map(Iterator::next));
    }
    Ok(outcomes)
}

/// Case ids in the golden index with no Rust case: ported in Python only.
pub fn unported(paths: &Paths) -> Result<Vec<String>> {
    let index = load_index(paths)?;
    let have: std::collections::BTreeSet<String> = cases::all().into_iter().map(|c| c.id).collect();
    Ok(index.keys().filter(|k| *k != "_meta" && !have.contains(*k)).cloned().collect())
}

/// Per backend, how many indexed cases Python Ibis itself answers the same
/// as on DuckDB: (same, total).
pub fn python_ibis_agreement(paths: &Paths) -> Result<BTreeMap<String, (usize, usize)>> {
    let mut out: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (k, meta) in load_index(paths)? {
        if k == "_meta" {
            continue;
        }
        if let Some(m) = meta.get("ibis").and_then(Json::as_object) {
            for (backend, v) in m {
                let e = out.entry(backend.clone()).or_default();
                e.0 += (v.as_str() == Some("same")) as usize;
                e.1 += 1;
            }
        }
    }
    Ok(out)
}

const TPCH_TABLES: [&str; 8] = ["customer", "lineitem", "nation", "orders", "part", "partsupp", "region", "supplier"];

/// Open the TPC-H database for `name` (generated by `bench/tpch_data.py 0.01`).
pub fn tpch_backend(paths: &Paths, name: &str) -> Result<Box<dyn Backend>> {
    let file = |f: &str| {
        let p = paths.tpch.join(f);
        if p.exists() {
            Ok(p.display().to_string())
        } else {
            Err(Error::Other(format!("{} missing; run `mise run setup`", p.display())))
        }
    };
    Ok(match name {
        "duckdb" => Box::new(ibis_duckdb::DuckDb::open(&file("tpch.duckdb")?)?),
        "sqlite" => Box::new(ibis_sqlite::Sqlite::open(&file("tpch.sqlite")?)?),
        "postgres" => connect(name, &format!("{} dbname=tpch_sf0_01", server_url(name)?))?,
        "polars" => {
            let b = ibis_polars::Polars::new();
            for t in TPCH_TABLES {
                b.create_table(t, &read_parquet(Path::new(&file(&format!("{t}.parquet"))?))?)?;
            }
            Box::new(b)
        }
        // loaded from the parquet files on first use, into a database of its
        // own (a schema, in Trino); the URL's last segment names it
        "clickhouse" | "impala" | "mysql" | "spark" | "trino" => {
            let url = server_url(name)?;
            let create =
                format!("CREATE {} IF NOT EXISTS tpch_sf0_01", if name == "trino" { "SCHEMA" } else { "DATABASE" });
            match name {
                "clickhouse" => ibis_clickhouse::ClickHouse::connect(&url)?.execute_statement(&create)?,
                "impala" => ibis_impala::Impala::connect(&url)?.execute_statement(&create)?,
                "mysql" => ibis_mysql::MySql::connect(&url)?.execute_statement(&create)?,
                "spark" => ibis_spark::Spark::connect(&url)?.execute_statement(&create)?,
                _ => ibis_trino::Trino::connect(&url)?.execute_statement(&create)?,
            }
            let server = url.rsplit_once('/').map_or(url.as_str(), |(server, _)| server);
            let b = connect(name, &format!("{server}/tpch_sf0_01"))?;
            let have = b.list_tables()?;
            for t in TPCH_TABLES {
                if !have.iter().any(|h| h == t) {
                    b.create_table(t, &read_parquet(Path::new(&file(&format!("{t}.parquet"))?))?)?;
                }
            }
            b
        }
        other => return Err(Error::Other(format!("unknown backend {other:?}"))),
    })
}

/// Run Ibis's TPC-H queries (ids `tpch::test_NN`, as in Ibis's test suite)
/// on each named backend, comparing to conformance/golden/tpch.
pub fn run_tpch(paths: &Paths, names: &[&str], filter: &str) -> Result<Vec<Outcome>> {
    let queries: Vec<_> = tpch::QUERIES
        .iter()
        .map(|(q, f)| (format!("tpch::test_{}", &q[1..]), *q, *f))
        .filter(|(id, _, _)| id.contains(filter))
        .collect();
    if queries.is_empty() {
        return Ok(vec![]);
    }
    let per_backend = each(names, |name| -> Result<Vec<Outcome>> {
        let mut outcomes = vec![];
        let backend = tpch_backend(paths, name)?;
        let db = tpch::Tpch::load(backend.as_ref())?;
        for (id, q, f) in &queries {
            let mut out = Outcome {
                id: id.clone(),
                backend: name.to_string(),
                status: Status::Pass,
                sql: None,
                detail: Json::Null,
            };
            // per backend: parity with Python Ibis on the same engine and data
            // engines loaded from the parquet files DuckDB was built from share DuckDB's goldens
            let golden_dir = if matches!(*name, "polars" | "clickhouse" | "impala" | "mysql" | "spark" | "trino") {
                "duckdb"
            } else {
                name
            };
            let golden = paths.golden.join(format!("tpch/{golden_dir}/{q}.parquet"));
            if !golden.exists() {
                out.status = Status::NoGolden;
                outcomes.push(out);
                continue;
            }
            let result = (|| -> Result<Option<Json>> {
                let expr = f(&db)?;
                out.sql = Some(backend.compile(&expr)?);
                let got = backend.execute(&expr)?;
                let want = read_parquet(&golden)?;
                Ok(diff(&canon(&want, true)?, &canon(&got, true)?))
            })();
            match result {
                Ok(None) => {}
                Ok(Some(d)) => {
                    out.status = Status::Fail;
                    out.detail = d;
                }
                Err(e) => {
                    out.status = error_status(name, &e);
                    out.detail = json!(e.to_string());
                }
            }
            outcomes.push(out);
        }
        Ok(outcomes)
    });
    Ok(per_backend.into_iter().collect::<Result<Vec<_>>>()?.into_iter().flatten().collect())
}
