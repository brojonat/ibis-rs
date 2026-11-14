//! Functions SQLite lacks, implemented in Rust and registered per connection,
//! so the default SQL from `ibis-sql` runs unchanged on SQLite.

use std::sync::Arc;

use regex::Regex;
use rusqlite::functions::{Aggregate, Context, FunctionFlags};
use rusqlite::{Connection, Result};

const DET: FunctionFlags = FunctionFlags::SQLITE_UTF8.union(FunctionFlags::SQLITE_DETERMINISTIC);

fn text(ctx: &Context, i: usize) -> Result<Option<String>> {
    ctx.get::<Option<String>>(i)
}

/// The compiled pattern in argument `i`, cached by SQLite for the statement
/// (compiling per row made regex cases ~100x slower).
fn regex(ctx: &Context, i: usize) -> Result<Arc<Regex>> {
    ctx.get_or_create_aux(i as std::os::raw::c_int, |v| -> std::result::Result<Regex, BoxError> {
        Ok(Regex::new(v.as_str()?)?)
    })
}

type UnaryF64 = fn(f64) -> f64;
type BinaryF64 = fn(f64, f64) -> f64;

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

fn pad(s: &str, n: usize, fill: &str, left: bool) -> String {
    let len = s.chars().count();
    if len >= n {
        return s.chars().take(n).collect();
    }
    let need: String = fill.chars().cycle().take(n - len).collect();
    if left { need + s } else { s.to_string() + &need }
}

pub fn register(conn: &Connection) -> Result<()> {
    conn.create_scalar_function("reverse", 1, DET, |ctx| {
        Ok(text(ctx, 0)?.map(|s| s.chars().rev().collect::<String>()))
    })?;
    conn.create_scalar_function("repeat", 2, DET, |ctx| {
        let n: i64 = ctx.get(1)?;
        Ok(text(ctx, 0)?.map(|s| s.repeat(n.max(0) as usize)))
    })?;
    conn.create_scalar_function("lpad", 3, DET, |ctx| {
        let (n, fill): (i64, String) = (ctx.get(1)?, ctx.get(2)?);
        Ok(text(ctx, 0)?.map(|s| pad(&s, n.max(0) as usize, &fill, true)))
    })?;
    conn.create_scalar_function("rpad", 3, DET, |ctx| {
        let (n, fill): (i64, String) = (ctx.get(1)?, ctx.get(2)?);
        Ok(text(ctx, 0)?.map(|s| pad(&s, n.max(0) as usize, &fill, false)))
    })?;
    conn.create_scalar_function("starts_with", 2, DET, |ctx| {
        Ok(match (text(ctx, 0)?, text(ctx, 1)?) {
            (Some(s), Some(p)) => Some(s.starts_with(&p)),
            _ => None,
        })
    })?;
    // 1-based character position, 0 if absent (DuckDB/Postgres STRPOS).
    conn.create_scalar_function("strpos", 2, DET, |ctx| {
        Ok(match (text(ctx, 0)?, text(ctx, 1)?) {
            (Some(s), Some(p)) => Some(s.find(&p).map_or(0, |b| s[..b].chars().count() as i64 + 1)),
            _ => None,
        })
    })?;
    conn.create_scalar_function("regexp_matches", 2, DET, |ctx| {
        let re = regex(ctx, 1)?;
        Ok(text(ctx, 0)?.map(|s| re.is_match(&s)))
    })?;
    conn.create_scalar_function("regexp_extract", 3, DET, |ctx| {
        let re = regex(ctx, 1)?;
        let idx: i64 = ctx.get(2)?;
        Ok(text(ctx, 0)?.map(|s| {
            re.captures(&s).and_then(|c| c.get(idx as usize).map(|m| m.as_str().to_string())).unwrap_or_default()
        }))
    })?;
    conn.create_scalar_function("regexp_replace", 4, DET, |ctx| {
        let re = regex(ctx, 1)?;
        let rep: String = ctx.get(2)?;
        Ok(text(ctx, 0)?.map(|s| re.replace_all(&s, rep.as_str()).into_owned()))
    })?;
    // Math functions: only present when SQLite is built with
    // SQLITE_ENABLE_MATH_FUNCTIONS, which the bundled build is not.
    let unary: [(&str, UnaryF64); 7] = [
        ("ceil", f64::ceil),
        ("floor", f64::floor),
        ("sqrt", f64::sqrt),
        ("exp", f64::exp),
        ("ln", f64::ln),
        ("log2", f64::log2),
        ("log10", f64::log10),
    ];
    for (name, f) in unary {
        conn.create_scalar_function(name, 1, DET, move |ctx| {
            Ok(ctx.get::<Option<f64>>(0)?.map(f).filter(|v| v.is_finite() || v.is_infinite()))
        })?;
    }
    let binary: [(&str, BinaryF64); 3] = [("power", f64::powf), ("pow", f64::powf), ("fmod", |a, b| a % b)];
    for (name, f) in binary {
        conn.create_scalar_function(name, 2, DET, move |ctx| {
            Ok(match (ctx.get::<Option<f64>>(0)?, ctx.get::<Option<f64>>(1)?) {
                (Some(a), Some(b)) => Some(f(a, b)).filter(|v| !v.is_nan()),
                _ => None,
            })
        })?;
    }
    for (name, sample, root) in
        [("var_samp", true, false), ("var_pop", false, false), ("stddev_samp", true, true), ("stddev_pop", false, true)]
    {
        conn.create_aggregate_function(name, 1, DET, Moments { sample, root })?;
    }
    Ok(())
}

/// Welford's online variance.
struct Moments {
    sample: bool,
    root: bool,
}

#[derive(Default)]
struct Acc {
    n: f64,
    mean: f64,
    m2: f64,
}

impl Aggregate<Acc, Option<f64>> for Moments {
    fn init(&self, _: &mut Context<'_>) -> Result<Acc> {
        Ok(Acc::default())
    }

    fn step(&self, ctx: &mut Context<'_>, acc: &mut Acc) -> Result<()> {
        if let Some(x) = ctx.get::<Option<f64>>(0)? {
            acc.n += 1.0;
            let d = x - acc.mean;
            acc.mean += d / acc.n;
            acc.m2 += d * (x - acc.mean);
        }
        Ok(())
    }

    fn finalize(&self, _: &mut Context<'_>, acc: Option<Acc>) -> Result<Option<f64>> {
        let Some(a) = acc else { return Ok(None) };
        let denom = if self.sample { a.n - 1.0 } else { a.n };
        if denom <= 0.0 {
            return Ok(None);
        }
        let v = a.m2 / denom;
        Ok(Some(if self.root { v.sqrt() } else { v }))
    }
}
