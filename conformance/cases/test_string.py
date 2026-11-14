from __future__ import annotations

from cases import Registry, column

r = Registry("test_string")
register = r.params

register(
    "test_string",
    {
        "contains": column(lambda t: t.string_col.contains("6")),
        "like": column(lambda t: t.string_col.like("6%")),
        "complex_like_escape": column(lambda t: t.string_col.like("6^%")),
        "complex_like_escape_match": column(lambda t: t.string_col.like("6^%%")),
        "ilike": column(lambda t: t.string_col.ilike("6%")),
        "rlike": column(lambda t: t.string_col.rlike("|".join(map(str, range(10))))),
        "re_search_substring": column(lambda t: ("a" + t.string_col + "a").re_search(r"\d+")),
        "re_search": column(lambda t: t.string_col.re_search(r"\d+")),
        "re_search_posix": column(lambda t: t.string_col.re_search(r"[[:digit:]]+")),
        "re_extract": column(lambda t: ("xyz" + t.string_col + "abcd").re_extract(r"(\d+)", 0)),
        "re_extract_group": column(lambda t: ("xyz" + t.string_col + "abcd").re_extract(r"(\d+)abc", 1)),
        "re_extract_posix": column(lambda t: t.string_col.re_extract(r"([[:digit:]]+)", 1)),
        "re_extract_whole_group": column(lambda t: (t.string_col + "1").re_extract(r"\d(\d+)", 0)),
        "re_extract_group_1": column(lambda t: t.date_string_col.re_extract(r"(\d+)\D(\d+)\D(\d+)", 1)),
        "re_extract_group_2": column(lambda t: t.date_string_col.re_extract(r"(\d+)\D(\d+)\D(\d+)", 2)),
        "re_extract_group_3": column(lambda t: t.date_string_col.re_extract(r"(\d+)\D(\d+)\D(\d+)", 3)),
        "re_extract_group_at_beginning": column(lambda t: t.date_string_col.re_extract(r"^(\d+)", 1)),
        "re_extract_group_at_end": column(lambda t: t.date_string_col.re_extract(r"(\d+)$", 1)),
        "re_replace_posix": column(lambda t: t.string_col.re_replace(r"[[:digit:]]+", "a")),
        "re_replace": column(lambda t: t.string_col.re_replace(r"\d+", "a")),
        "repeat_method": column(lambda t: t.string_col.repeat(2)),
        "repeat_left": column(lambda t: 2 * t.string_col),
        "repeat_right": column(lambda t: t.string_col * 2),
        "find": column(lambda t: t.string_col.find("a")),
        "find_start": column(lambda t: t.date_string_col.find("13", 3)),
        "lpad": column(lambda t: t.string_col.lpad(10, "a")),
        "rpad": column(lambda t: t.string_col.rpad(10, "a")),
        "lower": column(lambda t: t.string_col.lower()),
        "upper": column(lambda t: t.string_col.upper()),
        "reverse": column(lambda t: t.string_col.reverse()),
        "ascii_str": column(lambda t: t.string_col.ascii_str()),
        "length": column(lambda t: t.string_col.length()),
        "startswith": column(
            lambda t: t.int_col.cases((1, "abcd"), (2, "ABCD"), else_="dabc").startswith("abc")
        ),
        "endswith": column(
            lambda t: t.int_col.cases((1, "abcd"), (2, "ABCD"), else_="dabc").endswith("bcd")
        ),
        "startswith-simple": column(lambda t: t.date_string_col.startswith("2010-01")),
        "endswith-simple": column(lambda t: t.date_string_col.endswith("/10")),
        "strip": column(lambda t: t.string_col.strip()),
        "lstrip": column(lambda t: t.string_col.lstrip()),
        "rstrip": column(lambda t: t.string_col.rstrip()),
        "concat_columns": column(lambda t: t.string_col + t.date_string_col),
        "concat_column_scalar": column(lambda t: t.string_col + "a"),
        "concat_scalar_column": column(lambda t: "a" + t.string_col),
        "replace": column(lambda t: t.string_col.replace("1", "42")),
    },
)

# test_substring applies each function to `alltypes.date_string_col`.
register(
    "test_substring",
    {
        "substr": column(lambda t: t.date_string_col.substr(2, 3)),
        "substr-start-only": column(lambda t: t.date_string_col.substr(2)),
        "left": column(lambda t: t.date_string_col.left(2)),
        "right": column(lambda t: t.date_string_col.right(2)),
    },
)
