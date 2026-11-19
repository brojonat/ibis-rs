//! Apache Impala backend for ibis-rs.
//!
//! Statements go to an `impalad` over the HiveServer2 protocol (port 21050),
//! the one `impala-shell` and the JDBC driver use.
//!
//! What shapes the dialect:
//! - A number with a decimal point is an exact decimal however it is
//!   written, so floats are cast to `DOUBLE`.
//! - No `FILTER (WHERE ...)` on aggregates, no `INTERSECT ALL` or `EXCEPT
//!   ALL`, and no `NTH_VALUE`: the first two are rewritten, the last is
//!   reported as unsupported.
//! - Identifiers are case-insensitive and stored in lower case, so
//!   [`Backend::table`] reports a column created as `playerID` as `playerid`.
//! - String literals read backslash escapes, and two adjacent literals are
//!   one string.
//! - There is no bulk load in the protocol. [`Backend::create_table`] sends
//!   the rows inside `INSERT` statements, as text that Impala splits and
//!   casts: fine for test data and small tables, not a way to load a
//!   warehouse. Tables are stored as Parquet.

mod dialect;
#[cfg(test)]
mod tests;

use std::sync::Mutex;

use arrow_array::RecordBatch;
use ibis_core::convert::{batch_to_rows, format_date, format_timestamp, parse_date, parse_timestamp, rows_to_batch};
use ibis_core::{Backend, DataType, Error, Field, Result, Schema, Table, Value};
use ibis_hiveserver2::{Connection, Options, ResultSet, Values};
use ibis_sql::{Dialect, SqlClient, compile};

pub use dialect::ImpalaDialect;
use dialect::quote_str;

/// Rows are inserted in statements of about this many bytes,
const INSERT_BYTES: usize = 1 << 17;
/// and no more than this many rows: what four digits count.
const INSERT_ROWS: usize = 10_000;

pub struct Impala {
    conn: Mutex<Connection>,
    dialect: ImpalaDialect,
}

fn exec_err(sql: &str, message: impl std::fmt::Display) -> Error {
    Error::Execution { backend: "impala".into(), sql: sql.into(), message: message.to_string().trim().to_string() }
}

/// A type as `DESCRIBE` and result metadata spell it -> Ibis type.
fn parse_type(name: &str) -> DataType {
    let name = name.to_ascii_lowercase();
    match name.as_str() {
        "boolean" => DataType::Boolean,
        "tinyint" => DataType::Int8,
        "smallint" => DataType::Int16,
        "int" | "integer" => DataType::Int32,
        "bigint" => DataType::Int64,
        "float" | "real" => DataType::Float32,
        "double" => DataType::Float64,
        "date" => DataType::Date,
        "binary" => DataType::Binary,
        "null" => DataType::Null,
        "timestamp" => DataType::Timestamp { tz: None },
        _ => {
            if let Some(args) = name.strip_prefix("decimal(").and_then(|s| s.strip_suffix(')'))
                && let Some((p, s)) = args.split_once(',')
                && let (Ok(precision), Ok(scale)) = (p.trim().parse(), s.trim().parse())
            {
                return DataType::Decimal { precision, scale };
            }
            DataType::String
        }
    }
}

/// A result as rows, each cell read according to its column's type.
fn rows(result: ResultSet) -> Vec<Vec<Value>> {
    let mut rows = vec![Vec::with_capacity(result.columns.len()); result.num_rows()];
    for column in result.columns {
        let dtype = parse_type(&column.type_name);
        macro_rules! fill {
            ($values:expr, $convert:expr) => {
                for (row, v) in rows.iter_mut().zip($values) {
                    row.push(v.map_or(Value::Null, $convert));
                }
            };
        }
        match column.values {
            Values::Bool(values) => fill!(values, Value::Bool),
            Values::Int(values) => fill!(values, Value::Int),
            Values::Double(values) => fill!(values, Value::Float),
            Values::Binary(values) => fill!(values, Value::Binary),
            // dates, timestamps and decimals arrive as text
            Values::String(values) => fill!(values, |s: String| match dtype {
                DataType::Date => parse_date(&s).map_or(Value::String(s), Value::Date),
                DataType::Timestamp { .. } => parse_timestamp(&s).map_or(Value::String(s), Value::Timestamp),
                _ => Value::String(s),
            }),
        }
    }
    rows
}

/// One value as the text a cast to `dtype` reads back; `None` is NULL.
fn insert_text(v: &Value, dtype: &DataType) -> Option<String> {
    Some(match v {
        Value::Null => return None,
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) if f.is_nan() => "NaN".into(),
        Value::Float(f) if f.is_infinite() => if *f > 0.0 { "Infinity" } else { "-Infinity" }.into(),
        // a decimal does not read an exponent
        Value::Float(f) => match dtype {
            DataType::Decimal { scale, .. } => format!("{f:.*}", (*scale).max(0) as usize),
            _ => format!("{f:?}"),
        },
        Value::String(s) => s.clone(),
        Value::Binary(b) => b.iter().map(|byte| format!("{byte:02x}")).collect(),
        Value::Date(d) => format_date(*d),
        Value::Timestamp(t) => format_timestamp(*t),
        // TODO(nested)
        Value::Array(_) | Value::Struct(_) | Value::Map(_) => return None,
    })
}

/// Characters that can separate cells, rows and mark NULL inside an insert
/// statement's one string: control characters no ordinary text contains.
const SEPARATORS: [char; 25] = [
    '\x01', '\x02', '\x03', '\x04', '\x05', '\x06', '\x07', '\x08', '\x0b', '\x0e', '\x0f', '\x10', '\x11', '\x12',
    '\x13', '\x14', '\x15', '\x16', '\x17', '\x18', '\x19', '\x1a', '\x1c', '\x1d', '\x1e',
];

/// The rows one `INSERT` statement carries: each cell as text, or `None` for NULL.
type Chunk = Vec<Vec<Option<String>>>;

/// The `INSERT` statement carrying `chunk` into the table `q`.
///
/// All of it travels as one string literal. Impala plans an expression for
/// every cell of a `VALUES` list and keeps the plans of finished queries:
/// some tens of megabytes of rows sent that way cost the daemon gigabytes.
/// Here the plan is the same size whatever the row count: the literal is cut
/// into rows by joining it to the numbers 1 to 10,000, each row is cut into
/// cells, and each cell is cast to its column's type. The `GROUP BY` changes
/// no rows (the number makes each distinct). It is there because a subquery's
/// expressions are otherwise pasted into every use of them, and the search
/// for row `n` in the whole text would run again for each cell.
fn insert_statement(d: &ImpalaDialect, q: &str, schema: &Schema, chunk: &Chunk) -> Result<String> {
    let used = |c: char| chunk.iter().flatten().flatten().any(|text| text.contains(c));
    let mut free = SEPARATORS.into_iter().filter(|c| !used(*c));
    let (Some(cell), Some(row), Some(null)) = (free.next(), free.next(), free.next()) else {
        return Err(Error::Unsupported {
            backend: "impala".into(),
            op: "inserting strings that between them hold nearly every control character".into(),
        });
    };
    let mut text = String::new();
    for (i, cells) in chunk.iter().enumerate() {
        if i > 0 {
            text.push(row);
        }
        for (j, value) in cells.iter().enumerate() {
            if j > 0 {
                text.push(cell);
            }
            match value {
                Some(value) => text.push_str(value),
                None => text.push(null),
            }
        }
    }
    let select = schema
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let value = format!("NULLIF(SPLIT_PART(r, '{cell}', {}), '{null}')", i + 1);
            Ok(match f.dtype {
                DataType::String => value,
                DataType::Binary => format!("CAST(UNHEX({value}) AS BINARY)"),
                // no cast from text to BOOLEAN
                DataType::Boolean => format!("({value} = 'true')"),
                _ => format!("CAST({value} AS {})", d.type_name(&f.dtype)?),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let digits = (1..10).fold("SELECT 0 AS d".to_string(), |s, i| format!("{s} UNION ALL SELECT {i}"));
    Ok(format!(
        "WITH d AS ({digits}) INSERT INTO {q} SELECT {} FROM (SELECT n.n, SPLIT_PART(v.rows_text, '{row}', n.n) AS r \
         FROM (SELECT {} AS rows_text) v CROSS JOIN (SELECT a.d * 1000 + b.d * 100 + c.d * 10 + e.d + 1 AS n \
         FROM d a CROSS JOIN d b CROSS JOIN d c CROSS JOIN d e) n WHERE n.n <= {} GROUP BY 1, 2) t",
        select.join(", "),
        quote_str(&text),
        chunk.len()
    ))
}

/// `INSERT` statements carrying `data` into the table `q`, whose columns are `schema`'s.
fn insert_statements(d: &ImpalaDialect, q: &str, schema: &Schema, data: &[RecordBatch]) -> Result<Vec<String>> {
    let mut statements = vec![];
    let mut chunk: Chunk = vec![];
    let mut bytes = 0;
    for b in data {
        for row in batch_to_rows(b)? {
            let cells: Vec<Option<String>> =
                row.iter().zip(&schema.fields).map(|(v, f)| insert_text(v, &f.dtype)).collect();
            bytes += cells.iter().map(|c| c.as_ref().map_or(1, String::len) + 1).sum::<usize>();
            chunk.push(cells);
            if bytes >= INSERT_BYTES || chunk.len() == INSERT_ROWS {
                statements.push(insert_statement(d, q, schema, &chunk)?);
                chunk.clear();
                bytes = 0;
            }
        }
    }
    if !chunk.is_empty() {
        statements.push(insert_statement(d, q, schema, &chunk)?);
    }
    Ok(statements)
}

impl Impala {
    /// Connect to an `impalad`:
    /// `impala://[user[:password]@]host[:port][/database][?auth=plain]`.
    /// The port defaults to 21050. The connection is a bare socket, which is
    /// what an Impala without authentication expects; `?auth=plain` does the
    /// SASL `PLAIN` handshake of one set up for LDAP.
    pub fn connect(url: &str) -> Result<Self> {
        let bad = |why: &dyn std::fmt::Display| {
            Error::Invalid(format!(
                "impala: expected impala://[user[:password]@]host[:port][/database][?auth=plain], got {url:?} ({why})"
            ))
        };
        if !url.starts_with("impala://") {
            return Err(bad(&"not an impala:// URL"));
        }
        let options = Options::from_url(url, 21050, false).map_err(|e| bad(&e))?;
        let conn = Connection::open(&options).map_err(|e| Error::Other(format!("impala: {url}: {e}")))?;
        Ok(Impala { conn: Mutex::new(conn), dialect: ImpalaDialect })
    }

    fn run(&self, sql: &str) -> Result<ResultSet> {
        self.conn.lock().unwrap().execute(sql).map_err(|e| exec_err(sql, e))
    }

    /// Run one statement that returns no rows.
    pub fn execute_statement(&self, sql: &str) -> Result<()> {
        self.run(sql).map(|_| ())
    }

    /// Run a query, returning rows typed by what Impala says each column is.
    pub fn raw_sql(&self, sql: &str) -> Result<Vec<Vec<Value>>> {
        self.run(sql).map(rows)
    }
}

impl SqlClient for Impala {
    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    fn statement(&self, sql: &str) -> Result<()> {
        self.execute_statement(sql)
    }

    fn load(&self, name: &str, schema: &Schema, data: &[RecordBatch]) -> Result<()> {
        for statement in insert_statements(&self.dialect, &self.dialect.quote_ident(name), schema, data)? {
            self.execute_statement(&statement)?;
        }
        Ok(())
    }
}

impl Backend for Impala {
    ibis_sql::sql_client_methods!();
    fn name(&self) -> &str {
        "impala"
    }

    fn compile(&self, table: &Table) -> Result<String> {
        compile(&self.dialect, table)
    }

    fn execute(&self, table: &Table) -> Result<Vec<RecordBatch>> {
        let rows = self.raw_sql(&self.compile(table)?)?;
        Ok(vec![rows_to_batch(table.schema(), &rows)?])
    }

    fn table(&self, name: &str) -> Result<Table> {
        let q = self.dialect.quote_ident(name);
        // name, type, comment
        let described = self
            .raw_sql(&format!("DESCRIBE {q}"))
            .map_err(|e| Error::Other(format!("impala: no table {name:?}: {e}")))?;
        let fields = described
            .iter()
            .filter_map(|r| match (r.first(), r.get(1)) {
                (Some(Value::String(col)), Some(Value::String(ty))) => Some(Field::new(col, parse_type(ty))),
                _ => None,
            })
            .collect::<Vec<_>>();
        Ok(Table::named(name, Schema::new(fields)))
    }

    fn list_tables(&self) -> Result<Vec<String>> {
        let mut names: Vec<String> = self
            .raw_sql("SHOW TABLES")?
            .into_iter()
            .filter_map(|r| match r.into_iter().next() {
                Some(Value::String(s)) => Some(s),
                _ => None,
            })
            .collect();
        names.sort();
        Ok(names)
    }
}
