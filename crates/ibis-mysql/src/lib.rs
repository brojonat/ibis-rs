//! MySQL backend for ibis-rs.
//!
//! Queries go over the text protocol and each result cell is read according
//! to the column type the server reports, then converted to the Ibis output
//! schema.
//!
//! What shapes the dialect:
//! - No boolean type (`TINYINT(1)`; a truth value is 0 or 1), no NaN or
//!   infinity, and no `FULL OUTER JOIN`.
//! - `CAST` has its own type names (`SIGNED`, `CHAR`, ...).
//! - No `NULLS FIRST`/`LAST`, no `FILTER (WHERE ...)` on aggregates, and `||`
//!   means OR: each is rewritten.
//!
//! Whether string columns compare case-sensitively is the column collation's
//! business. The Ibis semantics (case-sensitive, bytewise order) need a
//! binary collation such as `utf8mb4_bin` on the columns; the connection is
//! set to it so that strings a query produces compare the same way.

mod dialect;
#[cfg(test)]
mod tests;

use std::sync::Mutex;

use arrow_array::RecordBatch;
use ibis_core::convert::{batch_to_rows, format_date, format_timestamp, parse_date, parse_timestamp, rows_to_batch};
use ibis_core::{Backend, DataType, Error, Field, Result, Schema, Table, Value};
use ibis_sql::{Dialect, SqlClient, compile, create_table_sql};
use mysql::consts::ColumnType;
use mysql::prelude::Queryable;
use mysql::{Conn, Opts};

pub use dialect::MySqlDialect;
use dialect::quote_str;

/// Rows are inserted in statements of about this many bytes.
const INSERT_BYTES: usize = 1 << 20;

pub struct MySql {
    conn: Mutex<Conn>,
    dialect: MySqlDialect,
}

fn exec_err(sql: &str, message: impl std::fmt::Display) -> Error {
    Error::Execution { backend: "mysql".into(), sql: sql.into(), message: message.to_string() }
}

/// `information_schema.columns` -> Ibis type. `column_type` is the full
/// spelling (`tinyint(1)`, `int unsigned`), `data_type` its first word.
fn parse_type(data_type: &str, column_type: &str, precision: Option<i64>, scale: Option<i64>) -> DataType {
    let unsigned = column_type.contains("unsigned");
    let int = |signed: DataType, unsigned_type: DataType| if unsigned { unsigned_type } else { signed };
    match data_type {
        "tinyint" if column_type == "tinyint(1)" => DataType::Boolean,
        "tinyint" => int(DataType::Int8, DataType::UInt8),
        "smallint" => int(DataType::Int16, DataType::UInt16),
        "mediumint" | "int" => int(DataType::Int32, DataType::UInt32),
        "bigint" => int(DataType::Int64, DataType::UInt64),
        "float" => DataType::Float32,
        "double" => DataType::Float64,
        "decimal" => match (precision, scale) {
            (Some(p), Some(s)) => DataType::Decimal { precision: p as u8, scale: s as i8 },
            _ => DataType::Float64,
        },
        "date" => DataType::Date,
        "datetime" => DataType::Timestamp { tz: None },
        "timestamp" => DataType::Timestamp { tz: Some("UTC".into()) },
        "binary" | "varbinary" | "tinyblob" | "blob" | "mediumblob" | "longblob" => DataType::Binary,
        _ => DataType::String,
    }
}

/// A result cell, which the text protocol sends as text, as a value of the
/// column type the server reported.
fn cell(v: mysql::Value, ty: ColumnType) -> Value {
    use ColumnType::*;
    let bytes = match v {
        mysql::Value::NULL => return Value::Null,
        mysql::Value::Bytes(b) => b,
        // the binary protocol's typed values; text queries do not produce them
        mysql::Value::Int(i) => return Value::Int(i),
        mysql::Value::UInt(u) => return i64::try_from(u).map_or_else(|_| Value::String(u.to_string()), Value::Int),
        mysql::Value::Float(f) => return Value::Float(f.into()),
        mysql::Value::Double(f) => return Value::Float(f),
        other => return Value::String(other.as_sql(true).trim_matches('\'').to_string()),
    };
    let text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => return Value::Binary(e.into_bytes()),
    };
    let typed = match ty {
        MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_INT24 | MYSQL_TYPE_LONG | MYSQL_TYPE_LONGLONG
        | MYSQL_TYPE_YEAR => text.parse().ok().map(Value::Int),
        MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE => text.parse().ok().map(Value::Float),
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => parse_date(&text).map(Value::Date),
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 | MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => {
            parse_timestamp(&text).map(Value::Timestamp)
        }
        _ => None,
    };
    // anything else (decimals, a BIGINT UNSIGNED past i64) stays text and is parsed to the output type
    typed.unwrap_or(Value::String(text))
}

/// One value as a literal in an `INSERT`.
fn insert_literal(v: &Value, out: &mut String) -> Result<()> {
    match v {
        Value::Null => out.push_str("NULL"),
        Value::Bool(b) => out.push(if *b { '1' } else { '0' }),
        Value::Int(i) => out.push_str(&i.to_string()),
        Value::Float(f) if !f.is_finite() => {
            return Err(Error::Unsupported {
                backend: "mysql".into(),
                op: "storing NaN or infinity (DOUBLE has neither)".into(),
            });
        }
        Value::Float(f) => out.push_str(&format!("{f:e}")),
        Value::String(s) => out.push_str(&quote_str(s)),
        Value::Binary(b) => {
            out.push_str("x'");
            b.iter().for_each(|byte| out.push_str(&format!("{byte:02x}")));
            out.push('\'');
        }
        Value::Date(d) => out.push_str(&format!("'{}'", format_date(*d))),
        Value::Timestamp(t) => out.push_str(&format!("'{}'", format_timestamp(*t))),
        Value::Array(_) | Value::Struct(_) | Value::Map(_) => {
            return Err(Error::Unsupported { backend: "mysql".into(), op: "storing arrays, structs or maps".into() });
        }
    }
    Ok(())
}

impl MySql {
    /// Connect: `mysql://user[:password]@host[:port]/database`.
    pub fn connect(url: &str) -> Result<Self> {
        let opts = Opts::from_url(url).map_err(|e| {
            Error::Invalid(format!("mysql: expected mysql://user[:password]@host[:port]/database, got {url:?}: {e}"))
        })?;
        let mut conn = Conn::new(opts).map_err(|e| exec_err("<connect>", e))?;
        for setup in [
            // strings a query produces (literals, casts) compare bytewise, as Ibis's do
            "SET NAMES utf8mb4 COLLATE utf8mb4_bin",
            // TIMESTAMP columns are read and written in the session's zone
            "SET time_zone = '+00:00'",
        ] {
            conn.query_drop(setup).map_err(|e| exec_err(setup, e))?;
        }
        Ok(MySql { conn: Mutex::new(conn), dialect: MySqlDialect })
    }

    /// Run one statement that returns no rows.
    pub fn execute_statement(&self, sql: &str) -> Result<()> {
        self.conn.lock().unwrap().query_drop(sql).map_err(|e| exec_err(sql, e))
    }

    /// Run a query, returning rows typed by what MySQL says each column is.
    pub fn raw_sql(&self, sql: &str) -> Result<Vec<Vec<Value>>> {
        let mut conn = self.conn.lock().unwrap();
        let result = conn.query_iter(sql).map_err(|e| exec_err(sql, e))?;
        let mut rows = vec![];
        for row in result {
            let row = row.map_err(|e| exec_err(sql, e))?;
            let columns = row.columns();
            rows.push(row.unwrap().into_iter().zip(columns.iter()).map(|(v, c)| cell(v, c.column_type())).collect());
        }
        Ok(rows)
    }
}

impl SqlClient for MySql {
    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    fn statement(&self, sql: &str) -> Result<()> {
        self.execute_statement(sql)
    }

    fn load(&self, name: &str, schema: &Schema, data: &[RecordBatch]) -> Result<()> {
        let _ = schema;
        let head = format!("INSERT INTO {} VALUES ", self.dialect.quote_ident(name));
        let mut stmt = String::new();
        for b in data {
            for row in batch_to_rows(b)? {
                stmt.push_str(if stmt.is_empty() { &head } else { ", " });
                stmt.push('(');
                for (i, v) in row.iter().enumerate() {
                    if i > 0 {
                        stmt.push_str(", ");
                    }
                    insert_literal(v, &mut stmt)?;
                }
                stmt.push(')');
                if stmt.len() >= INSERT_BYTES {
                    self.execute_statement(&stmt)?;
                    stmt.clear();
                }
            }
        }
        if !stmt.is_empty() {
            self.execute_statement(&stmt)?;
        }
        Ok(())
    }
}

impl Backend for MySql {
    ibis_sql::sql_client_methods!();
    fn name(&self) -> &str {
        "mysql"
    }

    fn compile(&self, table: &Table) -> Result<String> {
        compile(&self.dialect, table)
    }

    fn execute(&self, table: &Table) -> Result<Vec<RecordBatch>> {
        let rows = self.raw_sql(&self.compile(table)?)?;
        Ok(vec![rows_to_batch(table.schema(), &rows)?])
    }

    fn table(&self, name: &str) -> Result<Table> {
        let rows = self.raw_sql(&format!(
            "SELECT column_name, data_type, column_type, numeric_precision, numeric_scale \
             FROM information_schema.columns \
             WHERE table_schema = DATABASE() AND table_name = {} ORDER BY ordinal_position",
            quote_str(name)
        ))?;
        if rows.is_empty() {
            return Err(Error::Other(format!("mysql: no table {name:?}")));
        }
        let int = |v: &Value| match v {
            Value::Int(i) => Some(*i),
            _ => None,
        };
        let fields = rows
            .iter()
            .map(|r| match (&r[0], &r[1], &r[2]) {
                (Value::String(col), Value::String(data_type), Value::String(column_type)) => {
                    Ok(Field::new(col, parse_type(data_type, column_type, int(&r[3]), int(&r[4]))))
                }
                other => Err(Error::Other(format!("mysql: unexpected information_schema.columns row {other:?}"))),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Table::named(name, Schema::new(fields)))
    }

    fn create_table(&self, name: &str, data: &[RecordBatch]) -> Result<()> {
        let Some(first) = data.first() else {
            return Err(Error::Other("create_table needs at least one batch".into()));
        };
        let schema = Schema::from_arrow(&first.schema())?;
        let q = self.dialect.quote_ident(name);
        self.execute_statement(&format!("DROP TABLE IF EXISTS {q}"))?;
        self.execute_statement(&create_table_sql(&self.dialect, name, &schema)?)?;
        self.load(name, &schema, data)
    }

    fn list_tables(&self) -> Result<Vec<String>> {
        let rows = self.raw_sql(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = DATABASE() ORDER BY table_name",
        )?;
        Ok(rows
            .into_iter()
            .filter_map(|r| match r.into_iter().next() {
                Some(Value::String(s)) => Some(s),
                _ => None,
            })
            .collect())
    }
}
