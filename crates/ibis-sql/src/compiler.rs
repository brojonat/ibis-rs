//! IR -> SQL.
//!
//! Each `Rel` lowers to a `Select`. An op merges into the current `Select`
//! only when that preserves meaning; otherwise the current query is wrapped as
//! a derived table and the op applies to that. Correctness over pretty SQL.

use std::cell::Cell;

use ibis_core::{
    AggFunc, Analytic, BinaryOp, Bound, DataType, Error, Expr, Func, JoinKind, Rel, RelOp, Result, Schema, SetKind,
    Side, SortKey, Table, UnaryOp, Value, WindowSpec,
};

use crate::dialect::Dialect;

/// Compile a table expression to a SQL query string.
pub fn compile(d: &dyn Dialect, t: &Table) -> Result<String> {
    let c = RelCompiler { d, aliases: Cell::new(0) };
    Ok(c.rel(t.rel())?.render(d))
}

/// `CREATE TABLE` DDL for `schema` (no `OR REPLACE`: not portable).
pub fn create_table_sql(d: &dyn Dialect, name: &str, schema: &Schema) -> Result<String> {
    let cols = schema
        .fields
        .iter()
        .map(|f| Ok(format!("{} {}", d.quote_ident(&f.name), d.type_name(&f.dtype)?)))
        .collect::<Result<Vec<_>>>()?;
    Ok(format!("CREATE TABLE {} ({})", d.quote_ident(name), cols.join(", ")))
}

pub(crate) fn unsupported<T>(d: &dyn Dialect, what: impl Into<String>) -> Result<T> {
    Err(Error::Unsupported { backend: d.name().into(), op: what.into() })
}

// ---------------------------------------------------------------------------
// relations

#[derive(Default)]
struct Select {
    distinct: bool,
    /// `None` means `*`.
    items: Option<Vec<String>>,
    from: String,
    wheres: Vec<String>,
    group_by: Vec<String>,
    order_by: Vec<String>,
    limit: Option<u64>,
    offset: u64,
}

impl Select {
    fn from(from: String) -> Self {
        Select { from, ..Default::default() }
    }

    /// No projection, grouping, distinct or limit: a WHERE/SELECT list can still be added.
    fn is_plain(&self) -> bool {
        self.items.is_none() && self.group_by.is_empty() && !self.distinct && self.limit.is_none() && self.offset == 0
    }

    fn render(&self, d: &dyn Dialect) -> String {
        let mut s = String::from("SELECT ");
        if self.distinct {
            s.push_str("DISTINCT ");
        }
        match &self.items {
            None => s.push('*'),
            Some(items) => s.push_str(&items.join(", ")),
        }
        s.push_str(" FROM ");
        s.push_str(&self.from);
        if !self.wheres.is_empty() {
            s.push_str(" WHERE ");
            s.push_str(&self.wheres.join(" AND "));
        }
        if !self.group_by.is_empty() {
            s.push_str(" GROUP BY ");
            s.push_str(&self.group_by.join(", "));
        }
        if !self.order_by.is_empty() {
            s.push_str(" ORDER BY ");
            s.push_str(&self.order_by.join(", "));
        }
        s.push_str(&d.limit_offset(self.limit, self.offset));
        s
    }
}

struct RelCompiler<'a> {
    d: &'a dyn Dialect,
    aliases: Cell<usize>,
}

impl RelCompiler<'_> {
    fn alias(&self) -> String {
        let n = self.aliases.get();
        self.aliases.set(n + 1);
        format!("t{n}")
    }

    fn wrap(&self, s: Select) -> Select {
        Select::from(format!("({}) AS {}", s.render(self.d), self.alias()))
    }

    fn exprs<'s>(&'s self, schema: &'s Schema, in_project: bool) -> ExprCompiler<'s> {
        ExprCompiler { d: self.d, schema, in_project }
    }

    /// A relation usable in a FROM clause under `alias`.
    fn source(&self, r: &Rel, alias: &str) -> Result<String> {
        if let RelOp::Table { name } = &r.op {
            return Ok(format!("{} AS {alias}", self.d.quote_ident(name)));
        }
        Ok(format!("({}) AS {alias}", self.rel(r)?.render(self.d)))
    }

    fn rel(&self, r: &Rel) -> Result<Select> {
        let q = |s: &str| self.d.quote_ident(s);
        match &r.op {
            RelOp::Table { name } => Ok(Select::from(q(name))),

            RelOp::Filter { input, predicates } => {
                let mut s = self.rel(input)?;
                if !s.is_plain() {
                    s = self.wrap(s);
                }
                let c = self.exprs(&input.schema, false);
                for p in predicates {
                    if contains_window(p) {
                        return unsupported(self.d, "window function in filter (mutate first)");
                    }
                    s.wheres.push(c.sql(p)?);
                }
                Ok(s)
            }

            RelOp::Project { input, exprs } => {
                let mut s = self.rel(input)?;
                if !s.is_plain() {
                    s = self.wrap(s);
                }
                let c = self.exprs(&input.schema, true);
                let items =
                    exprs.iter().map(|(n, e)| Ok(format!("{} AS {}", c.sql(e)?, q(n)))).collect::<Result<Vec<_>>>()?;
                s.items = Some(items);
                Ok(s)
            }

            RelOp::Aggregate { input, groups, metrics } => {
                let mut s = self.rel(input)?;
                if !s.is_plain() {
                    s = self.wrap(s);
                }
                s.order_by.clear();
                let c = self.exprs(&input.schema, false);
                let mut items = vec![];
                for (n, e) in groups {
                    let sql = c.sql(e)?;
                    items.push(format!("{sql} AS {}", q(n)));
                    s.group_by.push(sql);
                }
                for (n, e) in metrics {
                    items.push(format!("{} AS {}", c.sql(e)?, q(n)));
                }
                s.items = Some(items);
                Ok(s)
            }

            RelOp::Sort { input, keys } => {
                let mut s = self.rel(input)?;
                if !s.is_plain() {
                    s = self.wrap(s);
                }
                let c = self.exprs(&input.schema, false);
                s.order_by = keys.iter().map(|k| c.sort_key(k)).collect::<Result<_>>()?;
                Ok(s)
            }

            RelOp::Limit { input, n, offset } => {
                let mut s = self.rel(input)?;
                if s.limit.is_some() || s.offset > 0 {
                    s = self.wrap(s);
                }
                s.limit = *n;
                s.offset = *offset;
                Ok(s)
            }

            RelOp::Distinct { input } => {
                let mut s = self.rel(input)?;
                if !s.group_by.is_empty() || s.distinct || s.limit.is_some() || s.offset > 0 || !s.order_by.is_empty() {
                    s = self.wrap(s);
                }
                s.distinct = true;
                Ok(s)
            }

            RelOp::Join { left, right, how, on, outputs } => {
                let (la, ra) = (self.alias(), self.alias());
                let col = |side: Side, c: &str| format!("{}.{}", if side == Side::Left { &la } else { &ra }, q(c));
                let cond = on
                    .iter()
                    .map(|(l, r)| format!("{} = {}", col(Side::Left, l), col(Side::Right, r)))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                let items =
                    outputs.iter().map(|(out, side, src)| format!("{} AS {}", col(*side, src), q(out))).collect();
                let (lsrc, rsrc) = (self.source(left, &la)?, self.source(right, &ra)?);
                let mut s = match how {
                    JoinKind::Semi | JoinKind::Anti => {
                        let not = if *how == JoinKind::Anti { "NOT " } else { "" };
                        let mut s = Select::from(lsrc);
                        s.wheres.push(format!("{not}EXISTS (SELECT 1 FROM {rsrc} WHERE {cond})"));
                        s
                    }
                    _ => {
                        let kw = match how {
                            JoinKind::Inner => "INNER JOIN",
                            JoinKind::Left => "LEFT OUTER JOIN",
                            JoinKind::Right => "RIGHT OUTER JOIN",
                            _ => "FULL OUTER JOIN",
                        };
                        Select::from(format!("{lsrc} {kw} {rsrc} ON {cond}"))
                    }
                };
                s.items = Some(items);
                Ok(s)
            }

            RelOp::SetOp { kind, distinct, left, right } => {
                let kw = match (kind, distinct) {
                    (SetKind::Union, false) => "UNION ALL",
                    (SetKind::Union, true) => "UNION",
                    (SetKind::Intersect, true) => "INTERSECT",
                    (SetKind::Intersect, false) => "INTERSECT ALL",
                    (SetKind::Difference, true) => "EXCEPT",
                    (SetKind::Difference, false) => "EXCEPT ALL",
                };
                // `SELECT * FROM (...) AS tN` on each side: SQLite rejects
                // parenthesized compound members.
                let l = self.source(left, &self.alias())?;
                let r = self.source(right, &self.alias())?;
                let body = format!("SELECT * FROM {l} {kw} SELECT * FROM {r}");
                Ok(Select::from(format!("({body}) AS {}", self.alias())))
            }
        }
    }
}

fn contains_window(e: &Expr) -> bool {
    matches!(e, Expr::Window(..) | Expr::Analytic(..)) || e.children().iter().any(|c| contains_window(c))
}

// ---------------------------------------------------------------------------
// expressions

/// Compiles expressions against one input schema. Handed to dialect hooks so
/// overrides can compile their arguments.
pub struct ExprCompiler<'a> {
    pub d: &'a dyn Dialect,
    pub schema: &'a Schema,
    /// In a projection, a bare reduction is broadcast: `agg(...) OVER ()`.
    pub in_project: bool,
}

impl ExprCompiler<'_> {
    pub fn dtype(&self, e: &Expr) -> Result<DataType> {
        e.dtype(self.schema)
    }

    pub fn sql(&self, e: &Expr) -> Result<String> {
        let d = self.d;
        Ok(match e {
            Expr::Column(n) => d.quote_ident(n),
            Expr::Literal(v, t) => self.literal(v, t)?,
            Expr::Alias(e, _) => self.sql(e)?,
            Expr::Cast(e, t) => match d.cast(self, e, t)? {
                Some(s) => s,
                None => format!("CAST({} AS {})", self.sql(e)?, d.type_name(t)?),
            },
            Expr::Unary(op, e) => {
                let a = self.sql(e)?;
                match op {
                    UnaryOp::Not => format!("(NOT {a})"),
                    UnaryOp::Negate => format!("(-{a})"),
                    UnaryOp::IsNull => format!("({a} IS NULL)"),
                    UnaryOp::NotNull => format!("({a} IS NOT NULL)"),
                }
            }
            Expr::Binary(op, l, r) => match d.binary(self, *op, l, r)? {
                Some(s) => s,
                None => self.default_binary(*op, l, r)?,
            },
            Expr::Func(f, args) => match d.func(self, *f, args)? {
                Some(s) => s,
                None => self.default_func(*f, args)?,
            },
            Expr::Agg { func, arg, filter } => {
                let filter = match filter {
                    Some(f) => format!(" FILTER (WHERE {})", self.sql(f)?),
                    None => String::new(),
                };
                let mut s = match d.agg(self, *func, arg.as_deref(), &filter)? {
                    Some(s) => s,
                    None => self.default_agg(*func, arg.as_deref(), &filter)?,
                };
                if self.in_project {
                    s = format!("{s} OVER ()");
                }
                s
            }
            Expr::Analytic(..) => return unsupported(d, format!("{e} without a window (use .over(...))")),
            Expr::Window(f, spec) => self.window(f, spec)?,
            Expr::Case { branches, default } => {
                let mut s = String::from("CASE");
                for (c, r) in branches {
                    s.push_str(&format!(" WHEN {} THEN {}", self.sql(c)?, self.sql(r)?));
                }
                if let Some(x) = default {
                    s.push_str(&format!(" ELSE {}", self.sql(x)?));
                }
                s.push_str(" END");
                s
            }
            Expr::InList { arg, list, negated } => {
                let items = list.iter().map(|e| self.sql(e)).collect::<Result<Vec<_>>>()?;
                let not = if *negated { "NOT " } else { "" };
                format!("({} {not}IN ({}))", self.sql(arg)?, items.join(", "))
            }
        })
    }

    pub fn sort_key(&self, k: &SortKey) -> Result<String> {
        Ok(format!(
            "{} {} NULLS {}",
            self.sql(&k.expr)?,
            if k.ascending { "ASC" } else { "DESC" },
            if k.nulls_first { "FIRST" } else { "LAST" }
        ))
    }

    pub fn literal(&self, v: &Value, t: &DataType) -> Result<String> {
        if let Some(s) = self.d.literal(v, t)? {
            return Ok(s);
        }
        Ok(match v {
            Value::Null if *t == DataType::Null => "NULL".into(),
            Value::Null => format!("CAST(NULL AS {})", self.d.type_name(t)?),
            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) if f.is_finite() => format!("{f:?}"),
            Value::Float(f) => format!(
                "CAST('{}' AS {})",
                if f.is_nan() {
                    "NaN"
                } else if *f > 0.0 {
                    "Infinity"
                } else {
                    "-Infinity"
                },
                self.d.type_name(&DataType::Float64)?
            ),
            Value::String(s) => quote_str(s),
            Value::Binary(_) => return unsupported(self.d, "binary literal"),
            Value::Date(days) => format!("DATE '{}'", ibis_core::convert::format_date(*days)),
            Value::Timestamp(us) => format!("TIMESTAMP '{}'", ibis_core::convert::format_timestamp(*us)),
        })
    }

    fn float(&self) -> Result<String> {
        self.d.type_name(&DataType::Float64)
    }

    pub fn default_binary(&self, op: BinaryOp, l: &Expr, r: &Expr) -> Result<String> {
        let (ls, rs) = (self.sql(l)?, self.sql(r)?);
        let (lt, rt) = (self.dtype(l)?, self.dtype(r)?);
        use BinaryOp::*;
        Ok(match op {
            Add if lt == DataType::String && rt == DataType::String => format!("({ls} || {rs})"),
            Mul if lt == DataType::String => format!("REPEAT({ls}, {rs})"),
            Mul if rt == DataType::String => format!("REPEAT({rs}, {ls})"),
            Add => format!("({ls} + {rs})"),
            Sub => format!("({ls} - {rs})"),
            Mul => format!("({ls} * {rs})"),
            // true division regardless of operand types
            Div => format!("(CAST({ls} AS {}) / {rs})", self.float()?),
            FloorDiv if lt.is_integer() && rt.is_integer() => {
                format!(
                    "CAST(FLOOR(CAST({ls} AS {}) / {rs}) AS {})",
                    self.float()?,
                    self.d.type_name(&DataType::Int64)?
                )
            }
            FloorDiv => format!("FLOOR({ls} / {rs})"),
            Mod => format!("({ls} % {rs})"),
            Pow => format!("POWER({ls}, {rs})"),
            Eq => format!("({ls} = {rs})"),
            NotEq => format!("({ls} <> {rs})"),
            Lt => format!("({ls} < {rs})"),
            Le => format!("({ls} <= {rs})"),
            Gt => format!("({ls} > {rs})"),
            Ge => format!("({ls} >= {rs})"),
            And => format!("({ls} AND {rs})"),
            Or => format!("({ls} OR {rs})"),
        })
    }

    /// Literal integer argument, if the expression is one.
    pub fn lit_int(e: &Expr) -> Option<i64> {
        match e {
            Expr::Literal(Value::Int(i), _) => Some(*i),
            _ => None,
        }
    }

    pub fn default_func(&self, f: Func, args: &[Expr]) -> Result<String> {
        let a = args.iter().map(|x| self.sql(x)).collect::<Result<Vec<_>>>()?;
        let x = a.first().cloned().unwrap_or_default();
        let int = |s: String| -> Result<String> { Ok(format!("CAST({s} AS {})", self.d.type_name(&DataType::Int32)?)) };
        let bigint =
            |s: String| -> Result<String> { Ok(format!("CAST({s} AS {})", self.d.type_name(&DataType::Int64)?)) };
        let to_int64 = self.dtype(&args[0]).map(|t| !t.is_integer()).unwrap_or(false);
        use Func::*;
        Ok(match f {
            Abs => format!("ABS({x})"),
            Ceil if to_int64 => bigint(format!("CEIL({x})"))?,
            Ceil => format!("CEIL({x})"),
            Floor if to_int64 => bigint(format!("FLOOR({x})"))?,
            Floor => format!("FLOOR({x})"),
            Round if a.len() == 1 && to_int64 => bigint(format!("ROUND({x})"))?,
            Round if a.len() == 1 => format!("ROUND({x})"),
            Round => format!("ROUND({x}, {})", a[1]),
            Sign => format!("SIGN({x})"),
            Sqrt => format!("SQRT({x})"),
            Exp => format!("EXP({x})"),
            Ln => format!("LN({x})"),
            Log10 => format!("LOG10({x})"),
            Log2 => format!("LOG2({x})"),
            Upper => format!("UPPER({x})"),
            Lower => format!("LOWER({x})"),
            Length => int(format!("LENGTH({x})"))?,
            Strip => format!("TRIM({x})"),
            LStrip => format!("LTRIM({x})"),
            RStrip => format!("RTRIM({x})"),
            Reverse => format!("REVERSE({x})"),
            Repeat => format!("REPEAT({x}, {})", a[1]),
            Substring => {
                let start = match Self::lit_int(&args[1]) {
                    Some(i) if i >= 0 => (i + 1).to_string(),
                    Some(_) => return unsupported(self.d, "substr with negative start"),
                    None => format!("({} + 1)", a[1]),
                };
                match a.get(2) {
                    Some(len) => format!("SUBSTRING({x}, {start}, {len})"),
                    None => format!("SUBSTRING({x}, {start})"),
                }
            }
            Right => format!("RIGHT({x}, {})", a[1]),
            StartsWith => format!("STARTS_WITH({x}, {})", a[1]),
            EndsWith => format!("(SUBSTRING({x}, LENGTH({x}) - LENGTH({p}) + 1) = {p})", p = a[1]),
            Contains => format!("(STRPOS({x}, {}) > 0)", a[1]),
            Find => match a.get(2) {
                None => bigint(format!("STRPOS({x}, {}) - 1", a[1]))?,
                Some(start) => bigint(format!(
                    "CASE WHEN STRPOS(SUBSTRING({x}, {start} + 1), {p}) = 0 THEN 0 ELSE STRPOS(SUBSTRING({x}, {start} + 1), {p}) + {start} END - 1",
                    p = a[1]
                ))?,
            },
            Replace => format!("REPLACE({x}, {}, {})", a[1], a[2]),
            Like => format!("({x} LIKE {})", a[1]),
            ILike => format!("({x} ILIKE {})", a[1]),
            RegexSearch => format!("REGEXP_MATCHES({x}, {})", a[1]),
            RegexExtract => format!("REGEXP_EXTRACT({x}, {}, {})", a[1], a[2]),
            RegexReplace => format!("REGEXP_REPLACE({x}, {}, {}, 'g')", a[1], a[2]),
            LPad => format!("LPAD({x}, {}, {})", a[1], a[2]),
            RPad => format!("RPAD({x}, {}, {})", a[1], a[2]),
            Capitalize => format!("(UPPER(SUBSTRING({x}, 1, 1)) || LOWER(SUBSTRING({x}, 2)))"),
            Ascii => int(format!("ASCII({x})"))?,
            Coalesce => format!("COALESCE({})", a.join(", ")),
            NullIf => format!("NULLIF({x}, {})", a[1]),
            Year => int(format!("EXTRACT(YEAR FROM {x})"))?,
            Month => int(format!("EXTRACT(MONTH FROM {x})"))?,
            Day => int(format!("EXTRACT(DAY FROM {x})"))?,
            Hour => int(format!("EXTRACT(HOUR FROM {x})"))?,
            Minute => int(format!("EXTRACT(MINUTE FROM {x})"))?,
            Second => int(format!("FLOOR(EXTRACT(SECOND FROM {x}))"))?,
            DayOfWeekIndex => int(format!("(EXTRACT(ISODOW FROM {x}) - 1)"))?,
        })
    }

    pub fn default_agg(&self, f: AggFunc, arg: Option<&Expr>, filter: &str) -> Result<String> {
        let x = match arg {
            Some(a) if self.dtype(a)? == DataType::Boolean && matches!(f, AggFunc::Sum | AggFunc::Mean) => {
                format!("CAST({} AS {})", self.sql(a)?, self.d.type_name(&DataType::Int64)?)
            }
            Some(a) => self.sql(a)?,
            None => String::new(),
        };
        use AggFunc::*;
        let call = match f {
            CountStar => "COUNT(*)".into(),
            Count => format!("COUNT({x})"),
            CountDistinct => format!("COUNT(DISTINCT {x})"),
            Sum => format!("SUM({x})"),
            Mean => format!("AVG({x})"),
            Min => format!("MIN({x})"),
            Max => format!("MAX({x})"),
            StdSamp => format!("STDDEV_SAMP({x})"),
            StdPop => format!("STDDEV_POP({x})"),
            VarSamp => format!("VAR_SAMP({x})"),
            VarPop => format!("VAR_POP({x})"),
            Any => format!("BOOL_OR({x})"),
            All => format!("BOOL_AND({x})"),
        };
        Ok(format!("{call}{filter}"))
    }

    fn window(&self, f: &Expr, spec: &WindowSpec) -> Result<String> {
        let over = self.over(spec)?;
        // compile the function without the broadcast `OVER ()`
        let inner = ExprCompiler { d: self.d, schema: self.schema, in_project: false };
        Ok(match f {
            Expr::Agg { .. } => format!("{} {over}", inner.sql(f)?),
            Expr::Analytic(a, args) => {
                let a_sql = args.iter().map(|x| inner.sql(x)).collect::<Result<Vec<_>>>()?;
                use Analytic::*;
                match a {
                    // Ibis ranks are 0-based
                    RowNumber => format!("(ROW_NUMBER() {over} - 1)"),
                    Rank => format!("(RANK() {over} - 1)"),
                    DenseRank => format!("(DENSE_RANK() {over} - 1)"),
                    PercentRank => format!("PERCENT_RANK() {over}"),
                    CumeDist => format!("CUME_DIST() {over}"),
                    Lag => format!("LAG({}) {over}", a_sql.join(", ")),
                    Lead => format!("LEAD({}) {over}", a_sql.join(", ")),
                    FirstValue => format!("FIRST_VALUE({}) {over}", a_sql[0]),
                    LastValue => format!("LAST_VALUE({}) {over}", a_sql[0]),
                }
            }
            other => return unsupported(self.d, format!("window over {other}")),
        })
    }

    fn over(&self, spec: &WindowSpec) -> Result<String> {
        let mut parts = vec![];
        if !spec.partition_by.is_empty() {
            let p = spec.partition_by.iter().map(|e| self.sql(e)).collect::<Result<Vec<_>>>()?;
            parts.push(format!("PARTITION BY {}", p.join(", ")));
        }
        if !spec.order_by.is_empty() {
            let o = spec.order_by.iter().map(|k| self.sort_key(k)).collect::<Result<Vec<_>>>()?;
            parts.push(format!("ORDER BY {}", o.join(", ")));
        }
        if let Some(fr) = &spec.frame {
            let b = |b: &Bound| match b {
                Bound::Unbounded => None,
                Bound::Preceding(n) => Some(format!("{n} PRECEDING")),
                Bound::CurrentRow => Some("CURRENT ROW".into()),
                Bound::Following(n) => Some(format!("{n} FOLLOWING")),
            };
            let start = b(&fr.start).unwrap_or_else(|| "UNBOUNDED PRECEDING".into());
            let end = b(&fr.end).unwrap_or_else(|| "UNBOUNDED FOLLOWING".into());
            parts.push(format!("{} BETWEEN {start} AND {end}", if fr.rows { "ROWS" } else { "RANGE" }));
        }
        Ok(format!("OVER ({})", parts.join(" ")))
    }
}

pub fn quote_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
