//! Relational IR (`Rel`) and the `Table` API that builds it.
//!
//! Every `Rel` node carries its output schema, computed (and validated) when
//! the node is built, so schema errors surface at the call that caused them.
//!
//! ## Binding (Ibis "dereferencing")
//!
//! Expressions can name columns two ways: unbound (`col("x")`, Ibis `_.x`),
//! resolved against whatever table they're applied to, or bound to a table
//! (`t.col("x")`, Ibis `t.x`). When a table operation receives a bound column
//! it rewrites it to a column of its input if the column's table *is* the
//! input or one of its ancestors (through filters, projections, joins, ...).
//!
//! What can't be resolved that way refers to some other table, and becomes a
//! subquery, following Ibis:
//!
//! - a reduction in a filter predicate, or over another table:
//!   `ScalarSubquery(that_table.aggregate(reduction))`
//! - `.any()` over another table: `Exists(that_table.filter(predicate))`
//! - a column of a one-row aggregate: `ScalarSubquery(agg.select(column))`
//! - any other column of another table stays a `Field`: a correlated
//!   reference, resolved when the enclosing query is compiled.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::datatypes::{Field, Schema};
use crate::error::{Result, invalid};
use crate::expr::{AggFunc, BinaryOp, Expr, SortKey};

/// Every relation node gets a unique id: it's the node's identity, used to
/// tell `t` from `t.view()` and to resolve bound columns.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct Rel {
    pub id: u64,
    pub op: RelOp,
    pub schema: Schema,
}

impl PartialEq for Rel {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RelOp {
    /// A named table in the backend's catalog.
    Table {
        name: String,
    },
    /// Same rows as `input` under a new identity (Ibis `t.view()`), for self-joins.
    View {
        input: Arc<Rel>,
    },
    /// Output exactly these named expressions, in order.
    Project {
        input: Arc<Rel>,
        exprs: Vec<(String, Expr)>,
    },
    /// Keep rows where all predicates are true.
    Filter {
        input: Arc<Rel>,
        predicates: Vec<Expr>,
    },
    /// Group by `groups` (empty = whole relation) and compute `metrics`.
    /// Output columns: groups, then metrics.
    Aggregate {
        input: Arc<Rel>,
        groups: Vec<(String, Expr)>,
        metrics: Vec<(String, Expr)>,
    },
    Sort {
        input: Arc<Rel>,
        keys: Vec<SortKey>,
    },
    Limit {
        input: Arc<Rel>,
        n: Option<u64>,
        offset: u64,
    },
    Distinct {
        input: Arc<Rel>,
    },
    /// Join on `predicates`, whose columns are `Field`s of `left` or `right`
    /// (or of an enclosing query). `outputs` lists the output columns as
    /// (output name, side, source column).
    Join {
        left: Arc<Rel>,
        right: Arc<Rel>,
        how: JoinKind,
        predicates: Vec<Expr>,
        outputs: Vec<(String, Side, String)>,
    },
    SetOp {
        kind: SetKind,
        distinct: bool,
        left: Arc<Rel>,
        right: Arc<Rel>,
    },
    /// One row per element of the array column `column`, which becomes the
    /// element; the other columns repeat. `offset` names a new last column
    /// holding the element's 0-based position. An empty or NULL array gives
    /// no rows, or with `keep_empty` one row with a NULL element.
    Unnest {
        input: Arc<Rel>,
        column: String,
        offset: Option<String>,
        keep_empty: bool,
    },
    /// The same rows, given a name that the SQL of a later `Sql` can read
    /// them by (Ibis `t.alias(name)`).
    Alias {
        input: Arc<Rel>,
        name: String,
    },
    /// A query in the backend's own SQL (`Backend::sql`, `Backend::sql_over`).
    /// It may read each of `views` as a table of that name.
    Sql {
        query: String,
        views: Vec<(String, Arc<Rel>)>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinKind {
    Inner,
    Left,
    Right,
    Outer,
    Semi,
    Anti,
}

/// Which rows `Table::drop_null` drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// Rows where any of the columns is NULL.
    Any,
    /// Rows where all of the columns are NULL.
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetKind {
    Union,
    Intersect,
    Difference,
}

/// (relation id, column) -> column of the relation the map was built for.
pub type DerefMap = HashMap<(u64, String), String>;

impl Rel {
    fn new(op: RelOp, schema: Schema) -> Rel {
        Rel { id: NEXT_ID.fetch_add(1, Ordering::Relaxed), op, schema }
    }

    /// This relation with each parameter of `values` replaced by its
    /// value, everywhere under it. Relations keep their ids, so a column
    /// bound to one of them (`t.col("x")`) still finds it.
    pub fn bind(self: &Arc<Rel>, values: &[(Expr, Expr)]) -> Result<Arc<Rel>> {
        let e = |e: &Expr| e.clone().bind(values);
        let named = |v: &[(String, Expr)]| v.iter().map(|(n, x)| Ok((n.clone(), e(x)?))).collect::<Result<Vec<_>>>();
        let all = |v: &[Expr]| v.iter().map(e).collect::<Result<Vec<_>>>();
        let r = |r: &Arc<Rel>| r.bind(values);
        let op = match &self.op {
            RelOp::Table { .. } => return Ok(self.clone()),
            RelOp::Sql { query, views } => RelOp::Sql {
                query: query.clone(),
                views: views.iter().map(|(n, v)| Ok((n.clone(), r(v)?))).collect::<Result<_>>()?,
            },
            RelOp::View { input } => RelOp::View { input: r(input)? },
            RelOp::Alias { input, name } => RelOp::Alias { input: r(input)?, name: name.clone() },
            RelOp::Project { input, exprs } => RelOp::Project { input: r(input)?, exprs: named(exprs)? },
            RelOp::Filter { input, predicates } => RelOp::Filter { input: r(input)?, predicates: all(predicates)? },
            RelOp::Aggregate { input, groups, metrics } => {
                RelOp::Aggregate { input: r(input)?, groups: named(groups)?, metrics: named(metrics)? }
            }
            RelOp::Sort { input, keys } => RelOp::Sort {
                input: r(input)?,
                keys: keys.iter().map(|k| Ok(SortKey { expr: e(&k.expr)?, ..k.clone() })).collect::<Result<_>>()?,
            },
            RelOp::Limit { input, n, offset } => RelOp::Limit { input: r(input)?, n: *n, offset: *offset },
            RelOp::Distinct { input } => RelOp::Distinct { input: r(input)? },
            RelOp::Join { left, right, how, predicates, outputs } => RelOp::Join {
                left: r(left)?,
                right: r(right)?,
                how: *how,
                predicates: all(predicates)?,
                outputs: outputs.clone(),
            },
            RelOp::SetOp { kind, distinct, left, right } => {
                RelOp::SetOp { kind: *kind, distinct: *distinct, left: r(left)?, right: r(right)? }
            }
            RelOp::Unnest { input, column, offset, keep_empty } => RelOp::Unnest {
                input: r(input)?,
                column: column.clone(),
                offset: offset.clone(),
                keep_empty: *keep_empty,
            },
        };
        Ok(Arc::new(Rel { id: self.id, op, schema: self.schema.clone() }))
    }

    pub fn inputs(&self) -> Vec<&Arc<Rel>> {
        match &self.op {
            RelOp::Table { .. } => vec![],
            RelOp::Sql { views, .. } => views.iter().map(|(_, view)| view).collect(),
            RelOp::View { input }
            | RelOp::Alias { input, .. }
            | RelOp::Project { input, .. }
            | RelOp::Filter { input, .. }
            | RelOp::Aggregate { input, .. }
            | RelOp::Sort { input, .. }
            | RelOp::Limit { input, .. }
            | RelOp::Unnest { input, .. }
            | RelOp::Distinct { input } => vec![input],
            RelOp::Join { left, right, .. } | RelOp::SetOp { left, right, .. } => vec![left, right],
        }
    }

    /// Which columns of this relation carry which columns of its ancestors
    /// unchanged. A `View` starts a new lineage, so `t` and `t.view()` never
    /// resolve to each other.
    pub fn deref_map(&self) -> DerefMap {
        let mut m: DerefMap = self.schema.names().map(|n| ((self.id, n.to_string()), n.to_string())).collect();
        let mut inherit = |input: &Rel, renames: &mut dyn Iterator<Item = (&str, &str)>| {
            let im = input.deref_map();
            for (out, src) in renames {
                for (k, v) in &im {
                    if v == src {
                        m.entry(k.clone()).or_insert_with(|| out.to_string());
                    }
                }
            }
        };
        match &self.op {
            RelOp::Table { .. } | RelOp::Sql { .. } | RelOp::View { .. } | RelOp::SetOp { .. } => {}
            RelOp::Filter { input, .. }
            | RelOp::Alias { input, .. }
            | RelOp::Sort { input, .. }
            | RelOp::Limit { input, .. }
            | RelOp::Distinct { input } => inherit(input, &mut input.schema.names().map(|n| (n, n))),
            RelOp::Unnest { input, column, .. } => {
                inherit(input, &mut input.schema.names().filter(|n| n != column).map(|n| (n, n)))
            }
            RelOp::Project { input, exprs: cols } | RelOp::Aggregate { input, groups: cols, .. } => inherit(
                input,
                &mut cols.iter().filter_map(|(n, e)| match e {
                    Expr::Column(c) => Some((n.as_str(), c.as_str())),
                    _ => None,
                }),
            ),
            RelOp::Join { left, right, outputs, .. } => {
                for (side, input) in [(Side::Left, left), (Side::Right, right)] {
                    inherit(
                        input,
                        &mut outputs.iter().filter(|o| o.1 == side).map(|(o, _, s)| (o.as_str(), s.as_str())),
                    )
                }
            }
        }
        m
    }

    /// An `Aggregate` with no grouping keys: exactly one row.
    fn is_scalar(&self) -> bool {
        matches!(&self.op, RelOp::Aggregate { groups, .. } if groups.is_empty())
    }
}

/// A lazy table expression. Cheap to clone; building never touches a backend.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    rel: Arc<Rel>,
}

/// Where an expression is used; reductions mean different things in each.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    /// Filter predicates: every reduction becomes a scalar subquery.
    Filter,
    /// Projections, group keys, metrics, sort keys: reductions over the
    /// input stay (aggregates, or broadcast windows in a projection).
    Value,
}

/// A join condition: an expression, or a pair of column names (left, right).
pub enum JoinPred {
    Expr(Expr),
    Keys(String, String),
}

impl From<Expr> for JoinPred {
    fn from(e: Expr) -> Self {
        JoinPred::Expr(e)
    }
}
impl From<(&str, &str)> for JoinPred {
    fn from((l, r): (&str, &str)) -> Self {
        JoinPred::Keys(l.into(), r.into())
    }
}
impl From<&(&str, &str)> for JoinPred {
    fn from((l, r): &(&str, &str)) -> Self {
        JoinPred::Keys(l.to_string(), r.to_string())
    }
}
impl From<&str> for JoinPred {
    fn from(k: &str) -> Self {
        JoinPred::Keys(k.into(), k.into())
    }
}

fn named(exprs: impl IntoIterator<Item = impl Into<Expr>>) -> Vec<(String, Expr)> {
    exprs
        .into_iter()
        .map(|e| {
            let e = e.into();
            let name = e.output_name();
            let inner = e.unaliased().clone();
            (name, inner)
        })
        .collect()
}

/// A column still bound to another table after binding, if there is one.
fn foreign_column(e: &Expr) -> Option<&str> {
    match e {
        Expr::Field { name, .. } => Some(name),
        e => e.children().into_iter().find_map(foreign_column),
    }
}

fn check_unique(fields: &[Field]) -> Result<()> {
    for (i, f) in fields.iter().enumerate() {
        if fields[..i].iter().any(|g| g.name == f.name) {
            return invalid(format!("duplicate column name {:?}", f.name));
        }
    }
    Ok(())
}

impl Table {
    /// An unbound table reference with a known schema (Ibis `ibis.table(schema, name)`).
    pub fn named(name: &str, schema: Schema) -> Table {
        Table::from_rel(Rel::new(RelOp::Table { name: name.to_string() }, schema))
    }

    fn from_rel(rel: Rel) -> Table {
        Table { rel: Arc::new(rel) }
    }

    pub fn from_arc(rel: Arc<Rel>) -> Table {
        Table { rel }
    }

    /// A query in a backend's own SQL, whose result has `schema`. Backends
    /// build this in `Backend::sql`, which reads the schema from the engine.
    pub fn sql(query: &str, mut schema: Schema) -> Table {
        schema.fields.iter_mut().for_each(|f| f.nullable = true);
        Table::from_rel(Rel::new(RelOp::Sql { query: query.to_string(), views: vec![] }, schema))
    }

    /// `Table::sql` for a query that reads the tables this expression names
    /// with [`Table::alias`]. Backends build this in `Backend::sql_over`.
    pub fn sql_over(&self, query: &str, schema: Schema) -> Result<Table> {
        Ok(self.wrap(RelOp::Sql { query: query.to_string(), views: self.aliases()? }, schema))
    }

    /// The same rows under a name: the SQL given to `Backend::sql_over` for
    /// this table, or for one built on it, reads them as a table of that
    /// name (Ibis `t.alias(name)`).
    pub fn alias(&self, name: &str) -> Table {
        self.wrap(RelOp::Alias { input: self.rel.clone(), name: name.to_string() }, self.schema().clone())
    }

    /// Every table named with [`Table::alias`] in this expression, the ones
    /// it is built on first. One name for two different tables is an error.
    pub fn aliases(&self) -> Result<Vec<(String, Arc<Rel>)>> {
        fn walk(rel: &Arc<Rel>, out: &mut Vec<(String, Arc<Rel>)>) -> Result<()> {
            for input in rel.inputs() {
                walk(input, out)?;
            }
            if let RelOp::Alias { name, .. } = &rel.op {
                match out.iter().find(|(seen, _)| seen == name) {
                    Some((_, other)) if other.id != rel.id => {
                        return invalid(format!("two tables in this expression have the alias {name:?}"));
                    }
                    Some(_) => {}
                    None => out.push((name.clone(), rel.clone())),
                }
            }
            Ok(())
        }
        let mut out = vec![];
        walk(&self.rel, &mut out)?;
        Ok(out)
    }

    fn wrap(&self, op: RelOp, mut schema: Schema) -> Table {
        // NOT NULL is a fact about a stored column, not about what a query returns
        schema.fields.iter_mut().for_each(|f| f.nullable = true);
        Table::from_rel(Rel::new(op, schema))
    }

    pub fn rel(&self) -> &Rel {
        &self.rel
    }

    pub fn arc(&self) -> Arc<Rel> {
        self.rel.clone()
    }

    pub fn schema(&self) -> &Schema {
        &self.rel.schema
    }

    /// One of this table's columns (Ibis `t.name`). Bound to this table:
    /// usable in operations on tables derived from it, and in subqueries.
    /// An unknown name is reported when the expression is used.
    pub fn col(&self, name: &str) -> Expr {
        Expr::Field { rel: self.rel.clone(), name: name.to_string() }
    }

    /// This table as a value, where it has one column and one row (Ibis
    /// `t.as_scalar()`).
    pub fn as_scalar(&self) -> Result<Expr> {
        match self.schema().len() {
            1 => Ok(Expr::ScalarSubquery(self.rel.clone())),
            n => invalid(format!("as_scalar needs a table of one column; this one has {n} ({})", self.schema())),
        }
    }

    /// The same rows under a new identity, for self-joins (Ibis `t.view()`).
    pub fn view(&self) -> Table {
        self.wrap(RelOp::View { input: self.rel.clone() }, self.schema().clone())
    }

    // -- binding ------------------------------------------------------------

    fn bind_with(&self, e: Expr, ctx: Ctx, d: &DerefMap) -> Result<Expr> {
        match e {
            Expr::Field { rel, name } => {
                if let Some(n) = d.get(&(rel.id, name.clone())) {
                    return Ok(Expr::Column(n.clone()));
                }
                if rel.is_scalar() {
                    let t = Table::from_arc(rel);
                    return Ok(Expr::ScalarSubquery(t.select([crate::expr::col(&name)])?.rel));
                }
                if rel.schema.field(&name).is_none() {
                    return invalid(format!("no column {name:?} in table {}", rel.schema));
                }
                Ok(Expr::Field { rel, name }) // correlated
            }
            Expr::Agg { func, arg, filter, options } => {
                let agg = Expr::Agg { func, arg, filter, options };
                let foreign: Vec<Arc<Rel>> = agg
                    .field_rels()
                    .into_iter()
                    .filter(|r| r.id != self.rel.id && (ctx == Ctx::Filter || !d.keys().any(|(id, _)| *id == r.id)))
                    .collect();
                if ctx == Ctx::Value && foreign.is_empty() {
                    return agg.map_children(&mut |c| self.bind_with(c, ctx, d));
                }
                let Expr::Agg { func, arg, filter, options } = agg else { unreachable!() };
                let src = match foreign.as_slice() {
                    [] => self.clone(),
                    [f] => Table::from_arc(f.clone()),
                    _ => return invalid("a reduction can't reference columns of more than one other table"),
                };
                if func == AggFunc::Any && filter.is_none() && !foreign.is_empty() {
                    let pred = *arg.expect("any() has an argument");
                    return Ok(Expr::Exists(src.filter([pred])?.rel));
                }
                let agg = Expr::Agg { func, arg, filter, options };
                Ok(Expr::ScalarSubquery(src.aggregate([agg.name("_")])?.rel))
            }
            Expr::InColumn { arg, column } => {
                let arg = self.bind_with(*arg, ctx, d)?;
                let src = match column.field_rels().as_slice() {
                    [] => self.clone(),
                    [r] => Table::from_arc(r.clone()),
                    _ => return invalid("isin: the column expression uses more than one table"),
                };
                let name = column.output_name();
                Ok(Expr::InSubquery { arg: Box::new(arg), rel: src.select([column.name(&name)])?.rel })
            }
            Expr::Window(..) if ctx == Ctx::Filter => invalid("window function in filter (mutate first)"),
            e => e.map_children(&mut |c| self.bind_with(c, ctx, d)),
        }
    }

    fn bind_named(&self, exprs: Vec<(String, Expr)>, ctx: Ctx) -> Result<Vec<(String, Expr)>> {
        let d = self.rel.deref_map();
        exprs.into_iter().map(|(n, e)| Ok((n, self.bind_with(e, ctx, &d)?))).collect()
    }

    // -- operations ---------------------------------------------------------

    fn project(&self, exprs: Vec<(String, Expr)>) -> Result<Table> {
        let exprs = self.bind_named(exprs, Ctx::Value)?;
        let fields =
            exprs.iter().map(|(n, e)| Ok(Field::new(n, e.dtype(self.schema())?))).collect::<Result<Vec<_>>>()?;
        check_unique(&fields)?;
        Ok(self.wrap(RelOp::Project { input: self.rel.clone(), exprs }, Schema::new(fields)))
    }

    /// Output exactly these columns/expressions. `&str` items are column names.
    pub fn select(&self, exprs: impl IntoIterator<Item = impl Into<Expr>>) -> Result<Table> {
        self.project(named(exprs))
    }

    /// Add or replace columns; replaced columns keep their position (Ibis `mutate`).
    pub fn mutate(&self, exprs: impl IntoIterator<Item = impl Into<Expr>>) -> Result<Table> {
        let new = named(exprs);
        let mut out: Vec<(String, Expr)> = self
            .schema()
            .names()
            .map(|n| {
                let e = new.iter().find(|(m, _)| m == n).map_or_else(|| crate::expr::col(n), |(_, e)| e.clone());
                (n.to_string(), e)
            })
            .collect();
        for (n, e) in new {
            if self.schema().field(&n).is_none() {
                out.push((n, e));
            }
        }
        self.project(out)
    }

    pub fn drop(&self, names: &[&str]) -> Result<Table> {
        for n in names {
            if self.schema().field(n).is_none() {
                return invalid(format!("no column {n:?} in {}", self.schema()));
            }
        }
        self.select(self.schema().names().filter(|n| !names.contains(n)).map(crate::expr::col).collect::<Vec<_>>())
    }

    /// Move `cols` to just after column `after`, keeping their order (Ibis
    /// `t.relocate(*cols, after=...)`).
    pub fn relocate<'a>(&self, cols: impl IntoIterator<Item = &'a str>, after: &str) -> Result<Table> {
        let cols: Vec<&str> = cols.into_iter().collect();
        if self.schema().field(after).is_none() {
            return invalid(format!("no column {after:?} in {}", self.schema()));
        }
        let mut order: Vec<&str> = vec![];
        for n in self.schema().names().filter(|n| !cols.contains(n)) {
            order.push(n);
            if n == after {
                order.extend(&cols);
            }
        }
        self.select(order)
    }

    /// One row per element of an array column (Ibis `t.unnest(column,
    /// offset=..., keep_empty=...)`). The column keeps its name and holds
    /// the element; `offset` adds a last column with the element's 0-based
    /// position; with `keep_empty` a row whose array is empty or NULL
    /// stays, with a NULL element. `column` may be an expression, which is
    /// added as a column under its name first.
    pub fn unnest(&self, column: impl Into<Expr>, offset: Option<&str>, keep_empty: bool) -> Result<Table> {
        let column: Expr = column.into();
        let name = match &column {
            Expr::Column(name) => name.clone(),
            other => {
                let name = other.output_name();
                return self.mutate([column.name(&name)])?.unnest(crate::expr::col(&name), offset, keep_empty);
            }
        };
        let mut fields = self.schema().fields.clone();
        let Some(field) = fields.iter_mut().find(|f| f.name == name) else {
            return invalid(format!("no column {name:?} in {}", self.schema()));
        };
        field.dtype = match &field.dtype {
            crate::DataType::Array(item) => (**item).clone(),
            other => return invalid(format!("unnest needs an array column, {name:?} is {other}")),
        };
        if let Some(offset) = offset {
            fields.push(Field::new(offset, crate::DataType::Int64));
        }
        check_unique(&fields)?;
        let op = RelOp::Unnest { input: self.rel.clone(), column: name, offset: offset.map(String::from), keep_empty };
        Ok(self.wrap(op, Schema::new(fields)))
    }

    /// Replace each of the struct columns `columns` by its fields, in place
    /// (Ibis `t.unpack(...)`). A field takes the place of an existing column
    /// of the same name, wherever that column is.
    pub fn unpack(&self, columns: &[&str]) -> Result<Table> {
        let mut out: Vec<(String, Expr, bool)> = vec![];
        for f in &self.schema().fields {
            let fields = match (&f.dtype, columns.contains(&f.name.as_str())) {
                (crate::DataType::Struct(fields), true) => fields,
                (other, true) => return invalid(format!("unpack needs a struct column, {:?} is {other}", f.name)),
                (_, false) => {
                    if !out.iter().any(|(n, ..)| *n == f.name) {
                        out.push((f.name.clone(), crate::expr::col(&f.name), false));
                    }
                    continue;
                }
            };
            for (name, _) in fields {
                let field = (name.clone(), crate::expr::col(&f.name).field(name), true);
                match out.iter_mut().find(|(n, ..)| n == name) {
                    Some(existing) => *existing = field,
                    None => out.push(field),
                }
            }
        }
        if let Some(missing) = columns.iter().find(|c| self.schema().field(c).is_none()) {
            return invalid(format!("no column {missing:?} in {}", self.schema()));
        }
        self.select(out.into_iter().map(|(name, e, _)| e.name(&name)).collect::<Vec<_>>())
    }

    /// The fields of a struct column as the columns of a table (Ibis `t.col.lift()`).
    pub fn lift(&self, column: &str) -> Result<Table> {
        match self.schema().field(column).map(|f| &f.dtype) {
            Some(crate::DataType::Struct(fields)) => self.select(
                fields.iter().map(|(name, _)| crate::expr::col(column).field(name).name(name)).collect::<Vec<_>>(),
            ),
            Some(other) => invalid(format!("lift needs a struct column, {column:?} is {other}")),
            None => invalid(format!("no column {column:?} in {}", self.schema())),
        }
    }

    pub fn filter(&self, predicates: impl IntoIterator<Item = Expr>) -> Result<Table> {
        let predicates: Vec<Expr> = predicates.into_iter().collect();
        let windowed = |p: &Expr| p.has(&|e| matches!(e, Expr::Window(..)));
        if predicates.iter().any(windowed) {
            // A window sees the rows as they are before the filter: each such
            // predicate is computed as a column, filtered on, and dropped.
            let names: Vec<String> = self.schema().names().map(String::from).collect();
            let (mut computed, mut plain) = (vec![], vec![]);
            for (i, p) in predicates.into_iter().enumerate() {
                if windowed(&p) {
                    let name = format!("_filter_{i}");
                    plain.push(crate::expr::col(&name));
                    computed.push(p.name(&name));
                } else {
                    plain.push(p);
                }
            }
            return self.mutate(computed)?.filter(plain)?.select(names.iter());
        }
        let d = self.rel.deref_map();
        let predicates =
            predicates.into_iter().map(|p| self.bind_with(p, Ctx::Filter, &d)).collect::<Result<Vec<_>>>()?;
        for p in &predicates {
            let t = p.dtype(self.schema())?;
            if t != crate::DataType::Boolean {
                return invalid(format!("filter predicate must be boolean, got {t}: {p}"));
            }
        }
        Ok(self.wrap(RelOp::Filter { input: self.rel.clone(), predicates }, self.schema().clone()))
    }

    /// Reduce the whole table (Ibis `t.aggregate(...)` with no `by`).
    pub fn aggregate(&self, metrics: impl IntoIterator<Item = impl Into<Expr>>) -> Result<Table> {
        self.group_by(Vec::<Expr>::new()).agg(metrics)
    }

    /// How often each value of `expr` occurs (Ibis `col.value_counts()`):
    /// the value, then its count in a column named `<name>_count`.
    pub fn value_counts(&self, expr: impl Into<Expr>) -> Result<Table> {
        let expr = expr.into();
        let count = format!("{}_count", expr.output_name());
        self.group_by([expr]).agg([crate::expr::count().name(&count)])
    }

    /// The `k` most frequent values of `expr` with their counts, most
    /// frequent first (Ibis `col.topk(k)`). Which of equally frequent
    /// values make it is up to the engine.
    pub fn topk(&self, expr: impl Into<Expr>, k: u64) -> Result<Table> {
        let counts = self.value_counts(expr)?;
        let count = counts.schema().fields[1].name.clone();
        Ok(counts.order_by([crate::expr::desc(crate::expr::col(&count))])?.limit(k, 0))
    }

    pub fn group_by(&self, keys: impl IntoIterator<Item = impl Into<Expr>>) -> GroupedTable {
        GroupedTable { table: self.clone(), keys: named(keys) }
    }

    pub fn order_by(&self, keys: impl IntoIterator<Item = impl Into<SortKey>>) -> Result<Table> {
        let d = self.rel.deref_map();
        let keys = keys
            .into_iter()
            .map(|k| {
                let k: SortKey = k.into();
                Ok(SortKey { expr: self.bind_with(k.expr, Ctx::Value, &d)?, ..k })
            })
            .collect::<Result<Vec<_>>>()?;
        for k in &keys {
            // a column of some other table: an aggregate has no `year` to sort by
            if let Some(name) = foreign_column(&k.expr) {
                return invalid(format!("order_by: {name:?} is not a column of this table ({})", self.schema()));
            }
            k.expr.dtype(self.schema())?;
        }
        Ok(self.wrap(RelOp::Sort { input: self.rel.clone(), keys }, self.schema().clone()))
    }

    pub fn limit(&self, n: u64, offset: u64) -> Table {
        self.wrap(RelOp::Limit { input: self.rel.clone(), n: Some(n), offset }, self.schema().clone())
    }

    /// Skip the first `n` rows (Ibis `t.limit(None, offset=n)`, `t[n:]`).
    pub fn offset(&self, n: u64) -> Table {
        self.wrap(RelOp::Limit { input: self.rel.clone(), n: None, offset: n }, self.schema().clone())
    }

    /// About `fraction` of the rows, each kept or not at random (Ibis
    /// `t.sample(fraction)`, by row). Another draw each time it runs.
    pub fn sample(&self, fraction: f64) -> Result<Table> {
        if !(0.0..=1.0).contains(&fraction) {
            return invalid(format!("sample: a fraction is between 0 and 1, got {fraction}"));
        }
        self.filter([crate::expr::random().lt(crate::expr::lit(fraction))])
    }

    /// This table with a value for each parameter (Ibis
    /// `expr.execute(params={param: value})`): pairs of a `param(..)` and
    /// the expression, usually a literal, that takes its place. The value
    /// is cast to the parameter's type, so a date parameter takes a string.
    pub fn bind(&self, values: impl IntoIterator<Item = (Expr, Expr)>) -> Result<Table> {
        let values: Vec<(Expr, Expr)> = values.into_iter().collect();
        if let Some((not, _)) = values.iter().find(|(p, _)| !matches!(p, Expr::Param(..))) {
            return invalid(format!("bind: {not} is not a parameter (make one with `param(dtype)`)"));
        }
        Ok(Table::from_arc(self.rel.bind(&values)?))
    }

    /// Python's `t[start:stop]` on the rows: `None` is an open end and a
    /// negative bound counts from the last row (Ibis `t[-3:]`).
    ///
    /// Bounds counted from the front are a limit and an offset. One counted
    /// from the back needs the number of rows: the rows are numbered (in
    /// the table's order, if it ends in an `order_by`) and counted by window
    /// functions, and filtered on that. Which rows a slice of an unordered
    /// table holds is up to the engine.
    pub fn slice(&self, start: Option<i64>, stop: Option<i64>) -> Result<Table> {
        let first = start.unwrap_or(0);
        match stop {
            None if first >= 0 => return Ok(self.offset(first as u64)),
            Some(stop) if first >= 0 && stop >= 0 => {
                return Ok(self.limit((stop - first).max(0) as u64, first as u64));
            }
            // nothing lies between a start counted from the back and an end at or before it
            Some(stop) if first < 0 && (stop == 0 || (stop < 0 && stop <= first)) => return Ok(self.limit(0, 0)),
            _ => {}
        }
        let keys = match &self.rel.op {
            RelOp::Sort { keys, .. } => keys.clone(),
            _ => vec![],
        };
        let ordered = !keys.is_empty();
        let names: Vec<String> = self.schema().names().map(String::from).collect();
        let (row, rows) = ("_slice_row", "_slice_rows");
        let numbered = self.mutate([
            crate::expr::row_number().over(crate::WindowSpec::new().order_by(keys)).name(row),
            crate::expr::count().over(crate::WindowSpec::new()).name(rows),
        ])?;
        let col = crate::expr::col;
        // the row with 0-based number `i` is kept if `lo <= i < hi`
        let bound = |b: i64| if b >= 0 { crate::expr::lit(b) } else { col(rows) + b };
        let mut kept = vec![];
        if first != 0 {
            kept.push(col(row).ge(bound(first)));
        }
        if let Some(stop) = stop {
            kept.push(col(row).lt(bound(stop)));
        }
        let sliced = numbered.filter(kept)?;
        let sliced = if ordered { sliced.order_by([col(row)])? } else { sliced };
        sliced.select(names.iter())
    }

    /// Replace NULLs in the named columns (Ibis `t.fill_null({col: value})`).
    /// Column types don't change.
    pub fn fill_null<'a>(&self, replacements: impl IntoIterator<Item = (&'a str, crate::Value)>) -> Result<Table> {
        let mut exprs = vec![];
        for (name, value) in replacements {
            let Some(f) = self.schema().field(name) else {
                return invalid(format!("no column {name:?} in {}", self.schema()));
            };
            let t = value.dtype();
            if !(f.dtype.is_numeric() && t.is_numeric()) && f.dtype != t {
                return invalid(format!("can't fill {name}: {} with a {t} value", f.dtype));
            }
            exprs.push(crate::expr::col(name).fill_null(crate::expr::lit(value)).cast(f.dtype.clone()).name(name));
        }
        if exprs.is_empty() { Ok(self.clone()) } else { self.mutate(exprs) }
    }

    /// Replace NULLs in every column with `value`, which must suit each
    /// column's type (Ibis `t.fill_null(value)`).
    pub fn fill_null_with(&self, value: impl Into<crate::Value>) -> Result<Table> {
        let value = value.into();
        self.fill_null(self.schema().names().map(|n| (n, value.clone())).collect::<Vec<_>>())
    }

    /// Drop rows with NULLs in `subset` (`None`: any column): rows where any
    /// of those columns is NULL, or only rows where all are (Ibis `drop_null`).
    pub fn drop_null(&self, subset: Option<&[&str]>, how: How) -> Result<Table> {
        let cols: Vec<&str> = match subset {
            Some(s) => s.to_vec(),
            None => self.schema().names().collect(),
        };
        let mut notnull = cols.iter().map(|c| crate::expr::col(c).notnull());
        let Some(first) = notnull.next() else {
            // No columns: "any is NULL" holds for no row, "all are NULL" for
            // every row (vacuously), as in Ibis and pandas.
            return match how {
                How::Any => Ok(self.clone()),
                How::All => self.filter([crate::expr::lit(false)]),
            };
        };
        let keep = notnull.fold(first, |acc, e| match how {
            How::Any => acc.and(e),
            How::All => acc.or(e),
        });
        self.filter([keep])
    }

    pub fn head(&self, n: u64) -> Table {
        self.limit(n, 0)
    }

    pub fn distinct(&self) -> Table {
        self.wrap(RelOp::Distinct { input: self.rel.clone() }, self.schema().clone())
    }

    /// Join on predicates: expressions over both tables' columns
    /// (`a.col("k").eq(b.col("k"))`), or `(left, right)` column-name pairs.
    /// Inner joins emit equal same-named keys once (from the left); other
    /// colliding right-side names get a `_right` suffix, as in Ibis.
    pub fn join(
        &self,
        right: &Table,
        on: impl IntoIterator<Item = impl Into<JoinPred>>,
        how: JoinKind,
    ) -> Result<Table> {
        let l = &self.rel;
        let dl = l.deref_map();
        // Joining a table with itself (or an ancestor of itself): the right
        // side needs its own identity, as Ibis does implicitly.
        let right = if dl.keys().any(|(id, _)| *id == right.rel.id) { right.view() } else { right.clone() };
        let r = &right.rel;
        let dr = r.deref_map();
        let field = |rel: &Arc<Rel>, name: &str| Expr::Field { rel: rel.clone(), name: name.to_string() };
        // Resolve every column to a Field of exactly `l` or `r`.
        fn resolve(e: Expr, l: &Arc<Rel>, r: &Arc<Rel>, dl: &DerefMap, dr: &DerefMap) -> Result<Expr> {
            let field = |rel: &Arc<Rel>, name: String| Expr::Field { rel: rel.clone(), name };
            match e {
                Expr::Column(n) if l.schema.field(&n).is_some() => Ok(field(l, n)),
                Expr::Column(n) if r.schema.field(&n).is_some() => Ok(field(r, n)),
                Expr::Column(n) => invalid(format!("join predicate column {n:?} is in neither table")),
                Expr::Field { rel, name } => {
                    let key = (rel.id, name);
                    if let Some(n) = dl.get(&key) {
                        Ok(field(l, n.clone()))
                    } else if let Some(n) = dr.get(&key) {
                        Ok(field(r, n.clone()))
                    } else {
                        Ok(Expr::Field { rel, name: key.1 }) // correlated
                    }
                }
                Expr::Agg { .. } => invalid("reductions aren't allowed in join predicates"),
                e => e.map_children(&mut |c| resolve(c, l, r, dl, dr)),
            }
        }
        let mut predicates = vec![];
        for p in on {
            let e = match p.into() {
                JoinPred::Expr(e) => e,
                JoinPred::Keys(a, b) => field(l, &a).eq(field(r, &b)),
            };
            let e = resolve(e, l, r, &dl, &dr)?;
            // every column is now a Field, which carries its own type
            let t = e.dtype(&Schema::new(vec![]))?;
            if t != crate::DataType::Boolean {
                return invalid(format!("join predicate must be boolean, got {t}: {e}"));
            }
            predicates.push(e);
        }
        // Same-named key pairs `l.k = r.k` (inner joins emit `k` once).
        let shared: Vec<String> = predicates
            .iter()
            .filter_map(|p| match p {
                Expr::Binary(BinaryOp::Eq, a, b) => match (&**a, &**b) {
                    (Expr::Field { rel: ra, name: na }, Expr::Field { rel: rb, name: nb })
                        if na == nb
                            && ra.id != rb.id
                            && [ra.id, rb.id].contains(&l.id)
                            && [ra.id, rb.id].contains(&r.id) =>
                    {
                        Some(na.clone())
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let (ls, rs) = (self.schema(), right.schema());
        let mut outputs: Vec<(String, Side, String)> =
            ls.names().map(|n| (n.to_string(), Side::Left, n.to_string())).collect();
        let mut fields: Vec<Field> = ls.fields.clone();
        if !matches!(how, JoinKind::Semi | JoinKind::Anti) {
            for f in &rs.fields {
                if how == JoinKind::Inner && shared.contains(&f.name) {
                    continue;
                }
                let name = if ls.field(&f.name).is_some() { format!("{}_right", f.name) } else { f.name.clone() };
                outputs.push((name.clone(), Side::Right, f.name.clone()));
                fields.push(Field::new(name, f.dtype.clone()));
            }
        }
        check_unique(&fields)?;
        Ok(self.wrap(RelOp::Join { left: l.clone(), right: r.clone(), how, predicates, outputs }, Schema::new(fields)))
    }

    pub fn inner_join(&self, right: &Table, on: impl IntoIterator<Item = impl Into<JoinPred>>) -> Result<Table> {
        self.join(right, on, JoinKind::Inner)
    }

    pub fn left_join(&self, right: &Table, on: impl IntoIterator<Item = impl Into<JoinPred>>) -> Result<Table> {
        self.join(right, on, JoinKind::Left)
    }

    /// Columns are matched by name, as in Ibis: `other` may list them in a
    /// different order. The result uses this table's order.
    fn set_op(&self, other: &Table, kind: SetKind, distinct: bool) -> Result<Table> {
        let (a, b) = (self.schema(), other.schema());
        let same = a.len() == b.len() && a.fields.iter().all(|x| b.field(&x.name).is_some_and(|y| x.dtype == y.dtype));
        if !same {
            return invalid(format!("set operation schemas differ: ({a}) vs ({b})"));
        }
        let other =
            if a.names().eq(b.names()) { other.clone() } else { other.select(a.names().collect::<Vec<_>>())? };
        Ok(self.wrap(RelOp::SetOp { kind, distinct, left: self.rel.clone(), right: other.rel.clone() }, a.clone()))
    }

    /// Rows of both tables; `distinct` removes duplicates (SQL `UNION` vs `UNION ALL`).
    pub fn union(&self, other: &Table, distinct: bool) -> Result<Table> {
        self.set_op(other, SetKind::Union, distinct)
    }
    /// Rows present in both tables. Without `distinct`, a row appears as
    /// often as in the table where it is rarer (`INTERSECT ALL`).
    pub fn intersect(&self, other: &Table, distinct: bool) -> Result<Table> {
        self.set_op(other, SetKind::Intersect, distinct)
    }
    /// Rows of this table that aren't in `other`. Without `distinct`, each
    /// row of `other` cancels one occurrence (`EXCEPT ALL`).
    pub fn difference(&self, other: &Table, distinct: bool) -> Result<Table> {
        self.set_op(other, SetKind::Difference, distinct)
    }
}

/// Result of `Table::group_by`; finish with `.agg(...)`.
pub struct GroupedTable {
    table: Table,
    keys: Vec<(String, Expr)>,
}

impl GroupedTable {
    pub fn agg(self, metrics: impl IntoIterator<Item = impl Into<Expr>>) -> Result<Table> {
        let t = &self.table;
        let schema = t.schema();
        let keys = t.bind_named(self.keys, Ctx::Value)?;
        let metrics = t.bind_named(named(metrics), Ctx::Value)?;
        let mut fields = vec![];
        for (n, e) in &keys {
            if e.is_reduction() {
                return invalid(format!("group key can't be a reduction: {e}"));
            }
            fields.push(Field::new(n, e.dtype(schema)?));
        }
        for (n, e) in &metrics {
            if !e.is_reduction() {
                return invalid(format!("metric must be a reduction: {e}"));
            }
            fields.push(Field::new(n, e.dtype(schema)?));
        }
        check_unique(&fields)?;
        Ok(t.wrap(RelOp::Aggregate { input: t.rel.clone(), groups: keys, metrics }, Schema::new(fields)))
    }
}
