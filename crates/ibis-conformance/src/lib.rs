//! Conformance runner: every ported Ibis case, on every backend, compared to
//! goldens produced by Python Ibis (conformance/golden).

pub mod cases;

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use arrow_array::RecordBatch;
use ibis_core::convert::batch_to_rows;
use ibis_core::prelude::*;
use ibis_core::{Error, Result, Value};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde_json::{Value as Json, json};

use crate::cases::{Case, Shape};

pub const FIXTURE: &str = "functional_alltypes";

/// Paths the runner needs, resolved from the repo root.
pub struct Paths {
    pub golden: PathBuf,
    pub testing_data: PathBuf,
}

impl Paths {
    pub fn from_repo_root(root: &Path) -> Self {
        let testing_data = std::env::var_os("IBIS_TESTING_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("ci/ibis-testing-data"));
        Paths { golden: root.join("conformance/golden"), testing_data }
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

/// A backend under test, loaded with the Ibis fixture tables.
pub fn backends(paths: &Paths, names: &[&str]) -> Result<Vec<Box<dyn Backend>>> {
    let fixture = read_parquet(&paths.testing_data.join(format!("parquet/{FIXTURE}.parquet")))?;
    names
        .iter()
        .map(|n| {
            let b: Box<dyn Backend> = match *n {
                "duckdb" => Box::new(ibis_duckdb::DuckDb::open_in_memory()?),
                "sqlite" => Box::new(ibis_sqlite::Sqlite::open_in_memory()?),
                "postgres" => {
                    let url = std::env::var("IBIS_TESTING_POSTGRES_URL").map_err(|_| {
                        Error::Other("postgres: set IBIS_TESTING_POSTGRES_URL (mise run pg-start does it)".into())
                    })?;
                    Box::new(ibis_postgres::Postgres::connect(&url)?)
                }
                other => return Err(Error::Other(format!("unknown backend {other:?}"))),
            };
            b.create_table(FIXTURE, &fixture)?;
            Ok(b)
        })
        .collect()
}

pub const ALL_BACKENDS: [&str; 3] = ["duckdb", "postgres", "sqlite"];

/// Build the table expression a case executes.
pub fn build(case: &Case, t: &Table) -> Result<Table> {
    match &case.shape {
        Shape::Table(f) => f(t),
        Shape::Column(f) => {
            let e = f(t)?.name("tmp");
            if e.is_reduction() { t.aggregate([e]) } else { t.select([col("id"), e]) }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Cell {
    Null,
    Num(f64),
    Bool(bool),
    Str(String),
    Date(i32),
    Ts(i64),
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
            Value::Date(d) => Cell::Date(d),
            Value::Timestamp(t) => Cell::Ts(t),
        }
    }

    /// Sort key; floats rounded so near-equal values sort together.
    fn key(&self) -> String {
        match self {
            Cell::Null => "\u{0}".into(),
            Cell::Num(f) if f.is_nan() => "NaN".into(),
            Cell::Num(f) => format!("{f:+.6e}"),
            other => format!("{other:?}"),
        }
    }

    fn approx_eq(&self, other: &Cell) -> bool {
        match (self, other) {
            (Cell::Num(a), Cell::Num(b)) => {
                (a.is_nan() && b.is_nan()) || a == b || (a - b).abs() <= 1e-9 + 1e-6 * a.abs().max(b.abs())
            }
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

fn run_one(case: &Case, backend: &dyn Backend, index: &BTreeMap<String, Json>, paths: &Paths) -> Outcome {
    let mut out = Outcome {
        id: case.id.clone(),
        backend: backend.name().into(),
        status: Status::Pass,
        sql: None,
        detail: Json::Null,
    };
    let Some(file) = index.get(&case.id).and_then(|m| m.get("file")).and_then(Json::as_str) else {
        out.status = Status::NoGolden;
        return out;
    };
    let result = (|| -> Result<Option<Json>> {
        let t = backend.table(FIXTURE)?;
        let expr = build(case, &t)?;
        out.sql = backend.compile(&expr).ok();
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
            out.status = Status::Error;
            out.detail = json!(e.to_string());
            false
        }
    };
    if case.notimpl.contains(&backend.name()) {
        out.status = if passed { Status::Xpass } else { Status::Xfail };
    }
    out
}

/// Run every case whose id contains `filter` on every backend.
pub fn run(paths: &Paths, backends: &[Box<dyn Backend>], filter: &str) -> Result<Vec<Outcome>> {
    let index = load_index(paths)?;
    let mut outcomes = vec![];
    for case in cases::all().iter().filter(|c| c.id.contains(filter)) {
        for b in backends {
            outcomes.push(run_one(case, b.as_ref(), &index, paths));
        }
    }
    Ok(outcomes)
}
