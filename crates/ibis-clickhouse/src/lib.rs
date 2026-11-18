//! ClickHouse backend for ibis-rs.
//!
//! Queries go over ClickHouse's HTTP interface and come back as
//! tab-separated text with a header of column types, which is converted to the
//! Ibis output schema.
//!
//! Three things about ClickHouse shape the dialect:
//! - Types are not nullable unless they say so. Every type this backend names
//!   is `Nullable(...)`, in tables and in casts.
//! - Its defaults differ from standard SQL (an outer join fills with zeros and
//!   empty strings, `sum` of no rows is 0, a bare `UNION` is an error), so each
//!   request sets the settings that restore the standard behaviour.
//! - String literals take backslash escapes.

mod dialect;
mod nested;
#[cfg(test)]
mod tests;

use arrow_array::RecordBatch;
use ibis_core::convert::{batch_to_rows, format_date, format_timestamp, parse_date, parse_timestamp, rows_to_batch};
use ibis_core::{Backend, DataType, Error, Field, Result, Schema, Table, Value};
use ibis_sql::json::{value_from_json, value_to_json};
use ibis_sql::{Dialect, SqlClient, compile, create_table_sql};

pub use dialect::ClickHouseDialect;
use dialect::quote_str;

/// Settings sent with every request, so queries mean what they mean in
/// standard SQL (and in Ibis).
const SETTINGS: [(&str, &str); 11] = [
    // outer joins fill unmatched columns with NULL, not the type's default
    ("join_use_nulls", "1"),
    // SUM/MIN/MAX/AVG of no rows is NULL
    ("aggregate_functions_null_for_empty", "1"),
    // CAST of a NULL to a type is NULL, whatever the type
    ("cast_keep_nullable", "1"),
    ("union_default_mode", "DISTINCT"),
    ("intersect_default_mode", "DISTINCT"),
    ("except_default_mode", "DISTINCT"),
    ("allow_experimental_correlated_subqueries", "1"),
    ("session_timezone", "UTC"),
    // `SELECT x + 1 AS x ... WHERE x > 0` filters on the column, as in standard SQL
    ("prefer_column_name_to_alias", "1"),
    // in JSON: NaN and the infinities by name rather than as null, and a
    // named tuple as an object
    ("output_format_json_quote_denormals", "1"),
    ("output_format_json_named_tuples_as_objects", "1"),
];

pub struct ClickHouse {
    agent: ureq::Agent,
    /// `http://host:port/`
    endpoint: String,
    database: String,
    user: String,
    password: String,
    dialect: ClickHouseDialect,
}

fn exec_err(sql: &str, message: impl std::fmt::Display) -> Error {
    Error::Execution { backend: "clickhouse".into(), sql: sql.into(), message: message.to_string() }
}

/// The parts of `scheme://[user[:password]@]host[:port][/database]`.
struct Url<'a> {
    scheme: &'a str,
    user: &'a str,
    password: &'a str,
    host: &'a str,
    database: &'a str,
}

fn parse_url(url: &str) -> Result<Url<'_>> {
    let bad = || {
        Error::Invalid(format!(
            "clickhouse: expected clickhouse://[user[:password]@]host[:port][/database], got {url:?}"
        ))
    };
    let (scheme, rest) = url.split_once("://").ok_or_else(bad)?;
    let scheme = match scheme {
        "clickhouse" | "http" => "http",
        "clickhouses" | "https" => "https",
        _ => return Err(bad()),
    };
    let (authority, database) = rest.split_once('/').unwrap_or((rest, ""));
    let (credentials, host) = authority.rsplit_once('@').unwrap_or(("", authority));
    let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
    if host.is_empty() {
        return Err(bad());
    }
    Ok(Url { scheme, user, password, host, database: database.trim_end_matches('/') })
}

/// Undo ClickHouse's TabSeparated escaping. `\N` (NULL) is handled by the caller.
fn unescape(field: &str) -> String {
    if !field.contains('\\') {
        return field.to_string();
    }
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn escape(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            _ => out.push(ch),
        }
    }
}

/// One value as a TabSeparated field.
fn tsv_field(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("\\N"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => out.push_str(&i.to_string()),
        Value::Float(f) if f.is_nan() => out.push_str("nan"),
        Value::Float(f) if f.is_infinite() => out.push_str(if *f > 0.0 { "inf" } else { "-inf" }),
        Value::Float(f) => out.push_str(&format!("{f:?}")),
        Value::String(s) => escape(s, out),
        Value::Binary(b) => escape(&String::from_utf8_lossy(b), out),
        Value::Date(d) => out.push_str(&format_date(*d)),
        Value::Timestamp(t) => out.push_str(&format_timestamp(*t)),
        // a table with such a column is inserted as JSON instead
        Value::Array(_) | Value::Struct(_) | Value::Map(_) => out.push_str("\\N"),
    }
}

/// A ClickHouse type name with `Nullable(...)` and `LowCardinality(...)` taken off.
fn bare_type(name: &str) -> &str {
    let mut name = name;
    for wrapper in ["LowCardinality(", "Nullable("] {
        if let Some(inner) = name.strip_prefix(wrapper).and_then(|s| s.strip_suffix(')')) {
            name = inner;
        }
    }
    name
}

/// The comma-separated parts of a type's argument list, not splitting inside
/// parentheses or quotes.
fn top_level(args: &str) -> Vec<&str> {
    let (mut parts, mut depth, mut quoted, mut start) = (vec![], 0, false, 0);
    for (i, ch) in args.char_indices() {
        match ch {
            '`' | '\'' | '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                parts.push(args[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(args[start..].trim());
    parts
}

/// A named tuple field, `name Type` or `` `name` Type ``, as its two halves.
fn split_field(field: &str) -> Option<(String, &str)> {
    if let Some(rest) = field.strip_prefix('`') {
        let (name, t) = rest.split_once('`')?;
        return Some((name.replace("\\`", "`"), t.trim()));
    }
    let (name, t) = field.split_once(' ')?;
    // `Array(Nullable(Int64))` has no name; a name has no parenthesis
    (!name.contains('(')).then(|| (name.to_string(), t.trim()))
}

/// A ClickHouse column type as an Ibis type.
fn parse_type(name: &str) -> DataType {
    let name = bare_type(name);
    let args = |prefix: &str| -> Option<Vec<&str>> {
        Some(name.strip_prefix(prefix)?.strip_suffix(')')?.split(',').map(str::trim).collect())
    };
    match name {
        "Bool" => DataType::Boolean,
        "Int8" => DataType::Int8,
        "Int16" => DataType::Int16,
        "Int32" => DataType::Int32,
        "Int64" => DataType::Int64,
        "UInt8" => DataType::UInt8,
        "UInt16" => DataType::UInt16,
        "UInt32" => DataType::UInt32,
        "UInt64" => DataType::UInt64,
        "Float32" => DataType::Float32,
        "Float64" => DataType::Float64,
        "Date" | "Date32" => DataType::Date,
        "DateTime" | "DateTime64" => DataType::Timestamp { tz: None },
        "Nothing" => DataType::Null,
        _ => {
            if let Some(a) = args("Decimal(")
                && let [p, s] = a[..]
                && let (Ok(precision), Ok(scale)) = (p.parse(), s.parse())
            {
                return DataType::Decimal { precision, scale };
            }
            if let Some(a) = args("DateTime64(").or_else(|| args("DateTime(")) {
                let tz = a.iter().find(|x| x.starts_with('\'')).map(|x| x.trim_matches('\'').to_string());
                return DataType::Timestamp { tz };
            }
            let inner = |prefix: &str| name.strip_prefix(prefix).and_then(|s| s.strip_suffix(')'));
            if let Some(item) = inner("Array(") {
                return DataType::array(parse_type(item));
            }
            if let Some(kv) = inner("Map(")
                && let [key, value] = top_level(kv)[..]
            {
                return DataType::map(parse_type(key), parse_type(value));
            }
            if let Some(fields) = inner("Tuple(") {
                // `name Type`, or just `Type` in an unnamed tuple, whose fields Ibis calls f1, f2, ...
                return DataType::Struct(
                    top_level(fields)
                        .into_iter()
                        .enumerate()
                        .map(|(i, field)| match split_field(field) {
                            Some((name, t)) => (name, parse_type(t)),
                            None => (format!("f{}", i + 1), parse_type(field)),
                        })
                        .collect(),
                );
            }
            DataType::String
        }
    }
}

/// A result cell as a value of its column's type.
fn cell(field: &str, dtype: &DataType) -> Value {
    if field == "\\N" {
        return Value::Null;
    }
    let text = unescape(field);
    let typed = match dtype {
        DataType::Boolean => match text.as_str() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        t if t.is_integer() => text.parse().ok().map(Value::Int),
        DataType::Float32 | DataType::Float64 => text.parse().ok().map(Value::Float),
        DataType::Date => parse_date(&text).map(Value::Date),
        DataType::Timestamp { .. } => parse_timestamp(&text).map(Value::Timestamp),
        _ => None,
    };
    // anything else (decimals, a UInt64 past i64) stays text and is parsed to the output type
    typed.unwrap_or(Value::String(text))
}

impl ClickHouse {
    /// Connect over HTTP: `clickhouse://[user[:password]@]host[:port][/database]`
    /// (`clickhouses://` for HTTPS; `http://` and `https://` work too).
    pub fn connect(url: &str) -> Result<Self> {
        let u = parse_url(url)?;
        let config = ureq::Agent::config_builder().http_status_as_error(false).build();
        let ch = ClickHouse {
            agent: ureq::Agent::new_with_config(config),
            endpoint: format!("{}://{}/", u.scheme, u.host),
            database: if u.database.is_empty() { "default" } else { u.database }.to_string(),
            user: if u.user.is_empty() { "default" } else { u.user }.to_string(),
            password: u.password.to_string(),
            dialect: ClickHouseDialect,
        };
        ch.send("SELECT 1", None)?;
        Ok(ch)
    }

    /// POST one statement. With `data`, the statement goes in the URL and the
    /// data is the body (how `INSERT ... FORMAT` takes its rows).
    fn send(&self, sql: &str, data: Option<String>) -> Result<String> {
        let mut req = self
            .agent
            .post(&self.endpoint)
            .header("X-ClickHouse-User", &self.user)
            .header("X-ClickHouse-Key", &self.password)
            .header("X-ClickHouse-Database", &self.database);
        for (k, v) in SETTINGS {
            req = req.query(k, v);
        }
        let res = match data {
            Some(body) => req.query("query", sql).send(body),
            None => req.send(sql),
        };
        let mut res = res.map_err(|e| exec_err(sql, e))?;
        let status = res.status();
        let body = res.body_mut().with_config().limit(u64::MAX).read_to_string().map_err(|e| exec_err(sql, e))?;
        if status.is_success() { Ok(body) } else { Err(exec_err(sql, body.trim())) }
    }

    /// Run one statement that returns no rows.
    pub fn execute_statement(&self, sql: &str) -> Result<()> {
        self.send(sql, None).map(|_| ())
    }

    /// Run a query whose result has arrays, tuples or maps: read as JSON, each
    /// cell as a value of its column's type in `schema`.
    fn json_rows(&self, sql: &str, schema: &Schema) -> Result<Vec<Vec<Value>>> {
        let body = self.send(&format!("{sql}\nFORMAT JSONCompactEachRow"), None)?;
        body.lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let cells: Vec<serde_json::Value> = serde_json::from_str(line)
                    .map_err(|e| exec_err(sql, format!("unreadable result row {line:?}: {e}")))?;
                Ok(cells.iter().zip(&schema.fields).map(|(cell, f)| value_from_json(cell, &f.dtype)).collect())
            })
            .collect()
    }

    /// Run a query, returning rows typed by what ClickHouse says each column is.
    pub fn raw_sql(&self, sql: &str) -> Result<Vec<Vec<Value>>> {
        let body = self.send(&format!("{sql}\nFORMAT TabSeparatedWithNamesAndTypes"), None)?;
        let mut lines = body.split('\n');
        let _names = lines.next();
        let types: Vec<DataType> = lines.next().unwrap_or("").split('\t').map(|t| parse_type(&unescape(t))).collect();
        Ok(lines
            .filter(|line| !line.is_empty())
            .map(|line| line.split('\t').zip(&types).map(|(field, t)| cell(field, t)).collect())
            .collect())
    }
}

impl SqlClient for ClickHouse {
    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    fn statement(&self, sql: &str) -> Result<()> {
        self.execute_statement(sql)
    }

    fn load(&self, name: &str, schema: &Schema, data: &[RecordBatch]) -> Result<()> {
        let q = self.dialect.quote_ident(name);
        let mut body = String::new();
        if schema.fields.iter().any(|f| f.dtype.is_nested()) {
            // arrays, tuples and maps travel as JSON; a NULL one becomes the type's default
            for b in data {
                for row in batch_to_rows(b)? {
                    let cells: Vec<serde_json::Value> = row.iter().map(value_to_json).collect();
                    body.push_str(&serde_json::Value::Array(cells).to_string());
                    body.push('\n');
                }
            }
            if body.is_empty() {
                return Ok(());
            }
            return self.send(&format!("INSERT INTO {q} FORMAT JSONCompactEachRow"), Some(body)).map(|_| ());
        }
        for b in data {
            for row in batch_to_rows(b)? {
                for (i, v) in row.iter().enumerate() {
                    if i > 0 {
                        body.push('\t');
                    }
                    tsv_field(v, &mut body);
                }
                body.push('\n');
            }
        }
        if body.is_empty() {
            return Ok(());
        }
        self.send(&format!("INSERT INTO {q} FORMAT TabSeparated"), Some(body)).map(|_| ())
    }
}

impl Backend for ClickHouse {
    ibis_sql::sql_client_methods!();
    fn name(&self) -> &str {
        "clickhouse"
    }

    fn compile(&self, table: &Table) -> Result<String> {
        compile(&self.dialect, table)
    }

    fn execute(&self, table: &Table) -> Result<Vec<RecordBatch>> {
        let sql = self.compile(table)?;
        let schema = table.schema();
        let rows = if schema.fields.iter().any(|f| f.dtype.is_nested()) {
            self.json_rows(&sql, schema)?
        } else {
            self.raw_sql(&sql)?
        };
        Ok(vec![rows_to_batch(schema, &rows)?])
    }

    fn table(&self, name: &str) -> Result<Table> {
        let rows = self.raw_sql(&format!(
            "SELECT name, type FROM system.columns WHERE database = currentDatabase() AND table = {} ORDER BY position",
            quote_str(name)
        ))?;
        if rows.is_empty() {
            return Err(Error::Other(format!("clickhouse: no table {name:?}")));
        }
        let fields = rows
            .iter()
            .map(|r| match (&r[0], &r[1]) {
                (Value::String(col), Value::String(ty)) => Ok(Field::new(col, parse_type(ty))),
                other => Err(Error::Other(format!("clickhouse: unexpected system.columns row {other:?}"))),
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
        // insertion order is kept: no sorting key
        self.execute_statement(&format!(
            "{} ENGINE = MergeTree ORDER BY tuple()",
            create_table_sql(&self.dialect, name, &schema)?
        ))?;
        self.load(name, &schema, data)
    }

    fn list_tables(&self) -> Result<Vec<String>> {
        let rows = self.raw_sql("SELECT name FROM system.tables WHERE database = currentDatabase() ORDER BY name")?;
        Ok(rows
            .into_iter()
            .filter_map(|r| match r.into_iter().next() {
                Some(Value::String(s)) => Some(s),
                _ => None,
            })
            .collect())
    }
}
