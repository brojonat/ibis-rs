//! Spark SQL backend for ibis-rs.
//!
//! Statements go to a Spark Thrift server (`sbin/start-thriftserver.sh`),
//! which speaks the HiveServer2 protocol: the endpoint a JDBC URL of the
//! form `jdbc:hive2://host:10000` names. This is not the cluster's master
//! URL, and not Spark Connect.
//!
//! What shapes the dialect:
//! - `1.5` is an exact decimal, so floats are written with an exponent.
//! - `TIMESTAMP` is an instant, shown in the session time zone (set to UTC
//!   here); the zone-less type is `TIMESTAMP_NTZ`.
//! - String literals read backslash escapes, and two adjacent literals are
//!   one string.
//! - There is no bulk load in the protocol. [`Backend::create_table`] sends
//!   the rows inside `INSERT` statements: fine for test data and small
//!   tables, not a way to load a lake.

mod dialect;
mod nested;
#[cfg(test)]
mod tests;

use std::sync::Mutex;

use arrow_array::RecordBatch;
use ibis_core::convert::{batch_to_rows, format_date, format_timestamp, parse_date, parse_timestamp, rows_to_batch};
use ibis_core::{Backend, DataType, Error, Field, Result, Schema, Table, Value};
use ibis_hiveserver2::{Connection, Options, ResultSet, Values};
use ibis_sql::json::{parse_json, value_to_json};
use ibis_sql::{Dialect, compile, create_table_sql};

pub use dialect::SparkDialect;
use dialect::quote_str;

/// Rows are inserted in statements of about this many bytes.
const INSERT_BYTES: usize = 1 << 20;

pub struct Spark {
    conn: Mutex<Connection>,
    dialect: SparkDialect,
}

/// The server's errors come with a Java stack trace: keep what precedes it.
fn exec_err(sql: &str, message: impl std::fmt::Display) -> Error {
    let message = message.to_string();
    let end = message.find("\n\tat ").unwrap_or(message.len());
    Error::Execution { backend: "spark".into(), sql: sql.into(), message: message[..end].trim_end().to_string() }
}

/// A type as `DESCRIBE` and result metadata spell it -> Ibis type.
fn parse_type(name: &str) -> DataType {
    if let Some(nested) = nested::parse_type(name, &parse_type) {
        return nested;
    }
    let name = name.to_ascii_lowercase();
    match name.as_str() {
        "boolean" => DataType::Boolean,
        "tinyint" | "byte" => DataType::Int8,
        "smallint" | "short" => DataType::Int16,
        "int" | "integer" => DataType::Int32,
        "bigint" | "long" => DataType::Int64,
        "float" | "real" => DataType::Float32,
        "double" => DataType::Float64,
        "date" => DataType::Date,
        "binary" => DataType::Binary,
        "void" | "null" => DataType::Null,
        "timestamp_ntz" => DataType::Timestamp { tz: None },
        "timestamp" | "timestamp_ltz" | "timestamp with local time zone" => {
            DataType::Timestamp { tz: Some("UTC".into()) }
        }
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

/// One value as the text `CAST(... AS dtype)` reads back; `None` is NULL.
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
        Value::Array(_) | Value::Struct(_) | Value::Map(_) => format!("{{\"v\":{}}}", value_to_json(v)),
    })
}

/// `INSERT` statements carrying `data` into the table `q`, whose columns are
/// `schema`'s. Every value travels as a string and is cast to its column's
/// type: a column of a `VALUES` list takes one type, and assignment to a
/// table column does not convert from text.
fn insert_statements(d: &SparkDialect, q: &str, schema: &Schema, data: &[RecordBatch]) -> Result<Vec<String>> {
    let select = schema
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            Ok(match f.dtype {
                DataType::Binary => format!("UNHEX(c{i})"),
                // sent as JSON, inside an object: FROM_JSON reads a struct on every version
                ref t if t.is_nested() => {
                    format!("FROM_JSON(c{i}, {}).v", quote_str(&format!("v {}", d.type_name(t)?)))
                }
                _ => format!("CAST(c{i} AS {})", d.type_name(&f.dtype)?),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let names: Vec<String> = (0..schema.len()).map(|i| format!("c{i}")).collect();
    let head = format!("INSERT INTO {q} SELECT {} FROM VALUES ", select.join(", "));
    let tail = format!(" AS v ({})", names.join(", "));
    let mut statements = vec![];
    let mut values = String::new();
    for b in data {
        for row in batch_to_rows(b)? {
            values.push_str(if values.is_empty() { "(" } else { ", (" });
            for (i, (v, f)) in row.iter().zip(&schema.fields).enumerate() {
                if i > 0 {
                    values.push_str(", ");
                }
                match insert_text(v, &f.dtype) {
                    Some(text) => values.push_str(&quote_str(&text)),
                    // typed, so that a column of nothing but NULLs is still a string
                    None => values.push_str("CAST(NULL AS STRING)"),
                }
            }
            values.push(')');
            if values.len() >= INSERT_BYTES {
                statements.push(format!("{head}{values}{tail}"));
                values.clear();
            }
        }
    }
    if !values.is_empty() {
        statements.push(format!("{head}{values}{tail}"));
    }
    Ok(statements)
}

impl Spark {
    /// Connect to a Spark Thrift server:
    /// `spark://[user[:password]@]host[:port][/database][?auth=nosasl]`.
    /// The port defaults to 10000 and the user to `anonymous`. The
    /// connection does the SASL `PLAIN` handshake, which is what a server
    /// left at `hive.server2.authentication=NONE` expects; `?auth=nosasl` is
    /// for one set to `NOSASL`. The session time zone is set to UTC.
    pub fn connect(url: &str) -> Result<Self> {
        let bad = |why: &dyn std::fmt::Display| {
            Error::Invalid(format!(
                "spark: expected spark://[user[:password]@]host[:port][/database][?auth=nosasl], got {url:?} ({why})"
            ))
        };
        if !url.starts_with("spark://") {
            return Err(bad(&"not a spark:// URL"));
        }
        let mut options = Options::from_url(url, 10000, true).map_err(|e| bad(&e))?;
        if options.user.is_empty() {
            options.user = "anonymous".into();
        }
        let conn = Connection::open(&options).map_err(|e| Error::Other(format!("spark: {url}: {e}")))?;
        let spark = Spark { conn: Mutex::new(conn), dialect: SparkDialect };
        spark.execute_statement("SET spark.sql.session.timeZone=UTC")?;
        // merging maps: the right one's value for a key both have, as in Ibis
        spark.execute_statement("SET spark.sql.mapKeyDedupPolicy=LAST_WIN")?;
        Ok(spark)
    }

    fn run(&self, sql: &str) -> Result<ResultSet> {
        self.conn.lock().unwrap().execute(sql).map_err(|e| exec_err(sql, e))
    }

    /// Run one statement that returns no rows.
    pub fn execute_statement(&self, sql: &str) -> Result<()> {
        self.run(sql).map(|_| ())
    }

    /// Run a query, returning rows typed by what Spark says each column is.
    pub fn raw_sql(&self, sql: &str) -> Result<Vec<Vec<Value>>> {
        self.run(sql).map(rows)
    }
}

impl Backend for Spark {
    fn name(&self) -> &str {
        "spark"
    }

    fn compile(&self, table: &Table) -> Result<String> {
        compile(&self.dialect, table)
    }

    fn execute(&self, table: &Table) -> Result<Vec<RecordBatch>> {
        let sql = self.compile(table)?;
        let schema = table.schema();
        if !schema.fields.iter().any(|f| f.dtype.is_nested()) {
            return Ok(vec![rows_to_batch(schema, &self.raw_sql(&sql)?)?]);
        }
        // Over Thrift an array, struct or map arrives as display text that
        // cannot be read back with certainty. Each is asked for as JSON
        // instead, inside an object: TO_JSON takes a struct on every version.
        let q = |name: &str| self.dialect.quote_ident(name);
        let columns: Vec<String> = schema
            .fields
            .iter()
            .map(|f| match f.dtype.is_nested() {
                true => format!("TO_JSON(NAMED_STRUCT('v', {})) AS {}", q(&f.name), q(&f.name)),
                false => q(&f.name),
            })
            .collect();
        let mut rows = self.raw_sql(&format!("SELECT {} FROM ({sql}) AS ibis_result", columns.join(", ")))?;
        for row in &mut rows {
            for (cell, f) in row.iter_mut().zip(&schema.fields).filter(|(_, f)| f.dtype.is_nested()) {
                if let Value::String(text) = cell {
                    let wrapped = DataType::Struct(vec![("v".into(), f.dtype.clone())]);
                    *cell = match parse_json(text, &wrapped) {
                        Some(Value::Struct(mut fields)) if fields.len() == 1 => fields.remove(0).1,
                        _ => return Err(exec_err(&sql, format!("unreadable {} value {text:?}", f.dtype))),
                    };
                }
            }
        }
        Ok(vec![rows_to_batch(schema, &rows)?])
    }

    fn table(&self, name: &str) -> Result<Table> {
        let q = self.dialect.quote_ident(name);
        // name, type, comment; after the columns, a partitioned table lists them again under "# ..." headings
        let described = self
            .raw_sql(&format!("DESCRIBE TABLE {q}"))
            .map_err(|e| Error::Other(format!("spark: no table {name:?}: {e}")))?;
        let fields = described
            .iter()
            .map_while(|r| match (r.first(), r.get(1)) {
                (Some(Value::String(col)), Some(Value::String(ty))) if !col.is_empty() && !col.starts_with('#') => {
                    Some(Field::new(col, parse_type(ty)))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
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
        for statement in insert_statements(&self.dialect, &q, &schema, data)? {
            self.execute_statement(&statement)?;
        }
        Ok(())
    }

    fn list_tables(&self) -> Result<Vec<String>> {
        // namespace, tableName, isTemporary
        let mut names: Vec<String> = self
            .raw_sql("SHOW TABLES")?
            .into_iter()
            .filter_map(|r| match r.into_iter().nth(1) {
                Some(Value::String(s)) => Some(s),
                _ => None,
            })
            .collect();
        names.sort();
        Ok(names)
    }
}
