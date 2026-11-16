//! What Trino spells or means differently from the defaults in `ibis-sql`.

use ibis_core::{BinaryOp, DataType, Error, Expr, Func, Result, Value};
use ibis_sql::{Dialect, ExprCompiler, quote_str, strftime_to_date_format};

pub struct TrinoDialect;

fn unsupported<T>(op: impl Into<String>) -> Result<T> {
    Err(Error::Unsupported { backend: "trino".into(), op: op.into() })
}

/// A finite float as a DOUBLE literal: `1.5` alone would be an exact DECIMAL.
pub(crate) fn double_literal(f: f64) -> String {
    if f.is_nan() {
        "nan()".into()
    } else if f.is_infinite() {
        if f > 0.0 { "infinity()" } else { "-infinity()" }.into()
    } else {
        format!("{f:E}")
    }
}

impl Dialect for TrinoDialect {
    fn name(&self) -> &str {
        "trino"
    }

    /// No unsigned integers: each takes the next wider signed type.
    fn type_name(&self, t: &DataType) -> Result<String> {
        Ok(match t {
            DataType::Null => "UNKNOWN".into(),
            DataType::Int8 => "TINYINT".into(),
            DataType::UInt8 => "SMALLINT".into(),
            DataType::UInt16 => "INTEGER".into(),
            DataType::UInt32 => "BIGINT".into(),
            DataType::UInt64 => "DECIMAL(20, 0)".into(),
            DataType::Binary => "VARBINARY".into(),
            DataType::Timestamp { tz: None } => "TIMESTAMP(6)".into(),
            DataType::Timestamp { tz: Some(_) } => "TIMESTAMP(6) WITH TIME ZONE".into(),
            t => ibis_sql::default_type_name("trino", t)?,
        })
    }

    fn literal(&self, v: &Value, _t: &DataType) -> Result<Option<String>> {
        Ok(match v {
            Value::Float(f) => Some(double_literal(*f)),
            _ => None,
        })
    }

    fn binary(&self, c: &ExprCompiler, op: BinaryOp, l: &Expr, r: &Expr) -> Result<Option<String>> {
        // REPEAT builds an array
        if op == BinaryOp::Mul {
            let (lt, rt) = (c.dtype(l)?, c.dtype(r)?);
            if lt == DataType::String {
                return Ok(Some(format!("ARRAY_JOIN(REPEAT({}, {}), '')", c.sql(l)?, c.sql(r)?)));
            }
            if rt == DataType::String {
                return Ok(Some(format!("ARRAY_JOIN(REPEAT({}, {}), '')", c.sql(r)?, c.sql(l)?)));
            }
        }
        Ok(None)
    }

    fn func(&self, c: &ExprCompiler, f: Func, args: &[Expr]) -> Result<Option<String>> {
        let a = |i: usize| c.sql(&args[i]);
        let int = |s: String| format!("CAST({s} AS INTEGER)");
        Ok(match f {
            Func::Repeat => Some(format!("ARRAY_JOIN(REPEAT({}, {}), '')", a(0)?, a(1)?)),
            // no RIGHT; a negative SUBSTR start past the beginning gives ''
            Func::Right => {
                let x = a(0)?;
                Some(format!("SUBSTR({x}, GREATEST(LENGTH({x}) - {} + 1, 1))", a(1)?))
            }
            Func::ILike => Some(format!("(LOWER({}) LIKE LOWER({}))", a(0)?, a(1)?)),
            Func::Ascii => Some(int(format!("CODEPOINT(CAST(SUBSTR({}, 1, 1) AS VARCHAR(1)))", a(0)?))),
            Func::RegexSearch => Some(format!("REGEXP_LIKE({}, {})", a(0)?, a(1)?)),
            // no match gives '' like DuckDB
            Func::RegexExtract => {
                let x = a(0)?;
                Some(format!(
                    "CASE WHEN {x} IS NULL THEN NULL ELSE COALESCE(REGEXP_EXTRACT({x}, {}, {}), '') END",
                    a(1)?,
                    a(2)?
                ))
            }
            Func::RegexReplace => Some(format!("REGEXP_REPLACE({}, {}, {})", a(0)?, a(1)?, a(2)?)),
            Func::IsNan => Some(format!("IS_NAN({})", a(0)?)),
            Func::IsInf => Some(format!("IS_INFINITE({})", a(0)?)),
            Func::Cot => Some(format!("(1E0 / TAN({}))", a(0)?)),
            // DAY_OF_WEEK: 1 is Monday
            Func::DayOfWeekIndex => Some(format!("CAST(DAY_OF_WEEK({}) - 1 AS SMALLINT)", a(0)?)),
            Func::DayOfWeekName => Some(format!("DATE_FORMAT({}, '%W')", a(0)?)),
            Func::Millisecond => Some(int(format!("MILLISECOND({})", a(0)?))),
            // no microsecond field anywhere: read it off the text
            Func::Microsecond => Some(int(format!("SUBSTR(CAST(CAST({} AS TIMESTAMP(6)) AS VARCHAR), 21, 6)", a(0)?))),
            Func::Truncate => {
                let unit = match ExprCompiler::lit_str(&args[1]) {
                    Some("Y") => "year",
                    Some("Q") => "quarter",
                    Some("M") => "month",
                    Some("W") => "week",
                    Some("D") => "day",
                    Some("h") => "hour",
                    Some("m") => "minute",
                    Some("s") => "second",
                    Some("ms") => "millisecond",
                    // already the finest unit a TIMESTAMP(6) has
                    Some("us") => return Ok(Some(a(0)?)),
                    _ => return unsupported("truncate with a non-literal or unknown unit"),
                };
                Some(format!("DATE_TRUNC('{unit}', {})", a(0)?))
            }
            Func::Strftime => {
                let Some(fmt) = ExprCompiler::lit_str(&args[1]) else {
                    return unsupported("strftime with a non-literal format");
                };
                Some(format!("DATE_FORMAT({}, {})", a(0)?, quote_str(&strftime_to_date_format("trino", fmt)?)))
            }
            Func::DateFromYmd => {
                Some(format!("CAST(FORMAT('%04d-%02d-%02d', {}, {}, {}) AS DATE)", a(0)?, a(1)?, a(2)?))
            }
            Func::TimestampFromYmdhms => Some(format!(
                "CAST(FORMAT('%04d-%02d-%02d %02d:%02d:%02d', {}, {}, {}, {}, {}, {}) AS TIMESTAMP(6))",
                a(0)?,
                a(1)?,
                a(2)?,
                a(3)?,
                a(4)?,
                a(5)?
            )),
            _ => None,
        })
    }

    /// "FILTER is not yet supported for window functions"
    fn supports_agg_filter(&self, windowed: bool) -> bool {
        !windowed
    }

    /// OFFSET comes before LIMIT.
    fn limit_offset(&self, limit: Option<u64>, offset: u64) -> String {
        let mut s = String::new();
        if offset > 0 {
            s.push_str(&format!(" OFFSET {offset}"));
        }
        if let Some(n) = limit {
            s.push_str(&format!(" LIMIT {n}"));
        }
        s
    }
}
