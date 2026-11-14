//! The Rust side of each conformance case, keyed by the same Ibis pytest id as
//! conformance/cases/<module>.py. One file per Ibis test module; keep each in
//! the same order as its Python twin.
//!
//! `notimpl` mirrors Ibis's `pytest.mark.notimpl`: backends where the case is
//! known not to pass yet. A marked case that passes is reported as `xpass`.

mod aggregation;
mod conditionals;
mod generic;
mod join;
mod numeric;
mod set_ops;
mod string;

use std::collections::BTreeMap;

use ibis_core::prelude::*;
use ibis_core::{Error, Result};

/// The fixture tables a case can use (Python: `c.alltypes`, `c.mem("x")`, ...).
pub struct Fx {
    pub alltypes: Table,
    pub batting: Table,
    pub awards_players: Table,
    /// The small in-memory tables of conformance/memtables.json.
    pub mem: BTreeMap<String, Table>,
}

impl Fx {
    pub fn mem(&self, name: &str) -> Result<Table> {
        self.mem.get(name).cloned().ok_or_else(|| Error::Other(format!("no memtable {name:?}")))
    }
}

pub enum Shape {
    /// fn(t) -> column expr over functional_alltypes; run as
    /// `t.select(id, tmp=expr)` (or `t.aggregate(tmp=expr)` if a reduction).
    Column(Box<dyn Fn(&Table) -> Result<Expr>>),
    /// fn() -> expr with no columns; run as `t.select(tmp=expr).limit(1)`.
    Scalar(Box<dyn Fn() -> Result<Expr>>),
    /// fn(t) -> table expr over functional_alltypes.
    Table(Box<dyn Fn(&Table) -> Result<Table>>),
    /// fn(fixtures) -> table expr.
    Tables(Box<dyn Fn(&Fx) -> Result<Table>>),
}

pub struct Case {
    pub id: String,
    pub shape: Shape,
    pub ordered: bool,
    pub notimpl: Vec<&'static str>,
}

impl Case {
    fn new(id: &str, shape: Shape) -> Case {
        Case { id: id.into(), shape, ordered: false, notimpl: vec![] }
    }

    /// Rows must come back in the golden's order (the expression ends in a total ordering).
    fn ordered(mut self) -> Self {
        self.ordered = true;
        self
    }

    fn notimpl(mut self, backends: &[&'static str]) -> Self {
        self.notimpl.extend(backends);
        self
    }

    /// Build the table expression this case executes.
    pub fn build(&self, fx: &Fx) -> Result<Table> {
        let t = &fx.alltypes;
        match &self.shape {
            Shape::Tables(f) => f(fx),
            Shape::Table(f) => f(t),
            Shape::Scalar(f) => Ok(t.select([f()?.name("tmp")])?.limit(1, 0)),
            Shape::Column(f) => {
                let e = f(t)?.name("tmp");
                if e.is_reduction() { t.aggregate([e]) } else { t.select([col("id"), e]) }
            }
        }
    }
}

fn column(id: &str, f: impl Fn(&Table) -> Result<Expr> + 'static) -> Case {
    Case::new(id, Shape::Column(Box::new(f)))
}

fn scalar(id: &str, f: impl Fn() -> Result<Expr> + 'static) -> Case {
    Case::new(id, Shape::Scalar(Box::new(f)))
}

fn table(id: &str, f: impl Fn(&Table) -> Result<Table> + 'static) -> Case {
    Case::new(id, Shape::Table(Box::new(f)))
}

fn tables(id: &str, f: impl Fn(&Fx) -> Result<Table> + 'static) -> Case {
    Case::new(id, Shape::Tables(Box::new(f)))
}

fn c(name: &str) -> Expr {
    col(name)
}

pub fn all() -> Vec<Case> {
    let mut v = vec![];
    string::register(&mut v);
    numeric::register(&mut v);
    aggregation::register(&mut v);
    generic::register(&mut v);
    conditionals::register(&mut v);
    join::register(&mut v);
    set_ops::register(&mut v);
    v
}
