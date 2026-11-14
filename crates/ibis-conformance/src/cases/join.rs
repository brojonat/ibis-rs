//! ibis/backends/tests/test_join.py

use ibis_core::prelude::*;
use ibis_core::{Expr, Result};

use super::{Case, Fx, c, tables};

fn left_right(fx: &Fx) -> Result<(Table, Table)> {
    let left = fx.batting.filter([fx.batting.col("yearID").eq(2015)])?;
    let right = fx.awards_players.filter([fx.awards_players.col("lgID").eq(lit("NL"))])?.drop(&["yearID", "lgID"])?;
    Ok((left, right))
}

pub fn register(v: &mut Vec<Case>) {
    let hows =
        [("inner", JoinKind::Inner), ("left", JoinKind::Left), ("right", JoinKind::Right), ("outer", JoinKind::Outer)];

    for (id, how) in hows {
        v.push(tables(&format!("test_mutating_join[{id}]"), move |fx| {
            let (left, right) = left_right(fx)?;
            left.join(&right, ["playerID"], how)
        }));
    }
    for (id, how) in [("semi", JoinKind::Semi), ("anti", JoinKind::Anti)] {
        v.push(tables(&format!("test_filtering_join[{id}]"), move |fx| {
            let (left, right) = left_right(fx)?;
            left.join(&right, ["playerID"], how)
        }));
    }

    v.push(tables("test_join_then_filter_no_column_overlap", |fx| {
        let left = fx.batting.filter([fx.batting.col("yearID").eq(2015)])?;
        let left = left.select([left.col("yearID").name("year"), c("RBI")])?;
        let right = fx.awards_players.filter([fx.awards_players.col("lgID").eq(lit("NL"))])?;
        let expr = left.join(&right, [left.col("year").eq(right.col("yearID"))], JoinKind::Inner)?;
        expr.filter([expr.col("RBI").eq(9)])
    }));
    v.push(tables("test_mutate_then_join_no_column_overlap", |fx| {
        let left = fx.batting.mutate([fx.batting.col("yearID").name("year")])?.filter([c("year").eq(2015)])?;
        let left = left.select(["year", "RBI"])?;
        let right = &fx.awards_players;
        left.join(right, [left.col("year").eq(right.col("yearID"))], JoinKind::Inner)?.aggregate([count().name("n")])
    }));

    // Python passes True / ibis.literal(True) / lists of them: all the same
    // predicates here, so each id maps to (value, how many).
    let trivial: [(&str, bool, usize); 16] = [
        ("true", true, 1),
        ("true-literal", true, 1),
        ("true-list", true, 1),
        ("true-literal-list", true, 1),
        ("true-true-list", true, 2),
        ("true-true-literal-list", true, 2),
        ("true-true-const-expr-list", true, 2),
        ("true-true-expr-const-list", true, 2),
        ("false", false, 1),
        ("false-literal", false, 1),
        ("false-list", false, 1),
        ("false-literal-list", false, 1),
        ("false-false-list", false, 2),
        ("false-false-literal-list", false, 2),
        ("false-false-const-expr-list", false, 2),
        ("false-false-expr-const-list", false, 2),
    ];
    for (hid, how) in hows {
        for (pid, value, n) in trivial {
            v.push(tables(&format!("test_join_with_trivial_predicate[{hid}-{pid}]"), move |fx| {
                let base = fx.awards_players.limit(5, 0);
                let left = base.select([c("playerID").name("left_key")])?;
                let right = base.select([c("playerID").name("right_key")])?;
                let predicates: Vec<Expr> = vec![lit(value); n];
                left.join(&right, predicates, how)?.aggregate([count().name("n")])
            }));
        }
    }

    let nullability = [
        ("left-xy", JoinKind::Left, true),
        ("left-x", JoinKind::Left, false),
        ("right-xy", JoinKind::Right, true),
        ("right-x", JoinKind::Right, false),
        ("outer-xy", JoinKind::Outer, true),
        ("outer-x", JoinKind::Outer, false),
    ];
    for (id, how, renamed) in nullability {
        v.push(tables(&format!("test_outer_join_nullability[{id}]"), move |fx| {
            let left = fx.mem("win")?.select([c("x")])?.filter([c("x").isin([1, 2].map(lit))])?;
            if renamed {
                let right = left.filter([c("x").eq(1)])?.select([c("x").name("y")])?;
                left.join(&right, [("x", "y")], how)
            } else {
                let right = left.filter([c("x").eq(1)])?;
                left.join(&right, ["x"], how)
            }
        }));
    }

    v.push(tables("test_join_conflicting_columns", |fx| {
        fx.mem("xyz1")?.join(&fx.mem("xyz2")?, ["x"], JoinKind::Inner)
    }));
}
