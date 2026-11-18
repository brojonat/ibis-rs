//! The SQL plugin seam. A backend's dialect overrides only what its engine
//! spells differently; everything else falls through to the defaults in
//! `compiler.rs` (DuckDB/Postgres-flavored ANSI SQL).

use ibis_core::{AggFunc, Analytic, BinaryOp, DataType, Expr, Func, Lambda, Result, Value};

use crate::compiler::ExprCompiler;

pub trait Dialect: Send + Sync {
    /// Backend name, used in `Unsupported` errors.
    fn name(&self) -> &str;

    fn quote_ident(&self, s: &str) -> String {
        format!("\"{}\"", s.replace('"', "\"\""))
    }

    /// The name of a type in DDL. The default spells nested types as DuckDB
    /// does: `T[]`, `STRUCT(a T, b U)`, `MAP(K, V)`.
    fn type_name(&self, t: &DataType) -> Result<String> {
        Ok(match t {
            DataType::Array(item) => format!("{}[]", self.type_name(item)?),
            DataType::Map(key, value) => format!("MAP({}, {})", self.type_name(key)?, self.type_name(value)?),
            DataType::Struct(fields) => {
                let fields = fields
                    .iter()
                    .map(|(name, t)| Ok(format!("{} {}", self.quote_ident(name), self.type_name(t)?)))
                    .collect::<Result<Vec<_>>>()?;
                format!("STRUCT({})", fields.join(", "))
            }
            t => default_type_name(self.name(), t)?,
        })
    }

    /// A type as the target of a `CAST`. Differs from `type_name` only where
    /// casts take their own vocabulary (MySQL: `SIGNED`, `CHAR`, ...).
    fn cast_type_name(&self, t: &DataType) -> Result<String> {
        self.type_name(t)
    }

    /// Spell a literal. Return `None` to use the default.
    fn literal(&self, _v: &Value, _t: &DataType) -> Result<Option<String>> {
        Ok(None)
    }

    /// Spell an array, struct or map literal; `c.literal_parts(v, t)` compiles
    /// its parts. Return `None` for DuckDB's `[..]`, `{'a': ..}` and `MAP`.
    fn nested_literal(&self, _c: &ExprCompiler, _v: &Value, _t: &DataType) -> Result<Option<String>> {
        Ok(None)
    }
    /// Per-op overrides. Return `Ok(None)` to use the default spelling;
    /// call `c.sql(arg)` to compile arguments.
    fn func(&self, _c: &ExprCompiler, _f: Func, _args: &[Expr]) -> Result<Option<String>> {
        Ok(None)
    }
    fn binary(&self, _c: &ExprCompiler, _op: BinaryOp, _l: &Expr, _r: &Expr) -> Result<Option<String>> {
        Ok(None)
    }
    /// `tail` is the call's ` FILTER (WHERE ...)` and ` OVER (...)` clauses
    /// (either may be absent): place it directly after the aggregate call,
    /// inside any expression wrapped around it.
    fn agg(&self, _c: &ExprCompiler, _f: AggFunc, _arg: Option<&Expr>, _tail: &str) -> Result<Option<String>> {
        Ok(None)
    }
    /// A ranking or offset function over a window. `args` are already
    /// compiled and `over` is the whole `OVER (...)` clause.
    fn analytic(&self, _f: Analytic, _args: &[String], _over: &str) -> Result<Option<String>> {
        Ok(None)
    }
    fn cast(&self, _c: &ExprCompiler, _e: &Expr, _to: &DataType) -> Result<Option<String>> {
        Ok(None)
    }
    /// `arr.map(f)` or `arr.filter(f)`. `body` is typed; inside it the
    /// element is `Expr::Param(param)` and its 0-based position
    /// `Expr::Param(index)`, each compiled to its bare name. Return `None`
    /// for DuckDB's `LIST_APPLY(arr, LAMBDA x, i: body)`.
    fn array_lambda(
        &self,
        _c: &ExprCompiler,
        _kind: Lambda,
        _arg: &Expr,
        _param: &str,
        _index: Option<&str>,
        _body: &Expr,
    ) -> Result<Option<String>> {
        Ok(None)
    }
    /// The join that unnests the array `array` (compiled SQL over the left
    /// side) into one row per element, under the alias `alias`.
    /// `with_offset` asks for the element's position as well; `keep_empty`
    /// keeps a row for an empty or NULL array.
    fn unnest(&self, array: &str, alias: &str, with_offset: bool, keep_empty: bool) -> Unnest {
        let ordinality = if with_offset { " WITH ORDINALITY" } else { "" };
        let columns = if with_offset { "(v, i)" } else { "(v)" };
        let table = format!("UNNEST({array}){ordinality} AS {alias}{columns}");
        Unnest {
            join: if keep_empty { format!("LEFT JOIN {table} ON TRUE") } else { format!("CROSS JOIN {table}") },
            value: format!("{alias}.v"),
            offset: format!("{alias}.i - 1"),
        }
    }

    /// One `ORDER BY` key over the compiled expression `expr`.
    fn sort_key(&self, expr: &str, ascending: bool, nulls_first: bool) -> String {
        format!(
            "{expr} {} NULLS {}",
            if ascending { "ASC" } else { "DESC" },
            if nulls_first { "FIRST" } else { "LAST" }
        )
    }

    /// Whether aggregates take `FILTER (WHERE ...)`; `windowed` asks about
    /// one with an `OVER` clause. If not, the condition moves into the
    /// argument: `SUM(CASE WHEN cond THEN x END)`.
    fn supports_agg_filter(&self, _windowed: bool) -> bool {
        true
    }

    /// Whether the engine has `FULL OUTER JOIN`. If not it is a left join
    /// plus the right rows no left row matches.
    fn supports_full_outer_join(&self) -> bool {
        true
    }

    /// Whether the engine has `INTERSECT ALL` and `EXCEPT ALL`. If not they
    /// are emulated with `ROW_NUMBER()`.
    fn supports_intersect_except_all(&self) -> bool {
        true
    }

    /// Whether `OFFSET` is refused without an `ORDER BY`. If so, a query
    /// that skips rows without ordering them is ordered by its first column.
    fn offset_needs_order_by(&self) -> bool {
        false
    }

    /// `LIMIT`/`OFFSET` clause. SQLite needs `LIMIT -1` to use a bare OFFSET.
    fn limit_offset(&self, limit: Option<u64>, offset: u64) -> String {
        let mut s = String::new();
        if let Some(n) = limit {
            s.push_str(&format!(" LIMIT {n}"));
        }
        if offset > 0 {
            s.push_str(&format!(" OFFSET {offset}"));
        }
        s
    }
}

/// A lateral join over the elements of an array, from [`Dialect::unnest`].
pub struct Unnest {
    /// The join clause, to follow the left side in `FROM`.
    pub join: String,
    /// The element, as SQL.
    pub value: String,
    /// The element's 0-based position, as SQL (used only when asked for).
    pub offset: String,
}

pub fn default_type_name(backend: &str, t: &DataType) -> Result<String> {
    use DataType::*;
    Ok(match t {
        Null => "NULL".into(),
        Boolean => "BOOLEAN".into(),
        Int8 => "TINYINT".into(),
        Int16 => "SMALLINT".into(),
        Int32 => "INTEGER".into(),
        Int64 => "BIGINT".into(),
        UInt8 => "UTINYINT".into(),
        UInt16 => "USMALLINT".into(),
        UInt32 => "UINTEGER".into(),
        UInt64 => "UBIGINT".into(),
        Float32 => "REAL".into(),
        Float64 => "DOUBLE".into(),
        Decimal { precision, scale } => format!("DECIMAL({precision}, {scale})"),
        String => "VARCHAR".into(),
        Binary => "BLOB".into(),
        Date => "DATE".into(),
        Timestamp { tz: None } => "TIMESTAMP".into(),
        Timestamp { tz: Some(_) } => {
            return Err(ibis_core::Error::Unsupported { backend: backend.into(), op: "timestamp with tz".into() });
        }
        // each engine that has them names them its own way, in `Dialect::type_name`
        Array(_) | Struct(_) | Map(..) => {
            return Err(ibis_core::Error::Unsupported { backend: backend.into(), op: format!("the type {t}") });
        }
    })
}

/// A C `strftime` pattern in the pattern language of MySQL's `DATE_FORMAT`,
/// which Trino's `date_format` shares. The two agree except that MySQL reads
/// `%M` as the month name (`%i` is minutes) and `%W` as the weekday name.
pub fn strftime_to_date_format(backend: &str, fmt: &str) -> Result<String> {
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
                return Err(ibis_core::Error::Unsupported {
                    backend: backend.into(),
                    op: format!("strftime directive %{}", other.map(String::from).unwrap_or_default()),
                });
            }
        });
    }
    Ok(out)
}

/// A C `strftime` pattern in the pattern language of Java's
/// `DateTimeFormatter`, which Spark's `date_format` takes: letters are
/// fields, so literal text is quoted.
pub fn strftime_to_java_format(backend: &str, fmt: &str) -> Result<String> {
    let mut out = String::new();
    // literal letters waiting for their closing quote
    let mut text = String::new();
    let flush = |out: &mut String, text: &mut String| {
        if !text.is_empty() {
            out.push_str(&format!("'{text}'"));
            text.clear();
        }
    };
    let mut chars = fmt.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '%' => {}
            // inside quotes, a doubled quote is one quote
            '\'' => {
                text.push_str("''");
                continue;
            }
            ch if ch.is_ascii_alphabetic() => {
                text.push(ch);
                continue;
            }
            ch => {
                flush(&mut out, &mut text);
                out.push(ch);
                continue;
            }
        }
        let field = match chars.next() {
            Some('Y') => "yyyy",
            Some('y') => "yy",
            Some('m') => "MM",
            Some('d') => "dd",
            Some('H') => "HH",
            Some('I') => "hh",
            Some('M') => "mm",
            Some('S') => "ss",
            Some('f') => "SSSSSS",
            Some('j') => "DDD",
            Some('p') => "a",
            Some('B') => "MMMM",
            Some('b') => "MMM",
            Some('A') => "EEEE",
            Some('a') => "EEE",
            Some('%') => "%",
            other => {
                return Err(ibis_core::Error::Unsupported {
                    backend: backend.into(),
                    op: format!("strftime directive %{}", other.map(String::from).unwrap_or_default()),
                });
            }
        };
        flush(&mut out, &mut text);
        out.push_str(field);
    }
    flush(&mut out, &mut text);
    Ok(out)
}

/// A dialect with no overrides; useful for tests and as documentation of the defaults.
pub struct GenericDialect;

impl Dialect for GenericDialect {
    fn name(&self) -> &str {
        "generic"
    }
}
