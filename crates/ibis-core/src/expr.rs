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
    /// `body` evaluated for each element of the array `arg`: the results
    /// (`Map`), or the elements it is true for (`Filter`). Inside `body`,
    /// `Param(param)` is the element and `Param(index)` its 0-based position.
    ArrayLambda {
        kind: Lambda,
        arg: Box<Expr>,
        param: String,
        index: Option<String>,
        body: Box<Expr>,
    },
    /// A parameter of an enclosing `ArrayLambda`, by name. The type is
    /// filled in when the lambda is typed (`Expr::lambda_body`).
    Param(String, DataType),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lambda {
    Map,
    Filter,
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
    /// 1-366.
    DayOfYear,
    /// 1-4.
    Quarter,
    /// Milliseconds within the second, 0-999.
    Millisecond,
    /// Microseconds within the second, 0-999999.
    Microsecond,
    /// ISO 8601 week number, 1-53.
    WeekOfYear,
    /// English day name, e.g. "Tuesday".
    DayOfWeekName,
    /// `truncate(x, unit)`: round down to the start of the unit. Units are
    /// Ibis's: Y Q M W (Monday) D h m s ms us.
    Truncate,
    /// `strftime(x, format)` with a literal C-style format.
    Strftime,
    /// `date_from_ymd(year, month, day)`
    DateFromYmd,
    /// `timestamp_from_ymdhms(year, month, day, hour, minute, second)`
    TimestampFromYmdhms,
    /// Smallest / largest of the arguments, row by row.
    Least,
    Greatest,
    /// `log(x, base)`
    Log,
    Radians,
    Degrees,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    /// `atan2(y, x)`
    Atan2,
    Cot,
    /// True for +/- infinity.
    IsInf,
    /// `clip(x, lower, upper)`: limit `x` to the bounds; a NULL bound is no
    /// bound, a NULL `x` stays NULL. Keeps `x`'s type.
    Clip,
    // arrays. Positions are **0-based** everywhere, as in Ibis.
    /// `array(a, b, ...)`: an array of the arguments.
    Array,
    /// Number of elements, int64 (NULL elements count).
    ArrayLength,
    /// `arr[i]`: a negative `i` counts from the end; out of range is NULL.
    ArrayIndex,
    /// The arrays one after another. A NULL array makes the result NULL.
    ArrayConcat,
    /// `repeat(arr, n)`: the array `n` times over.
    ArrayRepeat,
    /// `slice(arr, start, stop)`: Python's `arr[start:stop]`. A NULL bound
    /// is an open end; negative bounds count from the end.
    ArraySlice,
    /// Whether an element equals the value.
    ArrayContains,
    /// Position of the first element equal to the value, or -1.
    ArrayPosition,
    /// The array without the elements equal to the value.
    ArrayRemove,
    /// The distinct elements, in no particular order.
    ArrayDistinct,
    /// Ascending, NULLs last.
    ArraySort,
    /// The distinct elements of both arrays, in no particular order.
    ArrayUnion,
    /// The distinct elements the arrays share, in no particular order.
    ArrayIntersect,
    /// One level of nesting removed: `array<array<T>>` to `array<T>`.
    ArrayFlatten,
    /// One row per element; an empty or NULL array gives no rows. Only at
    /// the top of a projection, where it multiplies the rows of the other
    /// columns too.
    Unnest,
    /// The most frequent element.
    ArrayMode,
    /// Reductions over the elements, skipping NULLs.
    ArrayMin,
    ArrayMax,
    ArraySum,
    ArrayMean,
    ArrayAny,
    ArrayAll,
    /// `zip(a, b, ...)`: an array of structs `{f1, f2, ...}`, one per position.
    ArrayZip,
    /// `range(start, stop, step)`: the integers from `start` up to but not
    /// including `stop`; empty when `step` is 0 or points away from `stop`.
    Range,
    // structs
    /// `struct(name1, value1, name2, value2, ...)`; the names are literals.
    StructPack,
    /// `field(struct, name)`; the name is a literal.
    StructField,
    // maps
    /// `map(keys, values)` from two arrays of the same length.
    Map,
    /// `get(map, key, default)`: the value at `key`, else `default`.
    MapGet,
    MapContains,
    /// Number of entries, int64.
    MapLength,
    MapKeys,
    MapValues,
    /// Entries of both maps; on a shared key the right map's value wins.
    MapMerge,
}

impl Func {
    /// The function a name shared between types means for a first argument
    /// of type `first`: `length`, `contains` and `repeat` are one method on
    /// strings, arrays and maps, as in Ibis. Compilers call this before
    /// looking at the function.
    pub fn resolve(self, first: Option<&DataType>) -> Func {
        match (self, first) {
            (Func::Length, Some(DataType::Array(_))) => Func::ArrayLength,
            (Func::Length, Some(DataType::Map(..))) => Func::MapLength,
            (Func::Contains, Some(DataType::Array(_))) => Func::ArrayContains,
            (Func::Contains, Some(DataType::Map(..))) => Func::MapContains,
            (Func::Repeat, Some(DataType::Array(_))) => Func::ArrayRepeat,
            (f, _) => f,
        }
    }
}

/// Units `Expr::truncate` accepts.
pub const TRUNCATE_UNITS: [&str; 10] = ["Y", "Q", "M", "W", "D", "h", "m", "s", "ms", "us"];

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
    /// The values as an array, NULLs left out, in no particular order.
    Collect,
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

/// A timestamp literal from `YYYY-MM-DD HH:MM:SS[.ffffff]` (Ibis
/// `ibis.timestamp("...")`). An unparseable string becomes a NULL timestamp.
pub fn timestamp(s: &str) -> Expr {
    let t = DataType::Timestamp { tz: None };
    match crate::convert::parse_timestamp(s) {
        Some(us) => Expr::Literal(Value::Timestamp(us), t),
        None => null(t),
    }
}

/// Smallest of the arguments, row by row (Ibis `ibis.least`).
pub fn least(args: impl IntoIterator<Item = impl Into<Expr>>) -> Expr {
    func(Func::Least, args.into_iter().map(Into::into).collect())
}

/// Largest of the arguments, row by row (Ibis `ibis.greatest`).
pub fn greatest(args: impl IntoIterator<Item = impl Into<Expr>>) -> Expr {
    func(Func::Greatest, args.into_iter().map(Into::into).collect())
}

/// The constant pi (Ibis `ibis.pi`).
pub fn pi() -> Expr {
    lit(std::f64::consts::PI)
}

/// Euler's number (Ibis `ibis.e`).
pub fn e() -> Expr {
    lit(std::f64::consts::E)
}

/// A date from year, month and day expressions (Ibis `ibis.date(y, m, d)`).
pub fn date_from_ymd(year: impl Into<Expr>, month: impl Into<Expr>, day: impl Into<Expr>) -> Expr {
    func(Func::DateFromYmd, vec![year.into(), month.into(), day.into()])
}

/// A timestamp from its fields (Ibis `ibis.timestamp(y, m, d, h, mi, s)`).
pub fn timestamp_from_ymdhms(parts: [Expr; 6]) -> Expr {
    func(Func::TimestampFromYmdhms, parts.into())
}

/// An array of the given expressions (Ibis `ibis.array([...])`). A Rust
/// `Vec` or array of plain values is a literal too: `lit(vec![1, 2, 3])`.
pub fn array(items: impl IntoIterator<Item = impl Into<Expr>>) -> Expr {
    func(Func::Array, items.into_iter().map(Into::into).collect())
}

/// A struct of named expressions (Ibis `ibis.struct({...})`).
pub fn struct_<'a>(fields: impl IntoIterator<Item = (&'a str, Expr)>) -> Expr {
    func(Func::StructPack, fields.into_iter().flat_map(|(name, value)| [lit(name), value]).collect())
}

/// A map from an array of keys and an array of values (Ibis `ibis.map(keys, values)`).
pub fn map(keys: impl Into<Expr>, values: impl Into<Expr>) -> Expr {
    func(Func::Map, vec![keys.into(), values.into()])
}

/// The integers from `start` up to, not including, `stop`, `step` apart
/// (Ibis `ibis.range(start, stop, step)`).
pub fn range(start: impl Into<Expr>, stop: impl Into<Expr>, step: impl Into<Expr>) -> Expr {
    func(Func::Range, vec![start.into(), stop.into(), step.into()])
}

/// An argument that is a value to compare with or look up: an expression,
/// or a Rust value taken as a literal. Here a `&str` is a string, not a
/// column name.
pub trait IntoOperand {
    fn into_operand(self) -> Expr;
}
impl IntoOperand for Expr {
    fn into_operand(self) -> Expr {
        self
    }
}
macro_rules! operand_lit {
    ($($t:ty),*) => {$(
        impl IntoOperand for $t {
            fn into_operand(self) -> Expr { lit(self) }
        }
    )*};
}
operand_lit!(&str, String, i32, i64, f64, bool);

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

/// How deep lambdas nest inside `e`.
fn lambda_depth(e: &Expr) -> usize {
    let inner = e.children().into_iter().map(lambda_depth).max().unwrap_or(0);
    inner + usize::from(matches!(e, Expr::ArrayLambda { .. }))
}

/// Build a lambda from a Rust closure over its parameters. Parameters are
/// named after how deep the lambda nests (`ibis_x1` is innermost), so a
/// lambda never captures a parameter of one inside or around it, and the
/// same expression always compiles to the same text.
fn lambda(kind: Lambda, arg: Expr, f: impl FnOnce(Expr, Expr) -> Expr) -> Expr {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (x, i) = (format!("\u{0}x{id}"), format!("\u{0}i{id}"));
    let body = f(Expr::Param(x.clone(), DataType::Null), Expr::Param(i.clone(), DataType::Null));
    let depth = lambda_depth(&body) + 1;
    let (param, index) = (format!("ibis_x{depth}"), format!("ibis_i{depth}"));
    let uses_index = body.has(&|e| matches!(e, Expr::Param(n, _) if *n == i));
    let body = body
        .replace_param(&x, &Expr::Param(param.clone(), DataType::Null))
        .replace_param(&i, &Expr::Param(index.clone(), DataType::Null));
    Expr::ArrayLambda { kind, arg: Box::new(arg), param, index: uses_index.then_some(index), body: Box::new(body) }
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
    pub fn isinf(self) -> Expr {
        func(Func::IsInf, vec![self])
    }
    /// Logarithm in the given base.
    pub fn log(self, base: impl Into<Expr>) -> Expr {
        func(Func::Log, vec![self, base.into()])
    }
    pub fn radians(self) -> Expr {
        func(Func::Radians, vec![self])
    }
    pub fn degrees(self) -> Expr {
        func(Func::Degrees, vec![self])
    }
    pub fn sin(self) -> Expr {
        func(Func::Sin, vec![self])
    }
    pub fn cos(self) -> Expr {
        func(Func::Cos, vec![self])
    }
    pub fn tan(self) -> Expr {
        func(Func::Tan, vec![self])
    }
    pub fn asin(self) -> Expr {
        func(Func::Asin, vec![self])
    }
    pub fn acos(self) -> Expr {
        func(Func::Acos, vec![self])
    }
    pub fn atan(self) -> Expr {
        func(Func::Atan, vec![self])
    }
    /// `atan2(self, x)`: the angle of the point (x, self).
    pub fn atan2(self, x: impl Into<Expr>) -> Expr {
        func(Func::Atan2, vec![self, x.into()])
    }
    pub fn cot(self) -> Expr {
        func(Func::Cot, vec![self])
    }
    /// Limit to `[lower, upper]`; `None` leaves that side open. NULL stays
    /// NULL and the type doesn't change (Ibis `clip`).
    pub fn clip(self, lower: Option<Expr>, upper: Option<Expr>) -> Expr {
        let open = || null(DataType::Null);
        func(Func::Clip, vec![self, lower.unwrap_or_else(open), upper.unwrap_or_else(open)])
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
    /// Whether a string has the substring, an array the element, or a map the key.
    pub fn contains(self, x: impl IntoOperand) -> Expr {
        func(Func::Contains, vec![self, x.into_operand()])
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
    /// Monday = 0 (Ibis `x.day_of_week.index()`).
    pub fn day_of_week_index(self) -> Expr {
        func(Func::DayOfWeekIndex, vec![self])
    }
    /// English day name (Ibis `x.day_of_week.full_name()`).
    pub fn day_of_week_name(self) -> Expr {
        func(Func::DayOfWeekName, vec![self])
    }
    pub fn day_of_year(self) -> Expr {
        func(Func::DayOfYear, vec![self])
    }
    pub fn quarter(self) -> Expr {
        func(Func::Quarter, vec![self])
    }
    pub fn millisecond(self) -> Expr {
        func(Func::Millisecond, vec![self])
    }
    pub fn microsecond(self) -> Expr {
        func(Func::Microsecond, vec![self])
    }
    /// ISO 8601 week number.
    pub fn week_of_year(self) -> Expr {
        func(Func::WeekOfYear, vec![self])
    }
    /// The date part of a timestamp, or a string parsed as an ISO date.
    pub fn date(self) -> Expr {
        self.cast(DataType::Date)
    }
    /// Round down to the start of `unit`: one of `TRUNCATE_UNITS`
    /// (Y Q M W D h m s ms us; weeks start on Monday).
    pub fn truncate(self, unit: &str) -> Expr {
        func(Func::Truncate, vec![self, lit(unit)])
    }
    /// Format with a C `strftime` pattern, e.g. `"%Y-%m-%d"`.
    pub fn strftime(self, format: &str) -> Expr {
        func(Func::Strftime, vec![self, lit(format)])
    }

    // arrays
    /// Element `i` of an array, **0-based**; negative counts from the end
    /// and out of range is NULL (Ibis `arr[i]`).
    pub fn at(self, i: impl Into<Expr>) -> Expr {
        func(Func::ArrayIndex, vec![self, i.into()])
    }
    /// Python's `arr[start:stop]` on an array; `None` is an open end.
    pub fn slice(self, start: Option<i64>, stop: Option<i64>) -> Expr {
        let bound = |b: Option<i64>| b.map_or_else(|| null(DataType::Int64), lit);
        func(Func::ArraySlice, vec![self, bound(start), bound(stop)])
    }
    /// This array followed by the others.
    pub fn concat(self, others: impl IntoIterator<Item = impl Into<Expr>>) -> Expr {
        func(Func::ArrayConcat, std::iter::once(self).chain(others.into_iter().map(Into::into)).collect())
    }
    /// 0-based position of the first element equal to `x`, or -1 (Ibis `arr.index(x)`).
    pub fn index(self, x: impl IntoOperand) -> Expr {
        func(Func::ArrayPosition, vec![self, x.into_operand()])
    }
    /// The array without the elements equal to `x`.
    pub fn remove(self, x: impl IntoOperand) -> Expr {
        func(Func::ArrayRemove, vec![self, x.into_operand()])
    }
    /// The distinct elements of an array, in no particular order.
    pub fn unique(self) -> Expr {
        func(Func::ArrayDistinct, vec![self])
    }
    /// The elements in ascending order, NULLs last.
    pub fn sort(self) -> Expr {
        func(Func::ArraySort, vec![self])
    }
    /// The distinct elements of both arrays.
    pub fn union(self, other: impl Into<Expr>) -> Expr {
        func(Func::ArrayUnion, vec![self, other.into()])
    }
    /// The distinct elements both arrays have.
    pub fn intersect(self, other: impl Into<Expr>) -> Expr {
        func(Func::ArrayIntersect, vec![self, other.into()])
    }
    /// The array of `f(element)` (Ibis `arr.map(f)`).
    pub fn map(self, f: impl FnOnce(Expr) -> Expr) -> Expr {
        lambda(Lambda::Map, self, |x, _| f(x))
    }
    /// `map` with the element's 0-based position as a second argument.
    pub fn map_with_index(self, f: impl FnOnce(Expr, Expr) -> Expr) -> Expr {
        lambda(Lambda::Map, self, f)
    }
    /// The elements `f(element)` is true for (Ibis `arr.filter(f)`).
    pub fn filter(self, f: impl FnOnce(Expr) -> Expr) -> Expr {
        lambda(Lambda::Filter, self, |x, _| f(x))
    }
    /// `filter` with the element's 0-based position as a second argument.
    pub fn filter_with_index(self, f: impl FnOnce(Expr, Expr) -> Expr) -> Expr {
        lambda(Lambda::Filter, self, f)
    }
    /// One row per element of an array (Ibis `arr.unnest()`): use it as a
    /// column of `select` or `mutate`. Empty and NULL arrays give no rows.
    pub fn unnest(self) -> Expr {
        func(Func::Unnest, vec![self])
    }
    /// Most frequent element of each array (Ibis `modes`).
    pub fn modes(self) -> Expr {
        func(Func::ArrayMode, vec![self])
    }
    /// The values of a column as one array, NULLs left out (Ibis `collect`).
    pub fn collect(self) -> Expr {
        agg(AggFunc::Collect, self)
    }
    /// Remove one level of nesting from an array of arrays.
    pub fn flatten(self) -> Expr {
        func(Func::ArrayFlatten, vec![self])
    }
    /// Smallest element of each array, skipping NULLs (Ibis `mins`).
    pub fn mins(self) -> Expr {
        func(Func::ArrayMin, vec![self])
    }
    pub fn maxs(self) -> Expr {
        func(Func::ArrayMax, vec![self])
    }
    pub fn sums(self) -> Expr {
        func(Func::ArraySum, vec![self])
    }
    pub fn means(self) -> Expr {
        func(Func::ArrayMean, vec![self])
    }
    /// Whether any element of a boolean array is true.
    pub fn anys(self) -> Expr {
        func(Func::ArrayAny, vec![self])
    }
    pub fn alls(self) -> Expr {
        func(Func::ArrayAll, vec![self])
    }
    /// An array of structs `{f1, f2, ...}` pairing up the elements of this
    /// array and the others by position.
    pub fn zip(self, others: impl IntoIterator<Item = impl Into<Expr>>) -> Expr {
        func(Func::ArrayZip, std::iter::once(self).chain(others.into_iter().map(Into::into)).collect())
    }
    // structs
    /// A field of a struct (Ibis `s["name"]`, `s.name`).
    pub fn field(self, name: &str) -> Expr {
        func(Func::StructField, vec![self, lit(name)])
    }
    // maps
    /// The value at `key`, or NULL (Ibis `m[key]`, `m.get(key)`).
    pub fn get(self, key: impl IntoOperand) -> Expr {
        func(Func::MapGet, vec![self, key.into_operand(), null(DataType::Null)])
    }
    /// The value at `key`, or `default` (Ibis `m.get(key, default)`).
    pub fn get_or(self, key: impl IntoOperand, default: impl IntoOperand) -> Expr {
        func(Func::MapGet, vec![self, key.into_operand(), default.into_operand()])
    }
    /// The keys of a map, as an array.
    pub fn keys(self) -> Expr {
        func(Func::MapKeys, vec![self])
    }
    /// The values of a map, as an array in the order of `keys`.
    pub fn values(self) -> Expr {
        func(Func::MapValues, vec![self])
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

    /// Whether `pred` holds for this expression or any part of it.
    pub fn has(&self, pred: &impl Fn(&Expr) -> bool) -> bool {
        pred(self) || self.children().into_iter().any(|c| c.has(pred))
    }

    /// Whether the expression multiplies rows (contains an `unnest`).
    pub fn is_unnest(&self) -> bool {
        self.has(&|e| matches!(e, Expr::Func(Func::Unnest, _)))
    }

    /// This expression with every use of the lambda parameter `name`
    /// replaced by `with`.
    pub fn replace_param(self, name: &str, with: &Expr) -> Expr {
        match self {
            Expr::Param(n, _) if n == name => with.clone(),
            e => e.map_children(&mut |c| Ok(c.replace_param(name, with))).expect("infallible"),
        }
    }

    /// The body of an `ArrayLambda` with its parameters typed against
    /// `schema`: the element parameter as the array's element type, the
    /// index as int64. Compilers call this before compiling the body, so
    /// every part of it can be typed on its own.
    pub fn lambda_body(&self, schema: &Schema) -> Result<Expr> {
        let Expr::ArrayLambda { arg, param, index, body, .. } = self else {
            return invalid(format!("not a lambda: {self}"));
        };
        let item = match arg.dtype(schema)? {
            DataType::Array(item) => *item,
            DataType::Null => DataType::Null,
            other => return invalid(format!("map and filter need an array, got {other}")),
        };
        let mut body = (**body).clone().replace_param(param, &Expr::Param(param.clone(), item));
        if let Some(i) = index {
            body = body.replace_param(i, &Expr::Param(i.clone(), DataType::Int64));
        }
        Ok(body)
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
            Expr::Column(_)
            | Expr::Literal(..)
            | Expr::Field { .. }
            | Expr::ScalarSubquery(_)
            | Expr::Exists(_)
            | Expr::Param(..) => {
                vec![]
            }
            Expr::ArrayLambda { arg, body, .. } => vec![arg, body],
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
            | Expr::Exists(_)
            | Expr::Param(..)) => e,
            Expr::ArrayLambda { kind, arg, param, index, body } => {
                Expr::ArrayLambda { kind, arg: Box::new(f(*arg)?), param, index, body: Box::new(f(*body)?) }
            }
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
                    // arrays concatenate and repeat like strings; maps merge
                    Add if matches!(lt, T::Array(_)) || matches!(rt, T::Array(_)) => {
                        func_dtype(Func::ArrayConcat, &[lt, rt])?
                    }
                    Add if matches!(lt, T::Map(..)) || matches!(rt, T::Map(..)) => {
                        func_dtype(Func::MapMerge, &[lt, rt])?
                    }
                    Mul if matches!(lt, T::Array(_)) && rt.is_integer() => lt,
                    Mul if lt.is_integer() && matches!(rt, T::Array(_)) => rt,
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
                if let (Func::Truncate, Some(Expr::Literal(Value::String(u), _))) = (f, args.get(1))
                    && !TRUNCATE_UNITS.contains(&u.as_str())
                {
                    return invalid(format!("truncate: unknown unit {u:?}; expected one of {TRUNCATE_UNITS:?}"));
                }
                let ts = args.iter().map(|a| a.dtype(schema)).collect::<Result<Vec<_>>>()?;
                let literal_name = |e: &Expr| match e.unaliased() {
                    Expr::Literal(Value::String(s), _) => Ok(s.clone()),
                    other => invalid(format!("{f:?}: a field name must be a string literal, got {other}")),
                };
                match f.resolve(ts.first()) {
                    Func::StructField => match (&ts[0], literal_name(&args[1])?) {
                        (T::Struct(fields), name) => match fields.iter().find(|(n, _)| *n == name) {
                            Some((_, t)) => t.clone(),
                            None => {
                                let have: Vec<_> = fields.iter().map(|(n, _)| n).collect();
                                return invalid(format!("no field {name:?} in struct; have {have:?}"));
                            }
                        },
                        (other, name) => return invalid(format!("field {name:?} of a non-struct: {other}")),
                    },
                    Func::StructPack => T::Struct(
                        args.chunks(2)
                            .zip(ts.chunks(2))
                            .map(|(pair, t)| match (pair, t) {
                                ([name, _], [_, t]) => Ok((literal_name(name)?, t.clone())),
                                _ => invalid("struct: a field name without a value"),
                            })
                            .collect::<Result<_>>()?,
                    ),
                    f => func_dtype(f, &ts)?,
                }
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
                    (Collect, Some(t)) => T::array(t),
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
            Expr::Param(_, t) => t.clone(),
            Expr::ArrayLambda { kind, arg, .. } => {
                let body = self.lambda_body(schema)?.dtype(schema)?;
                match kind {
                    Lambda::Map => T::array(body),
                    Lambda::Filter if matches!(body, T::Boolean | T::Null) => arg.dtype(schema)?,
                    Lambda::Filter => return invalid(format!("filter needs a boolean condition, got {body}")),
                }
            }
        })
    }
}

/// What `+` and `*` mean on arrays and maps, as a function call: arrays
/// concatenate and repeat, maps merge. `None` for every other operand type.
pub fn nested_binary(op: BinaryOp, l: &Expr, r: &Expr, lt: &DataType, rt: &DataType) -> Option<Expr> {
    use DataType as T;
    let (l, r) = (|| l.clone(), || r.clone());
    Some(match (op, lt, rt) {
        (BinaryOp::Add, T::Array(_), _) | (BinaryOp::Add, _, T::Array(_)) => func(Func::ArrayConcat, vec![l(), r()]),
        (BinaryOp::Add, T::Map(..), _) | (BinaryOp::Add, _, T::Map(..)) => func(Func::MapMerge, vec![l(), r()]),
        (BinaryOp::Mul, T::Array(_), n) if n.is_integer() => func(Func::ArrayRepeat, vec![l(), r()]),
        (BinaryOp::Mul, n, T::Array(_)) if n.is_integer() => func(Func::ArrayRepeat, vec![r(), l()]),
        _ => return None,
    })
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
    DataType::unify(a, b)
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
        Year | Month | Day | Hour | Minute | Second | DayOfWeekIndex | DayOfYear | Quarter | Millisecond
        | Microsecond | WeekOfYear | DayOfWeekName | Truncate | Strftime => {
            if !first.is_temporal() && first != T::Null {
                return invalid(format!("{f:?} needs a date or timestamp, got {first}"));
            }
            match f {
                DayOfWeekIndex => T::Int16,
                DayOfWeekName | Strftime => T::String,
                Truncate => first,
                _ => T::Int32,
            }
        }
        Least | Greatest => ts.iter().try_fold(T::Null, |acc, t| unify(&acc, t))?,
        Log | Radians | Degrees | Sin | Cos | Tan | Asin | Acos | Atan | Atan2 | Cot => {
            if let Some(t) = ts.iter().find(|t| !t.is_numeric() && **t != T::Null) {
                return invalid(format!("{f:?} needs numeric arguments, got {t}"));
            }
            T::Float64
        }
        IsInf => T::Boolean,
        Clip => first,
        Array => T::array(ts.iter().try_fold(T::Null, |acc, t| unify(&acc, t))?),
        ArrayLength | ArrayPosition | MapLength => T::Int64,
        ArrayContains | MapContains => T::Boolean,
        ArrayConcat | ArrayUnion | ArrayIntersect | MapMerge => {
            let fits = |t: &T| match f {
                MapMerge => matches!(t, T::Map(..) | T::Null),
                _ => matches!(t, T::Array(_) | T::Null),
            };
            if let Some(t) = ts.iter().find(|t| !fits(t)) {
                return invalid(format!("{f:?} needs {}, got {t}", if f == MapMerge { "maps" } else { "arrays" }));
            }
            ts.iter().try_fold(T::Null, |acc, t| unify(&acc, t))?
        }
        ArrayIndex | ArrayRepeat | ArraySlice | ArrayRemove | ArrayDistinct | ArraySort | ArrayFlatten | ArrayMin
        | ArrayMax | ArraySum | ArrayMean | ArrayAny | ArrayAll | ArrayMode | Unnest => {
            let item = match &first {
                T::Array(item) => (**item).clone(),
                T::Null => T::Null,
                other => return invalid(format!("{f:?} needs an array, got {other}")),
            };
            match f {
                ArrayIndex | ArrayMin | ArrayMax | ArraySum | ArrayAny | ArrayAll | ArrayMode | Unnest => item,
                ArrayMean if item.is_integer() || item.is_floating() => T::Float64,
                ArrayMean => item,
                ArrayFlatten => match item {
                    T::Array(_) | T::Null => item,
                    other => return invalid(format!("flatten needs an array of arrays, got array<{other}>")),
                },
                _ => first,
            }
        }
        ArrayZip => T::array(T::Struct(
            ts.iter()
                .enumerate()
                .map(|(i, t)| match t {
                    T::Array(item) => Ok((format!("f{}", i + 1), (**item).clone())),
                    other => invalid(format!("zip needs arrays, got {other}")),
                })
                .collect::<Result<_>>()?,
        )),
        Range => {
            if let Some(t) = ts.iter().find(|t| !t.is_integer()) {
                return invalid(format!("range needs integers, got {t}"));
            }
            T::array(unify(&ts[0], &ts[1])?)
        }
        // typed where the field names are at hand, in `Expr::dtype`
        StructPack | StructField => return invalid(format!("{f:?} outside an expression")),
        Map => match (&ts[0], &ts[1]) {
            (T::Array(k), T::Array(v)) => T::map((**k).clone(), (**v).clone()),
            (k, v) => return invalid(format!("map needs an array of keys and one of values, got {k} and {v}")),
        },
        MapGet | MapKeys | MapValues => {
            let (key, value) = match &first {
                T::Map(k, v) => ((**k).clone(), (**v).clone()),
                T::Null => (T::Null, T::Null),
                other => return invalid(format!("{f:?} needs a map, got {other}")),
            };
            match f {
                MapKeys => T::array(key),
                MapValues => T::array(value),
                _ => unify(&value, ts.get(2).unwrap_or(&T::Null))?,
            }
        }
        DateFromYmd | TimestampFromYmdhms => {
            if let Some(t) = ts.iter().find(|t| !t.is_numeric()) {
                return invalid(format!("{f:?} needs numeric fields, got {t}"));
            }
            if f == DateFromYmd { T::Date } else { T::Timestamp { tz: None } }
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
            Expr::Param(n, _) => f.write_str(n),
            Expr::ArrayLambda { kind, arg, body, .. } => write!(f, "Array{kind:?}({arg}, {body})"),
        }
    }
}
