//! What ClickHouse spells or means differently from the defaults in `ibis-sql`.
//!
//! ClickHouse function names are case-sensitive and mostly camelCase; only the
//! ones it shares with standard SQL (`SUM`, `ROUND`, `COALESCE`, ...) also
//! answer to the upper-case spelling the defaults use.

use ibis_core::convert::{format_date, format_timestamp};
use ibis_core::{AggFunc, Analytic, DataType, Error, Expr, Func, Result, Value};
use ibis_sql::{Dialect, ExprCompiler};

pub struct ClickHouseDialect;

/// A string literal. ClickHouse reads backslash escapes inside quotes.
pub(crate) fn quote_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn unsupported<T>(op: impl Into<String>) -> Result<T> {
    Err(Error::Unsupported { backend: "clickhouse".into(), op: op.into() })
}

/// A C `strftime` pattern as a `formatDateTime` one. They agree except that
/// ClickHouse reads `%M` as the month name and `%i` as minutes.
fn format_datetime_pattern(fmt: &str) -> Result<String> {
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        out.push_str(match chars.next() {
            Some('Y') => "%Y",
            Some('y') => "%y",
            Some('m') => "%m",
            Some('d') => "%d",
            Some('H') => "%H",
            Some('I') => "%I",
            Some('M') => "%i",
            Some('S') => "%S",
            Some('f') => "%f",
            Some('j') => "%j",
            Some('p') => "%p",
            Some('B') => "%M",
            Some('b') => "%b",
            Some('A') => "%W",
            Some('a') => "%a",
            Some('%') => "%%",
            other => {
                return unsupported(format!("strftime directive %{}", other.map(String::from).unwrap_or_default()));
            }
        });
    }
    Ok(out)
}

impl Dialect for ClickHouseDialect {
    fn name(&self) -> &str {
        "clickhouse"
    }

    /// Always `Nullable`: a bare ClickHouse type cannot hold NULL, and a cast
    /// to one turns NULL into an error.
    fn type_name(&self, t: &DataType) -> Result<String> {
        let name = match t {
            DataType::Null => "Nothing".to_string(),
            DataType::Boolean => "Bool".into(),
            DataType::Int8 => "Int8".into(),
            DataType::Int16 => "Int16".into(),
            DataType::Int32 => "Int32".into(),
            DataType::Int64 => "Int64".into(),
            DataType::UInt8 => "UInt8".into(),
            DataType::UInt16 => "UInt16".into(),
            DataType::UInt32 => "UInt32".into(),
            DataType::UInt64 => "UInt64".into(),
            DataType::Float32 => "Float32".into(),
            DataType::Float64 => "Float64".into(),
            DataType::Decimal { precision, scale } => format!("Decimal({precision}, {scale})"),
            DataType::String | DataType::Binary => "String".into(),
            // Date only reaches back to 1970
            DataType::Date => "Date32".into(),
            DataType::Timestamp { tz: None } => "DateTime64(6)".into(),
            DataType::Timestamp { tz: Some(tz) } => format!("DateTime64(6, {})", quote_str(tz)),
            DataType::Array(_) | DataType::Struct(_) | DataType::Map(..) => {
                return unsupported(format!("the type {t}"));
            }
        };
        Ok(format!("Nullable({name})"))
    }

    fn literal(&self, v: &Value, _t: &DataType) -> Result<Option<String>> {
        Ok(match v {
            Value::String(s) => Some(quote_str(s)),
            Value::Date(days) => Some(format!("toDate32('{}')", format_date(*days))),
            Value::Timestamp(us) => Some(format!("toDateTime64('{}', 6)", format_timestamp(*us))),
            _ => None,
        })
    }

    fn func(&self, c: &ExprCompiler, f: Func, args: &[Expr]) -> Result<Option<String>> {
        let a = |i: usize| c.sql(&args[i]);
        let cast = |s: String, t: DataType| -> Result<Option<String>> {
            Ok(Some(format!("CAST({s} AS {})", self.type_name(&t)?)))
        };
        let int = |s: String| cast(s, DataType::Int32);
        Ok(match f {
            Func::Length => return int(format!("lengthUTF8({})", a(0)?)),
            Func::Reverse => Some(format!("reverseUTF8({})", a(0)?)),
            Func::StartsWith => Some(format!("startsWith({}, {})", a(0)?, a(1)?)),
            Func::EndsWith => Some(format!("endsWith({}, {})", a(0)?, a(1)?)),
            Func::Contains => Some(format!("(position({}, {}) > 0)", a(0)?, a(1)?)),
            Func::Find => {
                let (x, p) = (a(0)?, a(1)?);
                let found = match args.get(2) {
                    None => format!("position({x}, {p})"),
                    Some(start) => format!("position({x}, {p}, {} + 1)", c.sql(start)?),
                };
                return cast(format!("{found} - 1"), DataType::Int64);
            }
            Func::RegexSearch => Some(format!("match({}, {})", a(0)?, a(1)?)),
            Func::RegexExtract => Some(format!("regexpExtract({}, {}, {})", a(0)?, a(1)?, a(2)?)),
            Func::RegexReplace => Some(format!("replaceRegexpAll({}, {}, {})", a(0)?, a(1)?, a(2)?)),
            Func::IsNan => Some(format!("isNaN({})", a(0)?)),
            Func::IsInf => Some(format!("isInfinite({})", a(0)?)),
            Func::Cot => Some(format!("(1 / tan({}))", a(0)?)),
            Func::DayOfWeekIndex => return cast(format!("toDayOfWeek({}) - 1", a(0)?), DataType::Int16),
            Func::DayOfWeekName => Some(format!("dateName('weekday', {})", a(0)?)),
            Func::DayOfYear => return int(format!("toDayOfYear({})", a(0)?)),
            Func::WeekOfYear => return int(format!("toISOWeek({})", a(0)?)),
            Func::Millisecond => return int(format!("toUnixTimestamp64Milli({}) % 1000", a(0)?)),
            Func::Microsecond => return int(format!("toUnixTimestamp64Micro({}) % 1000000", a(0)?)),
            // ROUND of a float rounds half to even; Ibis rounds half away from zero
            Func::Round if c.dtype(&args[0])?.is_floating() => {
                let x = a(0)?;
                match args.get(1) {
                    None => return cast(format!("sign({x}) * floor(abs({x}) + 0.5)"), DataType::Int64),
                    Some(digits) => {
                        let scale = format!("pow(10, {})", c.sql(digits)?);
                        Some(format!("(sign({x}) * floor(abs({x}) * {scale} + 0.5) / {scale})"))
                    }
                }
            }
            // DATE_TRUNC stops at seconds
            Func::Truncate if matches!(ExprCompiler::lit_str(&args[1]), Some("ms")) => {
                Some(format!("toStartOfMillisecond({})", a(0)?))
            }
            Func::Truncate if matches!(ExprCompiler::lit_str(&args[1]), Some("us")) => {
                Some(format!("toStartOfMicrosecond({})", a(0)?))
            }
            Func::Strftime => {
                let Some(fmt) = ExprCompiler::lit_str(&args[1]) else {
                    return unsupported("strftime with a non-literal format");
                };
                Some(format!("formatDateTime({}, {})", a(0)?, quote_str(&format_datetime_pattern(fmt)?)))
            }
            Func::DateFromYmd => Some(format!("makeDate32({}, {}, {})", a(0)?, a(1)?, a(2)?)),
            Func::TimestampFromYmdhms => {
                let parts = (0..args.len()).map(a).collect::<Result<Vec<_>>>()?;
                Some(format!("makeDateTime64({}, 0, 6)", parts.join(", ")))
            }
            _ => None,
        })
    }

    fn analytic(&self, f: Analytic, _args: &[String], over: &str) -> Result<Option<String>> {
        Ok(match f {
            // unlike the other window functions, only the lower-case name exists
            Analytic::CumeDist => Some(format!("cume_dist() {over}")),
            _ => None,
        })
    }

    fn agg(&self, c: &ExprCompiler, f: AggFunc, arg: Option<&Expr>, tail: &str) -> Result<Option<String>> {
        let Some(x) = arg else { return Ok(None) };
        Ok(match f {
            // no BOOL_OR / BOOL_AND; a boolean is 0 or 1
            AggFunc::Any => Some(format!("max({}){tail}", c.sql(x)?)),
            AggFunc::All => Some(format!("min({}){tail}", c.sql(x)?)),
            _ => None,
        })
    }
}
