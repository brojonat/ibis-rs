//! IR -> SQL.
//!
//! Each `Rel` lowers to a `Select`. An op merges into the current `Select`
//! only when that preserves meaning; otherwise the current query is wrapped as
//! a derived table and the op applies to that. Correctness over pretty SQL.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

use ibis_core::{
    AggFunc, AggOptions, Analytic, BinaryOp, Bound, DataType, DerefMap, Error, Expr, Func, JoinKind, Rel, RelOp,
    Result, Schema, SetKind, Side, SortKey, Table, UnaryOp, Value, WindowSpec,
};

use crate::dialect::Dialect;

/// Compile a table expression to a SQL query string.
pub fn compile(d: &dyn Dialect, t: &Table) -> Result<String> {
    let c = RelCompiler { d, aliases: Cell::new(0), scopes: RefCell::new(vec![]) };
    // SQL with nothing built on it is run as it was written
    if let RelOp::Sql { query, views } = &t.rel().op {
        return c.sql_query(query, views);
    }
    Ok(c.rel(t.rel())?.render(d))
}

/// The text of a query in the engine's SQL that reads each of `views` as a
/// table of that name: the views are its common table expressions.
pub fn sql_query(d: &dyn Dialect, query: &str, views: &[(String, Arc<Rel>)]) -> Result<String> {
    let c = RelCompiler { d, aliases: Cell::new(0), scopes: RefCell::new(vec![]) };
    c.sql_query(query, views)
}

/// `KurtosisSamp` -> `kurtosis_samp`.
fn snake(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_uppercase() && !out.is_empty() {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

/// `CREATE TABLE` DDL for `schema` (no `OR REPLACE`: not portable).
pub fn create_table_sql(d: &dyn Dialect, name: &str, schema: &Schema) -> Result<String> {
    crate::ddl::create_table_statement(d, name, schema, false)
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
    /// Alias of the single FROM source, if there is one (not for joins).
    alias: Option<String>,
    wheres: Vec<String>,
    group_by: Vec<String>,
    /// Sort keys, most significant first, over the FROM source's columns.
    order: Vec<SortKey>,
    /// `order`, compiled.
    order_sql: Vec<String>,
    /// With `items`: FROM column -> output column, for columns passed through
    /// unchanged. Lets `order` be rewritten over the output when wrapping.
    passthrough: HashMap<String, String>,
    limit: Option<u64>,
    offset: u64,
}

/// `e` with its columns renamed through `map`; None if one isn't in it.
fn rename_columns(e: &Expr, map: &HashMap<String, String>) -> Option<Expr> {
    fn go(e: Expr, map: &HashMap<String, String>) -> Result<Expr> {
        match e {
            Expr::Column(n) => match map.get(&n) {
                Some(m) => Ok(Expr::Column(m.clone())),
                None => Err(Error::Invalid(String::new())),
            },
            e => e.map_children(&mut |c| go(c, map)),
        }
    }
    go(e.clone(), map).ok()
}

impl Select {
    fn from(from: String) -> Self {
        Select { from, ..Default::default() }
    }

    fn aliased(from: String, alias: String) -> Self {
        Select { from, alias: Some(alias), ..Default::default() }
    }

    /// No projection, grouping, distinct or limit: a WHERE/SELECT list can still be added.
    fn is_plain(&self) -> bool {
        self.items.is_none() && self.group_by.is_empty() && !self.distinct && self.limit.is_none() && self.offset == 0
    }

    /// The ordering written over this select's output columns, if it can
    /// be: every column the keys use must be passed through unchanged.
    /// A LIMIT pins the ordering to this select (it picks the rows).
    fn output_order(&self) -> Option<Vec<SortKey>> {
        if self.order.is_empty() || self.limit.is_some() || self.offset > 0 {
            return None;
        }
        if self.items.is_none() {
            return Some(self.order.clone());
        }
        self.order
            .iter()
            .map(|k| Some(SortKey { expr: rename_columns(&k.expr, &self.passthrough)?, ..k.clone() }))
            .collect()
    }

    fn clear_order(&mut self) {
        self.order.clear();
        self.order_sql.clear();
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
        if !self.order_sql.is_empty() {
            s.push_str(" ORDER BY ");
            s.push_str(&self.order_sql.join(", "));
        } else if self.offset > 0 && d.offset_needs_order_by() {
            // which rows an unordered OFFSET skips is the engine's choice; the first column's order is one
            s.push_str(" ORDER BY 1");
        }
        s.push_str(&d.limit_offset(self.limit, self.offset));
        s
    }
}

/// A relation visible to expressions: columns of relation `id` (or of its
/// ancestors, via `deref`) are spelled `alias."column"`.
struct Scope {
    id: u64,
    alias: String,
    deref: DerefMap,
}

struct RelCompiler<'a> {
    d: &'a dyn Dialect,
    aliases: Cell<usize>,
    /// Innermost last. Correlated subqueries look outward through it.
    scopes: RefCell<Vec<Scope>>,
}

impl RelCompiler<'_> {
    fn alias(&self) -> String {
        let n = self.aliases.get();
        self.aliases.set(n + 1);
        format!("t{n}")
    }

    /// `e` with each `unnest(arr)` replaced by the column of a lateral join
    /// over `arr`; the joins are added to `joins`.
    fn lift_unnests(&self, e: Expr, c: &ExprCompiler, joins: &mut Vec<String>) -> Result<Expr> {
        match e {
            Expr::Func(Func::Unnest, args) => {
                let item = c.dtype(&args[0])?.item().cloned().unwrap_or(DataType::Null);
                let lateral = self.d.unnest(&c.sql(&args[0])?, &item, &self.alias(), false, false);
                joins.push(lateral.join);
                Ok(Expr::Param(lateral.value, item))
            }
            e => e.map_children(&mut |child| self.lift_unnests(child, c, joins)),
        }
    }

    /// `s` as a derived table. `schema` is its output schema. Its ordering
    /// moves to the outer query when possible: a table stays sorted through
    /// later filters, projections and distincts, as in Ibis.
    fn wrap(&self, mut s: Select, schema: &Schema) -> Result<Select> {
        let order = s.output_order();
        if order.is_some() {
            s.clear_order();
        }
        let a = self.alias();
        let mut out = Select::aliased(format!("({}) AS {a}", s.render(self.d)), a);
        if let Some(keys) = order {
            self.push_order(&mut out, schema, keys)?;
        }
        Ok(out)
    }

    /// Sort `s` by `keys` (over `schema`, the columns of its FROM source),
    /// ahead of any ordering it already has: later sorts take precedence and
    /// earlier ones break ties, as in Ibis.
    fn push_order(&self, s: &mut Select, schema: &Schema, keys: Vec<SortKey>) -> Result<()> {
        let c = self.exprs(schema, false, &s.alias);
        let mut order: Vec<SortKey> = vec![];
        let mut sql = vec![];
        for k in keys {
            if !order.iter().any(|o| o.expr == k.expr) {
                sql.push(c.sort_key(&k)?);
                order.push(k);
            }
        }
        for (k, ksql) in std::mem::take(&mut s.order).into_iter().zip(std::mem::take(&mut s.order_sql)) {
            if !order.iter().any(|o| o.expr == k.expr) {
                order.push(k);
                sql.push(ksql);
            }
        }
        s.order = order;
        s.order_sql = sql;
        Ok(())
    }

    fn exprs<'s>(&'s self, schema: &'s Schema, in_project: bool, qual: &Option<String>) -> ExprCompiler<'s> {
        ExprCompiler { d: self.d, schema, in_project, qual: qual.clone(), rc: self }
    }

    /// Run `f` with `rels` (relation, alias) in scope.
    fn scoped<T>(&self, rels: &[(&Rel, &str)], f: impl FnOnce() -> Result<T>) -> Result<T> {
        let n = rels.len();
        self.scopes.borrow_mut().extend(rels.iter().map(|(r, a)| Scope {
            id: r.id,
            alias: a.to_string(),
            deref: r.deref_map(),
        }));
        let out = f();
        let mut sc = self.scopes.borrow_mut();
        let keep = sc.len() - n;
        sc.truncate(keep);
        out
    }

    /// Scope for expressions over `input`, compiled into `s`.
    fn in_scope<T>(&self, input: &Rel, alias: &Option<String>, f: impl FnOnce() -> Result<T>) -> Result<T> {
        match alias {
            Some(a) => self.scoped(&[(input, a)], f),
            None => f(),
        }
    }

    /// `alias."column"` for a column of relation `rel` visible in scope.
    /// A relation in scope under its own identity wins over one that merely
    /// derives from `rel`: in `a.join(a.filter(..))` both sides carry `a`'s
    /// columns, and `a.x` must mean the left side.
    fn field(&self, rel: &Rel, name: &str) -> Result<String> {
        let scopes = self.scopes.borrow();
        let q = |alias: &str, col: &str| format!("{alias}.{}", self.d.quote_ident(col));
        if let Some(sc) = scopes.iter().rev().find(|sc| sc.id == rel.id) {
            return Ok(q(&sc.alias, name));
        }
        for sc in scopes.iter().rev() {
            if let Some(c) = sc.deref.get(&(rel.id, name.to_string())) {
                return Ok(q(&sc.alias, c));
            }
        }
        Err(Error::Invalid(format!("column {name:?} belongs to a table that isn't part of this query")))
    }

    /// `query` after the views it reads. The query goes in a derived table
    /// because it may open with a `WITH` of its own.
    fn sql_query(&self, query: &str, views: &[(String, Arc<Rel>)]) -> Result<String> {
        if views.is_empty() {
            return Ok(query.to_string());
        }
        let ctes = views
            .iter()
            .map(|(name, view)| Ok(format!("{} AS ({})", self.d.quote_ident(name), self.rel(view)?.render(self.d))))
            .collect::<Result<Vec<_>>>()?;
        Ok(format!("WITH {} SELECT * FROM ({query}) AS ibis_sql", ctes.join(", ")))
    }

    /// A relation usable in a FROM clause under `alias`.
    fn source(&self, r: &Rel, alias: &str) -> Result<String> {
        match &r.op {
            RelOp::Table { name } => return Ok(format!("{} AS {alias}", self.d.quote_ident(name))),
            RelOp::Sql { query, views } => return Ok(format!("({}) AS {alias}", self.sql_query(query, views)?)),
            RelOp::View { input } | RelOp::Alias { input, .. } => return self.source(input, alias),
            _ => {}
        }
        Ok(format!("({}) AS {alias}", self.rel(r)?.render(self.d)))
    }

    fn rel(&self, r: &Rel) -> Result<Select> {
        let q = |s: &str| self.d.quote_ident(s);
        match &r.op {
            RelOp::Table { name } => {
                let a = self.alias();
                Ok(Select::aliased(format!("{} AS {a}", q(name)), a))
            }
            RelOp::Sql { query, views } => {
                let a = self.alias();
                Ok(Select::aliased(format!("({}) AS {a}", self.sql_query(query, views)?), a))
            }
            RelOp::View { input } | RelOp::Alias { input, .. } => self.rel(input),

            RelOp::Filter { input, predicates } => {
                let mut s = self.rel(input)?;
                if !s.is_plain() {
                    s = self.wrap(s, &input.schema)?;
                }
                let c = self.exprs(&input.schema, false, &s.alias);
                let wheres = self.in_scope(input, &s.alias, || {
                    predicates
                        .iter()
                        .map(|p| {
                            if contains_window(p) {
                                return unsupported(self.d, "window function in filter (mutate first)");
                            }
                            c.sql(p)
                        })
                        .collect::<Result<Vec<_>>>()
                })?;
                s.wheres.extend(wheres);
                Ok(s)
            }

            RelOp::Project { input, exprs } => {
                let mut s = self.rel(input)?;
                if !s.is_plain() {
                    s = self.wrap(s, &input.schema)?;
                }
                // where SELECT takes no set-returning function, an unnest is a
                // join whose column stands in for it
                let lateral = !self.d.unnest_in_select() && exprs.iter().any(|(_, e)| e.is_unnest());
                if lateral && s.alias.is_none() {
                    s = self.wrap(s, &input.schema)?;
                }
                let c = self.exprs(&input.schema, true, &s.alias);
                let mut joins = vec![];
                let items = self.in_scope(input, &s.alias, || {
                    exprs
                        .iter()
                        .map(|(n, e)| {
                            let sql = if lateral {
                                c.sql(&self.lift_unnests(e.clone(), &c, &mut joins)?)?
                            } else {
                                c.sql(e)?
                            };
                            Ok(format!("{sql} AS {}", q(n)))
                        })
                        .collect::<Result<Vec<_>>>()
                })?;
                for join in joins {
                    s.from = format!("{} {join}", s.from);
                }
                s.items = Some(items);
                for (n, e) in exprs {
                    if let Expr::Column(c) = e {
                        s.passthrough.entry(c.clone()).or_insert_with(|| n.clone());
                    }
                }
                Ok(s)
            }

            RelOp::Aggregate { input, groups, metrics } => {
                let mut s = self.rel(input)?;
                if !s.is_plain() {
                    s = self.wrap(s, &input.schema)?;
                }
                // row order doesn't survive aggregation (an ordering under a
                // LIMIT stayed inside the wrap above, where it picks the rows)
                s.clear_order();
                let c = self.exprs(&input.schema, false, &s.alias);
                let (items, group_by) = self.in_scope(input, &s.alias, || {
                    let (mut items, mut group_by) = (vec![], vec![]);
                    for (i, (n, e)) in groups.iter().enumerate() {
                        let sql = c.sql(e)?;
                        items.push(format!("{sql} AS {}", q(n)));
                        // A key that reads no column is named by its place in
                        // the select list: Postgres takes a constant in GROUP
                        // BY for such a place, and refuses a string there.
                        let constant = !e.has(&|e| !matches!(e, Expr::Literal(..) | Expr::Cast(..)));
                        group_by.push(if constant { (i + 1).to_string() } else { sql });
                    }
                    for (n, e) in metrics {
                        items.push(format!("{} AS {}", c.sql(e)?, q(n)));
                    }
                    Ok((items, group_by))
                })?;
                s.group_by = group_by;
                s.items = Some(items);
                Ok(s)
            }

            RelOp::Sort { input, keys } => {
                let mut s = self.rel(input)?;
                if !s.is_plain() {
                    s = self.wrap(s, &input.schema)?;
                }
                self.in_scope(input, &s.alias.clone(), || self.push_order(&mut s, &input.schema, keys.clone()))?;
                Ok(s)
            }

            RelOp::Limit { input, n, offset } => {
                let mut s = self.rel(input)?;
                if s.limit.is_some() || s.offset > 0 {
                    s = self.wrap(s, &input.schema)?;
                }
                s.limit = *n;
                s.offset = *offset;
                Ok(s)
            }

            RelOp::Distinct { input } => {
                let mut s = self.rel(input)?;
                // DISTINCT goes under the ordering: engines reject
                // `SELECT DISTINCT ... ORDER BY <expr not selected>`.
                let order = s.output_order();
                if order.is_some() {
                    s.clear_order();
                }
                if !s.group_by.is_empty() || s.distinct || s.limit.is_some() || s.offset > 0 || !s.order.is_empty() {
                    s = self.wrap(s, &input.schema)?;
                }
                s.distinct = true;
                if let Some(keys) = order {
                    s = self.wrap(s, &input.schema)?;
                    self.push_order(&mut s, &input.schema, keys)?;
                }
                Ok(s)
            }

            RelOp::Join { left, right, how, predicates, outputs } => {
                let (la, ra) = (self.alias(), self.alias());
                let col = |side: Side, c: &str| format!("{}.{}", if side == Side::Left { &la } else { &ra }, q(c));
                let c = self.exprs(&left.schema, false, &None);
                let cond = self.scoped(&[(left, &la), (right, &ra)], || {
                    predicates.iter().map(|p| c.sql(p)).collect::<Result<Vec<_>>>()
                })?;
                let cond = if cond.is_empty() { "TRUE".to_string() } else { cond.join(" AND ") };
                let items: Vec<String> =
                    outputs.iter().map(|(out, side, src)| format!("{} AS {}", col(*side, src), q(out))).collect();
                let (lsrc, rsrc) = (self.source(left, &la)?, self.source(right, &ra)?);
                let mut s = match how {
                    JoinKind::Semi | JoinKind::Anti => {
                        let not = if *how == JoinKind::Anti { "NOT " } else { "" };
                        let mut s = Select::aliased(lsrc, la.clone());
                        s.wheres.push(format!("{not}EXISTS (SELECT 1 FROM {rsrc} WHERE {cond})"));
                        s
                    }
                    // Every left row with its matches, then the right rows
                    // that have none.
                    JoinKind::Outer if !self.d.supports_full_outer_join() => {
                        let unmatched: Vec<String> = outputs
                            .iter()
                            .map(|(out, side, src)| match side {
                                Side::Left => format!("NULL AS {}", q(out)),
                                Side::Right => format!("{} AS {}", col(Side::Right, src), q(out)),
                            })
                            .collect();
                        let a = self.alias();
                        return Ok(Select::aliased(
                            format!(
                                "(SELECT {} FROM {lsrc} LEFT OUTER JOIN {rsrc} ON {cond} \
                                 UNION ALL SELECT {} FROM {rsrc} WHERE NOT EXISTS (SELECT 1 FROM {lsrc} WHERE {cond})) AS {a}",
                                items.join(", "),
                                unmatched.join(", ")
                            ),
                            a,
                        ));
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
                let body = if !distinct && *kind != SetKind::Union && !self.d.supports_intersect_except_all() {
                    // Number each row among its duplicates on both sides; the
                    // DISTINCT operator over (row, number) then keeps or
                    // cancels duplicates one for one, which is what ALL means.
                    let cols = left.schema.names().map(q).collect::<Vec<_>>().join(", ");
                    // The rows of a partition are all alike, so the order
                    // within one means nothing: it is there for the engines
                    // that will not number rows without one.
                    let first = left.schema.names().next().map(q).unwrap_or_default();
                    let n = self.d.quote_ident("_n");
                    let numbered = |src: &str| {
                        format!(
                            "SELECT {cols}, ROW_NUMBER() OVER (PARTITION BY {cols} ORDER BY {first}) AS {n} FROM {src}"
                        )
                    };
                    let kw = kw.trim_end_matches(" ALL");
                    format!("SELECT {cols} FROM ({} {kw} {}) AS {}", numbered(&l), numbered(&r), self.alias())
                } else {
                    format!("SELECT * FROM {l} {kw} SELECT * FROM {r}")
                };
                let a = self.alias();
                Ok(Select::aliased(format!("({body}) AS {a}"), a))
            }

            RelOp::Unnest { input, column, offset, keep_empty } => {
                let inner = self.rel(input)?;
                let mut s = self.wrap(inner, &input.schema)?;
                let t = s.alias.clone().expect("a wrapped select has an alias");
                let u = self.alias();
                let item = input.schema.field(column).and_then(|f| f.dtype.item()).cloned().unwrap_or(DataType::Null);
                let lateral = self.d.unnest(&format!("{t}.{}", q(column)), &item, &u, offset.is_some(), *keep_empty);
                let mut items: Vec<String> =
                    input
                        .schema
                        .names()
                        .map(|n| {
                            if n == column { format!("{} AS {}", lateral.value, q(n)) } else { format!("{t}.{}", q(n)) }
                        })
                        .collect();
                if let Some(name) = offset {
                    let bigint = self.d.cast_type_name(&DataType::Int64)?;
                    items.push(format!("CAST({} AS {bigint}) AS {}", lateral.offset, q(name)));
                }
                s.from = format!("{} {}", s.from, lateral.join);
                s.items = Some(items);
                // the ordering of the input is over columns that are still here
                // under the same names, apart from the array itself
                for n in input.schema.names().filter(|n| n != column) {
                    s.passthrough.insert(n.to_string(), n.to_string());
                }
                Ok(s)
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
    /// Alias of the relation `schema` describes; columns are spelled
    /// `alias."name"` so they can't be captured by a same-named output alias.
    qual: Option<String>,
    rc: &'a RelCompiler<'a>,
}

impl ExprCompiler<'_> {
    pub fn dtype(&self, e: &Expr) -> Result<DataType> {
        e.dtype(self.schema)
    }

    pub fn sql(&self, e: &Expr) -> Result<String> {
        let d = self.d;
        Ok(match e {
            Expr::Column(n) => match &self.qual {
                Some(a) => format!("{a}.{}", d.quote_ident(n)),
                None => d.quote_ident(n),
            },
            Expr::Field { rel, name } => self.rc.field(rel, name)?,
            Expr::ScalarSubquery(r) => format!("({})", self.rc.rel(r)?.render(d)),
            Expr::Exists(r) => format!("(EXISTS ({}))", self.rc.rel(r)?.render(d)),
            Expr::InColumn { .. } => {
                return Err(Error::Invalid("isin_col outside a table operation (select, filter, ...)".into()));
            }
            Expr::InSubquery { arg, rel } => {
                // engines widen a number to compare it, but not the fields of a struct
                let column = rel.schema.fields.first().map(|f| &f.dtype);
                let needle = match column {
                    Some(t) if t.is_nested() && self.dtype(arg)? != *t => {
                        self.sql(&arg.as_ref().clone().cast(t.clone()))?
                    }
                    _ => self.sql(arg)?,
                };
                let needle = match column {
                    Some(t) => d.in_subquery_needle(needle, t),
                    None => needle,
                };
                format!("({needle} IN ({}))", self.rc.rel(rel)?.render(d))
            }
            Expr::Literal(v, t) => self.literal(v, t)?,
            Expr::Alias(e, _) => self.sql(e)?,
            Expr::Cast(e, t) => match d.cast(self, e, t)? {
                Some(s) => s,
                None => format!("CAST({} AS {})", self.sql(e)?, d.cast_type_name(t)?),
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
            Expr::Binary(op, l, r) => {
                // `+` and `*` on arrays and maps are functions
                if let Some(call) = ibis_core::expr::nested_binary(*op, l, r, &self.dtype(l)?, &self.dtype(r)?) {
                    return self.sql(&call);
                }
                match d.binary(self, *op, l, r)? {
                    Some(s) => s,
                    None => self.default_binary(*op, l, r)?,
                }
            }
            Expr::Func(f, args) => {
                let first = args.first().map(|a| self.dtype(a)).transpose()?;
                let f = f.resolve(first.as_ref());
                match d.func(self, f, args)? {
                    Some(s) => s,
                    None => self.default_func(f, args)?,
                }
            }
            Expr::Agg { func, arg, filter, options } => {
                // in a projection a bare reduction is broadcast over all rows
                if let (true, AggFunc::First | AggFunc::Last, false) =
                    (self.in_project, func, options.order_by.is_empty())
                {
                    // read in an order, it is the first value of an ordered window
                    return self.sql(&e.clone().over(WindowSpec::new()));
                }
                let over = if self.in_project { Some("OVER ()") } else { None };
                self.agg(*func, arg.as_deref(), filter.as_deref(), options, over)?
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
            Expr::Param(name, _) => name.clone(),
            Expr::ArrayLambda { kind, arg, param, index, .. } => {
                let body = e.lambda_body(self.schema)?;
                match d.array_lambda(self, *kind, arg, param, index.as_deref(), &body)? {
                    Some(s) => s,
                    None => self.default_array_lambda(*kind, arg, param, index.as_deref(), body)?,
                }
            }
        })
    }

    /// An aggregate call with its FILTER and OVER clauses. Both must follow
    /// the call itself, inside anything a dialect wraps around it.
    fn agg(
        &self,
        func: AggFunc,
        arg: Option<&Expr>,
        filter: Option<&Expr>,
        options: &AggOptions,
        over: Option<&str>,
    ) -> Result<String> {
        if func == AggFunc::CountDistinctStar {
            return self.count_distinct_rows(filter, over);
        }
        // `collect`, `first` and `last` leave NULLs out unless told to keep
        // them: one more condition on the rows they see
        let present;
        let filter = match (func, arg) {
            (AggFunc::Collect | AggFunc::First | AggFunc::Last, Some(a)) if !options.include_null => {
                present = match filter {
                    Some(f) => a.clone().notnull().and(f.clone()),
                    None => a.clone().notnull(),
                };
                Some(&present)
            }
            _ => filter,
        };
        match (options.is_plain(), arg) {
            (false, Some(a)) => self.reduction(func, a, filter, options, over),
            _ => self.filtered_agg(func, arg, filter, over),
        }
    }

    /// The number of distinct rows of the relation (`Dialect::count_distinct_rows`).
    fn count_distinct_rows(&self, filter: Option<&Expr>, over: Option<&str>) -> Result<String> {
        let columns = self.schema.names().map(|name| self.sql(&ibis_core::prelude::col(name)));
        let columns = columns.collect::<Result<Vec<_>>>()?;
        // the condition goes in FILTER where there is one, else to the dialect
        let (mut tail, mut condition) = (String::new(), None);
        if let Some(f) = filter {
            let f = self.sql(f)?;
            match self.d.supports_agg_filter(over.is_some()) {
                true => tail = format!(" FILTER (WHERE {f})"),
                false => condition = Some(f),
            }
        }
        if let Some(o) = over {
            tail.push(' ');
            tail.push_str(o);
        }
        match self.d.count_distinct_rows(&columns, condition.as_deref(), &tail)? {
            Some(s) => Ok(s),
            None => unsupported(self.d, "count_distinct_star"),
        }
    }

    /// A reduction with more than an argument (`Dialect::reduction`).
    fn reduction(
        &self,
        func: AggFunc,
        arg: &Expr,
        filter: Option<&Expr>,
        options: &AggOptions,
        over: Option<&str>,
    ) -> Result<String> {
        use AggFunc::*;
        let (mut arg, mut options) = (arg.clone(), options.clone());
        let pair = matches!(func, CovSamp | CovPop | CorrSamp | CorrPop);
        if pair {
            // the covariance of booleans is that of 0 and 1
            for e in [&mut arg, &mut options.args[0]] {
                if self.dtype(e)? == DataType::Boolean {
                    *e = e.clone().cast(DataType::Int32);
                }
            }
        }
        let mut tail = String::new();
        if let Some(f) = filter {
            if self.d.supports_agg_filter(over.is_some()) {
                tail = format!(" FILTER (WHERE {})", self.sql(f)?);
            } else {
                if options.include_null {
                    // a row failing the condition would be a NULL that counts
                    return unsupported(self.d, format!("{} of NULLs with a condition", snake(&format!("{func:?}"))));
                }
                // rows failing the condition become NULL in what the reduction reads
                let when = |then: Expr| Expr::Case { branches: vec![(f.clone(), then)], default: None };
                arg = when(arg);
                if pair || matches!(func, ArgMin | ArgMax) {
                    options.args[0] = when(options.args[0].clone());
                }
            }
        }
        if let Some(o) = over {
            tail.push(' ');
            tail.push_str(o);
        }
        match self.d.reduction(self, func, &arg, &options, &tail)? {
            Some(s) => Ok(s),
            None => unsupported(self.d, snake(&format!("{func:?}"))),
        }
    }

    fn filtered_agg(
        &self,
        func: AggFunc,
        arg: Option<&Expr>,
        filter: Option<&Expr>,
        over: Option<&str>,
    ) -> Result<String> {
        if let Some(f) = filter
            && !self.d.supports_agg_filter(over.is_some())
        {
            // rows failing the condition become NULL, which aggregates skip
            let when = |then: Expr| Expr::Case { branches: vec![(f.clone(), then)], default: None };
            let (func, arg) = match arg {
                Some(a) if func != AggFunc::CountStar => (func, when(a.clone())),
                _ => (AggFunc::Count, when(Expr::Literal(Value::Int(1), DataType::Int8))),
            };
            return self.filtered_agg(func, Some(&arg), None, over);
        }
        let mut tail = match filter {
            Some(f) => format!(" FILTER (WHERE {})", self.sql(f)?),
            None => String::new(),
        };
        if let Some(o) = over {
            tail.push(' ');
            tail.push_str(o);
        }
        match self.d.agg(self, func, arg, &tail)? {
            Some(s) => Ok(s),
            None => self.default_agg(func, arg, &tail),
        }
    }

    pub fn sort_key(&self, k: &SortKey) -> Result<String> {
        Ok(self.d.sort_key(&self.sql(&k.expr)?, k.ascending, k.nulls_first))
    }

    pub fn literal(&self, v: &Value, t: &DataType) -> Result<String> {
        if let Some(s) = self.d.literal(v, t)? {
            return Ok(s);
        }
        Ok(match v {
            Value::Null if *t == DataType::Null => "NULL".into(),
            Value::Null => format!("CAST(NULL AS {})", self.d.cast_type_name(t)?),
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
                self.d.cast_type_name(&DataType::Float64)?
            ),
            Value::String(s) => quote_str(s),
            Value::Binary(_) => return unsupported(self.d, "binary literal"),
            Value::Date(days) => format!("DATE '{}'", ibis_core::convert::format_date(*days)),
            Value::Timestamp(us) => format!("TIMESTAMP '{}'", ibis_core::convert::format_timestamp(*us)),
            Value::Array(_) | Value::Struct(_) | Value::Map(_) => match self.d.nested_literal(self, v, t)? {
                Some(s) => s,
                None => self.nested_literal(v, t)?,
            },
        })
    }

    fn float(&self) -> Result<String> {
        self.d.cast_type_name(&DataType::Float64)
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
                    self.d.cast_type_name(&DataType::Int64)?
                )
            }
            FloorDiv => format!("CAST(FLOOR({ls} / {rs}) AS {})", self.d.cast_type_name(&DataType::Int64)?),
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
            Xor => format!("(({ls} OR {rs}) AND NOT ({ls} AND {rs}))"),
        })
    }

    /// Literal string argument, if the expression is one.
    pub fn lit_str(e: &Expr) -> Option<&str> {
        match e {
            Expr::Literal(Value::String(s), _) => Some(s),
            _ => None,
        }
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
        let int =
            |s: String| -> Result<String> { Ok(format!("CAST({s} AS {})", self.d.cast_type_name(&DataType::Int32)?)) };
        let bigint =
            |s: String| -> Result<String> { Ok(format!("CAST({s} AS {})", self.d.cast_type_name(&DataType::Int64)?)) };
        let to_int64 = args.first().is_some_and(|x| self.dtype(x).map(|t| !t.is_integer()).unwrap_or(false));
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
            DayOfWeekIndex => {
                format!("CAST(EXTRACT(ISODOW FROM {x}) - 1 AS {})", self.d.cast_type_name(&DataType::Int16)?)
            }
            IsNan => format!("ISNAN({x})"),
            DayOfYear => int(format!("EXTRACT(DOY FROM {x})"))?,
            Quarter => int(format!("EXTRACT(QUARTER FROM {x})"))?,
            // EXTRACT(MILLISECOND) includes the seconds: 5.359s is 5359
            Millisecond => int(format!("{} % 1000", bigint(format!("FLOOR(EXTRACT(MILLISECOND FROM {x}))"))?))?,
            Microsecond => int(format!("{} % 1000000", bigint(format!("FLOOR(EXTRACT(MICROSECOND FROM {x}))"))?))?,
            WeekOfYear => int(format!("EXTRACT(WEEK FROM {x})"))?,
            DayOfWeekName => format!("DAYNAME({x})"),
            Truncate => {
                let unit = match Self::lit_str(&args[1]) {
                    Some("Y") => "year",
                    Some("Q") => "quarter",
                    Some("M") => "month",
                    Some("W") => "week",
                    Some("D") => "day",
                    Some("h") => "hour",
                    Some("m") => "minute",
                    Some("s") => "second",
                    Some("ms") => "milliseconds",
                    Some("us") => "microseconds",
                    _ => return unsupported(self.d, "truncate with a non-literal or unknown unit"),
                };
                let s = format!("DATE_TRUNC('{unit}', {x})");
                // DATE_TRUNC of a date gives a timestamp on some engines
                if self.dtype(&args[0])? == DataType::Date { format!("CAST({s} AS DATE)") } else { s }
            }
            Strftime => format!("STRFTIME({x}, {})", a[1]),
            Least => format!("LEAST({})", a.join(", ")),
            Greatest => format!("GREATEST({})", a.join(", ")),
            // spelled with LN so it doesn't depend on LOG's argument order
            // or on which numeric types an engine's LOG accepts
            Log => format!("(LN({x}) / LN({}))", a[1]),
            Radians => format!("RADIANS({x})"),
            Degrees => format!("DEGREES({x})"),
            Sin => format!("SIN({x})"),
            Cos => format!("COS({x})"),
            Tan => format!("TAN({x})"),
            Asin => format!("ASIN({x})"),
            Acos => format!("ACOS({x})"),
            Atan => format!("ATAN({x})"),
            Atan2 => format!("ATAN2({x}, {})", a[1]),
            Cot => format!("COT({x})"),
            IsInf => format!("ISINF({x})"),
            Clip => {
                let open = |e: &Expr| matches!(e, Expr::Literal(Value::Null, _));
                let mut s = x.clone();
                if !open(&args[1]) {
                    s = format!("GREATEST({s}, {})", a[1]);
                }
                if !open(&args[2]) {
                    s = format!("LEAST({s}, {})", a[2]);
                }
                // LEAST/GREATEST skip NULLs on some engines; a NULL input must stay NULL
                let t = self.d.cast_type_name(&self.dtype(&args[0])?)?;
                format!("CASE WHEN {x} IS NULL THEN NULL ELSE CAST({s} AS {t}) END")
            }
            DateFromYmd => format!("MAKE_DATE({})", a.join(", ")),
            TimestampFromYmdhms => format!("MAKE_TIMESTAMP({})", a.join(", ")),
            nested => self.nested_func(nested, args, &a)?,
        })
    }

    pub fn default_agg(&self, f: AggFunc, arg: Option<&Expr>, filter: &str) -> Result<String> {
        let x = match arg {
            Some(a) if self.dtype(a)? == DataType::Boolean && matches!(f, AggFunc::Sum | AggFunc::Mean) => {
                format!("CAST({} AS {})", self.sql(a)?, self.d.cast_type_name(&DataType::Int64)?)
            }
            Some(a) => self.sql(a)?,
            None => String::new(),
        };
        use AggFunc::*;
        let call = match f {
            CountStar => "COUNT(*)".into(),
            CountDistinctStar => return unsupported(self.d, "count_distinct_star"),
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
            Collect => format!("ARRAY_AGG({x})"),
            // each engine names these its own way, or has none
            Mode | KurtosisSamp | KurtosisPop | ApproxCountDistinct | BitAnd | BitOr | BitXor | Arbitrary | Median
            | ApproxMedian | First | Last | ArgMin | ArgMax | CovSamp | CovPop | CorrSamp | CorrPop | Quantile
            | ApproxQuantile | GroupConcat => {
                return unsupported(self.d, snake(&format!("{f:?}")));
            }
        };
        Ok(format!("{call}{filter}"))
    }

    /// The reductions of two columns under their SQL-standard names
    /// (`COVAR_SAMP`, `COVAR_POP`, `CORR`), for a dialect's `reduction`.
    /// The standard's `CORR` is the population's.
    pub fn standard_reduction(
        &self,
        f: AggFunc,
        arg: &Expr,
        options: &AggOptions,
        tail: &str,
    ) -> Result<Option<String>> {
        let name = match f {
            AggFunc::CovSamp => "COVAR_SAMP",
            AggFunc::CovPop => "COVAR_POP",
            AggFunc::CorrPop => "CORR",
            _ => return Ok(None),
        };
        Ok(Some(format!("{name}({}, {}){tail}", self.sql(arg)?, self.sql(&options.args[0])?)))
    }

    /// `PERCENTILE_CONT(q) WITHIN GROUP (ORDER BY arg)`: the SQL-standard
    /// quantile, for a dialect's `reduction`. What cannot be interpolated
    /// (text, dates) gets `PERCENTILE_DISC`, one of the values.
    pub fn percentile(&self, arg: &Expr, options: &AggOptions, tail: &str) -> Result<String> {
        let name = if self.dtype(arg)?.is_numeric() { "PERCENTILE_CONT" } else { "PERCENTILE_DISC" };
        Ok(format!("{name}({}) WITHIN GROUP (ORDER BY {}){tail}", self.sql(&options.args[0])?, self.sql(arg)?))
    }

    /// ` ORDER BY ...` inside an aggregate's parentheses, for the order a
    /// reduction reads its rows in; empty when it has none.
    pub fn agg_order(&self, options: &AggOptions) -> Result<String> {
        if options.order_by.is_empty() {
            return Ok(String::new());
        }
        let keys = options.order_by.iter().map(|k| self.sort_key(k)).collect::<Result<Vec<_>>>()?;
        Ok(format!(" ORDER BY {}", keys.join(", ")))
    }

    /// `[DISTINCT ]arg[ ORDER BY ...]`: what goes inside the parentheses of
    /// an aggregate that reads distinct values, or reads them in an order.
    pub fn ordered(&self, arg: &Expr, options: &AggOptions) -> Result<String> {
        let distinct = if options.distinct { "DISTINCT " } else { "" };
        Ok(format!("{distinct}{}{}", self.sql(arg)?, self.agg_order(options)?))
    }

    /// `STRING_AGG(arg, separator ORDER BY ...)`, for a dialect's `reduction`.
    pub fn string_agg(&self, arg: &Expr, options: &AggOptions, tail: &str) -> Result<String> {
        let (x, sep) = (self.sql(arg)?, self.sql(&options.args[0])?);
        Ok(format!("STRING_AGG({x}, {sep}{}){tail}", self.agg_order(options)?))
    }

    /// Refuse an order the engine's form of a reduction cannot take.
    pub fn unordered(&self, options: &AggOptions, what: &str) -> Result<()> {
        match options.order_by.is_empty() {
            true => Ok(()),
            false => unsupported(self.d, format!("{what} with an order")),
        }
    }

    /// The fractions of a quantile, where the engine wants them as numbers
    /// in its own syntax, and whether they were given as an array.
    pub fn fractions(&self, options: &AggOptions) -> Result<(Vec<f64>, bool)> {
        match options.args.first() {
            Some(Expr::Literal(Value::Float(q), _)) => Ok((vec![*q], false)),
            Some(Expr::Literal(Value::Array(qs), _)) => {
                let qs = qs.iter().filter_map(|q| if let Value::Float(q) = q { Some(*q) } else { None });
                Ok((qs.collect(), true))
            }
            _ => unsupported(self.d, "a quantile that is not a literal"),
        }
    }

    /// `arg ORDER BY key`, smallest key first (or largest), NULL keys last:
    /// what an ordered aggregate reads to find `argmin` or `argmax`.
    pub fn by_key(&self, arg: &Expr, key: &Expr, smallest: bool) -> Result<String> {
        let key = SortKey { expr: key.clone(), ascending: smallest, nulls_first: false };
        Ok(format!("{} ORDER BY {}", self.sql(arg)?, self.sort_key(&key)?))
    }

    /// `NAME(arg)` and what follows the call: for a dialect's `agg`.
    pub fn call_agg(&self, name: &str, arg: &Expr, tail: &str) -> Result<Option<String>> {
        Ok(Some(format!("{name}({}){tail}", self.sql(arg)?)))
    }

    fn window(&self, f: &Expr, spec: &WindowSpec) -> Result<String> {
        // Ibis: a window with no frame covers the whole partition, ordered
        // or not. SQL's default with ORDER BY is a running frame, so spell
        // the frame out. Ranking and offset functions take no frame at all.
        let framed = match f {
            Expr::Analytic(a, _) => matches!(a, Analytic::FirstValue | Analytic::LastValue | Analytic::NthValue),
            _ => true,
        };
        let mut spec = spec.clone();
        if !framed {
            spec.frame = None;
        } else if spec.frame.is_none() && !spec.order_by.is_empty() {
            spec = spec.rows(None, None);
        }
        let over = self.over(&spec)?;
        // compile the function without the broadcast `OVER ()`
        let inner =
            ExprCompiler { d: self.d, schema: self.schema, in_project: false, qual: self.qual.clone(), rc: self.rc };
        Ok(match f {
            Expr::Agg { func, arg, filter, options } => {
                inner.agg(*func, arg.as_deref(), filter.as_deref(), options, Some(&over))?
            }
            Expr::Analytic(a, args) => {
                let a_sql = args.iter().map(|x| inner.sql(x)).collect::<Result<Vec<_>>>()?;
                if let Some(s) = self.d.analytic(*a, &a_sql, &over)? {
                    return Ok(s);
                }
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
                    Ntile => format!("(NTILE({}) {over} - 1)", a_sql[0]),
                    // Ibis counts from 0. A literal index stays a literal: MySQL takes nothing else.
                    NthValue => match a_sql[1].parse::<i64>() {
                        Ok(n) => format!("NTH_VALUE({}, {}) {over}", a_sql[0], n + 1),
                        Err(_) => format!("NTH_VALUE({}, {} + 1) {over}", a_sql[0], a_sql[1]),
                    },
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
