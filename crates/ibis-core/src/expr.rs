//! Scalar/column expressions: the `Expr` half of the IR, the builder API on
//! top of it, and type inference.
//!
//! Every op documents the meaning it has *in Ibis*; compilers must produce
//! exactly that, whatever their engine does natively.

use std::fmt;
use std::ops;
use std::sync::Arc;

use crate::datatypes::{DataType, Schema};
use crate::error::{Result, invalid};
use crate::table::Rel;
use crate::value::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// A column of the input relation, by name (Ibis `_.name`).
    Column(String),
    /// A column of a specific relation (Ibis `t.name`). Table operations
    /// rewrite these to `Column`s of their input when the relation is the input
    /// or one of its ancestors; what's left refers to an enclosing query
    /// (a correlated reference).
    Field {
        rel: Arc<Rel>,
        name: String,
    },
    /// A one-row, one-column relation used as a value.
    ScalarSubquery(Arc<Rel>),
    /// `EXISTS (rel)`
    Exists(Arc<Rel>),
    /// `arg IN (rel)`; `rel` has one column.
    InSubquery {
        arg: Box<Expr>,
        rel: Arc<Rel>,
    },
    /// `arg` is a member of `column`'s values (Ibis `x.isin(t.col)`), before
    /// a table operation turns it into `InSubquery`. `column` is an
    /// expression over one table: the one its bound columns name, else the
    /// table the expression is applied to.
    InColumn {
        arg: Box<Expr>,
        column: Box<Expr>,
    },
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
    /// Exactly one side true.
    Xor,
}

impl BinaryOp {
    pub fn is_comparison(self) -> bool {
        use BinaryOp::*;
        matches!(self, Eq | NotEq | Lt | Le | Gt | Ge)
    }
    pub fn is_logical(self) -> bool {
        matches!(self, BinaryOp::And | BinaryOp::Or | BinaryOp::Xor)
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
    /// True for NaN; NULL for NULL.
    IsNan,
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
    /// `ntile(buckets)`, **0-based**.
    Ntile,
    /// `nth(x, n)`, `n` **0-based**.
    NthValue,
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

/// A date literal from `YYYY-MM-DD` (Ibis `ibis.date("...")`).
/// An unparseable string becomes a NULL date.
pub fn date(s: &str) -> Expr {
    match crate::convert::parse_date(s) {
        Some(d) => Expr::Literal(Value::Date(d), DataType::Date),
        None => null(DataType::Date),
    }
}

/// A literal of an explicit type (Ibis `ibis.literal(v, type=...)`).
pub fn lit_as(v: impl Into<Value>, dtype: DataType) -> Expr {
    Expr::Literal(v.into(), dtype)
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

impl SortKey {
    pub fn nulls_first(mut self) -> SortKey {
        self.nulls_first = true;
        self
    }
    pub fn nulls_last(mut self) -> SortKey {
        self.nulls_first = false;
        self
    }
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
    /// Membership in a list of values. An empty list is `false` (Ibis), not
    /// the SQL syntax error `IN ()`.
    pub fn isin(self, list: impl IntoIterator<Item = impl Into<Expr>>) -> Expr {
        let list: Vec<Expr> = list.into_iter().map(Into::into).collect();
        if list.is_empty() {
            return lit(false);
        }
        Expr::InList { arg: Box::new(self), list, negated: false }
    }
    pub fn notin(self, list: impl IntoIterator<Item = impl Into<Expr>>) -> Expr {
        let list: Vec<Expr> = list.into_iter().map(Into::into).collect();
        if list.is_empty() {
            return lit(true);
        }
        Expr::InList { arg: Box::new(self), list, negated: true }
    }
    /// Membership in the values of a column (Ibis `x.isin(t.col)`):
    /// `self IN (SELECT column FROM ...)`. `column` is an expression over one
    /// table; unbound columns mean the table this expression is applied to.
    pub fn isin_col(self, column: Expr) -> Expr {
        Expr::InColumn { arg: Box::new(self), column: Box::new(column) }
    }
    pub fn notin_col(self, column: Expr) -> Expr {
        !self.isin_col(column)
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
    pub fn isnan(self) -> Expr {
        func(Func::IsNan, vec![self])
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
    /// No row is true (Ibis `notany`).
    pub fn notany(self) -> Expr {
        !self.any()
    }
    /// Not every row is true (Ibis `notall`).
    pub fn notall(self) -> Expr {
        !self.all()
    }
    /// Ascending sort key (NULLs last).
    pub fn asc(self) -> SortKey {
        asc(self)
    }
    /// Descending sort key (NULLs last).
    pub fn desc(self) -> SortKey {
        desc(self)
    }
    /// Restrict a reduction to rows matching `cond` (Ibis `where=`).
    pub fn filter_where(self, cond: Expr) -> Expr {
        match self {
            Expr::Agg { func, arg, .. } => Expr::Agg { func, arg, filter: Some(Box::new(cond)) },
            other => other,
        }
    }

    // analytics
    //
    // Ranking functions order by their argument; `.over(w)` puts the window's
    // own ordering ahead of it, as Ibis does.
    fn ranked(self, a: Analytic, args: Vec<Expr>) -> Expr {
        Expr::Window(Box::new(Expr::Analytic(a, args)), WindowSpec::new().order_by([asc(self)]))
    }
    pub fn rank(self) -> Expr {
        self.ranked(Analytic::Rank, vec![])
    }
    pub fn dense_rank(self) -> Expr {
        self.ranked(Analytic::DenseRank, vec![])
    }
    /// `rank / (rows - 1)`, in [0, 1].
    pub fn percent_rank(self) -> Expr {
        self.ranked(Analytic::PercentRank, vec![])
    }
    pub fn cume_dist(self) -> Expr {
        self.ranked(Analytic::CumeDist, vec![])
    }
    /// Bucket number, **0-based**, when the rows are split into `buckets`
    /// groups of near-equal size.
    pub fn ntile(self, buckets: i64) -> Expr {
        self.ranked(Analytic::Ntile, vec![lit(buckets)])
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
    /// The `n`th value of the window frame, **0-based**; NULL if the frame is shorter.
    pub fn nth(self, n: i64) -> Expr {
        Expr::Analytic(Analytic::NthValue, vec![self, lit(n)])
    }

    /// Evaluate over a window. Applies to every reduction and analytic
    /// function in the expression, so `(x - x.mean()).over(w)` and
    /// `x.any().notany().over(w)` work. An already windowed function keeps
    /// its frame unless `w` has one, and its ordering after `w`'s.
    pub fn over(self, w: WindowSpec) -> Expr {
        fn windowable(e: &Expr) -> bool {
            matches!(e, Expr::Agg { .. } | Expr::Analytic(..) | Expr::Window(..))
                || e.children().iter().any(|c| windowable(c))
        }
        fn go(e: Expr, w: &WindowSpec) -> Expr {
            match e {
                Expr::Window(f, mut spec) => {
                    if !w.partition_by.is_empty() {
                        spec.partition_by = w.partition_by.clone();
                    }
                    let mut order = w.order_by.clone();
                    for k in spec.order_by {
                        if !order.iter().any(|o| o.expr == k.expr) {
                            order.push(k);
                        }
                    }
                    spec.order_by = order;
                    spec.frame = w.frame.clone().or(spec.frame);
                    Expr::Window(f, spec)
                }
                e @ (Expr::Agg { .. } | Expr::Analytic(..)) => Expr::Window(Box::new(e), w.clone()),
                e if windowable(&e) => e.map_children(&mut |c| Ok(go(c, w))).expect("infallible"),
                e => e,
            }
        }
        if windowable(&self) {
            go(self, &w)
        } else {
            // not a window function: reported when the expression is used
            Expr::Window(Box::new(self), w)
        }
    }

    fn cumulative(self, order_by: impl IntoIterator<Item = impl Into<SortKey>>) -> Expr {
        self.over(WindowSpec::new().order_by(order_by).rows(None, Some(0)))
    }
    /// Running sum in `order_by` order (Ibis `cumsum`); the ordering can
    /// also come from a later `.over(window)`.
    pub fn cumsum(self, order_by: impl IntoIterator<Item = impl Into<SortKey>>) -> Expr {
        self.sum().cumulative(order_by)
    }
    pub fn cummean(self, order_by: impl IntoIterator<Item = impl Into<SortKey>>) -> Expr {
        self.mean().cumulative(order_by)
    }
    pub fn cummin(self, order_by: impl IntoIterator<Item = impl Into<SortKey>>) -> Expr {
        self.min().cumulative(order_by)
    }
    pub fn cummax(self, order_by: impl IntoIterator<Item = impl Into<SortKey>>) -> Expr {
        self.max().cumulative(order_by)
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
// Ibis spells boolean AND/OR as `&` / `|`.
op_impl!(BitAnd, bitand, BinaryOp::And);
op_impl!(BitOr, bitor, BinaryOp::Or);
op_impl!(BitXor, bitxor, BinaryOp::Xor);

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
            Expr::Column(n) | Expr::Alias(_, n) | Expr::Field { name: n, .. } => n.clone(),
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
            Expr::Column(_) | Expr::Literal(..) | Expr::Field { .. } | Expr::ScalarSubquery(_) | Expr::Exists(_) => {
                vec![]
            }
            Expr::InSubquery { arg, .. } | Expr::InColumn { arg, .. } => vec![arg],
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

    /// Rebuild this node with `f` applied to each direct child (the same
    /// children `children()` lists, in the same order).
    pub fn map_children(self, f: &mut impl FnMut(Expr) -> Result<Expr>) -> Result<Expr> {
        Ok(match self {
            e @ (Expr::Column(_)
            | Expr::Literal(..)
            | Expr::Field { .. }
            | Expr::ScalarSubquery(_)
            | Expr::Exists(_)) => e,
            Expr::InSubquery { arg, rel } => Expr::InSubquery { arg: Box::new(f(*arg)?), rel },
            Expr::InColumn { arg, column } => Expr::InColumn { arg: Box::new(f(*arg)?), column },
            Expr::Alias(e, n) => Expr::Alias(Box::new(f(*e)?), n),
            Expr::Cast(e, t) => Expr::Cast(Box::new(f(*e)?), t),
            Expr::Unary(op, e) => Expr::Unary(op, Box::new(f(*e)?)),
            Expr::Binary(op, l, r) => Expr::Binary(op, Box::new(f(*l)?), Box::new(f(*r)?)),
            Expr::Func(func, a) => Expr::Func(func, a.into_iter().map(&mut *f).collect::<Result<_>>()?),
            Expr::Analytic(a, args) => Expr::Analytic(a, args.into_iter().map(&mut *f).collect::<Result<_>>()?),
            Expr::Agg { func, arg, filter } => Expr::Agg {
                func,
                arg: arg.map(|x| f(*x).map(Box::new)).transpose()?,
                filter: filter.map(|x| f(*x).map(Box::new)).transpose()?,
            },
            Expr::Window(e, spec) => {
                let e = Box::new(f(*e)?);
                let partition_by = spec.partition_by.into_iter().map(&mut *f).collect::<Result<_>>()?;
                let order_by =
                    spec.order_by.into_iter().map(|k| Ok(SortKey { expr: f(k.expr)?, ..k })).collect::<Result<_>>()?;
                Expr::Window(e, WindowSpec { partition_by, order_by, frame: spec.frame })
            }
            Expr::Case { branches, default } => Expr::Case {
                branches: branches.into_iter().map(|(c, r)| Ok((f(c)?, f(r)?))).collect::<Result<_>>()?,
                default: default.map(|x| f(*x).map(Box::new)).transpose()?,
            },
            Expr::InList { arg, list, negated } => Expr::InList {
                arg: Box::new(f(*arg)?),
                list: list.into_iter().map(&mut *f).collect::<Result<_>>()?,
                negated,
            },
        })
    }

    /// Distinct relations referenced by `Field`s in this expression (not
    /// looking inside subqueries), in first-seen order.
    pub fn field_rels(&self) -> Vec<Arc<Rel>> {
        fn go(e: &Expr, out: &mut Vec<Arc<Rel>>) {
            if let Expr::Field { rel, .. } = e
                && !out.iter().any(|r| r.id == rel.id)
            {
                out.push(rel.clone());
            }
            for c in e.children() {
                go(c, out);
            }
        }
        let mut out = vec![];
        go(self, &mut out);
        out
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
            Expr::Field { rel, name } => match rel.schema.field(name) {
                Some(f) => f.dtype.clone(),
                None => {
                    let have: Vec<_> = rel.schema.names().collect();
                    return invalid(format!("no column {name:?} in table; have {have:?}"));
                }
            },
            Expr::ScalarSubquery(rel) => match rel.schema.fields.as_slice() {
                [f] => f.dtype.clone(),
                _ => return invalid(format!("scalar subquery must have one column, has {}", rel.schema.len())),
            },
            Expr::Exists(_) => T::Boolean,
            Expr::InColumn { arg, .. } => {
                arg.dtype(schema)?;
                T::Boolean
            }
            Expr::InSubquery { arg, rel } => {
                arg.dtype(schema)?;
                if rel.schema.len() != 1 {
                    return invalid(format!("IN subquery must have one column, has {}", rel.schema.len()));
                }
                T::Boolean
            }
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
                    FloorDiv => T::Int64,
                    Add | Sub | Mul | Mod if lt.is_integer() && rt.is_integer() => integral_binop(*op, l, &lt, r, &rt),
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
                    (Sum, Some(T::Decimal { scale, .. })) => T::Decimal { precision: 38, scale },
                    (Sum, Some(t)) => t,
                    // Ibis: the mean of a decimal is that decimal type
                    (Mean, Some(t @ T::Decimal { .. })) => t,
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
                    RowNumber | Rank | DenseRank | Ntile => T::Int64,
                    PercentRank | CumeDist => T::Float64,
                    Lag | Lead | FirstValue | LastValue | NthValue => match ts.first() {
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

/// Ibis's result type for integer arithmetic: wide enough for every value
/// the operands could produce. A literal contributes its value, anything else
/// the bounds of its type; the result is at least as wide as both operands.
/// So `int16_col + 1` is int32 (32767 + 1 overflows int16) but `int16_col % 2`
/// stays int16.
fn integral_binop(op: BinaryOp, l: &Expr, lt: &DataType, r: &Expr, rt: &DataType) -> DataType {
    fn bounds(e: &Expr, t: &DataType) -> Vec<i128> {
        match e.unaliased() {
            Expr::Literal(Value::Int(i), _) => vec![*i as i128],
            _ => t.int_bounds().map(|(lo, hi)| vec![lo, hi]).unwrap_or_default(),
        }
    }
    let width = |t: &DataType| t.int_bounds().map_or(0, |(_, hi)| hi);
    let mut out = if width(lt) >= width(rt) { lt.clone() } else { rt.clone() };
    for a in bounds(l, lt) {
        for b in bounds(r, rt) {
            let v = match op {
                BinaryOp::Add => a.checked_add(b),
                BinaryOp::Sub => a.checked_sub(b),
                BinaryOp::Mul => a.checked_mul(b),
                // Python's %: the result takes the divisor's sign
                BinaryOp::Mod if b != 0 => Some(a.rem_euclid(b) + if b < 0 && a.rem_euclid(b) != 0 { b } else { 0 }),
                _ => None,
            };
            let t = v.map_or(DataType::Int64, DataType::smallest_int);
            if width(&t) > width(&out) {
                out = t;
            }
        }
    }
    out
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
        StartsWith | EndsWith | Contains | Like | ILike | RegexSearch | IsNan => T::Boolean,
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
            Expr::Column(n) | Expr::Field { name: n, .. } => f.write_str(n),
            Expr::ScalarSubquery(_) => f.write_str("ScalarSubquery"),
            Expr::Exists(_) => f.write_str("ExistsSubquery"),
            Expr::InSubquery { arg, .. } | Expr::InColumn { arg, .. } => write!(f, "InSubquery({arg})"),
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
