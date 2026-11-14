//! The SQL plugin seam. A backend's dialect overrides only what its engine
//! spells differently; everything else falls through to the defaults in
//! `compiler.rs` (DuckDB/Postgres-flavored ANSI SQL).

use ibis_core::{AggFunc, BinaryOp, DataType, Expr, Func, Result, Value};

use crate::compiler::ExprCompiler;

pub trait Dialect: Send + Sync {
    /// Backend name, used in `Unsupported` errors.
    fn name(&self) -> &str;

    fn quote_ident(&self, s: &str) -> String {
        format!("\"{}\"", s.replace('"', "\"\""))
    }

    fn type_name(&self, t: &DataType) -> Result<String> {
        default_type_name(self.name(), t)
    }

    /// Spell a literal. Return `None` to use the default.
    fn literal(&self, _v: &Value, _t: &DataType) -> Result<Option<String>> {
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
    /// `filter` is `""` or ` FILTER (WHERE ...)`; place it directly after the
    /// aggregate call (not after any expression wrapped around it).
    fn agg(&self, _c: &ExprCompiler, _f: AggFunc, _arg: Option<&Expr>, _filter: &str) -> Result<Option<String>> {
        Ok(None)
    }
    fn cast(&self, _c: &ExprCompiler, _e: &Expr, _to: &DataType) -> Result<Option<String>> {
        Ok(None)
    }

    /// Whether the engine has `INTERSECT ALL` and `EXCEPT ALL`. If not they
    /// are emulated with `ROW_NUMBER()`.
    fn supports_intersect_except_all(&self) -> bool {
        true
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
    })
}

/// A dialect with no overrides; useful for tests and as documentation of the defaults.
pub struct GenericDialect;

impl Dialect for GenericDialect {
    fn name(&self) -> &str {
        "generic"
    }
}
