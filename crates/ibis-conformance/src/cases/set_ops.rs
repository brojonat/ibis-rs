//! ibis/backends/tests/test_set_ops.py

use ibis_core::Result;
use ibis_core::prelude::*;

use super::{Case, c, table};

// The union_subsets fixture's shuffled column orders (random.seed(89)).
const COLS_A: [&str; 13] = [
    "id",
    "timestamp_col",
    "date_string_col",
    "float_col",
    "year",
    "smallint_col",
    "double_col",
    "month",
    "bigint_col",
    "tinyint_col",
    "int_col",
    "string_col",
    "bool_col",
];
const COLS_B: [&str; 13] = [
    "bigint_col",
    "float_col",
    "timestamp_col",
    "date_string_col",
    "year",
    "int_col",
    "month",
    "smallint_col",
    "id",
    "double_col",
    "tinyint_col",
    "string_col",
    "bool_col",
];
const COLS_C: [&str; 13] = [
    "string_col",
    "int_col",
    "id",
    "date_string_col",
    "bigint_col",
    "tinyint_col",
    "smallint_col",
    "timestamp_col",
    "double_col",
    "float_col",
    "month",
    "bool_col",
    "year",
];

fn ids(t: &Table, lo: i64, hi: i64) -> Result<Table> {
    t.filter([c("id").ge(lo) & c("id").le(hi)])
}

fn subsets(t: &Table) -> Result<(Table, Table, Table)> {
    Ok((ids(t, 5200, 5210)?.select(COLS_A)?, ids(t, 5205, 5215)?.select(COLS_B)?, ids(t, 5213, 5220)?.select(COLS_C)?))
}

pub fn register(v: &mut Vec<Case>) {
    for (id, distinct) in [("all", false), ("distinct", true)] {
        let union = table(&format!("test_union[{id}]"), move |t| {
            let (a, b, _) = subsets(t)?;
            a.union(&b, distinct)?.order_by(["id"])
        });
        v.push(if distinct { union.ordered() } else { union });

        // INTERSECT ALL / EXCEPT ALL: SQLite has neither (Python Ibis fails
        // there); ibis-sqlite emulates them.
        let intersect = table(&format!("test_intersect[{id}]"), move |t| {
            let (a, b, c) = (ids(t, 5200, 5210)?, ids(t, 5205, 5215)?, ids(t, 5195, 5208)?);
            a.intersect(&b, distinct)?.intersect(&c, distinct)?.order_by(["id"])
        })
        .ordered();
        v.push(intersect);

        let difference = table(&format!("test_difference[{id}]"), move |t| {
            let (a, b, c) = (ids(t, 5200, 5210)?, ids(t, 5205, 5215)?, ids(t, 5195, 5202)?);
            a.difference(&b, distinct)?.difference(&c, distinct)?.order_by(["id"])
        })
        .ordered();
        v.push(difference);
    }

    v.push(table("test_union_mixed_distinct", |t| {
        let (a, b, c) = subsets(t)?;
        a.union(&b, true)?.union(&c, false)?.order_by(["id"])
    }));

    for (oid, ordered) in [("False", false), ("True", true)] {
        for (did, distinct) in [("True", true), ("False", false)] {
            v.push(table(&format!("test_top_level_union[{oid}-{did}]"), move |t| {
                let side = |n: i64| -> Result<Table> {
                    let s = t.select([c("bigint_col").name("a")])?.filter([c("a").eq(n)])?.distinct();
                    if ordered { s.order_by(["a"]) } else { Ok(s) }
                };
                Ok(side(10)?.union(&side(20)?, distinct)?.limit(2, 0))
            }));

            for opname in ["intersect", "difference"] {
                v.push(table(&format!("test_top_level_intersect_difference[{oid}-{opname}-{did}]"), move |t| {
                    let side = |x: i64, y: i64| -> Result<Table> {
                        let s =
                            t.select([c("bigint_col").name("a")])?.filter([c("a").eq(x) | c("a").eq(y)])?.distinct();
                        if ordered { s.order_by(["a"]) } else { Ok(s) }
                    };
                    let (t1, t2) = (side(10, 20)?, side(20, 30)?);
                    let out = if opname == "intersect" {
                        t1.intersect(&t2, distinct)?
                    } else {
                        t1.difference(&t2, distinct)?
                    };
                    Ok(out.limit(2, 0))
                }));
            }
        }
    }
}
