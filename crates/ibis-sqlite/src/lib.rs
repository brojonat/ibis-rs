//! SQLite backend for ibis-rs.
//!
//! Two tools close the gap between SQLite and the Ibis semantics:
//!
//! - **Polyfills**: Rust implementations of functions SQLite lacks
//!   (`REVERSE`, `REPEAT`, `LPAD`, `REGEXP_*`, `STDDEV_SAMP`, ...), registered
//!   on the connection so the default SQL from `ibis-sql` runs unchanged.
//! - **Dialect overrides** where SQLite's *meaning* differs: LIKE is
//!   case-insensitive, dates are text, booleans are integers.
//!
//! Results are converted from SQLite's dynamic values to the Ibis output
//! schema. Column types round-trip through declared type names (`BOOLEAN`,
//! `TIMESTAMP`, ...), as Ibis's own SQLite backend does.

mod polyfill;

use std::sync::Mutex;

use arrow_array::RecordBatch;
use ibis_core::convert::{batch_to_rows, format_date, format_timestamp, rows_to_batch};
use ibis_core::{AggFunc, Backend, BinaryOp, DataType, Error, Expr, Field, Func, Result, Schema, Table, Value};
use ibis_sql::{Dialect, ExprCompiler, SqlClient, compile, create_table_sql, quote_str};
use rusqlite::Connection;
use rusqlite::types::Value as SqlValue;

pub struct SqliteDialect;

/// SQL LIKE pattern -> GLOB pattern (GLOB is case-sensitive, like Ibis's LIKE).
fn like_to_glob(p: &str) -> String {
    let mut out = String::new();
    for c in p.chars() {
        match c {
            '%' => out.push('*'),
            '_' => out.push('?'),
            '*' => out.push_str("[*]"),
            '?' => out.push_str("[?]"),
            '[' => out.push_str("[[]"),
            c => out.push(c),
        }
    }
    out
}

impl Dialect for SqliteDialect {
    fn name(&self) -> &str {
        "sqlite"
    }

    /// Declared types: chosen so SQLite's affinity rules store values sensibly
    /// *and* the name round-trips to the Ibis type in `Sqlite::table`.
    fn type_name(&self, t: &DataType) -> Result<String> {
        Ok(match t {
            DataType::Float64 => "DOUBLE".into(),
            DataType::Float32 => "FLOAT".into(),
            DataType::String => "TEXT".into(),
            DataType::Timestamp { .. } => "TIMESTAMP".into(),
            t if t.is_integer() || matches!(t, DataType::Boolean | DataType::Date | DataType::Binary) => {
                ibis_sql::default_type_name("sqlite", t)?
            }
            t => ibis_sql::default_type_name("sqlite", t)?,
        })
    }

    /// No TRUNCATE.
    fn truncate_table(&self, name: &str) -> String {
        format!("DELETE FROM {name}")
    }

    fn literal(&self, v: &Value, _t: &DataType) -> Result<Option<String>> {
        Ok(match v {
            Value::Bool(b) => Some((*b as i32).to_string()),
            // SQLite has no NaN (it becomes NULL); infinities are out-of-range literals
            Value::Float(f) if f.is_nan() => Some("NULL".into()),
            Value::Float(f) if f.is_infinite() => Some(if *f > 0.0 { "9e999" } else { "-9e999" }.into()),
            Value::Date(d) => Some(quote_str(&format_date(*d))),
            Value::Timestamp(t) => Some(quote_str(&format_timestamp(*t))),
            _ => None,
        })
    }

    fn cast(&self, c: &ExprCompiler, e: &Expr, to: &DataType) -> Result<Option<String>> {
        let x = c.sql(e)?;
        Ok(match to {
            DataType::Date => Some(format!("DATE({x})")),
            DataType::Timestamp { .. } => Some(format!("DATETIME({x})")),
            DataType::Boolean => Some(format!("(CAST({x} AS INTEGER) <> 0)")),
            DataType::String => Some(format!("CAST({x} AS TEXT)")),
            DataType::Float32 | DataType::Float64 => Some(format!("CAST({x} AS REAL)")),
            t if t.is_integer() => Some(format!("CAST({x} AS INTEGER)")),
            _ => None,
        })
    }

    fn supports_intersect_except_all(&self) -> bool {
        false
    }

    /// SQLite has no bare OFFSET: `LIMIT -1` means no limit.
    fn limit_offset(&self, limit: Option<u64>, offset: u64) -> String {
        match (limit, offset) {
            (None, 0) => String::new(),
            (None, o) => format!(" LIMIT -1 OFFSET {o}"),
            (Some(n), 0) => format!(" LIMIT {n}"),
            (Some(n), o) => format!(" LIMIT {n} OFFSET {o}"),
        }
    }

    fn binary(&self, c: &ExprCompiler, op: BinaryOp, l: &Expr, r: &Expr) -> Result<Option<String>> {
        // SQLite gives NULL for x / 0. Ibis's `/` follows IEEE 754; SQLite has
        // infinities but no NaN, so 0 / 0 stays NULL.
        if op == BinaryOp::Div
            && !matches!(r, Expr::Literal(v, _) if !matches!(v, Value::Int(0)) && *v != Value::Float(0.0))
        {
            let (ls, rs) = (c.sql(l)?, c.sql(r)?);
            return Ok(Some(format!(
                "CASE WHEN {rs} = 0 THEN CASE WHEN {ls} > 0 THEN 9e999 WHEN {ls} < 0 THEN -9e999 END \
                 ELSE CAST({ls} AS REAL) / {rs} END"
            )));
        }
        // `%` on REAL operands truncates them to integers in SQLite.
        if op == BinaryOp::Mod && (c.dtype(l)?.is_floating() || c.dtype(r)?.is_floating()) {
            return Ok(Some(format!("FMOD({}, {})", c.sql(l)?, c.sql(r)?)));
        }
        Ok(None)
    }

    fn func(&self, c: &ExprCompiler, f: Func, args: &[Expr]) -> Result<Option<String>> {
        let a = |i: usize| c.sql(&args[i]);
        let extract = |fmt: &str| -> Result<Option<String>> {
            Ok(Some(format!("CAST(STRFTIME('{fmt}', {}) AS INTEGER)", a(0)?)))
        };
        Ok(match f {
            Func::Like | Func::ILike => {
                let Expr::Literal(Value::String(p), _) = &args[1] else {
                    return Err(Error::Unsupported {
                        backend: "sqlite".into(),
                        op: "LIKE with a non-literal pattern".into(),
                    });
                };
                if f == Func::Like {
                    Some(format!("({} GLOB {})", a(0)?, quote_str(&like_to_glob(p))))
                } else {
                    Some(format!("(LOWER({}) LIKE LOWER({}))", a(0)?, a(1)?))
                }
            }
            // SQLite stores NaN as NULL, so there is nothing to test for
            Func::IsNan => return Err(Error::Unsupported { backend: "sqlite".into(), op: "isnan".into() }),
            Func::Ascii => Some(format!("UNICODE({})", a(0)?)),
            Func::Right => Some(format!("SUBSTR({}, -({}))", a(0)?, a(1)?)),
            Func::Year => extract("%Y")?,
            Func::Month => extract("%m")?,
            Func::Day => extract("%d")?,
            Func::Hour => extract("%H")?,
            Func::Minute => extract("%M")?,
            Func::Second => extract("%S")?,
            Func::DayOfWeekIndex => Some(format!("((CAST(STRFTIME('%w', {}) AS INTEGER) + 6) % 7)", a(0)?)),
            Func::IsInf => Some(format!("(ABS({}) = 9e999)", a(0)?)),
            Func::Radians => Some(format!("({} * 0.017453292519943295)", a(0)?)),
            Func::Degrees => Some(format!("({} * 57.29577951308232)", a(0)?)),
            // SQLite's scalar MIN/MAX: NULL if any argument is NULL
            Func::Least => {
                Some(format!("MIN({})", args.iter().map(|x| c.sql(x)).collect::<Result<Vec<_>>>()?.join(", ")))
            }
            Func::Greatest => {
                Some(format!("MAX({})", args.iter().map(|x| c.sql(x)).collect::<Result<Vec<_>>>()?.join(", ")))
            }
            Func::Clip => {
                let open = |e: &Expr| matches!(e, Expr::Literal(Value::Null, _));
                let x = a(0)?;
                let mut s = x.clone();
                if !open(&args[1]) {
                    s = format!("MAX({s}, {})", a(1)?);
                }
                if !open(&args[2]) {
                    s = format!("MIN({s}, {})", a(2)?);
                }
                Some(format!("CASE WHEN {x} IS NULL THEN NULL ELSE {s} END"))
            }
            Func::DayOfYear => extract("%j")?,
            Func::WeekOfYear => extract("%V")?,
            Func::Quarter => Some(format!("((CAST(STRFTIME('%m', {}) AS INTEGER) + 2) / 3)", a(0)?)),
            // Timestamps are ISO text; STRFTIME's %f rounds to milliseconds,
            // so read the fraction's digits instead.
            Func::Microsecond | Func::Millisecond => {
                let x = a(0)?;
                let micros = format!(
                    "CASE WHEN INSTR({x}, '.') > 0 \
                     THEN CAST(SUBSTR(SUBSTR({x}, INSTR({x}, '.') + 1) || '000000', 1, 6) AS INTEGER) ELSE 0 END"
                );
                Some(if f == Func::Microsecond { format!("({micros})") } else { format!("(({micros}) / 1000)") })
            }
            Func::DayOfWeekName => Some(format!(
                "CASE STRFTIME('%w', {}) WHEN '0' THEN 'Sunday' WHEN '1' THEN 'Monday' WHEN '2' THEN 'Tuesday' \
                 WHEN '3' THEN 'Wednesday' WHEN '4' THEN 'Thursday' WHEN '5' THEN 'Friday' WHEN '6' THEN 'Saturday' END",
                a(0)?
            )),
            Func::Strftime => Some(format!("STRFTIME({}, {})", a(1)?, a(0)?)),
            Func::DateFromYmd => Some(format!("DATE(PRINTF('%04d-%02d-%02d', {}, {}, {}))", a(0)?, a(1)?, a(2)?)),
            Func::TimestampFromYmdhms => Some(format!(
                "DATETIME(PRINTF('%04d-%02d-%02d %02d:%02d:%02d', {}, {}, {}, {}, {}, {}))",
                a(0)?,
                a(1)?,
                a(2)?,
                a(3)?,
                a(4)?,
                a(5)?
            )),
            Func::Truncate => {
                let x = a(0)?;
                let s = match ExprCompiler::lit_str(&args[1]) {
                    Some("Y") => format!("DATETIME({x}, 'start of year')"),
                    Some("Q") => format!(
                        "DATETIME(STRFTIME('%Y', {x}) || '-' || \
                         PRINTF('%02d', ((CAST(STRFTIME('%m', {x}) AS INTEGER) - 1) / 3) * 3 + 1) || '-01')"
                    ),
                    Some("M") => format!("DATETIME({x}, 'start of month')"),
                    // back to Monday
                    Some("W") => format!(
                        "DATETIME({x}, 'start of day', '-' || ((CAST(STRFTIME('%w', {x}) AS INTEGER) + 6) % 7) || ' days')"
                    ),
                    Some("D") => format!("DATETIME({x}, 'start of day')"),
                    Some("h") => format!("STRFTIME('%Y-%m-%d %H:00:00', {x})"),
                    Some("m") => format!("STRFTIME('%Y-%m-%d %H:%M:00', {x})"),
                    Some("s") => format!("STRFTIME('%Y-%m-%d %H:%M:%S', {x})"),
                    Some("ms") => format!("STRFTIME('%Y-%m-%d %H:%M:%f', {x})"),
                    Some("us") => x,
                    _ => return Err(Error::Unsupported { backend: "sqlite".into(), op: "truncate unit".into() }),
                };
                Some(if c.dtype(&args[0])? == DataType::Date { format!("DATE({s})") } else { s })
            }
            _ => None,
        })
    }

    fn agg(&self, c: &ExprCompiler, f: AggFunc, arg: Option<&Expr>, filter: &str) -> Result<Option<String>> {
        Ok(match (f, arg) {
            (AggFunc::Any, Some(x)) => Some(format!("(MAX({}){filter} <> 0)", c.sql(x)?)),
            (AggFunc::All, Some(x)) => Some(format!("(MIN({}){filter} <> 0)", c.sql(x)?)),
            _ => None,
        })
    }
}

pub struct Sqlite {
    conn: Mutex<Connection>,
    dialect: SqliteDialect,
}

fn exec_err(sql: &str, e: rusqlite::Error) -> Error {
    Error::Execution { backend: "sqlite".into(), sql: sql.into(), message: e.to_string() }
}

/// Declared column type -> Ibis type (inverse of `SqliteDialect::type_name`,
/// plus SQLite's own affinity rules for tables we didn't create).
fn parse_declared(decl: &str) -> DataType {
    let d = decl.to_ascii_uppercase();
    match d.as_str() {
        "BOOLEAN" => DataType::Boolean,
        "TINYINT" => DataType::Int8,
        "SMALLINT" => DataType::Int16,
        "INTEGER" => DataType::Int32,
        "BIGINT" | "INT" => DataType::Int64,
        "FLOAT" | "REAL" => DataType::Float32,
        "DOUBLE" => DataType::Float64,
        "DATE" => DataType::Date,
        "TIMESTAMP" | "DATETIME" => DataType::Timestamp { tz: None },
        "BLOB" => DataType::Binary,
        d if d.contains("INT") => DataType::Int64,
        d if d.contains("CHAR") || d.contains("CLOB") || d.contains("TEXT") => DataType::String,
        d if d.contains("REAL") || d.contains("FLOA") || d.contains("DOUB") => DataType::Float64,
        _ => DataType::String,
    }
}

fn to_sql_value(v: Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(b as i64),
        Value::Int(i) => SqlValue::Integer(i),
        Value::Float(f) => SqlValue::Real(f),
        Value::String(s) => SqlValue::Text(s),
        Value::Binary(b) => SqlValue::Blob(b),
        Value::Date(d) => SqlValue::Text(format_date(d)),
        Value::Timestamp(t) => SqlValue::Text(format_timestamp(t)),
        // no nested types; `create_table` refuses them before a value gets here
        other @ (Value::Array(_) | Value::Struct(_) | Value::Map(_)) => SqlValue::Text(format!("{other:?}")),
    }
}

fn from_sql_value(v: SqlValue) -> Value {
    match v {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(i) => Value::Int(i),
        SqlValue::Real(f) => Value::Float(f),
        SqlValue::Text(s) => Value::String(s),
        SqlValue::Blob(b) => Value::Binary(b),
    }
}

impl Sqlite {
    pub fn new(conn: Connection) -> Result<Self> {
        polyfill::register(&conn).map_err(|e| exec_err("<register polyfills>", e))?;
        Ok(Sqlite { conn: Mutex::new(conn), dialect: SqliteDialect })
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::new(Connection::open_in_memory().map_err(|e| exec_err("<open>", e))?)
    }

    pub fn open(path: &str) -> Result<Self> {
        Self::new(Connection::open(path).map_err(|e| exec_err("<open>", e))?)
    }

    /// Run SQL, returning dynamically typed rows.
    pub fn raw_sql(&self, sql: &str) -> Result<Vec<Vec<Value>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql).map_err(|e| exec_err(sql, e))?;
        let n = stmt.column_count();
        let rows = stmt
            .query_map([], |r| (0..n).map(|i| r.get::<_, SqlValue>(i).map(from_sql_value)).collect())
            .map_err(|e| exec_err(sql, e))?;
        rows.collect::<std::result::Result<_, _>>().map_err(|e| exec_err(sql, e))
    }
}

impl SqlClient for Sqlite {
    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    fn statement(&self, sql: &str) -> Result<()> {
        self.conn.lock().unwrap().execute_batch(sql).map_err(|e| exec_err(sql, e))
    }

    fn load(&self, name: &str, schema: &Schema, data: &[RecordBatch]) -> Result<()> {
        let q = self.dialect.quote_ident(name);
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| exec_err("BEGIN", e))?;
        let placeholders = vec!["?"; schema.len()].join(", ");
        let ins = format!("INSERT INTO {q} VALUES ({placeholders})");
        {
            let mut stmt = tx.prepare(&ins).map_err(|e| exec_err(&ins, e))?;
            for b in data {
                for row in batch_to_rows(b)? {
                    let vals: Vec<SqlValue> = row.into_iter().map(to_sql_value).collect();
                    stmt.execute(rusqlite::params_from_iter(vals)).map_err(|e| exec_err(&ins, e))?;
                }
            }
        }
        tx.commit().map_err(|e| exec_err("COMMIT", e))
    }
}

impl Backend for Sqlite {
    ibis_sql::sql_client_methods!();
    fn name(&self) -> &str {
        "sqlite"
    }

    fn compile(&self, table: &Table) -> Result<String> {
        compile(&self.dialect, table)
    }

    fn execute(&self, table: &Table) -> Result<Vec<RecordBatch>> {
        let sql = self.compile(table)?;
        let rows = self.raw_sql(&sql)?;
        Ok(vec![rows_to_batch(table.schema(), &rows)?])
    }

    fn table(&self, name: &str) -> Result<Table> {
        let sql = format!("SELECT name, type FROM pragma_table_info({})", quote_str(name));
        let rows = self.raw_sql(&sql)?;
        if rows.is_empty() {
            return Err(Error::Other(format!("sqlite: no table {name:?}")));
        }
        let fields = rows
            .into_iter()
            .map(|r| match (&r[0], &r[1]) {
                (Value::String(n), Value::String(t)) => Ok(Field::new(n, parse_declared(t))),
                other => Err(Error::Other(format!("unexpected table_info row {other:?}"))),
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
        let ddl = format!("DROP TABLE IF EXISTS {q}; {}", create_table_sql(&self.dialect, name, &schema)?);
        self.statement(&ddl)?;
        self.load(name, &schema, data)
    }

    fn list_tables(&self) -> Result<Vec<String>> {
        let rows = self.raw_sql("SELECT name FROM sqlite_master WHERE type IN ('table', 'view') ORDER BY 1")?;
        Ok(rows
            .into_iter()
            .filter_map(|r| match r.into_iter().next() {
                Some(Value::String(s)) => Some(s),
                _ => None,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use ibis_core::prelude::*;

    use super::*;

    #[test]
    fn like_is_case_sensitive_like_ibis() {
        assert_eq!(like_to_glob("6%_x*"), "6*?x[*]");
    }

    #[test]
    fn end_to_end_with_polyfills_and_types() {
        let con = Sqlite::open_in_memory().unwrap();
        let schema =
            Schema::of(&[("s", DataType::String), ("b", DataType::Boolean), ("ts", DataType::Timestamp { tz: None })]);
        let rows = vec![
            vec![Value::from("Abc"), Value::Bool(true), Value::Timestamp(1_262_304_000_000_000)],
            vec![Value::from("abc"), Value::Bool(false), Value::Timestamp(1_262_390_400_000_000)],
        ];
        con.create_table("t", &[rows_to_batch(&schema, &rows).unwrap()]).unwrap();
        let t = con.table("t").unwrap();
        assert_eq!(t.schema(), &schema);
        let q = t
            .filter([col("s").like("A%")])
            .unwrap()
            .select([
                col("s").reverse().name("r"),
                col("s").lpad(5, "-").name("p"),
                col("ts").year().name("y"),
                col("b"),
            ])
            .unwrap();
        let out = batch_to_rows(&con.execute(&q).unwrap()[0]).unwrap();
        assert_eq!(out, vec![vec![Value::from("cbA"), Value::from("--Abc"), Value::Int(2010), Value::Bool(true)]]);
    }
}
