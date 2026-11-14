//! The Rust side of each conformance case, keyed by the same Ibis pytest id as
//! conformance/cases.py. Keep the two files in the same order.
//!
//! `notimpl` mirrors Ibis's `pytest.mark.notimpl`: backends where the case is
//! known not to pass yet. A marked case that passes is reported as `xpass`.

use ibis_core::prelude::*;
use ibis_core::{Expr, Result};

pub enum Shape {
    /// fn(t) -> column expr; run as `t.select(id, tmp=expr)` (or aggregate if a reduction).
    Column(Box<dyn Fn(&Table) -> Result<Expr>>),
    /// fn(t) -> table expr.
    Table(Box<dyn Fn(&Table) -> Result<Table>>),
}

pub struct Case {
    pub id: String,
    pub shape: Shape,
    pub ordered: bool,
    pub notimpl: Vec<&'static str>,
}

fn column(id: &str, f: impl Fn(&Table) -> Result<Expr> + 'static) -> Case {
    Case { id: id.into(), shape: Shape::Column(Box::new(f)), ordered: false, notimpl: vec![] }
}

fn table(id: &str, f: impl Fn(&Table) -> Result<Table> + 'static) -> Case {
    Case { id: id.into(), shape: Shape::Table(Box::new(f)), ordered: false, notimpl: vec![] }
}

impl Case {
    #[allow(dead_code)] // used as cases get marked
    fn notimpl(mut self, backends: &[&'static str]) -> Self {
        self.notimpl.extend(backends);
        self
    }
}

fn c(name: &str) -> Expr {
    col(name)
}

pub fn all() -> Vec<Case> {
    let mut v = vec![];
    let s = |id: &str| format!("test_string[{id}]");

    // --- test_string ----------------------------------------------------------
    v.push(column(&s("contains"), |_| Ok(c("string_col").contains("6"))));
    v.push(column(&s("like"), |_| Ok(c("string_col").like("6%"))));
    v.push(column(&s("complex_like_escape"), |_| Ok(c("string_col").like("6^%"))));
    v.push(column(&s("complex_like_escape_match"), |_| Ok(c("string_col").like("6^%%"))));
    v.push(column(&s("ilike"), |_| Ok(c("string_col").ilike("6%"))));
    v.push(column(&s("rlike"), |_| Ok(c("string_col").rlike("0|1|2|3|4|5|6|7|8|9"))));
    v.push(column(&s("re_search_substring"), |_| Ok((lit("a") + c("string_col") + lit("a")).re_search(r"\d+"))));
    v.push(column(&s("re_search"), |_| Ok(c("string_col").re_search(r"\d+"))));
    v.push(column(&s("re_search_posix"), |_| Ok(c("string_col").re_search(r"[[:digit:]]+"))));
    v.push(column(&s("re_extract"), |_| Ok((lit("xyz") + c("string_col") + lit("abcd")).re_extract(r"(\d+)", 0))));
    v.push(column(&s("re_extract_group"), |_| {
        Ok((lit("xyz") + c("string_col") + lit("abcd")).re_extract(r"(\d+)abc", 1))
    }));
    v.push(column(&s("re_extract_posix"), |_| Ok(c("string_col").re_extract(r"([[:digit:]]+)", 1))));
    v.push(column(&s("re_extract_whole_group"), |_| Ok((c("string_col") + lit("1")).re_extract(r"\d(\d+)", 0))));
    v.push(column(&s("re_extract_group_1"), |_| Ok(c("date_string_col").re_extract(r"(\d+)\D(\d+)\D(\d+)", 1))));
    v.push(column(&s("re_extract_group_2"), |_| Ok(c("date_string_col").re_extract(r"(\d+)\D(\d+)\D(\d+)", 2))));
    v.push(column(&s("re_extract_group_3"), |_| Ok(c("date_string_col").re_extract(r"(\d+)\D(\d+)\D(\d+)", 3))));
    v.push(column(&s("re_extract_group_at_beginning"), |_| Ok(c("date_string_col").re_extract(r"^(\d+)", 1))));
    v.push(column(&s("re_extract_group_at_end"), |_| Ok(c("date_string_col").re_extract(r"(\d+)$", 1))));
    v.push(column(&s("re_replace_posix"), |_| Ok(c("string_col").re_replace(r"[[:digit:]]+", "a"))));
    v.push(column(&s("re_replace"), |_| Ok(c("string_col").re_replace(r"\d+", "a"))));
    v.push(column(&s("repeat_method"), |_| Ok(c("string_col").repeat(2))));
    v.push(column(&s("repeat_left"), |_| Ok(lit(2) * c("string_col"))));
    v.push(column(&s("repeat_right"), |_| Ok(c("string_col") * 2)));
    v.push(column(&s("find"), |_| Ok(c("string_col").find("a", None))));
    v.push(column(&s("find_start"), |_| Ok(c("date_string_col").find("13", Some(3)))));
    v.push(column(&s("lpad"), |_| Ok(c("string_col").lpad(10, "a"))));
    v.push(column(&s("rpad"), |_| Ok(c("string_col").rpad(10, "a"))));
    v.push(column(&s("lower"), |_| Ok(c("string_col").lower())));
    v.push(column(&s("upper"), |_| Ok(c("string_col").upper())));
    v.push(column(&s("reverse"), |_| Ok(c("string_col").reverse())));
    v.push(column(&s("ascii_str"), |_| Ok(c("string_col").ascii_str())));
    v.push(column(&s("length"), |_| Ok(c("string_col").length())));
    let abcd = || c("int_col").cases([(lit(1), lit("abcd")), (lit(2), lit("ABCD"))], Some(lit("dabc")));
    v.push(column(&s("startswith"), move |_| Ok(abcd().startswith("abc"))));
    v.push(column(&s("endswith"), move |_| Ok(abcd().endswith("bcd"))));
    v.push(column(&s("startswith-simple"), |_| Ok(c("date_string_col").startswith("2010-01"))));
    v.push(column(&s("endswith-simple"), |_| Ok(c("date_string_col").endswith("/10"))));
    v.push(column(&s("strip"), |_| Ok(c("string_col").strip())));
    v.push(column(&s("lstrip"), |_| Ok(c("string_col").lstrip())));
    v.push(column(&s("rstrip"), |_| Ok(c("string_col").rstrip())));
    v.push(column(&s("concat_columns"), |_| Ok(c("string_col") + c("date_string_col"))));
    v.push(column(&s("concat_column_scalar"), |_| Ok(c("string_col") + lit("a"))));
    v.push(column(&s("concat_scalar_column"), |_| Ok(lit("a") + c("string_col"))));
    v.push(column(&s("replace"), |_| Ok(c("string_col").replace("1", "42"))));

    // --- test_substring (on date_string_col) ----------------------------------
    let sub = |id: &str| format!("test_substring[{id}]");
    v.push(column(&sub("substr"), |_| Ok(c("date_string_col").substr(2, Some(lit(3))))));
    v.push(column(&sub("substr-start-only"), |_| Ok(c("date_string_col").substr(2, None))));
    v.push(column(&sub("left"), |_| Ok(c("date_string_col").left(2))));
    v.push(column(&sub("right"), |_| Ok(c("date_string_col").right(2))));

    // --- test_numeric ----------------------------------------------------------
    let m = |id: &str| format!("test_simple_math_functions_columns[{id}]");
    v.push(column(&m("abs-neg"), |_| Ok((-c("double_col")).abs())));
    v.push(column(&m("abs"), |_| Ok(c("double_col").abs())));
    v.push(column(&m("ceil"), |_| Ok(c("double_col").ceil())));
    v.push(column(&m("floor"), |_| Ok(c("double_col").floor())));
    v.push(column(&m("sign"), |_| Ok(c("double_col").sign())));
    v.push(column(&m("sign-negative"), |_| Ok((-c("double_col")).sign())));
    let cm = |id: &str| format!("test_complex_math_functions_columns[{id}]");
    v.push(column(&cm("sqrt"), |_| Ok((c("double_col") + 1).sqrt())));
    v.push(column(&cm("exp"), |_| Ok((c("double_col") + 1).exp())));
    v.push(column(&cm("log2"), |_| Ok((c("double_col") + 1).log2())));
    v.push(column(&cm("ln"), |_| Ok((c("double_col") + 1).ln())));
    v.push(column(&cm("log10"), |_| Ok((c("double_col") + 1).log10())));
    let b = |id: &str| format!("test_binary_arithmetic_operations[{id}]");
    let small = || c("smallint_col") + 1;
    v.push(column(&b("add"), move |_| Ok(c("double_col") + small())));
    v.push(column(&b("sub"), move |_| Ok(c("double_col") - small())));
    v.push(column(&b("mul"), move |_| Ok(c("double_col") * small())));
    v.push(column(&b("truediv"), move |_| Ok(c("double_col") / small())));
    v.push(column(&b("floordiv"), move |_| Ok(c("double_col").floordiv(small()))));
    v.push(column(&b("pow"), move |_| Ok(c("double_col").pow(small()))));
    v.push(column("test_mod", move |_| Ok(c("smallint_col") % small())));
    v.push(column("test_floating_mod", move |_| Ok(c("double_col") % small())));

    // --- test_aggregation ------------------------------------------------------
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

    v
}
