//! What MySQL spells or means differently from the defaults in `ibis-sql`.

use ibis_core::{AggFunc, BinaryOp, DataType, Error, Expr, Func, Result, Value};
use ibis_sql::{Dialect, ExprCompiler, strftime_to_date_format};

pub struct MySqlDialect;

/// A string literal. MySQL reads backslash escapes inside quotes.
pub(crate) fn quote_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
}

fn unsupported<T>(op: impl Into<String>) -> Result<T> {
    Err(Error::Unsupported { backend: "mysql".into(), op: op.into() })
}

impl Dialect for MySqlDialect {
    fn name(&self) -> &str {
        "mysql"
    }

    fn quote_ident(&self, s: &str) -> String {
        format!("`{}`", s.replace('`', "``"))
    }

    /// Column types. There is no boolean: `BOOLEAN` is `TINYINT(1)`, which
    /// `MySql::table` reads back as one.
    fn type_name(&self, t: &DataType) -> Result<String> {
        Ok(match t {
            DataType::Int32 => "INT".into(),
            DataType::UInt8 => "TINYINT UNSIGNED".into(),
            DataType::UInt16 => "SMALLINT UNSIGNED".into(),
            DataType::UInt32 => "INT UNSIGNED".into(),
            DataType::UInt64 => "BIGINT UNSIGNED".into(),
            DataType::Float32 => "FLOAT".into(),
            DataType::String => "TEXT".into(),
            // DATETIME is the zone-less type; TIMESTAMP is stored in UTC
            DataType::Timestamp { tz: None } => "DATETIME(6)".into(),
            DataType::Timestamp { tz: Some(_) } => "TIMESTAMP(6)".into(),
            t => ibis_sql::default_type_name("mysql", t)?,
        })
    }

    /// `CAST` takes a vocabulary of its own: no `BIGINT`, `VARCHAR` or `BOOLEAN`.
    fn cast_type_name(&self, t: &DataType) -> Result<String> {
        Ok(match t {
            DataType::Null => return unsupported("cast to null"),
            DataType::Boolean | DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::Int64 => "SIGNED".into(),
            DataType::UInt8 | DataType::UInt16 | DataType::UInt32 | DataType::UInt64 => "UNSIGNED".into(),
            DataType::Float32 => "FLOAT".into(),
            DataType::Float64 => "DOUBLE".into(),
            DataType::Decimal { precision, scale } => format!("DECIMAL({precision}, {scale})"),
            DataType::String => "CHAR".into(),
            DataType::Binary => "BINARY".into(),
            DataType::Date => "DATE".into(),
            DataType::Timestamp { .. } => "DATETIME(6)".into(),
            DataType::Array(_) | DataType::Struct(_) | DataType::Map(..) => {
                return unsupported(format!("the type {t}"));
            }
        })
    }

    fn literal(&self, v: &Value, _t: &DataType) -> Result<Option<String>> {
        Ok(match v {
            Value::String(s) => Some(quote_str(s)),
            Value::Float(f) if !f.is_finite() => return unsupported("NaN and infinity (DOUBLE has neither)"),
            // with an exponent it is a DOUBLE; `1.5` would be an exact DECIMAL
            Value::Float(f) => Some(format!("{f:e}")),
            _ => None,
        })
    }

    fn cast(&self, c: &ExprCompiler, e: &Expr, to: &DataType) -> Result<Option<String>> {
        let from = c.dtype(e)?;
        Ok(match to {
            // a truth value is 0 or 1
            DataType::Boolean if from.is_numeric() || from == DataType::Boolean => {
                Some(format!("({} <> 0)", c.sql(e)?))
            }
            _ => None,
        })
    }

    fn binary(&self, c: &ExprCompiler, op: BinaryOp, l: &Expr, r: &Expr) -> Result<Option<String>> {
        // `||` is logical OR
        if op == BinaryOp::Add && c.dtype(l)? == DataType::String && c.dtype(r)? == DataType::String {
            return Ok(Some(format!("CONCAT({}, {})", c.sql(l)?, c.sql(r)?)));
        }
        Ok(None)
    }

    fn func(&self, c: &ExprCompiler, f: Func, args: &[Expr]) -> Result<Option<String>> {
        let a = |i: usize| c.sql(&args[i]);
        // without a NaN or an infinity, no value is one; NULL stays NULL
        let never = |x: String| format!("({x} IS NULL AND NULL)");
        Ok(match f {
            // LENGTH counts bytes
            Func::Length => Some(format!("CAST(CHAR_LENGTH({}) AS SIGNED)", a(0)?)),
            Func::StartsWith => Some(format!("(LEFT({x}, CHAR_LENGTH({p})) = {p})", x = a(0)?, p = a(1)?)),
            Func::EndsWith => Some(format!("(RIGHT({x}, CHAR_LENGTH({p})) = {p})", x = a(0)?, p = a(1)?)),
            Func::Contains => Some(format!("(LOCATE({}, {}) > 0)", a(1)?, a(0)?)),
            Func::Find => Some(match args.get(2) {
                None => format!("(LOCATE({}, {}) - 1)", a(1)?, a(0)?),
                Some(start) => format!("(LOCATE({}, {}, {} + 1) - 1)", a(1)?, a(0)?, c.sql(start)?),
            }),
            Func::ILike => Some(format!("(LOWER({}) LIKE LOWER({}))", a(0)?, a(1)?)),
            Func::Capitalize => {
                let x = a(0)?;
                Some(format!("CONCAT(UPPER(SUBSTRING({x}, 1, 1)), LOWER(SUBSTRING({x}, 2)))"))
            }
            // 'c': case-sensitive whatever the collation
            Func::RegexSearch => Some(format!("REGEXP_LIKE({}, {}, 'c')", a(0)?, a(1)?)),
            // REGEXP_SUBSTR returns the whole match; a group is cut out of it
            // by matching again. No match gives '' like DuckDB.
            Func::RegexExtract => {
                let (x, p) = (a(0)?, a(1)?);
                let whole = format!("REGEXP_SUBSTR({x}, {p}, 1, 1, 'c')");
                let matched = match ExprCompiler::lit_int(&args[2]) {
                    Some(0) => whole,
                    Some(group) => format!("REGEXP_REPLACE({whole}, {p}, '${group}', 1, 1, 'c')"),
                    None => return unsupported("regex extract with a non-literal group"),
                };
                Some(format!("CASE WHEN {x} IS NULL THEN NULL ELSE COALESCE({matched}, '') END"))
            }
            Func::RegexReplace => Some(format!("REGEXP_REPLACE({}, {}, {}, 1, 0, 'c')", a(0)?, a(1)?, a(2)?)),
            Func::IsNan | Func::IsInf => Some(never(a(0)?)),
            // ROUND of a DOUBLE rounds half to even; Ibis rounds half away from zero
            Func::Round if c.dtype(&args[0])?.is_floating() => {
                let x = a(0)?;
                Some(match args.get(1) {
                    None => format!("CAST(SIGN({x}) * FLOOR(ABS({x}) + 0.5) AS SIGNED)"),
                    Some(digits) => {
                        let scale = format!("POW(10, {})", c.sql(digits)?);
                        format!("(SIGN({x}) * FLOOR(ABS({x}) * {scale} + 0.5) / {scale})")
                    }
                })
            }
            // WEEKDAY: 0 is Monday
            Func::DayOfWeekIndex => Some(format!("CAST(WEEKDAY({}) AS SIGNED)", a(0)?)),
            Func::DayOfYear => Some(format!("CAST(DAYOFYEAR({}) AS SIGNED)", a(0)?)),
            Func::WeekOfYear => Some(format!("CAST(WEEKOFYEAR({}) AS SIGNED)", a(0)?)),
            Func::Millisecond => Some(format!("CAST(FLOOR(MICROSECOND({}) / 1000) AS SIGNED)", a(0)?)),
            Func::Microsecond => Some(format!("CAST(MICROSECOND({}) AS SIGNED)", a(0)?)),
            // no DATE_TRUNC
            Func::Truncate => {
                let x = a(0)?;
                let fmt = |f: &str| format!("DATE_FORMAT({x}, '{f}')");
                let s = match ExprCompiler::lit_str(&args[1]) {
                    Some("Y") => fmt("%Y-01-01"),
                    Some("Q") => format!("MAKEDATE(YEAR({x}), 1) + INTERVAL (QUARTER({x}) - 1) QUARTER"),
                    Some("M") => fmt("%Y-%m-01"),
                    Some("W") => format!("DATE({x}) - INTERVAL WEEKDAY({x}) DAY"),
                    Some("D") => format!("DATE({x})"),
                    Some("h") => fmt("%Y-%m-%d %H:00:00"),
                    Some("m") => fmt("%Y-%m-%d %H:%i:00"),
                    Some("s") => fmt("%Y-%m-%d %H:%i:%s"),
                    Some("ms") => format!("{x} - INTERVAL (MICROSECOND({x}) % 1000) MICROSECOND"),
                    Some("us") => x.clone(),
                    _ => return unsupported("truncate with a non-literal or unknown unit"),
                };
                Some(format!("CAST({s} AS {})", self.cast_type_name(&c.dtype(&args[0])?)?))
            }
            Func::Strftime => {
                let Some(fmt) = ExprCompiler::lit_str(&args[1]) else {
                    return unsupported("strftime with a non-literal format");
                };
                Some(format!("DATE_FORMAT({}, {})", a(0)?, quote_str(&strftime_to_date_format("mysql", fmt)?)))
            }
            Func::DateFromYmd => {
                Some(format!("STR_TO_DATE(CONCAT_WS('-', {}, {}, {}), '%Y-%m-%d')", a(0)?, a(1)?, a(2)?))
            }
            Func::TimestampFromYmdhms => Some(format!(
                "CAST(TIMESTAMP(STR_TO_DATE(CONCAT_WS('-', {}, {}, {}), '%Y-%m-%d'), MAKETIME({}, {}, {})) AS DATETIME(6))",
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
            // no BOOL_OR / BOOL_AND; a boolean is 0 or 1
            AggFunc::Any => Some(format!("MAX({}){tail}", c.sql(x)?)),
            AggFunc::All => Some(format!("MIN({}){tail}", c.sql(x)?)),
            AggFunc::ApproxCountDistinct => Some(format!("COUNT(DISTINCT {}){tail}", c.sql(x)?)),
            // unsigned in MySQL; the columns are not
            AggFunc::BitAnd => Some(format!("CAST(BIT_AND({}){tail} AS SIGNED)", c.sql(x)?)),
            AggFunc::BitOr => Some(format!("CAST(BIT_OR({}){tail} AS SIGNED)", c.sql(x)?)),
            AggFunc::BitXor => Some(format!("CAST(BIT_XOR({}){tail} AS SIGNED)", c.sql(x)?)),
            _ => None,
        })
    }

    /// No `NULLS FIRST` / `NULLS LAST`. NULL sorts as the smallest value, so
    /// the other placement takes a leading key on nullness.
    fn sort_key(&self, expr: &str, ascending: bool, nulls_first: bool) -> String {
        let dir = if ascending { "ASC" } else { "DESC" };
        if nulls_first == ascending {
            format!("{expr} {dir}")
        } else {
            format!("{expr} IS NULL {}, {expr} {dir}", if nulls_first { "DESC" } else { "ASC" })
        }
    }

    fn supports_agg_filter(&self, _windowed: bool) -> bool {
        false
    }

    fn supports_full_outer_join(&self) -> bool {
        false
    }

    /// OFFSET only comes with a LIMIT; the manual's "no limit" is 2^64 - 1.
    fn limit_offset(&self, limit: Option<u64>, offset: u64) -> String {
        match (limit, offset) {
            (None, 0) => String::new(),
            (None, o) => format!(" LIMIT 18446744073709551615 OFFSET {o}"),
            (Some(n), 0) => format!(" LIMIT {n}"),
            (Some(n), o) => format!(" LIMIT {n} OFFSET {o}"),
        }
    }
}
