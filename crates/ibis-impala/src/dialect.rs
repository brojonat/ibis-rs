//! What Impala spells or means differently from the defaults in `ibis-sql`.

use ibis_core::convert::format_timestamp;
use ibis_core::{AggFunc, AggOptions, Analytic, DataType, Error, Expr, Func, Result, Value};
use ibis_sql::{Dialect, ExprCompiler};

pub struct ImpalaDialect;

/// A string literal. Backslash escapes are read inside quotes, and two
/// adjacent literals are concatenated, so `'it''s'` would be `its`.
pub(crate) fn quote_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn unsupported<T>(op: impl Into<String>) -> Result<T> {
    Err(Error::Unsupported { backend: "impala".into(), op: op.into() })
}

/// A C `strftime` pattern in the pattern language of Impala's
/// `from_timestamp`: runs of `y`, `M`, `d`, `H`, `m`, `s` and `S`, with
/// everything else taken as it is.
fn strftime_to_impala_format(fmt: &str) -> Result<String> {
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            if ch.is_ascii_alphabetic() {
                return unsupported(format!("strftime format with the literal letter {ch:?}"));
            }
            out.push(ch);
            continue;
        }
        out.push_str(match chars.next() {
            Some('Y') => "yyyy",
            Some('y') => "yy",
            Some('m') => "MM",
            Some('d') => "dd",
            Some('H') => "HH",
            Some('M') => "mm",
            Some('S') => "ss",
            Some('f') => "SSSSSS",
            Some('b') => "MMM",
            Some('B') => "MMMM",
            other => {
                return unsupported(format!("strftime directive %{}", other.map(String::from).unwrap_or_default()));
            }
        });
    }
    Ok(out)
}

impl Dialect for ImpalaDialect {
    fn name(&self) -> &str {
        "impala"
    }
    /// Every column takes NULL: there is no `NOT NULL` to declare.
    fn column_type(&self, t: &DataType, _nullable: bool) -> Result<String> {
        self.type_name(t)
    }
    fn temporary(&self) -> Option<&'static str> {
        None
    }
    fn create_table_suffix(&self) -> &'static str {
        " STORED AS PARQUET"
    }

    fn quote_ident(&self, s: &str) -> String {
        format!("`{}`", s.replace('`', "``"))
    }

    /// No unsigned integers: each takes the next wider signed type. The one
    /// timestamp type has no zone.
    fn type_name(&self, t: &DataType) -> Result<String> {
        Ok(match t {
            DataType::Null => return unsupported("the null type"),
            DataType::Int32 => "INT".into(),
            DataType::UInt8 => "SMALLINT".into(),
            DataType::UInt16 => "INT".into(),
            DataType::UInt32 => "BIGINT".into(),
            DataType::UInt64 => "DECIMAL(20, 0)".into(),
            DataType::Float32 => "FLOAT".into(),
            DataType::String => "STRING".into(),
            DataType::Binary => "BINARY".into(),
            DataType::Timestamp { .. } => "TIMESTAMP".into(),
            t => ibis_sql::default_type_name("impala", t)?,
        })
    }

    fn literal(&self, v: &Value, _t: &DataType) -> Result<Option<String>> {
        Ok(match v {
            Value::String(s) => Some(quote_str(s)),
            // any way of writing the number is an exact DECIMAL, exponent or not
            Value::Float(f) if f.is_finite() => Some(format!("CAST({f:?} AS DOUBLE)")),
            // no TIMESTAMP '...' literal
            Value::Timestamp(us) => Some(format!("CAST('{}' AS TIMESTAMP)", format_timestamp(*us))),
            Value::Binary(b) => Some(format!(
                "CAST(UNHEX('{}') AS BINARY)",
                b.iter().map(|byte| format!("{byte:02X}")).collect::<String>()
            )),
            _ => None,
        })
    }

    fn func(&self, c: &ExprCompiler, f: Func, args: &[Expr]) -> Result<Option<String>> {
        let a = |i: usize| c.sql(&args[i]);
        let int = |s: String| format!("CAST({s} AS INT)");
        Ok(match f {
            // in UTC, whatever zone the session is in
            Func::Now => Some("UTC_TIMESTAMP()".into()),
            Func::Today => Some("CAST(UTC_TIMESTAMP() AS DATE)".into()),
            Func::StringToTimestamp | Func::StringToDate => {
                let Some(fmt) = ExprCompiler::lit_str(&args[1]) else {
                    return unsupported("a string parsed by a non-literal format");
                };
                let (x, fmt) = (a(0)?, quote_str(&strftime_to_impala_format(fmt)?));
                Some(match f {
                    Func::StringToDate => format!("CAST(TO_TIMESTAMP({x}, {fmt}) AS DATE)"),
                    _ => format!("TO_TIMESTAMP({x}, {fmt})"),
                })
            }
            // the year of the Thursday of the week (DAYOFWEEK: Sunday is 1)
            Func::IsoYear => {
                let x = a(0)?;
                Some(int(format!("YEAR(DAYS_ADD({x}, 3 - PMOD(DAYOFWEEK({x}) + 5, 7)))")))
            }
            Func::EpochSeconds => Some(int(format!("UNIX_TIMESTAMP({})", a(0)?))),
            Func::StartsWith => Some(format!("(STRLEFT({x}, LENGTH({p})) = {p})", x = a(0)?, p = a(1)?)),
            Func::EndsWith => Some(format!("(STRRIGHT({x}, LENGTH({p})) = {p})", x = a(0)?, p = a(1)?)),
            Func::Contains => Some(format!("(LOCATE({}, {}) > 0)", a(1)?, a(0)?)),
            Func::Find => Some(match args.get(2) {
                None => format!("CAST(LOCATE({}, {}) - 1 AS BIGINT)", a(1)?, a(0)?),
                Some(start) => format!("CAST(LOCATE({}, {}, {} + 1) - 1 AS BIGINT)", a(1)?, a(0)?, c.sql(start)?),
            }),
            Func::RegexSearch => Some(format!("REGEXP_LIKE({}, {})", a(0)?, a(1)?)),
            Func::RegexReplace => Some(format!("REGEXP_REPLACE({}, {}, {})", a(0)?, a(1)?, a(2)?)),
            Func::IsNan => Some(format!("IS_NAN({})", a(0)?)),
            Func::IsInf => Some(format!("IS_INF({})", a(0)?)),
            // DAYOFWEEK: 1 is Sunday
            Func::DayOfWeekIndex => Some(format!("CAST(PMOD(DAYOFWEEK({}) - 2, 7) AS SMALLINT)", a(0)?)),
            Func::DayOfYear => Some(int(format!("DAYOFYEAR({})", a(0)?))),
            Func::WeekOfYear => Some(int(format!("WEEKOFYEAR({})", a(0)?))),
            Func::Millisecond => Some(int(format!("EXTRACT(MILLISECOND FROM {}) % 1000", a(0)?))),
            // EXTRACT stops at milliseconds
            Func::Microsecond => Some(int(format!("PMOD(UTC_TO_UNIX_MICROS({}), 1000000)", a(0)?))),
            Func::Truncate => {
                let x = a(0)?;
                let unit = match ExprCompiler::lit_str(&args[1]) {
                    Some("Y") => "YEAR",
                    // DATE_TRUNC has no quarter
                    Some("Q") => return Ok(Some(format!("TRUNC({x}, 'Q')"))),
                    Some("M") => "MONTH",
                    Some("W") => "WEEK",
                    Some("D") => "DAY",
                    Some("h") => "HOUR",
                    Some("m") => "MINUTE",
                    Some("s") => "SECOND",
                    Some("ms") => "MILLISECONDS",
                    Some("us") => "MICROSECONDS",
                    _ => return unsupported("truncate with a non-literal or unknown unit"),
                };
                Some(format!("DATE_TRUNC('{unit}', {x})"))
            }
            Func::Strftime => {
                let Some(fmt) = ExprCompiler::lit_str(&args[1]) else {
                    return unsupported("strftime with a non-literal format");
                };
                Some(format!(
                    "FROM_TIMESTAMP(CAST({} AS TIMESTAMP), {})",
                    a(0)?,
                    quote_str(&strftime_to_impala_format(fmt)?)
                ))
            }
            Func::DateFromYmd => Some(format!(
                "CAST(CONCAT(LPAD(CAST({} AS STRING), 4, '0'), '-', LPAD(CAST({} AS STRING), 2, '0'), '-', \
                 LPAD(CAST({} AS STRING), 2, '0')) AS DATE)",
                a(0)?,
                a(1)?,
                a(2)?
            )),
            Func::TimestampFromYmdhms => Some(format!(
                "CAST(CONCAT(LPAD(CAST({} AS STRING), 4, '0'), '-', LPAD(CAST({} AS STRING), 2, '0'), '-', \
                 LPAD(CAST({} AS STRING), 2, '0'), ' ', LPAD(CAST({} AS STRING), 2, '0'), ':', \
                 LPAD(CAST({} AS STRING), 2, '0'), ':', LPAD(CAST({} AS STRING), 2, '0')) AS TIMESTAMP)",
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

    fn agg(&self, c: &ExprCompiler, f: AggFunc, arg: Option<&Expr>, tail: &str) -> Result<Option<String>> {
        let Some(x) = arg else { return Ok(None) };
        Ok(match f {
            // no BOOL_OR / BOOL_AND; false sorts before true
            AggFunc::Any => Some(format!("MAX({}){tail}", c.sql(x)?)),
            AggFunc::All => Some(format!("MIN({}){tail}", c.sql(x)?)),
            AggFunc::ApproxCountDistinct => c.call_agg("NDV", x, tail)?,
            AggFunc::ApproxMedian => c.call_agg("APPX_MEDIAN", x, tail)?,
            // no arrays to gather values into
            AggFunc::Collect => return unsupported("collect"),
            _ => None,
        })
    }

    /// Several arguments to one `COUNT(DISTINCT ...)`, as in Ibis; a row
    /// failing the condition is made all NULLs.
    fn count_distinct_rows(&self, columns: &[String], condition: Option<&str>, tail: &str) -> Result<Option<String>> {
        let columns: Vec<String> = match condition {
            Some(c) => columns.iter().map(|x| format!("CASE WHEN {c} THEN {x} END")).collect(),
            None => columns.to_vec(),
        };
        Ok(Some(format!("COUNT(DISTINCT {}){tail}", columns.join(", "))))
    }
    /// `DATEDIFF` counts days and `INT_MONTHS_BETWEEN` months; shorter
    /// units come from the seconds between the two.
    fn date_diff(&self, unit: ibis_core::IntervalUnit, start: &str, end: &str) -> Result<Option<String>> {
        use ibis_core::IntervalUnit::*;
        let months = format!("INT_MONTHS_BETWEEN({end}, {start})");
        let days = format!("DATEDIFF({end}, {start})");
        let seconds = format!("(UNIX_TIMESTAMP({end}) - UNIX_TIMESTAMP({start}))");
        let count = match unit {
            Year => format!("{months} DIV 12"),
            Quarter => format!("{months} DIV 3"),
            Month => months,
            Week => format!("{days} DIV 7"),
            Day => days,
            Hour => format!("{seconds} DIV 3600"),
            Minute => format!("{seconds} DIV 60"),
            Second => seconds,
            _ => return unsupported("a delta finer than seconds"),
        };
        Ok(Some(format!("CAST({count} AS BIGINT)")))
    }
    fn timestamp_from_micros(&self, micros: &str) -> Result<Option<String>> {
        Ok(Some(format!("MICROSECONDS_ADD(CAST('1970-01-01' AS TIMESTAMP), {micros})")))
    }
    /// `moment + INTERVAL count DAYS`; a date moved is a timestamp.
    fn date_shift(
        &self,
        c: &ExprCompiler,
        moment: &str,
        t: &DataType,
        step: &ibis_core::Shift,
    ) -> Result<Option<String>> {
        let sign = if step.back { '-' } else { '+' };
        let unit = ibis_sql::interval_unit_name(step.unit);
        let moved = format!("{moment} {sign} INTERVAL ({}) {unit}S", c.sql(&step.count)?);
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
        if f != AggFunc::GroupConcat {
            return Ok(None);
        }
        c.unordered(options, "group_concat")?;
        Ok(Some(format!("GROUP_CONCAT({}, {}){tail}", c.sql(arg)?, c.sql(&options.args[0])?)))
    }

    fn analytic(&self, f: Analytic, args: &[String], over: &str) -> Result<Option<String>> {
        match f {
            Analytic::NthValue => unsupported("nth (no NTH_VALUE)"),
            // NTILE hands the rows left over after an even split to buckets
            // of its own choosing; the standard gives them to the first ones.
            // With q rows per bucket and r left over, the first r buckets
            // hold q + 1 rows each.
            Analytic::Ntile => {
                let Some(partition) = over.strip_suffix(')') else { return Ok(None) };
                let k = &args[0];
                let i = format!("(ROW_NUMBER() {over} - 1)");
                let count = format!("COUNT(*) {partition} ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING)");
                let (q, r) = (format!("({count} DIV {k})"), format!("({count} % {k})"));
                Ok(Some(format!(
                    "CAST(CASE WHEN {i} < {r} * ({q} + 1) THEN {i} DIV ({q} + 1) \
                     ELSE {r} + ({i} - {r} * ({q} + 1)) DIV {q} END AS BIGINT)"
                )))
            }
            _ => Ok(None),
        }
    }

    fn supports_agg_filter(&self, _windowed: bool) -> bool {
        false
    }

    fn supports_intersect_except_all(&self) -> bool {
        false
    }

    fn offset_needs_order_by(&self) -> bool {
        true
    }

    /// OFFSET only comes with a LIMIT.
    fn limit_offset(&self, limit: Option<u64>, offset: u64) -> String {
        match (limit, offset) {
            (None, 0) => String::new(),
            (None, o) => format!(" LIMIT {} OFFSET {o}", i64::MAX),
            (Some(n), 0) => format!(" LIMIT {n}"),
            (Some(n), o) => format!(" LIMIT {n} OFFSET {o}"),
        }
    }
}
