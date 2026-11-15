//! ibis-rs: a portable lazy dataframe API, modeled on Python's
//! [Ibis](https://ibis-project.org). Write a query once against [`Table`] and
//! [`Expr`], then run it on whichever engine you connect to.
//!
//! This crate is the front door: it re-exports the API from `ibis-core` and
//! puts each backend behind a cargo feature. `polars` is on by default;
//! `duckdb`, `sqlite` and `postgres` are opt-in.
//!
//! ```toml
//! [dependencies]
//! ibis-rs = "0.1"                                                   # Polars
//! ibis-rs = { version = "0.1", features = ["sqlite", "postgres"] }  # more engines
//! ```
//!
//! ```
//! # #[cfg(feature = "polars")]
//! # fn main() -> ibis_rs::Result<()> {
//! use ibis_rs::prelude::*;
//!
//! // "sqlite://app.db" or "postgres://user@host/db" runs the same query there.
//! let con = ibis_rs::connect("polars://")?;
//! # use std::sync::Arc;
//! # use ibis_rs::arrow_array::{Float64Array, RecordBatch, StringArray};
//! # let batch = RecordBatch::try_from_iter([
//! #     ("region", Arc::new(StringArray::from(vec!["eu", "eu", "us"])) as _),
//! #     ("amount", Arc::new(Float64Array::from(vec![50.0, 150.0, 200.0])) as _),
//! # ])?;
//! # con.create_table("orders", &[batch])?;
//! let orders = con.table("orders")?;
//!
//! let expr = orders
//!     .filter([col("amount").gt(lit(100.0))])?
//!     .group_by(["region"])
//!     .agg([col("amount").sum().name("total"), count().name("n")])?
//!     .order_by([desc("total")])?;
//!
//! println!("{}", con.compile(&expr)?); // the SQL (or Polars plan) this backend will run
//! let batches = con.execute(&expr)?; // Arrow record batches
//! # assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 2);
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "polars"))]
//! # fn main() {}
//! ```
//!
//! Backend-specific methods (`raw_sql`, registering a Polars `DataFrame`,
//! opening from an existing connection) live on the concrete types:
//! [`polars::Polars`], [`sqlite::Sqlite`], [`duckdb::DuckDb`],
//! [`postgres::Postgres`].

pub use ibis_core::*;

/// The IR -> SQL compiler and the [`sql::Dialect`] trait, for writing a SQL backend.
pub use ibis_sql as sql;

/// Arrow arrays, the type of query results. Re-exported so one dependency is enough.
pub use arrow_array;

#[cfg(feature = "duckdb")]
pub use ibis_duckdb as duckdb;
#[cfg(feature = "polars")]
pub use ibis_polars as polars;
#[cfg(feature = "postgres")]
pub use ibis_postgres as postgres;
#[cfg(feature = "sqlite")]
pub use ibis_sqlite as sqlite;

/// Connect to a backend by URL, like Python's `ibis.connect`.
///
/// | URL | backend | feature |
/// |---|---|---|
/// | `polars://` (in memory) | Polars | `polars` (default) |
/// | `duckdb://` (in memory), `duckdb://path/to/file.db` | DuckDB | `duckdb` |
/// | `sqlite://` (in memory), `sqlite://path/to/file.db` | SQLite | `sqlite` |
/// | `postgres://user:pass@host:5432/db`, `postgresql://...` | Postgres | `postgres` |
///
/// Everything after `duckdb://` or `sqlite://` is a file path, so an absolute
/// path has three slashes: `sqlite:///var/data/app.db`.
///
/// Take the URL from the environment to develop on one engine and deploy on
/// another without changing code.
pub fn connect(url: &str) -> Result<Box<dyn Backend>> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(Error::Other(format!(
            "connect: `{url}` is not a URL; expected polars://, duckdb://, sqlite:// or postgres://"
        )));
    };
    match scheme {
        "polars" => polars_at(rest),
        "duckdb" => duckdb_at(rest),
        "sqlite" => sqlite_at(rest),
        "postgres" | "postgresql" => postgres_at(url),
        other => Err(Error::Other(format!(
            "connect: unknown backend `{other}` in `{url}`; expected polars://, duckdb://, sqlite:// or postgres://"
        ))),
    }
}

#[allow(dead_code)]
fn disabled(backend: &str) -> Result<Box<dyn Backend>> {
    Err(Error::Other(format!(
        "connect: ibis-rs was built without the `{backend}` feature; enable it in Cargo.toml: \
         ibis-rs = {{ version = \"0.1\", features = [\"{backend}\"] }}"
    )))
}

#[allow(dead_code)]
fn in_memory(path: &str) -> bool {
    path.is_empty() || path == ":memory:"
}

#[cfg(feature = "polars")]
fn polars_at(rest: &str) -> Result<Box<dyn Backend>> {
    if !rest.is_empty() {
        return Err(Error::Other(format!("connect: polars:// takes no path (got `{rest}`); tables live in memory")));
    }
    Ok(Box::new(polars::Polars::new()))
}
#[cfg(not(feature = "polars"))]
fn polars_at(_: &str) -> Result<Box<dyn Backend>> {
    disabled("polars")
}

#[cfg(feature = "duckdb")]
fn duckdb_at(path: &str) -> Result<Box<dyn Backend>> {
    let con = if in_memory(path) { duckdb::DuckDb::open_in_memory()? } else { duckdb::DuckDb::open(path)? };
    Ok(Box::new(con))
}
#[cfg(not(feature = "duckdb"))]
fn duckdb_at(_: &str) -> Result<Box<dyn Backend>> {
    disabled("duckdb")
}

#[cfg(feature = "sqlite")]
fn sqlite_at(path: &str) -> Result<Box<dyn Backend>> {
    let con = if in_memory(path) { sqlite::Sqlite::open_in_memory()? } else { sqlite::Sqlite::open(path)? };
    Ok(Box::new(con))
}
#[cfg(not(feature = "sqlite"))]
fn sqlite_at(_: &str) -> Result<Box<dyn Backend>> {
    disabled("sqlite")
}

#[cfg(feature = "postgres")]
fn postgres_at(url: &str) -> Result<Box<dyn Backend>> {
    Ok(Box::new(postgres::Postgres::connect(url)?))
}
#[cfg(not(feature = "postgres"))]
fn postgres_at(_: &str) -> Result<Box<dyn Backend>> {
    disabled("postgres")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::prelude::*;
    use super::{Result, connect};
    use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};

    fn err(url: &str) -> String {
        match connect(url) {
            Ok(_) => panic!("{url} should not connect"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn rejects_what_it_cannot_dispatch() {
        assert!(err("app.db").contains("is not a URL"));
        assert!(err("mysql://localhost/db").contains("unknown backend `mysql`"));
    }

    #[cfg(not(feature = "duckdb"))]
    #[test]
    fn names_the_missing_feature() {
        assert!(err("duckdb://").contains("without the `duckdb` feature"));
    }

    /// The same query through `Box<dyn Backend>`, whichever engine is behind it.
    #[allow(dead_code)]
    fn totals(url: &str) -> Result<Vec<(String, f64, i64)>> {
        let con = connect(url)?;
        let batch = RecordBatch::try_from_iter([
            ("region", Arc::new(StringArray::from(vec!["eu", "eu", "us", "us"])) as Arc<dyn Array>),
            ("amount", Arc::new(Float64Array::from(vec![50.0, 150.0, 200.0, 300.0])) as Arc<dyn Array>),
        ])?;
        con.create_table("orders", &[batch])?;
        let expr = con
            .table("orders")?
            .filter([col("amount").gt(lit(100.0))])?
            .group_by(["region"])
            .agg([col("amount").sum().name("total"), count().name("n")])?
            .order_by([desc("total")])?;
        let mut out = Vec::new();
        for b in con.execute(&expr)? {
            let region = b.column(0).as_any().downcast_ref::<StringArray>().unwrap();
            let total = b.column(1).as_any().downcast_ref::<Float64Array>().unwrap();
            let n = b.column(2).as_any().downcast_ref::<Int64Array>().unwrap();
            for i in 0..b.num_rows() {
                out.push((region.value(i).to_string(), total.value(i), n.value(i)));
            }
        }
        Ok(out)
    }

    #[allow(dead_code)]
    fn expected() -> Vec<(String, f64, i64)> {
        vec![("us".into(), 500.0, 2), ("eu".into(), 150.0, 1)]
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_in_memory() {
        assert_eq!(totals("sqlite://").unwrap(), expected());
        assert_eq!(totals("sqlite://:memory:").unwrap(), expected());
    }

    #[cfg(feature = "polars")]
    #[test]
    fn polars_in_memory() {
        assert_eq!(totals("polars://").unwrap(), expected());
        assert!(err("polars://some/path").contains("takes no path"));
    }

    #[cfg(feature = "duckdb")]
    #[test]
    fn duckdb_in_memory() {
        assert_eq!(totals("duckdb://").unwrap(), expected());
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_file_path() {
        let path = std::env::temp_dir().join(format!("ibis-rs-connect-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let url = format!("sqlite://{}", path.display());
        assert_eq!(totals(&url).unwrap(), expected());
        // a second connection sees the table the first one wrote
        assert_eq!(connect(&url).unwrap().list_tables().unwrap(), ["orders"]);
        std::fs::remove_file(&path).unwrap();
    }

    /// Nothing listens on port 1: the error must come from the Postgres backend,
    /// which shows the URL was dispatched to it.
    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_urls_reach_the_postgres_backend() {
        for url in ["postgres://u@127.0.0.1:1/db", "postgresql://u@127.0.0.1:1/db"] {
            assert!(err(url).contains("postgres failed"), "{}", err(url));
        }
    }
}
