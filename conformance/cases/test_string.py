# SPDX-License-Identifier: Apache-2.0
# Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
# Modified for ibis-rs; see NOTICE at the repository root.

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

# --- the rest of test_string.py ----------------------------------------------
from functools import reduce  # noqa: E402
from operator import add  # noqa: E402

import ibis  # noqa: E402
from ibis import _  # noqa: E402

from cases import scalar, table, tables  # noqa: E402

L = ibis.literal

register(
    "test_string_literal",
    {
        "string": scalar(lambda: L("STRING")),
        "string-quote1": scalar(lambda: L("STRI'NG")),
        "string-quote2": scalar(lambda: L('STRI"NG')),
    },
)
r.add("test_string_col_is_unicode", column(lambda t: t.string_col))
register(
    "test_string",
    {
        "translate": column(lambda t: t.string_col.translate("01", "ab")),
        "find_in_set": column(lambda t: t.string_col.find_in_set(["1"])),
        "find_in_set_all_missing": column(lambda t: t.string_col.find_in_set(["a"])),
        "split": column(lambda t: t.date_string_col.split("/")),
        "join": column(lambda t: L("-").join(["a", t.string_col, "c"])),
    },
)
register(
    "test_substring",
    {
        "slice": column(lambda t: t.date_string_col[1:3]),
        "positive-index": column(lambda t: t.date_string_col[2]),
        "negative-index": column(lambda t: t.date_string_col[-2]),
        "expr_slice_begin": column(lambda t: t.date_string_col[t.date_string_col.length() - 1 :]),
        "expr_slice_end": column(lambda t: t.date_string_col[: t.date_string_col.length()]),
        "expr_empty_slice": column(lambda t: t.date_string_col[:]),
        "expr_slice_begin_end": column(
            lambda t: t.date_string_col[t.date_string_col.length() - 2 : t.date_string_col.length() - 1]
        ),
    },
)
r.add("test_re_replace_global", scalar(lambda: L("aba").re_replace("a", "c")))
r.add(
    "test_substr_with_null_values",
    table(
        lambda t: t.select(
            "id", substr_col_null=ibis.cases((t.bool_col, t.string_col), else_=None).substr(0, 2)
        )
    ),
)
register(
    "test_capitalize",
    {
        pid: scalar(lambda inp=inp: L(inp, type="string").capitalize())
        for pid, inp in {
            "none": None,
            "empty": "",
            "no_change": "Abc",
            "lower_to_upper": "abc",
            "mixed_to_upper": "aBC",
            "leading_space": " abc",
            "leading_digit": "9abc",
            "mixed_with_space": "aBc dEf",
            "mixed_with_hyphen": "aBc-dEf",
            "mixed_with_digit": "aBc1dEf",
        }.items()
    },
)
r.add("test_array_string_join#1", scalar(lambda: L(",").join(ibis.array(["a", "b", "c"]))))
r.add("test_array_string_join#2", scalar(lambda: ibis.array(["a", "b", "c"]).join(",")))
# test_empty_array_string_join is a procedure (crates/ibis-conformance/src/strings.rs):
# the test wants NULL for an empty array, and DuckDB 2 answers ''.
r.add(
    "test_subs_with_re_replace",
    scalar(lambda: L("hi").re_replace("i", "a").substitute({"d": "b"}, else_="k")),
)
r.add("test_multiple_subs", scalar(lambda: L("foo").substitute({"foo": "FOO", "bar": "BAR"})))
register(
    "test_levenshtein",
    {
        "python": scalar(lambda: L("kitten").levenshtein("sitting")),
        "ibis": scalar(lambda: L("kitten").levenshtein(L("sitting"))),
    },
)
register(
    "test_no_conditional_percent_escape",
    {
        "case": scalar(lambda: ibis.cases((True, "%"))),
        "ifelse": scalar(lambda: ibis.ifelse(True, "%", ibis.null())),
    },
)
r.add("test_non_match_regex_search_is_false", scalar(lambda: L("foo").re_search("bar")))
r.add("test_re_split", scalar(lambda: L(",a,,,,c").re_split(",+")))
# the Ibis test reads five rows; every row splits the same way
r.add("test_re_split_column", column(lambda t: t.string_col.re_split(r"\d+")))
r.add(
    "test_re_split_column_multiple_patterns",
    table(
        lambda t: t.filter(t.string_col.isin(("1", "2"))).select(
            "id", splits=lambda t: t.string_col.re_split(ibis.ifelse(t.string_col == "1", "0|1", r"\d+"))
        )
    ),
)
NULL_STR = L(None, type="string")
register(
    "test_concat_with_null",
    {
        "null-a": scalar(lambda: NULL_STR + "a"),
        "null-null": scalar(lambda: NULL_STR + NULL_STR),
        "a-null": scalar(lambda: "a" + NULL_STR),
    },
)
CONCAT_ARGS = {
    "null-null": lambda: (L(None, str), None),
    "abc-null": lambda: (L("abc"), None),
    "abc-typed-null": lambda: (L("abc"), L(None, str)),
    "abc-def-null": lambda: (L("abc"), "def", None),
}
for aid, args in CONCAT_ARGS.items():
    r.add(f"test_concat[concat-{aid}]", scalar(lambda args=args: args()[0].concat(*args()[1:])))
    r.add(f"test_concat[add-{aid}]", scalar(lambda args=args: reduce(add, args())))

# test_string_methods_accents_and_emoji and ..._no_accents_and_no_emoji: the
# Ibis tests create these seven and five strings as a table (memtables.json)
# and read the column back in the order of index_col.
ACCENTS = {
    "contains": lambda t: t.string_col.contains("c,"),
    "contains_multi": lambda t: t.string_col.contains("123"),
    "find": lambda t: t.string_col.find("123"),
    "rpad": lambda t: t.string_col.rpad(4, "-"),
    "rpad_gt": lambda t: t.string_col.rpad(8, "-"),
    "lpad_lt": lambda t: t.string_col.lpad(4, "-"),
    "lpad_gt": lambda t: t.string_col.lpad(8, "-"),
    "len": lambda t: t.string_col.length(),
    "find_in_set": lambda t: t.string_col.find_in_set(["aBc", "123"]),
    "lstrip": lambda t: t.string_col.lstrip(),
    "rstrip": lambda t: t.string_col.rstrip(),
    "strip": lambda t: t.string_col.strip(),
    "upper": lambda t: t.string_col.upper(),
    "lower": lambda t: t.string_col.lower(),
}
for pid, f in ACCENTS.items():
    r.add(
        f"test_string_methods_accents_and_emoji[{pid}]",
        tables(lambda c, f=f: c.mem("strings_accents").select("index_col", tmp=f)),
    )
for pid in ["rpad_lt", "rpad_gt", "lpad_lt", "lpad_gt"]:
    f = ACCENTS[pid.replace("rpad_lt", "rpad")]
    r.add(
        f"test_string_methods_no_accents_and_no_emoji[{pid}]",
        tables(lambda c, f=f: c.mem("strings_plain").select("index_col", tmp=f)),
    )
