//! Relations -> Polars `LazyFrame`s.

use ibis_core::{
    AggFunc, BinaryOp, DataType, Expr, Field, Func, JoinKind, Rel, RelOp, Result, Schema, SetKind, Side, SortKey,
    UnaryOp, Value,
};
use polars::prelude as pl;

use crate::expr::{Scope, Tx};
use crate::{Polars, pl_err, to_pl, unsupported};

const LEFT: &str = "__l_";
const RIGHT: &str = "__r_";

pub(crate) struct Planner<'a> {
    pub be: &'a Polars,
}

/// Cast to the type Ibis infers, so later ops see what Ibis would.
fn typed(e: pl::Expr, t: &DataType) -> pl::Expr {
    if *t == DataType::Null { e } else { e.cast(to_pl(t)) }
}

fn all_of(mut preds: Vec<pl::Expr>) -> Option<pl::Expr> {
    let first = if preds.is_empty() { return None } else { preds.remove(0) };
    Some(preds.into_iter().fold(first, |a, b| a.and(b)))
}

fn filtered(lf: pl::LazyFrame, preds: Vec<pl::Expr>) -> pl::LazyFrame {
    match all_of(preds) {
        Some(p) => lf.filter(p),
        None => lf,
    }
}

/// `a AND b AND c` as its parts: each can be planned on its own.
fn conjuncts<'e>(preds: impl IntoIterator<Item = &'e Expr>) -> Vec<&'e Expr> {
    fn go<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
        match e {
            Expr::Binary(BinaryOp::And, a, b) => {
                go(a, out);
                go(b, out);
            }
            _ => out.push(e),
        }
    }
    let mut out = vec![];
    for p in preds {
        go(p, &mut out);
    }
    out
}

/// `e` with each `unnest(arr)` replaced by a column that will hold the
/// elements of `arr`; the columns are added to `lifted` with their arrays.
fn lift_unnests(e: Expr, schema: &Schema, lifted: &mut Vec<(String, Expr, DataType)>) -> Result<Expr> {
    match e {
        Expr::Func(Func::Unnest, mut args) if !args.is_empty() => {
            let array = args.remove(0);
            let name = format!("__u{}", lifted.len());
            let t = array.dtype(schema)?;
            lifted.push((name.clone(), array, t));
            Ok(Expr::Column(name))
        }
        e => e.map_children(&mut |child| lift_unnests(child, schema, lifted)),
    }
}

fn has_field(e: &Expr) -> bool {
    matches!(e, Expr::Field { .. }) || e.children().iter().any(|c| has_field(c))
}

fn has_column(e: &Expr) -> bool {
    matches!(e, Expr::Column(_)) || e.children().iter().any(|c| has_column(c))
}

/// One type for a pair of join keys: Polars does not join `int32` to `int64`.
fn key_type(a: DataType, b: DataType) -> Result<DataType> {
    if a == b { Ok(a) } else { DataType::promote(&a, &b) }
}

pub(crate) fn sort_options(keys: &[SortKey]) -> pl::SortMultipleOptions {
    pl::SortMultipleOptions::default()
        .with_order_descending_multi(keys.iter().map(|k| !k.ascending).collect::<Vec<_>>())
        .with_nulls_last_multi(keys.iter().map(|k| !k.nulls_first).collect::<Vec<_>>())
        .with_maintain_order(true)
}

/// The scopes `e`'s field references resolve to (0 = left, 1 = right);
/// false if one resolves to neither.
fn sides(tx: &Tx, e: &Expr, out: &mut Vec<usize>) -> bool {
    match e {
        Expr::Field { rel, name } => match tx.field(rel, name) {
            Some((i, _)) => {
                out.push(i);
                true
            }
            None => false,
        },
        _ => e.children().iter().all(|c| sides(tx, c, out)),
    }
}

/// Correlated predicates of a subquery, split into what a join can use.
struct Correlation {
    /// (outer expression, inner expression) that must be equal
    keys: Vec<(pl::Expr, pl::Expr)>,
    /// predicates on the inner table alone
    local: Vec<pl::Expr>,
    /// predicates reading both tables that are not such equalities
    residual: Vec<pl::Expr>,
}

impl Planner<'_> {
    fn tx<'s>(&'s self, schema: &'s Schema, scopes: &'s [Scope]) -> Tx<'s> {
        Tx { be: self.be, schema, scopes, col_prefix: "", subs: &[], lambda: None }
    }

    pub(crate) fn rel(&self, rel: &Rel) -> Result<pl::LazyFrame> {
        Ok(match &rel.op {
            RelOp::Table { name } => self.be.frame(name)?,
            RelOp::View { input } => self.rel(input)?,
            RelOp::Sql { .. } => return unsupported("a SQL query (there is no SQL here)"),
            RelOp::Project { input, exprs } => self.project(self.rel(input)?, &input.schema, exprs, &[])?,
            RelOp::Filter { input, predicates } => self.filter(input, predicates)?,
            RelOp::Aggregate { input, groups, metrics } => {
                let lf = self.rel(input)?;
                let tx = self.tx(&input.schema, &[]);
                let named = |cols: &[(String, Expr)]| {
                    cols.iter()
                        .map(|(n, e)| Ok(typed(tx.expr(e)?, &e.dtype(&input.schema)?).alias(n.as_str())))
                        .collect::<Result<Vec<_>>>()
                };
                if groups.is_empty() {
                    lf.select(named(metrics)?)
                } else {
                    lf.group_by_stable(named(groups)?).agg(named(metrics)?)
                }
            }
            RelOp::Sort { input, keys } => {
                let lf = self.rel(input)?;
                let tx = self.tx(&input.schema, &[]);
                let by = keys.iter().map(|k| tx.expr(&k.expr)).collect::<Result<Vec<_>>>()?;
                lf.sort_by_exprs(by, sort_options(keys))
            }
            RelOp::Limit { input, n, offset } => {
                let len = n.map(|n| n.min(pl::IdxSize::MAX as u64) as pl::IdxSize).unwrap_or(pl::IdxSize::MAX);
                self.rel(input)?.slice(*offset as i64, len)
            }
            RelOp::Distinct { input } => self.rel(input)?.unique_stable(None, pl::UniqueKeepStrategy::First),
            RelOp::Join { left, right, how, predicates, outputs } => {
                self.join(left, right, *how, predicates, outputs)?
            }
            RelOp::SetOp { kind, distinct, left, right } => self.set_op(rel, *kind, *distinct, left, right)?,
            RelOp::Unnest { input, column, offset, keep_empty } => {
                let lf = self.rel(input)?;
                let options = pl::ExplodeOptions { empty_as_null: *keep_empty, keep_nulls: *keep_empty };
                match offset {
                    None => lf.explode(pl::cols([column.as_str()]), options),
                    Some(offset) => {
                        // the elements' positions, exploded alongside them
                        let len = pl::col(column.as_str()).list().len().cast(pl::DataType::Int64);
                        let positions = pl::int_ranges(pl::lit(0i64), len, pl::lit(1i64), pl::DataType::Int64);
                        lf.with_columns([positions.alias(offset.as_str())])
                            .explode(pl::cols([column.as_str(), offset.as_str()]), options)
                    }
                }
            }
        })
    }

    /// Output exactly `exprs`, keeping the columns named in `keep` as well.
    fn project(
        &self,
        lf: pl::LazyFrame,
        schema: &Schema,
        exprs: &[(String, Expr)],
        keep: &[String],
    ) -> Result<pl::LazyFrame> {
        let kept = keep.iter().map(|k| pl::col(k.as_str()));
        if exprs.iter().all(|(_, e)| matches!(e, Expr::Column(_))) {
            let cols = exprs.iter().map(|(n, e)| match e {
                Expr::Column(c) => pl::col(c.as_str()).alias(n.as_str()),
                _ => unreachable!(),
            });
            return Ok(lf.select(kept.chain(cols).collect::<Vec<_>>()));
        }
        // `arr.unnest()`: the arrays become columns, the frame is exploded
        // over them, and the expressions read the elements from there
        let mut lifted: Vec<(String, Expr, DataType)> = vec![];
        let (mut lf, mut wide) = (lf, None);
        let lifted_exprs: Vec<(String, Expr)>;
        let (schema, exprs) = if exprs.iter().any(|(_, e)| e.is_unnest()) {
            lifted_exprs = exprs
                .iter()
                .map(|(n, e)| Ok((n.clone(), lift_unnests(e.clone(), schema, &mut lifted)?)))
                .collect::<Result<_>>()?;
            let tx = self.tx(schema, &[]);
            let arrays = lifted
                .iter()
                .map(|(name, array, t)| Ok(typed(tx.expr(array)?, t).alias(name.as_str())))
                .collect::<Result<Vec<_>>>()?;
            let names = lifted.iter().map(|(name, ..)| name.as_str());
            lf = lf
                .with_columns(arrays)
                .explode(pl::cols(names), pl::ExplodeOptions { empty_as_null: false, keep_nulls: false });
            let elements =
                lifted.iter().map(|(name, _, t)| Field::new(name, t.item().cloned().unwrap_or(DataType::Null)));
            let fields = schema.fields.iter().cloned().chain(elements).collect();
            (&*wide.insert(Schema::new(fields)), lifted_exprs.as_slice())
        } else {
            (schema, exprs)
        };
        // `with_columns` broadcasts reductions to every row, which is what a
        // reduction in a projection means; temporary names keep new columns
        // from shadowing the inputs other expressions still read.
        let tx = self.tx(schema, &[]);
        let tmp = exprs
            .iter()
            .enumerate()
            .map(|(i, (_, e))| Ok(typed(tx.expr(e)?, &e.dtype(schema)?).alias(format!("__o{i}"))))
            .collect::<Result<Vec<_>>>()?;
        let out = exprs.iter().enumerate().map(|(i, (n, _))| pl::col(format!("__o{i}")).alias(n.as_str()));
        Ok(lf.with_columns(tmp).select(kept.chain(out).collect::<Vec<_>>()))
    }

    fn filter(&self, input: &Rel, predicates: &[Expr]) -> Result<pl::LazyFrame> {
        let mut lf = self.rel(input)?;
        // EXISTS / NOT EXISTS become semi / anti joins
        let mut rest = vec![];
        for p in conjuncts(predicates) {
            match p {
                Expr::Exists(sub) => lf = self.exists(lf, input, sub, false)?,
                Expr::Unary(UnaryOp::Not, inner) if matches!(&**inner, Expr::Exists(_)) => {
                    let Expr::Exists(sub) = &**inner else { unreachable!() };
                    lf = self.exists(lf, input, sub, true)?
                }
                _ => rest.push(p),
            }
        }
        // correlated scalar subqueries become joined-in columns
        fn scalar_subqueries<'e>(e: &'e Expr, out: &mut Vec<&'e Rel>) {
            if let Expr::ScalarSubquery(rel) = e {
                out.push(rel);
            }
            for c in e.children() {
                scalar_subqueries(c, out);
            }
        }
        let mut subs: Vec<(u64, String)> = vec![];
        for p in &rest {
            let mut found = vec![];
            scalar_subqueries(p, &mut found);
            for sub in found {
                if subs.iter().any(|(id, _)| *id == sub.id) {
                    continue;
                }
                let column = format!("__sq{}", subs.len());
                if let Some(joined) = self.join_scalar_subquery(lf.clone(), input, sub, &column)? {
                    lf = joined;
                    subs.push((sub.id, column));
                }
            }
        }
        let tx = Tx { be: self.be, schema: &input.schema, scopes: &[], col_prefix: "", subs: &subs, lambda: None };
        let preds = rest.iter().map(|p| tx.expr(p)).collect::<Result<Vec<_>>>()?;
        let lf = filtered(lf, preds);
        Ok(if subs.is_empty() { lf } else { lf.select(input.schema.names().map(pl::col).collect::<Vec<_>>()) })
    }

    /// Split a subquery's filter by how each predicate relates to the outer
    /// table. Outer columns are read unprefixed, inner ones as `inner_prefix`.
    fn correlation(
        &self,
        outer: &Rel,
        inner: &Rel,
        predicates: &[Expr],
        inner_prefix: &'static str,
    ) -> Result<Correlation> {
        let scopes = [Scope { id: outer.id, deref: outer.deref_map(), prefix: "" }];
        let outer_tx = self.tx(&outer.schema, &scopes);
        let local_tx = self.tx(&inner.schema, &[]);
        let both_tx = Tx {
            be: self.be,
            schema: &inner.schema,
            scopes: &scopes,
            col_prefix: inner_prefix,
            subs: &[],
            lambda: None,
        };
        let mut c = Correlation { keys: vec![], local: vec![], residual: vec![] };
        for p in conjuncts(predicates) {
            if !has_field(p) {
                c.local.push(local_tx.expr(p)?);
                continue;
            }
            if let Expr::Binary(BinaryOp::Eq, a, b) = p {
                let pair = if has_field(a) && !has_column(a) && !has_field(b) {
                    Some((a, b))
                } else if has_field(b) && !has_column(b) && !has_field(a) {
                    Some((b, a))
                } else {
                    None
                };
                if let Some((o, i)) = pair {
                    let t = key_type(o.dtype(&outer.schema)?, i.dtype(&inner.schema)?)?;
                    c.keys.push((typed(outer_tx.expr(o)?, &t), typed(both_tx.expr(i)?, &t)));
                    continue;
                }
            }
            c.residual.push(both_tx.expr(p)?);
        }
        Ok(c)
    }

    fn prefixed(&self, lf: pl::LazyFrame, schema: &Schema, prefix: &str) -> pl::LazyFrame {
        lf.select(schema.names().map(|n| pl::col(n).alias(format!("{prefix}{n}"))).collect::<Vec<_>>())
    }

    /// `EXISTS (inner WHERE ...)` as a semi join, or an anti join when negated.
    fn exists(&self, outer_lf: pl::LazyFrame, outer: &Rel, sub: &Rel, negated: bool) -> Result<pl::LazyFrame> {
        // what a subquery selects does not matter to EXISTS
        let mut sub = sub;
        while let RelOp::Project { input, .. } = &sub.op {
            sub = input;
        }
        let (inner, predicates): (&Rel, &[Expr]) = match &sub.op {
            RelOp::Filter { input, predicates } => (input, predicates),
            _ => (sub, &[]),
        };
        let c = self.correlation(outer, inner, predicates, RIGHT)?;
        if c.keys.is_empty() && c.residual.is_empty() {
            return unsupported("EXISTS over a subquery that does not refer to the outer table");
        }
        let inner_lf = self.prefixed(filtered(self.rel(inner)?, c.local), &inner.schema, RIGHT);
        let how = if negated { pl::JoinType::Anti } else { pl::JoinType::Semi };
        let (outer_on, inner_on): (Vec<_>, Vec<_>) = c.keys.into_iter().unzip();
        if c.residual.is_empty() {
            return Ok(outer_lf.join_builder().with(inner_lf).how(how).left_on(outer_on).right_on(inner_on).finish());
        }
        // Other conditions: number the outer rows, find the ones with a match
        // by joining and filtering, then keep (or drop) those rows.
        let numbered = outer_lf.with_row_index("__rid", None);
        let pairs = if outer_on.is_empty() {
            numbered.clone().cross_join(inner_lf, None)
        } else {
            numbered
                .clone()
                .join_builder()
                .with(inner_lf)
                .how(pl::JoinType::Inner)
                .left_on(outer_on)
                .right_on(inner_on)
                .finish()
        };
        let matched = filtered(pairs, c.residual).select([pl::col("__rid")]).unique(None, pl::UniqueKeepStrategy::Any);
        let on = [pl::col("__rid")];
        let kept = numbered.join_builder().with(matched).how(how).left_on(on.clone()).right_on(on).finish();
        Ok(kept.select(outer.schema.names().map(pl::col).collect::<Vec<_>>()))
    }

    /// A scalar subquery that reads the outer row, such as
    /// `(SELECT avg(x) FROM inner WHERE inner.k = outer.k)`: aggregate `inner`
    /// per key once and left join the result, as `column`. `None` if the
    /// subquery does not have that shape (it is then run on its own).
    fn join_scalar_subquery(
        &self,
        outer_lf: pl::LazyFrame,
        outer: &Rel,
        sub: &Rel,
        column: &str,
    ) -> Result<Option<pl::LazyFrame>> {
        // projections on top of the aggregate, outermost first
        let mut projects = vec![];
        let mut node = sub;
        while let RelOp::Project { input, exprs } = &node.op {
            projects.push((input, exprs));
            node = input;
        }
        let RelOp::Aggregate { input: source, groups, metrics } = &node.op else { return Ok(None) };
        let RelOp::Filter { input: inner, predicates } = &source.op else { return Ok(None) };
        if !groups.is_empty() || !predicates.iter().any(has_field) {
            return Ok(None);
        }
        let c = self.correlation(outer, inner, predicates, "")?;
        if !c.residual.is_empty() {
            return unsupported("a scalar subquery correlated by something other than an equality");
        }
        let keys: Vec<String> = (0..c.keys.len()).map(|i| format!("{column}_k{i}")).collect();
        let (outer_on, inner_on): (Vec<_>, Vec<_>) = c.keys.into_iter().unzip();
        let by = inner_on.into_iter().zip(&keys).map(|(e, k)| e.alias(k.as_str())).collect::<Vec<_>>();
        let tx = self.tx(&inner.schema, &[]);
        let aggs = metrics
            .iter()
            .map(|(n, e)| Ok(typed(tx.expr(e)?, &e.dtype(&inner.schema)?).alias(n.as_str())))
            .collect::<Result<Vec<_>>>()?;
        let mut lf = filtered(self.rel(inner)?, c.local).group_by(by).agg(aggs);
        for (input, exprs) in projects.iter().rev() {
            lf = self.project(lf, &input.schema, exprs, &keys)?;
        }
        let value = pl::col(sub.schema.fields[0].name.as_str());
        // a count over no rows is 0, not the NULL an unmatched left join gives
        let counts = projects.is_empty()
            && matches!(
                &metrics[0].1,
                Expr::Agg { func: AggFunc::Count | AggFunc::CountStar | AggFunc::CountDistinct, .. }
            );
        let mut cols: Vec<pl::Expr> = keys.iter().map(|k| pl::col(k.as_str())).collect();
        cols.push(value.alias(column));
        let joined = outer_lf
            .join_builder()
            .with(lf.select(cols))
            .how(pl::JoinType::Left)
            .left_on(outer_on)
            .right_on(keys.iter().map(|k| pl::col(k.as_str())).collect::<Vec<_>>())
            .coalesce(pl::JoinCoalesce::KeepColumns)
            .maintain_order(pl::MaintainOrderJoin::Left)
            .finish();
        Ok(Some(if counts { joined.with_columns([pl::col(column).fill_null(pl::lit(0))]) } else { joined }))
    }

    fn join(
        &self,
        left: &Rel,
        right: &Rel,
        how: JoinKind,
        predicates: &[Expr],
        outputs: &[(String, Side, String)],
    ) -> Result<pl::LazyFrame> {
        // Prefix every column so the two sides can never collide, whatever they are called.
        let mut l = self.prefixed(self.rel(left)?, &left.schema, LEFT);
        let mut r = self.prefixed(self.rel(right)?, &right.schema, RIGHT);
        let scopes = [
            Scope { id: left.id, deref: left.deref_map(), prefix: LEFT },
            Scope { id: right.id, deref: right.deref_map(), prefix: RIGHT },
        ];
        let tx = self.tx(&left.schema, &scopes);

        // Each condition is one of: an equality between the sides (a join key), a
        // constant, a filter on one side, or something else (inner joins only).
        let (mut left_on, mut right_on, mut residual, mut constant) = (vec![], vec![], vec![], vec![]);
        for p in conjuncts(predicates) {
            let mut refs = vec![];
            if !sides(&tx, p, &mut refs) {
                return unsupported("a join predicate that refers to an outer table");
            }
            let only = |s: &[usize], i: usize| !s.is_empty() && s.iter().all(|x| *x == i);
            if refs.is_empty() {
                if !matches!(p, Expr::Literal(Value::Bool(true), _)) {
                    constant.push(tx.expr(p)?);
                }
                continue;
            }
            if let Expr::Binary(BinaryOp::Eq, a, b) = p {
                let (mut sa, mut sb) = (vec![], vec![]);
                sides(&tx, a, &mut sa);
                sides(&tx, b, &mut sb);
                let pair = if only(&sa, 0) && only(&sb, 1) {
                    Some((a, b))
                } else if only(&sa, 1) && only(&sb, 0) {
                    Some((b, a))
                } else {
                    None
                };
                if let Some((lk, rk)) = pair {
                    let t = key_type(lk.dtype(&left.schema)?, rk.dtype(&right.schema)?)?;
                    left_on.push(typed(tx.expr(lk)?, &t));
                    right_on.push(typed(tx.expr(rk)?, &t));
                    continue;
                }
            }
            // A condition on one side alone filters that side before the join,
            // as long as the join does not have to keep that side's unmatched rows.
            use JoinKind::*;
            if only(&refs, 0) && matches!(how, Inner | Right | Semi) {
                l = l.filter(tx.expr(p)?);
            } else if only(&refs, 1) && matches!(how, Inner | Left | Semi | Anti) {
                r = r.filter(tx.expr(p)?);
            } else if how == Inner {
                residual.push(tx.expr(p)?);
            } else {
                return unsupported(format!(
                    "{how:?} join on a condition that is not an equality between the two sides"
                ));
            }
        }

        let kind = match how {
            JoinKind::Inner => pl::JoinType::Inner,
            JoinKind::Left => pl::JoinType::Left,
            JoinKind::Right => pl::JoinType::Right,
            JoinKind::Outer => pl::JoinType::Full,
            JoinKind::Semi => pl::JoinType::Semi,
            JoinKind::Anti => pl::JoinType::Anti,
        };
        let joined = if left_on.is_empty() && constant.is_empty() && how == JoinKind::Inner {
            l.cross_join(r, None)
        } else {
            if left_on.is_empty() || !constant.is_empty() {
                // A constant key on each side: equal when the constant conditions hold (every
                // row pairs with every row), different when they do not (nothing pairs).
                let holds = all_of(constant).unwrap_or(pl::lit(true));
                l = l.with_columns([pl::lit(1i32).alias("__lk")]);
                r = r.with_columns([pl::when(holds).then(pl::lit(1i32)).otherwise(pl::lit(0i32)).alias("__rk")]);
                left_on.push(pl::col("__lk"));
                right_on.push(pl::col("__rk"));
            }
            l.join_builder()
                .with(r)
                .how(kind)
                .left_on(left_on)
                .right_on(right_on)
                .coalesce(pl::JoinCoalesce::KeepColumns)
                .maintain_order(pl::MaintainOrderJoin::LeftRight)
                .finish()
        };
        let joined = filtered(joined, residual);
        let out = outputs.iter().map(|(name, side, src)| {
            let prefix = if *side == Side::Left { LEFT } else { RIGHT };
            pl::col(format!("{prefix}{src}")).alias(name.as_str())
        });
        Ok(joined.select(out.collect::<Vec<_>>()))
    }

    fn set_op(&self, rel: &Rel, kind: SetKind, distinct: bool, left: &Rel, right: &Rel) -> Result<pl::LazyFrame> {
        // columns are matched by name; both sides take the output types
        let shaped = |r: &Rel| -> Result<pl::LazyFrame> {
            let cols = rel.schema.fields.iter().map(|f| typed(pl::col(f.name.as_str()), &f.dtype)).collect::<Vec<_>>();
            Ok(self.rel(r)?.select(cols))
        };
        let (l, r) = (shaped(left)?, shaped(right)?);
        let names: Vec<pl::Expr> = rel.schema.names().map(pl::col).collect();
        let unique = |lf: pl::LazyFrame| lf.unique_stable(None, pl::UniqueKeepStrategy::First);
        if kind == SetKind::Union {
            let both = pl::concat([l, r], pl::UnionArgs::default()).map_err(pl_err)?;
            return Ok(if distinct { unique(both) } else { both });
        }
        let how = if kind == SetKind::Intersect { pl::JoinType::Semi } else { pl::JoinType::Anti };
        let matching = |l: pl::LazyFrame, r: pl::LazyFrame, on: Vec<pl::Expr>| {
            l.join_builder().with(r).how(how.clone()).left_on(on.clone()).right_on(on).join_nulls(true).finish()
        };
        if distinct {
            return Ok(matching(unique(l), r, names));
        }
        // ALL: the nth copy of a row matches the nth copy on the other side
        let nth = pl::int_range(pl::lit(0), pl::len(), 1, pl::DataType::Int64)
            .over(names.clone())
            .map_err(pl_err)?
            .alias("__n");
        let mut on = names.clone();
        on.push(pl::col("__n"));
        Ok(matching(l.with_columns([nth.clone()]), r.with_columns([nth]), on).select(names))
    }
}
