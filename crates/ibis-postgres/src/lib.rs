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
use ibis_core::{
    AggFunc, AggOptions, Backend, BinaryOp, DataType, Error, Expr, Field, Func, Lambda, Result, Schema, Table, Value,
};
use ibis_sql::json::{decode_nested, value_to_json};
use ibis_sql::{Dialect, ExprCompiler, SqlClient, compile, quote_str};
use postgres::types::{FromSql, Kind, Type};
use postgres::{Client, NoTls, Row};

mod nested;

pub struct PostgresDialect;

impl Dialect for PostgresDialect {
    fn name(&self) -> &str {
        "postgres"
    }
    fn merge(&self) -> bool {
        true
    }

    fn type_name(&self, t: &DataType) -> Result<String> {
        if let Some(name) = nested::type_name(self, t)? {
            return Ok(name);
        }
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

    fn nested_literal(&self, c: &ExprCompiler, v: &Value, t: &DataType) -> Result<Option<String>> {
        nested::literal(c, v, t)
    }

    fn array_lambda(
        &self,
        c: &ExprCompiler,
        kind: Lambda,
        arg: &Expr,
        param: &str,
        index: Option<&str>,
        body: &Expr,
    ) -> Result<Option<String>> {
        nested::array_lambda(c, kind, arg, param, index, body).map(Some)
    }

    fn func(&self, c: &ExprCompiler, f: Func, args: &[Expr]) -> Result<Option<String>> {
        if let Some(sql) = nested::func(self, c, f, args)? {
            return Ok(Some(sql));
        }
        let a = |i: usize| c.sql(&args[i]);
        let float_arg = args.first().is_some_and(|x| c.dtype(x).map(|t| !t.is_integer()).unwrap_or(false));
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
            AggFunc::Mode => Some(format!("MODE() WITHIN GROUP (ORDER BY {}){filter}", c.sql(x)?)),
            // the middle value of what cannot be interpolated is one of the values
            AggFunc::Median | AggFunc::ApproxMedian => {
                let percentile = if t.is_numeric() { "PERCENTILE_CONT" } else { "PERCENTILE_DISC" };
                Some(format!("{percentile}(0.5) WITHIN GROUP (ORDER BY {}){filter}", c.sql(x)?))
            }
            AggFunc::ApproxCountDistinct => Some(format!("COUNT(DISTINCT {}){filter}", c.sql(x)?)),
            AggFunc::BitAnd => Some(inner("BIT_AND")?),
            AggFunc::BitOr => Some(inner("BIT_OR")?),
            AggFunc::BitXor => Some(inner("BIT_XOR")?),
            AggFunc::Arbitrary => Some(format!("ANY_VALUE({}){filter}", c.sql(x)?)),
            // no FIRST or LAST: the ends of the values gathered
            AggFunc::First | AggFunc::Last => Some(ends(f, &format!("ARRAY_AGG({}){filter}", c.sql(x)?))),
            _ => None,
        })
    }
    /// The unit goes inside the quotes: `INTERVAL '1' MILLISECOND` is not
    /// Postgres. A date moved is a timestamp.
    fn date_shift(
        &self,
        c: &ExprCompiler,
        moment: &str,
        t: &DataType,
        step: &ibis_core::Shift,
    ) -> Result<Option<String>> {
        if step.unit == ibis_core::IntervalUnit::Nanosecond {
            return Err(Error::Unsupported { backend: "postgres".into(), op: "an interval of nanoseconds".into() });
        }
        let sign = if step.back { '-' } else { '+' };
        let unit = ibis_sql::interval_unit_name(step.unit);
        let moved = match step.literal() {
            Some(n) => format!("{moment} {sign} INTERVAL '{n} {unit}'"),
            None => format!("{moment} {sign} {} * INTERVAL '1 {unit}'", c.sql(&step.count)?),
        };
        Ok(Some(match t {
            DataType::Date => format!("CAST({moved} AS DATE)"),
            _ => format!("({moved})"),
        }))
    }
    fn reduction(
        &self,
        c: &ExprCompiler,
        f: AggFunc,
        arg: &Expr,
        options: &AggOptions,
        tail: &str,
    ) -> Result<Option<String>> {
        match f {
            AggFunc::ArgMin | AggFunc::ArgMax => {
                let ordered = c.by_key(arg, &options.args[0], f == AggFunc::ArgMin)?;
                Ok(Some(format!("(ARRAY_AGG({ordered}){tail})[1]")))
            }
            AggFunc::Quantile => Ok(Some(c.percentile(arg, options, tail)?)),
            AggFunc::GroupConcat => Ok(Some(c.string_agg(arg, options, tail)?)),
            AggFunc::First | AggFunc::Last => {
                Ok(Some(ends(f, &format!("ARRAY_AGG({}){tail}", c.ordered(arg, options)?))))
            }
            AggFunc::Collect => Ok(Some(format!("ARRAY_AGG({}){tail}", c.ordered(arg, options)?))),
            // exact: Postgres has no approximate quantile
            AggFunc::ApproxQuantile => Ok(Some(c.percentile(arg, options, tail)?)),
            _ => c.standard_reduction(f, arg, options, tail),
        }
    }
}

/// A C `strftime` pattern as a Postgres `TO_CHAR` template. Literal text is
/// double-quoted so letters in it aren't read as template patterns.
/// The first or the last of `values`, an `ARRAY_AGG(...)` call: Postgres
/// has no FIRST or LAST.
fn ends(f: AggFunc, values: &str) -> String {
    match f {
        AggFunc::First => format!("({values})[1]"),
        _ => format!("({values})[ARRAY_LENGTH({values}, 1)]"),
    }
}

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

/// What a column comment written by `create_table` starts with; the Ibis
/// type of the column follows.
const TYPE_COMMENT: &str = "ibis-rs type: ";

/// The `information_schema` name of an array's element type, from the
/// element's `udt_name`.
fn udt_type(udt: &str) -> &str {
    match udt {
        "bool" => "boolean",
        "int2" => "smallint",
        "int4" => "integer",
        "int8" => "bigint",
        "float4" => "real",
        "float8" => "double precision",
        "timestamp" => "timestamp without time zone",
        "timestamptz" => "timestamp with time zone",
        other => other,
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
        Type::JSONB | Type::JSON => get::<JsonText>(row, i)?.map(|j| Value::String(j.0)),
        ref other => match other.kind() {
            Kind::Array(item) => array_cell(row, i, item)?,
            _ => {
                return Err(Error::Unsupported {
                    backend: "postgres".into(),
                    op: format!("result column of type {other}"),
                });
            }
        },
    };
    Ok(v.unwrap_or(Value::Null))
}

/// A one-dimensional array column, element type `item`. JSONB elements stay
/// JSON text here; `execute` reads them as the Ibis type says.
fn array_cell(row: &Row, i: usize, item: &Type) -> Result<Option<Value>> {
    fn items<'a, T: FromSql<'a>>(row: &'a Row, i: usize, f: impl Fn(T) -> Value) -> Result<Option<Value>> {
        let cells = get::<Vec<Option<T>>>(row, i)?;
        Ok(cells.map(|cells| Value::Array(cells.into_iter().map(|c| c.map_or(Value::Null, &f)).collect())))
    }
    match *item {
        Type::BOOL => items(row, i, Value::Bool),
        Type::INT2 => items(row, i, |x: i16| Value::Int(x.into())),
        Type::INT4 => items(row, i, |x: i32| Value::Int(x.into())),
        Type::INT8 => items(row, i, Value::Int),
        Type::FLOAT4 => items(row, i, |x: f32| Value::Float(x.into())),
        Type::FLOAT8 => items(row, i, Value::Float),
        Type::NUMERIC => items(row, i, |n: NumericText| Value::String(n.0)),
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN => items(row, i, Value::String),
        Type::BYTEA => items(row, i, Value::Binary),
        Type::DATE => items(row, i, |d: NaiveDate| Value::Date((d - epoch()).num_days() as i32)),
        Type::TIMESTAMP => items(row, i, |t: NaiveDateTime| Value::Timestamp(t.and_utc().timestamp_micros())),
        Type::TIMESTAMPTZ => items(row, i, |t: chrono::DateTime<chrono::Utc>| Value::Timestamp(t.timestamp_micros())),
        Type::JSONB | Type::JSON => items(row, i, |j: JsonText| Value::String(j.0)),
        ref other => {
            Err(Error::Unsupported { backend: "postgres".into(), op: format!("result column of type {other}[]") })
        }
    }
}

/// JSON or JSONB in binary form, as its text. JSONB leads with a version byte.
struct JsonText(String);

impl<'a> FromSql<'a> for JsonText {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> std::result::Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        let text = if *ty == Type::JSONB { raw.get(1..).ok_or("empty jsonb")? } else { raw };
        Ok(JsonText(std::str::from_utf8(text)?.to_string()))
    }

    fn accepts(ty: &Type) -> bool {
        matches!(*ty, Type::JSONB | Type::JSON)
    }
}

/// One element of an array literal: quoted, unless it is a number, a
/// boolean or NULL.
fn array_element(v: &Value) -> String {
    let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    match v {
        Value::Null => "NULL".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) if f.is_nan() => "NaN".into(),
        Value::Float(f) if f.is_infinite() => if *f > 0.0 { "Infinity" } else { "-Infinity" }.into(),
        Value::Float(f) => format!("{f:?}"),
        Value::String(s) => q(s),
        Value::Binary(b) => q(&format!("\\x{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())),
        Value::Date(d) => q(&format_date(*d)),
        Value::Timestamp(t) => q(&format_timestamp(*t)),
        Value::Array(items) => format!("{{{}}}", items.iter().map(array_element).collect::<Vec<_>>().join(",")),
        json @ (Value::Struct(_) | Value::Map(_)) => q(&value_to_json(json).to_string()),
    }
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
        Value::Array(_) => q(&array_element(v)),
        Value::Struct(_) | Value::Map(_) => q(&value_to_json(v).to_string()),
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

impl SqlClient for Postgres {
    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    /// A struct and a map are both JSONB: a comment says which, and of what.
    fn after_create(&self, name: &str, schema: &Schema) -> Vec<String> {
        let q = |n: &str| self.dialect.quote_ident(n);
        let comment = |f: &Field| {
            let text = quote_str(&format!("{TYPE_COMMENT}{}", f.dtype));
            format!("COMMENT ON COLUMN {}.{} IS {text}", q(name), q(&f.name))
        };
        schema.fields.iter().filter(|f| f.dtype.is_nested()).map(comment).collect()
    }

    fn statement(&self, sql: &str) -> Result<()> {
        self.execute_batch(sql)
    }

    fn load(&self, name: &str, schema: &Schema, data: &[RecordBatch]) -> Result<()> {
        let _ = schema;
        let copy = format!("COPY {} FROM STDIN (FORMAT csv)", self.dialect.quote_ident(name));
        let mut client = self.client.lock().unwrap();
        let mut w = client.copy_in(&copy).map_err(|e| exec_err(&copy, e))?;
        for b in data {
            for row in batch_to_rows(b)? {
                let line = row.iter().map(csv_field).collect::<Vec<_>>().join(",");
                writeln!(w, "{line}").map_err(|e| exec_err(&copy, e))?;
            }
        }
        w.finish().map(|_| ()).map_err(|e| pg_err(&copy, e))
    }
}

impl Backend for Postgres {
    ibis_sql::sql_client_methods!();
    fn name(&self) -> &str {
        "postgres"
    }

    fn compile(&self, table: &Table) -> Result<String> {
        compile(&self.dialect, table)
    }

    fn execute(&self, table: &Table) -> Result<Vec<RecordBatch>> {
        let sql = self.compile(table)?;
        let mut rows = self.raw_sql(&sql)?;
        // structs and maps arrive as JSON text
        let nested: Vec<(usize, &DataType)> = table
            .schema()
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.dtype.is_nested())
            .map(|(i, f)| (i, &f.dtype))
            .collect();
        for row in &mut rows {
            for (i, t) in &nested {
                row[*i] = decode_nested(std::mem::replace(&mut row[*i], Value::Null), t);
            }
        }
        Ok(vec![rows_to_batch(table.schema(), &rows)?])
    }

    fn table(&self, name: &str) -> Result<Table> {
        // the comment is where `create_table` wrote the Ibis type of a nested column;
        // a temporary table is in a schema of its own, and hides a lasting one of its name
        let sql = format!(
            "SELECT column_name::text, data_type::text, numeric_precision, numeric_scale, udt_name::text, \
             col_description(format('%I.%I', table_schema, table_name)::regclass, ordinal_position), \
             is_nullable::text \
             FROM information_schema.columns \
             WHERE table_name = {name} AND table_schema = COALESCE( \
               (SELECT n.nspname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                WHERE c.relname = {name} AND n.oid = pg_my_temp_schema()), current_schema()) \
             ORDER BY ordinal_position",
            name = quote_str(name)
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
                let (element, comment): (Option<String>, Option<String>) = (r.get(4), r.get(5));
                let declared =
                    comment.as_deref().and_then(|c| c.strip_prefix(TYPE_COMMENT)).and_then(|t| t.parse().ok());
                let dtype = match (declared, ty.as_str(), element.as_deref().and_then(|e| e.strip_prefix('_'))) {
                    (Some(t), _, _) => t,
                    (None, "ARRAY", Some(item)) => DataType::array(parse_type(udt_type(item), None, None)),
                    _ => parse_type(&ty, r.get(2), r.get(3)),
                };
                let mut field = Field::new(&col, dtype);
                field.nullable = r.get::<_, String>(6) != "NO";
                field
            })
            .collect();
        Ok(Table::named(name, Schema::new(fields)))
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
    fn arrays_are_copied_as_array_literals_and_structs_as_json() {
        let array = Value::Array(vec!["a\"b".into(), Value::Null, "c,d".into()]);
        assert_eq!(array_element(&array), r#"{"a\"b",NULL,"c,d"}"#);
        // the literal is then one CSV field
        assert_eq!(csv_field(&Value::Array(vec![1.into(), Value::Null])), "\"{1,NULL}\"");
        let s = Value::Struct(vec![("a".into(), 1.into()), ("b".into(), "x".into())]);
        assert_eq!(csv_field(&s), r#""{""a"":1,""b"":""x""}""#);
    }

    #[test]
    fn structs_and_maps_are_jsonb_and_arrays_of_arrays_are_refused() {
        let d = PostgresDialect;
        assert_eq!(d.type_name(&DataType::array(DataType::Int64)).unwrap(), "BIGINT[]");
        assert_eq!(d.type_name(&DataType::map(DataType::String, DataType::Int64)).unwrap(), "JSONB");
        assert_eq!(d.type_name(&DataType::array(DataType::structure(&[("a", DataType::Int64)]))).unwrap(), "JSONB[]");
        let nested = DataType::array(DataType::array(DataType::Int64));
        assert!(matches!(d.type_name(&nested), Err(Error::Unsupported { .. })));
    }

    #[test]
    fn csv_quoting() {
        assert_eq!(csv_field(&Value::Null), "");
        assert_eq!(csv_field(&Value::from("")), "\"\"");
        assert_eq!(csv_field(&Value::from("a\"b")), "\"a\"\"b\"");
    }
}
