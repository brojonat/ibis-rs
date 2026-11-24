//! Expressions -> Polars expressions, with the meaning Ibis gives each op.

use ibis_core::{
    AggFunc, AggOptions, Analytic, BinaryOp, Bound, DataType, DerefMap, Expr, Func, Rel, Result, Schema, UnaryOp,
    Value, WindowSpec,
};
use polars::prelude as pl;

use crate::plan::Planner;
use crate::{Polars, pl_err, to_pl, unsupported};

/// A relation whose columns `Field` references may resolve to: its id, what
/// it carries of its ancestors, and the prefix its columns have in the frame.
pub(crate) struct Scope {
    pub id: u64,
    pub deref: DerefMap,
    pub prefix: &'static str,
}

#[derive(Clone, Copy)]
pub(crate) struct Tx<'a> {
    pub be: &'a Polars,
    /// Schema of the relation plain `Column`s refer to, for type inference.
    pub schema: &'a Schema,
    pub scopes: &'a [Scope],
    /// Prefix plain `Column`s carry in the frame (a join side's).
    pub col_prefix: &'static str,
    /// Correlated scalar subqueries already joined in: relation id -> column holding the value.
    pub subs: &'a [(u64, String)],
    /// Inside a lambda: the names of its element and index parameters.
    pub lambda: Option<(&'a str, Option<&'a str>)>,
}

fn f64_of(e: pl::Expr) -> pl::Expr {
    e.cast(pl::DataType::Float64)
}

pub(crate) fn null() -> pl::Expr {
    pl::lit(pl::NULL)
}

pub(crate) fn lit_int(e: &Expr) -> Option<i64> {
    match e {
        Expr::Literal(Value::Int(i), _) => Some(*i),
        Expr::Cast(inner, _) | Expr::Alias(inner, _) => lit_int(inner),
        _ => None,
    }
}

pub(crate) fn lit_str(e: &Expr) -> Option<&str> {
    match e {
        Expr::Literal(Value::String(s), _) => Some(s),
        Expr::Cast(inner, _) | Expr::Alias(inner, _) => lit_str(inner),
        _ => None,
    }
}

fn is_null_lit(e: &Expr) -> bool {
    matches!(e, Expr::Literal(Value::Null, _))
}

pub(crate) fn literal(v: &Value, t: &DataType) -> Result<pl::Expr> {
    let e = match v {
        Value::Null => null(),
        Value::Bool(b) => pl::lit(*b),
        Value::Int(i) => pl::lit(*i),
        Value::Float(f) => pl::lit(*f),
        Value::String(s) => pl::lit(s.as_str()),
        Value::Binary(b) => pl::lit(b.as_slice()),
        Value::Date(d) => pl::lit(*d).cast(pl::DataType::Date),
        Value::Timestamp(us) => pl::lit(*us).cast(pl::DataType::Datetime(pl::TimeUnit::Microseconds, None)),
        // one value of a one-row column
        Value::Array(_) | Value::Struct(_) | Value::Map(_) => {
            return Ok(pl::lit(crate::series("", t, std::slice::from_ref(v))?).first());
        }
    };
    Ok(if *t == DataType::Null { e } else { e.cast(to_pl(t)) })
}

/// A SQL LIKE pattern as an anchored regex.
fn like_regex(pattern: &str, case_insensitive: bool) -> String {
    let mut re = String::from(if case_insensitive { "(?is)^" } else { "(?s)^" });
    for ch in pattern.chars() {
        match ch {
            '%' => re.push_str(".*"),
            '_' => re.push('.'),
            c if "\\.+*?()|[]{}^$#&-~".contains(c) => {
                re.push('\\');
                re.push(c);
            }
            c => re.push(c),
        }
    }
    re.push('$');
    re
}

/// `\1` (the SQL engines' group reference) -> `${1}` (the regex crate's).
fn group_refs(replacement: &str) -> String {
    let mut out = String::new();
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('\\', Some(d)) if d.is_ascii_digit() => {
                out.push_str(&format!("${{{d}}}"));
                chars.next();
            }
            ('$', _) => out.push_str("$$"),
            _ => out.push(c),
        }
    }
    out
}

impl Tx<'_> {
    /// Which scope a field reference belongs to, and its column there. An
    /// exact match on the relation wins over one it was derived from.
    pub(crate) fn field(&self, rel: &Rel, name: &str) -> Option<(usize, String)> {
        if let Some(i) = self.scopes.iter().position(|s| s.id == rel.id) {
            return Some((i, name.to_string()));
        }
        self.scopes
            .iter()
            .enumerate()
            .find_map(|(i, s)| s.deref.get(&(rel.id, name.to_string())).map(|c| (i, c.clone())))
    }

    pub(crate) fn dtype(&self, e: &Expr) -> Result<DataType> {
        e.dtype(self.schema)
    }

    fn args(&self, args: &[Expr]) -> Result<Vec<pl::Expr>> {
        args.iter().map(|a| self.expr(a)).collect()
    }

    /// Run an uncorrelated subquery now; its result becomes a literal.
    fn subquery(&self, rel: &Rel) -> Result<Vec<Vec<Value>>> {
        self.be.rows(Planner { be: self.be }.rel(rel)?, &rel.schema)
    }

    pub(crate) fn expr(&self, e: &Expr) -> Result<pl::Expr> {
        Ok(match e {
            Expr::Column(n) => pl::col(format!("{}{n}", self.col_prefix)),
            Expr::Field { rel, name } => match self.field(rel, name) {
                Some((i, c)) => pl::col(format!("{}{c}", self.scopes[i].prefix)),
                None => {
                    return unsupported(format!(
                        "a reference to column `{name}` of an outer table (correlated subquery)"
                    ));
                }
            },
            Expr::ScalarSubquery(rel) if self.subs.iter().any(|(id, _)| *id == rel.id) => {
                let (_, column) = self.subs.iter().find(|(id, _)| *id == rel.id).unwrap();
                pl::col(column.as_str())
            }
            Expr::ScalarSubquery(rel) => {
                let rows = self.subquery(rel)?;
                let v = rows.first().and_then(|r| r.first()).cloned().unwrap_or(Value::Null);
                literal(&v, &rel.schema.fields[0].dtype)?
            }
            Expr::Exists(_) => return unsupported("EXISTS (correlated subquery)"),
            Expr::InSubquery { arg, rel } => {
                let t = &rel.schema.fields[0].dtype;
                let vals: Vec<Value> = self.subquery(rel)?.into_iter().map(|mut r| r.remove(0)).collect();
                let has_null = vals.iter().any(Value::is_null);
                let present: Vec<Value> = vals.into_iter().filter(|v| !v.is_null()).collect();
                let set = pl::lit(crate::series("", t, &present)?).implode(false);
                let found = self.expr(arg)?.cast(to_pl(t)).is_in(set, false);
                // SQL: x IN (..., NULL) is NULL rather than false when nothing matches
                if has_null { pl::when(found.clone()).then(found).otherwise(null()) } else { found }
            }
            Expr::InColumn { .. } => {
                return Err(ibis_core::Error::Invalid(
                    "isin_col outside a table operation (select, filter, ...)".into(),
                ));
            }
            // an interval is what a date is moved by (`shift`), not a value by itself
            Expr::Literal(_, DataType::Interval(_)) | Expr::Cast(_, DataType::Interval(_)) => {
                return unsupported("an interval outside date arithmetic");
            }
            Expr::Literal(v, t) => literal(v, t)?,
            Expr::Alias(inner, _) => self.expr(inner)?,
            Expr::Cast(inner, t) => self.expr(inner)?.cast(to_pl(t)),
            Expr::Unary(op, inner) => {
                let a = self.expr(inner)?;
                match op {
                    UnaryOp::Not => a.not(),
                    UnaryOp::Negate => -a,
                    UnaryOp::IsNull => a.is_null(),
                    UnaryOp::NotNull => a.is_not_null(),
                }
            }
            Expr::Binary(op, l, r) => {
                let (lt, rt) = (self.dtype(l)?, self.dtype(r)?);
                match (&lt, &rt) {
                    (moment, DataType::Interval(_)) if moment.is_temporal() => {
                        return self.shift(l, r, *op == BinaryOp::Sub);
                    }
                    (DataType::Interval(_), moment) if moment.is_temporal() => return self.shift(r, l, false),
                    (DataType::Interval(_), _) | (_, DataType::Interval(_)) => {
                        return unsupported("an interval outside date arithmetic");
                    }
                    _ => {}
                }
                // `+` and `*` on arrays and maps are functions
                match ibis_core::expr::nested_binary(*op, l, r, &lt, &rt) {
                    Some(call) => self.expr(&call)?,
                    None => self.binary(e, *op, l, r)?,
                }
            }
            Expr::Func(f, args) => {
                let first = args.first().map(|a| self.dtype(a)).transpose()?;
                self.func(e, f.resolve(first.as_ref()), args)?
            }
            Expr::Agg { func, arg, filter, options } => {
                self.agg(e, *func, arg.as_deref(), filter.as_deref(), options)?
            }
            Expr::Analytic(..) => return unsupported(format!("{e} without a window (use .over(...))")),
            Expr::Window(f, spec) => self.window(f, spec)?,
            Expr::Case { branches, default } => {
                let t = to_pl(&self.dtype(e)?);
                let mut acc = match default {
                    Some(d) => self.expr(d)?.cast(t.clone()),
                    None => null().cast(t.clone()),
                };
                for (cond, result) in branches.iter().rev() {
                    acc = pl::when(self.expr(cond)?).then(self.expr(result)?.cast(t.clone())).otherwise(acc);
                }
                acc
            }
            Expr::InList { arg, list, negated } => {
                let a = self.expr(arg)?;
                let mut found = pl::lit(false);
                for item in list {
                    found = found.or(a.clone().eq(self.expr(item)?));
                }
                if *negated { found.not() } else { found }
            }
            Expr::Param(name, _) => self.param(name)?,
            Expr::ArrayLambda { .. } => self.lambda(e)?,
        })
    }

    fn binary(&self, e: &Expr, op: BinaryOp, l: &Expr, r: &Expr) -> Result<pl::Expr> {
        let (a, b) = (self.expr(l)?, self.expr(r)?);
        let out = self.dtype(e)?;
        let t = to_pl(&out);
        use BinaryOp::*;
        if op == Mul && out == DataType::String {
            // Ibis: string * n repeats the string
            let (text, n) = if self.dtype(l)? == DataType::String { (l, r) } else { (r, l) };
            return self.func(
                &Expr::Func(Func::Repeat, vec![text.clone(), n.clone()]),
                Func::Repeat,
                &[text.clone(), n.clone()],
            );
        }
        Ok(match op {
            // compute in the Ibis result type: Polars would stay in the operand type and overflow
            Add | Sub | Mul if out.is_numeric() => {
                let (a, b) = (a.cast(t.clone()), b.cast(t));
                match op {
                    Add => a + b,
                    Sub => a - b,
                    _ => a * b,
                }
            }
            Add => a + b,
            Sub => a - b,
            Mul => a * b,
            // IEEE: x/0 is +-inf, 0/0 is NaN
            Div => (f64_of(a) / f64_of(b)).cast(t),
            FloorDiv => (f64_of(a) / f64_of(b)).floor().cast(t),
            Mod if out.is_integer() => {
                // Polars takes the divisor's sign (like Python); Ibis's engines take the dividend's
                let (a, b) = (a.cast(t.clone()), b.cast(t));
                let m = a.clone() % b.clone();
                let off = m.clone().neq(pl::lit(0)).and(m.clone().lt(pl::lit(0)).neq(a.lt(pl::lit(0))));
                pl::when(off).then(m.clone() - b).otherwise(m)
            }
            Mod => {
                let (a, b) = (f64_of(a), f64_of(b));
                (a.clone() - b.clone() * (a / b).round(0, pl::RoundMode::ToZero)).cast(t)
            }
            Pow => f64_of(a).pow(f64_of(b)).cast(t),
            Eq => a.eq(b),
            NotEq => a.neq(b),
            Lt => a.lt(b),
            Le => a.lt_eq(b),
            Gt => a.gt(b),
            Ge => a.gt_eq(b),
            And => a.and(b),
            Or => a.or(b),
            Xor => a.xor(b),
        })
    }

    fn func(&self, e: &Expr, f: Func, args: &[Expr]) -> Result<pl::Expr> {
        let a = self.args(args)?;
        let x = a.first().cloned().unwrap_or_else(null);
        let out = self.dtype(e)?;
        let t = to_pl(&out);
        let str_arg = |i: usize, what: &str| match args.get(i).and_then(lit_str) {
            Some(s) => Ok(s.to_string()),
            None => unsupported(format!("{f:?} with a non-literal {what}")),
        };
        let int_arg = |i: usize, what: &str| match args.get(i).and_then(lit_int) {
            Some(n) => Ok(n),
            None => unsupported(format!("{f:?} with a non-literal {what}")),
        };
        // null in, null out, for functions whose Polars form would fill a value in
        let keep_null = |x: &pl::Expr, v: pl::Expr| pl::when(x.clone().is_null()).then(null()).otherwise(v);
        use Func::*;
        Ok(match f {
            Abs => x.abs(),
            Ceil => x.ceil().cast(t),
            Floor => x.floor().cast(t),
            Round => {
                let digits = if args.len() > 1 { int_arg(1, "number of digits")? } else { 0 };
                if digits < 0 {
                    return unsupported("round to a negative number of digits");
                }
                x.round(digits as u32, pl::RoundMode::HalfAwayFromZero).cast(t)
            }
            Sign => x.sign().cast(t),
            Sqrt => f64_of(x).sqrt(),
            Exp => f64_of(x).exp(),
            Ln => f64_of(x).log(pl::lit(std::f64::consts::E)),
            Log10 => f64_of(x).log(pl::lit(10.0)),
            Log2 => f64_of(x).log(pl::lit(2.0)),
            Log => f64_of(x).log(f64_of(a[1].clone())),
            Radians => f64_of(x).radians(),
            Degrees => f64_of(x).degrees(),
            Sin => f64_of(x).sin(),
            Cos => f64_of(x).cos(),
            Tan => f64_of(x).tan(),
            Asin => f64_of(x).arcsin(),
            Acos => f64_of(x).arccos(),
            Atan => f64_of(x).arctan(),
            Atan2 => f64_of(x).arctan2(f64_of(a[1].clone())),
            Cot => f64_of(x).cot(),
            IsNan => f64_of(x).is_nan(),
            IsInf => f64_of(x).is_infinite(),
            Clip => {
                let mut s = x;
                if !is_null_lit(&args[1]) {
                    s = s.clip_min(a[1].clone());
                }
                if !is_null_lit(&args[2]) {
                    s = s.clip_max(a[2].clone());
                }
                s.cast(t)
            }
            Least => polars::lazy::dsl::min_horizontal(&a).map_err(pl_err)?.cast(t),
            Greatest => polars::lazy::dsl::max_horizontal(&a).map_err(pl_err)?.cast(t),
            Coalesce => pl::coalesce(&a).cast(t),
            NullIf => pl::when(x.clone().eq(a[1].clone())).then(null()).otherwise(x),

            Upper => x.str().to_uppercase(),
            Lower => x.str().to_lowercase(),
            Length => x.str().len_chars().cast(t),
            Strip => x.str().strip_chars(null()),
            LStrip => x.str().strip_chars_start(null()),
            RStrip => x.str().strip_chars_end(null()),
            Reverse => x.str().reverse(),
            Repeat => {
                let n = int_arg(1, "count")?.max(0) as usize;
                keep_null(&x, pl::concat_str(vec![x.clone(); n], "", false))
            }
            Substring => {
                if args.get(1).and_then(lit_int).is_some_and(|i| i < 0) {
                    return unsupported("substr with negative start");
                }
                x.str().slice(a[1].clone(), a.get(2).cloned().unwrap_or_else(null))
            }
            Right => x.str().tail(a[1].clone()),
            StartsWith => x.str().starts_with(a[1].clone()),
            EndsWith => x.str().ends_with(a[1].clone()),
            Contains => x.str().contains_literal(a[1].clone()),
            Find => {
                // Ibis: the position in the whole string, -1 when absent
                let (hay, start) = match a.get(2) {
                    Some(st) => (x.clone().str().slice(st.clone(), null()), st.clone().cast(pl::DataType::Int64)),
                    None => (x.clone(), pl::lit(0i64)),
                };
                let pos = hay.str().find_literal(a[1].clone()).cast(pl::DataType::Int64);
                keep_null(&x, (pos + start).fill_null(pl::lit(-1))).cast(t)
            }
            Replace => x.str().replace_all(a[1].clone(), a[2].clone(), true),
            Like => x.str().contains(pl::lit(like_regex(&str_arg(1, "pattern")?, false)), true),
            ILike => x.str().contains(pl::lit(like_regex(&str_arg(1, "pattern")?, true)), true),
            RegexSearch => x.str().contains(a[1].clone(), true),
            RegexExtract => {
                let group = int_arg(2, "group index")?.max(0) as usize;
                // the SQL engines give '' when the pattern does not match
                keep_null(&x, x.clone().str().extract(a[1].clone(), group).fill_null(pl::lit("")))
            }
            RegexReplace => x.str().replace_all(a[1].clone(), pl::lit(group_refs(&str_arg(2, "replacement")?)), false),
            LPad | RPad => {
                let fill = str_arg(2, "fill string")?;
                let mut chars = fill.chars();
                let (Some(c), None) = (chars.next(), chars.next()) else {
                    return unsupported(format!("{f:?} with a fill string that is not one character"));
                };
                let padded =
                    if f == LPad { x.str().pad_start(a[1].clone(), c) } else { x.str().pad_end(a[1].clone(), c) };
                // SQL padding also truncates to the target length
                padded.str().slice(pl::lit(0), a[1].clone())
            }
            Capitalize => keep_null(
                &x,
                pl::concat_str(
                    [
                        x.clone().str().head(pl::lit(1)).str().to_uppercase(),
                        x.clone().str().slice(pl::lit(1), null()).str().to_lowercase(),
                    ],
                    "",
                    false,
                ),
            ),
            Ascii => {
                // The code point of the first character, decoded from its UTF-8 bytes
                // (read as one hex number); 0 for the empty string.
                let first = x.clone().str().head(pl::lit(1));
                let v =
                    first.clone().str().hex_encode().str().to_integer(pl::lit(16), Some(pl::DataType::Int64), false);
                let byte = |shift: i64, bits: i64| v.clone().floor_div(pl::lit(shift)) % pl::lit(bits);
                let code = pl::when(first.clone().str().len_bytes().eq(pl::lit(1)))
                    .then(v.clone())
                    .when(first.clone().str().len_bytes().eq(pl::lit(2)))
                    .then(byte(256, 32) * pl::lit(64) + byte(1, 64))
                    .when(first.clone().str().len_bytes().eq(pl::lit(3)))
                    .then(byte(65536, 16) * pl::lit(4096) + byte(256, 64) * pl::lit(64) + byte(1, 64))
                    .when(first.str().len_bytes().eq(pl::lit(4)))
                    .then(
                        byte(16777216, 8) * pl::lit(262144)
                            + byte(65536, 64) * pl::lit(4096)
                            + byte(256, 64) * pl::lit(64)
                            + byte(1, 64),
                    )
                    .otherwise(pl::lit(0i64));
                keep_null(&x, code).cast(t)
            }

            Year => x.dt().year().cast(t),
            Month => x.dt().month().cast(t),
            Day => x.dt().day().cast(t),
            Hour => x.dt().hour().cast(t),
            Minute => x.dt().minute().cast(t),
            Second => x.dt().second().cast(t),
            Millisecond => x.dt().millisecond().cast(t),
            Microsecond => x.dt().microsecond().cast(t),
            // Polars: Monday = 1; Ibis: Monday = 0
            DayOfWeekIndex => (x.dt().weekday().cast(pl::DataType::Int32) - pl::lit(1)).cast(t),
            DayOfWeekName => x.dt().strftime("%A"),
            DayOfYear => x.dt().ordinal_day().cast(t),
            Quarter => x.dt().quarter().cast(t),
            WeekOfYear => x.dt().week().cast(t),
            Truncate => {
                let every = match args.get(1).and_then(lit_str) {
                    Some("Y") => "1y",
                    Some("Q") => "1q",
                    Some("M") => "1mo",
                    Some("W") => "1w",
                    Some("D") => "1d",
                    Some("h") => "1h",
                    Some("m") => "1m",
                    Some("s") => "1s",
                    Some("ms") => "1ms",
                    Some("us") => "1us",
                    _ => return unsupported("truncate with a non-literal or unknown unit"),
                };
                x.dt().truncate(pl::lit(every)).cast(t)
            }
            Strftime => x.dt().strftime(&str_arg(1, "format")?),
            DateFromYmd => pl::datetime(pl::DatetimeArgs::new(a[0].clone(), a[1].clone(), a[2].clone())).cast(t),
            TimestampFromYmdhms => {
                pl::datetime(pl::DatetimeArgs::new(a[0].clone(), a[1].clone(), a[2].clone()).with_hms(
                    a[3].clone(),
                    a[4].clone(),
                    a[5].clone(),
                ))
                .cast(t)
            }
            _ => self.nested(e, f, args, &a)?,
        })
    }

    fn agg(
        &self,
        e: &Expr,
        func: AggFunc,
        arg: Option<&Expr>,
        filter: Option<&Expr>,
        options: &AggOptions,
    ) -> Result<pl::Expr> {
        let t = to_pl(&self.dtype(e)?);
        let pred = filter.map(|p| self.expr(p)).transpose()?;
        // the other argument of a reduction of two, on the rows the first is read on
        let other = || -> Result<pl::Expr> {
            let y = self.expr(&options.args[0])?;
            Ok(match &pred {
                Some(p) => y.filter(p.clone()),
                None => y,
            })
        };
        let x = match arg {
            Some(a) => {
                let x = self.expr(a)?;
                // sums and means of booleans count the trues
                let x = if self.dtype(a)? == DataType::Boolean && matches!(func, AggFunc::Sum | AggFunc::Mean) {
                    x.cast(pl::DataType::Int64)
                } else {
                    x
                };
                match &pred {
                    Some(p) => x.filter(p.clone()),
                    None => x,
                }
            }
            None => null(),
        };
        // SQL aggregates of no rows are NULL; Polars sums nothing to 0
        let some = |x: &pl::Expr, v: pl::Expr| pl::when(x.clone().count().gt(pl::lit(0))).then(v).otherwise(null());
        use AggFunc::*;
        Ok(match func {
            CountStar => match pred {
                Some(p) => p.fill_null(pl::lit(false)).cast(pl::DataType::Int64).sum(),
                None => pl::len(),
            },
            Count => x.count(),
            CountDistinct => x.drop_nulls().n_unique(),
            Sum => some(&x, x.clone().cast(t.clone()).sum()),
            Mean => f64_of(x).mean(),
            Min => x.min(),
            Max => x.max(),
            StdSamp => f64_of(x).std(1),
            StdPop => f64_of(x).std(0),
            VarSamp => f64_of(x).var(1),
            VarPop => f64_of(x).var(0),
            // max/min of booleans: NULL for no rows, where any()/all() would say false/true
            Any => x.max(),
            All => x.min(),
            CountDistinctStar => {
                let row = pl::as_struct(self.schema.names().map(pl::col).collect::<Vec<_>>());
                match pred {
                    Some(p) => row.filter(p).n_unique(),
                    None => row.n_unique(),
                }
            }
            First | Last | Collect => {
                // the values that count, in the order they are read in
                let present = x.clone().is_not_null();
                let keep = |e: pl::Expr| if options.include_null { e } else { e.filter(present.clone()) };
                let mut values = keep(x.clone());
                let any = values.clone().len().gt(pl::lit(0));
                if !options.order_by.is_empty() {
                    let (keys, order) = self.agg_order(options, &pred, keep)?;
                    values = values.sort_by(keys, order);
                }
                if options.distinct {
                    values = values.unique_stable();
                }
                match func {
                    First => values.first(),
                    Last => values.last(),
                    // SQL's ARRAY_AGG of no rows is NULL
                    // Told the order does not matter, Polars drops the sort before it
                    _ => pl::when(any).then(values.implode(!options.order_by.is_empty())).otherwise(null()),
                }
            }
            Mode => x.drop_nulls().mode(true).first(),
            // Fisher's definition (0 for a normal distribution), as pandas and DuckDB compute it
            KurtosisSamp => f64_of(x).kurtosis(true, false),
            KurtosisPop => f64_of(x).kurtosis(true, true),
            // exact: an approximation that is never wrong
            ApproxCountDistinct => x.drop_nulls().n_unique(),
            BitAnd => x.bitwise_and(),
            BitOr => x.bitwise_or(),
            BitXor => x.bitwise_xor(),
            Arbitrary => x.drop_nulls().first(),
            Median | ApproxMedian => x.median(),
            ArgMin | ArgMax => {
                // rows with a NULL key do not count; of equal keys the first
                let key = other()?;
                let keyed = key.clone().is_not_null();
                let order =
                    pl::SortMultipleOptions::default().with_order_descending(func == ArgMax).with_maintain_order(true);
                x.filter(keyed.clone()).sort_by([key.filter(keyed)], order).first()
            }
            GroupConcat => {
                let Some(sep) = lit_str(&options.args[0]) else {
                    return unsupported("group_concat with a separator that is not a literal");
                };
                let mut strings = x.clone();
                if !options.order_by.is_empty() {
                    let (keys, order) = self.agg_order(options, &pred, |key| key)?;
                    strings = strings.sort_by(keys, order);
                }
                // no strings: NULL, where joining nothing gives ''
                some(&x, strings.str().join(sep, true))
            }
            Quantile | ApproxQuantile => match &options.args[0] {
                Expr::Literal(Value::Float(q), _) => x.quantile(pl::lit(*q), pl::QuantileMethod::Linear),
                _ => return unsupported("an array of quantiles"),
            },
            CovSamp | CovPop | CorrSamp | CorrPop => {
                // as in SQL: only the rows where both are present
                let y = other()?;
                let both = x.clone().is_not_null().and(y.clone().is_not_null());
                let (x, y) = (f64_of(x.filter(both.clone())), f64_of(y.filter(both)));
                match func {
                    CovSamp => pl::cov(x, y, 1),
                    CovPop => pl::cov(x, y, 0),
                    _ => pl::pearson_corr(x, y),
                }
            }
        }
        .cast(t))
    }

    /// A timestamp or a date moved by an interval, one step after the
    /// other: `offset_by` takes the count and its unit as a string.
    fn shift(&self, moment: &Expr, by: &Expr, back: bool) -> Result<pl::Expr> {
        use ibis_core::IntervalUnit::*;
        let mut moved = self.expr(moment)?;
        for step in by.shifts()? {
            let back = step.back != back;
            let unit = match step.unit {
                Year => "y",
                Quarter => "q",
                Month => "mo",
                Week => "w",
                Day => "d",
                Hour => "h",
                Minute => "m",
                Second => "s",
                Millisecond => "ms",
                Microsecond => "us",
                Nanosecond => "ns",
            };
            let by = match step.literal() {
                Some(n) => pl::lit(format!("{}{unit}", if back { -n } else { n })),
                None => {
                    let count = self.expr(&step.count)?.cast(pl::DataType::Int64);
                    let count = if back { -count } else { count };
                    pl::concat_str([count.cast(pl::DataType::String), pl::lit(unit)], "", false)
                }
            };
            moved = moved.dt().offset_by(by);
        }
        Ok(moved)
    }

    /// The keys a reduction's rows are ordered by, and how: on the rows
    /// that pass its condition, and of those the ones `keep` leaves.
    fn agg_order(
        &self,
        options: &AggOptions,
        pred: &Option<pl::Expr>,
        keep: impl Fn(pl::Expr) -> pl::Expr,
    ) -> Result<(Vec<pl::Expr>, pl::SortMultipleOptions)> {
        let mut keys = vec![];
        for k in &options.order_by {
            let key = self.expr(&k.expr)?;
            keys.push(keep(match pred {
                Some(p) => key.filter(p.clone()),
                None => key,
            }));
        }
        let order = pl::SortMultipleOptions::default()
            .with_order_descending_multi(options.order_by.iter().map(|k| !k.ascending).collect::<Vec<_>>())
            .with_nulls_last_multi(options.order_by.iter().map(|k| !k.nulls_first).collect::<Vec<_>>())
            .with_maintain_order(true);
        Ok((keys, order))
    }

    fn window(&self, f: &Expr, spec: &WindowSpec) -> Result<pl::Expr> {
        let parts: Vec<pl::Expr> =
            self.args(&spec.partition_by)?.into_iter().enumerate().map(|(i, p)| p.alias(format!("__p{i}"))).collect();
        // Polars orders a window by one direction for all keys
        let order = || -> Result<Option<(Vec<pl::Expr>, pl::SortOptions)>> {
            let Some(first) = spec.order_by.first() else { return Ok(None) };
            if spec.order_by.iter().any(|k| k.ascending != first.ascending || k.nulls_first != first.nulls_first) {
                return unsupported("a window ordered by keys with different directions");
            }
            // the same key twice adds nothing, and Polars rejects it
            // a key the window is also partitioned by is constant within a partition
            let mut seen: Vec<String> = spec.partition_by.iter().map(|p| p.to_string()).collect();
            let mut by = vec![];
            for k in &spec.order_by {
                let text = k.expr.to_string();
                if !seen.contains(&text) {
                    seen.push(text);
                    // Polars packs the keys into a struct: their names must differ
                    by.push(self.expr(&k.expr)?.alias(format!("__w{}", by.len())));
                }
            }
            if by.is_empty() {
                return Ok(None);
            }
            let opts =
                pl::SortOptions::default().with_order_descending(!first.ascending).with_nulls_last(!first.nulls_first);
            Ok(Some((by, opts)))
        };
        let over = |x: pl::Expr, order: Option<(Vec<pl::Expr>, pl::SortOptions)>| -> Result<pl::Expr> {
            if parts.is_empty() && order.is_none() {
                return Ok(x); // the whole table: a projection broadcasts it
            }
            let p = if parts.is_empty() { None } else { Some(parts.clone()) };
            x.over_with_options(p, order, Default::default()).map_err(pl_err)
        };
        // Frame bounds as row offsets from the current row; None = unbounded.
        let offset = |b: &Bound| match b {
            Bound::Unbounded => None,
            Bound::Preceding(n) => Some(-(*n as i64)),
            Bound::CurrentRow => Some(0),
            Bound::Following(n) => Some(*n as i64),
        };
        let frame = match &spec.frame {
            None => None,
            Some(fr) if fr.start == Bound::Unbounded && fr.end == Bound::Unbounded => None,
            Some(fr) if !fr.rows => return unsupported("a RANGE window frame"),
            Some(fr) => Some((offset(&fr.start), offset(&fr.end))),
        };
        let int = |e: pl::Expr| e.cast(pl::DataType::Int64);
        // Everything below is evaluated per partition, in window order.
        let idx = || pl::int_range(pl::lit(0), pl::len(), 1, pl::DataType::Int64);
        let last_idx = || int(pl::len()) - pl::lit(1);
        match f {
            Expr::Agg { func, arg, filter, options } => {
                let Some((lo, hi)) = frame else {
                    // order does not change a whole-partition aggregate
                    return over(self.agg(f, *func, arg.as_deref(), filter.as_deref(), options)?, None);
                };
                if filter.is_some() {
                    return unsupported("a framed window aggregate with a filter");
                }
                if !options.is_plain() {
                    return unsupported(format!("{func:?} over a window frame"));
                }
                let t = to_pl(&self.dtype(f)?);
                let x = match arg {
                    Some(a) if self.dtype(a)? == DataType::Boolean => int(self.expr(a)?),
                    Some(a) => self.expr(a)?,
                    None => pl::lit(1i64) + idx() * pl::lit(0i64),
                };
                // Sum of `v` (no NULLs) over the frame, as a difference of prefix sums.
                let frame_sum = |v: pl::Expr| {
                    let c = v.clone().cum_sum(false);
                    let total = v.sum();
                    let upper = match hi {
                        None => total.clone(),
                        Some(h) if h >= 0 => c.clone().shift(pl::lit(-h)).fill_null(total.clone()),
                        Some(h) => c.clone().shift(pl::lit(-h)).fill_null(pl::lit(0)),
                    };
                    let lower = match lo {
                        None => pl::lit(0),
                        Some(l) if l <= 0 => c.shift(pl::lit(1 - l)).fill_null(pl::lit(0)),
                        Some(l) => c.shift(pl::lit(1 - l)).fill_null(total),
                    };
                    upper - lower
                };
                let count = frame_sum(int(x.clone().is_not_null()));
                let have = |v: pl::Expr| pl::when(count.clone().gt(pl::lit(0))).then(v).otherwise(null());
                // Min or max over the frame: a running extreme when one end is open,
                // otherwise the extreme of the row's neighbours.
                let extreme = |max: bool| -> Result<pl::Expr> {
                    let cum = |e: pl::Expr, reverse: bool| {
                        let c = if max { e.cum_max(reverse) } else { e.cum_min(reverse) };
                        let fill = if reverse {
                            pl::FillNullStrategy::Backward(None)
                        } else {
                            pl::FillNullStrategy::Forward(None)
                        };
                        c.fill_null_with_strategy(fill)
                    };
                    let whole = if max { x.clone().max() } else { x.clone().min() };
                    Ok(match (lo, hi) {
                        (None, Some(h)) if h >= 0 => cum(x.clone(), false).shift(pl::lit(-h)).fill_null(whole),
                        (None, Some(h)) => cum(x.clone(), false).shift(pl::lit(-h)),
                        (Some(l), None) if l <= 0 => cum(x.clone(), true).shift(pl::lit(-l)).fill_null(whole),
                        (Some(l), None) => cum(x.clone(), true).shift(pl::lit(-l)),
                        (Some(l), Some(h)) if h - l <= 256 => {
                            let near: Vec<pl::Expr> = (l..=h).map(|k| x.clone().shift(pl::lit(-k))).collect();
                            if near.is_empty() {
                                null()
                            } else if max {
                                polars::lazy::dsl::max_horizontal(&near).map_err(pl_err)?
                            } else {
                                polars::lazy::dsl::min_horizontal(&near).map_err(pl_err)?
                            }
                        }
                        _ => return unsupported("min/max over a window frame wider than 256 rows"),
                    })
                };
                let e = match func {
                    AggFunc::CountStar | AggFunc::Count => count.clone(),
                    AggFunc::Sum => have(frame_sum(x.cast(t.clone()).fill_null(pl::lit(0)))),
                    AggFunc::Mean => have(frame_sum(f64_of(x).fill_null(pl::lit(0.0))) / f64_of(count.clone())),
                    AggFunc::Min | AggFunc::All => have(extreme(false)?),
                    AggFunc::Max | AggFunc::Any => have(extreme(true)?),
                    other => return unsupported(format!("{other:?} over a window frame")),
                };
                over(e.cast(t), order()?)
            }
            Expr::Analytic(a, args) => {
                let t = to_pl(&self.dtype(&Expr::Window(Box::new(f.clone()), spec.clone()))?);
                let xs = self.args(args)?;
                let keys = spec.order_by.iter().map(|k| self.expr(&k.expr)).collect::<Result<Vec<_>>>()?;
                // In window order, a row starts a new group of peers when any ordering key changes.
                let new_peer = || {
                    keys.iter()
                        .fold(idx().eq(pl::lit(0)), |acc, k| acc.or(k.clone().neq_missing(k.clone().shift(pl::lit(1)))))
                };
                // 0-based position of the first / one past the last row among a row's peers
                let first_peer = || {
                    pl::when(new_peer())
                        .then(idx())
                        .otherwise(null())
                        .fill_null_with_strategy(pl::FillNullStrategy::Forward(None))
                };
                let after_peers = || {
                    pl::when(new_peer().shift(pl::lit(-1)).fill_null(pl::lit(true)))
                        .then(idx() + pl::lit(1))
                        .otherwise(null())
                        .fill_null_with_strategy(pl::FillNullStrategy::Backward(None))
                };
                // first and last row of the frame, clamped to the partition
                let (lo, hi) = frame.unwrap_or((None, None));
                let start = || match lo {
                    None => pl::lit(0i64) * idx(),
                    Some(l) => (idx() + pl::lit(l)).clip(pl::lit(0i64), last_idx() + pl::lit(1)),
                };
                let end = || match hi {
                    None => last_idx() + pl::lit(0i64) * idx(),
                    Some(h) => (idx() + pl::lit(h)).clip(pl::lit(-1i64), last_idx()),
                };
                let at = |x: &pl::Expr, pos: pl::Expr| {
                    pl::when(pos.clone().gt_eq(start()).and(pos.clone().lt_eq(end())))
                        .then(x.clone().gather(pos, true))
                        .otherwise(null())
                };
                use Analytic::*;
                let e = match a {
                    // Ibis ranks are 0-based
                    RowNumber => idx(),
                    Rank => first_peer(),
                    DenseRank => int(new_peer()).cum_sum(false) - pl::lit(1),
                    PercentRank => f64_of(first_peer()) / f64_of(last_idx()),
                    CumeDist => f64_of(after_peers()) / f64_of(pl::len()),
                    Ntile => {
                        let Some(n) = args.first().and_then(lit_int).filter(|n| *n > 0) else {
                            return unsupported("ntile with a non-literal bucket count");
                        };
                        // the first `len % n` buckets hold one row more than the rest
                        let (len, n) = (int(pl::len()), pl::lit(n));
                        let (q, r) = (len.clone().floor_div(n.clone()), len % n);
                        let big = r.clone() * (q.clone() + pl::lit(1));
                        pl::when(idx().lt(big.clone()))
                            .then(idx().floor_div(q.clone() + pl::lit(1)))
                            .otherwise(r + (idx() - big).floor_div(q))
                    }
                    Lag | Lead => {
                        let n = match args.get(1) {
                            Some(off) if is_null_lit(off) => 1,
                            Some(off) => match lit_int(off) {
                                Some(n) => n,
                                None => return unsupported("lag/lead with a non-literal offset"),
                            },
                            None => 1,
                        };
                        let n = if *a == Lag { n } else { -n };
                        match args.get(2) {
                            Some(d) if !is_null_lit(d) => xs[0].clone().shift_and_fill(pl::lit(n), xs[2].clone()),
                            _ => xs[0].clone().shift(pl::lit(n)),
                        }
                    }
                    FirstValue => at(&xs[0], start()),
                    LastValue => at(&xs[0], end()),
                    NthValue => at(&xs[0], start() + int(xs[1].clone())),
                };
                over(e.cast(t), order()?)
            }
            other => unsupported(format!("window over {other}")),
        }
    }
}
