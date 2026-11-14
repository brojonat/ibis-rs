//! Scalar/column expressions: the `Expr` half of the IR, the builder API on
//! top of it, and type inference.
//!
//! Every op documents the meaning it has *in Ibis*; compilers must produce
//! exactly that, whatever their engine does natively.

use std::fmt;
use std::ops;

use crate::datatypes::{DataType, Schema};
use crate::error::{Result, invalid};
use crate::value::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// A column of the input relation, by name.
    Column(String),
    Literal(Value, DataType),
    /// Names an expression; only meaningful at the top of a projection.
    Alias(Box<Expr>, String),
    Cast(Box<Expr>, DataType),
    Unary(UnaryOp, Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    /// Scalar function. Non-column parameters (pattern, length, ...) are
    /// literal arguments so every op has one uniform shape.
    Func(Func, Vec<Expr>),
    /// Reduction. Inside a projection with no window, it is broadcast over the
    /// whole relation (`agg OVER ()`), as Ibis does.
    Agg {
        func: AggFunc,
        arg: Option<Box<Expr>>,
        filter: Option<Box<Expr>>,
    },
    /// Analytic-only function (row_number, rank, lag, ...). Must be windowed.
    Analytic(Analytic, Vec<Expr>),
    Window(Box<Expr>, WindowSpec),
    /// `CASE WHEN c1 THEN r1 ... ELSE default END`
    Case {
        branches: Vec<(Expr, Expr)>,
        default: Option<Box<Expr>>,
    },
    InList {
        arg: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Negate,
    IsNull,
    NotNull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    /// Numeric addition, or string concatenation when both sides are strings.
    Add,
    Sub,
    /// Numeric multiply, or string repeat when the left side is a string.
    Mul,
    /// True division: always floating point (`1 / 2 == 0.5`).
    Div,
    /// Floor division (rounds toward negative infinity); integer result for
    /// integer inputs.
    FloorDiv,
    /// Remainder with the sign of the dividend (SQL semantics, as Ibis on DuckDB).
    Mod,
    Pow,
    Eq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinaryOp {
    pub fn is_comparison(self) -> bool {
        use BinaryOp::*;
        matches!(self, Eq | NotEq | Lt | Le | Gt | Ge)
    }
    pub fn is_logical(self) -> bool {
        matches!(self, BinaryOp::And | BinaryOp::Or)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Func {
    // numeric
    Abs,
    /// Ceiling; int64 for floating input.
    Ceil,
    /// Floor; int64 for floating input.
    Floor,
    /// `round(x)` -> int64 (half to even, like Ibis/numpy); `round(x, d)` -> float64.
    Round,
    Sign,
    Sqrt,
    Exp,
    Ln,
    Log10,
    Log2,
    // strings
    Upper,
    Lower,
    /// Number of characters, int32.
    Length,
    /// Strip whitespace from both ends.
    Strip,
    LStrip,
    RStrip,
    Reverse,
    /// `repeat(s, n)`
    Repeat,
    /// `substr(s, start, [length])` with **0-based** start.
    Substring,
    /// Last `n` characters.
    Right,
    StartsWith,
    EndsWith,
    /// Substring containment (not a pattern).
    Contains,
    /// 0-based index of the first occurrence, or -1. Optional 0-based start.
    Find,
    /// Plain (non-regex) replace of all occurrences.
    Replace,
    /// SQL LIKE pattern (case-sensitive everywhere).
    Like,
    ILike,
    /// True if the regex matches anywhere in the string.
    RegexSearch,
    /// Group `i` of the first match (0 = whole match); NULL... see Ibis.
    RegexExtract,
    /// Replace all regex matches.
    RegexReplace,
    LPad,
    RPad,
    Capitalize,
    /// Code point of the first character, int32.
    Ascii,
    // nulls
    Coalesce,
    NullIf,
    // temporal: extract fields as int32
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    /// Monday = 0.
    DayOfWeekIndex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggFunc {
    /// COUNT(*) over the relation (no argument).
    CountStar,
    /// Non-null count.
    Count,
    CountDistinct,
    Sum,
    Mean,
    Min,
    Max,
    StdSamp,
    StdPop,
    VarSamp,
    VarPop,
    Any,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Analytic {
    /// **0-based**, like Ibis.
    RowNumber,
    /// `min_rank`, **0-based**.
    Rank,
    /// **0-based**.
    DenseRank,
    PercentRank,
    CumeDist,
    /// `lag(x, [offset], [default])`
    Lag,
    Lead,
    FirstValue,
    LastValue,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct WindowSpec {
    pub partition_by: Vec<Expr>,
    pub order_by: Vec<SortKey>,
    pub frame: Option<Frame>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub rows: bool,
    pub start: Bound,
    pub end: Bound,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Bound {
    Unbounded,
    Preceding(u64),
    CurrentRow,
    Following(u64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SortKey {
    pub expr: Expr,
    pub ascending: bool,
    /// Ibis default: NULLs last in both directions.
    pub nulls_first: bool,
}

// ---------------------------------------------------------------------------
// constructors

/// Reference a column of the table the expression is applied to (Ibis `_.name`).
pub fn col(name: &str) -> Expr {
    Expr::Column(name.to_string())
}

pub fn lit(v: impl Into<Value>) -> Expr {
    let v = v.into();
    let t = v.dtype();
    Expr::Literal(v, t)
}

/// A typed NULL literal.
pub fn null(dtype: DataType) -> Expr {
    Expr::Literal(Value::Null, dtype)
}

/// Number of rows (Ibis `t.count()`).
pub fn count() -> Expr {
    Expr::Agg { func: AggFunc::CountStar, arg: None, filter: None }
}

/// 0-based row number; use with `.over(...)`.
pub fn row_number() -> Expr {
    Expr::Analytic(Analytic::RowNumber, vec![])
}

pub fn coalesce(args: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::Func(Func::Coalesce, args.into_iter().collect())
}

pub fn asc(e: impl Into<Expr>) -> SortKey {
    SortKey { expr: e.into(), ascending: true, nulls_first: false }
}

pub fn desc(e: impl Into<Expr>) -> SortKey {
    SortKey { expr: e.into(), ascending: false, nulls_first: false }
}

impl From<Expr> for SortKey {
    fn from(e: Expr) -> Self {
        asc(e)
    }
}
impl From<&str> for SortKey {
    fn from(s: &str) -> Self {
        asc(col(s))
    }
}

/// `CASE` builder (Ibis `ibis.cases((cond, result), ..., else_=...)`).
pub fn cases(branches: impl IntoIterator<Item = (Expr, Expr)>, default: Option<Expr>) -> Expr {
    Expr::Case { branches: branches.into_iter().collect(), default: default.map(Box::new) }
}

impl WindowSpec {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn partition_by(mut self, keys: impl IntoIterator<Item = impl Into<Expr>>) -> Self {
        self.partition_by = keys.into_iter().map(Into::into).collect();
        self
    }
    pub fn order_by(mut self, keys: impl IntoIterator<Item = impl Into<SortKey>>) -> Self {
        self.order_by = keys.into_iter().map(Into::into).collect();
        self
    }
    /// ROWS frame: `preceding`/`following` of `None` mean unbounded.
    pub fn rows(mut self, preceding: Option<u64>, following: Option<u64>) -> Self {
        self.frame = Some(Frame {
            rows: true,
            start: preceding.map_or(Bound::Unbounded, |n| if n == 0 { Bound::CurrentRow } else { Bound::Preceding(n) }),
            end: following.map_or(Bound::Unbounded, |n| if n == 0 { Bound::CurrentRow } else { Bound::Following(n) }),
        });
        self
    }
}

// Conversions so APIs can take `"col"`, `3`, `2.5`, `true` directly.
// Note: a `&str` is a **column name**; string literals need `lit("...")`.
impl From<&str> for Expr {
    fn from(s: &str) -> Self {
        col(s)
    }
}
impl From<&String> for Expr {
    fn from(s: &String) -> Self {
        col(s)
    }
}
macro_rules! from_lit {
    ($($t:ty),*) => {$(
        impl From<$t> for Expr {
            fn from(v: $t) -> Self { lit(v) }
        }
    )*};
}
from_lit!(i32, i64, f64, bool);

// ---------------------------------------------------------------------------
// builder methods (flat, Ibis-style)

fn bin(op: BinaryOp, l: Expr, r: impl Into<Expr>) -> Expr {
    Expr::Binary(op, Box::new(l), Box::new(r.into()))
}
fn func(f: Func, args: Vec<Expr>) -> Expr {
    Expr::Func(f, args)
}
fn agg(f: AggFunc, arg: Expr) -> Expr {
    Expr::Agg { func: f, arg: Some(Box::new(arg)), filter: None }
}

impl Expr {
    /// Give the expression an output name (Ibis `.name()`).
    pub fn name(self, n: &str) -> Expr {
        match self {
            Expr::Alias(e, _) => Expr::Alias(e, n.to_string()),
            e => Expr::Alias(Box::new(e), n.to_string()),
        }
    }
    pub fn cast(self, t: DataType) -> Expr {
        Expr::Cast(Box::new(self), t)
    }

    // comparison / logic
    pub fn eq(self, o: impl Into<Expr>) -> Expr {
        bin(BinaryOp::Eq, self, o)
    }
    pub fn ne(self, o: impl Into<Expr>) -> Expr {
        bin(BinaryOp::NotEq, self, o)
    }
    pub fn lt(self, o: impl Into<Expr>) -> Expr {
        bin(BinaryOp::Lt, self, o)
    }
    pub fn le(self, o: impl Into<Expr>) -> Expr {
        bin(BinaryOp::Le, self, o)
    }
    pub fn gt(self, o: impl Into<Expr>) -> Expr {
        bin(BinaryOp::Gt, self, o)
    }
    pub fn ge(self, o: impl Into<Expr>) -> Expr {
        bin(BinaryOp::Ge, self, o)
    }
    pub fn and(self, o: impl Into<Expr>) -> Expr {
        bin(BinaryOp::And, self, o)
    }
    pub fn or(self, o: impl Into<Expr>) -> Expr {
        bin(BinaryOp::Or, self, o)
    }
    pub fn floordiv(self, o: impl Into<Expr>) -> Expr {
        bin(BinaryOp::FloorDiv, self, o)
    }
    pub fn pow(self, o: impl Into<Expr>) -> Expr {
        bin(BinaryOp::Pow, self, o)
    }
    pub fn between(self, lo: impl Into<Expr>, hi: impl Into<Expr>) -> Expr {
        self.clone().ge(lo).and(self.le(hi))
    }
    pub fn isnull(self) -> Expr {
        Expr::Unary(UnaryOp::IsNull, Box::new(self))
    }
    pub fn notnull(self) -> Expr {
        Expr::Unary(UnaryOp::NotNull, Box::new(self))
    }
    pub fn isin(self, list: impl IntoIterator<Item = impl Into<Expr>>) -> Expr {
        Expr::InList { arg: Box::new(self), list: list.into_iter().map(Into::into).collect(), negated: false }
    }
    pub fn notin(self, list: impl IntoIterator<Item = impl Into<Expr>>) -> Expr {
        Expr::InList { arg: Box::new(self), list: list.into_iter().map(Into::into).collect(), negated: true }
    }
    /// `cond.ifelse(a, b)`
    pub fn ifelse(self, then: impl Into<Expr>, otherwise: impl Into<Expr>) -> Expr {
        cases([(self, then.into())], Some(otherwise.into()))
    }
    /// Ibis `x.cases((value, result), ..., else_=...)`: a simple CASE on equality.
    pub fn cases(self, pairs: impl IntoIterator<Item = (Expr, Expr)>, default: Option<Expr>) -> Expr {
        let branches = pairs.into_iter().map(|(v, r)| (self.clone().eq(v), r));
        cases(branches, default)
    }
    pub fn fill_null(self, v: impl Into<Expr>) -> Expr {
        coalesce([self, v.into()])
    }
    pub fn nullif(self, v: impl Into<Expr>) -> Expr {
        func(Func::NullIf, vec![self, v.into()])
    }

    // numeric
    pub fn abs(self) -> Expr {
        func(Func::Abs, vec![self])
    }
    pub fn ceil(self) -> Expr {
        func(Func::Ceil, vec![self])
    }
    pub fn floor(self) -> Expr {
        func(Func::Floor, vec![self])
    }
    pub fn round(self, digits: Option<i64>) -> Expr {
        let mut a = vec![self];
        a.extend(digits.map(lit));
        func(Func::Round, a)
    }
    pub fn sign(self) -> Expr {
        func(Func::Sign, vec![self])
    }
    pub fn sqrt(self) -> Expr {
        func(Func::Sqrt, vec![self])
    }
    pub fn exp(self) -> Expr {
        func(Func::Exp, vec![self])
    }
    pub fn ln(self) -> Expr {
        func(Func::Ln, vec![self])
    }
    pub fn log10(self) -> Expr {
        func(Func::Log10, vec![self])
    }
    pub fn log2(self) -> Expr {
        func(Func::Log2, vec![self])
    }

    // strings
    pub fn upper(self) -> Expr {
        func(Func::Upper, vec![self])
    }
    pub fn lower(self) -> Expr {
        func(Func::Lower, vec![self])
    }
    pub fn length(self) -> Expr {
        func(Func::Length, vec![self])
    }
    pub fn strip(self) -> Expr {
        func(Func::Strip, vec![self])
    }
    pub fn lstrip(self) -> Expr {
        func(Func::LStrip, vec![self])
    }
    pub fn rstrip(self) -> Expr {
        func(Func::RStrip, vec![self])
    }
    pub fn reverse(self) -> Expr {
        func(Func::Reverse, vec![self])
    }
    pub fn capitalize(self) -> Expr {
        func(Func::Capitalize, vec![self])
    }
    pub fn ascii_str(self) -> Expr {
        func(Func::Ascii, vec![self])
    }
    pub fn repeat(self, n: impl Into<Expr>) -> Expr {
        func(Func::Repeat, vec![self, n.into()])
    }
    /// 0-based `start`.
    pub fn substr(self, start: impl Into<Expr>, length: Option<Expr>) -> Expr {
        let mut a = vec![self, start.into()];
        a.extend(length);
        func(Func::Substring, a)
    }
    pub fn left(self, n: i64) -> Expr {
        self.substr(0, Some(lit(n)))
    }
    pub fn right(self, n: i64) -> Expr {
        func(Func::Right, vec![self, lit(n)])
    }
    pub fn startswith(self, s: &str) -> Expr {
        func(Func::StartsWith, vec![self, lit(s)])
    }
    pub fn endswith(self, s: &str) -> Expr {
        func(Func::EndsWith, vec![self, lit(s)])
    }
    pub fn contains(self, s: &str) -> Expr {
        func(Func::Contains, vec![self, lit(s)])
    }
    pub fn find(self, s: &str, start: Option<i64>) -> Expr {
        let mut a = vec![self, lit(s)];
        a.extend(start.map(lit));
        func(Func::Find, a)
    }
    pub fn replace(self, pat: &str, rep: &str) -> Expr {
        func(Func::Replace, vec![self, lit(pat), lit(rep)])
    }
    pub fn like(self, pat: &str) -> Expr {
        func(Func::Like, vec![self, lit(pat)])
    }
    pub fn ilike(self, pat: &str) -> Expr {
        func(Func::ILike, vec![self, lit(pat)])
    }
    pub fn re_search(self, pat: &str) -> Expr {
        func(Func::RegexSearch, vec![self, lit(pat)])
    }
    /// Alias of `re_search`.
    pub fn rlike(self, pat: &str) -> Expr {
        self.re_search(pat)
    }
    pub fn re_extract(self, pat: &str, idx: i64) -> Expr {
        func(Func::RegexExtract, vec![self, lit(pat), lit(idx)])
    }
    pub fn re_replace(self, pat: &str, rep: &str) -> Expr {
        func(Func::RegexReplace, vec![self, lit(pat), lit(rep)])
    }
    pub fn lpad(self, len: i64, pad: &str) -> Expr {
        func(Func::LPad, vec![self, lit(len), lit(pad)])
    }
    pub fn rpad(self, len: i64, pad: &str) -> Expr {
        func(Func::RPad, vec![self, lit(len), lit(pad)])
    }

    // temporal
    pub fn year(self) -> Expr {
        func(Func::Year, vec![self])
    }
    pub fn month(self) -> Expr {
        func(Func::Month, vec![self])
    }
    pub fn day(self) -> Expr {
        func(Func::Day, vec![self])
    }
    pub fn hour(self) -> Expr {
        func(Func::Hour, vec![self])
    }
    pub fn minute(self) -> Expr {
        func(Func::Minute, vec![self])
    }
    pub fn second(self) -> Expr {
        func(Func::Second, vec![self])
    }
    pub fn day_of_week_index(self) -> Expr {
        func(Func::DayOfWeekIndex, vec![self])
    }

    // reductions
    pub fn count(self) -> Expr {
        agg(AggFunc::Count, self)
    }
    pub fn nunique(self) -> Expr {
        agg(AggFunc::CountDistinct, self)
    }
    pub fn sum(self) -> Expr {
        agg(AggFunc::Sum, self)
    }
    pub fn mean(self) -> Expr {
        agg(AggFunc::Mean, self)
    }
    pub fn min(self) -> Expr {
        agg(AggFunc::Min, self)
    }
    pub fn max(self) -> Expr {
        agg(AggFunc::Max, self)
    }
    /// Sample standard deviation (Ibis default `how="sample"`).
    pub fn std(self) -> Expr {
        agg(AggFunc::StdSamp, self)
    }
    pub fn std_pop(self) -> Expr {
        agg(AggFunc::StdPop, self)
    }
    pub fn var(self) -> Expr {
        agg(AggFunc::VarSamp, self)
    }
    pub fn var_pop(self) -> Expr {
        agg(AggFunc::VarPop, self)
    }
    pub fn any(self) -> Expr {
        agg(AggFunc::Any, self)
    }
    pub fn all(self) -> Expr {
        agg(AggFunc::All, self)
    }
    /// Restrict a reduction to rows matching `cond` (Ibis `where=`).
    pub fn filter_where(self, cond: Expr) -> Expr {
        match self {
            Expr::Agg { func, arg, .. } => Expr::Agg { func, arg, filter: Some(Box::new(cond)) },
            other => other,
        }
    }

    // analytics
    pub fn rank(self) -> Expr {
        Expr::Window(Box::new(Expr::Analytic(Analytic::Rank, vec![])), WindowSpec::new().order_by([asc(self)]))
    }
    pub fn dense_rank(self) -> Expr {
        Expr::Window(Box::new(Expr::Analytic(Analytic::DenseRank, vec![])), WindowSpec::new().order_by([asc(self)]))
    }
    pub fn lag(self, offset: Option<i64>) -> Expr {
        let mut a = vec![self];
        a.extend(offset.map(lit));
        Expr::Analytic(Analytic::Lag, a)
    }
    pub fn lead(self, offset: Option<i64>) -> Expr {
        let mut a = vec![self];
        a.extend(offset.map(lit));
        Expr::Analytic(Analytic::Lead, a)
    }
    pub fn first(self) -> Expr {
        Expr::Analytic(Analytic::FirstValue, vec![self])
    }
    pub fn last(self) -> Expr {
        Expr::Analytic(Analytic::LastValue, vec![self])
    }
    pub fn over(self, w: WindowSpec) -> Expr {
        match self {
            // re-windowing an already windowed rank keeps its ORDER BY
            Expr::Window(f, mut spec) => {
                spec.partition_by = w.partition_by;
                if !w.order_by.is_empty() {
                    spec.order_by = w.order_by;
                }
                spec.frame = w.frame.or(spec.frame);
                Expr::Window(f, spec)
            }
            e => Expr::Window(Box::new(e), w),
        }
    }
    pub fn cumsum(self, order_by: impl IntoIterator<Item = impl Into<SortKey>>) -> Expr {
        self.sum().over(WindowSpec::new().order_by(order_by).rows(None, Some(0)))
    }
}

macro_rules! op_impl {
    ($tr:ident, $m:ident, $op:expr) => {
        impl<T: Into<Expr>> ops::$tr<T> for Expr {
            type Output = Expr;
            fn $m(self, rhs: T) -> Expr {
                bin($op, self, rhs)
            }
        }
    };
}
op_impl!(Add, add, BinaryOp::Add);
op_impl!(Sub, sub, BinaryOp::Sub);
op_impl!(Mul, mul, BinaryOp::Mul);
op_impl!(Div, div, BinaryOp::Div);
op_impl!(Rem, rem, BinaryOp::Mod);

impl ops::Neg for Expr {
    type Output = Expr;
    fn neg(self) -> Expr {
        Expr::Unary(UnaryOp::Negate, Box::new(self))
    }
}
impl ops::Not for Expr {
    type Output = Expr;
    fn not(self) -> Expr {
        Expr::Unary(UnaryOp::Not, Box::new(self))
    }
}

// ---------------------------------------------------------------------------
// analysis

impl Expr {
    /// The output column name for this expression.
    pub fn output_name(&self) -> String {
        match self {
            Expr::Column(n) | Expr::Alias(_, n) => n.clone(),
            e => e.to_string(),
        }
    }

    /// Strip a top-level alias.
    pub fn unaliased(&self) -> &Expr {
        match self {
            Expr::Alias(e, _) => e.unaliased(),
            e => e,
        }
    }

    /// True if this expression contains a reduction that isn't windowed.
    pub fn is_reduction(&self) -> bool {
        match self {
            Expr::Agg { .. } => true,
            Expr::Window(..) => false,
            e => e.children().iter().any(|c| c.is_reduction()),
        }
    }

    pub fn children(&self) -> Vec<&Expr> {
        match self {
            Expr::Column(_) | Expr::Literal(..) => vec![],
            Expr::Alias(e, _) | Expr::Cast(e, _) | Expr::Unary(_, e) => vec![e],
            Expr::Binary(_, l, r) => vec![l, r],
            Expr::Func(_, a) | Expr::Analytic(_, a) => a.iter().collect(),
            Expr::Agg { arg, filter, .. } => arg.iter().chain(filter.iter()).map(|b| &**b).collect(),
            Expr::Window(f, spec) => {
                let mut v: Vec<&Expr> = vec![f];
                v.extend(spec.partition_by.iter());
                v.extend(spec.order_by.iter().map(|k| &k.expr));
                v
            }
            Expr::Case { branches, default } => {
                let mut v: Vec<&Expr> = branches.iter().flat_map(|(c, r)| [c, r]).collect();
                v.extend(default.iter().map(|b| &**b));
                v
            }
            Expr::InList { arg, list, .. } => std::iter::once(&**arg).chain(list.iter()).collect(),
        }
    }

    /// Infer the output type against `schema`. This is also validation:
    /// unknown columns and ill-typed ops are errors.
    pub fn dtype(&self, schema: &Schema) -> Result<DataType> {
        use DataType as T;
        Ok(match self {
            Expr::Column(n) => match schema.field(n) {
                Some(f) => f.dtype.clone(),
                None => {
                    let have: Vec<_> = schema.names().collect();
                    return invalid(format!("no column {n:?}; have {have:?}"));
                }
            },
            Expr::Literal(_, t) => t.clone(),
            Expr::Alias(e, _) => e.dtype(schema)?,
            Expr::Cast(e, t) => {
                e.dtype(schema)?;
                t.clone()
            }
            Expr::Unary(op, e) => {
                let t = e.dtype(schema)?;
                match op {
                    UnaryOp::Not | UnaryOp::IsNull | UnaryOp::NotNull => T::Boolean,
                    UnaryOp::Negate => t,
                }
            }
            Expr::Binary(op, l, r) => {
                let (lt, rt) = (l.dtype(schema)?, r.dtype(schema)?);
                use BinaryOp::*;
                match op {
                    _ if op.is_comparison() || op.is_logical() => T::Boolean,
                    Add if lt == T::String && rt == T::String => T::String,
                    Mul if (lt == T::String && rt.is_integer()) || (lt.is_integer() && rt == T::String) => T::String,
                    Div | Pow => T::Float64,
                    FloorDiv if lt.is_integer() && rt.is_integer() => T::Int64,
                    FloorDiv => T::Float64,
                    _ => {
                        if !lt.is_numeric() && lt != T::Null || !rt.is_numeric() && rt != T::Null {
                            return invalid(format!("{op:?} needs numeric operands, got {lt} and {rt}"));
                        }
                        T::promote(&lt, &rt)?
                    }
                }
            }
            Expr::Func(f, args) => {
                let ts = args.iter().map(|a| a.dtype(schema)).collect::<Result<Vec<_>>>()?;
                func_dtype(*f, &ts)?
            }
            Expr::Agg { func, arg, filter } => {
                if let Some(f) = filter {
                    f.dtype(schema)?;
                }
                let t = arg.as_ref().map(|a| a.dtype(schema)).transpose()?;
                use AggFunc::*;
                match (func, t) {
                    (CountStar | Count | CountDistinct, _) => T::Int64,
                    (Sum, Some(t)) if t.is_integer() || t == T::Boolean => T::Int64,
                    (Sum, Some(t)) if t.is_floating() => T::Float64,
                    (Sum, Some(t)) => t,
                    (Mean | StdSamp | StdPop | VarSamp | VarPop, _) => T::Float64,
                    (Min | Max, Some(t)) => t,
                    (Any | All, _) => T::Boolean,
                    (f, None) => return invalid(format!("{f:?} needs an argument")),
                }
            }
            Expr::Analytic(a, args) => {
                let ts = args.iter().map(|a| a.dtype(schema)).collect::<Result<Vec<_>>>()?;
                use Analytic::*;
                match a {
                    RowNumber | Rank | DenseRank => T::Int64,
                    PercentRank | CumeDist => T::Float64,
                    Lag | Lead | FirstValue | LastValue => match ts.first() {
                        Some(t) => t.clone(),
                        None => return invalid(format!("{a:?} needs an argument")),
                    },
                }
            }
            Expr::Window(f, spec) => {
                for p in &spec.partition_by {
                    p.dtype(schema)?;
                }
                for k in &spec.order_by {
                    k.expr.dtype(schema)?;
                }
                match &**f {
                    Expr::Agg { .. } | Expr::Analytic(..) => f.dtype(schema)?,
                    other => return invalid(format!("can't window a non-analytic expression: {other}")),
                }
            }
            Expr::Case { branches, default } => {
                let mut out = T::Null;
                for (c, r) in branches {
                    if c.dtype(schema)? != T::Boolean {
                        return invalid("CASE condition must be boolean");
                    }
                    out = unify(&out, &r.dtype(schema)?)?;
                }
                if let Some(d) = default {
                    out = unify(&out, &d.dtype(schema)?)?;
                }
                out
            }
            Expr::InList { arg, list, .. } => {
                arg.dtype(schema)?;
                for e in list {
                    e.dtype(schema)?;
                }
                T::Boolean
            }
        })
    }
}

fn unify(a: &DataType, b: &DataType) -> Result<DataType> {
    if a == b {
        return Ok(a.clone());
    }
    match (a, b) {
        (DataType::Null, x) | (x, DataType::Null) => Ok(x.clone()),
        (x, y) if x.is_numeric() && y.is_numeric() => DataType::promote(x, y),
        _ => invalid(format!("incompatible types {a} and {b}")),
    }
}

fn func_dtype(f: Func, ts: &[DataType]) -> Result<DataType> {
    use DataType as T;
    use Func::*;
    let first = ts.first().cloned().unwrap_or(T::Null);
    Ok(match f {
        Abs | Sign => first,
        Ceil | Floor if first.is_floating() || matches!(first, T::Decimal { .. }) => T::Int64,
        Ceil | Floor => first,
        Round if ts.len() == 1 && !first.is_integer() => T::Int64,
        Round if ts.len() == 1 => first,
        Round => T::Float64,
        Sqrt | Exp | Ln | Log10 | Log2 => T::Float64,
        Upper | Lower | Strip | LStrip | RStrip | Reverse | Repeat | Substring | Right | Replace | RegexExtract
        | RegexReplace | LPad | RPad | Capitalize => T::String,
        Length | Ascii => T::Int32,
        StartsWith | EndsWith | Contains | Like | ILike | RegexSearch => T::Boolean,
        Find => T::Int64,
        Coalesce => ts.iter().try_fold(T::Null, |acc, t| unify(&acc, t))?,
        NullIf => first,
        Year | Month | Day | Hour | Minute | Second | DayOfWeekIndex => {
            if !first.is_temporal() && first != T::Null {
                return invalid(format!("{f:?} needs a date or timestamp, got {first}"));
            }
            T::Int32
        }
    })
}

// ---------------------------------------------------------------------------
// display: Ibis-like default names, e.g. `Add(int_col, 1)`

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn list(f: &mut fmt::Formatter<'_>, name: &str, args: &[&Expr]) -> fmt::Result {
            write!(f, "{name}(")?;
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{a}")?;
            }
            f.write_str(")")
        }
        match self {
            Expr::Column(n) => f.write_str(n),
            Expr::Literal(v, _) => match v {
                Value::Null => f.write_str("None"),
                Value::Bool(b) => write!(f, "{}", if *b { "True" } else { "False" }),
                Value::Int(i) => write!(f, "{i}"),
                Value::Float(x) => write!(f, "{x}"),
                Value::String(s) => write!(f, "{s:?}"),
                other => write!(f, "{other:?}"),
            },
            Expr::Alias(_, n) => f.write_str(n),
            Expr::Cast(e, t) => write!(f, "Cast({e}, {t})"),
            Expr::Unary(op, e) => list(f, &format!("{op:?}"), &[e]),
            Expr::Binary(op, l, r) => list(f, &format!("{op:?}"), &[l, r]),
            Expr::Func(func, a) => list(f, &format!("{func:?}"), &a.iter().collect::<Vec<_>>()),
            Expr::Agg { func, arg, .. } => list(f, &format!("{func:?}"), &arg.iter().map(|b| &**b).collect::<Vec<_>>()),
            Expr::Analytic(a, args) => list(f, &format!("{a:?}"), &args.iter().collect::<Vec<_>>()),
            Expr::Window(e, _) => write!(f, "{e}"),
            Expr::Case { .. } => f.write_str("SearchedCase"),
            Expr::InList { arg, .. } => write!(f, "InValues({arg})"),
        }
    }
}
