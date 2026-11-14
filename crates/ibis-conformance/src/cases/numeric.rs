//! ibis/backends/tests/test_numeric.py

use super::{Case, c, column};

pub fn cases(v: &mut Vec<Case>) {
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
}
