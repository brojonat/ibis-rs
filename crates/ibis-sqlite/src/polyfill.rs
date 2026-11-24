//! Functions SQLite lacks, implemented in Rust and registered per connection,
//! so the default SQL from `ibis-sql` runs unchanged on SQLite.

use std::sync::Arc;

use regex::Regex;
use rusqlite::functions::{Aggregate, Context, FunctionFlags};
use rusqlite::types::Value as SqlValue;
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

fn translate(s: &str, from: &str, to: &str) -> String {
    let (from, to): (Vec<char>, Vec<char>) = (from.chars().collect(), to.chars().collect());
    s.chars()
        .filter_map(|c| match from.iter().position(|f| *f == c) {
            Some(i) => to.get(i).copied(),
            None => Some(c),
        })
        .collect()
}

/// The edits (a character inserted, deleted or replaced) between two strings.
fn levenshtein(a: &str, b: &str) -> i64 {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let replace = diagonal + usize::from(ca != *cb);
            diagonal = row[j + 1];
            row[j + 1] = replace.min(row[j] + 1).min(diagonal + 1);
        }
    }
    row[b.len()] as i64
}

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
    // An int64 from any value: FNV-1a over its type and its text.
    conn.create_scalar_function("ibis_hash", 1, DET, |ctx| {
        let v = ctx.get_raw(0);
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |bytes: &[u8]| {
            for b in bytes {
                h = (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        feed(&[v.data_type() as u8]);
        match v {
            rusqlite::types::ValueRef::Null => {}
            rusqlite::types::ValueRef::Integer(i) => feed(&i.to_le_bytes()),
            rusqlite::types::ValueRef::Real(f) => feed(&f.to_le_bytes()),
            rusqlite::types::ValueRef::Text(t) | rusqlite::types::ValueRef::Blob(t) => feed(t),
        }
        Ok(h as i64)
    })?;
    // UPPER and LOWER know ASCII only.
    conn.create_scalar_function("ibis_upper", 1, DET, |ctx| Ok(text(ctx, 0)?.map(|s| s.to_uppercase())))?;
    conn.create_scalar_function("ibis_lower", 1, DET, |ctx| Ok(text(ctx, 0)?.map(|s| s.to_lowercase())))?;
    // Each character found in `from` becomes the one at its place in `to`,
    // or goes if `to` is shorter (Postgres TRANSLATE).
    conn.create_scalar_function("ibis_translate", 3, DET, |ctx| {
        Ok(match (text(ctx, 0)?, text(ctx, 1)?, text(ctx, 2)?) {
            (Some(s), Some(from), Some(to)) => Some(translate(&s, &from, &to)),
            _ => None,
        })
    })?;
    conn.create_scalar_function("ibis_levenshtein", 2, DET, |ctx| {
        Ok(match (text(ctx, 0)?, text(ctx, 1)?) {
            (Some(a), Some(b)) => Some(levenshtein(&a, &b)),
            _ => None,
        })
    })?;
    // SQLite formats moments (`strftime`) and has nothing to read them
    // back: text by a C `strptime` pattern, as the text of a timestamp;
    // NULL where it does not match.
    conn.create_scalar_function("ibis_strptime", 2, DET, |ctx| {
        Ok(match (text(ctx, 0)?, text(ctx, 1)?) {
            (Some(s), Some(format)) => {
                ibis_core::convert::strptime(&s, &format).map(ibis_core::convert::format_timestamp)
            }
            _ => None,
        })
    })?;
    // Math functions: only present when SQLite is built with
    // SQLITE_ENABLE_MATH_FUNCTIONS, which the bundled build is not.
    let unary: [(&str, UnaryF64); 14] = [
        ("sin", f64::sin),
        ("cos", f64::cos),
        ("tan", f64::tan),
        ("asin", f64::asin),
        ("acos", f64::acos),
        ("atan", f64::atan),
        ("cot", |x| 1.0 / x.tan()),
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
    let binary: [(&str, BinaryF64); 4] =
        [("power", f64::powf), ("pow", f64::powf), ("fmod", |a, b| a % b), ("atan2", f64::atan2)];
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
    for (name, op) in [("ibis_bit_and", Bits::And), ("ibis_bit_or", Bits::Or), ("ibis_bit_xor", Bits::Xor)] {
        conn.create_aggregate_function(name, 1, DET, op)?;
    }
    conn.create_aggregate_function("ibis_first", 1, DET, End { last: false, nulls: false })?;
    conn.create_aggregate_function("ibis_last", 1, DET, End { last: true, nulls: false })?;
    conn.create_aggregate_function("ibis_first_null", 1, DET, End { last: false, nulls: true })?;
    conn.create_aggregate_function("ibis_last_null", 1, DET, End { last: true, nulls: true })?;
    conn.create_aggregate_function("ibis_mode", 1, DET, Mode)?;
    conn.create_aggregate_function("ibis_argmin", 2, DET, ArgEnd { largest: false })?;
    conn.create_aggregate_function("ibis_argmax", 2, DET, ArgEnd { largest: true })?;
    Ok(())
}

/// The first argument in the row where the second, the key, is smallest
/// (or largest); of equal keys the first met. NULL keys do not count.
struct ArgEnd {
    largest: bool,
}

/// How SQLite orders two values that are not NULL: numbers, then text, then blobs.
fn compare(a: &SqlValue, b: &SqlValue) -> std::cmp::Ordering {
    use SqlValue::*;
    use std::cmp::Ordering::*;
    match (a, b) {
        (Integer(x), Integer(y)) => x.cmp(y),
        (Integer(x), Real(y)) => (*x as f64).total_cmp(y),
        (Real(x), Integer(y)) => x.total_cmp(&(*y as f64)),
        (Real(x), Real(y)) => x.total_cmp(y),
        (Text(x), Text(y)) => x.cmp(y),
        (Blob(x), Blob(y)) => x.cmp(y),
        (Integer(_) | Real(_), _) | (Text(_), Blob(_)) => Less,
        _ => Greater,
    }
}

type Best = Option<(SqlValue, SqlValue)>;

impl Aggregate<Best, Option<SqlValue>> for ArgEnd {
    fn init(&self, _: &mut Context<'_>) -> Result<Best> {
        Ok(None)
    }

    fn step(&self, ctx: &mut Context<'_>, best: &mut Best) -> Result<()> {
        let (arg, key): (SqlValue, SqlValue) = (ctx.get(0)?, ctx.get(1)?);
        if key == SqlValue::Null {
            return Ok(());
        }
        let wanted = if self.largest { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less };
        if best.as_ref().is_none_or(|(_, seen)| compare(&key, seen) == wanted) {
            *best = Some((arg, key));
        }
        Ok(())
    }

    fn finalize(&self, _: &mut Context<'_>, best: Option<Best>) -> Result<Option<SqlValue>> {
        Ok(best.flatten().map(|(arg, _)| arg))
    }
}

/// Bitwise AND, OR or XOR of the integers of a group; NULL for none.
#[derive(Clone, Copy)]
enum Bits {
    And,
    Or,
    Xor,
}

impl Aggregate<Option<i64>, Option<i64>> for Bits {
    fn init(&self, _: &mut Context<'_>) -> Result<Option<i64>> {
        Ok(None)
    }

    fn step(&self, ctx: &mut Context<'_>, acc: &mut Option<i64>) -> Result<()> {
        if let Some(x) = ctx.get::<Option<i64>>(0)? {
            *acc = Some(match (*acc, self) {
                (None, _) => x,
                (Some(a), Bits::And) => a & x,
                (Some(a), Bits::Or) => a | x,
                (Some(a), Bits::Xor) => a ^ x,
            });
        }
        Ok(())
    }

    fn finalize(&self, _: &mut Context<'_>, acc: Option<Option<i64>>) -> Result<Option<i64>> {
        Ok(acc.flatten())
    }
}

/// The first or the last value of a group: of those that are not NULL,
/// or of all of them (`nulls`).
struct End {
    last: bool,
    nulls: bool,
}

impl Aggregate<Option<SqlValue>, Option<SqlValue>> for End {
    fn init(&self, _: &mut Context<'_>) -> Result<Option<SqlValue>> {
        Ok(None)
    }

    fn step(&self, ctx: &mut Context<'_>, acc: &mut Option<SqlValue>) -> Result<()> {
        let x: SqlValue = ctx.get(0)?;
        if (self.nulls || x != SqlValue::Null) && (self.last || acc.is_none()) {
            *acc = Some(x);
        }
        Ok(())
    }

    fn finalize(&self, _: &mut Context<'_>, acc: Option<Option<SqlValue>>) -> Result<Option<SqlValue>> {
        Ok(acc.flatten())
    }
}

/// The most frequent value of a group that is not NULL; of several, the
/// one met first.
struct Mode;

impl Aggregate<Vec<(SqlValue, usize)>, Option<SqlValue>> for Mode {
    fn init(&self, _: &mut Context<'_>) -> Result<Vec<(SqlValue, usize)>> {
        Ok(vec![])
    }

    fn step(&self, ctx: &mut Context<'_>, acc: &mut Vec<(SqlValue, usize)>) -> Result<()> {
        let x: SqlValue = ctx.get(0)?;
        if x == SqlValue::Null {
            return Ok(());
        }
        match acc.iter_mut().find(|(seen, _)| *seen == x) {
            Some(entry) => entry.1 += 1,
            None => acc.push((x, 1)),
        }
        Ok(())
    }

    fn finalize(&self, _: &mut Context<'_>, acc: Option<Vec<(SqlValue, usize)>>) -> Result<Option<SqlValue>> {
        // `max_by_key` keeps the last of equals: read from the end to keep the first
        Ok(acc.unwrap_or_default().into_iter().rev().max_by_key(|(_, n)| *n).map(|(value, _)| value))
    }
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
