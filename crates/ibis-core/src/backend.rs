//! The plugin boundary: what every backend implements.

use arrow_array::RecordBatch;

use crate::error::Result;
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
}
