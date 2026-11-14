//! ibis/backends/tests/test_aggregation.py

use ibis_core::prelude::*;

use super::{Case, c, table};

pub fn cases(v: &mut Vec<Case>) {
    let aggs: Vec<(&str, fn() -> Expr)> = vec![
        ("mean", || c("double_col").mean()),
        ("min", || c("double_col").min()),
        ("max", || c("double_col").max()),
        ("complex_sum", || (c("double_col") + 5).sum()),
        ("timestamp_max", || c("timestamp_col").max()),
    ];
    for (id, f) in &aggs {
        let f = *f;
        v.push(table(&format!("test_aggregate[{id}]"), move |t| t.aggregate([f().name("tmp")])));
    }
    for (id, f) in &aggs {
        let f = *f;
        v.push(table(&format!("test_aggregate_grouped[{id}]"), move |t| {
            t.group_by(["bigint_col"]).agg([f().name("tmp")])
        }));
    }

    let reductions: Vec<(&str, fn() -> Expr)> = vec![
        ("count", || c("bool_col").count()),
        ("nunique", || c("bool_col").nunique()),
        ("any", || c("bool_col").any()),
        ("any_negate", || c("bool_col").any()),
        ("all", || c("bool_col").all()),
        ("all_negate", || c("bool_col").all()),
        ("sum", || c("double_col").sum()),
        ("bool_sum", || c("int_col").gt(0).sum()),
        ("mean", || c("double_col").mean()),
        ("min", || c("double_col").min()),
        ("max", || c("double_col").max()),
        ("std", || c("double_col").std()),
        ("var", || c("double_col").var()),
        ("std_pop", || c("double_col").std_pop()),
        ("var_pop", || c("double_col").var_pop()),
        ("count_star", count),
    ];
    let conds: Vec<(&str, fn() -> Option<Expr>)> =
        vec![("no_cond", || None), ("is_in", || Some(c("string_col").isin([lit("1"), lit("7")])))];
    for (cid, cond) in &conds {
        for (rid, red) in &reductions {
            let (cond, red, negate) = (*cond, *red, rid.ends_with("_negate"));
            v.push(table(&format!("test_reduction_ops[{cid}-{rid}]"), move |t| {
                let mut e = red();
                if let Some(w) = cond() {
                    e = e.filter_where(w);
                }
                if negate {
                    e = !e;
                }
                t.filter([c("id").lt(1550)])?.aggregate([e.name("tmp")])
            }));
        }
    }
}
