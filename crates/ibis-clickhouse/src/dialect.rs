//! What ClickHouse spells or means differently from the defaults in `ibis-sql`.
//!
//! ClickHouse function names are case-sensitive and mostly camelCase; only the
//! ones it shares with standard SQL (`SUM`, `ROUND`, `COALESCE`, ...) also
//! answer to the upper-case spelling the defaults use.

use ibis_core::convert::{format_date, format_timestamp};
use ibis_core::{AggFunc, AggOptions, Analytic, DataType, Error, Expr, Func, Lambda, Result, Value};
use ibis_sql::{Dialect, ExprCompiler, Unnest};

use crate::nested;

pub struct ClickHouseDialect;

/// A string literal. ClickHouse reads backslash escapes inside quotes.
pub(crate) fn quote_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

pub(crate) fn unsupported<T>(op: impl Into<String>) -> Result<T> {
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

    /// A column that takes no NULL is one whose type is not `Nullable`.
    fn column_type(&self, t: &DataType, nullable: bool) -> Result<String> {
        let name = self.type_name(t)?;
        let bare = name.strip_prefix("Nullable(").and_then(|n| n.strip_suffix(')'));
        Ok(match (nullable, bare) {
            (false, Some(bare)) => bare.to_string(),
            _ => name,
        })
    }
    /// A temporary table lasts as long as a session, and each statement
    /// sent over HTTP is a session of its own.
    fn temporary(&self) -> Option<&'static str> {
        None
    }
    /// Insertion order is kept: there is no sorting key.
    fn create_table_suffix(&self) -> &'static str {
        " ENGINE = MergeTree ORDER BY tuple()"
    }

    /// Always `Nullable`: a bare ClickHouse type cannot hold NULL, and a cast
    /// to one turns NULL into an error.
    fn type_name(&self, t: &DataType) -> Result<String> {
        // arrays, tuples and maps cannot be Nullable
        if let Some(name) = nested::type_name(self, t)? {
            return Ok(name);
        }
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
            DataType::Array(_) | DataType::Struct(_) | DataType::Map(..) => unreachable!("named above"),
            DataType::Interval(_) => return unsupported(format!("the type {t}")),
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

    fn rename_table(&self, old: &str, new: &str) -> String {
        format!("RENAME TABLE {old} TO {new}")
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

    fn unnest(&self, array: &str, _item: &DataType, alias: &str, with_offset: bool, keep_empty: bool) -> Unnest {
        nested::unnest(array, alias, with_offset, keep_empty)
    }

    fn func(&self, c: &ExprCompiler, f: Func, args: &[Expr]) -> Result<Option<String>> {
        if let Some(sql) = nested::func(self, c, f, args)? {
            return Ok(Some(sql));
        }
        let a = |i: usize| c.sql(&args[i]);
        let cast = |s: String, t: DataType| -> Result<Option<String>> {
            Ok(Some(format!("CAST({s} AS {})", self.type_name(&t)?)))
        };
        let int = |s: String| cast(s, DataType::Int32);
        Ok(match f {
            Func::StringToTimestamp | Func::StringToDate => {
                let Some(fmt) = ExprCompiler::lit_str(&args[1]) else {
                    return unsupported("a string parsed by a non-literal format");
                };
                // parseDateTime reads the patterns formatDateTime writes; NULL where it does not match
                let parsed = format!("parseDateTimeOrNull({}, {})", a(0)?, quote_str(&format_datetime_pattern(fmt)?));
                return match f {
                    Func::StringToDate => cast(parsed, DataType::Date),
                    _ => cast(parsed, DataType::Timestamp { tz: None }),
                };
            }
            Func::IsoYear => return int(format!("toISOYear({})", a(0)?)),
            Func::EpochSeconds => return int(format!("toRelativeSecondNum({})", a(0)?)),
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
            AggFunc::Collect => Some(format!("groupArray({}){tail}", c.sql(x)?)),
            AggFunc::ApproxCountDistinct => c.call_agg("uniq", x, tail)?,
            AggFunc::BitAnd => c.call_agg("groupBitAnd", x, tail)?,
            AggFunc::BitOr => c.call_agg("groupBitOr", x, tail)?,
            AggFunc::BitXor => c.call_agg("groupBitXor", x, tail)?,
            // `any` and `anyLast` pass over NULLs
            AggFunc::Arbitrary | AggFunc::First => c.call_agg("any", x, tail)?,
            AggFunc::Last => c.call_agg("anyLast", x, tail)?,
            // interpolates between the two middle values, as the others do
            AggFunc::Median => c.call_agg("quantileExactInclusive(0.5)", x, tail)?,
            AggFunc::ApproxMedian => c.call_agg("median", x, tail)?,
            _ => None,
        })
    }

    fn date_diff(&self, unit: ibis_core::IntervalUnit, start: &str, end: &str) -> Result<Option<String>> {
        let unit = ibis_sql::interval_unit_name(unit).to_lowercase();
        Ok(Some(format!("dateDiff('{unit}', {start}, {end})")))
    }
    /// In UTC, whatever zone the server is in.
    fn timestamp_from_micros(&self, micros: &str) -> Result<Option<String>> {
        Ok(Some(format!("fromUnixTimestamp64Micro({micros}, 'UTC')")))
    }
    /// `moment + toIntervalDay(count)`: the functions take a count that is
    /// not a literal, which `INTERVAL n DAY` does not everywhere.
    fn date_shift(
        &self,
        c: &ExprCompiler,
        moment: &str,
        _t: &DataType,
        step: &ibis_core::Shift,
    ) -> Result<Option<String>> {
        use ibis_core::IntervalUnit::*;
        let name = match step.unit {
            Year => "toIntervalYear",
            Quarter => "toIntervalQuarter",
            Month => "toIntervalMonth",
            Week => "toIntervalWeek",
            Day => "toIntervalDay",
            Hour => "toIntervalHour",
            Minute => "toIntervalMinute",
            Second => "toIntervalSecond",
            Millisecond => "toIntervalMillisecond",
            Microsecond => "toIntervalMicrosecond",
            Nanosecond => "toIntervalNanosecond",
        };
        let sign = if step.back { '-' } else { '+' };
        Ok(Some(format!("({moment} {sign} {name}({}))", c.sql(&step.count)?)))
    }
    fn reduction(
        &self,
        c: &ExprCompiler,
        f: AggFunc,
        arg: &Expr,
        options: &AggOptions,
        tail: &str,
    ) -> Result<Option<String>> {
        if let AggFunc::Quantile | AggFunc::ApproxQuantile = f {
            // The fractions are parameters of the function. `quantile`
            // samples large inputs; `quantileExactInclusive` interpolates as
            // the other engines do, and takes numbers only.
            let (fractions, many) = c.fractions(options)?;
            let dtype = c.dtype(arg)?;
            let numeric = dtype.is_numeric();
            let name = match (f == AggFunc::Quantile, numeric, many) {
                (true, true, true) => "quantilesExactInclusive",
                (true, true, false) => "quantileExactInclusive",
                // Of dates: one of the values, and of two middle ones the
                // lower, as PERCENTILE_DISC picks (`quantileExact` the higher).
                (true, false, true) => "quantilesExactLow",
                (true, false, false) => "quantileExactLow",
                (false, _, true) => "quantiles",
                (false, _, false) => "quantile",
            };
            let fractions: Vec<String> = fractions.iter().map(|q| format!("{q:?}")).collect();
            let fractions = fractions.join(", ");
            // no quantile function takes a Date32: as days since 1970, and back
            if dtype == DataType::Date && !many {
                return Ok(Some(format!("toDate32({name}({fractions})(toInt32({})){tail})", c.sql(arg)?)));
            }
            return Ok(Some(format!("{name}({fractions})({}){tail}", c.sql(arg)?)));
        }
        if let AggFunc::First | AggFunc::Last | AggFunc::Collect = f {
            // `any`, `anyLast` and `groupArray` pass over NULLs, and no
            // aggregate reads its rows in an order
            let what = if f == AggFunc::Collect { "collect" } else { "first or last" };
            c.unordered(options, what)?;
            if options.include_null {
                return Err(Error::Unsupported { backend: "clickhouse".into(), op: format!("{what} of NULLs") });
            }
            return Ok(match f {
                AggFunc::Collect => Some(format!("groupUniqArray({}){tail}", c.sql(arg)?)),
                _ => None,
            });
        }
        let (x, other) = (c.sql(arg)?, c.sql(&options.args[0])?);
        if f == AggFunc::GroupConcat {
            c.unordered(options, "group_concat")?;
            // no strings: NULL, where joining an empty array gives ''
            let list = format!("groupArray({x}){tail}");
            return Ok(Some(format!("if(empty({list}), NULL, arrayStringConcat({list}, {other}))")));
        }
        Ok(Some(match f {
            // `argMin` passes over a NULL argument; inside a tuple it is a value
            AggFunc::ArgMin => format!("tupleElement(argMin(tuple({x}), {other}){tail}, 1)"),
            AggFunc::ArgMax => format!("tupleElement(argMax(tuple({x}), {other}){tail}, 1)"),
            AggFunc::CovSamp => format!("covarSamp({x}, {other}){tail}"),
            AggFunc::CovPop => format!("covarPop({x}, {other}){tail}"),
            // the only one ClickHouse names (Ibis: "only implements `sample` correlation")
            AggFunc::CorrSamp => format!("corr({x}, {other}){tail}"),
            _ => return Ok(None),
        }))
    }
}
