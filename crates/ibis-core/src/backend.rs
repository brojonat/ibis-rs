//! The plugin boundary: what every backend implements.

use std::sync::Arc;

use arrow_array::RecordBatch;

use crate::datatypes::Schema;
use crate::error::{Error, Result};
use crate::table::Table;

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

    /// Create (or replace) a table from Arrow data.
    fn create_table(&self, name: &str, data: &[RecordBatch]) -> Result<()>;

    fn list_tables(&self) -> Result<Vec<String>>;

    /// Create an empty table with this schema, replacing one of the same
    /// name (Ibis `con.create_table(name, schema=...)`).
    fn create_empty_table(&self, name: &str, schema: &Schema) -> Result<()> {
        self.create_table(name, &[RecordBatch::new_empty(Arc::new(schema.to_arrow()))])
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

    #[doc(hidden)]
    fn unsupported(&self, op: &str) -> Result<()> {
        Err(Error::Unsupported { backend: self.name().to_string(), op: op.to_string() })
    }
}
