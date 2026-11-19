//! The plugin boundary: what every backend implements.

use std::sync::atomic::{AtomicUsize, Ordering};

use arrow_array::RecordBatch;

use crate::datatypes::Schema;
use crate::error::{Error, Result};
use crate::table::Table;

/// How a table is created (Ibis `create_table(..., overwrite=, temp=)`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CreateOptions {
    /// Replace a table of the same name. Without it, a table that is
    /// already there is an error.
    pub overwrite: bool,
    /// A temporary table: one that goes when the connection does.
    pub temp: bool,
}

impl CreateOptions {
    /// A lasting table that replaces one of the same name.
    pub const REPLACE: CreateOptions = CreateOptions { overwrite: true, temp: false };
}

/// A name for a table or view the library creates and drops again.
pub fn scratch_name(purpose: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!("ibis_{purpose}_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// A backend compiles the IR to something its engine runs, runs it, and
/// returns Arrow data shaped like the Ibis output schema (`table.schema()`).
pub trait Backend: Send + Sync {
    /// Short identifier: "duckdb", "sqlite", ...
    fn name(&self) -> &str;

    /// The query this backend would run, for inspection (Ibis `to_sql`).
    fn compile(&self, table: &Table) -> Result<String>;

    /// Run the query. The returned batches use `table.schema()` as their schema.
    fn execute(&self, table: &Table) -> Result<Vec<RecordBatch>>;

    /// Bind a catalog table, reading its schema from the backend.
    fn table(&self, name: &str) -> Result<Table>;

    /// Create a table with this schema and fill it with `data` (which may
    /// be empty). A column whose field is not nullable is created `NOT
    /// NULL`. The data's columns are matched to the schema's by name, or by
    /// position when the names differ, and cast to their types.
    fn create_table_with(
        &self,
        name: &str,
        schema: &Schema,
        data: &[RecordBatch],
        options: CreateOptions,
    ) -> Result<()>;

    fn list_tables(&self) -> Result<Vec<String>>;

    /// Create (or replace) a table from Arrow data, with the data's schema.
    fn create_table(&self, name: &str, data: &[RecordBatch]) -> Result<()> {
        let Some(first) = data.first() else {
            return Err(Error::Invalid("create_table needs at least one batch (for the schema)".into()));
        };
        self.create_table_with(name, &Schema::from_arrow(&first.schema())?, data, CreateOptions::REPLACE)
    }

    /// Create an empty table with this schema, replacing one of the same
    /// name (Ibis `con.create_table(name, schema=...)`).
    fn create_empty_table(&self, name: &str, schema: &Schema) -> Result<()> {
        self.create_table_with(name, schema, &[], CreateOptions::REPLACE)
    }

    /// Create a table from the result of a query on this backend. The query
    /// is run before anything is dropped, so it may read the table it
    /// replaces.
    fn create_table_from(&self, name: &str, source: &Table, options: CreateOptions) -> Result<()> {
        let mut schema = source.schema().clone();
        // what a query returns can always be NULL
        schema.fields.iter_mut().for_each(|f| f.nullable = true);
        self.create_table_with(name, &schema, &self.execute(source)?, options)
    }

    /// Drop a table. With `force`, a table that is not there is not an error.
    fn drop_table(&self, name: &str, force: bool) -> Result<()> {
        let _ = (name, force);
        self.unsupported("drop_table")
    }

    /// Append rows to a table, or with `overwrite` replace its rows (Ibis
    /// `con.insert`). The data's columns are matched to the table's by
    /// name, or by position when the names differ, and cast to their types.
    fn insert(&self, name: &str, data: &[RecordBatch], overwrite: bool) -> Result<()> {
        let _ = (name, data, overwrite);
        self.unsupported("insert")
    }

    /// `insert` with the result of a query on this backend as the data.
    fn insert_from(&self, name: &str, source: &Table, overwrite: bool) -> Result<()> {
        self.insert(name, &self.execute(source)?, overwrite)
    }

    /// Update the rows of a table whose `on` column matches a row of
    /// `data`, and insert the rows of `data` that match none (Ibis
    /// `con.upsert`). The data may have only some of the table's columns:
    /// the others keep their values, and are NULL in inserted rows.
    fn upsert(&self, name: &str, data: &[RecordBatch], on: &str) -> Result<()> {
        // the data goes through a table of its own, so the engine can merge it
        let staging = scratch_name("upsert");
        self.create_table(&staging, data)?;
        let merged = self.table(&staging).and_then(|source| self.upsert_from(name, &source, on));
        self.drop_table(&staging, true)?;
        merged
    }

    /// `upsert` with the result of a query on this backend as the data.
    fn upsert_from(&self, name: &str, source: &Table, on: &str) -> Result<()> {
        let _ = (name, source, on);
        self.unsupported("upsert")
    }

    fn rename_table(&self, old: &str, new: &str) -> Result<()> {
        let _ = (old, new);
        self.unsupported("rename_table")
    }

    /// Define a view over a query. With `overwrite`, a view of the same
    /// name is replaced; without, it is an error.
    fn create_view(&self, name: &str, source: &Table, overwrite: bool) -> Result<()> {
        let _ = (name, source, overwrite);
        self.unsupported("create_view")
    }

    /// Drop a view. With `force`, a view that is not there is not an error.
    fn drop_view(&self, name: &str, force: bool) -> Result<()> {
        let _ = (name, force);
        self.unsupported("drop_view")
    }

    /// A query in the backend's own SQL as a table expression (Ibis
    /// `con.sql`): its columns are read from the backend, and further
    /// operations compile around it.
    fn sql(&self, query: &str) -> Result<Table> {
        let _ = query;
        Err(Error::Unsupported { backend: self.name().to_string(), op: "sql".into() })
    }

    #[doc(hidden)]
    fn unsupported(&self, op: &str) -> Result<()> {
        Err(Error::Unsupported { backend: self.name().to_string(), op: op.to_string() })
    }
}
