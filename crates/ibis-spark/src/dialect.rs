//! What Spark SQL spells or means differently from the defaults in `ibis-sql`.

use ibis_core::convert::format_timestamp;
use ibis_core::{AggFunc, AggOptions, BinaryOp, DataType, Error, Expr, Func, Lambda, Result, Value};
use ibis_sql::{Dialect, ExprCompiler, Unnest, strftime_to_java_format};

use crate::nested;

pub struct SparkDialect;

/// A string literal. Backslash escapes are read inside quotes, and two
/// adjacent literals are concatenated, so `'it''s'` would be `its`.
pub(crate) fn quote_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// A regular expression for `java.util.regex`. It has no POSIX bracket
/// classes: `[[:digit:]]` is read as a set of the characters `:digt`. Each
/// `[:name:]` becomes the `\p{Name}` that means the same.
pub(crate) fn java_regex(pattern: &str) -> String {
    const CLASSES: [(&str, &str); 13] = [
        ("[:alnum:]", r"\p{Alnum}"),
        ("[:alpha:]", r"\p{Alpha}"),
        ("[:ascii:]", r"\p{ASCII}"),
        ("[:blank:]", r"\p{Blank}"),
        ("[:cntrl:]", r"\p{Cntrl}"),
        ("[:digit:]", r"\p{Digit}"),
        ("[:graph:]", r"\p{Graph}"),
        ("[:lower:]", r"\p{Lower}"),
        ("[:print:]", r"\p{Print}"),
        ("[:punct:]", r"\p{Punct}"),
        ("[:space:]", r"\p{Space}"),
        ("[:upper:]", r"\p{Upper}"),
        ("[:xdigit:]", r"\p{XDigit}"),
    ];
    CLASSES.iter().fold(pattern.to_string(), |p, (posix, java)| p.replace(posix, java))
}

pub(crate) fn unsupported<T>(op: impl Into<String>) -> Result<T> {
    Err(Error::Unsupported { backend: "spark".into(), op: op.into() })
}

impl Dialect for SparkDialect {
    fn name(&self) -> &str {
        "spark"
    }
    /// Every column takes NULL: there is no `NOT NULL` to declare.
    fn column_type(&self, t: &DataType, _nullable: bool) -> Result<String> {
        self.type_name(t)
    }
    fn temporary(&self) -> Option<&'static str> {
        None
    }

    fn quote_ident(&self, s: &str) -> String {
        format!("`{}`", s.replace('`', "``"))
    }

    /// No unsigned integers: each takes the next wider signed type.
    /// `TIMESTAMP` is an instant shown in the session time zone;
    /// `TIMESTAMP_NTZ` is the zone-less one.
    fn type_name(&self, t: &DataType) -> Result<String> {
        if let Some(name) = nested::type_name(self, t)? {
            return Ok(name);
        }
        Ok(match t {
            DataType::Null => "VOID".into(),
            DataType::Int32 => "INT".into(),
            DataType::UInt8 => "SMALLINT".into(),
            DataType::UInt16 => "INT".into(),
            DataType::UInt32 => "BIGINT".into(),
            DataType::UInt64 => "DECIMAL(20, 0)".into(),
            DataType::Float32 => "FLOAT".into(),
            DataType::String => "STRING".into(),
            DataType::Binary => "BINARY".into(),
            DataType::Timestamp { tz: None } => "TIMESTAMP_NTZ".into(),
            DataType::Timestamp { tz: Some(_) } => "TIMESTAMP".into(),
            t => ibis_sql::default_type_name("spark", t)?,
        })
    }

    fn literal(&self, v: &Value, t: &DataType) -> Result<Option<String>> {
        Ok(match v {
            Value::String(s) => Some(quote_str(s)),
            // with an exponent it is a DOUBLE; `1.5` would be an exact DECIMAL
            Value::Float(f) if f.is_finite() => Some(format!("{f:E}")),
            Value::Binary(b) => Some(format!("X'{}'", b.iter().map(|byte| format!("{byte:02X}")).collect::<String>())),
            Value::Timestamp(us) if matches!(t, DataType::Timestamp { tz: None }) => {
                Some(format!("TIMESTAMP_NTZ '{}'", format_timestamp(*us)))
            }
            _ => None,
        })
    }

    fn binary(&self, c: &ExprCompiler, op: BinaryOp, l: &Expr, r: &Expr) -> Result<Option<String>> {
        // In ANSI mode (the default since Spark 4) division by zero raises, and
        // out of it the answer is NULL. Ibis's `/` follows IEEE 754: x / 0 is
        // +/-Infinity, 0 / 0 is NaN.
        if op == BinaryOp::Div
            && !matches!(r, Expr::Literal(v, _) if !matches!(v, Value::Int(0)) && *v != Value::Float(0.0))
        {
            let (ls, rs) = (c.sql(l)?, c.sql(r)?);
            return Ok(Some(format!(
                "CASE WHEN {rs} = 0 THEN CAST(CASE WHEN {ls} IS NULL THEN NULL WHEN {ls} > 0 THEN 'Infinity' \
                 WHEN {ls} < 0 THEN '-Infinity' ELSE 'NaN' END AS DOUBLE) ELSE CAST({ls} AS DOUBLE) / {rs} END"
            )));
        }
        Ok(None)
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
        nested::array_lambda(c, kind, arg, param, index, body).map(Some)
    }

    /// A generator stands alone in a SELECT list: not inside an expression.
    fn unnest_in_select(&self) -> bool {
        false
    }

    fn unnest(&self, array: &str, _item: &DataType, alias: &str, with_offset: bool, keep_empty: bool) -> Unnest {
        nested::unnest(array, alias, with_offset, keep_empty)
    }

    /// `COLLECT_LIST` leaves NULLs out by itself.
    fn agg(&self, c: &ExprCompiler, f: AggFunc, arg: Option<&Expr>, tail: &str) -> Result<Option<String>> {
        Ok(match (f, arg) {
            (AggFunc::Collect, Some(x)) => Some(format!("COLLECT_LIST({}){tail}", c.sql(x)?)),
            (AggFunc::ApproxCountDistinct, Some(x)) => c.call_agg("APPROX_COUNT_DISTINCT", x, tail)?,
            // the second argument: pass over NULLs
            (AggFunc::Arbitrary | AggFunc::First, Some(x)) => Some(format!("FIRST({}, TRUE){tail}", c.sql(x)?)),
            (AggFunc::Last, Some(x)) => Some(format!("LAST({}, TRUE){tail}", c.sql(x)?)),
            (AggFunc::Median, Some(x)) => Some(format!("PERCENTILE({}, 0.5){tail}", c.sql(x)?)),
            (AggFunc::ApproxMedian, Some(x)) => Some(format!("PERCENTILE_APPROX({}, 0.5){tail}", c.sql(x)?)),
            _ => None,
        })
    }

    fn date_diff(&self, unit: ibis_core::IntervalUnit, start: &str, end: &str) -> Result<Option<String>> {
        if unit == ibis_core::IntervalUnit::Nanosecond {
            return unsupported("a delta in nanoseconds");
        }
        Ok(Some(format!("TIMESTAMPDIFF({}, {start}, {end})", ibis_sql::interval_unit_name(unit))))
    }
    /// `TIMESTAMP_MICROS` gives a moment in the session's zone.
    fn timestamp_from_micros(&self, micros: &str) -> Result<Option<String>> {
        Ok(Some(format!("TIMESTAMPADD(MICROSECOND, {micros}, TIMESTAMP_NTZ'1970-01-01 00:00:00')")))
    }
    /// A struct is a value even where a field of it is NULL.
    fn count_distinct_rows(&self, columns: &[String], condition: Option<&str>, tail: &str) -> Result<Option<String>> {
        Ok(match condition {
            None => Some(format!("COUNT(DISTINCT STRUCT({})){tail}", columns.join(", "))),
            Some(_) => None,
        })
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
            // the second argument: whether to pass over NULLs
            AggFunc::First | AggFunc::Last => {
                c.unordered(options, "first or last")?;
                let name = if f == AggFunc::First { "FIRST" } else { "LAST" };
                let skip = if options.include_null { "FALSE" } else { "TRUE" };
                Ok(Some(format!("{name}({}, {skip}){tail}", c.sql(arg)?)))
            }
            AggFunc::Collect => {
                c.unordered(options, "collect")?;
                if options.include_null {
                    // COLLECT_LIST and COLLECT_SET pass over NULLs
                    return Err(Error::Unsupported { backend: "spark".into(), op: "collect of NULLs".into() });
                }
                let name = if options.distinct { "COLLECT_SET" } else { "COLLECT_LIST" };
                Ok(Some(format!("{name}({}){tail}", c.sql(arg)?)))
            }
            AggFunc::ArgMin => Ok(Some(format!("MIN_BY({}, {}){tail}", c.sql(arg)?, other()?))),
            AggFunc::ArgMax => Ok(Some(format!("MAX_BY({}, {}){tail}", c.sql(arg)?, other()?))),
            // one number for the sample and for the population
            AggFunc::CorrSamp => Ok(Some(format!("CORR({}, {}){tail}", c.sql(arg)?, other()?))),
            AggFunc::Quantile => Ok(Some(format!("PERCENTILE({}, {}){tail}", c.sql(arg)?, other()?))),
            AggFunc::GroupConcat => {
                c.unordered(options, "group_concat")?;
                // no strings: NULL, where joining an empty array gives ''
                let list = format!("COLLECT_LIST({}){tail}", c.sql(arg)?);
                Ok(Some(format!("ARRAY_JOIN(CASE WHEN SIZE({list}) = 0 THEN NULL ELSE {list} END, {})", other()?)))
            }
            AggFunc::ApproxQuantile => {
                let x = arg.clone().cast(DataType::Float64);
                Ok(Some(format!("PERCENTILE_APPROX({}, {}){tail}", c.sql(&x)?, other()?)))
            }
            _ => c.standard_reduction(f, arg, options, tail),
        }
    }

    fn func(&self, c: &ExprCompiler, f: Func, args: &[Expr]) -> Result<Option<String>> {
        if let Some(sql) = nested::func(self, c, f, args)? {
            return Ok(Some(sql));
        }
        let a = |i: usize| c.sql(&args[i]);
        // a literal pattern is rewritten for Java; a computed one is the caller's business
        let pattern = |i: usize| match ExprCompiler::lit_str(&args[i]) {
            Some(p) => Ok(quote_str(&java_regex(p))),
            None => c.sql(&args[i]),
        };
        let int = |s: String| format!("CAST({s} AS INT)");
        Ok(match f {
            // in UTC, whatever zone the session is in
            Func::Now => Some("CONVERT_TIMEZONE(CURRENT_TIMEZONE(), 'UTC', LOCALTIMESTAMP())".into()),
            Func::Today => Some("CAST(CONVERT_TIMEZONE(CURRENT_TIMEZONE(), 'UTC', LOCALTIMESTAMP()) AS DATE)".into()),
            Func::StringToTimestamp | Func::StringToDate => {
                let Some(fmt) = ExprCompiler::lit_str(&args[1]) else {
                    return unsupported("a string parsed by a non-literal format");
                };
                let (x, fmt) = (a(0)?, quote_str(&strftime_to_java_format("spark", fmt)?));
                Some(match f {
                    Func::StringToDate => format!("TO_DATE({x}, {fmt})"),
                    _ => format!("TO_TIMESTAMP_NTZ({x}, {fmt})"),
                })
            }
            Func::IsoYear => Some(int(format!("EXTRACT(YEAROFWEEK FROM {})", a(0)?))),
            // UNIX_TIMESTAMP reads its argument in the session's zone
            Func::EpochSeconds => {
                Some(int(format!("TIMESTAMPDIFF(SECOND, TIMESTAMP_NTZ'1970-01-01 00:00:00', {})", a(0)?)))
            }
            Func::StartsWith => Some(format!("STARTSWITH({}, {})", a(0)?, a(1)?)),
            Func::EndsWith => Some(format!("ENDSWITH({}, {})", a(0)?, a(1)?)),
            Func::Contains => Some(format!("CONTAINS({}, {})", a(0)?, a(1)?)),
            Func::Find => Some(match args.get(2) {
                None => format!("CAST(LOCATE({}, {}) - 1 AS BIGINT)", a(1)?, a(0)?),
                Some(start) => format!("CAST(LOCATE({}, {}, {} + 1) - 1 AS BIGINT)", a(1)?, a(0)?, c.sql(start)?),
            }),
            Func::RegexSearch => Some(format!("REGEXP_LIKE({}, {})", a(0)?, pattern(1)?)),
            Func::RegexExtract => Some(format!("REGEXP_EXTRACT({}, {}, {})", a(0)?, pattern(1)?, a(2)?)),
            Func::RegexReplace => Some(format!("REGEXP_REPLACE({}, {}, {})", a(0)?, pattern(1)?, a(2)?)),
            // no ISINF
            Func::IsInf => Some(format!("(ABS({}) = CAST('Infinity' AS DOUBLE))", a(0)?)),
            // WEEKDAY: 0 is Monday
            Func::DayOfWeekIndex => Some(format!("CAST(WEEKDAY({}) AS SMALLINT)", a(0)?)),
            Func::DayOfWeekName => Some(format!("DATE_FORMAT({}, 'EEEE')", a(0)?)),
            // EXTRACT has no field finer than SECOND, which carries the fraction
            Func::Millisecond => Some(int(format!("FLOOR(EXTRACT(SECOND FROM {}) * 1000) % 1000", a(0)?))),
            Func::Microsecond => Some(int(format!("FLOOR(EXTRACT(SECOND FROM {}) * 1000000) % 1000000", a(0)?))),
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
                    Some("us") => "microsecond",
                    _ => return unsupported("truncate with a non-literal or unknown unit"),
                };
                // DATE_TRUNC answers with a TIMESTAMP whatever it was given
                let t = self.cast_type_name(&c.dtype(&args[0])?)?;
                Some(format!("CAST(DATE_TRUNC('{unit}', {}) AS {t})", a(0)?))
            }
            Func::Strftime => {
                let Some(fmt) = ExprCompiler::lit_str(&args[1]) else {
                    return unsupported("strftime with a non-literal format");
                };
                Some(format!("DATE_FORMAT({}, {})", a(0)?, quote_str(&strftime_to_java_format("spark", fmt)?)))
            }
            Func::TimestampFromYmdhms => {
                Some(format!("MAKE_TIMESTAMP_NTZ({}, {}, {}, {}, {}, {})", a(0)?, a(1)?, a(2)?, a(3)?, a(4)?, a(5)?))
            }
            _ => None,
        })
    }

    /// "FILTER is not supported for window functions"
    fn supports_agg_filter(&self, windowed: bool) -> bool {
        !windowed
    }
}
