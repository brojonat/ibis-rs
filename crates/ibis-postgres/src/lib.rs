//! Postgres backend for ibis-rs.
//!
//! The dialect overrides only what Postgres spells or means differently from
//! the DuckDB-flavored defaults in `ibis-sql`: `DOUBLE PRECISION`, `~` for
//! regex search, no `LOG2`, `ROUND`/`%` that need `NUMERIC`, and integer
//! aggregates that return `NUMERIC`.
//!
//! Results come back over the binary protocol and are converted to the Ibis
//! output schema; `NUMERIC` is decoded to its exact decimal text first.

use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::Mutex;

use arrow_array::RecordBatch;
use chrono::{NaiveDate, NaiveDateTime};
use ibis_core::convert::{batch_to_rows, format_date, format_timestamp, rows_to_batch};
use ibis_core::{AggFunc, Backend, BinaryOp, DataType, Error, Expr, Field, Func, Result, Schema, Table, Value};
use ibis_sql::{Dialect, ExprCompiler, compile, create_table_sql, quote_str};
use postgres::types::{FromSql, Type};
use postgres::{Client, NoTls, Row};

pub struct PostgresDialect;

impl Dialect for PostgresDialect {
    fn name(&self) -> &str {
        "postgres"
    }

    fn type_name(&self, t: &DataType) -> Result<String> {
        Ok(match t {
            DataType::Int8 | DataType::UInt8 => "SMALLINT".into(),
            DataType::UInt16 => "INTEGER".into(),
            DataType::UInt32 => "BIGINT".into(),
            DataType::UInt64 => "NUMERIC(20, 0)".into(),
            DataType::Float64 => "DOUBLE PRECISION".into(),
            DataType::String => "TEXT".into(),
            DataType::Binary => "BYTEA".into(),
            DataType::Timestamp { tz: Some(_) } => "TIMESTAMPTZ".into(),
            t => ibis_sql::default_type_name("postgres", t)?,
        })
    }

    fn binary(&self, c: &ExprCompiler, op: BinaryOp, l: &Expr, r: &Expr) -> Result<Option<String>> {
        // Postgres raises on division by zero; Ibis's `/` follows IEEE 754:
        // x / 0 is +/-Infinity, 0 / 0 is NaN.
        if op == BinaryOp::Div
            && !matches!(r, Expr::Literal(v, _) if !matches!(v, Value::Int(0)) && *v != Value::Float(0.0))
        {
            let (ls, rs) = (c.sql(l)?, c.sql(r)?);
            let f = "DOUBLE PRECISION";
            return Ok(Some(format!(
                "CASE WHEN {rs} = 0 THEN CAST(CASE WHEN {ls} IS NULL THEN NULL WHEN {ls} > 0 THEN 'Infinity' \
                 WHEN {ls} < 0 THEN '-Infinity' ELSE 'NaN' END AS {f}) ELSE CAST({ls} AS {f}) / {rs} END"
            )));
        }
        // `%` isn't defined for DOUBLE PRECISION.
        if op == BinaryOp::Mod && (c.dtype(l)?.is_floating() || c.dtype(r)?.is_floating()) {
            return Ok(Some(format!(
                "CAST(MOD(CAST({} AS NUMERIC), CAST({} AS NUMERIC)) AS DOUBLE PRECISION)",
                c.sql(l)?,
                c.sql(r)?
            )));
        }
        Ok(None)
    }

    fn func(&self, c: &ExprCompiler, f: Func, args: &[Expr]) -> Result<Option<String>> {
        let a = |i: usize| c.sql(&args[i]);
        let float_arg = c.dtype(&args[0]).map(|t| !t.is_integer()).unwrap_or(false);
        Ok(match f {
            Func::RegexSearch => Some(format!("({} ~ {})", a(0)?, a(1)?)),
            // Wrap the whole pattern in a group so Ibis's index 0 (whole match)
            // is Postgres group 1. No match gives '' like DuckDB.
            Func::RegexExtract => {
                let x = a(0)?;
                Some(format!(
                    "CASE WHEN {x} IS NULL THEN NULL ELSE COALESCE((REGEXP_MATCH({x}, '(' || {} || ')'))[{} + 1], '') END",
                    a(1)?,
                    a(2)?
                ))
            }
            // Postgres orders NaN as equal to itself (and above everything)
            Func::IsNan => Some(format!("({} = CAST('NaN' AS DOUBLE PRECISION))", a(0)?)),
            Func::IsInf => {
                let x = a(0)?;
                Some(format!(
                    "({x} = CAST('Infinity' AS DOUBLE PRECISION) OR {x} = CAST('-Infinity' AS DOUBLE PRECISION))"
                ))
            }
            Func::DayOfWeekName => Some(format!("TRIM(TO_CHAR({}, 'Day'))", a(0)?)),
            Func::Strftime => {
                let Some(fmt) = ExprCompiler::lit_str(&args[1]) else {
                    return Err(Error::Unsupported {
                        backend: "postgres".into(),
                        op: "strftime with a non-literal format".into(),
                    });
                };
                Some(format!("TO_CHAR({}, {})", a(0)?, quote_str(&to_char_format(fmt)?)))
            }
            Func::Log2 => Some(format!("(LN({x}) / LN(2.0))", x = a(0)?)),
            Func::Log10 => Some(format!("LOG({})", a(0)?)),
            // ROUND(DOUBLE PRECISION) rounds half to even; NUMERIC rounds half
            // away from zero like DuckDB, and only NUMERIC takes a digits arg.
            Func::Round if float_arg => match args.get(1) {
                None => Some(format!("CAST(ROUND(CAST({} AS NUMERIC)) AS BIGINT)", a(0)?)),
                Some(_) => Some(format!("CAST(ROUND(CAST({} AS NUMERIC), {}) AS DOUBLE PRECISION)", a(0)?, a(1)?)),
            },
            _ => None,
        })
    }

    fn agg(&self, c: &ExprCompiler, f: AggFunc, arg: Option<&Expr>, filter: &str) -> Result<Option<String>> {
        // Postgres returns NUMERIC for SUM(BIGINT) and for AVG/STDDEV/VAR of
        // integers; cast back to the Ibis output type.
        let Some(x) = arg else { return Ok(None) };
        let t = c.dtype(x)?;
        let int_like = t.is_integer() || t == DataType::Boolean;
        let inner = |name: &str| -> Result<String> {
            let s = c.sql(x)?;
            // no direct BOOLEAN -> BIGINT cast in Postgres
            let s = if t == DataType::Boolean { format!("CAST({s} AS INTEGER)") } else { s };
            Ok(format!("{name}({s}){filter}"))
        };
        Ok(match f {
            AggFunc::Sum if int_like => Some(format!("CAST({} AS BIGINT)", inner("SUM")?)),
            AggFunc::Mean | AggFunc::StdSamp | AggFunc::StdPop | AggFunc::VarSamp | AggFunc::VarPop if int_like => {
                let name = match f {
                    AggFunc::Mean => "AVG",
                    AggFunc::StdSamp => "STDDEV_SAMP",
                    AggFunc::StdPop => "STDDEV_POP",
                    AggFunc::VarSamp => "VAR_SAMP",
                    _ => "VAR_POP",
                };
                Some(format!("CAST({} AS DOUBLE PRECISION)", inner(name)?))
            }
            _ => None,
        })
    }
}

/// A C `strftime` pattern as a Postgres `TO_CHAR` template. Literal text is
/// double-quoted so letters in it aren't read as template patterns.
fn to_char_format(fmt: &str) -> Result<String> {
    let mut out = String::new();
    let mut text = String::new();
    let flush = |text: &mut String, out: &mut String| {
        if !text.is_empty() {
            out.push_str(&format!("\"{}\"", text.replace('"', "\\\"")));
            text.clear();
        }
    };
    let mut chars = fmt.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            text.push(ch);
            continue;
        }
        let pattern = match chars.next() {
            Some('Y') => "YYYY",
            Some('y') => "YY",
            Some('m') => "MM",
            Some('d') => "DD",
            Some('H') => "HH24",
            Some('I') => "HH12",
            Some('M') => "MI",
            Some('S') => "SS",
            Some('f') => "US",
            Some('j') => "DDD",
            Some('p') => "AM",
            Some('B') => "FMMonth",
            Some('b') => "Mon",
            Some('A') => "FMDay",
            Some('a') => "Dy",
            Some('%') => {
                text.push('%');
                continue;
            }
            other => {
                return Err(Error::Unsupported {
                    backend: "postgres".into(),
                    op: format!("strftime directive %{}", other.map(String::from).unwrap_or_default()),
                });
            }
        };
        flush(&mut text, &mut out);
        out.push_str(pattern);
    }
    flush(&mut text, &mut out);
    Ok(out)
}

pub struct Postgres {
    client: Mutex<Client>,
    dialect: PostgresDialect,
}

fn exec_err(sql: &str, e: impl std::fmt::Display) -> Error {
    Error::Execution { backend: "postgres".into(), sql: sql.into(), message: e.to_string() }
}

/// `postgres::Error` displays server errors as just "db error"; surface the
/// server's message, detail and hint.
fn pg_err(sql: &str, e: postgres::Error) -> Error {
    let message = match e.as_db_error() {
        Some(db) => {
            let mut m = format!("{}: {}", db.severity(), db.message());
            if let Some(d) = db.detail() {
                m.push_str(&format!(" (detail: {d})"));
            }
            if let Some(h) = db.hint() {
                m.push_str(&format!(" (hint: {h})"));
            }
            m
        }
        None => e.to_string(),
    };
    Error::Execution { backend: "postgres".into(), sql: sql.into(), message }
}

/// `information_schema.columns.data_type` -> Ibis type.
fn parse_type(name: &str, precision: Option<i32>, scale: Option<i32>) -> DataType {
    match name {
        "boolean" => DataType::Boolean,
        "smallint" => DataType::Int16,
        "integer" => DataType::Int32,
        "bigint" => DataType::Int64,
        "real" => DataType::Float32,
        "double precision" => DataType::Float64,
        "numeric" => match (precision, scale) {
            (Some(p), Some(s)) => DataType::Decimal { precision: p as u8, scale: s as i8 },
            _ => DataType::Float64,
        },
        "date" => DataType::Date,
        "timestamp without time zone" => DataType::Timestamp { tz: None },
        "timestamp with time zone" => DataType::Timestamp { tz: Some("UTC".into()) },
        "bytea" => DataType::Binary,
        _ => DataType::String,
    }
}

/// NUMERIC in binary form, decoded to exact decimal text (`as_f64`/`as_i64`
/// in `ibis_core::convert` parse it to the output type).
struct NumericText(String);

impl<'a> FromSql<'a> for NumericText {
    fn from_sql(_: &Type, raw: &'a [u8]) -> std::result::Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        let word = |i: usize| -> std::result::Result<i16, Box<dyn std::error::Error + Sync + Send>> {
            Ok(i16::from_be_bytes(raw.get(i..i + 2).ok_or("short numeric")?.try_into()?))
        };
        let (ndigits, weight, sign, dscale) = (word(0)? as usize, word(2)? as i32, word(4)? as u16, word(6)? as usize);
        let digits = (0..ndigits).map(|i| word(8 + 2 * i)).collect::<std::result::Result<Vec<_>, _>>()?;
        let special = match sign {
            0xC000 => Some("NaN"),
            0xD000 => Some("inf"),
            0xF000 => Some("-inf"),
            _ => None,
        };
        if let Some(s) = special {
            return Ok(NumericText(s.into()));
        }
        let mut s = String::new();
        if sign == 0x4000 {
            s.push('-');
        }
        let digit = |i: i32| if i >= 0 { digits.get(i as usize).copied().unwrap_or(0) } else { 0 };
        if weight < 0 {
            s.push('0');
        } else {
            for i in 0..=weight {
                if i == 0 { write!(s, "{}", digit(i)) } else { write!(s, "{:04}", digit(i)) }?;
            }
        }
        if dscale > 0 {
            let mut frac = String::new();
            let mut i = weight + 1;
            while frac.len() < dscale {
                write!(frac, "{:04}", digit(i))?;
                i += 1;
            }
            frac.truncate(dscale);
            s.push('.');
            s.push_str(&frac);
        }
        Ok(NumericText(s))
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::NUMERIC
    }
}

fn get<'a, T: FromSql<'a>>(row: &'a Row, i: usize) -> Result<Option<T>> {
    row.try_get::<_, Option<T>>(i).map_err(|e| exec_err("<decode>", e))
}

fn epoch() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()
}

fn cell(row: &Row, i: usize) -> Result<Value> {
    let ty = row.columns()[i].type_();
    let v = match *ty {
        Type::BOOL => get::<bool>(row, i)?.map(Value::Bool),
        Type::INT2 => get::<i16>(row, i)?.map(|x| Value::Int(x.into())),
        Type::INT4 => get::<i32>(row, i)?.map(|x| Value::Int(x.into())),
        Type::INT8 => get::<i64>(row, i)?.map(Value::Int),
        Type::FLOAT4 => get::<f32>(row, i)?.map(|x| Value::Float(x.into())),
        Type::FLOAT8 => get::<f64>(row, i)?.map(Value::Float),
        Type::NUMERIC => get::<NumericText>(row, i)?.map(|n| Value::String(n.0)),
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN => {
            get::<String>(row, i)?.map(Value::String)
        }
        Type::BYTEA => get::<Vec<u8>>(row, i)?.map(Value::Binary),
        Type::DATE => get::<NaiveDate>(row, i)?.map(|d| Value::Date((d - epoch()).num_days() as i32)),
        Type::TIMESTAMP => get::<NaiveDateTime>(row, i)?.map(|t| Value::Timestamp(t.and_utc().timestamp_micros())),
        Type::TIMESTAMPTZ => {
            get::<chrono::DateTime<chrono::Utc>>(row, i)?.map(|t| Value::Timestamp(t.timestamp_micros()))
        }
        ref other => {
            return Err(Error::Unsupported {
                backend: "postgres".into(),
                op: format!("result column of type {other}"),
            });
        }
    };
    Ok(v.unwrap_or(Value::Null))
}

/// One CSV field for `COPY ... (FORMAT csv)`: unquoted empty is NULL,
/// everything else is quoted so empty strings survive.
fn csv_field(v: &Value) -> String {
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) if f.is_nan() => "NaN".into(),
        Value::Float(f) if f.is_infinite() => if *f > 0.0 { "Infinity" } else { "-Infinity" }.into(),
        Value::Float(f) => format!("{f:?}"),
        Value::String(s) => q(s),
        Value::Binary(b) => q(&format!("\\x{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())),
        Value::Date(d) => format_date(*d),
        Value::Timestamp(t) => format_timestamp(*t),
        // TODO(nested)
        Value::Array(_) | Value::Struct(_) | Value::Map(_) => String::new(),
    }
}

impl Postgres {
    pub fn new(client: Client) -> Self {
        Postgres { client: Mutex::new(client), dialect: PostgresDialect }
    }

    /// Connect with a libpq-style string, e.g. `host=localhost user=postgres dbname=ibis_testing`
    /// or `postgresql://user@host/db`.
    pub fn connect(params: &str) -> Result<Self> {
        let client = Client::connect(params, NoTls).map_err(|e| exec_err("<connect>", e))?;
        Ok(Self::new(client))
    }

    /// Run SQL, returning dynamically typed rows.
    pub fn raw_sql(&self, sql: &str) -> Result<Vec<Vec<Value>>> {
        let rows = self.client.lock().unwrap().query(sql, &[]).map_err(|e| pg_err(sql, e))?;
        rows.iter().map(|r| (0..r.len()).map(|i| cell(r, i)).collect()).collect()
    }

    /// Run one or more statements that return no rows.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        self.client.lock().unwrap().batch_execute(sql).map_err(|e| pg_err(sql, e))
    }
}

impl Backend for Postgres {
    fn name(&self) -> &str {
        "postgres"
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
        let sql = format!(
            "SELECT column_name::text, data_type::text, numeric_precision, numeric_scale \
             FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = {} ORDER BY ordinal_position",
            quote_str(name)
        );
        let rows = self.client.lock().unwrap().query(&sql, &[]).map_err(|e| pg_err(&sql, e))?;
        if rows.is_empty() {
            return Err(Error::Other(format!("postgres: no table {name:?}")));
        }
        let fields = rows
            .iter()
            .map(|r| {
                let col: String = r.get(0);
                let ty: String = r.get(1);
                Field::new(&col, parse_type(&ty, r.get(2), r.get(3)))
            })
            .collect();
        Ok(Table::named(name, Schema::new(fields)))
    }

    fn create_table(&self, name: &str, data: &[RecordBatch]) -> Result<()> {
        let Some(first) = data.first() else {
            return Err(Error::Other("create_table needs at least one batch".into()));
        };
        let schema = Schema::from_arrow(&first.schema())?;
        let q = self.dialect.quote_ident(name);
        let mut client = self.client.lock().unwrap();
        let mut tx = client.transaction().map_err(|e| exec_err("BEGIN", e))?;
        let ddl = format!("DROP TABLE IF EXISTS {q}; {}", create_table_sql(&self.dialect, name, &schema)?);
        tx.batch_execute(&ddl).map_err(|e| pg_err(&ddl, e))?;
        let copy = format!("COPY {q} FROM STDIN (FORMAT csv)");
        let mut w = tx.copy_in(&copy).map_err(|e| exec_err(&copy, e))?;
        for b in data {
            for row in batch_to_rows(b)? {
                let line = row.iter().map(csv_field).collect::<Vec<_>>().join(",");
                writeln!(w, "{line}").map_err(|e| exec_err(&copy, e))?;
            }
        }
        w.finish().map_err(|e| pg_err(&copy, e))?;
        tx.commit().map_err(|e| exec_err("COMMIT", e))
    }

    fn list_tables(&self) -> Result<Vec<String>> {
        let rows = self.raw_sql(
            "SELECT table_name::text FROM information_schema.tables WHERE table_schema = current_schema() ORDER BY 1",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_decoding() {
        // 12345.678 = digits [1, 2345, 6780], weight 1, dscale 3
        let mut raw = vec![];
        for w in [3i16, 1, 0, 3, 1, 2345, 6780] {
            raw.extend_from_slice(&w.to_be_bytes());
        }
        assert_eq!(NumericText::from_sql(&Type::NUMERIC, &raw).unwrap().0, "12345.678");
        // -0.05 = digits [500], weight -1, dscale 2
        let mut raw = vec![];
        for w in [1i16, -1, 0x4000, 2, 500] {
            raw.extend_from_slice(&w.to_be_bytes());
        }
        assert_eq!(NumericText::from_sql(&Type::NUMERIC, &raw).unwrap().0, "-0.05");
    }

    #[test]
    fn csv_quoting() {
        assert_eq!(csv_field(&Value::Null), "");
        assert_eq!(csv_field(&Value::from("")), "\"\"");
        assert_eq!(csv_field(&Value::from("a\"b")), "\"a\"\"b\"");
    }
}
