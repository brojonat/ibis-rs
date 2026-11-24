// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! ibis/backends/tests/test_string.py

use ibis_core::prelude::*;

use super::{Case, c, column, scalar, table, tables};

pub fn register(v: &mut Vec<Case>) {
    let s = |id: &str| format!("test_string[{id}]");
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

    rest(v);
}

/// What is left of test_string.py, in the order of the Python file.
fn rest(v: &mut Vec<Case>) {
    use DataType as T;
    // no arrays to split a string into
    let no_arrays: &[&str] = &super::NO_NESTED;
    v.push(scalar("test_string_literal[string]", || Ok(lit("STRING"))));
    v.push(scalar("test_string_literal[string-quote1]", || Ok(lit("STRI'NG"))));
    v.push(scalar("test_string_literal[string-quote2]", || Ok(lit("STRI\"NG"))));
    v.push(column("test_string_col_is_unicode", |_| Ok(c("string_col"))));

    let s = |id: &str| format!("test_string[{id}]");
    // MySQL has no TRANSLATE, and Polars nothing that maps characters (as in Ibis)
    v.push(column(&s("translate"), |_| Ok(c("string_col").translate("01", "ab"))).notimpl(&["mysql", "polars"]));
    v.push(column(&s("find_in_set"), |_| Ok(c("string_col").find_in_set(["1"]))));
    v.push(column(&s("find_in_set_all_missing"), |_| Ok(c("string_col").find_in_set(["a"]))));
    v.push(column(&s("split"), |_| Ok(c("date_string_col").split("/"))).notimpl(no_arrays));
    v.push(column(&s("join"), |_| Ok(lit("-").join([lit("a"), c("string_col"), lit("c")]))));

    // Python's `s[1:3]`, `s[2]`, `s[-2]`: `slice` and `at`, as on an array
    let sub = |id: &str| format!("test_substring[{id}]");
    let d = || c("date_string_col");
    v.push(column(&sub("slice"), move |_| Ok(d().slice(Some(1), Some(3)))));
    v.push(column(&sub("positive-index"), move |_| Ok(d().at(2))));
    v.push(column(&sub("negative-index"), move |_| Ok(d().at(-2))));
    v.push(column(&sub("expr_slice_begin"), move |_| Ok(d().substr(d().length() - 1, None))));
    v.push(column(&sub("expr_slice_end"), move |_| Ok(d().substr(0, Some(d().length())))));
    v.push(column(&sub("expr_empty_slice"), move |_| Ok(d().slice(None, None))));
    v.push(column(&sub("expr_slice_begin_end"), move |_| {
        Ok(d().substr(d().length() - 2, Some((d().length() - 1) - (d().length() - 2))))
    }));

    v.push(scalar("test_re_replace_global", || Ok(lit("aba").re_replace("a", "c"))));
    v.push(table("test_substr_with_null_values", |t| {
        let picked = cases([(c("bool_col"), c("string_col"))], None).substr(0, Some(lit(2)));
        t.select([c("id"), picked.name("substr_col_null")])
    }));

    let capitalize: [(&str, Option<&str>); 10] = [
        ("none", None),
        ("empty", Some("")),
        ("no_change", Some("Abc")),
        ("lower_to_upper", Some("abc")),
        ("mixed_to_upper", Some("aBC")),
        ("leading_space", Some(" abc")),
        ("leading_digit", Some("9abc")),
        ("mixed_with_space", Some("aBc dEf")),
        ("mixed_with_hyphen", Some("aBc-dEf")),
        ("mixed_with_digit", Some("aBc1dEf")),
    ];
    for (id, input) in capitalize {
        v.push(scalar(&format!("test_capitalize[{id}]"), move || Ok(lit_as(input, T::String).capitalize())));
    }

    let abc = || array([lit("a"), lit("b"), lit("c")]);
    v.push(scalar("test_array_string_join#1", move || Ok(lit(",").join([abc()]))).notimpl(no_arrays));
    v.push(scalar("test_array_string_join#2", move || Ok(abc().join([lit(",")]))).notimpl(no_arrays));
    v.push(scalar("test_subs_with_re_replace", || {
        Ok(lit("hi").re_replace("i", "a").substitute([("d", "b")], Some(lit("k"))))
    }));
    v.push(scalar("test_multiple_subs", || Ok(lit("foo").substitute([("foo", "FOO"), ("bar", "BAR")], None))));
    // the two ids differ in whether Python wraps the other string in a literal
    for id in ["python", "ibis"] {
        v.push(
            scalar(&format!("test_levenshtein[{id}]"), || Ok(lit("kitten").levenshtein("sitting")))
                .notimpl(&["mysql", "polars"]),
        );
    }
    let percent = |id: &str| format!("test_no_conditional_percent_escape[{id}]");
    v.push(scalar(&percent("case"), || Ok(cases([(lit(true), lit("%"))], None))));
    v.push(scalar(&percent("ifelse"), || Ok(lit(true).ifelse(lit("%"), null(T::Null)))));
    v.push(scalar("test_non_match_regex_search_is_false", || Ok(lit("foo").re_search("bar"))));
    v.push(scalar("test_re_split", || Ok(lit(",a,,,,c").re_split(",+"))).notimpl(no_arrays));
    v.push(column("test_re_split_column", |_| Ok(c("string_col").re_split(r"\d+"))).notimpl(no_arrays));
    v.push(
        table("test_re_split_column_multiple_patterns", |t| {
            let pattern = c("string_col").eq(lit("1")).ifelse(lit("0|1"), lit(r"\d+"));
            t.filter([c("string_col").isin([lit("1"), lit("2")])])?
                .select([c("id"), c("string_col").re_split(pattern).name("splits")])
        })
        .notimpl(no_arrays)
        // one pattern for the whole column there (as in Ibis)
        .notimpl(&["polars", "clickhouse"]),
    );

    let none = || null(T::String);
    let with_null = |id: &str| format!("test_concat_with_null[{id}]");
    v.push(scalar(&with_null("null-a"), move || Ok(none() + lit("a"))));
    v.push(scalar(&with_null("null-null"), move || Ok(none() + none())));
    v.push(scalar(&with_null("a-null"), move || Ok(lit("a") + none())));
    // Python's bare `None` is an untyped NULL; `concat` takes one, `+` wants a string
    type Parts = fn() -> Vec<Expr>;
    let concat: [(&str, Parts, Parts); 4] = [
        ("null-null", || vec![null(T::String), null(T::Null)], || vec![null(T::String), null(T::String)]),
        ("abc-null", || vec![lit("abc"), null(T::Null)], || vec![lit("abc"), null(T::String)]),
        ("abc-typed-null", || vec![lit("abc"), null(T::String)], || vec![lit("abc"), null(T::String)]),
        (
            "abc-def-null",
            || vec![lit("abc"), lit("def"), null(T::Null)],
            || vec![lit("abc"), lit("def"), null(T::String)],
        ),
    ];
    for (id, method, operator) in concat {
        v.push(scalar(&format!("test_concat[concat-{id}]"), move || {
            let mut parts = method().into_iter();
            Ok(parts.next().expect("a first string").concat(parts))
        }));
        v.push(scalar(&format!("test_concat[add-{id}]"), move || {
            Ok(operator().into_iter().reduce(|a, b| a + b).expect("a first string"))
        }));
    }

    // test_string_methods_accents_and_emoji and ..._no_accents_and_no_emoji:
    // seven and five strings as a table (memtables.json), read back by index_col
    type Method = fn(Expr) -> Expr;
    let methods: [(&str, Method); 14] = [
        ("contains", |s| s.contains("c,")),
        ("contains_multi", |s| s.contains("123")),
        ("find", |s| s.find("123", None)),
        ("rpad", |s| s.rpad(4, "-")),
        ("rpad_gt", |s| s.rpad(8, "-")),
        ("lpad_lt", |s| s.lpad(4, "-")),
        ("lpad_gt", |s| s.lpad(8, "-")),
        ("len", |s| s.length()),
        ("find_in_set", |s| s.find_in_set(["aBc", "123"])),
        ("lstrip", |s| s.lstrip()),
        ("rstrip", |s| s.rstrip()),
        ("strip", |s| s.strip()),
        ("upper", |s| s.upper()),
        ("lower", |s| s.lower()),
    ];
    let on = |table: &'static str, f: Method| {
        move |fx: &super::Fx| fx.mem(table)?.select([c("index_col"), f(c("string_col")).name("tmp")])
    };
    for (id, f) in methods {
        v.push(tables(&format!("test_string_methods_accents_and_emoji[{id}]"), on("strings_accents", f)));
    }
    for (id, f) in
        [("rpad_lt", methods[3].1), ("rpad_gt", methods[4].1), ("lpad_lt", methods[5].1), ("lpad_gt", methods[6].1)]
    {
        v.push(tables(&format!("test_string_methods_no_accents_and_no_emoji[{id}]"), on("strings_plain", f)));
    }
}
