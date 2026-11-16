// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! ibis/backends/tests/test_numeric.py

use ibis_core::prelude::*;

use super::{Case, c, column, scalar, table};

pub fn register(v: &mut Vec<Case>) {
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

    more(v);
}

type Num = fn() -> Expr;

/// The rest of test_numeric.py, in the order of the Python file.
fn more(v: &mut Vec<Case>) {
    // SQLite has no NaN: it stores NULL, so nothing there can be (or be
    // tested for) NaN. Ibis marks these notimpl on SQLite too.
    let no_nan: &[&str] = &["sqlite"];
    // MySQL has neither NaN nor infinity, and no way to write one.
    let no_inf: &[&str] = &["mysql"];

    let literals: [(&str, Num); 4] = [
        ("float-literal", || lit(1.3)),
        ("nan-literal", || lit(f64::NAN)),
        ("inf-literal", || lit(f64::INFINITY)),
        ("-inf-literal", || lit(f64::NEG_INFINITY)),
    ];
    for (fid, f) in [("isnan", Expr::isnan as fn(Expr) -> Expr), ("isinf", Expr::isinf)] {
        for col in ["float", "double"] {
            let case =
                column(&format!("test_isnan_isinf[{fid}-{col}-column]"), move |_| Ok(f(c(&format!("{col}_col")))));
            v.push(if fid == "isnan" { case.notimpl(no_nan) } else { case });
        }
        for (oid, operand) in literals {
            let case = scalar(&format!("test_isnan_isinf[{fid}-{oid}]"), move || Ok(f(operand())));
            let case = if fid == "isnan" || oid == "nan-literal" { case.notimpl(no_nan) } else { case };
            v.push(if oid == "float-literal" { case } else { case.notimpl(no_inf) });
        }
    }

    let math: [(&str, Num); 17] = [
        ("abs-neg", || lit(-5).abs()),
        ("abs", || lit(5).abs()),
        ("least", || least([lit(10), lit(1)])),
        ("greatest", || greatest([lit(10), lit(1)])),
        ("round", || lit(5.5).round(None)),
        ("round-digits", || lit(5.556).round(Some(2))),
        ("ceil", || lit(5.556).ceil()),
        ("floor", || lit(5.556).floor()),
        ("exp", || lit(5.556).exp()),
        ("sign-pos", || lit(5.556).sign()),
        ("sign-neg", || lit(-5.556).sign()),
        ("sign-zero", || lit(0).sign()),
        ("sqrt", || lit(5.556).sqrt()),
        ("log-base", || lit(5.556).log(2)),
        ("ln0", || lit(5.556).ln()),
        ("ln1", || lit(5.556).ln()),
        ("log2", || lit(5.556).log2()),
    ];
    for (id, f) in math {
        v.push(scalar(&format!("test_math_functions_literals[{id}]"), move || Ok(f())));
    }
    // Ibis lists these four three times each (pytest numbers the duplicate ids)
    for i in 0..3 {
        let m = |id: String, f: Num| scalar(&format!("test_math_functions_literals[{id}]"), move || Ok(f()));
        v.push(m(format!("log10_{i}"), || lit(5.556).log10()));
        v.push(m(format!("radians{i}"), || lit(5.556).radians()));
        v.push(m(format!("degrees{i}"), || lit(5.556).degrees()));
        v.push(m(format!("mod{i}"), || lit(11) % 3));
    }

    let trig: [(&str, Num); 8] = [
        ("acos", || lit(0.0).acos()),
        ("asin", || lit(0.0).asin()),
        ("atan", || lit(0.0).atan()),
        ("atan2", || lit(0.0).atan2(1.0)),
        ("cos", || lit(0.0).cos()),
        ("cot", || lit(1.0).cot()),
        ("sin", || lit(0.0).sin()),
        ("tan", || lit(0.0).tan()),
    ];
    for (id, f) in trig {
        v.push(scalar(&format!("test_trig_functions_literals[{id}]"), move || Ok(f())));
    }

    // double_col scaled by its max (90.9), as the Ibis test does
    let dc_max = 90.9;
    let trig_columns: [(&str, Num); 7] = [
        ("acos", || c("dc").acos()),
        ("asin", || c("dc").asin()),
        ("atan", || c("dc").atan()),
        ("atan2", || c("dc").atan2(c("dc"))),
        ("cos", || c("dc").cos()),
        ("sin", || c("dc").sin()),
        ("tan", || c("dc").tan()),
    ];
    for (id, f) in trig_columns {
        v.push(table(&format!("test_trig_functions_columns[{id}]"), move |t| {
            t.mutate([(c("double_col") / dc_max).name("dc")])?.select([f().name("tmp")])
        }));
    }
    v.push(table("test_cotangent", move |t| t.select([(lit(0.5) + c("double_col") / dc_max).cot().name("tmp")])));
    v.push(scalar("test_floor_divide_precedence", || Ok(lit(16).floordiv(lit(4) / lit(2)))));

    let n = |id: &str| format!("test_backend_specific_numerics[{id}]");
    v.push(column(&n("round"), |_| Ok(c("double_col").round(None))));
    v.push(column(&n("round-with-param"), |_| Ok((c("double_col") + 0.05).round(Some(3)))));
    v.push(column(&n("least-all-columns"), |_| Ok(least([c("bigint_col"), c("int_col")]))));
    v.push(column(&n("least-scalar"), |_| Ok(least([c("bigint_col"), c("int_col"), lit(-2)]))));
    v.push(column(&n("greatest-all-columns"), |_| Ok(greatest([c("bigint_col"), c("int_col")]))));
    v.push(column(&n("greatest-scalar"), |_| Ok(greatest([c("bigint_col"), c("int_col"), lit(-2)]))));
    v.push(scalar("test_integer_truediv", || Ok(lit(1) / lit(2))));

    // x / 0 is +/-Infinity and 0 / 0 is NaN. Every column has zeros, so
    // every case has NaNs, which SQLite and MySQL can't represent.
    for (did, zero) in [("0", lit(0) as Expr), ("0.0", lit(0.0))] {
        for col in ["tinyint_col", "smallint_col", "int_col", "bigint_col", "float_col", "double_col"] {
            let zero = zero.clone();
            v.push(
                column(&format!("test_divide_by_zero[{col}-{did}]"), move |_| Ok(c(col) / zero.clone()))
                    .notimpl(no_nan)
                    .notimpl(no_inf),
            );
        }
    }

    let clip: [(&str, fn(Expr) -> Expr); 8] = [
        ("lower-int", |x| x.clip(Some(lit(0)), None)),
        ("lower-float", |x| x.clip(Some(lit(0.0)), None)),
        ("upper-int", |x| x.clip(None, Some(lit(0)))),
        ("lower-upper-expr", |x| x.clone().clip(Some(x.clone() - 1), Some(x + 1))),
        ("lower-upper-int", |x| x.clip(Some(lit(0)), Some(lit(1)))),
        ("lower-upper-float", |x| x.clip(Some(lit(0)), Some(lit(1.0)))),
        ("null-lower", |x| x.nullif(1).clip(Some(lit(0)), None)),
        ("null-upper", |x| x.nullif(1).clip(None, Some(lit(0)))),
    ];
    for (id, f) in clip {
        v.push(column(&format!("test_clip[{id}]"), move |_| Ok(f(c("int_col")))));
    }
    v.push(scalar("test_constants[pi]", || Ok(pi())));
    v.push(scalar("test_constants[e]", || Ok(e())));
}
