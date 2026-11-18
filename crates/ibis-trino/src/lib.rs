//! Trino backend for ibis-rs.
//!
//! Statements go over Trino's HTTP protocol: one POST starts a query, and the
//! JSON pages it answers with are followed until there is no next one. Each
//! cell is read according to the column type the page reports, then converted
//! to the Ibis output schema.
//!
//! Three things about Trino to know:
//! - Identifiers are case-insensitive and stored in lower case, so
//!   [`Backend::table`] reports a column created as `playerID` as `playerid`.
//! - It is strictly typed: `1.5` is an exact decimal, so floats are written
//!   with an exponent, and inserted rows are cast to the column types.
//! - The protocol has no bulk load. [`Backend::create_table`] sends the rows
//!   inside `INSERT` statements, a few megabytes at a time: fine for test data
//!   and small tables, not a way to load a warehouse.

mod dialect;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use arrow_array::RecordBatch;
use ibis_core::convert::{batch_to_rows, format_date, format_timestamp, parse_date, parse_timestamp, rows_to_batch};
use ibis_core::{Backend, DataType, Error, Field, Result, Schema, Table, Value};
use ibis_sql::{Dialect, compile, create_table_sql, quote_str};
use serde_json::Value as Json;

pub use dialect::TrinoDialect;

/// Rows are inserted in statements of about this many bytes; the server
/// refuses one longer than a million characters.
const INSERT_BYTES: usize = 700_000;
/// Insert statements in flight at once.
const INSERT_WORKERS: usize = 4;

pub struct Trino {
    agent: ureq::Agent,
    /// `http://host:port`
    endpoint: String,
    user: String,
    /// `Basic ...`, when the URL carries a password.
    authorization: Option<String>,
    catalog: String,
    schema: String,
    dialect: TrinoDialect,
}

fn exec_err(sql: &str, message: impl std::fmt::Display) -> Error {
    Error::Execution { backend: "trino".into(), sql: sql.into(), message: message.to_string() }
}

/// The parts of `scheme://user[:password]@host[:port]/catalog[/schema]`.
struct Url<'a> {
    scheme: &'a str,
    user: &'a str,
    password: &'a str,
    host: &'a str,
    catalog: &'a str,
    schema: &'a str,
}

fn parse_url(url: &str) -> Result<Url<'_>> {
    let bad =
        || Error::Invalid(format!("trino: expected trino://user[:password]@host[:port]/catalog[/schema], got {url:?}"));
    let (scheme, rest) = url.split_once("://").ok_or_else(bad)?;
    let scheme = match scheme {
        "trino" | "http" => "http",
        "trinos" | "https" => "https",
        _ => return Err(bad()),
    };
    let (authority, path) = rest.split_once('/').ok_or_else(bad)?;
    let (credentials, host) = authority.rsplit_once('@').unwrap_or(("", authority));
    let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
    let (catalog, schema) = path.trim_end_matches('/').split_once('/').unwrap_or((path.trim_end_matches('/'), ""));
    if host.is_empty() || catalog.is_empty() {
        return Err(bad());
    }
    Ok(Url { scheme, user, password, host, catalog, schema })
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, b| n << 8 | *b as u32) << (8 * (3 - chunk.len()));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A Trino type name (`bigint`, `decimal(15, 2)`, `timestamp(6) with time zone`)
/// as an Ibis type.
fn parse_type(name: &str) -> DataType {
    let name = name.to_ascii_lowercase();
    match name.as_str() {
        "boolean" => DataType::Boolean,
        "tinyint" => DataType::Int8,
        "smallint" => DataType::Int16,
        "integer" => DataType::Int32,
        "bigint" => DataType::Int64,
        "real" => DataType::Float32,
        "double" => DataType::Float64,
        "date" => DataType::Date,
        "varbinary" => DataType::Binary,
        "unknown" => DataType::Null,
        _ => {
            if let Some(args) = name.strip_prefix("decimal(").and_then(|s| s.strip_suffix(')'))
                && let Some((p, s)) = args.split_once(',')
                && let (Ok(precision), Ok(scale)) = (p.trim().parse(), s.trim().parse())
            {
                return DataType::Decimal { precision, scale };
            }
            if name.starts_with("timestamp") {
                return DataType::Timestamp { tz: name.ends_with("with time zone").then(|| "UTC".to_string()) };
            }
            DataType::String
        }
    }
}

/// A result cell (JSON) as a value of its column's type.
fn cell(j: &Json, dtype: &DataType) -> Value {
    match (j, dtype) {
        (Json::Null, _) => Value::Null,
        (Json::Bool(b), _) => Value::Bool(*b),
        (Json::Number(n), t) if t.is_integer() => n.as_i64().map_or_else(|| Value::String(n.to_string()), Value::Int),
        (Json::Number(n), _) => n.as_f64().map_or_else(|| Value::String(n.to_string()), Value::Float),
        // JSON has no NaN or infinity: they arrive as strings
        (Json::String(s), DataType::Float32 | DataType::Float64) => match s.as_str() {
            "NaN" => Value::Float(f64::NAN),
            "Infinity" => Value::Float(f64::INFINITY),
            "-Infinity" => Value::Float(f64::NEG_INFINITY),
            other => other.parse().map_or_else(|_| Value::String(s.clone()), Value::Float),
        },
        (Json::String(s), DataType::Date) => parse_date(s).map_or_else(|| Value::String(s.clone()), Value::Date),
        (Json::String(s), DataType::Timestamp { .. }) => {
            // "2020-01-02 03:04:05.000000 UTC": the zone is a third word
            let end = s.match_indices(' ').nth(1).map_or(s.len(), |(i, _)| i);
            parse_timestamp(&s[..end]).map_or_else(|| Value::String(s.clone()), Value::Timestamp)
        }
        // decimals stay text and are parsed to the output type
        (Json::String(s), _) => Value::String(s.clone()),
        // arrays, maps and rows: their JSON text
        (other, _) => Value::String(other.to_string()),
    }
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
        // TODO(nested)
        Value::Array(_) | Value::Struct(_) | Value::Map(_) => return None,
    })
}

impl Trino {
    /// Connect over HTTP: `trino://user[:password]@host[:port]/catalog[/schema]`
    /// (`trinos://` for HTTPS; `http://` and `https://` work too). The schema
    /// defaults to `default`. A password is sent as HTTP basic authentication,
    /// which Trino only accepts over HTTPS.
    pub fn connect(url: &str) -> Result<Self> {
        let u = parse_url(url)?;
        let config = ureq::Agent::config_builder().http_status_as_error(false).build();
        let trino = Trino {
            agent: ureq::Agent::new_with_config(config),
            endpoint: format!("{}://{}", u.scheme, u.host),
            user: if u.user.is_empty() { "ibis" } else { u.user }.to_string(),
            authorization: (!u.password.is_empty())
                .then(|| format!("Basic {}", base64(format!("{}:{}", u.user, u.password).as_bytes()))),
            catalog: u.catalog.to_string(),
            schema: if u.schema.is_empty() { "default" } else { u.schema }.to_string(),
            dialect: TrinoDialect,
        };
        trino.run("SELECT 1")?;
        Ok(trino)
    }

    /// One protocol request: start `sql`, or fetch the page at `next`.
    fn request(&self, sql: &str, next: Option<&str>) -> Result<Json> {
        let mut busy = 0;
        loop {
            let res = match next {
                None => {
                    let mut req = self
                        .agent
                        .post(format!("{}/v1/statement", self.endpoint))
                        .header("X-Trino-User", &self.user)
                        .header("X-Trino-Catalog", &self.catalog)
                        .header("X-Trino-Schema", &self.schema)
                        .header("X-Trino-Time-Zone", "UTC")
                        .header("X-Trino-Source", "ibis-rs")
                        // without it, timestamps arrive cut to milliseconds
                        .header("X-Trino-Client-Capabilities", "PARAMETRIC_DATETIME");
                    if let Some(auth) = &self.authorization {
                        req = req.header("Authorization", auth);
                    }
                    req.send(sql)
                }
                Some(uri) => {
                    let mut req = self.agent.get(uri).header("X-Trino-User", &self.user);
                    if let Some(auth) = &self.authorization {
                        req = req.header("Authorization", auth);
                    }
                    req.call()
                }
            };
            let mut res = res.map_err(|e| exec_err(sql, e))?;
            let status = res.status();
            // the protocol's "try again": the server is busy
            if matches!(status.as_u16(), 502..=504) && busy < 100 {
                busy += 1;
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            let body = res.body_mut().with_config().limit(u64::MAX).read_to_string().map_err(|e| exec_err(sql, e))?;
            if !status.is_success() {
                return Err(exec_err(sql, format!("HTTP {status}: {}", body.trim())));
            }
            return serde_json::from_str(&body).map_err(|e| exec_err(sql, format!("unreadable response: {e}")));
        }
    }

    /// Run one statement to completion: rows typed by what Trino says each column is.
    fn run(&self, sql: &str) -> Result<Vec<Vec<Value>>> {
        let mut page = self.request(sql, None)?;
        let mut types: Vec<DataType> = vec![];
        let mut rows = vec![];
        loop {
            if let Some(error) = page.get("error") {
                let name = error["errorName"].as_str().unwrap_or("ERROR");
                return Err(exec_err(sql, format!("{name}: {}", error["message"].as_str().unwrap_or("no message"))));
            }
            if types.is_empty()
                && let Some(columns) = page["columns"].as_array()
            {
                types = columns.iter().map(|c| parse_type(c["type"].as_str().unwrap_or(""))).collect();
            }
            for row in page["data"].as_array().into_iter().flatten() {
                rows.push(row.as_array().into_iter().flatten().zip(&types).map(|(j, t)| cell(j, t)).collect());
            }
            let Some(next) = page["nextUri"].as_str().map(String::from) else { break };
            page = self.request(sql, Some(&next))?;
        }
        Ok(rows)
    }

    /// Run one statement that returns no rows.
    pub fn execute_statement(&self, sql: &str) -> Result<()> {
        self.run(sql).map(|_| ())
    }

    /// Run a query, returning rows typed by what Trino says each column is.
    pub fn raw_sql(&self, sql: &str) -> Result<Vec<Vec<Value>>> {
        self.run(sql)
    }

    fn strings(&self, sql: &str) -> Result<Vec<String>> {
        Ok(self
            .run(sql)?
            .into_iter()
            .filter_map(|r| match r.into_iter().next() {
                Some(Value::String(s)) => Some(s),
                _ => None,
            })
            .collect())
    }
}

/// `INSERT` statements carrying `data` into the table `q`, whose columns are `schema`'s.
///
/// The rows travel column-wise: one JSON array of strings per column,
/// unnested side by side and cast to the column types. A `VALUES` list reads
/// more plainly, but Trino plans an expression per cell and keeps the plans of
/// finished queries for minutes; 100,000 rows sent that way cost it gigabytes.
fn insert_statements(d: &TrinoDialect, q: &str, schema: &Schema, data: &[RecordBatch]) -> Result<Vec<String>> {
    let select = schema
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            Ok(match f.dtype {
                DataType::Binary => format!("FROM_HEX(c{i})"),
                _ => format!("CAST(c{i} AS {})", d.type_name(&f.dtype)?),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let names: Vec<String> = (0..schema.len()).map(|i| format!("c{i}")).collect();
    let statement = |columns: &[String]| {
        let arrays: Vec<String> =
            columns.iter().map(|json| format!("CAST(JSON_PARSE('[{json}]') AS ARRAY(VARCHAR))")).collect();
        format!(
            "INSERT INTO {q} SELECT {} FROM UNNEST({}) AS v ({})",
            select.join(", "),
            arrays.join(", "),
            names.join(", ")
        )
    };
    let mut statements = vec![];
    // per column, the elements of its JSON array so far, as they sit inside a SQL string literal
    let mut columns = vec![String::new(); schema.len()];
    for b in data {
        for row in batch_to_rows(b)? {
            for ((v, f), column) in row.iter().zip(&schema.fields).zip(&mut columns) {
                if !column.is_empty() {
                    column.push(',');
                }
                match insert_text(v, &f.dtype) {
                    Some(text) => column.push_str(&Json::String(text).to_string().replace('\'', "''")),
                    None => column.push_str("null"),
                }
            }
            if columns.iter().map(String::len).sum::<usize>() >= INSERT_BYTES {
                statements.push(statement(&columns));
                columns.iter_mut().for_each(String::clear);
            }
        }
    }
    if columns.iter().any(|c| !c.is_empty()) {
        statements.push(statement(&columns));
    }
    Ok(statements)
}

impl Backend for Trino {
    fn name(&self) -> &str {
        "trino"
    }

    fn compile(&self, table: &Table) -> Result<String> {
        compile(&self.dialect, table)
    }

    fn execute(&self, table: &Table) -> Result<Vec<RecordBatch>> {
        let rows = self.run(&self.compile(table)?)?;
        Ok(vec![rows_to_batch(table.schema(), &rows)?])
    }

    fn table(&self, name: &str) -> Result<Table> {
        let rows = self.run(&format!(
            "SELECT column_name, data_type FROM information_schema.columns \
             WHERE table_schema = current_schema AND table_name = {} ORDER BY ordinal_position",
            quote_str(&name.to_lowercase())
        ))?;
        if rows.is_empty() {
            return Err(Error::Other(format!("trino: no table {name:?}")));
        }
        let fields = rows
            .iter()
            .map(|r| match (&r[0], &r[1]) {
                (Value::String(col), Value::String(ty)) => Ok(Field::new(col, parse_type(ty))),
                other => Err(Error::Other(format!("trino: unexpected information_schema.columns row {other:?}"))),
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
        let statements = insert_statements(&self.dialect, &q, &schema, data)?;
        let next = AtomicUsize::new(0);
        let worker = || -> Result<()> {
            while let Some(stmt) = statements.get(next.fetch_add(1, Ordering::Relaxed)) {
                self.execute_statement(stmt)?;
            }
            Ok(())
        };
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..INSERT_WORKERS.min(statements.len())).map(|_| scope.spawn(worker)).collect();
            workers.into_iter().try_for_each(|w| w.join().expect("insert worker panicked"))
        })
    }

    fn list_tables(&self) -> Result<Vec<String>> {
        self.strings(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = current_schema ORDER BY table_name",
        )
    }
}
