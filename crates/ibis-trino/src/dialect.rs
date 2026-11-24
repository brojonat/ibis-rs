//! What Trino spells or means differently from the defaults in `ibis-sql`.

use ibis_core::{AggFunc, AggOptions, BinaryOp, DataType, Error, Expr, Func, Lambda, Result, Value};
use ibis_sql::{Dialect, ExprCompiler, Unnest, quote_str, strftime_to_date_format};

use crate::nested;

pub struct TrinoDialect;

pub(crate) fn unsupported<T>(op: impl Into<String>) -> Result<T> {
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
    fn temporary(&self) -> Option<&'static str> {
        None
    }

    /// No unsigned integers: each takes the next wider signed type.
    fn type_name(&self, t: &DataType) -> Result<String> {
        if let Some(name) = nested::type_name(self, t)? {
            return Ok(name);
        }
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

    fn nested_literal(&self, c: &ExprCompiler, v: &Value, t: &DataType) -> Result<Option<String>> {
        nested::literal(self, c, v, t)
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
        nested::array_lambda(self, c, kind, arg, param, index, body).map(Some)
    }

    fn unnest_in_select(&self) -> bool {
        false
    }

    /// A subquery is a row of its columns: a struct is compared with the
    /// row holding the one struct column, not with the column.
    fn in_subquery_needle(&self, needle: String, column: &DataType) -> String {
        if matches!(column, DataType::Struct(_)) { format!("ROW({needle})") } else { needle }
    }

    fn unnest(&self, array: &str, item: &DataType, alias: &str, with_offset: bool, keep_empty: bool) -> Unnest {
        nested::unnest(array, item, alias, with_offset, keep_empty)
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

    fn agg(&self, c: &ExprCompiler, f: AggFunc, arg: Option<&Expr>, tail: &str) -> Result<Option<String>> {
        let Some(x) = arg else { return Ok(None) };
        use AggFunc::*;
        match f {
            KurtosisSamp => c.call_agg("KURTOSIS", x, tail),
            ApproxCountDistinct => c.call_agg("APPROX_DISTINCT", x, tail),
            BitAnd => c.call_agg("BITWISE_AND_AGG", x, tail),
            BitOr => c.call_agg("BITWISE_OR_AGG", x, tail),
            BitXor => c.call_agg("BITWISE_XOR_AGG", x, tail),
            Arbitrary => c.call_agg("ARBITRARY", x, tail),
            ApproxMedian => Ok(Some(format!("APPROX_PERCENTILE({}, 0.5){tail}", c.sql(x)?))),
            // no FIRST or LAST: the ends of the values gathered (NULL past them)
            First => Ok(Some(format!("ELEMENT_AT(ARRAY_AGG({}){tail}, 1)", c.sql(x)?))),
            Last => Ok(Some(format!("ELEMENT_AT(ARRAY_AGG({}){tail}, -1)", c.sql(x)?))),
            _ => Ok(None),
        }
    }

    fn count_distinct_rows(&self, columns: &[String], condition: Option<&str>, tail: &str) -> Result<Option<String>> {
        Ok(match condition {
            None => Some(format!("COUNT(DISTINCT ROW({})){tail}", columns.join(", "))),
            Some(_) => None,
        })
    }
    fn date_diff(&self, unit: ibis_core::IntervalUnit, start: &str, end: &str) -> Result<Option<String>> {
        use ibis_core::IntervalUnit::*;
        if let Microsecond | Nanosecond = unit {
            return unsupported("a delta finer than milliseconds");
        }
        let unit = ibis_sql::interval_unit_name(unit).to_lowercase();
        Ok(Some(format!("DATE_DIFF('{unit}', {start}, {end})")))
    }
    /// `FROM_UNIXTIME_NANOS` gives a moment with a zone: its UTC wall clock.
    fn timestamp_from_micros(&self, micros: &str) -> Result<Option<String>> {
        Ok(Some(format!("CAST(FROM_UNIXTIME_NANOS(({micros}) * 1000) AT TIME ZONE 'UTC' AS TIMESTAMP(6))")))
    }
    /// `DATE_ADD('day', count, moment)`: it takes any count, and a date
    /// stays a date. It has no unit finer than the millisecond.
    fn date_shift(
        &self,
        c: &ExprCompiler,
        moment: &str,
        _t: &DataType,
        step: &ibis_core::Shift,
    ) -> Result<Option<String>> {
        use ibis_core::IntervalUnit::*;
        if let Microsecond | Nanosecond = step.unit {
            return unsupported("an interval finer than a millisecond");
        }
        let unit = ibis_sql::interval_unit_name(step.unit).to_lowercase();
        let count = match step.back {
            true => format!("-({})", c.sql(&step.count)?),
            false => c.sql(&step.count)?,
        };
        Ok(Some(format!("DATE_ADD('{unit}', {count}, {moment})")))
    }
    fn reduction(
        &self,
        c: &ExprCompiler,
        f: AggFunc,
        arg: &Expr,
        options: &AggOptions,
        tail: &str,
    ) -> Result<Option<String>> {
        let other = || c.sql(&options.args[0]);
        match f {
            AggFunc::ArgMin => Ok(Some(format!("MIN_BY({}, {}){tail}", c.sql(arg)?, other()?))),
            AggFunc::ArgMax => Ok(Some(format!("MAX_BY({}, {}){tail}", c.sql(arg)?, other()?))),
            AggFunc::GroupConcat => {
                let ordered = format!("{}{}", c.sql(arg)?, c.agg_order(options)?);
                Ok(Some(format!("ARRAY_JOIN(ARRAY_AGG({ordered}){tail}, {})", other()?)))
            }
            // the ends of the values gathered, NULLs among them if they are kept
            AggFunc::First => Ok(Some(format!("ELEMENT_AT(ARRAY_AGG({}){tail}, 1)", c.ordered(arg, options)?))),
            AggFunc::Last => Ok(Some(format!("ELEMENT_AT(ARRAY_AGG({}){tail}, -1)", c.ordered(arg, options)?))),
            AggFunc::Collect => Ok(Some(format!("ARRAY_AGG({}){tail}", c.ordered(arg, options)?))),
            // approximate only (Ibis: "backend implements approximate quantiles")
            AggFunc::ApproxQuantile => {
                let x = arg.clone().cast(DataType::Float64);
                Ok(Some(format!("APPROX_PERCENTILE({}, {}){tail}", c.sql(&x)?, other()?)))
            }
            _ => c.standard_reduction(f, arg, options, tail),
        }
    }

    fn func(&self, c: &ExprCompiler, f: Func, args: &[Expr]) -> Result<Option<String>> {
        if let Some(sql) = nested::func(self, c, f, args)? {
            return Ok(Some(sql));
        }
        let a = |i: usize| c.sql(&args[i]);
        let int = |s: String| format!("CAST({s} AS INTEGER)");
        Ok(match f {
            Func::IsoYear => Some(int(format!("YEAR_OF_WEEK({})", a(0)?))),
            // TO_UNIXTIME reads a timestamp in the session's zone
            Func::EpochSeconds => Some(int(format!("DATE_DIFF('second', TIMESTAMP '1970-01-01 00:00:00', {})", a(0)?))),
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
