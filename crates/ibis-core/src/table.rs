//! Relational IR (`Rel`) and the `Table` API that builds it.
//!
//! Every `Rel` node carries its output schema, computed (and validated) when
//! the node is built, so schema errors surface at the call that caused them.

use std::sync::Arc;

use crate::datatypes::{Field, Schema};
use crate::error::{Result, invalid};
use crate::expr::{Expr, SortKey};

#[derive(Clone, Debug, PartialEq)]
pub struct Rel {
    pub op: RelOp,
    pub schema: Schema,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RelOp {
    /// A named table in the backend's catalog.
    Table {
        name: String,
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
    /// Equi-join on (left column, right column) pairs. `outputs` lists the
    /// output columns as (output name, side, source column).
    Join {
        left: Arc<Rel>,
        right: Arc<Rel>,
        how: JoinKind,
        on: Vec<(String, String)>,
        outputs: Vec<(String, Side, String)>,
    },
    SetOp {
        kind: SetKind,
        distinct: bool,
        left: Arc<Rel>,
        right: Arc<Rel>,
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

impl Rel {
    pub fn inputs(&self) -> Vec<&Arc<Rel>> {
        match &self.op {
            RelOp::Table { .. } => vec![],
            RelOp::Project { input, .. }
            | RelOp::Filter { input, .. }
            | RelOp::Aggregate { input, .. }
            | RelOp::Sort { input, .. }
            | RelOp::Limit { input, .. }
            | RelOp::Distinct { input } => vec![input],
            RelOp::Join { left, right, .. } | RelOp::SetOp { left, right, .. } => vec![left, right],
        }
    }
}

/// A lazy table expression. Cheap to clone; building never touches a backend.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    rel: Arc<Rel>,
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
        Table::from_rel(Rel { op: RelOp::Table { name: name.to_string() }, schema })
    }

    fn from_rel(rel: Rel) -> Table {
        Table { rel: Arc::new(rel) }
    }

    fn wrap(&self, op: RelOp, schema: Schema) -> Table {
        Table::from_rel(Rel { op, schema })
    }

    pub fn rel(&self) -> &Rel {
        &self.rel
    }

    pub fn schema(&self) -> &Schema {
        &self.rel.schema
    }

    /// Reference one of this table's columns, checking it exists.
    pub fn col(&self, name: &str) -> Result<Expr> {
        match self.schema().field(name) {
            Some(_) => Ok(crate::expr::col(name)),
            None => invalid(format!("no column {name:?} in {}", self.schema())),
        }
    }

    fn project(&self, exprs: Vec<(String, Expr)>) -> Result<Table> {
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
            self.col(n)?;
        }
        self.select(self.schema().names().filter(|n| !names.contains(n)).map(crate::expr::col).collect::<Vec<_>>())
    }

    pub fn filter(&self, predicates: impl IntoIterator<Item = Expr>) -> Result<Table> {
        let predicates: Vec<Expr> = predicates.into_iter().collect();
        for p in &predicates {
            let t = p.dtype(self.schema())?;
            if t != crate::DataType::Boolean {
                return invalid(format!("filter predicate must be boolean, got {t}: {p}"));
            }
            if p.is_reduction() {
                return invalid(format!("filter predicate can't be a reduction: {p}"));
            }
        }
        Ok(self.wrap(RelOp::Filter { input: self.rel.clone(), predicates }, self.schema().clone()))
    }

    /// Reduce the whole table (Ibis `t.aggregate(...)` with no `by`).
    pub fn aggregate(&self, metrics: impl IntoIterator<Item = impl Into<Expr>>) -> Result<Table> {
        self.group_by(Vec::<Expr>::new()).agg(metrics)
    }

    pub fn group_by(&self, keys: impl IntoIterator<Item = impl Into<Expr>>) -> GroupedTable {
        GroupedTable { table: self.clone(), keys: named(keys) }
    }

    pub fn order_by(&self, keys: impl IntoIterator<Item = impl Into<SortKey>>) -> Result<Table> {
        let keys: Vec<SortKey> = keys.into_iter().map(Into::into).collect();
        for k in &keys {
            k.expr.dtype(self.schema())?;
        }
        Ok(self.wrap(RelOp::Sort { input: self.rel.clone(), keys }, self.schema().clone()))
    }

    pub fn limit(&self, n: u64, offset: u64) -> Table {
        self.wrap(RelOp::Limit { input: self.rel.clone(), n: Some(n), offset }, self.schema().clone())
    }

    pub fn head(&self, n: u64) -> Table {
        self.limit(n, 0)
    }

    pub fn distinct(&self) -> Table {
        self.wrap(RelOp::Distinct { input: self.rel.clone() }, self.schema().clone())
    }

    /// Equi-join on column pairs. Same-named key pairs are emitted once (from
    /// the left); other colliding right-side names get a `_right` suffix, as in Ibis.
    pub fn join(&self, right: &Table, on: &[(&str, &str)], how: JoinKind) -> Result<Table> {
        let (ls, rs) = (self.schema(), right.schema());
        for (l, r) in on {
            let (lt, rt) = (self.col(l)?.dtype(ls)?, right.col(r)?.dtype(rs)?);
            if lt != rt && !(lt.is_numeric() && rt.is_numeric()) {
                return invalid(format!("join key types differ: {l}: {lt} vs {r}: {rt}"));
            }
        }
        let mut outputs: Vec<(String, Side, String)> =
            ls.names().map(|n| (n.to_string(), Side::Left, n.to_string())).collect();
        let mut fields: Vec<Field> = ls.fields.clone();
        if !matches!(how, JoinKind::Semi | JoinKind::Anti) {
            for f in &rs.fields {
                let shared_key = on.iter().any(|(l, r)| *l == f.name && *r == f.name);
                if shared_key {
                    continue;
                }
                let name = if ls.field(&f.name).is_some() { format!("{}_right", f.name) } else { f.name.clone() };
                outputs.push((name.clone(), Side::Right, f.name.clone()));
                fields.push(Field::new(name, f.dtype.clone()));
            }
        }
        check_unique(&fields)?;
        let on = on.iter().map(|(l, r)| (l.to_string(), r.to_string())).collect();
        Ok(self.wrap(
            RelOp::Join { left: self.rel.clone(), right: right.rel.clone(), how, on, outputs },
            Schema::new(fields),
        ))
    }

    fn set_op(&self, other: &Table, kind: SetKind, distinct: bool) -> Result<Table> {
        let (a, b) = (self.schema(), other.schema());
        let same =
            a.len() == b.len() && a.fields.iter().zip(&b.fields).all(|(x, y)| x.name == y.name && x.dtype == y.dtype);
        if !same {
            return invalid(format!("set operation schemas differ: ({a}) vs ({b})"));
        }
        Ok(self.wrap(RelOp::SetOp { kind, distinct, left: self.rel.clone(), right: other.rel.clone() }, a.clone()))
    }

    pub fn union(&self, other: &Table, distinct: bool) -> Result<Table> {
        self.set_op(other, SetKind::Union, distinct)
    }
    pub fn intersect(&self, other: &Table) -> Result<Table> {
        self.set_op(other, SetKind::Intersect, true)
    }
    pub fn difference(&self, other: &Table) -> Result<Table> {
        self.set_op(other, SetKind::Difference, true)
    }
}

/// Result of `Table::group_by`; finish with `.agg(...)`.
pub struct GroupedTable {
    table: Table,
    keys: Vec<(String, Expr)>,
}

impl GroupedTable {
    pub fn agg(self, metrics: impl IntoIterator<Item = impl Into<Expr>>) -> Result<Table> {
        let schema = self.table.schema();
        let metrics = named(metrics);
        let mut fields = vec![];
        for (n, e) in &self.keys {
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
        Ok(self
            .table
            .wrap(RelOp::Aggregate { input: self.table.rel.clone(), groups: self.keys, metrics }, Schema::new(fields)))
    }
}
