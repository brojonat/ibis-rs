//! ibis/backends/tests/test_string.py

use ibis_core::prelude::*;

use super::{Case, c, column};

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
}
