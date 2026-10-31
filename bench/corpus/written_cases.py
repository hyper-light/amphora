#!/usr/bin/env python3
"""Written cases (G2 step 2): at least ten statements written for each inventory entry, each group of
them run on a fresh SQLite database and written as its own sqllogictest file with SQLite's answers.
written/index.json names the entry each file was written for and the line its cases start on; the
corpus counts a case for that entry and for whatever its text shows (tagger.py), so the entries the
text alone can't show (the datatype page's rules) are counted where they were written.

The cases vary what the entry acts on: a function over the storage classes and edge values, an
operator over pairs of operands, a PRAGMA in its read, schema-qualified, table-valued and set forms,
a rule over the declared types and values it decides, a date modifier over base times, a production
over its branches. The step's named features get families of their own: window frames (every unit,
bound and exclusion), recursive CTEs, JSON over a table, UPSERT and RETURNING, generated columns,
STRICT tables, collations and triggers. An entry the rest reach fewer than ten times gets its own (EXTRA).

A query's answer is typed I, R or T by its values, and sorted (rowsort) unless the statement orders
its rows itself; a statement that returns rows but whose rows are SQLite's own business (EXPLAIN) is
kept as a statement.

usage: written_cases.py <features.json> <out dir>
  writes <out dir>/written/<kind>/<entry>.test, written/index.json and written-cases.json (cases per entry)
"""
import hashlib
import json
import os
import re
import sqlite3
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import slt  # noqa: E402
import tagger as tagmod  # noqa: E402

FEATURES, OUT = sys.argv[1], sys.argv[2]
NEED = 10

BASE = [
    "CREATE TABLE t1(a INTEGER, b TEXT, c REAL, d BLOB, e)",
    "INSERT INTO t1 VALUES (1, 'one', 1.5, x'01', 1), (2, 'two', 2.5, x'02', 'two'), (3, 'three', NULL, NULL, 3.5),"
    " (4, NULL, 4.5, x'0405', NULL), (5, 'Five', -5.5, x'', 'x'), (6, '', 0.0, x'ff', 6), (NULL, 'null-a', 7.25, x'07', x'07')",
    "CREATE TABLE t2(a INTEGER PRIMARY KEY, b TEXT COLLATE NOCASE, c INTEGER)",
    "INSERT INTO t2 VALUES (1, 'x', 10), (2, 'Y', 20), (3, 'y', 20), (6, 'z', NULL), (7, 'X', 70)",
]
VALUES = ["NULL", "0", "1", "-7", "2.5", "-0.25", "'abc'", "'ABC'", "''", "'12'", "'3.5x'", "x'00ff'", "9223372036854775807", "-9223372036854775808", "1e308"]
NUMS = ["0", "1", "-1", "2.5", "-2.5", "0.5", "10", "100", "NULL", "'4'", "1e3", "0.001"]
TEXTS = ["'hello'", "'Hello World'", "''", "NULL", "'  pad  '", "'héllo'", "'a,b,c'", "'100'", "'ABC'", "'x''y'", "'tab\tchar'", "'日本'"]
J = ["'{\"a\":1,\"b\":[1,2,{\"c\":3}],\"d\":null}'", "'[1,2,3]'", "'{\"x\":{\"y\":{\"z\":true}}}'", "'[]'", "'{}'", "'\"str\"'", "'42'", "'null'",
     "'[{\"k\":1},{\"k\":2}]'", "'{\"a\":\"text\",\"n\":-1.5}'", "'{\"a\":[[]]}'", "NULL"]
PATHS = ["'$'", "'$.a'", "'$.b'", "'$.b[0]'", "'$.b[#-1]'", "'$.x.y'", "'$.zz'", "'$[1]'", "'$.b[2].c'", "'$.a[0]'"]
TIMES = ["'2024-01-31'", "'2024-02-29 12:30:00'", "'2023-12-31 23:59:59'", "'2000-01-01'", "'1970-01-01 00:00:00'", "'2024-03-15 08:05:00.123'",
         "'2038-01-19 03:14:07'", "'1999-12-31T23:00:00'", "'2024-06-30'", "2460385.5", "1710460800", "'now'"]


def fn_cases(name, kind):
    """Calls of a function over values that cover its arguments' kinds."""
    one = {"abs": NUMS, "hex": VALUES, "length": VALUES, "lower": TEXTS, "upper": TEXTS, "ltrim": TEXTS, "rtrim": TEXTS, "trim": TEXTS,
           "quote": VALUES, "typeof": VALUES, "unicode": TEXTS, "octet_length": VALUES, "sign": NUMS, "soundex": TEXTS, "unhex": ["'4142'", "'00ff'", "''", "'zz'", "NULL", "'414'", "'4a4B'", "'0001'", "'ffff'", "'616263'"],
           "unistr": ["'\\0041'", "'\\u00e9'", "'a\\\\b'", "''", "NULL", "'\\U0001F600'", "'x\\0042y'", "'\\+00263A'", "'plain'", "'\\00e9\\00e9'"],
           "unistr_quote": TEXTS, "zeroblob": ["0", "1", "4", "16", "-1", "NULL", "'3'", "2.9", "100", "8"], "randomblob": ["0", "1", "4", "16", "-1", "NULL", "'3'", "2.9", "100", "8"],
           "random": [""] * 1, "likely": VALUES[:10], "unlikely": VALUES[:10], "sqlite_version": [""], "sqlite_source_id": [""], "changes": [""], "total_changes": [""],
           "last_insert_rowid": [""], "sqlite_compileoption_get": ["0", "1", "2", "-1", "NULL", "100", "'1'", "3", "4", "5"],
           "sqlite_compileoption_used": ["'THREADSAFE'", "'SQLITE_THREADSAFE'", "'ENABLE_FTS5'", "'NOPE'", "NULL", "''", "'DEFAULT_MEMSTATUS'", "'OMIT_LOAD_EXTENSION'", "'ENABLE_JSON1'", "'ENABLE_RTREE'"],
           "load_extension": ["'/nonexistent'"], "sqlite_offset": ["a"]}
    two = {"instr": [("'hello'", "'l'"), ("'hello'", "'z'"), ("'hello'", "''"), ("NULL", "'a'"), ("'abcabc'", "'bc'"), ("x'0102'", "x'02'"), ("123", "2"), ("'héllo'", "'l'"), ("''", "'a'"), ("'aaa'", "'aa'")],
           "ifnull": [(a, b) for a, b in zip(VALUES, reversed(VALUES))][:10], "nullif": [(a, a) for a in VALUES[:5]] + [(a, b) for a, b in zip(VALUES[5:], VALUES[6:])][:5],
           "glob": [("'a*'", "'abc'"), ("'a?c'", "'abc'"), ("'[a-c]x'", "'bx'"), ("'*'", "''"), ("'A*'", "'abc'"), ("NULL", "'a'"), ("'[^a]*'", "'bc'"), ("'a\\*'", "'a*'"), ("'*.txt'", "'f.txt'"), ("'?'", "'ab'")],
           "like": [("'a%'", "'abc'"), ("'A%'", "'abc'"), ("'_b_'", "'abc'"), ("'%'", "''"), ("'a\\%'", "'a%'"), ("NULL", "'a'"), ("'%c'", "'abc'"), ("'ab'", "'AB'"), ("'%b%'", "'xbx'"), ("'a_'", "'a'")],
           "round": [(n, d) for n, d in zip(NUMS, ["0", "1", "2", "-1", "3", "NULL", "1", "0", "2", "1"])], "substr": [(t, s) for t, s in zip(TEXTS, ["1", "2", "-2", "0", "3", "1", "-1", "2", "1", "2"])],
           "substring": [(t, s) for t, s in zip(TEXTS, ["2", "1", "-3", "0", "4", "1", "-1", "2", "1", "2"])], "max": [(a, b) for a, b in zip(VALUES, VALUES[1:])][:10], "min": [(a, b) for a, b in zip(VALUES, VALUES[2:])][:10],
           "likelihood": [(v, p) for v, p in zip(VALUES, ["0.5", "0.0", "1.0", "0.25", "0.75", "0.5", "0.1", "0.9", "0.5", "0.3"])],
           "coalesce": [(a, b) for a, b in zip(VALUES, VALUES[3:])][:10], "replace": [("'banana'", "'an'"), ("'aaa'", "'a'"), ("'x'", "''"), ("NULL", "'a'"), ("'abc'", "'abc'"), ("'abab'", "'ba'"), ("'héllo'", "'l'"), ("'123'", "2"), ("''", "'a'"), ("'A'", "'a'")]}
    if name in ("replace",):
        return [f"SELECT replace({a}, {b}, 'X')" for a, b in two[name]]
    if name in ("substr", "substring"):
        return [f"SELECT {name}({a}, {b})" for a, b in two[name]] + [f"SELECT {name}({a}, {b}, 2)" for a, b in two[name][:4]]
    if name in two:
        return [f"SELECT {name}({a}, {b})" for a, b in two[name]]
    if name in one:
        vals = one[name]
        if name == "random":
            return ["SELECT typeof(random())", "SELECT random() IS NOT NULL", "SELECT random() BETWEEN -9223372036854775808 AND 9223372036854775807", "SELECT abs(random() % 10) < 10",
                    "SELECT count(*) FROM t1 WHERE random() IS NOT NULL", "SELECT typeof(random() / 2)", "SELECT random() = random() OR 1", "SELECT length(CAST(random() AS TEXT)) > 0",
                    "SELECT (SELECT random()) IS NOT NULL", "SELECT sum(random() IS NOT NULL) FROM t1"]
        if vals == [""]:
            return [f"SELECT typeof({name}())", f"SELECT {name}() IS NOT NULL", f"SELECT length({name}()) > 0" if name.startswith("sqlite") else f"SELECT {name}() >= 0",
                    f"SELECT {name}() = {name}()", f"SELECT coalesce({name}(), 0) IS NOT NULL", f"SELECT CAST({name}() AS TEXT) IS NOT NULL",
                    f"SELECT ({name}()) IS ({name}())", f"SELECT count(*) FROM t1 WHERE {name}() IS NOT NULL", f"SELECT max(1, length(CAST({name}() AS TEXT)))", f"SELECT typeof({name}()) IN ('text', 'integer')"]
        if name in ("randomblob",):
            return [f"SELECT length(randomblob({v}))" for v in vals]
        if name == "load_extension":
            return [f"SELECT load_extension('/nonexistent/x{i}')" for i in range(10)]
        if name == "sqlite_offset":
            return [f"SELECT typeof(sqlite_offset({c})) FROM t1 LIMIT 1" for c in "abcde"] + [f"SELECT sqlite_offset({c}) > 0 FROM t1 WHERE a = 2" for c in "abcde"] + ["SELECT sqlite_offset(1)"]
        return [f"SELECT {name}({v})" for v in vals[:12]]
    many = {"char": ["72, 105", "65", "", "0x263A", "NULL", "97, 98, 99", "233", "128512", "-1", "1114111"], "concat": ["'a', 'b'", "'a', NULL, 'b'", "1, 2.5", "NULL", "'', ''", "x'41', 'b'", "'x'", "'a', 'b', 'c', 'd'", "1e3, -1", "'é', 'x'"],
            "concat_ws": ["'-', 'a', 'b'", "',', 'a', NULL, 'b'", "NULL, 'a'", "'', 'x', 'y'", "'ab', 1, 2", "'-', NULL", "' ', 'a'", "'|', 1.5, 'x', NULL, 3", "'--', '', ''", "'x', 'é'"],
            "format": ["'%d-%s', 7, 'x'", "'%5.2f', 3.14159", "'%-5s|', 'ab'", "'%x', 255", "'%%'", "'%q', 'it''s'", "'%c', 65", "'%e', 12345.678", "'%03d', 7", "'%s %s', NULL, 1"],
            "printf": ["'%d-%s', 7, 'x'", "'%5.2f', 3.14159", "'%-5s|', 'ab'", "'%x', 255", "'%%'", "'%q', 'it''s'", "'%c', 65", "'%e', 12345.678", "'%03d', 7", "'%s %s', NULL, 1"],
            "iif": ["1, 'a', 'b'", "0, 'a', 'b'", "NULL, 'a', 'b'", "'x', 1, 2", "2 > 1, 'y', 'n'", "1 = 2, NULL, 'z'", "0.5, 1, 0", "'0', 'a', 'b'", "1, 2", "0, 1, 0, 2, 3"],
            "if": ["1, 'a', 'b'", "0, 'a', 'b'", "NULL, 'a', 'b'", "'x', 1, 2", "2 > 1, 'y', 'n'", "1 = 2, NULL, 'z'", "0.5, 1, 0", "'0', 'a', 'b'", "1, 2", "0, 1, 0, 2, 3"]}
    if name in many:
        return [f"SELECT {name}({a})" for a in many[name]]
    if kind == "math":
        args = {"atan2": [f"{a}, {b}" for a, b in zip(NUMS, reversed(NUMS))], "log": [f"{a}" for a in NUMS] + ["2, 8", "10, 100"], "mod": [f"{a}, {b}" for a, b in zip(NUMS, ["3", "2", "-2", "1.5", "0", "7", "3", "NULL", "4", "2"])],
                "pow": [f"{a}, {b}" for a, b in zip(NUMS, ["2", "0", "-1", "0.5", "3", "2", "1", "2", "NULL", "2"])], "power": [f"{a}, {b}" for a, b in zip(NUMS, ["3", "1", "-2", "2", "0.5", "2", "1", "2", "NULL", "3"])],
                "pi": [""] * 10}.get(name, NUMS)
        if name == "pi":
            return ["SELECT round(pi(), 6)", "SELECT pi() > 3", "SELECT typeof(pi())", "SELECT round(pi() * 2, 4)", "SELECT pi() = pi()", "SELECT round(sin(pi() / 2), 6)",
                    "SELECT round(cos(pi()), 6)", "SELECT CAST(pi() AS INTEGER)", "SELECT round(pi() / 4, 5)", "SELECT pi() - 3 < 0.2"]
        return [f"SELECT round({name}({a}), 6)" for a in args[:12]]
    if kind == "datetime":
        base = {"date": "date({t})", "time": "time({t})", "datetime": "datetime({t})", "julianday": "julianday({t})", "unixepoch": "unixepoch({t})",
                "strftime": "strftime('%Y-%m-%d %H:%M:%S %j %w %f %s', {t})", "timediff": "timediff({t}, '2024-01-01')"}[name]
        return [f"SELECT {base.format(t=t)}" for t in TIMES if t != "'now'"] + [f"SELECT typeof({base.format(t=chr(39) + 'now' + chr(39))})"]
    if kind == "json":
        if name in ("json_each", "json_tree", "jsonb_each", "jsonb_tree"):
            src = (lambda d: f"jsonb({d})") if name.startswith("jsonb") else (lambda d: d)
            return [f"SELECT key, value, type, fullkey FROM {name}({src(d)}) ORDER BY id" for d in J[:11] if d != "NULL"]
        if name in ("json_group_array", "jsonb_group_array", "json_group_object", "jsonb_group_object"):
            col = ["a", "b", "c", "e", "a * 2", "upper(b)", "a IS NULL", "length(b)", "c > 2", "coalesce(b, '')"]
            wrap = (lambda x: f"json({x})") if name.startswith("jsonb") else (lambda x: x)
            if "object" in name:
                return [wrap(f"{name}(coalesce(b, 'k' || rowid), {c})").join(["SELECT ", " FROM (SELECT * FROM t1 ORDER BY rowid)"]) for c in col]
            return [wrap(f"{name}({c})").join(["SELECT ", " FROM (SELECT * FROM t1 ORDER BY rowid)"]) for c in col]
        one_arg = {"json", "jsonb", "json_quote", "json_valid", "json_error_position", "json_pretty", "json_array_length", "json_type"}
        wrap = (lambda x: f"json({x})") if name.startswith("jsonb") else (lambda x: x)
        if name in ("json_array", "jsonb_array", "json_object", "jsonb_object"):
            args = ["1, 2", "'a', NULL", "", "1.5, 'x', json('[1]')", "NULL", "'k', 1", "'a', 'b', 'c', 'd'", "x'01'" if "array" in name else "'n', NULL", "json('{}'), 3" if "array" in name else "'o', json('{}')", "-1, 0"]
            if "object" in name:
                args = ["'k', 1", "'a', NULL", "", "'x', 1.5, 'y', 'z'", "'j', json('[1]')", "'k', 'v'", "'a', 1, 'b', 2", "'n', -1", "'o', json('{}')", "'e', ''"]
            return [f"SELECT {wrap(name + '(' + a + ')')}" for a in args]
        if name in one_arg:
            if name == "json_quote":
                return [f"SELECT json_quote({v})" for v in VALUES[:12] if not v.startswith("x'")]
            return [f"SELECT {wrap(name + '(' + d + ')')}" for d in J] + ([f"SELECT json_valid({d}, 2)" for d in ["'{a:1}'", "'[1,]'"]] if name == "json_valid" else [])
        if name in ("json_extract", "jsonb_extract", "json_remove", "jsonb_remove"):
            return [f"SELECT {wrap(name + '(' + J[0] + ', ' + p + ')')}" for p in PATHS] + [f"SELECT {wrap(name + '(' + J[1] + ', ' + chr(39) + '$[0]' + chr(39) + ')')}"]
        if name in ("json_patch", "jsonb_patch"):
            pairs = [("'{\"a\":1}'", "'{\"b\":2}'"), ("'{\"a\":1}'", "'{\"a\":null}'"), ("'{\"a\":{\"x\":1}}'", "'{\"a\":{\"y\":2}}'"), ("'[1]'", "'{\"a\":1}'"), ("'{}'", "'{}'"),
                     ("'{\"a\":[1]}'", "'{\"a\":[2]}'"), ("'1'", "'2'"), ("'{\"a\":1}'", "'3'"), ("'{\"a\":1,\"b\":2}'", "'{\"b\":null,\"c\":3}'"), ("'null'", "'{\"a\":1}'")]
            return [f"SELECT {wrap(name + '(' + a + ', ' + b + ')')}" for a, b in pairs]
        # setters: (json, path, value) pairs
        pv = list(zip(PATHS, ["9", "'x'", "NULL", "json('[1]')", "1.5", "json('{\"q\":1}')", "true", "-1", "'s'", "0"]))
        return [f"SELECT {wrap(name + '(' + J[0] + ', ' + p + ', ' + v + ')')}" for p, v in pv] + [f"SELECT {wrap(name + '(' + chr(39) + '[1,3]' + chr(39) + ', ' + chr(39) + '$[1]' + chr(39) + ', 2)')}"]
    if kind == "aggregate":
        cols = ["a", "b", "c", "e", "DISTINCT a", "a * 2", "length(b)", "c + a", "CASE WHEN a > 2 THEN a END", "coalesce(c, 0)"]
        extra = {"group_concat": ", '|'", "string_agg": ", ','", "percentile": ", 50", "percentile_cont": ", 0.25", "percentile_disc": ", 0.75"}.get(name, "")
        if name == "string_agg":
            return [f"SELECT string_agg(CAST({c.replace('DISTINCT ', '')} AS TEXT), ',') FROM t1" for c in cols]
        if name in ("percentile", "percentile_cont", "percentile_disc", "median"):
            return [f"SELECT {name}({c.replace('DISTINCT ', '')}{extra}) FROM t1" for c in cols if "b" not in c or "length" in c] + [f"SELECT {name}(c{extra}) FROM t1 WHERE a > 2", f"SELECT {name}(a{extra}) FROM t1 GROUP BY a % 2 ORDER BY 1"]
        return [f"SELECT {name}({c}{extra if 'DISTINCT' not in c else ''}) FROM t1" for c in cols] + [f"SELECT a % 2, {name}(a{extra}) FROM t1 GROUP BY 1 ORDER BY 1"]
    if kind == "window":
        call = {"ntile": "ntile(2)", "lag": "lag(b)", "lead": "lead(b, 2, 'none')", "nth_value": "nth_value(b, 2)", "first_value": "first_value(b)", "last_value": "last_value(b)"}.get(name, f"{name}()")
        overs = ["ORDER BY a", "ORDER BY a DESC", "PARTITION BY a % 2 ORDER BY a", "ORDER BY b", "ORDER BY c NULLS FIRST", "ORDER BY a ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING",
                 "ORDER BY a RANGE BETWEEN 1 PRECEDING AND 1 FOLLOWING", "ORDER BY a GROUPS BETWEEN CURRENT ROW AND 1 FOLLOWING", "PARTITION BY b IS NULL ORDER BY a", "ORDER BY e"]
        return [f"SELECT a, {call} OVER ({o}) FROM t1 ORDER BY a" for o in overs]
    return []


OPERANDS = [("1", "2"), ("2", "1"), ("'a'", "'b'"), ("NULL", "1"), ("2.5", "2"), ("'10'", "9"), ("x'01'", "x'02'"), ("-3", "4"), ("'abc'", "'ABC'"), ("7", "7"), ("0", "'0'"), ("9223372036854775807", "1")]


def op_cases(op):
    binary = {"||", "->", "->>", "*", "/", "%", "+", "-", "&", "|", "<<", ">>", "<", ">", "<=", ">=", "=", "==", "<>", "!=", "IS", "IS NOT", "IS DISTINCT FROM", "IS NOT DISTINCT FROM", "AND", "OR"}
    if op in binary:
        if op in ("->", "->>"):
            return [f"SELECT {d} {op} {p}" for d in J[:4] for p in PATHS[:3]][:12]
        return [f"SELECT {a} {op} {b}" for a, b in OPERANDS]
    unary = {"~": "~", "+ (unary)": "+", "- (unary)": "-", "NOT": "NOT "}
    if op in unary:
        return [f"SELECT {unary[op]}{v}" for v, _ in OPERANDS]
    forms = {
        "COLLATE": [f"SELECT {a} = {b} COLLATE {c}" for (a, b) in [("'a'", "'A'"), ("'x '", "'x'"), ("'b'", "'B'"), ("'é'", "'É'")] for c in ("NOCASE", "BINARY", "RTRIM")][:12],
        "ESCAPE": [f"SELECT {a} LIKE {b} ESCAPE {e}" for a, b, e in [("'a%'", "'a!%'", "'!'"), ("'a_'", "'a!_'", "'!'"), ("'ab'", "'a!_'", "'!'"), ("'%'", "'#%'", "'#'"), ("'a%b'", "'a\\%b'", "'\\'"),
                                                                       ("'x'", "'x'", "'!'"), ("NULL", "'a'", "'!'"), ("'100%'", "'100!%'", "'!'"), ("'a!'", "'a!!'", "'!'"), ("'_'", "'!_'", "'!'")]],
        "BETWEEN": [f"SELECT {v} BETWEEN {lo} AND {hi}" for v, lo, hi in [("2", "1", "3"), ("5", "1", "3"), ("'b'", "'a'", "'c'"), ("NULL", "1", "2"), ("2.5", "2", "3"), ("1", "1", "1"), ("'10'", "9", "11"), ("0", "-1", "NULL"), ("x'02'", "x'01'", "x'03'"), ("-5", "-10", "0")]],
        "IN": [f"SELECT {v} IN ({l})" for v, l in [("2", "1, 2, 3"), ("5", "1, 2"), ("NULL", "1"), ("1", "NULL, 1"), ("2", "NULL, 1"), ("'a'", "'a', 'b'"), ("1", "SELECT a FROM t1"), ("9", "SELECT a FROM t1"), ("'1'", "1"), ("2.0", "2")]],
        "LIKE": [f"SELECT {a} LIKE {b}" for b, a in [("'a%'", "'abc'"), ("'A%'", "'abc'"), ("'_b_'", "'abc'"), ("'%'", "''"), ("'%c'", "'abc'"), ("'ab'", "'AB'"), ("'é%'", "'É'"), ("NULL", "'a'"), ("'%b%'", "'xbx'"), ("'a_'", "'a'")]],
        "GLOB": [f"SELECT {a} GLOB {b}" for b, a in [("'a*'", "'abc'"), ("'a?c'", "'abc'"), ("'[a-c]x'", "'bx'"), ("'*'", "''"), ("'A*'", "'abc'"), ("NULL", "'a'"), ("'[^a]*'", "'bc'"), ("'*.txt'", "'f.txt'"), ("'?'", "'ab'"), ("'a[*]'", "'a*'")]],
        "REGEXP": [f"SELECT {a} REGEXP {b}" for a, b in [("'abc'", "'b'"), ("'abc'", "'^b'"), ("'abc'", "'c$'"), ("'aaa'", "'a+'"), ("''", "'.'"), ("NULL", "'a'"), ("'x1'", "'[0-9]'"), ("'Abc'", "'abc'"), ("'a.c'", "'a\\.c'"), ("'abc'", "'a|z'")]],
        "MATCH": [f"SELECT 'abc' MATCH {p}" for p in ["'a'", "'b'", "'x'", "''", "NULL", "'abc'", "'a*'", "'c'", "'A'", "'ab'"]],
        "ISNULL": [f"SELECT {v} ISNULL" for v, _ in OPERANDS[:10]], "NOTNULL": [f"SELECT {v} NOTNULL" for v, _ in OPERANDS[:10]], "NOT NULL": [f"SELECT {v} NOT NULL" for v, _ in OPERANDS[:10]],
        "EXISTS": [f"SELECT EXISTS (SELECT 1 FROM t1 WHERE {w})" for w in ["a = 3", "a = 99", "b IS NULL", "c > 4", "1", "0", "e = 'x'", "a IN (1, 2)", "d IS NULL", "b LIKE 'f%'"]],
        "CASE": [f"SELECT CASE {c} END" for c in ["WHEN 1 THEN 'a' ELSE 'b'", "WHEN 0 THEN 'a' ELSE 'b'", "WHEN NULL THEN 'a'", "1 WHEN 1 THEN 'one' WHEN 2 THEN 'two'", "3 WHEN 1 THEN 'one' ELSE 'other'",
                                                    "NULL WHEN NULL THEN 'n' ELSE 'not'", "WHEN 2 > 1 THEN 1 WHEN 1 THEN 2", "'a' WHEN 'A' THEN 1 ELSE 0", "WHEN 'x' THEN 1 ELSE 0", "2.0 WHEN 2 THEN 'eq' ELSE 'ne'"]],
        "CAST": [f"SELECT CAST({v} AS {t})" for v, t in [("'12abc'", "INTEGER"), ("3.9", "INTEGER"), ("5", "TEXT"), ("'1e2'", "REAL"), ("x'3132'", "INTEGER"), ("NULL", "TEXT"), ("'abc'", "NUMERIC"), ("1.0", "NUMERIC"), ("-2.5", "INTEGER"), ("'  7 '", "INTEGER"), ("12", "BLOB")]],
    }
    if op in forms:
        return forms[op]
    if op.startswith("NOT "):
        base = forms.get(op[4:]) or []
        return [c.replace(f" {op[4:]} ", f" NOT {op[4:]} ", 1).replace("SELECT EXISTS", "SELECT NOT EXISTS") for c in base]
    return []


BOOL = ["1", "0", "ON", "OFF", "TRUE", "FALSE", "YES", "NO"]
PRAGMA_SET = {  # values each setting is set to, in turn; process-wide limits end at their default
    **{n: BOOL for n in ("automatic_index", "case_sensitive_like", "foreign_keys", "recursive_triggers", "reverse_unordered_selects", "cell_size_check", "defer_foreign_keys",
                         "ignore_check_constraints", "read_uncommitted", "trusted_schema", "legacy_alter_table", "checkpoint_fullfsync", "fullfsync", "count_changes", "empty_result_callbacks",
                         "full_column_names", "short_column_names", "query_only")},
    "application_id": ["0", "7", "-1", "2147483647", "-2147483648", "42"], "user_version": ["0", "3", "-5", "99", "2147483647", "1"],
    "cache_size": ["-2000", "100", "-1", "0", "5000", "-4096"], "busy_timeout": ["0", "100", "5000", "1", "250", "-1"],
    "secure_delete": ["0", "1", "FAST", "ON", "OFF", "TRUE"], "temp_store": ["0", "1", "2", "DEFAULT", "FILE", "MEMORY"], "synchronous": ["0", "1", "2", "3", "OFF", "NORMAL", "FULL", "EXTRA"],
    "journal_mode": ["DELETE", "TRUNCATE", "PERSIST", "MEMORY", "OFF", "delete"], "locking_mode": ["EXCLUSIVE", "NORMAL", "exclusive", "normal", "Exclusive", "Normal"],
    "cache_spill": ["0", "1", "100", "ON", "OFF", "2000"], "analysis_limit": ["0", "100", "1000", "-1", "50", "400"], "journal_size_limit": ["-1", "0", "1048576", "4096", "65536", "-1"],
    "mmap_size": ["0", "4096", "1048576", "268435456", "65536", "0"], "soft_heap_limit": ["1000000", "500000", "2000000000", "8000000", "100000000", "0"],
    # a positive hard limit can't be lifted again (the PRAGMA only lowers it) and would hold for every
    # file after it in the process: the cases give it the values it ignores, and read it
    "hard_heap_limit": ["0", "-1", "-2", "-100", "-5", "-6"], "threads": ["0", "1", "2", "4", "8", "0"],
    "wal_autocheckpoint": ["1000", "0", "100", "-1", "500", "1000"], "max_page_count": ["1073741823", "100000", "500000", "4294967294", "1", "1073741823"],
    "page_size": ["4096", "8192", "1024", "512", "65536", "4096"], "auto_vacuum": ["0", "1", "2", "NONE", "FULL", "INCREMENTAL"],
    "encoding": ["'UTF-8'", "'UTF-16'", "'UTF-16le'", "'UTF-16be'", "'utf-8'", "'UTF-8'"], "default_cache_size": ["2000", "100", "-100", "500", "0", "2000"],
    "writable_schema": ["ON", "OFF", "1", "0", "RESET", "NO"],
}
PRAGMA_TABLE_ARG = {"table_info", "table_xinfo", "index_list", "foreign_key_list", "table_list"}
PRAGMA_INDEX_ARG = {"index_info", "index_xinfo"}


def pragma_cases(name):
    out = [f"PRAGMA {name}", f"PRAGMA main.{name}", f"SELECT * FROM pragma_{name}", f"SELECT count(*) >= 0 FROM pragma_{name}"]
    if name in PRAGMA_TABLE_ARG:
        out = [f"PRAGMA {name}(t1)", f"PRAGMA {name}(t2)", f"PRAGMA main.{name}(t1)", f"SELECT * FROM pragma_{name}('t1')", f"SELECT * FROM pragma_{name}('t2')",
               f"SELECT count(*) FROM pragma_{name}('t1')", f"PRAGMA {name}('t1')", f"PRAGMA {name}(nope)", f"SELECT count(*) FROM pragma_{name}('nope')", f"PRAGMA {name}(\"t2\")"]
    elif name in PRAGMA_INDEX_ARG:
        out = [f"PRAGMA {name}(i1)", f"PRAGMA {name}(i2)", f"PRAGMA main.{name}(i1)", f"SELECT * FROM pragma_{name}('i1')", f"SELECT * FROM pragma_{name}('i2')",
               f"SELECT count(*) FROM pragma_{name}('i1')", f"PRAGMA {name}('i2')", f"PRAGMA {name}(nope)", f"SELECT count(*) FROM pragma_{name}('nope')", f"PRAGMA {name}(t2_ab)"]
    elif name in PRAGMA_SET:
        sets = ["PRAGMA {n} = {v}", "PRAGMA main.{n} = {v}", "PRAGMA {n}({v})", "PRAGMA main.{n}({v})"]
        reads = ["PRAGMA {n}", "PRAGMA main.{n}", "SELECT * FROM pragma_{n}"]
        for i, v in enumerate(PRAGMA_SET[name]):
            out += [sets[i % len(sets)].format(n=name, v=v), reads[i % len(reads)].format(n=name)]
    elif name == "schema_version":
        out += ["CREATE TABLE sv1(a)", "PRAGMA schema_version", "SELECT schema_version FROM pragma_schema_version", "DROP TABLE sv1", "PRAGMA temp.schema_version", "CREATE INDEX svi ON t1(a)",
                "SELECT schema_version > 0 FROM pragma_schema_version", "PRAGMA schema_version = 100", "PRAGMA main.schema_version"]
    elif name in ("integrity_check", "quick_check"):
        out += [f"PRAGMA {name}(1)", f"PRAGMA {name}(t1)", f"PRAGMA main.{name}(5)", f"SELECT * FROM pragma_{name}(1)", f"PRAGMA {name}(t2)", f"PRAGMA {name}(100)"]
    elif name == "foreign_key_check":
        out += ["PRAGMA foreign_key_check(t1)", "PRAGMA foreign_key_check(fk)", "SELECT count(*) FROM pragma_foreign_key_check('fk')", "PRAGMA main.foreign_key_check(fk)", "PRAGMA foreign_key_check(t2)", "PRAGMA foreign_key_check(pk)"]
    elif name in ("wal_checkpoint",):
        out += ["PRAGMA wal_checkpoint(PASSIVE)", "PRAGMA wal_checkpoint(FULL)", "PRAGMA wal_checkpoint(RESTART)", "PRAGMA wal_checkpoint(TRUNCATE)", "PRAGMA main.wal_checkpoint(PASSIVE)",
                "PRAGMA main.wal_checkpoint(TRUNCATE)", "PRAGMA wal_checkpoint(NOPE)", "PRAGMA temp.wal_checkpoint"]
    elif name == "optimize":
        out += ["PRAGMA optimize(0x02)", "PRAGMA optimize(0xfffe)", "PRAGMA main.optimize(1)", "PRAGMA optimize(-1)", "PRAGMA optimize(0x10002)", "PRAGMA optimize(0)"]
    elif name == "incremental_vacuum":
        out += ["PRAGMA incremental_vacuum(1)", "PRAGMA incremental_vacuum(0)", "PRAGMA main.incremental_vacuum(5)", "PRAGMA incremental_vacuum(-1)", "PRAGMA incremental_vacuum(100)", "PRAGMA incremental_vacuum(2)"]
    elif name == "database_list":  # its file column names this run's own temporary files: left out
        out = ["SELECT seq, name FROM pragma_database_list", "SELECT name FROM pragma_database_list ORDER BY seq", "ATTACH ':memory:' AS dl1", "SELECT seq, name FROM pragma_database_list ORDER BY seq",
               "SELECT count(*) FROM pragma_database_list", "CREATE TEMP TABLE dlt(a)", "SELECT name FROM pragma_database_list WHERE name <> 'main' ORDER BY seq", "DETACH dl1",
               "SELECT group_concat(name, ',' ORDER BY seq) FROM pragma_database_list", "SELECT file IS NOT NULL FROM pragma_database_list WHERE name = 'main'", "SELECT name FROM pragma_database_list WHERE seq > 0"]
    else:
        out += [f"SELECT * FROM main.pragma_{name}", f"PRAGMA temp.{name}", f"SELECT count(*) FROM pragma_{name} WHERE 1", f"SELECT * FROM pragma_{name} ORDER BY 1",
                f"SELECT * FROM pragma_{name} LIMIT 1", f"SELECT count(*) FROM (SELECT * FROM pragma_{name})", f"SELECT p.* FROM t2 JOIN pragma_{name} AS p ON 1 WHERE t2.a = 1"]
    return out


PRAGMA_SETUP = ["CREATE INDEX i1 ON t1(a)", "CREATE UNIQUE INDEX i2 ON t1(b, a DESC)", "CREATE INDEX t2_ab ON t2(b, c)", "CREATE TABLE pk(k INTEGER PRIMARY KEY, v TEXT)",
                "CREATE TABLE fk(x INTEGER REFERENCES pk(k), y TEXT)", "INSERT INTO pk VALUES (1, 'a')", "INSERT INTO fk VALUES (1, 'ok'), (2, 'dangling')"]

TYPES = ["INT", "INTEGER", "TINYINT", "BIGINT", "UNSIGNED BIG INT", "INT2", "CHARACTER(20)", "VARCHAR(255)", "NCHAR(55)", "TEXT", "CLOB", "BLOB", "", "REAL", "DOUBLE", "DOUBLE PRECISION",
         "FLOAT", "NUMERIC", "DECIMAL(10,5)", "BOOLEAN", "DATE", "DATETIME", "CHARINT", "FLOATING POINT", "POINT", "STRING", "ANY"]
RULE_VALUES = ["'12'", "'12.0'", "'1.5'", "'abc'", "12", "12.5", "x'3132'", "NULL", "'0x10'", "' 7 '", "'1e3'", "-0.0"]


def rule_cases(kind, name):
    """Cases for each datatype rule: the declared types and values it decides."""
    if kind == "rule.affinity":
        # each declared type the rule decides, every value stored through it, then what was stored
        out = []
        for i, t in enumerate([t for t in TYPES if name in tagmod.affinity_rules(t)][:4]):
            out.append(f"CREATE TABLE r{i}(x {t})" if t else f"CREATE TABLE r{i}(x)")
            out += [f"INSERT INTO r{i} VALUES ({v})" for v in RULE_VALUES]
            out.append(f"SELECT typeof(x), quote(x) FROM r{i} ORDER BY rowid")
        return [([], q) for q in out]
    if kind == "rule.expression-affinity":
        exprs = {"an IN list has no affinity": ["1 IN ('1')", "'1' IN (1)", "t IN (1)", "n IN ('1')", "1 IN (SELECT t FROM r)", "n IN (SELECT t FROM r)", "t IN (SELECT n FROM r)", "1.0 IN ('1.0')", "'a' IN ('A')", "n IN (1, '1')"],
                 "a column keeps its affinity": ["n = '1'", "t = 1", "(n) = '1'", "(t) = 1", "r.n = '1'", "n > '0'", "t < 2", "n = '1.0'", "t = 1.0", "n IS '1'"],
                 "an operator removes affinity": ["+n = '1'", "+t = 1", "n + 0 = '1'", "t || '' = 1", "-(-n) = '1'", "n * 1 = '1'", "coalesce(n) = '1'", "+n > '0'", "abs(n) = '1'", "(+t) = 1"],
                 "CAST gives its type's affinity": ["CAST(n AS TEXT) = 1", "CAST(t AS INTEGER) = '1'", "CAST('1' AS INTEGER) = '1'", "CAST(1 AS TEXT) = 1", "CAST(t AS REAL) = '1'", "CAST(n AS BLOB) = 1",
                                                    "CAST('5' AS NUMERIC) = '5'", "CAST(5 AS TEXT) > 10", "CAST(n AS NUMERIC) = '1'", "CAST(t AS TEXT) = 1"],
                 "COLLATE keeps its operand's affinity": ["(n COLLATE NOCASE) = '1'", "(t COLLATE BINARY) = 1", "(n COLLATE RTRIM) = '1 '", "(t COLLATE NOCASE) = 1.0", "(n COLLATE NOCASE) > '0'",
                                                          "(t COLLATE RTRIM) = 1", "(n COLLATE BINARY) IN ('1')", "(t COLLATE NOCASE) IS 1", "(n COLLATE NOCASE) = '01'", "(n COLLATE NOCASE) < '2'"],
                 "otherwise no affinity": ["1 = '1'", "'1' = 1", "1.0 = '1.0'", "x'31' = '1'", "'a' > 1", "NULL = '1'", "2 > '10'", "'2' > 10", "1 IS '1'", "(1) = ('1')"],
                 "view and subquery columns": ["(SELECT n FROM r) = '1'", "(SELECT t FROM r) = 1", "v_n = '1'", "v_t = 1", "v_e = '2'", "(SELECT n + 0 FROM r) = '1'", "v_n > '0'", "v_t < 2", "(SELECT CAST(n AS TEXT) FROM r) = 1", "v_e = 2"],
                 "compound view columns": ["c_n = '1'", "c_n = 1", "c_n > '0'", "c_t = 1", "c_t = '1'", "c_n IN ('1', '2')", "c_n BETWEEN '0' AND '2'", "c_t IS 1", "c_n + 0 = 1", "c_t || '' = '1'"]}[name]
        setup = ["CREATE TABLE r(n INTEGER, t TEXT)", "INSERT INTO r VALUES (1, '1')", "CREATE VIEW v AS SELECT n AS v_n, t AS v_t, n + 1 AS v_e FROM r",
                 "CREATE VIEW c AS SELECT n AS c_n, t AS c_t FROM r UNION ALL SELECT n, t FROM r"]
        src = "v" if any("v_" in e for e in exprs) else ("c" if any("c_" in e for e in exprs) else "r")
        return [(setup, f"SELECT {e} FROM {src}") for e in exprs]
    if kind == "rule.comparison":
        pairs = {"NULL is less than any value": ["SELECT x FROM (SELECT NULL AS x UNION ALL SELECT {v}) ORDER BY x"],
                 "numbers before text and blobs": ["SELECT {n} < {s}", "SELECT {n} < x'00'"], "numbers compare numerically": ["SELECT {n} < {m}", "SELECT {n} = {m}"],
                 "text before blobs": ["SELECT {s} < x'00'", "SELECT {s} < x'ff'"], "text compares by collation": ["SELECT {s} < {t}", "SELECT {s} < {t} COLLATE NOCASE"],
                 "blobs compare by memcmp": ["SELECT x'0102' < {b}", "SELECT {b} = x'01'"],
                 "numeric affinity goes to the other operand": ["SELECT n > {s} FROM r", "SELECT n = {s} FROM r"], "text affinity goes to an operand with none": ["SELECT t = {n} FROM r", "SELECT t > {n} FROM r"],
                 "otherwise compared as is": ["SELECT {s} = {n}", "SELECT {b} = {s}"], "BETWEEN is two comparisons": ["SELECT t BETWEEN {n} AND {m} FROM r", "SELECT n BETWEEN {s} AND {t} FROM r"],
                 "IN compares as equality": ["SELECT n IN (SELECT t FROM r) FROM r WHERE {n} >= 0", "SELECT {s} IN (SELECT n FROM r)"]}[name]
        nums, strs, blobs = ["1", "2", "10", "-1", "2.5", "0"], ["'1'", "'2'", "'10'", "'a'", "''", "'1.5'"], ["x'01'", "x'0101'", "x''", "x'ff'", "x'02'", "x'00'"]
        setup = ["CREATE TABLE r(n NUMERIC, t TEXT)", "INSERT INTO r VALUES (10, '10'), ('5', 5)"]
        out = []
        for i in range(12):
            tmpl, j = pairs[i % len(pairs)], i // len(pairs)
            q = tmpl.format(v=(nums + strs)[j % 12], n=nums[j % 6], m=nums[(j + 2) % 6], s=strs[j % 6], t=strs[(j + 3) % 6], b=blobs[j % 6])
            out.append((setup, q))
        return out
    if kind == "rule.sort":
        mixed = "SELECT x'00' AS x UNION ALL SELECT 'b' UNION ALL SELECT 2 UNION ALL SELECT NULL UNION ALL SELECT 1.5 UNION ALL SELECT 'A' UNION ALL SELECT 2.0 UNION ALL SELECT '2'"
        cases = {"ORDER BY orders by storage class": [f"SELECT x FROM ({mixed}) ORDER BY x{d}" for d in ("", " DESC", " NULLS LAST", " DESC NULLS FIRST")] + [f"SELECT typeof(x) FROM ({mixed}) ORDER BY x, typeof(x)", f"SELECT x FROM ({mixed}) ORDER BY x COLLATE NOCASE",
                                                                                                                                                    f"SELECT max(x) FROM ({mixed})", f"SELECT min(x) FROM ({mixed})", f"SELECT x FROM ({mixed}) WHERE x IS NOT NULL ORDER BY x LIMIT 3", f"SELECT x FROM ({mixed}) ORDER BY x LIMIT 2 OFFSET 2"],
                 "GROUP BY keeps classes apart": [f"SELECT count(*) AS n FROM ({mixed}) GROUP BY x ORDER BY n, min(typeof(x))", f"SELECT count(DISTINCT x) FROM ({mixed})", "SELECT count(*) FROM (SELECT 1 AS x UNION ALL SELECT 1.0 UNION ALL SELECT '1') GROUP BY x",
                                                  "SELECT count(*) FROM (SELECT DISTINCT x FROM (SELECT 1 AS x UNION ALL SELECT '1'))", "SELECT typeof(x), count(*) FROM (SELECT 2 AS x UNION ALL SELECT 2.0) GROUP BY x",
                                                  "SELECT count(*) FROM (SELECT x'31' AS x UNION ALL SELECT '1') GROUP BY x", "SELECT count(*) FROM (SELECT NULL AS x UNION ALL SELECT NULL) GROUP BY x", "SELECT count(*) FROM (SELECT 'a' AS x UNION ALL SELECT 'A') GROUP BY x",
                                                  "SELECT count(*) FROM (SELECT 'a' AS x UNION ALL SELECT 'A') GROUP BY x COLLATE NOCASE", "SELECT count(*) FROM (SELECT 1e0 AS x UNION ALL SELECT 1) GROUP BY x"],
                 "compound SELECTs compare as is": ["SELECT count(*) FROM (SELECT 1 UNION SELECT '1')", "SELECT count(*) FROM (SELECT 1 INTERSECT SELECT '1')", "SELECT count(*) FROM (SELECT 1 EXCEPT SELECT '1')",
                                                    "SELECT count(*) FROM (SELECT n FROM r UNION SELECT t FROM r)", "SELECT count(*) FROM (SELECT 1 UNION SELECT 1.0)", "SELECT count(*) FROM (SELECT x'31' UNION SELECT '1')",
                                                    "SELECT count(*) FROM (SELECT 'a' UNION SELECT 'A')", "SELECT count(*) FROM (SELECT NULL UNION SELECT NULL)", "SELECT count(*) FROM (SELECT n FROM r INTERSECT SELECT t FROM r)",
                                                    "SELECT count(*) FROM (SELECT 2 UNION ALL SELECT '2')"]}[name]
        return [(["CREATE TABLE r(n INTEGER, t TEXT)", "INSERT INTO r VALUES (1, '1')"], q) for q in cases]
    if kind == "rule.collation":
        setup = ["CREATE TABLE r(b TEXT, n TEXT COLLATE NOCASE, t TEXT COLLATE RTRIM)", "INSERT INTO r VALUES ('a', 'a', 'a '), ('B', 'B', 'B'), ('a ', 'A', 'a')"]
        qs = {"columns default to BINARY": ["SELECT b = 'A' FROM r", "SELECT count(*) FROM r WHERE b = 'a'", "SELECT count(*) FROM r WHERE b < 'b'", "SELECT b FROM r ORDER BY b", "SELECT count(DISTINCT b) FROM r",
                                            "SELECT min(b) FROM r", "SELECT b IN ('A', 'B') FROM r", "SELECT b BETWEEN 'A' AND 'Z' FROM r", "SELECT b > 'a' FROM r", "SELECT b FROM r GROUP BY b ORDER BY 1"],
              "an explicit COLLATE wins, left first": ["SELECT n = 'A' COLLATE BINARY FROM r", "SELECT b COLLATE NOCASE = n COLLATE BINARY FROM r", "SELECT 'A' COLLATE NOCASE = b FROM r", "SELECT b = 'A' COLLATE NOCASE FROM r",
                                                       "SELECT t COLLATE BINARY = 'a' FROM r", "SELECT count(*) FROM r WHERE b = 'A' COLLATE NOCASE", "SELECT n COLLATE BINARY < 'b' FROM r", "SELECT 'a ' COLLATE RTRIM = b FROM r",
                                                       "SELECT b COLLATE RTRIM = 'a' COLLATE NOCASE FROM r", "SELECT count(*) FROM r WHERE n = 'a' COLLATE BINARY"],
              "then a column's collation, left first": ["SELECT n = b FROM r", "SELECT b = n FROM r", "SELECT t = b FROM r", "SELECT b = t FROM r", "SELECT n = t FROM r", "SELECT t = n FROM r",
                                                        "SELECT count(*) FROM r WHERE n = 'A'", "SELECT count(*) FROM r WHERE 'A' = n", "SELECT count(*) FROM r WHERE t = 'a'", "SELECT +n = 'A' FROM r"],
              "otherwise BINARY": ["SELECT 'a' = 'A'", "SELECT 'a' < 'B'", "SELECT 'a ' = 'a'", "SELECT 'é' = 'É'", "SELECT 'ab' > 'a'", "SELECT 'B' < 'a'", "SELECT '' < 'a'", "SELECT 'Z' < 'a'", "SELECT 'a' IN ('A')", "SELECT 'a' BETWEEN 'A' AND 'B'"],
              "the leftmost COLLATE anywhere wins": ["SELECT 'a' COLLATE NOCASE = ('A' COLLATE BINARY)", "SELECT 'x' = ('X' COLLATE NOCASE) || ''", "SELECT ('a' COLLATE NOCASE) || '' = 'A'", "SELECT 'a' = upper('A' COLLATE NOCASE)",
                                                     "SELECT (b COLLATE NOCASE) = n COLLATE BINARY FROM r", "SELECT 'A' || '' = 'a' COLLATE NOCASE", "SELECT ('a' COLLATE RTRIM) = ('a ' COLLATE NOCASE)", "SELECT 'A' = ('a' COLLATE NOCASE)",
                                                     "SELECT ('a' COLLATE BINARY) = ('A' COLLATE NOCASE)", "SELECT coalesce('a' COLLATE NOCASE) = 'A'"],
              "IN uses the left operand's collation": ["SELECT 'a' COLLATE NOCASE IN ('A')", "SELECT 'a' IN ('A' COLLATE NOCASE)", "SELECT n IN ('A') FROM r", "SELECT b IN ('A') FROM r", "SELECT b COLLATE NOCASE IN ('A', 'X') FROM r",
                                                       "SELECT 'A' IN (SELECT n FROM r)", "SELECT n IN (SELECT b FROM r) FROM r", "SELECT t IN ('a') FROM r", "SELECT 'a ' IN (SELECT t FROM r)", "SELECT b IN (SELECT n FROM r) FROM r"],
              "ORDER BY uses COLLATE, then the column's": ["SELECT n FROM r ORDER BY n, b", "SELECT n FROM r ORDER BY n COLLATE BINARY", "SELECT b FROM r ORDER BY b COLLATE NOCASE, b", "SELECT t FROM r ORDER BY t, b",
                                                           "SELECT b FROM r ORDER BY b", "SELECT n FROM r ORDER BY n DESC, b", "SELECT b FROM r ORDER BY b COLLATE RTRIM, b", "SELECT n || '' FROM r ORDER BY 1", "SELECT n FROM r ORDER BY n COLLATE RTRIM, b",
                                                           "SELECT upper(n) FROM r ORDER BY n, 1"]}[name]
        return [(setup, q) for q in qs]
    return []


def kind_cases(f):
    """Cases for entries the other generators don't cover: syntax, keywords, words, types."""
    k, name = f["kind"], f["name"]
    if k == "datetime.modifier":
        mods = {"NNN days": ["+1 day", "-3 days", "+0.5 days", "10 days", "-365 days"], "NNN hours": ["+1 hour", "-25 hours", "+0.5 hours", "12 hours", "-1 hours"],
                "NNN minutes": ["+1 minute", "-90 minutes", "+0.5 minutes", "1440 minutes", "-1 minutes"], "NNN seconds": ["+1 second", "-3600 seconds", "+0.5 seconds", "86400 seconds", "-1 seconds"],
                "NNN months": ["+1 month", "-1 months", "+13 months", "12 months", "-25 months"], "NNN years": ["+1 year", "-1 years", "+100 years", "4 years", "-2000 years"]}.get(name)
        base = ["'2024-01-31'", "'2024-02-29 12:30:00'", "'2000-01-01'", "'1999-12-31 23:59:59'"]
        if mods:
            return [f"SELECT datetime({b}, '{m}')" for b in base for m in mods][:12]
        simple = {"start of month": "start of month", "start of year": "start of year", "start of day": "start of day", "weekday N": "weekday {d}", "unixepoch": "unixepoch", "julianday": "julianday",
                  "auto": "auto", "localtime": "localtime", "utc": "utc", "subsec": "subsec", "subsecond": "subsecond", "ceiling": "ceiling", "floor": "floor"}[name]
        if name in ("unixepoch", "auto"):
            return [f"SELECT datetime({v}, '{simple}')" for v in ["0", "1710460800", "-86400", "1e9", "2147483647", "1710460800.5", "'1710460800'", "86400", "1", "NULL"]]
        if name == "julianday":
            return [f"SELECT datetime({v}, 'julianday')" for v in ["2460385.5", "2451545", "0", "2440587.5", "2460385.75", "'2460385.5'", "1721425.5", "5373484.5", "2460000", "NULL"]]
        if name in ("localtime", "utc", "subsec", "subsecond"):
            return [f"SELECT typeof(datetime({b}, '{simple}'))" for b in base + ["'now'", "0", "'2038-01-19 03:14:07'"]] + [f"SELECT length(datetime({b}, '{simple}')) > 0" for b in base[:3]]
        if name in ("ceiling", "floor"):
            return [f"SELECT date({b}, '{m}', '{simple}')" for b in ["'2024-01-31'", "'2023-01-29'", "'2024-03-31'", "'2024-05-31'"] for m in ("+1 month", "-1 month", "+1 year")][:12]
        if name == "weekday N":
            return [f"SELECT date({b}, 'weekday {d}')" for b in base[:2] for d in range(7)][:12]
        return [f"SELECT datetime({b}, '{simple}')" for b in base + ["'2024-03-15 08:05:00'", "'2038-01-19 03:14:07'", "'1970-01-01'", "2460385.5", "'2024-12-31 23:59:59'", "'2024-06-15'"]]
    if k == "parameter":
        q = {"?NNN": ["SELECT ?1", "SELECT ?2 IS NULL", "SELECT coalesce(?1, 'unbound')", "SELECT ?1 + 1", "SELECT typeof(?3)", "SELECT ?10 IS ?10", "SELECT count(*) FROM t1 WHERE a = ?1", "SELECT ?1, ?2", "SELECT length(?5)", "SELECT ?999 IS NULL"],
             "?": ["SELECT ?", "SELECT ? IS NULL", "SELECT coalesce(?, 'unbound')", "SELECT ? + 1", "SELECT typeof(?)", "SELECT ?, ?", "SELECT count(*) FROM t1 WHERE a = ?", "SELECT ? IS ?", "SELECT length(?)", "SELECT ifnull(?, 0)"],
             ":AAAA": ["SELECT :v", "SELECT :v IS NULL", "SELECT coalesce(:name, 'unbound')", "SELECT :x + 1", "SELECT typeof(:t)", "SELECT :a, :b", "SELECT count(*) FROM t1 WHERE a = :id", "SELECT :v IS :v", "SELECT length(:s)", "SELECT ifnull(:n, 0)"],
             "@AAAA": ["SELECT @v", "SELECT @v IS NULL", "SELECT coalesce(@name, 'unbound')", "SELECT @x + 1", "SELECT typeof(@t)", "SELECT @a, @b", "SELECT count(*) FROM t1 WHERE a = @id", "SELECT @v IS @v", "SELECT length(@s)", "SELECT ifnull(@n, 0)"],
             "$AAAA": ["SELECT $v", "SELECT $v IS NULL", "SELECT coalesce($name, 'unbound')", "SELECT $x + 1", "SELECT typeof($t)", "SELECT $a, $b", "SELECT count(*) FROM t1 WHERE a = $id", "SELECT $v IS $v", "SELECT length($s)", "SELECT ifnull($n, 0)"]}[name]
        return q
    if k == "affinity":
        decl = {"TEXT": "TEXT", "NUMERIC": "NUMERIC", "INTEGER": "INTEGER", "REAL": "REAL", "BLOB": "BLOB"}[name]
        return [f"SELECT typeof(CAST({v} AS {decl})), CAST({v} AS {decl})" for v in RULE_VALUES[:10]]
    if k == "storage_class":
        lit = {"NULL": ["NULL", "(NULL)", "NULL + 1", "CAST(NULL AS TEXT)", "nullif(1, 1)", "max(NULL, NULL)", "(SELECT NULL)", "coalesce(NULL)", "NULL || 'x'", "-NULL"],
               "INTEGER": ["1", "-7", "0x10", "9223372036854775807", "CAST('5' AS INTEGER)", "length('abc')", "1 + 1", "(SELECT a FROM t1 LIMIT 1)", "abs(-3)", "7 / 2"],
               "REAL": ["1.5", "-0.0", "1e3", "2.0", "CAST(1 AS REAL)", "7.0 / 2", "1e308 * 10", "pi()", "avg(1)", "0.1 + 0.2"],
               "TEXT": ["'x'", "''", "'1'", "CAST(1 AS TEXT)", "upper('a')", "'a' || 1", "quote(1)", "hex(1)", "printf('%d', 3)", "char(65)"],
               "BLOB": ["x'01'", "x''", "zeroblob(2)", "CAST('a' AS BLOB)", "randomblob(0)", "x'00ff'", "unhex('41')", "(SELECT d FROM t1 LIMIT 1)", "jsonb('1')", "x'414243'"]}[name]
        return [f"SELECT typeof({v}), quote({v})" if name != "BLOB" or "randomblob" not in v else f"SELECT typeof({v})" for v in lit]
    if k == "collation":
        return [f"SELECT {a} = {b} COLLATE {name}" for a, b in [("'a'", "'A'"), ("'a '", "'a'"), ("'b'", "'B'"), ("'abc'", "'ABC'"), ("'x  '", "'x'"), ("'é'", "'É'"), ("''", "' '"), ("'Z'", "'z'"), ("'a1'", "'A1'"), ("' a'", "'a'")]] + [f"SELECT b FROM t1 ORDER BY b COLLATE {name}, a"]
    if k == "expression":
        return {"scalar subquery": [f"SELECT (SELECT {e} FROM t1)" for e in ["max(a)", "min(b)", "count(*)", "a WHERE a = 2", "b ORDER BY a DESC LIMIT 1", "avg(c)", "sum(a) WHERE a > 2", "a WHERE a = 99", "group_concat(a)", "total(c)"]],
                "correlated subquery": [f"SELECT a, (SELECT {e} FROM t2 WHERE t2.a = t1.a) FROM t1 ORDER BY a" for e in ["b", "c", "count(*)", "max(c)", "b || '!'", "c * 2", "coalesce(b, '-')", "length(b)", "c IS NULL", "min(b)"]],
                "row value": ["SELECT (1, 2) = (1, 2)", "SELECT (1, 2) < (1, 3)", "SELECT (2, 'y') IN (SELECT a, b FROM t2)", "SELECT (1, NULL) = (1, NULL)", "SELECT (a, b) = (2, 'two') FROM t1", "SELECT (1, 2, 3) > (1, 2, 2)",
                              "SELECT count(*) FROM t1 WHERE (a, b) IN (VALUES (1, 'one'), (2, 'two'))", "SELECT (1, 'a') <> (1, 'b')", "SELECT (SELECT a, b FROM t1 WHERE a = 1) = (1, 'one')", "SELECT (a, c) BETWEEN (1, 0) AND (3, 0) FROM t1"],
                "boolean expression": [f"SELECT CASE WHEN {v} THEN 1 ELSE 0 END" for v in ["'abc'", "'1x'", "0.5", "-1", "'0'", "''", "NULL", "x'01'", "'0.0'", "1e-10"]] + ["SELECT count(*) FROM t1 WHERE b", "SELECT count(*) FROM t1 WHERE c"]}[name]
    if k == "word":
        return {"TRUE": ["SELECT TRUE", "SELECT TRUE + 1", "SELECT TRUE = 1", "SELECT typeof(TRUE)", "SELECT NOT TRUE", "SELECT TRUE AND FALSE", "SELECT 2 IS TRUE", "SELECT 0 IS NOT TRUE", "SELECT count(*) FROM t1 WHERE TRUE", "SELECT TRUE || ''"],
                "FALSE": ["SELECT FALSE", "SELECT FALSE + 1", "SELECT FALSE = 0", "SELECT typeof(FALSE)", "SELECT NOT FALSE", "SELECT FALSE OR TRUE", "SELECT 0 IS FALSE", "SELECT 2 IS NOT FALSE", "SELECT count(*) FROM t1 WHERE FALSE", "SELECT FALSE || ''"],
                "STRICT": [], "STORED": [], "ROWID": [], "EXPRESSIONS": [], "E": [f"SELECT {v}" for v in ["1e3", "1E3", "2.5e-1", "1e+2", ".5e1", "1e0", "9e-3", "1E308", "3e-308", "12.5E2"]]}.get(name, [])
    return []


def word_groups(name):
    """Words that need their own schema: STRICT, STORED, ROWID, EXPRESSIONS."""
    if name == "STRICT":
        return [(["CREATE TABLE st(k INTEGER, v TEXT, r REAL, b BLOB, a ANY) STRICT"], q) for q in [
            "INSERT INTO st VALUES (1, 'x', 1.5, x'01', 1)", "INSERT INTO st VALUES ('2', 'y', 2, x'02', 'any')", "INSERT INTO st VALUES ('abc', 'z', 1, NULL, 1)", "INSERT INTO st VALUES (3, 3, 3, 3, 3)",
            "INSERT INTO st VALUES (4, 'w', 'nope', NULL, 1)", "SELECT typeof(k), typeof(v), typeof(r) FROM st", "INSERT INTO st(k) VALUES (5.0)", "INSERT INTO st(k) VALUES (5.5)",
            "SELECT count(*) FROM st", "INSERT INTO st(b) VALUES ('text')"]]
    if name == "STORED":
        return [(["CREATE TABLE g(a INT, b INT AS (a * 2) STORED, c TEXT GENERATED ALWAYS AS (upper(t)) STORED, t TEXT)"], q) for q in [
            "INSERT INTO g(a, t) VALUES (3, 'x')", "INSERT INTO g(a, t) VALUES (NULL, NULL)", "SELECT a, b, c FROM g ORDER BY a", "INSERT INTO g(a, b) VALUES (1, 2)", "UPDATE g SET a = 10 WHERE a = 3",
            "SELECT b FROM g WHERE a = 10", "SELECT count(*) FROM g WHERE c IS NULL", "PRAGMA table_xinfo(g)", "INSERT INTO g VALUES (1, 2, 'c', 't')", "SELECT typeof(b) FROM g"]]
    if name == "ROWID":
        return [(["CREATE TABLE w(k INT PRIMARY KEY, v TEXT) WITHOUT ROWID"], q) for q in [
            "INSERT INTO w VALUES (1, 'a')", "INSERT INTO w VALUES (2, 'b'), (3, 'c')", "SELECT k, v FROM w ORDER BY k", "SELECT rowid FROM w", "INSERT INTO w VALUES (1, 'dup')",
            "INSERT INTO w VALUES (NULL, 'n')", "UPDATE w SET v = 'z' WHERE k = 2", "DELETE FROM w WHERE k = 3", "SELECT count(*) FROM w", "INSERT OR REPLACE INTO w VALUES (1, 'new')"]]
    if name == "EXPRESSIONS":
        return [(["CREATE INDEX ei ON t1(lower(b))", "CREATE INDEX ej ON t1(a + 1)"], q) for q in [
            "REINDEX EXPRESSIONS", "REINDEX EXPRESSIONS; SELECT count(*) FROM t1 WHERE lower(b) = 'one'", "REINDEX main.EXPRESSIONS", "SELECT count(*) FROM t1 WHERE a + 1 = 3",
            "REINDEX EXPRESSIONS; SELECT count(*) FROM t1 WHERE a + 1 > 2", "REINDEX ei", "REINDEX", "REINDEX t1", "REINDEX EXPRESSIONS; PRAGMA integrity_check", "REINDEX main.ei"]]
    return []


# Syntax productions: each production's branches, its errors included (a statement SQLite refuses is a
# case too: `statement error`). Productions a group of another already reaches (join-constraint in
# join-clause's) are counted there.
SYNTAX = {
    "aggregate-function-invocation": [([], [
        "SELECT count(*) FROM t1", "SELECT count(DISTINCT b) FROM t1", "SELECT sum(a) FILTER (WHERE a > 2) FROM t1", "SELECT group_concat(b ORDER BY a DESC) FROM t1",
        "SELECT a % 2, max(c) FROM t1 GROUP BY a % 2 ORDER BY 1", "SELECT avg(a), total(c) FROM t1", "SELECT count(e) FROM t1", "SELECT string_agg(b, '-' ORDER BY b) FROM t1",
        "SELECT min(b) FROM t1 WHERE a > 1", "SELECT count(*) FILTER (WHERE b IS NULL) FROM t1", "SELECT group_concat(DISTINCT a) FROM t1", "SELECT sum(DISTINCT a % 3) FROM t1"])],
    "alter-table-stmt": [(["CREATE TABLE at(x INTEGER, y TEXT)", "INSERT INTO at VALUES (1, 'a')"], [
        "ALTER TABLE at RENAME TO at2", "ALTER TABLE at2 RENAME COLUMN y TO z", "ALTER TABLE at2 ADD COLUMN w REAL DEFAULT 1.5", "ALTER TABLE at2 ADD w2 TEXT", "SELECT * FROM at2",
        "ALTER TABLE at2 DROP COLUMN w2", "ALTER TABLE at2 RENAME z TO y", "ALTER TABLE main.at2 RENAME TO at3", "ALTER TABLE at3 ADD COLUMN c TEXT NOT NULL DEFAULT 'x' CHECK (c <> '')",
        "ALTER TABLE at3 DROP x", "ALTER TABLE nope RENAME TO z", "SELECT sql FROM sqlite_schema WHERE name = 'at3'", "ALTER TABLE at3 ADD COLUMN k INTEGER PRIMARY KEY",
        "ALTER TABLE at3 ADD COLUMN u TEXT UNIQUE", "ALTER TABLE at3 ADD COLUMN n TEXT NOT NULL", "SELECT * FROM at3"])],
    "analyze-stmt": [(["CREATE INDEX i1 ON t1(a)", "CREATE INDEX i2 ON t2(c)"], [
        "ANALYZE", "SELECT tbl, idx, stat FROM sqlite_stat1 ORDER BY tbl, idx", "ANALYZE main", "ANALYZE t1", "ANALYZE main.t1", "ANALYZE i1", "ANALYZE temp", "ANALYZE nope",
        "SELECT count(*) FROM sqlite_stat1", "DELETE FROM sqlite_stat1", "ANALYZE t2", "SELECT tbl, idx FROM sqlite_stat1 ORDER BY 1, 2"])],
    "attach-stmt": [([], [
        "ATTACH ':memory:' AS aux", "ATTACH DATABASE ':memory:' AS aux2", "CREATE TABLE aux.x(a)", "INSERT INTO aux.x VALUES (1), (2)", "SELECT * FROM aux.x ORDER BY a",
        "SELECT name FROM pragma_database_list ORDER BY seq", "ATTACH ':memory:' AS aux", "ATTACH '' AS tmpdb", "SELECT count(*) FROM aux.sqlite_schema",
        "CREATE TABLE aux2.y AS SELECT a * 10 AS b FROM aux.x", "SELECT b FROM aux2.y ORDER BY b", "ATTACH ':memory:' AS main", "SELECT a FROM x ORDER BY a"])],
    "begin-stmt": [([], [
        "BEGIN", "INSERT INTO t2 VALUES (10, 'b', 1)", "COMMIT", "BEGIN DEFERRED", "SELECT count(*) FROM t2", "ROLLBACK", "BEGIN IMMEDIATE TRANSACTION", "UPDATE t2 SET c = 0 WHERE a = 10",
        "END", "BEGIN EXCLUSIVE", "BEGIN", "END TRANSACTION", "BEGIN TRANSACTION", "DELETE FROM t2 WHERE a = 10", "ROLLBACK TRANSACTION", "SELECT a, c FROM t2 WHERE a = 10"])],
    "column-constraint": [([], [
        "CREATE TABLE cc1(a INTEGER PRIMARY KEY ASC AUTOINCREMENT, b TEXT NOT NULL ON CONFLICT REPLACE DEFAULT 'dflt', c INTEGER UNIQUE ON CONFLICT IGNORE, d REAL CHECK (d >= 0),"
        " e TEXT COLLATE NOCASE, f INTEGER REFERENCES t2(a), g INTEGER GENERATED ALWAYS AS (c * 2) VIRTUAL, h TEXT AS (upper(e)) STORED, i INTEGER CONSTRAINT i_nn NOT NULL DEFAULT (1 + 1))",
        "INSERT INTO cc1(b, c, d, e, f) VALUES ('x', 1, 1.5, 'Abc', 1)", "INSERT INTO cc1(b, c, d) VALUES (NULL, 2, 0)", "INSERT INTO cc1(b, c, d) VALUES ('dup', 1, 0)",
        "INSERT INTO cc1(b, c, d) VALUES ('neg', 3, -1)", "SELECT a, b, c, d, e, g, h, i FROM cc1 ORDER BY a", "SELECT count(*) FROM cc1 WHERE e = 'ABC'",
        "CREATE TABLE cc2(k TEXT PRIMARY KEY DESC ON CONFLICT FAIL, v INTEGER DEFAULT -1, w INTEGER DEFAULT (length(CURRENT_DATE) = 10))", "INSERT INTO cc2(k) VALUES ('a')",
        "INSERT INTO cc2(k) VALUES ('a')", "SELECT k, v, w FROM cc2", "CREATE TABLE cc3(x INTEGER CONSTRAINT x_pos CHECK (x > 0) CONSTRAINT x_nn NOT NULL)", "INSERT INTO cc3 VALUES (0)",
        "INSERT INTO cc3 VALUES (NULL)", "INSERT INTO cc3 VALUES (5)", "SELECT x FROM cc3", "SELECT name FROM sqlite_sequence"])],
    "column-def": [([], [
        "CREATE TABLE cd1(a)", "CREATE TABLE cd2(a INT, b VARCHAR(10), c DECIMAL(10, 2))", "CREATE TABLE cd3(\"quoted col\" TEXT, [bracket col] INT, `tick col` REAL)",
        "INSERT INTO cd3 VALUES ('q', 1, 2.5)", "SELECT \"quoted col\", [bracket col], `tick col` FROM cd3", "CREATE TABLE cd4(a INTEGER NOT NULL, b TEXT DEFAULT 'x' COLLATE NOCASE)",
        "SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info('cd4') ORDER BY cid", "CREATE TABLE cd5(a, a)", "CREATE TABLE cd6(rowid INTEGER, oid TEXT)",
        "INSERT INTO cd6 VALUES (5, 'o')", "SELECT rowid, oid, _rowid_ FROM cd6", "CREATE TABLE cd7(a UNSIGNED BIG INT, b DOUBLE PRECISION, c NATIVE CHARACTER(70))",
        "SELECT name, type FROM pragma_table_info('cd7') ORDER BY cid", "CREATE TABLE cd8(\"\")", "CREATE TABLE cd9(select, \"from\")"])],
    "column-name-list": [([], [
        "UPDATE t2 SET (b, c) = ('q', 1) WHERE a = 1", "UPDATE t2 SET (c) = (5) WHERE a = 2", "UPDATE t2 SET (b, c) = (SELECT 'w', 9) WHERE a = 3", "SELECT a, b, c FROM t2 ORDER BY a",
        "UPDATE t2 SET (b, c) = (c, b) WHERE a = 6", "UPDATE t2 SET (b, c) = ('only one')", "UPDATE t2 SET (c, b) = (SELECT max(a), min(b) FROM t1) WHERE a = 7", "SELECT a, b, c FROM t2 ORDER BY a",
        "INSERT INTO t2(a, b, c) VALUES (1, 'z', 0) ON CONFLICT (a) DO UPDATE SET (b, c) = (excluded.b, excluded.c + 1)", "SELECT a, b, c FROM t2 WHERE a = 1",
        "UPDATE t2 SET (b) = 'scalar' WHERE a = 1", "UPDATE t2 SET (a, c) = (100, 100) WHERE a = 7", "SELECT a, c FROM t2 WHERE a = 100"])],
    "comment-syntax": [([], [
        "SELECT 1 -- trailing comment", "SELECT /* inline */ 2", "/* leading */ SELECT 3", "SELECT 4 /* to the end", "-- a line\nSELECT 5", "SELECT 6 /* a */ + /* b */ 1",
        "SELECT '-- not a comment', '/* nor this */'", "SELECT 7 --", "SELECT/**/8", "SELECT 9 /* multi\nline */ + 1", "SELECT a FROM t1 -- the rows\nWHERE a = 2", "SELECT 10 /* nested /* no */",
        "SELECT 11 /* a */ /* b */"])],
    "commit-stmt": [([], [
        "BEGIN", "INSERT INTO t2 VALUES (20, 'c', 2)", "COMMIT", "SELECT count(*) FROM t2 WHERE a = 20", "BEGIN", "COMMIT TRANSACTION", "BEGIN", "END", "COMMIT", "BEGIN",
        "DELETE FROM t2 WHERE a = 20", "END TRANSACTION", "SELECT count(*) FROM t2 WHERE a = 20"])],
    "common-table-expression": [([], [
        "WITH x AS (SELECT 1 AS v) SELECT v FROM x", "WITH x(p, q) AS (SELECT 1, 2) SELECT q, p FROM x", "WITH x AS MATERIALIZED (SELECT a FROM t1 WHERE a > 3) SELECT count(*) FROM x",
        "WITH x AS NOT MATERIALIZED (SELECT a FROM t1) SELECT max(a) FROM x", "WITH x AS (SELECT a FROM t1), y AS (SELECT a FROM x WHERE a % 2 = 0) SELECT a FROM y ORDER BY a",
        "WITH x AS (SELECT a, b FROM t2) SELECT t1.a, x.b FROM t1 JOIN x ON x.a = t1.a ORDER BY t1.a", "SELECT (WITH x AS (SELECT 41) SELECT * FROM x) + 1",
        "WITH x AS (SELECT 5 AS a) INSERT INTO t2(a, b) SELECT a, 'cte' FROM x", "WITH x AS (SELECT 5 AS a) UPDATE t2 SET c = 55 WHERE a IN (SELECT a FROM x)",
        "WITH x AS (SELECT 5 AS a) DELETE FROM t2 WHERE a IN x RETURNING a, b, c", "WITH x(a) AS (VALUES (1), (2), (3)) SELECT sum(a) FROM x", "WITH x AS (SELECT 1), x AS (SELECT 2) SELECT * FROM x"])],
    "compound-operator": [([], [
        "SELECT a FROM t1 UNION SELECT a FROM t2 ORDER BY 1", "SELECT a FROM t1 UNION ALL SELECT a FROM t2 ORDER BY 1", "SELECT a FROM t1 INTERSECT SELECT a FROM t2 ORDER BY 1",
        "SELECT a FROM t1 EXCEPT SELECT a FROM t2 ORDER BY 1", "SELECT b FROM t2 UNION SELECT b FROM t1 ORDER BY 1", "SELECT 1 UNION SELECT 1", "SELECT 1 UNION ALL SELECT 1",
        "SELECT NULL INTERSECT SELECT NULL", "SELECT 1 EXCEPT SELECT NULL", "VALUES (1), (2) EXCEPT VALUES (2)",
        "SELECT a FROM t1 WHERE a < 3 UNION ALL SELECT a FROM t1 WHERE a > 5 EXCEPT SELECT 1 ORDER BY 1", "SELECT 1, 2 UNION SELECT 3"])],
    "compound-select-stmt": [([], [
        "SELECT a FROM t1 UNION SELECT c FROM t2 ORDER BY 1 LIMIT 3", "SELECT a FROM t1 UNION ALL SELECT a FROM t2 ORDER BY 1 DESC LIMIT 2 OFFSET 1", "SELECT b FROM t1 INTERSECT SELECT b FROM t2",
        "SELECT a, b FROM t1 EXCEPT SELECT a, b FROM t2 ORDER BY a", "SELECT 1 AS x UNION SELECT 2 ORDER BY x DESC",
        "WITH w AS (SELECT a FROM t1) SELECT a FROM w UNION SELECT a + 100 FROM w ORDER BY 1 LIMIT 4", "SELECT a FROM t1 UNION SELECT a FROM t2 UNION SELECT 99 ORDER BY 1",
        "SELECT count(*) FROM (SELECT a FROM t1 INTERSECT SELECT a FROM t2)", "SELECT a FROM t1 UNION SELECT a FROM t2 ORDER BY a COLLATE BINARY DESC NULLS LAST",
        "VALUES (3) UNION ALL VALUES (1) ORDER BY 1", "SELECT a FROM t1 ORDER BY a UNION SELECT 1", "SELECT max(a) FROM t1 UNION ALL SELECT min(a) FROM t1"])],
    "conflict-clause": [(["CREATE TABLE cf(k INTEGER PRIMARY KEY, u TEXT UNIQUE, n TEXT NOT NULL)", "INSERT INTO cf VALUES (1, 'a', 'x'), (2, 'b', 'y')"], [
        "INSERT OR IGNORE INTO cf VALUES (1, 'z', 'z')", "INSERT OR REPLACE INTO cf VALUES (2, 'a', 'r')", "SELECT * FROM cf ORDER BY k", "INSERT OR ABORT INTO cf VALUES (3, 'r', NULL)",
        "INSERT OR FAIL INTO cf VALUES (4, 'a', 'f')", "BEGIN", "INSERT INTO cf VALUES (5, 'q', 'q')", "INSERT OR ROLLBACK INTO cf VALUES (5, 'q2', 'q2')", "SELECT count(*) FROM cf",
        "UPDATE OR IGNORE cf SET u = 'a' WHERE k = 2", "UPDATE OR REPLACE cf SET u = 'zz' WHERE k = 2", "REPLACE INTO cf VALUES (9, 'zz', 'rep')", "SELECT * FROM cf ORDER BY k",
        "CREATE TABLE cf2(a TEXT UNIQUE ON CONFLICT REPLACE, b INTEGER NOT NULL ON CONFLICT IGNORE)", "INSERT INTO cf2 VALUES ('x', 1), ('x', 2), ('y', NULL)", "SELECT * FROM cf2"])],
    "create-index-stmt": [([], [
        "CREATE INDEX ci1 ON t1(a)", "CREATE UNIQUE INDEX ci2 ON t2(b, c)", "CREATE INDEX IF NOT EXISTS ci1 ON t1(b)", "CREATE INDEX ci3 ON t1(c) WHERE c > 0", "CREATE INDEX ci4 ON t1(lower(b), a DESC)",
        "CREATE INDEX ci5 ON t1(b COLLATE NOCASE ASC)", "CREATE INDEX main.ci6 ON t1(e)", "CREATE INDEX ci1 ON t1(a)", "CREATE UNIQUE INDEX ci7 ON t1(a % 2)",
        "SELECT name, tbl_name FROM sqlite_schema WHERE type = 'index' AND name LIKE 'ci%' ORDER BY name", "SELECT count(*) FROM t1 WHERE lower(b) = 'two'", "CREATE INDEX ci8 ON nope(a)",
        "CREATE INDEX ci9 ON t1(random())", "CREATE INDEX ci10 ON t1(a) WHERE b > (SELECT 1)", "CREATE INDEX temp.ci11 ON t1(a)"])],
    "create-table-stmt": [([], [
        "CREATE TABLE ct1(a, b)", "CREATE TEMP TABLE ct2(a INTEGER)", "CREATE TEMPORARY TABLE ct3(a)", "CREATE TABLE IF NOT EXISTS ct1(z)", "CREATE TABLE ct4 AS SELECT a, b FROM t1 WHERE a > 2",
        "SELECT * FROM ct4 ORDER BY a", "CREATE TABLE ct5(k INTEGER PRIMARY KEY, v) WITHOUT ROWID", "CREATE TABLE ct6(k INTEGER, v TEXT) STRICT", "CREATE TABLE ct7(k TEXT PRIMARY KEY, v ANY) WITHOUT ROWID, STRICT",
        "CREATE TABLE main.ct8(x)", "CREATE TABLE temp.ct9(x)", "CREATE TABLE ct1(y)", "CREATE TABLE ct10(a) WITHOUT ROWID", "SELECT name, type FROM sqlite_schema WHERE name LIKE 'ct%' ORDER BY name",
        "SELECT name FROM sqlite_temp_schema ORDER BY name", "CREATE TABLE sqlite_x(a)", "CREATE TABLE ct11 AS SELECT count(*) AS n, max(c) AS m FROM t1", "SELECT name, type FROM pragma_table_info('ct11') ORDER BY cid"])],
    "create-view-stmt": [([], [
        "CREATE VIEW cv1 AS SELECT a, b FROM t1 WHERE a > 2", "SELECT * FROM cv1 ORDER BY a", "CREATE TEMP VIEW cv2 AS SELECT count(*) AS n FROM t2", "SELECT n FROM cv2",
        "CREATE VIEW IF NOT EXISTS cv1 AS SELECT 1", "CREATE VIEW cv3(x, y) AS SELECT a, c FROM t2", "SELECT x, y FROM cv3 ORDER BY x", "CREATE VIEW cv4 AS SELECT t1.a, t2.b FROM t1 JOIN t2 USING (a)",
        "SELECT * FROM cv4 ORDER BY a", "CREATE VIEW cv5 AS SELECT a % 2 AS g, sum(c) AS s FROM t1 GROUP BY g", "SELECT g, s FROM cv5 ORDER BY g", "INSERT INTO cv1 VALUES (9, 'x')",
        "CREATE VIEW cv6(x) AS SELECT 1, 2", "CREATE TEMPORARY VIEW main.cv7 AS SELECT 1", "CREATE VIEW cv8 AS SELECT * FROM cv8"])],
    "create-virtual-table-stmt": [([], [
        "CREATE VIRTUAL TABLE vt1 USING fts5(title, body)", "INSERT INTO vt1 VALUES ('sqlite', 'small fast reliable'), ('postgres', 'reliable and extensible')",
        "SELECT title FROM vt1 WHERE vt1 MATCH 'reliable' ORDER BY title", "SELECT title FROM vt1 WHERE body MATCH 'fast'", "CREATE VIRTUAL TABLE IF NOT EXISTS vt1 USING fts5(x)",
        "CREATE VIRTUAL TABLE vt2 USING rtree(id, minx, maxx)", "INSERT INTO vt2 VALUES (1, 0, 10), (2, 5, 15)", "SELECT id FROM vt2 WHERE minx <= 7 AND maxx >= 7 ORDER BY id",
        "CREATE VIRTUAL TABLE vt3 USING fts4(content)", "INSERT INTO vt3 VALUES ('one two three')", "SELECT count(*) FROM vt3 WHERE vt3 MATCH 'two'", "CREATE VIRTUAL TABLE vt4 USING nope_module(x)",
        "CREATE VIRTUAL TABLE main.vt5 USING fts5(a, tokenize = 'porter')", "INSERT INTO vt5 VALUES ('running')", "SELECT count(*) FROM vt5 WHERE vt5 MATCH 'run'",
        "SELECT name FROM sqlite_schema WHERE sql LIKE 'CREATE VIRTUAL%' ORDER BY name", "CREATE VIRTUAL TABLE temp.vt6 USING dbstat", "SELECT count(*) > 0 FROM vt6"])],
    "cte-table-name": [([], [
        "WITH c(x) AS (SELECT 1) SELECT x FROM c", "WITH c(x, y) AS (SELECT 1, 2) SELECT y FROM c", "WITH c(x) AS (VALUES (5), (6)) SELECT sum(x) FROM c",
        "WITH c(\"odd name\") AS (SELECT 3) SELECT \"odd name\" FROM c", "WITH c(x) AS (SELECT 1, 2) SELECT * FROM c", "WITH c(a, b) AS (SELECT b, a FROM t2) SELECT a, b FROM c ORDER BY b",
        "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 4) SELECT group_concat(n) FROM c", "WITH c(x) AS MATERIALIZED (SELECT a FROM t1) SELECT count(x) FROM c",
        "WITH c(x) AS (SELECT a FROM t1), d(y) AS (SELECT x * 2 FROM c) SELECT max(y) FROM d", "WITH c(x, x) AS (SELECT 1, 2) SELECT x FROM c", "WITH c(x) AS (SELECT 1) SELECT c.x FROM c",
        "WITH \"c t\"(x) AS (SELECT 9) SELECT x FROM \"c t\""])],
    "delete-stmt-limited": [([], [
        "DELETE FROM t1 ORDER BY a LIMIT 1", "DELETE FROM t1 WHERE a > 2 LIMIT 2", "DELETE FROM t1 LIMIT 1 OFFSET 1", "DELETE FROM t2 ORDER BY c DESC LIMIT 1",
        "DELETE FROM t1 WHERE b IS NULL ORDER BY a LIMIT 1", "DELETE FROM t2 LIMIT 0", "DELETE FROM t2 ORDER BY a", "DELETE FROM t1 WHERE a = 1 LIMIT -1",
        "DELETE FROM t2 WHERE a > 1 ORDER BY b LIMIT 1 OFFSET 1", "DELETE FROM t1 LIMIT 2, 1", "SELECT count(*) FROM t1", "SELECT count(*) FROM t2"])],
    "delete-stmt": [([], [
        "DELETE FROM t1 WHERE a = 1", "DELETE FROM t1 WHERE b IS NULL RETURNING a, c", "SELECT count(*) FROM t1", "DELETE FROM main.t1 WHERE a > 5", "DELETE FROM t1 AS x WHERE x.a = 2",
        "DELETE FROM t2 WHERE a IN (SELECT a FROM t1)", "SELECT a FROM t2 ORDER BY a", "WITH k AS (SELECT 3 AS a) DELETE FROM t1 WHERE a IN k", "DELETE FROM t1 WHERE c > (SELECT avg(c) FROM t1) RETURNING *",
        "DELETE FROM t2", "SELECT count(*) FROM t2", "DELETE FROM nope", "DELETE FROM t1 INDEXED BY nope WHERE a = 1", "DELETE FROM t1 NOT INDEXED WHERE a = 5", "SELECT count(*) FROM t1"])],
    "detach-stmt": [(["ATTACH ':memory:' AS d1", "ATTACH ':memory:' AS d2"], [
        "DETACH d1", "DETACH DATABASE d2", "DETACH d1", "SELECT name FROM pragma_database_list ORDER BY seq", "ATTACH ':memory:' AS d3", "BEGIN", "CREATE TABLE d3.x(a)", "DETACH d3", "COMMIT",
        "DETACH d3", "DETACH main", "DETACH temp", "ATTACH ':memory:' AS d1", "DETACH \"d1\""])],
    "drop-index-stmt": [(["CREATE INDEX di1 ON t1(a)", "CREATE INDEX di2 ON t2(c)", "CREATE INDEX di3 ON t1(b)"], [
        "DROP INDEX di1", "DROP INDEX IF EXISTS di1", "DROP INDEX di1", "DROP INDEX main.di2", "DROP INDEX IF EXISTS main.nope", "DROP INDEX temp.di3", "DROP INDEX di3",
        "SELECT count(*) FROM sqlite_schema WHERE type = 'index'", "CREATE INDEX di4 ON t1(c)", "DROP INDEX \"di4\"", "DROP INDEX sqlite_autoindex_t2_1", "DROP INDEX IF EXISTS di4"])],
    "drop-table-stmt": [(["CREATE TABLE dt1(a)", "CREATE TABLE dt2(a)", "CREATE TEMP TABLE dt3(a)", "CREATE VIEW dv AS SELECT * FROM dt1"], [
        "DROP TABLE dt1", "SELECT * FROM dv", "DROP TABLE IF EXISTS dt1", "DROP TABLE dt1", "DROP TABLE main.dt2", "DROP TABLE temp.dt3", "DROP TABLE IF EXISTS main.nope", "DROP TABLE dv",
        "CREATE TABLE dt4(k INTEGER PRIMARY KEY)", "CREATE TABLE dt5(r INTEGER REFERENCES dt4(k))", "INSERT INTO dt4 VALUES (1)", "INSERT INTO dt5 VALUES (1)", "PRAGMA foreign_keys = ON",
        "DROP TABLE dt4", "SELECT count(*) FROM sqlite_schema WHERE name LIKE 'dt%'", "DROP TABLE sqlite_schema"])],
    "drop-trigger-stmt": [(["CREATE TABLE tg(a)", "CREATE TRIGGER tr1 AFTER INSERT ON tg BEGIN SELECT 1; END", "CREATE TRIGGER tr2 BEFORE DELETE ON tg BEGIN SELECT 1; END",
                            "CREATE TEMP TRIGGER tr3 AFTER UPDATE ON tg BEGIN SELECT 1; END"], [
        "DROP TRIGGER tr1", "DROP TRIGGER IF EXISTS tr1", "DROP TRIGGER tr1", "DROP TRIGGER main.tr2", "DROP TRIGGER temp.tr3", "DROP TRIGGER IF EXISTS main.nope",
        "SELECT count(*) FROM sqlite_schema WHERE type = 'trigger'", "CREATE TRIGGER tr4 AFTER INSERT ON tg BEGIN SELECT 1; END", "DROP TABLE tg", "SELECT count(*) FROM sqlite_schema WHERE type = 'trigger'",
        "DROP TRIGGER tr4", "DROP TRIGGER \"nope\""])],
    "drop-view-stmt": [(["CREATE VIEW dv1 AS SELECT 1", "CREATE VIEW dv2 AS SELECT 2", "CREATE TEMP VIEW dv3 AS SELECT 3", "CREATE VIEW dv4 AS SELECT * FROM dv1"], [
        "DROP VIEW dv1", "SELECT * FROM dv4", "DROP VIEW IF EXISTS dv1", "DROP VIEW dv1", "DROP VIEW main.dv2", "DROP VIEW temp.dv3", "DROP VIEW IF EXISTS main.nope", "DROP VIEW t1", "DROP VIEW dv4",
        "SELECT count(*) FROM sqlite_schema WHERE type = 'view'", "CREATE VIEW dv5 AS SELECT 5", "DROP VIEW \"dv5\""])],
    "expr": [([], [
        "SELECT 1 + 2 * 3", "SELECT (1 + 2) * 3", "SELECT -a FROM t1 ORDER BY a", "SELECT a FROM t1 WHERE a BETWEEN 2 AND 4 ORDER BY a", "SELECT b || '!' FROM t1 ORDER BY a",
        "SELECT CASE WHEN a > 3 THEN 'big' ELSE 'small' END FROM t1 ORDER BY a", "SELECT CAST(c AS INTEGER) FROM t1 ORDER BY a", "SELECT a IN (1, 3, 5) FROM t1 ORDER BY a",
        "SELECT EXISTS (SELECT 1 FROM t2 WHERE t2.a = 1)", "SELECT b LIKE 't%' FROM t1 ORDER BY a", "SELECT a IS NULL, a IS NOT NULL FROM t1 ORDER BY a", "SELECT abs(c) FROM t1 ORDER BY a",
        "SELECT b COLLATE NOCASE = 'FIVE' FROM t1 ORDER BY a", "SELECT (SELECT max(a) FROM t2) - a FROM t1 ORDER BY a", "SELECT a & 1, a | 8, a << 2, a >> 1, ~a FROM t1 ORDER BY a",
        "SELECT ?1 IS NULL", "SELECT t1.a FROM t1 WHERE t1.a = 2", "SELECT main.t1.a FROM t1 WHERE a = 3", "SELECT (a, b) = (2, 'two') FROM t1 ORDER BY a", "SELECT a NOT BETWEEN 2 AND 4 FROM t1 ORDER BY a",
        "SELECT RAISE(IGNORE)", "SELECT 1 +", "SELECT ((((((((((1))))))))))"])],
    "factored-select-stmt": [([], [
        "WITH x AS (SELECT a FROM t1) SELECT a FROM x UNION SELECT a FROM t2 ORDER BY 1 LIMIT 3", "WITH x AS (SELECT 1 AS v) SELECT v FROM x ORDER BY v LIMIT 1 OFFSET 0",
        "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r WHERE n < 3) SELECT n FROM r UNION ALL SELECT n * 10 FROM r ORDER BY 1", "WITH a AS (SELECT 1), b AS (SELECT 2) SELECT * FROM a, b",
        "WITH x AS (SELECT a, b FROM t2) SELECT b, count(*) FROM x GROUP BY b ORDER BY b", "WITH x AS (SELECT a FROM t1) SELECT a FROM x ORDER BY a DESC LIMIT 2",
        "WITH x AS (SELECT a FROM t1) SELECT a FROM x INTERSECT SELECT a FROM t2", "WITH x AS (SELECT a FROM t1) SELECT * FROM (SELECT a FROM x WHERE a > 4) ORDER BY 1",
        "WITH x AS (SELECT 2 AS a) SELECT t1.b FROM t1, x WHERE t1.a = x.a", "WITH x AS (SELECT a FROM t1) VALUES ((SELECT count(*) FROM x))",
        "WITH x AS (SELECT a FROM t1 ORDER BY a LIMIT 2) SELECT sum(a) FROM x", "WITH x AS (SELECT a FROM t1) SELECT a FROM x ORDER BY a LIMIT 2, 3"])],
    "filter-clause": [([], [
        "SELECT count(*) FILTER (WHERE a > 2) FROM t1", "SELECT sum(c) FILTER (WHERE c > 0) FROM t1", "SELECT a % 2, count(*) FILTER (WHERE b IS NOT NULL) FROM t1 GROUP BY 1 ORDER BY 1",
        "SELECT group_concat(b, ',') FILTER (WHERE length(b) > 3) FROM t1", "SELECT avg(a) FILTER (WHERE a <> 3), avg(a) FROM t1", "SELECT max(c) FILTER (WHERE a < 3) FROM t1",
        "SELECT a, sum(a) FILTER (WHERE a % 2 = 1) OVER (ORDER BY a) FROM t1 ORDER BY a", "SELECT count(*) FILTER (WHERE 0) FROM t1", "SELECT total(c) FILTER (WHERE b LIKE 't%') FROM t1",
        "SELECT count(DISTINCT a % 3) FILTER (WHERE a > 1) FROM t1", "SELECT abs(a) FILTER (WHERE a > 1) FROM t1",
        "SELECT json_group_array(a) FILTER (WHERE a IS NOT NULL) FROM (SELECT a FROM t1 ORDER BY a)"])],
    "foreign-key-clause": [(["PRAGMA foreign_keys = ON", "CREATE TABLE fp(k INTEGER PRIMARY KEY, u TEXT UNIQUE)", "INSERT INTO fp VALUES (1, 'a'), (2, 'b'), (3, 'c')"], [
        "CREATE TABLE fc1(r INTEGER REFERENCES fp(k) ON DELETE CASCADE ON UPDATE CASCADE)", "CREATE TABLE fc2(r INTEGER REFERENCES fp ON DELETE SET NULL)",
        "CREATE TABLE fc3(r TEXT DEFAULT 'a' REFERENCES fp(u) ON DELETE SET DEFAULT ON UPDATE RESTRICT)", "CREATE TABLE fc4(r INTEGER REFERENCES fp(k) ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED)",
        "INSERT INTO fc1 VALUES (1), (2)", "INSERT INTO fc2 VALUES (2)", "INSERT INTO fc3 VALUES ('c')", "INSERT INTO fc4 VALUES (3)", "INSERT INTO fc1 VALUES (9)", "DELETE FROM fp WHERE k = 2",
        "SELECT r FROM fc1 ORDER BY r", "SELECT r FROM fc2", "UPDATE fp SET k = 10 WHERE k = 1", "SELECT r FROM fc1", "UPDATE fp SET u = 'z' WHERE k = 3", "DELETE FROM fc4", "DELETE FROM fp WHERE k = 3",
        "SELECT r FROM fc3", "BEGIN", "INSERT INTO fc4 VALUES (42)", "INSERT INTO fp VALUES (42, 'q')", "COMMIT", "SELECT r FROM fc4 ORDER BY r",
        "CREATE TABLE fc5(a, b, FOREIGN KEY (a, b) REFERENCES fp(k, u) MATCH SIMPLE NOT DEFERRABLE INITIALLY IMMEDIATE)", "SELECT \"table\", \"from\", \"to\", on_update, on_delete, match FROM pragma_foreign_key_list('fc5') ORDER BY seq",
        "INSERT INTO fc5 VALUES (1, 'a')"])],
    "function-arguments": [([], [
        "SELECT count(*) FROM t1", "SELECT count(DISTINCT b) FROM t1", "SELECT group_concat(b, ';' ORDER BY a DESC) FROM t1", "SELECT max(1, 2, 3)", "SELECT coalesce(NULL, NULL, 'third')",
        "SELECT substr('abcdef', 2, 3)", "SELECT printf('%s-%d', 'x', 5)", "SELECT sum(ALL a) FROM t1", "SELECT count(ALL b) FROM t1", "SELECT string_agg(DISTINCT b, ',' ORDER BY b) FROM t2",
        "SELECT json_object('a', 1, 'b', 2)", "SELECT char(72, 105)", "SELECT abs()", "SELECT count(DISTINCT a, b) FROM t1"])],
    "indexed-column": [([], [
        "CREATE INDEX ix1 ON t1(a ASC)", "CREATE INDEX ix2 ON t1(b DESC, a ASC)", "CREATE INDEX ix3 ON t1(b COLLATE NOCASE)", "CREATE INDEX ix4 ON t1(b COLLATE RTRIM DESC)", "CREATE INDEX ix5 ON t1(a * 2)",
        "CREATE INDEX ix6 ON t1(substr(b, 1, 1) COLLATE NOCASE)", "CREATE UNIQUE INDEX ix7 ON t2(b COLLATE BINARY)", "CREATE TABLE ixt(k TEXT, v, PRIMARY KEY (k COLLATE NOCASE DESC))",
        "INSERT INTO ixt VALUES ('a', 1)", "INSERT INTO ixt VALUES ('A', 2)", "INSERT INTO t2(a, b, c) VALUES (30, 'x', 0) ON CONFLICT (b) DO NOTHING",
        "INSERT INTO t2(a, b, c) VALUES (30, 'x', 0) ON CONFLICT (b COLLATE BINARY) DO NOTHING", "SELECT name FROM pragma_index_xinfo('ix2') WHERE key ORDER BY seqno",
        "SELECT \"desc\", coll FROM pragma_index_xinfo('ix4') WHERE key", "SELECT count(*) FROM t2 WHERE a = 30"])],
    "insert-stmt": [([], [
        "INSERT INTO t2 VALUES (40, 'v', 4)", "INSERT INTO t2(b, c) VALUES ('auto', 5)", "INSERT INTO t2 DEFAULT VALUES", "INSERT INTO t2 SELECT a + 100, b, a FROM t1 WHERE a IS NOT NULL",
        "SELECT count(*), max(a) FROM t2", "INSERT INTO t2(a, b) VALUES (50, 'm'), (51, 'n') RETURNING a, b", "REPLACE INTO t2 VALUES (40, 'replaced', 0)", "INSERT OR REPLACE INTO t2(a, b) VALUES (51, 'again')",
        "WITH v(x) AS (VALUES (60), (61)) INSERT INTO t2(a) SELECT x FROM v", "INSERT INTO main.t2 AS t(a, b) VALUES (70, 'alias')", "INSERT INTO t2 VALUES (1, 'dup', 0)", "INSERT INTO t2(a, b) VALUES (80)",
        "INSERT INTO t2(nope) VALUES (1)", "SELECT a, b, c FROM t2 WHERE a >= 40 ORDER BY a", "INSERT INTO t2 VALUES (90, 'x', 1) ON CONFLICT DO NOTHING"])],
    "join-clause": [([], [
        "SELECT t1.a, t2.b FROM t1 JOIN t2 ON t1.a = t2.a ORDER BY 1", "SELECT t1.a, t2.b FROM t1, t2 WHERE t1.a = t2.a ORDER BY 1", "SELECT t1.a, t2.b FROM t1 LEFT JOIN t2 ON t1.a = t2.a ORDER BY 1",
        "SELECT t1.a, t2.a FROM t1 RIGHT JOIN t2 ON t1.a = t2.a ORDER BY 2", "SELECT t1.a, t2.a FROM t1 FULL JOIN t2 ON t1.a = t2.a ORDER BY coalesce(t1.a, t2.a)", "SELECT count(*) FROM t1 CROSS JOIN t2",
        "SELECT a, t2.b FROM t1 JOIN t2 USING (a) ORDER BY a", "SELECT a, b FROM t1 NATURAL JOIN t2 ORDER BY a", "SELECT x.a, y.a FROM t1 AS x JOIN t1 AS y ON y.a = x.a + 1 ORDER BY 1",
        "SELECT t1.a FROM t1 JOIN t2 ON t1.a = t2.a JOIN t1 AS t3 ON t3.a = t2.a ORDER BY 1", "SELECT count(*) FROM t1 INNER JOIN t2 ON 1",
        "SELECT t1.a, t2.c FROM t1 LEFT OUTER JOIN t2 ON t1.a = t2.a AND t2.c > 10 ORDER BY 1", "SELECT * FROM t1 JOIN t2 USING (nope)", "SELECT t1.a, t2.a FROM t1 RIGHT OUTER JOIN t2 USING (a) ORDER BY 2",
        "SELECT t1.a, t2.a FROM t1 FULL OUTER JOIN t2 USING (a) WHERE t1.a IS NULL ORDER BY 2", "SELECT a FROM t1 NATURAL LEFT JOIN t2 ORDER BY a", "SELECT count(*) FROM t1 JOIN t2"])],
    "literal-value": [([], [
        "SELECT 1, 1.5, 'text', x'0A', NULL", "SELECT TRUE, FALSE", "SELECT typeof(CURRENT_DATE), length(CURRENT_DATE)", "SELECT typeof(CURRENT_TIME), length(CURRENT_TIME)",
        "SELECT typeof(CURRENT_TIMESTAMP), length(CURRENT_TIMESTAMP)", "SELECT 'it''s'", "SELECT X'ABCDEF'", "SELECT ''", "SELECT x''", "SELECT -0.0, +5", "SELECT 'multi\nline'", "SELECT x'abc'"])],
    "numeric-literal": [([], [
        "SELECT 1", "SELECT 1.5", "SELECT .5", "SELECT 5.", "SELECT 1e3", "SELECT 1E-3", "SELECT 0x1F", "SELECT 0XFF", "SELECT 1_000_000", "SELECT 0x7FFFFFFFFFFFFFFF", "SELECT 9223372036854775808",
        "SELECT -9223372036854775808", "SELECT 0xFFFFFFFFFFFFFFFF", "SELECT 1.5e+2", "SELECT 0x", "SELECT 1__0", "SELECT 1e", "SELECT 0x1_F"])],
    "ordering-term": [([], [
        "SELECT a FROM t1 ORDER BY a", "SELECT a FROM t1 ORDER BY a DESC", "SELECT a FROM t1 ORDER BY a NULLS LAST", "SELECT a FROM t1 ORDER BY a DESC NULLS FIRST",
        "SELECT b FROM t1 ORDER BY b COLLATE NOCASE, a", "SELECT a, b FROM t1 ORDER BY 2, 1", "SELECT a FROM t1 ORDER BY -a", "SELECT a AS x FROM t1 ORDER BY x", "SELECT a FROM t1 ORDER BY c IS NULL, c",
        "SELECT b FROM t2 ORDER BY b, a", "SELECT b FROM t2 ORDER BY b COLLATE BINARY, a", "SELECT a FROM t1 ORDER BY 3", "SELECT a FROM t1 ORDER BY (SELECT 1), a",
        "SELECT a FROM t1 ORDER BY a ASC NULLS FIRST LIMIT 3"])],
    "over-clause": [([], [
        "SELECT a, row_number() OVER (ORDER BY a) FROM t1 ORDER BY a", "SELECT a, rank() OVER w FROM t1 WINDOW w AS (ORDER BY a % 3) ORDER BY a",
        "SELECT a, sum(a) OVER (w ROWS 1 PRECEDING) FROM t1 WINDOW w AS (ORDER BY a) ORDER BY a", "SELECT a, count(*) OVER () FROM t1 ORDER BY a",
        "SELECT a, max(c) OVER (PARTITION BY a % 2) FROM t1 ORDER BY a", "SELECT a, dense_rank() OVER (PARTITION BY a % 2 ORDER BY a DESC) FROM t1 ORDER BY a",
        "SELECT a, avg(c) OVER (ORDER BY a ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) FROM t1 ORDER BY a", "SELECT a, lag(a) OVER win FROM t1 WINDOW win AS (ORDER BY a) ORDER BY a",
        "SELECT a, sum(a) FILTER (WHERE a > 2) OVER (ORDER BY a) FROM t1 ORDER BY a", "SELECT a, count(*) OVER nope FROM t1", "SELECT a, sum(a) OVER (w ORDER BY a) FROM t1 WINDOW w AS (ORDER BY a)",
        "SELECT a, percent_rank() OVER (ORDER BY a) FROM t1 ORDER BY a"])],
    "qualified-table-name": [(["CREATE INDEX q1 ON t1(a)"], [
        "UPDATE t1 INDEXED BY q1 SET b = 'q' WHERE a = 1", "DELETE FROM t1 NOT INDEXED WHERE a = 6", "UPDATE main.t1 AS x SET c = x.c * 2 WHERE x.a = 2",
        "DELETE FROM main.t1 AS x INDEXED BY q1 WHERE x.a = 3", "SELECT a, b, c FROM t1 ORDER BY a", "UPDATE t1 INDEXED BY nope SET b = 'z' WHERE a = 1",
        "SELECT a FROM t1 INDEXED BY q1 WHERE a > 1 ORDER BY a", "SELECT a FROM t1 NOT INDEXED WHERE a > 1 ORDER BY a", "SELECT a FROM t1 INDEXED BY q1 WHERE b = 'two'",
        "SELECT a FROM main.t1 AS m INDEXED BY q1 WHERE m.a = 4", "UPDATE t1 NOT INDEXED SET a = a WHERE a = 4", "DELETE FROM t1 INDEXED BY q1"])],
    "reindex-stmt": [(["CREATE INDEX r1 ON t1(b COLLATE NOCASE)", "CREATE INDEX r2 ON t2(c)"], [
        "REINDEX", "REINDEX t1", "REINDEX r2", "REINDEX main.t2", "REINDEX main.r1", "REINDEX NOCASE", "REINDEX BINARY", "REINDEX RTRIM", "REINDEX nope", "PRAGMA integrity_check", "REINDEX temp.t1",
        "REINDEX \"t2\""])],
    "release-stmt": [([], [
        "SAVEPOINT s1", "INSERT INTO t2 VALUES (100, 's1', 1)", "SAVEPOINT s2", "INSERT INTO t2 VALUES (101, 's2', 2)", "RELEASE s2", "SELECT count(*) FROM t2 WHERE a >= 100", "RELEASE SAVEPOINT s1",
        "SELECT count(*) FROM t2 WHERE a >= 100", "RELEASE s1", "SAVEPOINT \"quoted\"", "RELEASE \"quoted\"", "SAVEPOINT a", "SAVEPOINT b", "RELEASE a", "RELEASE b"])],
    "result-column": [([], [
        "SELECT * FROM t2 ORDER BY a", "SELECT t1.* FROM t1 WHERE a = 1", "SELECT a AS x, b y FROM t1 WHERE a = 2", "SELECT a + 1 FROM t1 WHERE a = 3", "SELECT t1.*, t2.c FROM t1 JOIN t2 USING (a) ORDER BY 1",
        "SELECT *, a * 2 FROM t2 WHERE a = 1", "SELECT \"a\" FROM t1 WHERE a = 4", "SELECT [b] FROM t1 WHERE a = 5", "SELECT nope.* FROM t1", "SELECT *", "SELECT 1 AS \"one col\"",
        "SELECT a 'alias' FROM t1 WHERE a = 6", "SELECT main.t2.* FROM t2 WHERE a = 2"])],
    "rollback-stmt": [([], [
        "BEGIN", "INSERT INTO t2 VALUES (200, 'r', 0)", "ROLLBACK", "SELECT count(*) FROM t2 WHERE a = 200", "BEGIN", "SAVEPOINT sp", "INSERT INTO t2 VALUES (201, 'r', 0)", "ROLLBACK TO sp",
        "SELECT count(*) FROM t2 WHERE a = 201", "ROLLBACK TRANSACTION TO SAVEPOINT sp", "COMMIT", "ROLLBACK", "ROLLBACK TO nope", "BEGIN", "ROLLBACK TRANSACTION"])],
    "savepoint-stmt": [([], [
        "SAVEPOINT a1", "SAVEPOINT a2", "INSERT INTO t2 VALUES (300, 'sv', 3)", "ROLLBACK TO a2", "SELECT count(*) FROM t2 WHERE a = 300", "RELEASE a1", "SAVEPOINT a1", "SAVEPOINT a1",
        "INSERT INTO t2 VALUES (301, 'sv', 3)", "RELEASE a1", "SELECT count(*) FROM t2 WHERE a = 301", "ROLLBACK TO a1", "RELEASE a1", "SELECT count(*) FROM t2 WHERE a = 301"])],
    "select-core": [([], [
        "SELECT a FROM t1 WHERE a > 1 GROUP BY a HAVING count(*) = 1 ORDER BY a", "SELECT DISTINCT a % 2 FROM t1", "SELECT ALL a % 2 FROM t1", "VALUES (1, 'a'), (2, 'b')",
        "SELECT a % 3 AS g, count(*) FROM t1 GROUP BY g HAVING count(*) > 1", "SELECT a FROM t1 WINDOW w AS (ORDER BY a)", "SELECT a, sum(a) OVER w FROM t1 WINDOW w AS (ORDER BY a) ORDER BY a",
        "SELECT count(*) FROM t1 HAVING count(*) > 3", "SELECT 1 WHERE 0", "SELECT DISTINCT b FROM t2", "SELECT a FROM t1 GROUP BY a HAVING a > 4", "VALUES (1), ('two')", "VALUES (1, 2), (3)",
        "SELECT a FROM t1 HAVING 1"])],
    "select-stmt": [([], [
        "SELECT 1", "SELECT a, b FROM t1 WHERE a < 3 ORDER BY a", "SELECT DISTINCT b FROM t2 ORDER BY 1", "SELECT a % 2, count(*) FROM t1 GROUP BY 1 ORDER BY 1", "SELECT * FROM t1 LIMIT 2 OFFSET 2",
        "SELECT a FROM t1 ORDER BY a LIMIT 3", "SELECT count(*) FROM t1 WHERE b IS NOT NULL", "VALUES (1, 2)", "SELECT a FROM t1 UNION SELECT a FROM t2 ORDER BY 1",
        "WITH x AS (SELECT 1 AS a) SELECT a FROM x", "SELECT (SELECT count(*) FROM t2) AS n", "SELECT a FROM t1 WHERE a IN (SELECT a FROM t2) ORDER BY a", "SELECT FROM t1"])],
    "signed-number": [([], [
        "CREATE TABLE sn1(a DECIMAL(10, -2), b VARCHAR(+20), c NUMERIC(5))", "CREATE TABLE sn2(a INTEGER DEFAULT -1, b REAL DEFAULT +2.5, c DEFAULT -0x10)", "INSERT INTO sn2 DEFAULT VALUES",
        "SELECT a, b, c FROM sn2", "PRAGMA cache_size = -500", "PRAGMA cache_size", "PRAGMA user_version = -3", "PRAGMA user_version", "PRAGMA user_version = +4", "PRAGMA user_version",
        "SELECT type FROM pragma_table_info('sn1') ORDER BY cid", "CREATE TABLE sn3(a DEFAULT - 'x')"])],
    "simple-function-invocation": [([], [
        "SELECT abs(-4)", "SELECT length('four')", "SELECT upper(b) FROM t1 WHERE a = 1", "SELECT typeof(random())", "SELECT changes() >= 0", "SELECT coalesce(NULL, 2)", "SELECT round(2.567, 2)",
        "SELECT nullif(a, 2) FROM t1 ORDER BY a", "SELECT instr(b, 'e') FROM t1 ORDER BY a", "SELECT hex(d) FROM t1 ORDER BY a", "SELECT nope(1)", "SELECT ltrim('  x')"])],
    "sql-stmt": [([], [
        "EXPLAIN SELECT 1", "EXPLAIN QUERY PLAN SELECT a FROM t1 WHERE a = 1", "EXPLAIN INSERT INTO t2 VALUES (1, 'x', 1)", "EXPLAIN QUERY PLAN SELECT * FROM t1 JOIN t2 USING (a)",
        "EXPLAIN UPDATE t1 SET a = 1", "EXPLAIN QUERY PLAN DELETE FROM t2 WHERE c > 1", "EXPLAIN CREATE TABLE x(a)", "EXPLAIN QUERY PLAN WITH c AS (SELECT 1) SELECT * FROM c", "EXPLAIN DROP TABLE t1",
        "EXPLAIN QUERY PLAN SELECT count(*) FROM t1 GROUP BY b", "EXPLAIN PRAGMA user_version", "EXPLAIN QUERY PLAN VALUES (1)", "EXPLAIN EXPLAIN SELECT 1", "SELECT count(*) FROM t1"])],
    "sql-stmt-list": [([], [
        "CREATE TABLE l1(a); INSERT INTO l1 VALUES (1); INSERT INTO l1 VALUES (2)", "SELECT count(*) FROM l1", "UPDATE l1 SET a = a + 1; DELETE FROM l1 WHERE a = 3", "SELECT a FROM l1",
        "BEGIN; INSERT INTO l1 VALUES (5); COMMIT", "SELECT sum(a) FROM l1", "SELECT 1; SELECT 2", "CREATE TABLE l2(b); DROP TABLE l2", "SAVEPOINT x; INSERT INTO l1 VALUES (9); ROLLBACK TO x; RELEASE x",
        "SELECT count(*) FROM l1", "INSERT INTO l1 VALUES (7); INSERT INTO nope VALUES (1)", "SELECT count(*) FROM l1", "CREATE TABLE l3(c); CREATE TABLE l3(c)",
        "SELECT count(*) FROM sqlite_schema WHERE name = 'l3'"])],
    "table-constraint": [([], [
        "CREATE TABLE tc1(a, b, PRIMARY KEY (a, b))", "CREATE TABLE tc2(a, b, UNIQUE (a), CHECK (b > 0))", "CREATE TABLE tc3(a, b, CONSTRAINT pk PRIMARY KEY (a DESC) ON CONFLICT REPLACE)",
        "CREATE TABLE tc4(a, b, FOREIGN KEY (a) REFERENCES tc1(a), CONSTRAINT u UNIQUE (b COLLATE NOCASE) ON CONFLICT IGNORE)", "INSERT INTO tc1 VALUES (1, 1), (1, 2)", "INSERT INTO tc1 VALUES (1, 1)",
        "INSERT INTO tc2 VALUES (1, 0)", "INSERT INTO tc3 VALUES (1, 'x'), (1, 'y')", "SELECT * FROM tc3", "INSERT INTO tc4 VALUES (1, 'q'), (2, 'Q')", "SELECT * FROM tc4",
        "CREATE TABLE tc5(a, PRIMARY KEY (a), PRIMARY KEY (a))", "CREATE TABLE tc6(a INTEGER, b, PRIMARY KEY (a AUTOINCREMENT))", "CREATE TABLE tc7(a, CHECK (a IN (1, 2)) CHECK (a > 0))",
        "INSERT INTO tc7 VALUES (3)", "INSERT INTO tc7 VALUES (2)", "SELECT a FROM tc7"])],
    "table-options": [([], [
        "CREATE TABLE to1(k INTEGER PRIMARY KEY, v) WITHOUT ROWID", "CREATE TABLE to2(k INTEGER, v TEXT) STRICT", "CREATE TABLE to3(k TEXT PRIMARY KEY, v INT) STRICT, WITHOUT ROWID",
        "INSERT INTO to2 VALUES (1, 'a')", "INSERT INTO to2 VALUES ('x', 'a')", "INSERT INTO to3 VALUES ('k', '5')", "SELECT k, v, typeof(v) FROM to3", "CREATE TABLE to4(a) STRICT",
        "CREATE TABLE to5(a INT) WITHOUT ROWID", "CREATE TABLE to6(a INT PRIMARY KEY) WITHOUT ROWID, WITHOUT ROWID", "SELECT name, wr, strict FROM pragma_table_list WHERE name LIKE 'to%' ORDER BY name",
        "CREATE TABLE to7(a INTEGER PRIMARY KEY) STRICT, STRICT", "CREATE TABLE to8(a) WITHOUT nothing"])],
    "table-or-subquery": [(["CREATE INDEX ts1 ON t1(a)"], [
        "SELECT a FROM t1 WHERE a = 1", "SELECT x.a FROM main.t1 AS x WHERE x.a = 2", "SELECT s.n FROM (SELECT count(*) AS n FROM t1) AS s", "SELECT * FROM (t1 JOIN t2 USING (a)) WHERE a < 3 ORDER BY a",
        "SELECT value FROM json_each('[1, 2, 3]') ORDER BY value", "SELECT a FROM t1 INDEXED BY ts1 WHERE a > 4 ORDER BY a", "SELECT a FROM t1 NOT INDEXED WHERE a > 4 ORDER BY a",
        "SELECT key FROM json_each('{\"k\": 1}') AS j", "SELECT * FROM (SELECT 1 AS p), (SELECT 2 AS q)", "SELECT t.a FROM (SELECT a FROM t1 ORDER BY a LIMIT 2) t ORDER BY 1",
        "SELECT * FROM (VALUES (1, 2), (3, 4))", "SELECT * FROM nope", "SELECT name FROM pragma_table_info('t1') ORDER BY cid", "SELECT count(*) FROM main.sqlite_schema"])],
    "type-name": [([], [
        "CREATE TABLE tn(a INT, b INTEGER, c TINYINT, d SMALLINT, e MEDIUMINT, f BIGINT, g UNSIGNED BIG INT, h INT2, i INT8, j CHARACTER(20), k VARCHAR(255), l VARYING CHARACTER(255), m NCHAR(55),"
        " n NATIVE CHARACTER(70), o NVARCHAR(100), p TEXT, q CLOB, r BLOB, s REAL, t DOUBLE, u DOUBLE PRECISION, v FLOAT, w NUMERIC, x DECIMAL(10, 5), y BOOLEAN, z DATE, aa DATETIME)",
        "SELECT name, type FROM pragma_table_info('tn') ORDER BY cid", "INSERT INTO tn(a, j, s, w, y, z) VALUES ('1', 2, '3.5', '4.0', 'true', '2024-01-01')",
        "SELECT typeof(a), typeof(j), typeof(s), typeof(w), typeof(y), typeof(z) FROM tn", "CREATE TABLE tn2(a \"quoted type\", b [weird type](1, 2))", "SELECT type FROM pragma_table_info('tn2') ORDER BY cid",
        "CREATE TABLE tn3(a VARCHAR(-1))", "CREATE TABLE tn4(a INT(11) UNSIGNED ZEROFILL)", "SELECT type FROM pragma_table_info('tn4')", "CREATE TABLE tn5(a TEXT(10, 20, 30))",
        "SELECT CAST(1 AS VARCHAR(10))", "SELECT CAST('12' AS DECIMAL(4, 1))"])],
    "update-stmt-limited": [([], [
        "UPDATE t1 SET b = 'x' ORDER BY a LIMIT 1", "UPDATE t1 SET b = 'y' WHERE a > 2 LIMIT 2", "UPDATE t1 SET b = 'z' LIMIT 1 OFFSET 1", "UPDATE t2 SET c = 0 ORDER BY c DESC LIMIT 1",
        "UPDATE t1 SET c = 0 WHERE b IS NULL ORDER BY a LIMIT 1", "UPDATE t2 SET c = 1 LIMIT 0", "UPDATE t2 SET c = 2 ORDER BY a", "UPDATE t1 SET a = a WHERE a = 1 LIMIT -1",
        "UPDATE t2 SET b = 'q' WHERE a > 1 ORDER BY b LIMIT 1 OFFSET 1", "UPDATE t1 SET e = 0 LIMIT 2, 1", "SELECT a, b FROM t1 ORDER BY a", "SELECT a, c FROM t2 ORDER BY a"])],
    "update-stmt": [([], [
        "UPDATE t1 SET b = 'u' WHERE a = 1", "UPDATE t1 SET b = upper(b), c = c + 1 WHERE a > 4", "UPDATE t2 SET c = t1.a FROM t1 WHERE t1.a = t2.a AND t1.a < 3", "SELECT a, c FROM t2 ORDER BY a",
        "UPDATE t1 SET c = 0 WHERE c IS NULL RETURNING a, c", "UPDATE OR IGNORE t2 SET a = 1 WHERE a = 2", "WITH x AS (SELECT 6 AS a) UPDATE t2 SET b = 'six' WHERE a IN x",
        "UPDATE t2 AS z SET c = z.c + 1 WHERE z.a = 7", "UPDATE t1 SET a = a + 10", "SELECT a FROM t1 ORDER BY a", "UPDATE t1 SET nope = 1", "UPDATE t2 SET c = (SELECT max(a) FROM t1) WHERE a = 1",
        "UPDATE t2 SET c = x.m FROM (SELECT max(a) AS m FROM t1) AS x WHERE t2.a = 3", "SELECT a, b, c FROM t2 ORDER BY a", "UPDATE t2 SET a = NULL WHERE a = 3"])],
    "vacuum-stmt": [([], [
        "VACUUM", "VACUUM main", "SELECT count(*) FROM t1", "VACUUM temp", "BEGIN", "VACUUM", "ROLLBACK", "VACUUM nope", "PRAGMA auto_vacuum = INCREMENTAL", "VACUUM", "PRAGMA auto_vacuum",
        "VACUUM \"main\"", "PRAGMA integrity_check"])],
    "with-clause": [([], [
        "WITH x AS (SELECT 1 AS a) SELECT a FROM x", "WITH RECURSIVE x(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM x WHERE n < 3) SELECT n FROM x",
        "WITH x AS (SELECT a FROM t1), y AS (SELECT a FROM t2) SELECT count(*) FROM x JOIN y USING (a)", "WITH x AS (SELECT 2 AS k) UPDATE t2 SET c = -1 WHERE a = (SELECT k FROM x)",
        "WITH x AS (SELECT 3 AS k) DELETE FROM t2 WHERE a = (SELECT k FROM x)", "WITH x(k, v) AS (VALUES (8, 'w')) INSERT INTO t2(a, b) SELECT k, v FROM x", "SELECT a, b, c FROM t2 ORDER BY a",
        "WITH x AS (SELECT 1) SELECT (WITH y AS (SELECT 2) SELECT * FROM y)", "WITH x AS (SELECT * FROM x) SELECT * FROM x", "WITH x AS (SELECT 1) INSERT INTO nope SELECT * FROM x",
        "WITH RECURSIVE a(n) AS (SELECT 1), b(m) AS (SELECT n FROM a) SELECT m FROM b"])],
}


def window_family():
    """Every frame unit with every bound pair and exclusion, over a rotating aggregate or value function
    and partitioning; then the forms without BETWEEN and named windows that add a frame."""
    fns = ["sum(a)", "count(*)", "first_value(b)", "last_value(b)", "nth_value(a, 2)", "group_concat(a, '.')", "avg(c)", "min(c)", "max(b)", "count(b)", "total(a)"]
    orders = ["ORDER BY a", "PARTITION BY a % 2 ORDER BY a", "ORDER BY a DESC", "ORDER BY c"]
    starts = ["UNBOUNDED PRECEDING", "1 PRECEDING", "CURRENT ROW", "1 FOLLOWING"]
    ends = ["1 PRECEDING", "CURRENT ROW", "1 FOLLOWING", "UNBOUNDED FOLLOWING"]
    excludes = ["", " EXCLUDE NO OTHERS", " EXCLUDE CURRENT ROW", " EXCLUDE GROUP", " EXCLUDE TIES"]
    cases, i = [], 0
    for unit in ("ROWS", "RANGE", "GROUPS"):
        for s in starts:
            for e in ends:
                for x in excludes:
                    fn, order = fns[i % len(fns)], orders[(i // len(fns)) % len(orders)]
                    i += 1
                    cases.append(f"SELECT a, {fn} OVER ({order} {unit} BETWEEN {s} AND {e}{x}) FROM t1 ORDER BY a")
    for unit in ("ROWS", "RANGE", "GROUPS"):
        for s in ("UNBOUNDED PRECEDING", "2 PRECEDING", "CURRENT ROW"):
            cases.append(f"SELECT a, sum(a) OVER (ORDER BY a {unit} {s}) FROM t1 ORDER BY a")
    cases += [
        "SELECT a, sum(a) OVER (w ROWS BETWEEN 1 PRECEDING AND CURRENT ROW) FROM t1 WINDOW w AS (PARTITION BY a % 2 ORDER BY a) ORDER BY a",
        "SELECT a, count(*) OVER w2 FROM t1 WINDOW w1 AS (ORDER BY a), w2 AS (w1 RANGE BETWEEN 2 PRECEDING AND CURRENT ROW) ORDER BY a",
        "SELECT a, sum(a) OVER (w ROWS 1 PRECEDING) FROM t1 WINDOW w AS (ORDER BY a ROWS CURRENT ROW)",
        "SELECT a, sum(a) OVER (ORDER BY a RANGE BETWEEN 1.5 PRECEDING AND 0.5 FOLLOWING) FROM t1 ORDER BY a",
        "SELECT a, sum(a) OVER (ORDER BY b RANGE BETWEEN 1 PRECEDING AND CURRENT ROW) FROM t1",
        "SELECT a, sum(a) OVER (ORDER BY a, c RANGE BETWEEN 1 PRECEDING AND CURRENT ROW) FROM t1",
        "SELECT a, sum(a) OVER (ROWS BETWEEN -1 PRECEDING AND CURRENT ROW) FROM t1",
        "SELECT a, sum(a) OVER (ROWS BETWEEN UNBOUNDED FOLLOWING AND CURRENT ROW) FROM t1",
        "SELECT a, sum(a) OVER (GROUPS BETWEEN 1 PRECEDING AND CURRENT ROW) FROM t1",
        "SELECT a, sum(a) OVER (ORDER BY a ROWS BETWEEN a PRECEDING AND CURRENT ROW) FROM t1",
        "SELECT a, sum(a) OVER (ORDER BY a ROWS BETWEEN (SELECT 1) PRECEDING AND CURRENT ROW) FROM t1 ORDER BY a",
        "SELECT a % 3 AS g, a, ntile(2) OVER (PARTITION BY a % 3 ORDER BY a ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING) FROM t1 ORDER BY g, a",
        "SELECT a, cume_dist() OVER (ORDER BY a % 3 GROUPS BETWEEN 1 PRECEDING AND 1 FOLLOWING EXCLUDE GROUP) FROM t1 ORDER BY a",
    ]
    return [("syntax:frame-spec", [], cases)]


def recursive_family():
    org = ["CREATE TABLE org(name TEXT PRIMARY KEY, boss TEXT REFERENCES org)",
           "INSERT INTO org VALUES ('Alice', NULL), ('Bob', 'Alice'), ('Cindy', 'Alice'), ('Dave', 'Bob'), ('Emma', 'Bob'), ('Fred', 'Cindy'), ('Gail', 'Cindy')",
           "CREATE TABLE edge(src INTEGER, dst INTEGER)", "INSERT INTO edge VALUES (1, 2), (2, 3), (3, 1), (3, 4), (5, 6)"]
    return [("syntax:recursive-cte", org, [
        "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 10) SELECT sum(n) FROM c",
        "WITH RECURSIVE c(n) AS (SELECT 1 UNION SELECT n % 5 + 1 FROM c) SELECT count(*) FROM c",
        "WITH RECURSIVE fib(a, b, i) AS (SELECT 0, 1, 0 UNION ALL SELECT b, a + b, i + 1 FROM fib WHERE i < 20) SELECT a FROM fib WHERE i = 20",
        "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c LIMIT 5) SELECT group_concat(n) FROM c",
        "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c LIMIT 3 OFFSET 2) SELECT group_concat(n) FROM c",
        "WITH RECURSIVE under(name, level) AS (VALUES ('Alice', 0) UNION ALL SELECT org.name, under.level + 1 FROM org JOIN under ON org.boss = under.name ORDER BY 2 DESC) SELECT name, level FROM under",
        "WITH RECURSIVE under(name, level) AS (VALUES ('Alice', 0) UNION ALL SELECT org.name, under.level + 1 FROM org JOIN under ON org.boss = under.name ORDER BY 2) SELECT name, level FROM under",
        "WITH RECURSIVE chain(name) AS (SELECT 'Gail' UNION ALL SELECT boss FROM org JOIN chain USING (name) WHERE boss IS NOT NULL) SELECT group_concat(name, ' < ') FROM chain",
        "WITH RECURSIVE reach(x) AS (SELECT 1 UNION SELECT dst FROM edge JOIN reach ON src = x) SELECT x FROM reach ORDER BY x",
        "WITH RECURSIVE reach(x, d) AS (SELECT 1, 0 UNION ALL SELECT dst, d + 1 FROM edge JOIN reach ON src = x WHERE d < 6) SELECT x, min(d) FROM reach GROUP BY x ORDER BY x",
        "WITH RECURSIVE r(x) AS (SELECT 1 UNION SELECT x * 2 FROM r WHERE x < 50 UNION SELECT x * 3 FROM r WHERE x < 50) SELECT count(*), max(x) FROM r",
        "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 3) SELECT a.n, b.n FROM c AS a, c AS b ORDER BY 1, 2",
        "WITH RECURSIVE c(n) AS (SELECT n FROM c) SELECT * FROM c",
        "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT max(n) + 1 FROM c WHERE n < 3) SELECT * FROM c",
        "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT c.n + 1 FROM c, c AS d WHERE c.n < 3) SELECT * FROM c",
        "WITH RECURSIVE dates(d) AS (VALUES ('2024-01-01') UNION ALL SELECT date(d, '+1 day') FROM dates WHERE d < '2024-01-10') SELECT count(*), max(d) FROM dates",
        "WITH RECURSIVE split(word, rest) AS (SELECT '', 'a,b,c,d' || ',' UNION ALL SELECT substr(rest, 1, instr(rest, ',') - 1), substr(rest, instr(rest, ',') + 1) FROM split WHERE rest <> '')"
        " SELECT word FROM split WHERE word <> ''",
        "WITH RECURSIVE c(n) AS MATERIALIZED (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 5) SELECT sum(n) FROM c",
        "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 5) INSERT INTO t2(a, b) SELECT n + 500, 'gen' FROM c",
        "SELECT count(*) FROM t2 WHERE a > 500",
        "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 1000) SELECT count(*), sum(n) FROM c",
        "WITH RECURSIVE c(n) AS (SELECT 10 UNION ALL SELECT n - 3 FROM c WHERE n > 0) SELECT n FROM c",
        "WITH RECURSIVE r(i, s) AS (SELECT 1, 'x' UNION ALL SELECT i + 1, s || 'x' FROM r WHERE i < 5) SELECT s FROM r ORDER BY i DESC LIMIT 1",
        "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 5 ORDER BY n DESC LIMIT 3) SELECT group_concat(n) FROM c",
        "WITH RECURSIVE x(i) AS (VALUES (1) UNION ALL SELECT i + 1 FROM x WHERE i < 30), m(i, v) AS (SELECT i, i * i FROM x) SELECT sum(v) FROM m WHERE i % 3 = 0",
        "WITH RECURSIVE t(n, f) AS (SELECT 1, 1 UNION ALL SELECT n + 1, f * (n + 1) FROM t WHERE n < 15) SELECT f FROM t WHERE n = 15",
        "WITH RECURSIVE gcd(a, b) AS (SELECT 1071, 462 UNION ALL SELECT b, a % b FROM gcd WHERE b > 0) SELECT a FROM gcd WHERE b = 0",
        "WITH RECURSIVE sub(name, depth) AS (SELECT name, 0 FROM org WHERE boss IS NULL UNION ALL SELECT o.name, s.depth + 1 FROM org AS o JOIN sub AS s ON o.boss = s.name)"
        " SELECT depth, count(*) FROM sub GROUP BY depth ORDER BY depth",
    ])]


def upsert_family():
    kv = ["CREATE TABLE kv(k TEXT PRIMARY KEY, v INTEGER, n INTEGER DEFAULT 0, u TEXT UNIQUE)", "INSERT INTO kv VALUES ('a', 1, 0, 'ua'), ('b', 2, 0, 'ub')"]
    return [("syntax:upsert-clause", kv, [
        "INSERT INTO kv(k, v) VALUES ('a', 10) ON CONFLICT (k) DO UPDATE SET v = excluded.v, n = n + 1",
        "INSERT INTO kv(k, v) VALUES ('a', 20) ON CONFLICT (k) DO UPDATE SET v = excluded.v WHERE excluded.v > kv.v RETURNING k, v, n",
        "INSERT INTO kv(k, v) VALUES ('a', 5) ON CONFLICT (k) DO UPDATE SET v = excluded.v WHERE excluded.v > kv.v RETURNING k, v",
        "INSERT INTO kv(k, v) VALUES ('c', 3) ON CONFLICT (k) DO NOTHING RETURNING *",
        "INSERT INTO kv(k, v) VALUES ('c', 33) ON CONFLICT DO NOTHING RETURNING *",
        "INSERT INTO kv(k, v, u) VALUES ('d', 4, 'ua') ON CONFLICT (k) DO UPDATE SET v = 0",
        "INSERT INTO kv(k, v, u) VALUES ('d', 4, 'ua') ON CONFLICT (u) DO UPDATE SET v = excluded.v + 100 RETURNING k, v, u",
        "INSERT INTO kv(k, v, u) VALUES ('b', 7, 'ub') ON CONFLICT (k) DO UPDATE SET v = -1 ON CONFLICT (u) DO NOTHING RETURNING k, v",
        "INSERT INTO kv(k, v) VALUES ('e', 1), ('e', 2), ('e', 3) ON CONFLICT (k) DO UPDATE SET v = v + excluded.v RETURNING k, v",
        "INSERT INTO kv(k, v) SELECT b, a FROM t1 WHERE b IS NOT NULL ON CONFLICT (k) DO UPDATE SET n = n + 1",
        "INSERT INTO kv(k, v) SELECT b, a FROM t1 WHERE true ON CONFLICT (k) DO UPDATE SET n = n + 1",
        "INSERT INTO kv(k, v) VALUES ('a', 1) ON CONFLICT (v) DO NOTHING",
        "INSERT INTO kv AS t(k, v) VALUES ('a', 50) ON CONFLICT (k) DO UPDATE SET v = t.v + excluded.v RETURNING t.k, t.v",
        "INSERT INTO kv(k, v) VALUES ('a', 1) ON CONFLICT (k) DO UPDATE SET k = 'b'",
        "INSERT INTO kv(k, v) VALUES ('h', 8) ON CONFLICT (k) DO UPDATE SET v = 0 RETURNING excluded.v",
        "INSERT INTO t2(a, b, c) VALUES (2, 'two', 0) ON CONFLICT (a) DO UPDATE SET c = excluded.c * 10 WHERE excluded.b = 'two' RETURNING a, b, c",
        "INSERT INTO t2(b, c) VALUES ('new', 1) ON CONFLICT DO UPDATE SET c = 99 RETURNING a > 0, b, c",
        "WITH src(k, v) AS (VALUES ('a', 100), ('z', 26)) INSERT INTO kv(k, v) SELECT k, v FROM src WHERE 1 ON CONFLICT (k) DO UPDATE SET v = excluded.v RETURNING k, v",
        "INSERT INTO kv(k, v) VALUES ('a', 1) ON CONFLICT (k) WHERE v > 0 DO NOTHING",
        "INSERT INTO kv(k, v) VALUES ('q', 1) ON CONFLICT DO UPDATE SET v = 2 ON CONFLICT (k) DO NOTHING",
        "SELECT k, v, n, u FROM kv ORDER BY k",
    ]), ("syntax:returning-clause", kv, [
        "INSERT INTO kv(k, v) VALUES ('f', 6) RETURNING k, v, n, u, rowid",
        "INSERT INTO kv(k, v) VALUES ('g', 7) RETURNING upper(k) AS big, v * 2 AS double, (SELECT count(*) FROM kv) AS cnt",
        "UPDATE kv SET v = v + 1 WHERE k IN ('a', 'b') RETURNING k, v",
        "UPDATE kv SET v = 0 WHERE k = 'zzz' RETURNING *",
        "DELETE FROM kv WHERE k = 'g' RETURNING k, v",
        "DELETE FROM kv WHERE v < 0 RETURNING *",
        "REPLACE INTO kv(k, v) VALUES ('a', 0) RETURNING k, v, n",
        "INSERT OR IGNORE INTO kv(k, v) VALUES ('a', 9) RETURNING k",
        "UPDATE kv SET v = 1 RETURNING count(*)",
        "INSERT INTO kv(k, v) VALUES ('i', 9) RETURNING *, k || '!'",
        "UPDATE kv SET n = n + 10 WHERE v IS NOT NULL RETURNING k, n - 10 AS was",
        "DELETE FROM kv WHERE k = 'i' RETURNING k AS gone",
        "INSERT INTO kv(k) SELECT 'x' || a FROM t1 WHERE a < 3 RETURNING k",
        "UPDATE kv SET v = v * 2 FROM t1 WHERE t1.a = kv.v RETURNING kv.k, kv.v",
        "INSERT INTO kv(k, v) VALUES ('j', 1) RETURNING nope",
        "DELETE FROM kv RETURNING (SELECT max(k) FROM kv)",
        "SELECT k, v, n FROM kv ORDER BY k",
    ])]


def generated_family():
    gc = ["CREATE TABLE gc(a INTEGER, b TEXT, c INTEGER GENERATED ALWAYS AS (a * 10) VIRTUAL, d TEXT AS (upper(b)) STORED, e AS (a + length(b)), f INTEGER AS (c + 1) STORED)"]
    return [("keyword:GENERATED", gc, [
        "INSERT INTO gc(a, b) VALUES (1, 'x'), (2, 'yy'), (NULL, 'z'), (3, NULL)", "SELECT a, b, c, d, e, f FROM gc ORDER BY rowid", "UPDATE gc SET a = a + 1 WHERE a IS NOT NULL",
        "SELECT a, c, f FROM gc ORDER BY rowid", "INSERT INTO gc(a, b, c) VALUES (1, 'q', 5)", "UPDATE gc SET d = 'no'", "CREATE INDEX gci ON gc(c)", "SELECT a FROM gc WHERE c = 30",
        "SELECT count(*) FROM gc WHERE d = 'YY'", "SELECT name, hidden FROM pragma_table_xinfo('gc') ORDER BY cid", "ALTER TABLE gc ADD COLUMN g INTEGER AS (a * a)",
        "ALTER TABLE gc ADD COLUMN h INTEGER AS (a * a) STORED", "SELECT g FROM gc ORDER BY rowid", "CREATE TABLE gc2(a INTEGER, b AS (b + 1))", "CREATE TABLE gc3(a INTEGER PRIMARY KEY, b AS (a))",
        "INSERT INTO gc3(a) VALUES (5)", "SELECT a, b FROM gc3", "CREATE TABLE gc4(a, b AS (random()))", "CREATE TABLE gc5(a, b INTEGER GENERATED ALWAYS AS (a) STORED NOT NULL CHECK (b > 0))",
        "INSERT INTO gc5(a) VALUES (1)", "INSERT INTO gc5(a) VALUES (0)", "INSERT INTO gc5(a) VALUES (NULL)", "SELECT a, b FROM gc5", "CREATE TABLE gc6(a, b AS (a) PRIMARY KEY)",
        "CREATE TABLE gc7(a, b AS (a) DEFAULT 1)", "SELECT * FROM gc ORDER BY rowid", "CREATE TABLE gc8(a, b AS (a) STORED UNIQUE)", "INSERT INTO gc8(a) VALUES (1)", "INSERT INTO gc8(a) VALUES (1)",
        "CREATE TABLE gc9(a, b AS (c), c AS (a + 1))", "INSERT INTO gc9(a) VALUES (1)", "SELECT a, b, c FROM gc9", "CREATE TABLE gc10(a, b AS (a) STORED) WITHOUT ROWID",
        "CREATE TABLE gc11(a INTEGER PRIMARY KEY, b AS (a * 2) STORED) WITHOUT ROWID", "INSERT INTO gc11(a) VALUES (4)", "SELECT a, b FROM gc11"])]


def strict_family():
    st = ["CREATE TABLE st(i INTEGER, r REAL, t TEXT, b BLOB, a ANY, n INT) STRICT"]
    return [("word:STRICT", st, [
        "INSERT INTO st VALUES (1, 1.5, 'x', x'00', 1, 2)", "INSERT INTO st(i) VALUES ('12')", "INSERT INTO st(i) VALUES ('12.0')", "INSERT INTO st(i) VALUES ('abc')", "INSERT INTO st(i) VALUES (1.5)",
        "INSERT INTO st(r) VALUES (3)", "INSERT INTO st(r) VALUES ('4.5')", "INSERT INTO st(r) VALUES (x'01')", "INSERT INTO st(t) VALUES (5)", "INSERT INTO st(b) VALUES ('text')",
        "INSERT INTO st(a) VALUES ('1')", "INSERT INTO st(n) VALUES (NULL)", "SELECT typeof(i), typeof(r), typeof(t), typeof(b), typeof(a) FROM st ORDER BY rowid",
        "UPDATE st SET i = 'nope' WHERE rowid = 1", "CREATE TABLE st2(x VARCHAR(10)) STRICT", "CREATE TABLE st3(x) STRICT", "CREATE TABLE st4(k INTEGER PRIMARY KEY, v TEXT NOT NULL) STRICT",
        "INSERT INTO st4(v) VALUES ('a')", "INSERT INTO st4 VALUES ('1', 'b')", "INSERT INTO st4 VALUES ('one', 'c')", "SELECT k, v FROM st4 ORDER BY k", "SELECT strict FROM pragma_table_list WHERE name = 'st'",
        "ALTER TABLE st ADD COLUMN z TEXT", "ALTER TABLE st ADD COLUMN y FLOAT", "INSERT INTO st(i) VALUES (9223372036854775807)", "INSERT INTO st(i) VALUES ('9223372036854775808')",
        "INSERT INTO st(a) VALUES (x'ff'), (2.5), (NULL)", "SELECT typeof(a) FROM st WHERE a IS NOT NULL ORDER BY rowid", "INSERT INTO st(t) VALUES (x'41')", "INSERT INTO st(r) VALUES (9223372036854775807)",
        "SELECT r, typeof(r) FROM st WHERE r > 1e18"])]


def collation_family():
    out = []
    for coll, extra in (("BINARY", "'A' < 'a'"), ("NOCASE", "'abc' = 'ABC'"), ("RTRIM", "'x ' = 'x'")):
        lc = coll.lower()
        out.append((f"collation:{coll}", [f"CREATE TABLE co_{lc}(k TEXT COLLATE {coll}, n INTEGER)",
                                          f"INSERT INTO co_{lc} VALUES ('a', 1), ('A', 2), ('a ', 3), ('b', 4), ('B ', 5), ('', 6), (NULL, 7), ('ab', 8)"], [
            f"SELECT k, n FROM co_{lc} ORDER BY k, n", f"SELECT count(DISTINCT k) FROM co_{lc}", f"SELECT k, count(*) FROM co_{lc} GROUP BY k ORDER BY k, 2", f"SELECT n FROM co_{lc} WHERE k = 'a' ORDER BY n",
            f"SELECT n FROM co_{lc} WHERE k > 'a' ORDER BY n", f"SELECT min(k), max(k) FROM co_{lc}", f"SELECT n FROM co_{lc} WHERE k IN ('A', 'b') ORDER BY n", f"CREATE INDEX co_{lc}_k ON co_{lc}(k)",
            f"SELECT n FROM co_{lc} WHERE k = 'a' ORDER BY n", f"SELECT n FROM co_{lc} WHERE k BETWEEN 'a' AND 'b' ORDER BY n", f"SELECT k FROM co_{lc} UNION SELECT 'A' ORDER BY 1",
            f"SELECT DISTINCT k FROM co_{lc} ORDER BY 1", f"SELECT n FROM co_{lc} WHERE k COLLATE BINARY = 'a' ORDER BY n", f"SELECT {extra} COLLATE {coll}",
            f"SELECT 'a' = 'A' COLLATE {coll}, 'a ' = 'a' COLLATE {coll}", f"CREATE UNIQUE INDEX co_{lc}_u ON co_{lc}(k)", f"SELECT n FROM co_{lc} WHERE k LIKE 'a%' ORDER BY n",
            f"SELECT group_concat(k, '|' ORDER BY k COLLATE {coll}, n) FROM co_{lc}", f"SELECT k FROM co_{lc} WHERE k IS NOT NULL ORDER BY k COLLATE {coll} DESC, n LIMIT 3"]))
    return out


def trigger_family():
    setup = ["CREATE TABLE acct(id INTEGER PRIMARY KEY, bal INTEGER NOT NULL DEFAULT 0, name TEXT)", "CREATE TABLE log(seq INTEGER PRIMARY KEY, what TEXT, old INTEGER, new INTEGER)",
             "INSERT INTO acct VALUES (1, 100, 'a'), (2, 50, 'b')"]
    return [("syntax:create-trigger-stmt", setup, [
        "CREATE TRIGGER t_ins AFTER INSERT ON acct BEGIN INSERT INTO log(what, new) VALUES ('ins', NEW.bal); END",
        "CREATE TRIGGER t_upd AFTER UPDATE OF bal ON acct FOR EACH ROW WHEN NEW.bal <> OLD.bal BEGIN INSERT INTO log(what, old, new) VALUES ('upd', OLD.bal, NEW.bal); END",
        "CREATE TRIGGER t_del BEFORE DELETE ON acct BEGIN INSERT INTO log(what, old) VALUES ('del', OLD.bal); END",
        "CREATE TRIGGER t_neg BEFORE UPDATE ON acct WHEN NEW.bal < 0 BEGIN SELECT RAISE(ABORT, 'negative balance'); END",
        "INSERT INTO acct(id, bal, name) VALUES (3, 10, 'c')", "UPDATE acct SET bal = bal - 20 WHERE id = 1", "UPDATE acct SET bal = bal WHERE id = 2", "UPDATE acct SET bal = -5 WHERE id = 2",
        "UPDATE acct SET name = 'bb' WHERE id = 2", "DELETE FROM acct WHERE id = 3", "SELECT what, old, new FROM log ORDER BY seq",
        "CREATE TRIGGER t_ign BEFORE INSERT ON acct WHEN NEW.name = 'skip' BEGIN SELECT RAISE(IGNORE); END", "INSERT INTO acct(id, name) VALUES (4, 'skip')", "SELECT count(*) FROM acct WHERE id = 4",
        "CREATE VIEW rich AS SELECT id, name FROM acct WHERE bal > 60",
        "CREATE TRIGGER t_view INSTEAD OF INSERT ON rich BEGIN INSERT INTO acct(id, bal, name) VALUES (NEW.id, 1000, NEW.name); END", "INSERT INTO rich VALUES (5, 'via view')",
        "SELECT id, bal, name FROM acct ORDER BY id",
        "CREATE TRIGGER t_view_upd INSTEAD OF UPDATE OF name ON rich BEGIN UPDATE acct SET name = upper(NEW.name) WHERE id = OLD.id; END", "UPDATE rich SET name = 'shout' WHERE id = 5",
        "SELECT name FROM acct WHERE id = 5", "CREATE TRIGGER t_view_del INSTEAD OF DELETE ON rich BEGIN DELETE FROM acct WHERE id = OLD.id; END", "DELETE FROM rich WHERE id = 5",
        "CREATE TEMP TRIGGER t_tmp AFTER INSERT ON log BEGIN UPDATE log SET what = what || '*' WHERE seq = NEW.seq; END", "INSERT INTO acct(id, bal, name) VALUES (6, 1, 'f')",
        "SELECT what FROM log ORDER BY seq DESC LIMIT 1", "CREATE TRIGGER IF NOT EXISTS t_ins AFTER INSERT ON acct BEGIN SELECT 1; END", "CREATE TRIGGER t_ins AFTER INSERT ON acct BEGIN SELECT 1; END",
        "CREATE TRIGGER t_fail BEFORE INSERT ON acct WHEN NEW.id > 100 BEGIN SELECT RAISE(FAIL, 'id too big'); END", "INSERT INTO acct(id) VALUES (101)", "BEGIN",
        "INSERT INTO acct(id, name) VALUES (7, 'g')", "CREATE TRIGGER t_rb BEFORE INSERT ON acct WHEN NEW.name = 'rollback' BEGIN SELECT RAISE(ROLLBACK, 'rolled back'); END",
        "INSERT INTO acct(id, name) VALUES (8, 'rollback')", "SELECT count(*) FROM acct WHERE id = 7", "COMMIT",
        "CREATE TRIGGER t_bump BEFORE INSERT ON acct BEGIN UPDATE acct SET bal = bal + 1 WHERE id = 1; END", "INSERT INTO acct(id, name) VALUES (9, 'h')", "SELECT bal FROM acct WHERE id = 1",
        "PRAGMA recursive_triggers = ON", "CREATE TABLE cnt(n INTEGER)", "INSERT INTO cnt VALUES (0)", "CREATE TRIGGER t_rec AFTER UPDATE ON cnt WHEN NEW.n < 5 BEGIN UPDATE cnt SET n = n + 1; END",
        "UPDATE cnt SET n = 1", "SELECT n FROM cnt", "CREATE TRIGGER t_bad AFTER INSERT ON nope BEGIN SELECT 1; END", "CREATE TRIGGER t_bad2 INSTEAD OF INSERT ON acct BEGIN SELECT 1; END",
        "CREATE TRIGGER t_bad3 BEFORE INSERT ON rich BEGIN SELECT 1; END", "SELECT name, tbl_name FROM sqlite_schema WHERE type = 'trigger' ORDER BY name", "DROP TRIGGER t_upd",
        "UPDATE acct SET bal = 0 WHERE id = 2", "SELECT count(*) FROM log WHERE what LIKE 'upd%'",
        "CREATE TRIGGER t_each AFTER DELETE ON acct FOR EACH ROW BEGIN INSERT INTO log(what, old) VALUES ('each', OLD.id); DELETE FROM log WHERE what = 'ins'; END",
        "DELETE FROM acct WHERE id IN (6, 9)", "SELECT what, old FROM log WHERE what IN ('each', 'ins') ORDER BY seq",
        "CREATE TRIGGER t_ret AFTER INSERT ON acct BEGIN INSERT INTO log(what) VALUES ('x') RETURNING seq; END",
        "CREATE TRIGGER t_qual AFTER INSERT ON acct BEGIN INSERT INTO main.log(what) VALUES ('x'); END",
        "CREATE TRIGGER t_case AFTER UPDATE ON acct BEGIN INSERT INTO log(what) VALUES (CASE WHEN NEW.bal > OLD.bal THEN 'up' ELSE 'down' END); END", "UPDATE acct SET bal = bal + 5 WHERE id = 1",
        "SELECT what FROM log ORDER BY seq DESC LIMIT 1"])]


def json_family():
    doc = ["CREATE TABLE doc(id INTEGER PRIMARY KEY, body TEXT)",
           "INSERT INTO doc VALUES (1, '{\"name\":\"a\",\"tags\":[\"x\",\"y\"],\"n\":1}'), (2, '{\"name\":\"b\",\"tags\":[],\"n\":2.5}'),"
           " (3, '{\"name\":null,\"tags\":[\"z\"],\"nested\":{\"k\":[1,2,3]}}'), (4, 'not json')"]
    return [("function.json:json_each", doc, [
        "SELECT id, body ->> '$.name' FROM doc WHERE json_valid(body) ORDER BY id", "SELECT id, body -> '$.tags' FROM doc WHERE json_valid(body) ORDER BY id",
        "SELECT id, json_array_length(body, '$.tags') FROM doc WHERE json_valid(body) ORDER BY id",
        "SELECT doc.id, j.value FROM doc, json_each(doc.body, '$.tags') AS j WHERE json_valid(doc.body) ORDER BY doc.id, j.key",
        "SELECT count(*) FROM doc, json_tree(doc.body) WHERE json_valid(doc.body) AND json_tree.type = 'integer'", "SELECT id FROM doc WHERE json_valid(body) AND body ->> '$.n' > 1 ORDER BY id",
        "SELECT json_group_array(body ->> '$.name') FROM (SELECT body FROM doc WHERE json_valid(body) ORDER BY id)",
        "SELECT json_group_object(id, body -> '$.n') FROM (SELECT id, body FROM doc WHERE json_valid(body) ORDER BY id)", "UPDATE doc SET body = json_set(body, '$.n', 10) WHERE id = 1 RETURNING body",
        "UPDATE doc SET body = json_insert(body, '$.tags[#]', 'w') WHERE id = 2 RETURNING body", "SELECT body -> '$.nested.k[1]', body ->> '$.nested.k[#-1]' FROM doc WHERE id = 3",
        "SELECT id, json_type(body, '$.n') FROM doc WHERE json_valid(body) ORDER BY id", "SELECT body -> '$.name' FROM doc WHERE id = 4", "SELECT json_extract(body, '$.name', '$.n') FROM doc WHERE id = 1",
        "SELECT id FROM doc WHERE json_valid(body) AND EXISTS (SELECT 1 FROM json_each(body, '$.tags') WHERE value = 'z')",
        "SELECT json_each.key, json_each.value FROM json_each('{\"b\":2,\"a\":1}') ORDER BY key", "SELECT fullkey, path, atom FROM json_tree('{\"a\":[1,{\"b\":2}]}') ORDER BY id",
        "CREATE INDEX doc_name ON doc(body ->> '$.name')", "DELETE FROM doc WHERE NOT json_valid(body)", "CREATE INDEX doc_name ON doc(body ->> '$.name')",
        "SELECT id FROM doc WHERE body ->> '$.name' = 'b'", "SELECT jsonb(body) = jsonb(body) FROM doc WHERE id = 1", "SELECT json(jsonb(body)) FROM doc WHERE id = 2",
        "SELECT body ->> 'name' FROM doc WHERE id = 1", "SELECT body -> 0 FROM doc WHERE id = 1", "SELECT '[10,20,30]' -> 1, '[10,20,30]' ->> -1",
        "SELECT key, value FROM doc, json_each(doc.body) WHERE doc.id = 3 ORDER BY key", "SELECT json_remove(body, '$.tags', '$.nested') FROM doc WHERE id = 3",
        "SELECT json_patch(body, '{\"n\":null,\"extra\":true}') FROM doc WHERE id = 1"])]


FAMILIES = window_family() + recursive_family() + upsert_family() + generated_family() + strict_family() + collation_family() + trigger_family() + json_family()


def top_order_by(sql):
    """Whether the statement orders its own rows: an ORDER BY outside every parenthesis."""
    depth, prev = 0, ""
    for m in tagmod.TOKEN.finditer(sql):
        if m.lastgroup in ("ws", "comment"):
            continue
        t = m.group().upper()
        if t == "(":
            depth += 1
        elif t == ")":
            depth -= 1
        elif depth == 0 and prev == "ORDER" and t == "BY":
            return True
        prev = t
    return False


def run_group(setup, cases):
    """One group on a fresh database: its setup, then each case as SQLite answers it, as records
    (kind, sql, "ok" | "error" | (types, sort, values))."""
    fd, db = tempfile.mkstemp(prefix="written-", suffix=".db", dir=os.path.expanduser("~/.cache"))
    os.close(fd)
    con = slt.connect(db)
    out = []
    try:
        for s in BASE + setup:
            for part in slt.split(s):
                con.execute(part).fetchall()
        for q in cases:
            assert slt.record_sql(q) == q and not q.startswith("#"), q  # each case as a record carries it
            parts = slt.split(q)
            try:
                if len(parts) > 1 or re.match(r"\s*EXPLAIN\b", q, re.I):
                    for part in parts:
                        slt.fetch(con.execute(part))
                    out.append(("statement", q, "ok"))
                    continue
                cur = con.execute(q)
                rows = slt.fetch(cur)
            except sqlite3.Error:
                out.append(("statement", q, "error"))
                continue
            if cur.description is None:
                out.append(("statement", q, "ok"))
            else:
                types = slt.column_types(rows, len(cur.description))
                sort = "nosort" if top_order_by(q) else "rowsort"
                out.append(("query", q, (types, sort, slt.answer(rows, types, sort))))
    finally:
        con.close()
        os.remove(db)
    return out


def file_text(fid, setup, records):
    """The group as a sqllogictest file, and the line its cases start on."""
    lines = ["hash-threshold 8", "", f"# written for {fid}: the shared tables, the group's own setup, then its cases", ""]
    for s in BASE + setup:
        lines += ["statement ok", s, ""]
    first = len(lines) + 1
    for kind, q, detail in records:
        if kind == "statement":
            lines += [f"statement {detail}", q, ""]
            continue
        types, sort, vals = detail
        hashed = len(vals) > 8 or any(not v.strip() for v in vals)
        lines += [f"query {types} {sort}", q, "----"] + ([f"{len(vals)} values hashing to {slt.md5_of(vals)}"] if hashed else vals) + [""]
    return "\n".join(lines), first


def merge(pairs):
    """(setup, query) pairs to groups: consecutive queries on the same setup share a group."""
    out = []
    for setup, q in pairs:
        if out and out[-1][0] == setup:
            out[-1][1].append(q)
        else:
            out.append((setup, [q]))
    return out


def groups_for(f):
    fid, k, name = f["id"], f["kind"], f["name"]
    if k.startswith("function."):
        return [([], fn_cases(name, k.split(".")[1]))]
    if k == "operator":
        return [([], op_cases(name))]
    if k == "pragma":
        if name in ("soft_heap_limit", "hard_heap_limit"):
            # process-wide limits: from both off, and back to both off (a hard limit caps the soft one)
            return [(PRAGMA_SETUP + ["PRAGMA hard_heap_limit = 0", "PRAGMA soft_heap_limit = 0"], pragma_cases(name) + ["PRAGMA soft_heap_limit = 0", "PRAGMA soft_heap_limit"])]
        return [(PRAGMA_SETUP, pragma_cases(name))]
    if k.startswith("rule."):
        return merge(rule_cases(k, name))
    if k == "word" and name in ("STRICT", "STORED", "ROWID", "EXPRESSIONS"):
        return merge(word_groups(name))
    if k == "syntax":
        return SYNTAX.get(name, [])
    if k == "keyword":
        return []  # keywords come from the rest; EXTRA fills any left short
    return [([], kind_cases(f))]


def main():
    feats = json.load(open(FEATURES))["features"]
    ids = {f["id"] for f in feats}
    tagger = tagmod.Tagger(FEATURES)
    groups = []  # (entry, setup, cases)
    for f in feats:
        groups += [(f["id"], setup, qs) for setup, qs in groups_for(f) if qs]
    groups += FAMILIES
    for fid, _, _ in groups:
        assert fid in ids, fid
    reached = {}  # entry -> distinct case texts that reach it (by its text, or written for it)

    def count(fid, qs):
        for q in qs:
            feats_q, _ = tagger.tag(q)
            for e in feats_q | {fid}:
                reached.setdefault(e, set()).add(" ".join(q.split()))
    for fid, _, qs in groups:
        count(fid, qs)
    for f in feats:
        if len(reached.get(f["id"], ())) < NEED and f["id"] in EXTRA:
            groups.append((f["id"], [], EXTRA[f["id"]]))
            count(f["id"], EXTRA[f["id"]])
    root = os.path.join(OUT, "written")
    os.makedirs(root, exist_ok=True)
    index, seen, cases, errors = {}, {}, {}, 0
    for fid, setup, qs in groups:
        records = run_group(setup, qs)
        errors += sum(1 for r in records if r[2] == "error")
        kind, name = fid.split(":", 1)
        slug = (re.sub(r"[^A-Za-z0-9]+", "-", name).strip("-").lower()[:40] or "op") + "-" + hashlib.md5(fid.encode()).hexdigest()[:6]
        seen[slug] = seen.get(slug, 0) + 1
        rel = os.path.join("written", kind, slug + (f"-{seen[slug]}" if seen[slug] > 1 else "") + ".test")
        text, first = file_text(fid, setup, records)
        os.makedirs(os.path.dirname(os.path.join(OUT, rel)), exist_ok=True)
        open(os.path.join(OUT, rel), "w").write(text)
        index[rel] = {"entry": fid, "from_line": first}
        cases[fid] = cases.get(fid, 0) + len(records)
    json.dump(index, open(os.path.join(root, "index.json"), "w"), indent=1, sort_keys=True)
    short = sorted((len(reached.get(f["id"], ())), f["id"]) for f in feats if len(reached.get(f["id"], ())) < NEED)
    json.dump({"cases": cases, "reached": {e: len(s) for e, s in reached.items() if e in ids}, "short": short}, open(os.path.join(OUT, "written-cases.json"), "w"), indent=1, sort_keys=True)
    print(f"{sum(cases.values())} written cases in {len(index)} files, {errors} of them errors SQLite reports; entries reached by fewer than {NEED}: {len(short)}")
    for n, e in short:
        print(f"  short: {e} ({n})")


# Entries the rest of the written cases reach fewer than NEED times: cases of their own, each list
# written for the entry it's keyed by (a statement that shows the entry's effect counts for it too).
FK_ON = "PRAGMA foreign_keys = ON"
EXTRA = {
    "keyword:CROSS": [
        "SELECT count(*) FROM t1 CROSS JOIN t2", "SELECT t1.a, t2.a FROM t1 CROSS JOIN t2 WHERE t1.a = t2.a ORDER BY 1", "SELECT count(*) FROM t1 AS x CROSS JOIN t1 AS y",
        "SELECT x.a FROM t2 AS x CROSS JOIN t2 AS y WHERE x.a = y.c / 10 ORDER BY 1", "SELECT count(*) FROM t1 CROSS JOIN t2 CROSS JOIN t1 AS t3", "SELECT min(t1.a + t2.a) FROM t1 CROSS JOIN t2",
        "SELECT count(*) FROM (SELECT 1) CROSS JOIN t2", "SELECT t2.b FROM t1 CROSS JOIN t2 WHERE t1.a = 1 ORDER BY t2.a", "SELECT count(*) FROM t1 CROSS JOIN t2 ON t1.a = t2.a",
        "SELECT count(*) FROM t1 CROSS JOIN t2 USING (a)", "SELECT max(t1.c * t2.c) FROM t1 CROSS JOIN t2", "SELECT count(*) FROM t1 NATURAL CROSS JOIN t2"],
    "keyword:INNER": [
        "SELECT t1.a, t2.b FROM t1 INNER JOIN t2 ON t1.a = t2.a ORDER BY 1", "SELECT count(*) FROM t1 INNER JOIN t2 USING (a)", "SELECT t1.b FROM t1 INNER JOIN t2 ON t2.b = t1.b COLLATE NOCASE ORDER BY 1",
        "SELECT x.a FROM t1 AS x INNER JOIN t1 AS y ON x.a = y.a - 1 ORDER BY 1", "SELECT count(*) FROM t1 INNER JOIN t2 ON t1.a > t2.a",
        "SELECT t2.a, count(t1.a) FROM t2 INNER JOIN t1 ON t1.a <= t2.a GROUP BY t2.a ORDER BY 1", "SELECT * FROM t2 INNER JOIN (SELECT a AS k FROM t1 WHERE a > 2) AS s ON s.k = t2.a ORDER BY 1",
        "SELECT count(*) FROM t1 INNER JOIN t2 ON 0", "SELECT t1.a FROM t1 INNER JOIN t2 ON t1.a = t2.a INNER JOIN t2 AS t3 ON t3.a = t1.a + 1 ORDER BY 1", "SELECT count(*) FROM t1 NATURAL INNER JOIN t2",
        "SELECT sum(t2.c) FROM t1 INNER JOIN t2 ON t1.a = t2.a WHERE t1.b IS NOT NULL"],
    "keyword:LEFT": [
        "SELECT t1.a, t2.b FROM t1 LEFT JOIN t2 ON t1.a = t2.a ORDER BY 1", "SELECT t1.a FROM t1 LEFT JOIN t2 ON t1.a = t2.a WHERE t2.a IS NULL ORDER BY 1", "SELECT count(t2.a) FROM t1 LEFT OUTER JOIN t2 USING (a)",
        "SELECT t1.a, t2.c FROM t1 LEFT JOIN t2 ON t1.a = t2.a AND t2.c > 15 ORDER BY 1", "SELECT t2.a, t1.b FROM t2 LEFT JOIN t1 ON t1.a = t2.a ORDER BY 1", "SELECT count(*) FROM t1 LEFT JOIN t2 ON 0",
        "SELECT t1.a, coalesce(t2.b, '-') FROM t1 LEFT JOIN t2 USING (a) ORDER BY 1", "SELECT a FROM t1 NATURAL LEFT JOIN t2 ORDER BY 1", "SELECT t1.a, x.a FROM t1 LEFT JOIN t1 AS x ON x.a = t1.a + 1 ORDER BY 1",
        "SELECT t1.a, t2.a, t3.a FROM t1 LEFT JOIN t2 ON t2.a = t1.a LEFT JOIN t2 AS t3 ON t3.a = t2.a + 1 ORDER BY 1", "SELECT t1.a FROM t1 LEFT JOIN t2 ON t1.a = t2.a WHERE t2.c IS NOT NULL ORDER BY 1"],
    "keyword:RIGHT": [
        "SELECT t1.a, t2.a FROM t1 RIGHT JOIN t2 ON t1.a = t2.a ORDER BY 2", "SELECT t2.a FROM t1 RIGHT JOIN t2 ON t1.a = t2.a WHERE t1.a IS NULL ORDER BY 1", "SELECT count(t1.a) FROM t1 RIGHT OUTER JOIN t2 USING (a)",
        "SELECT t2.a, t1.c FROM t1 RIGHT JOIN t2 ON t1.a = t2.a AND t1.c > 2 ORDER BY 1", "SELECT count(*) FROM t1 RIGHT JOIN t2 ON 0", "SELECT a FROM t1 NATURAL RIGHT JOIN t2 ORDER BY 1",
        "SELECT t1.a, t2.a FROM t2 RIGHT JOIN t1 ON t1.a = t2.a ORDER BY 1", "SELECT x.a, y.a FROM t2 AS x RIGHT JOIN t2 AS y ON y.a = x.a + 1 ORDER BY 2",
        "SELECT t2.a, coalesce(t1.b, '-') FROM t1 RIGHT JOIN t2 USING (a) ORDER BY 1", "SELECT t1.a, t2.a, t3.a FROM t1 RIGHT JOIN t2 ON t1.a = t2.a RIGHT JOIN t2 AS t3 ON t3.a = t2.a ORDER BY 3",
        "SELECT t1.a, t2.a FROM t1 LEFT JOIN t2 ON t1.a = t2.a RIGHT JOIN t1 AS t3 ON t3.a = t1.a ORDER BY 1"],
    "keyword:FULL": [
        "SELECT t1.a, t2.a FROM t1 FULL JOIN t2 ON t1.a = t2.a ORDER BY coalesce(t1.a, t2.a)", "SELECT count(*) FROM t1 FULL OUTER JOIN t2 USING (a)", "SELECT count(*) FROM t1 FULL JOIN t2 ON 0",
        "SELECT t1.a, t2.a FROM t1 FULL JOIN t2 ON t1.a = t2.a WHERE t1.a IS NULL OR t2.a IS NULL ORDER BY coalesce(t1.a, t2.a)", "SELECT a FROM t1 NATURAL FULL JOIN t2 ORDER BY 1",
        "SELECT count(t1.a), count(t2.a) FROM t1 FULL JOIN t2 ON t1.a = t2.a", "SELECT x.a, y.a FROM t2 AS x FULL JOIN t2 AS y ON y.a = x.a + 1 ORDER BY coalesce(x.a, y.a)",
        "SELECT t1.a, t2.a FROM t1 FULL JOIN t2 ON t1.a = t2.a AND t2.c > 15 ORDER BY coalesce(t1.a, t2.a)", "SELECT sum(coalesce(t1.a, 0) + coalesce(t2.a, 0)) FROM t1 FULL JOIN t2 USING (a)",
        "SELECT t1.a, t2.a, t3.a FROM t1 FULL JOIN t2 ON t1.a = t2.a FULL JOIN t2 AS t3 ON t3.a = t1.a ORDER BY 1, 2, 3", "SELECT a FROM t1 FULL JOIN t2 USING (a) ORDER BY 1"],
    "keyword:OUTER": [
        "SELECT t1.a, t2.b FROM t1 LEFT OUTER JOIN t2 ON t1.a = t2.a ORDER BY 1", "SELECT t1.a, t2.a FROM t1 RIGHT OUTER JOIN t2 ON t1.a = t2.a ORDER BY 2",
        "SELECT t1.a, t2.a FROM t1 FULL OUTER JOIN t2 ON t1.a = t2.a ORDER BY coalesce(t1.a, t2.a)", "SELECT count(*) FROM t1 LEFT OUTER JOIN t2 USING (a)", "SELECT a FROM t1 NATURAL LEFT OUTER JOIN t2 ORDER BY 1",
        "SELECT count(*) FROM t1 RIGHT OUTER JOIN t2 ON 0", "SELECT count(*) FROM t1 FULL OUTER JOIN t2 ON t1.a = t2.a AND t1.a > 3", "SELECT t1.a FROM t1 LEFT OUTER JOIN t2 ON t1.a = t2.a WHERE t2.a IS NULL ORDER BY 1",
        "SELECT a FROM t1 NATURAL RIGHT OUTER JOIN t2 ORDER BY 1", "SELECT a FROM t1 NATURAL FULL OUTER JOIN t2 ORDER BY 1", "SELECT count(*) FROM t1 OUTER JOIN t2"],
    "keyword:NATURAL": [
        "SELECT a, b FROM t1 NATURAL JOIN t2 ORDER BY a", "SELECT count(*) FROM t1 NATURAL JOIN t1 AS x", "SELECT a FROM t1 NATURAL LEFT JOIN t2 ORDER BY 1", "SELECT a FROM t1 NATURAL RIGHT JOIN t2 ORDER BY 1",
        "SELECT a FROM t1 NATURAL FULL JOIN t2 ORDER BY 1", "SELECT count(*) FROM t1 NATURAL INNER JOIN t2", "SELECT * FROM (SELECT 1 AS k, 'x' AS v) NATURAL JOIN (SELECT 1 AS k, 'y' AS w)",
        "SELECT * FROM (SELECT 1 AS k) NATURAL JOIN (SELECT 2 AS j)", "SELECT count(*) FROM t1 NATURAL JOIN t2 NATURAL JOIN t1 AS t3", "SELECT k FROM (SELECT 1 AS k UNION SELECT 2) NATURAL JOIN (SELECT 2 AS k) ORDER BY 1",
        "SELECT * FROM t1 NATURAL JOIN t2 USING (a)"],
    "keyword:ACTION": [
        FK_ON, "CREATE TABLE pa(k INTEGER PRIMARY KEY)", "CREATE TABLE ca(r INTEGER REFERENCES pa(k) ON DELETE NO ACTION)", "CREATE TABLE cb(r INTEGER REFERENCES pa(k) ON UPDATE NO ACTION ON DELETE NO ACTION)",
        "INSERT INTO pa VALUES (1), (2)", "INSERT INTO ca VALUES (1)", "INSERT INTO cb VALUES (2)", "DELETE FROM pa WHERE k = 1", "UPDATE pa SET k = 20 WHERE k = 2", "DELETE FROM ca", "DELETE FROM pa WHERE k = 1",
        "SELECT k FROM pa ORDER BY k", "CREATE TABLE cc(r REFERENCES pa ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED)", "SELECT on_delete, on_update FROM pragma_foreign_key_list('cb')",
        "CREATE TABLE cd(r REFERENCES pa ON DELETE ACTION)"],
    "keyword:CASCADE": [
        FK_ON, "CREATE TABLE pc(k INTEGER PRIMARY KEY, v TEXT)", "CREATE TABLE cc1(r INTEGER REFERENCES pc(k) ON DELETE CASCADE)", "CREATE TABLE cc2(r INTEGER REFERENCES pc(k) ON UPDATE CASCADE)",
        "CREATE TABLE cc3(r INTEGER REFERENCES pc(k) ON DELETE CASCADE ON UPDATE CASCADE)", "INSERT INTO pc VALUES (1, 'a'), (2, 'b'), (3, 'c')", "INSERT INTO cc1 VALUES (1), (2), (2)",
        "INSERT INTO cc2 VALUES (3)", "INSERT INTO cc3 VALUES (1)", "DELETE FROM pc WHERE k = 2", "SELECT r FROM cc1 ORDER BY r", "UPDATE pc SET k = 30 WHERE k = 3", "SELECT r FROM cc2",
        "UPDATE pc SET k = 10 WHERE k = 1", "SELECT r FROM cc3", "SELECT r FROM cc1", "DELETE FROM pc", "SELECT count(*) FROM cc1", "DROP TABLE pc CASCADE"],
    "keyword:RESTRICT": [
        FK_ON, "CREATE TABLE pr(k INTEGER PRIMARY KEY)", "CREATE TABLE cr1(r INTEGER REFERENCES pr(k) ON DELETE RESTRICT)", "CREATE TABLE cr2(r INTEGER REFERENCES pr(k) ON UPDATE RESTRICT)",
        "INSERT INTO pr VALUES (1), (2), (3)", "INSERT INTO cr1 VALUES (1)", "INSERT INTO cr2 VALUES (2)", "DELETE FROM pr WHERE k = 1", "UPDATE pr SET k = 20 WHERE k = 2", "DELETE FROM pr WHERE k = 3",
        "UPDATE pr SET k = 10 WHERE k = 1", "SELECT k FROM pr ORDER BY k", "BEGIN", "DELETE FROM cr1", "DELETE FROM pr WHERE k = 1", "COMMIT", "SELECT k FROM pr ORDER BY k",
        "CREATE TABLE cr3(r REFERENCES pr ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED)", "INSERT INTO cr3 VALUES (2)", "DELETE FROM pr WHERE k = 2", "DROP TABLE cr1 RESTRICT"],
    "keyword:DEFERRABLE": [
        FK_ON, "CREATE TABLE pd(k INTEGER PRIMARY KEY)", "CREATE TABLE cd1(r INTEGER REFERENCES pd(k) DEFERRABLE INITIALLY DEFERRED)", "CREATE TABLE cd2(r INTEGER REFERENCES pd(k) DEFERRABLE INITIALLY IMMEDIATE)",
        "CREATE TABLE cd3(r INTEGER REFERENCES pd(k) NOT DEFERRABLE)", "CREATE TABLE cd4(r INTEGER REFERENCES pd(k) DEFERRABLE)", "BEGIN", "INSERT INTO cd1 VALUES (1)", "INSERT INTO pd VALUES (1)", "COMMIT",
        "BEGIN", "INSERT INTO cd2 VALUES (2)", "ROLLBACK", "INSERT INTO cd3 VALUES (3)", "BEGIN", "INSERT INTO cd4 VALUES (4)", "ROLLBACK", "BEGIN", "INSERT INTO cd1 VALUES (5)", "COMMIT", "ROLLBACK",
        "SELECT r FROM cd1 ORDER BY r", "CREATE TABLE cd5(r INTEGER, FOREIGN KEY (r) REFERENCES pd(k) NOT DEFERRABLE INITIALLY DEFERRED)", "BEGIN", "INSERT INTO cd5 VALUES (6)", "ROLLBACK"],
    "keyword:DEFERRED": [
        FK_ON, "CREATE TABLE pf(k INTEGER PRIMARY KEY)", "CREATE TABLE cf1(r INTEGER REFERENCES pf(k) DEFERRABLE INITIALLY DEFERRED)", "BEGIN DEFERRED", "INSERT INTO cf1 VALUES (1)", "INSERT INTO pf VALUES (1)",
        "COMMIT", "BEGIN DEFERRED TRANSACTION", "INSERT INTO cf1 VALUES (2)", "COMMIT", "ROLLBACK", "SELECT r FROM cf1 ORDER BY r", "PRAGMA defer_foreign_keys = ON", "CREATE TABLE cf2(r INTEGER REFERENCES pf(k))",
        "BEGIN DEFERRED", "INSERT INTO cf2 VALUES (9)", "INSERT INTO pf VALUES (9)", "COMMIT", "SELECT r FROM cf2", "BEGIN DEFERRED", "SELECT count(*) FROM pf", "END"],
    "keyword:IMMEDIATE": [
        FK_ON, "CREATE TABLE pi(k INTEGER PRIMARY KEY)", "CREATE TABLE ci1(r INTEGER REFERENCES pi(k) DEFERRABLE INITIALLY IMMEDIATE)", "BEGIN IMMEDIATE", "INSERT INTO pi VALUES (1)", "INSERT INTO ci1 VALUES (1)",
        "COMMIT", "BEGIN IMMEDIATE TRANSACTION", "INSERT INTO ci1 VALUES (2)", "ROLLBACK", "BEGIN IMMEDIATE", "BEGIN IMMEDIATE", "ROLLBACK", "SELECT r FROM ci1",
        "CREATE TABLE ci2(r INTEGER REFERENCES pi(k) NOT DEFERRABLE INITIALLY IMMEDIATE)", "INSERT INTO ci2 VALUES (7)", "SELECT count(*) FROM ci2"],
    "keyword:INITIALLY": [
        FK_ON, "CREATE TABLE pn(k INTEGER PRIMARY KEY)", "CREATE TABLE cn1(r INTEGER REFERENCES pn(k) DEFERRABLE INITIALLY DEFERRED)", "CREATE TABLE cn2(r INTEGER REFERENCES pn(k) DEFERRABLE INITIALLY IMMEDIATE)",
        "CREATE TABLE cn3(r INTEGER REFERENCES pn(k) NOT DEFERRABLE INITIALLY DEFERRED)", "BEGIN", "INSERT INTO cn1 VALUES (1)", "INSERT INTO cn2 VALUES (1)", "ROLLBACK", "BEGIN", "INSERT INTO cn1 VALUES (2)",
        "INSERT INTO pn VALUES (2)", "COMMIT", "BEGIN", "INSERT INTO cn3 VALUES (3)", "ROLLBACK", "SELECT r FROM cn1", "CREATE TABLE cn4(r REFERENCES pn INITIALLY DEFERRED)"],
    "keyword:FOREIGN": [
        FK_ON, "CREATE TABLE po(a INTEGER, b TEXT, PRIMARY KEY (a, b))", "CREATE TABLE co1(x INTEGER, y TEXT, FOREIGN KEY (x, y) REFERENCES po(a, b))",
        "CREATE TABLE co2(x INTEGER, FOREIGN KEY (x) REFERENCES t2(a) ON DELETE SET NULL)", "CREATE TABLE co3(x, CONSTRAINT fk_x FOREIGN KEY (x) REFERENCES t2)", "INSERT INTO po VALUES (1, 'a')",
        "INSERT INTO co1 VALUES (1, 'a')", "INSERT INTO co1 VALUES (1, 'b')", "INSERT INTO co2 VALUES (6)", "DELETE FROM t2 WHERE a = 6", "SELECT x FROM co2", "INSERT INTO co3 VALUES (99)",
        "SELECT \"table\", \"from\", \"to\" FROM pragma_foreign_key_list('co1') ORDER BY seq", "PRAGMA foreign_key_check", "CREATE TABLE co4(x, FOREIGN KEY (x, x) REFERENCES po(a))",
        "CREATE TABLE co5(x, FOREIGN KEY REFERENCES po)"],
    "keyword:REFERENCES": [
        FK_ON, "CREATE TABLE pe(k INTEGER PRIMARY KEY, u TEXT UNIQUE)", "CREATE TABLE ce1(r REFERENCES pe)", "CREATE TABLE ce2(r REFERENCES pe(k))", "CREATE TABLE ce3(r REFERENCES pe(u))",
        "CREATE TABLE ce4(r INTEGER REFERENCES pe(k) ON DELETE SET DEFAULT DEFAULT 0)", "INSERT INTO pe VALUES (0, 'zero'), (1, 'one')", "INSERT INTO ce1 VALUES (1)", "INSERT INTO ce2 VALUES (2)",
        "INSERT INTO ce3 VALUES ('one')", "INSERT INTO ce3 VALUES ('two')", "INSERT INTO ce4 VALUES (1)", "DELETE FROM pe WHERE k = 1", "SELECT r FROM ce4", "CREATE TABLE ce5(r REFERENCES nope(k))",
        "INSERT INTO ce5 VALUES (1)", "CREATE TABLE ce6(r REFERENCES pe(v))", "INSERT INTO ce6 VALUES (1)"],
    "keyword:CONSTRAINT": [
        "CREATE TABLE ks1(a INTEGER CONSTRAINT pk PRIMARY KEY, b TEXT CONSTRAINT nn NOT NULL CONSTRAINT dflt DEFAULT 'd')", "INSERT INTO ks1(a) VALUES (1)", "INSERT INTO ks1(a, b) VALUES (2, NULL)",
        "CREATE TABLE ks2(a, b, CONSTRAINT u_ab UNIQUE (a, b), CONSTRAINT ck CHECK (a < b))", "INSERT INTO ks2 VALUES (1, 2)", "INSERT INTO ks2 VALUES (1, 2)", "INSERT INTO ks2 VALUES (3, 2)",
        "CREATE TABLE ks3(a CONSTRAINT c1 CHECK (a > 0) CONSTRAINT c2 CHECK (a < 10))", "INSERT INTO ks3 VALUES (11)", "INSERT INTO ks3 VALUES (5)", "SELECT a, b FROM ks1 ORDER BY a",
        "CREATE TABLE ks4(a, CONSTRAINT fk FOREIGN KEY (a) REFERENCES ks1(a))", "CREATE TABLE ks5(a CONSTRAINT)", "CREATE TABLE ks6(a CONSTRAINT x COLLATE NOCASE)", "SELECT count(*) FROM ks2"],
    "keyword:CHECK": [
        "CREATE TABLE kc1(a INTEGER CHECK (a > 0), b TEXT CHECK (length(b) < 5))", "INSERT INTO kc1 VALUES (1, 'ok')", "INSERT INTO kc1 VALUES (0, 'ok')", "INSERT INTO kc1 VALUES (2, 'too long')",
        "INSERT INTO kc1 VALUES (NULL, NULL)", "UPDATE kc1 SET a = -1 WHERE a = 1", "CREATE TABLE kc2(a, b, CHECK (a <> b), CHECK (a IS NOT NULL))", "INSERT INTO kc2 VALUES (1, 1)", "INSERT INTO kc2 VALUES (NULL, 1)",
        "INSERT INTO kc2 VALUES (1, 2)", "PRAGMA ignore_check_constraints = ON", "INSERT INTO kc1 VALUES (-5, 'x')", "PRAGMA ignore_check_constraints = OFF", "SELECT a, b FROM kc1 ORDER BY rowid",
        "CREATE TABLE kc3(a CHECK (a > (SELECT 1)))", "CREATE TABLE kc4(a CHECK (random() IS NOT NULL))", "INSERT INTO kc4 VALUES (1)", "CREATE TABLE kc5(a INTEGER CHECK (typeof(a) = 'integer'))",
        "INSERT INTO kc5 VALUES ('12')", "INSERT INTO kc5 VALUES ('x')", "SELECT a, typeof(a) FROM kc5"],
    "keyword:AUTOINCREMENT": [
        "CREATE TABLE ka(k INTEGER PRIMARY KEY AUTOINCREMENT, v TEXT)", "INSERT INTO ka(v) VALUES ('a'), ('b')", "DELETE FROM ka WHERE k = 2", "INSERT INTO ka(v) VALUES ('c')", "SELECT k, v FROM ka ORDER BY k",
        "SELECT name, seq FROM sqlite_sequence", "INSERT INTO ka VALUES (100, 'big')", "INSERT INTO ka(v) VALUES ('next')", "SELECT max(k) FROM ka", "UPDATE sqlite_sequence SET seq = 1000 WHERE name = 'ka'",
        "INSERT INTO ka(v) VALUES ('after')", "SELECT k FROM ka WHERE v = 'after'", "CREATE TABLE kb(k INT PRIMARY KEY AUTOINCREMENT)", "CREATE TABLE kc(k INTEGER PRIMARY KEY DESC AUTOINCREMENT)",
        "CREATE TABLE kd(k INTEGER PRIMARY KEY AUTOINCREMENT) WITHOUT ROWID", "DELETE FROM ka", "INSERT INTO ka(v) VALUES ('fresh')", "SELECT k FROM ka"],
    "keyword:ALWAYS": [
        "CREATE TABLE kg(a INTEGER, b INTEGER GENERATED ALWAYS AS (a + 1), c TEXT GENERATED ALWAYS AS (printf('%03d', a)) STORED, d GENERATED ALWAYS AS (a * 2) VIRTUAL)", "INSERT INTO kg(a) VALUES (1), (20), (NULL)",
        "SELECT a, b, c, d FROM kg ORDER BY rowid", "UPDATE kg SET a = 5 WHERE a = 1", "SELECT b, c, d FROM kg WHERE a = 5", "INSERT INTO kg(a, b) VALUES (1, 1)", "CREATE INDEX kgi ON kg(c)",
        "SELECT a FROM kg WHERE c = '020'", "CREATE TABLE kh(a, b GENERATED ALWAYS AS (a) STORED GENERATED ALWAYS AS (a))", "CREATE TABLE ki(a, b ALWAYS AS (a))", "SELECT count(*) FROM kg WHERE d > 5",
        "SELECT name, hidden FROM pragma_table_xinfo('kg') ORDER BY cid"],
    "keyword:ABORT": [
        "CREATE TABLE kab(k INTEGER PRIMARY KEY, v TEXT UNIQUE ON CONFLICT ABORT)", "INSERT INTO kab VALUES (1, 'a')", "INSERT INTO kab VALUES (2, 'a')", "INSERT OR ABORT INTO kab VALUES (1, 'x')",
        "BEGIN", "INSERT INTO kab VALUES (3, 'c')", "INSERT OR ABORT INTO kab VALUES (4, 'c')", "COMMIT", "SELECT k, v FROM kab ORDER BY k", "UPDATE OR ABORT kab SET v = 'a' WHERE k = 3",
        "CREATE TABLE kac(a NOT NULL ON CONFLICT ABORT)", "INSERT INTO kac VALUES (NULL)", "INSERT INTO kac VALUES (1), (NULL)", "SELECT count(*) FROM kac",
        "CREATE TRIGGER kat BEFORE INSERT ON kab WHEN NEW.v = 'z' BEGIN SELECT RAISE(ABORT, 'no z'); END", "INSERT INTO kab VALUES (9, 'z')", "SELECT count(*) FROM kab"],
    "keyword:FAIL": [
        "CREATE TABLE kf(k INTEGER PRIMARY KEY, v TEXT UNIQUE ON CONFLICT FAIL)", "INSERT INTO kf VALUES (1, 'a'), (2, 'b')", "INSERT INTO kf VALUES (3, 'c'), (4, 'a'), (5, 'e')", "SELECT k FROM kf ORDER BY k",
        "INSERT OR FAIL INTO kf VALUES (6, 'f'), (1, 'dup'), (7, 'g')", "SELECT k FROM kf ORDER BY k", "UPDATE OR FAIL kf SET v = 'zz' WHERE k >= 2", "SELECT k, v FROM kf ORDER BY k",
        "CREATE TABLE kg2(a NOT NULL ON CONFLICT FAIL)", "INSERT INTO kg2 VALUES (1), (NULL), (3)", "SELECT a FROM kg2",
        "CREATE TRIGGER kft BEFORE INSERT ON kf WHEN NEW.v = 'q' BEGIN SELECT RAISE(FAIL, 'no q'); END", "INSERT INTO kf VALUES (20, 'p'), (21, 'q'), (22, 'r')", "SELECT k FROM kf WHERE k >= 20 ORDER BY k"],
    "keyword:IGNORE": [
        "CREATE TABLE kig(k INTEGER PRIMARY KEY, v TEXT UNIQUE ON CONFLICT IGNORE, n TEXT NOT NULL ON CONFLICT IGNORE)", "INSERT INTO kig VALUES (1, 'a', 'x'), (2, 'a', 'y'), (3, 'c', NULL)",
        "SELECT k, v, n FROM kig ORDER BY k", "INSERT OR IGNORE INTO kig VALUES (1, 'z', 'z')", "UPDATE OR IGNORE kig SET k = 1", "SELECT k FROM kig ORDER BY k", "INSERT OR IGNORE INTO t2 VALUES (1, 'dup', 0)",
        "INSERT OR IGNORE INTO t2 VALUES (90, 'new', 0)", "SELECT count(*) FROM t2", "CREATE TRIGGER kit BEFORE INSERT ON kig WHEN NEW.v = 'skip' BEGIN SELECT RAISE(IGNORE); END",
        "INSERT INTO kig VALUES (9, 'skip', 'x')", "SELECT count(*) FROM kig WHERE k = 9", "REPLACE INTO kig VALUES (1, 'r', 'r')", "SELECT v FROM kig WHERE k = 1"],
    "keyword:CURRENT_DATE": [
        "SELECT typeof(CURRENT_DATE)", "SELECT length(CURRENT_DATE)", "SELECT CURRENT_DATE IS NOT NULL", "SELECT substr(CURRENT_DATE, 5, 1)", "SELECT CURRENT_DATE > '2000-01-01'", "SELECT date(CURRENT_DATE) = CURRENT_DATE",
        "CREATE TABLE kd1(a, d DEFAULT CURRENT_DATE)", "INSERT INTO kd1(a) VALUES (1)", "SELECT length(d), typeof(d) FROM kd1", "SELECT CURRENT_DATE BETWEEN '1970-01-01' AND '9999-12-31'",
        "SELECT typeof(julianday(CURRENT_DATE))", "SELECT CURRENT_DATE LIKE '____-__-__'"],
    "keyword:CURRENT_TIME": [
        "SELECT typeof(CURRENT_TIME)", "SELECT length(CURRENT_TIME)", "SELECT CURRENT_TIME IS NOT NULL", "SELECT substr(CURRENT_TIME, 3, 1)", "SELECT CURRENT_TIME >= '00:00:00'", "SELECT time(CURRENT_TIME) IS NOT NULL",
        "CREATE TABLE kt1(a, t DEFAULT CURRENT_TIME)", "INSERT INTO kt1(a) VALUES (1)", "SELECT length(t), typeof(t) FROM kt1", "SELECT CURRENT_TIME BETWEEN '00:00:00' AND '23:59:60'",
        "SELECT CURRENT_TIME LIKE '__:__:__'", "SELECT typeof(CURRENT_TIME || '')"],
    "keyword:CURRENT_TIMESTAMP": [
        "SELECT typeof(CURRENT_TIMESTAMP)", "SELECT length(CURRENT_TIMESTAMP)", "SELECT CURRENT_TIMESTAMP IS NOT NULL", "SELECT substr(CURRENT_TIMESTAMP, 11, 1)", "SELECT CURRENT_TIMESTAMP > '2000-01-01 00:00:00'",
        "SELECT datetime(CURRENT_TIMESTAMP) IS NOT NULL", "CREATE TABLE ks(a, ts DEFAULT CURRENT_TIMESTAMP)", "INSERT INTO ks(a) VALUES (1)", "SELECT length(ts), typeof(ts) FROM ks",
        "SELECT CURRENT_TIMESTAMP LIKE '____-__-__ __:__:__'", "SELECT unixepoch(CURRENT_TIMESTAMP) > 0", "SELECT date(CURRENT_TIMESTAMP) = CURRENT_DATE OR 1"],
    "keyword:DATABASE": [
        "ATTACH DATABASE ':memory:' AS kd_a", "ATTACH DATABASE '' AS kd_b", "ATTACH DATABASE ':memory:' AS kd_a", "CREATE TABLE kd_a.x(v)", "INSERT INTO kd_a.x VALUES (1)", "SELECT v FROM kd_a.x",
        "SELECT name FROM pragma_database_list WHERE name LIKE 'kd%' ORDER BY seq", "DETACH DATABASE kd_b", "DETACH DATABASE kd_b", "DETACH DATABASE kd_a", "ATTACH DATABASE ':memory:' AS \"kd c\"",
        "DETACH DATABASE \"kd c\"", "SELECT count(*) FROM pragma_database_list"],
    "keyword:TEMPORARY": [
        "CREATE TEMPORARY TABLE kt(a)", "INSERT INTO kt VALUES (1)", "SELECT a FROM temp.kt", "CREATE TEMPORARY VIEW kv AS SELECT a FROM kt", "SELECT a FROM kv",
        "CREATE TEMPORARY TRIGGER ktr AFTER INSERT ON kt BEGIN SELECT 1; END", "CREATE TEMPORARY TABLE IF NOT EXISTS kt(b)", "CREATE TEMPORARY TABLE kt2 AS SELECT a * 2 AS d FROM kt",
        "SELECT d FROM kt2", "SELECT name FROM sqlite_temp_schema ORDER BY name", "CREATE TEMPORARY TABLE main.kt3(a)", "CREATE TEMPORARY INDEX kti ON kt(a)", "DROP TABLE temp.kt2"],
    "keyword:EXCLUSIVE": [
        "BEGIN EXCLUSIVE", "INSERT INTO t2 VALUES (60, 'e', 1)", "COMMIT", "BEGIN EXCLUSIVE TRANSACTION", "SELECT count(*) FROM t2", "ROLLBACK", "PRAGMA locking_mode = EXCLUSIVE", "PRAGMA locking_mode",
        "BEGIN EXCLUSIVE", "BEGIN EXCLUSIVE", "END", "PRAGMA locking_mode = NORMAL", "BEGIN EXCLUSIVE", "UPDATE t2 SET c = 2 WHERE a = 60", "END TRANSACTION", "SELECT c FROM t2 WHERE a = 60"],
    "keyword:COMMIT": [
        "BEGIN", "INSERT INTO t2 VALUES (61, 'c', 1)", "COMMIT", "COMMIT", "BEGIN", "COMMIT TRANSACTION", "BEGIN IMMEDIATE", "UPDATE t2 SET c = 3 WHERE a = 61", "COMMIT", "SELECT c FROM t2 WHERE a = 61",
        "SAVEPOINT s", "INSERT INTO t2 VALUES (62, 's', 1)", "COMMIT", "SELECT count(*) FROM t2 WHERE a = 62", "BEGIN", "DELETE FROM t2 WHERE a > 60", "COMMIT", "SELECT count(*) FROM t2 WHERE a > 60"],
    "keyword:TRANSACTION": [
        "BEGIN TRANSACTION", "INSERT INTO t2 VALUES (63, 't', 1)", "COMMIT TRANSACTION", "BEGIN DEFERRED TRANSACTION", "ROLLBACK TRANSACTION", "BEGIN IMMEDIATE TRANSACTION", "END TRANSACTION",
        "BEGIN EXCLUSIVE TRANSACTION", "SAVEPOINT x", "ROLLBACK TRANSACTION TO SAVEPOINT x", "ROLLBACK TRANSACTION", "SELECT count(*) FROM t2 WHERE a = 63", "END TRANSACTION", "BEGIN TRANSACTION t_name", "COMMIT"],
    "keyword:FOR": [
        "CREATE TABLE kfr(a)", "CREATE TABLE kfl(n)", "CREATE TRIGGER kfr1 AFTER INSERT ON kfr FOR EACH ROW BEGIN INSERT INTO kfl VALUES (NEW.a); END",
        "CREATE TRIGGER kfr2 BEFORE DELETE ON kfr FOR EACH ROW WHEN OLD.a > 1 BEGIN INSERT INTO kfl VALUES (-OLD.a); END", "INSERT INTO kfr VALUES (1), (2), (3)", "DELETE FROM kfr WHERE a >= 2",
        "SELECT n FROM kfl ORDER BY rowid", "CREATE TRIGGER kfr3 AFTER UPDATE ON kfr FOR EACH ROW BEGIN INSERT INTO kfl VALUES (NEW.a * 100); END", "UPDATE kfr SET a = a + 1", "SELECT n FROM kfl ORDER BY rowid",
        "CREATE TRIGGER kfr4 AFTER INSERT ON kfr FOR EACH STATEMENT BEGIN SELECT 1; END", "CREATE TRIGGER kfr5 AFTER INSERT ON kfr FOR ROW BEGIN SELECT 1; END"],
    "keyword:EACH": [
        "CREATE TABLE ke(a)", "CREATE TABLE kel(n)", "CREATE TRIGGER ke1 AFTER INSERT ON ke FOR EACH ROW BEGIN INSERT INTO kel VALUES (NEW.a); END", "INSERT INTO ke VALUES (1), (2)",
        "CREATE TRIGGER ke2 BEFORE UPDATE ON ke FOR EACH ROW WHEN NEW.a > 10 BEGIN SELECT RAISE(IGNORE); END", "UPDATE ke SET a = a * 20 WHERE a = 1", "UPDATE ke SET a = 5 WHERE a = 2",
        "SELECT a FROM ke ORDER BY a", "CREATE TRIGGER ke3 AFTER DELETE ON ke FOR EACH ROW BEGIN DELETE FROM kel WHERE n = OLD.a; END", "DELETE FROM ke WHERE a = 1", "SELECT n FROM kel ORDER BY n",
        "CREATE TRIGGER ke4 AFTER DELETE ON ke FOR EACH BEGIN SELECT 1; END", "SELECT count(*) FROM sqlite_schema WHERE type = 'trigger'"],
    "keyword:INSTEAD": [
        "CREATE TABLE kin(a, b)", "CREATE VIEW kinv AS SELECT a, b FROM kin", "CREATE TRIGGER kin1 INSTEAD OF INSERT ON kinv BEGIN INSERT INTO kin VALUES (NEW.a, upper(NEW.b)); END",
        "INSERT INTO kinv VALUES (1, 'x')", "SELECT a, b FROM kin", "CREATE TRIGGER kin2 INSTEAD OF UPDATE ON kinv BEGIN UPDATE kin SET b = NEW.b || '!' WHERE a = OLD.a; END",
        "UPDATE kinv SET b = 'y' WHERE a = 1", "SELECT b FROM kin", "CREATE TRIGGER kin3 INSTEAD OF DELETE ON kinv BEGIN DELETE FROM kin WHERE a = OLD.a; END", "DELETE FROM kinv WHERE a = 1",
        "SELECT count(*) FROM kin", "CREATE TRIGGER kin4 INSTEAD OF INSERT ON kin BEGIN SELECT 1; END", "CREATE TRIGGER kin5 INSTEAD OF UPDATE OF b ON kinv BEGIN SELECT 1; END", "INSERT INTO kinv SELECT 2, 'z'",
        "SELECT a, b FROM kinv"],
    "keyword:OF": [
        "CREATE TABLE kof(a, b, c)", "CREATE TABLE kofl(n)", "CREATE TRIGGER kof1 AFTER UPDATE OF b ON kof BEGIN INSERT INTO kofl VALUES ('b'); END",
        "CREATE TRIGGER kof2 AFTER UPDATE OF a, c ON kof BEGIN INSERT INTO kofl VALUES ('a or c'); END", "INSERT INTO kof VALUES (1, 2, 3)", "UPDATE kof SET b = 20", "UPDATE kof SET a = 10", "UPDATE kof SET c = 30, b = 21",
        "SELECT n FROM kofl ORDER BY rowid", "CREATE VIEW kofv AS SELECT a, b FROM kof", "CREATE TRIGGER kof3 INSTEAD OF UPDATE OF b ON kofv BEGIN UPDATE kof SET b = NEW.b; END", "UPDATE kofv SET b = 99",
        "SELECT b FROM kof", "CREATE TRIGGER kof4 AFTER UPDATE OF nope ON kof BEGIN SELECT 1; END", "UPDATE kof SET a = 0", "CREATE TRIGGER kof5 AFTER INSERT OF a ON kof BEGIN SELECT 1; END"],
    "keyword:BEFORE": [
        "CREATE TABLE kbf(a)", "CREATE TABLE kbfl(n)", "CREATE TRIGGER kbf1 BEFORE INSERT ON kbf BEGIN INSERT INTO kbfl VALUES (NEW.a); END", "CREATE TRIGGER kbf2 BEFORE UPDATE ON kbf BEGIN INSERT INTO kbfl VALUES (OLD.a); END",
        "CREATE TRIGGER kbf3 BEFORE DELETE ON kbf BEGIN INSERT INTO kbfl VALUES (-OLD.a); END", "INSERT INTO kbf VALUES (1)", "UPDATE kbf SET a = 2", "DELETE FROM kbf", "SELECT n FROM kbfl ORDER BY rowid",
        "CREATE TRIGGER kbf4 BEFORE INSERT ON kbf WHEN NEW.a < 0 BEGIN SELECT RAISE(ABORT, 'negative'); END", "INSERT INTO kbf VALUES (-1)", "CREATE VIEW kbfv AS SELECT a FROM kbf",
        "CREATE TRIGGER kbf5 BEFORE INSERT ON kbfv BEGIN SELECT 1; END", "SELECT count(*) FROM kbf"],
    "keyword:RAISE": [
        "CREATE TABLE kr(a)", "CREATE TRIGGER kr1 BEFORE INSERT ON kr WHEN NEW.a = 1 BEGIN SELECT RAISE(IGNORE); END", "CREATE TRIGGER kr2 BEFORE INSERT ON kr WHEN NEW.a = 2 BEGIN SELECT RAISE(ABORT, 'two'); END",
        "CREATE TRIGGER kr3 BEFORE INSERT ON kr WHEN NEW.a = 3 BEGIN SELECT RAISE(FAIL, 'three'); END", "CREATE TRIGGER kr4 BEFORE INSERT ON kr WHEN NEW.a = 4 BEGIN SELECT RAISE(ROLLBACK, 'four'); END",
        "INSERT INTO kr VALUES (1)", "INSERT INTO kr VALUES (2)", "INSERT INTO kr VALUES (3)", "BEGIN", "INSERT INTO kr VALUES (5)", "INSERT INTO kr VALUES (4)", "SELECT count(*) FROM kr", "SELECT RAISE(ABORT, 'outside')",
        "CREATE TRIGGER kr5 BEFORE INSERT ON kr BEGIN SELECT CASE WHEN NEW.a > 100 THEN RAISE(ABORT, 'big') END; END", "INSERT INTO kr VALUES (101)", "INSERT INTO kr VALUES (50)", "SELECT a FROM kr",
        "CREATE TRIGGER kr6 BEFORE INSERT ON kr BEGIN SELECT RAISE(ABORT); END"],
    "keyword:RENAME": [
        "CREATE TABLE krn(a, b)", "ALTER TABLE krn RENAME TO krn2", "ALTER TABLE krn2 RENAME COLUMN a TO x", "ALTER TABLE krn2 RENAME b TO y", "SELECT name FROM pragma_table_info('krn2') ORDER BY cid",
        "CREATE VIEW krnv AS SELECT x FROM krn2", "ALTER TABLE krn2 RENAME COLUMN x TO z", "SELECT sql FROM sqlite_schema WHERE name = 'krnv'", "ALTER TABLE krn2 RENAME TO t1", "ALTER TABLE krn2 RENAME COLUMN nope TO w",
        "ALTER TABLE krn2 RENAME COLUMN z TO y", "CREATE INDEX krni ON krn2(z)", "ALTER TABLE krn2 RENAME TO krn3", "SELECT tbl_name FROM sqlite_schema WHERE name = 'krni'", "ALTER TABLE main.krn3 RENAME TO \"krn 4\""],
    "keyword:VACUUM": [
        "VACUUM", "VACUUM main", "VACUUM temp", "CREATE TABLE kvac(a)", "INSERT INTO kvac VALUES (1), (2)", "DELETE FROM kvac", "VACUUM", "SELECT count(*) FROM kvac", "BEGIN", "VACUUM", "ROLLBACK", "VACUUM nope",
        "PRAGMA auto_vacuum = FULL", "VACUUM", "PRAGMA auto_vacuum", "PRAGMA incremental_vacuum", "VACUUM \"main\"", "ATTACH ':memory:' AS kva", "VACUUM kva"],
    "keyword:ATTACH": [
        "ATTACH ':memory:' AS katt1", "ATTACH '' AS katt2", "ATTACH ':memory:' AS katt1", "ATTACH DATABASE ':memory:' AS katt3", "CREATE TABLE katt1.t(a)", "INSERT INTO katt1.t VALUES (1)",
        "SELECT a FROM katt1.t", "SELECT seq > 1, name FROM pragma_database_list WHERE name LIKE 'katt%' ORDER BY seq", "ATTACH ':memory:' AS main", "ATTACH ':memory:' AS temp", "ATTACH ':memory:' || '' AS katt4",
        "ATTACH ':memory:' AS \"katt 5\"", "SELECT count(*) FROM pragma_database_list"],
    "keyword:DETACH": [
        "ATTACH ':memory:' AS kdt1", "ATTACH ':memory:' AS kdt2", "DETACH kdt1", "DETACH kdt1", "DETACH DATABASE kdt2", "DETACH main", "DETACH temp", "ATTACH ':memory:' AS kdt3", "BEGIN",
        "CREATE TABLE kdt3.t(a)", "DETACH kdt3", "COMMIT", "DETACH kdt3", "SELECT count(*) FROM pragma_database_list", "DETACH nope"],
    "keyword:EXCEPT": [
        "SELECT a FROM t1 EXCEPT SELECT a FROM t2 ORDER BY 1", "SELECT b FROM t2 EXCEPT SELECT b FROM t1 ORDER BY 1", "SELECT 1 EXCEPT SELECT 1", "SELECT NULL EXCEPT SELECT NULL",
        "VALUES (1), (2), (3) EXCEPT VALUES (2)", "SELECT a, b FROM t1 EXCEPT SELECT a, b FROM t2 ORDER BY 1", "SELECT count(*) FROM (SELECT a FROM t1 EXCEPT SELECT a FROM t2)",
        "SELECT a FROM t1 EXCEPT SELECT a FROM t2 EXCEPT SELECT 4 ORDER BY 1", "SELECT a FROM t1 UNION SELECT 9 EXCEPT SELECT a FROM t2 ORDER BY 1", "SELECT 1, 2 EXCEPT SELECT 1",
        "SELECT a FROM t1 EXCEPT ALL SELECT a FROM t2", "SELECT a FROM t1 WHERE a > 2 EXCEPT SELECT a FROM t1 WHERE a > 4 ORDER BY 1 DESC LIMIT 1"],
    "keyword:INTERSECT": [
        "SELECT a FROM t1 INTERSECT SELECT a FROM t2 ORDER BY 1", "SELECT b FROM t2 INTERSECT SELECT b FROM t1", "SELECT 1 INTERSECT SELECT 1", "SELECT NULL INTERSECT SELECT NULL",
        "VALUES (1), (2) INTERSECT VALUES (2), (3)", "SELECT a, b FROM t1 INTERSECT SELECT a, b FROM t2", "SELECT count(*) FROM (SELECT a FROM t1 INTERSECT SELECT a FROM t2)",
        "SELECT a FROM t1 INTERSECT SELECT a FROM t2 INTERSECT SELECT 3", "SELECT a FROM t1 INTERSECT SELECT a FROM t2 UNION SELECT 99 ORDER BY 1", "SELECT 1 INTERSECT SELECT 1, 2",
        "SELECT a FROM t1 INTERSECT ALL SELECT a FROM t2", "SELECT a FROM t1 INTERSECT SELECT c / 10 FROM t2 ORDER BY 1"],
    "keyword:PLAN": [
        "EXPLAIN QUERY PLAN SELECT * FROM t1", "EXPLAIN QUERY PLAN SELECT * FROM t1 WHERE a = 1", "EXPLAIN QUERY PLAN SELECT * FROM t2 WHERE a = 1", "EXPLAIN QUERY PLAN SELECT t1.a FROM t1 JOIN t2 USING (a)",
        "EXPLAIN QUERY PLAN SELECT a FROM t1 ORDER BY a", "EXPLAIN QUERY PLAN SELECT count(*) FROM t1 GROUP BY b", "EXPLAIN QUERY PLAN SELECT a FROM t1 UNION SELECT a FROM t2",
        "EXPLAIN QUERY PLAN SELECT * FROM t1 WHERE a IN (SELECT a FROM t2)", "EXPLAIN QUERY PLAN UPDATE t2 SET c = 1 WHERE a = 2", "EXPLAIN QUERY PLAN DELETE FROM t1 WHERE b = 'x'",
        "EXPLAIN QUERY PLAN WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 3) SELECT * FROM c", "EXPLAIN PLAN SELECT 1"],
    "keyword:QUERY": [
        "EXPLAIN QUERY PLAN SELECT b FROM t1", "EXPLAIN QUERY PLAN SELECT b FROM t1 WHERE a > 2", "EXPLAIN QUERY PLAN SELECT max(a) FROM t1", "EXPLAIN QUERY PLAN SELECT * FROM t1 LEFT JOIN t2 USING (a)",
        "EXPLAIN QUERY PLAN SELECT DISTINCT b FROM t2", "EXPLAIN QUERY PLAN SELECT a FROM t1 WHERE EXISTS (SELECT 1 FROM t2 WHERE t2.a = t1.a)", "EXPLAIN QUERY PLAN INSERT INTO t2 SELECT * FROM t2",
        "EXPLAIN QUERY PLAN SELECT a, sum(a) OVER (ORDER BY a) FROM t1", "EXPLAIN QUERY PLAN SELECT * FROM t1 ORDER BY b LIMIT 2", "EXPLAIN QUERY PLAN SELECT * FROM t1, t2 WHERE t1.a = t2.a AND t1.b = t2.b",
        "EXPLAIN QUERY SELECT 1"],
    "keyword:MATERIALIZED": [
        "WITH x AS MATERIALIZED (SELECT a FROM t1) SELECT count(*) FROM x", "WITH x AS NOT MATERIALIZED (SELECT a FROM t1) SELECT count(*) FROM x",
        "WITH x AS MATERIALIZED (SELECT a FROM t1 WHERE a > 2), y AS NOT MATERIALIZED (SELECT a FROM x) SELECT sum(a) FROM y", "WITH x(v) AS MATERIALIZED (VALUES (1), (2)) SELECT max(v) FROM x",
        "WITH x AS MATERIALIZED (SELECT random() AS r) SELECT a.r = b.r FROM x AS a, x AS b", "WITH RECURSIVE c(n) AS MATERIALIZED (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 4) SELECT sum(n) FROM c",
        "WITH x AS NOT MATERIALIZED (SELECT a, b FROM t2) SELECT b FROM x WHERE a = 2", "WITH x AS MATERIALIZED (SELECT a FROM t1) SELECT t2.b FROM t2 JOIN x USING (a) ORDER BY 1",
        "WITH x AS MATERIALIZED (SELECT a FROM t1) INSERT INTO t2(a, b) SELECT a + 1000, 'm' FROM x WHERE a IS NOT NULL", "SELECT count(*) FROM t2 WHERE a > 1000", "WITH x AS MATERIALIZED SELECT 1 SELECT * FROM x"],
    "keyword:ASC": [
        "SELECT a FROM t1 ORDER BY a ASC", "SELECT b FROM t1 ORDER BY b ASC, a ASC", "SELECT a FROM t1 ORDER BY a ASC NULLS LAST", "SELECT a % 2, a FROM t1 ORDER BY 1 ASC, 2 DESC", "CREATE INDEX kasc ON t1(a ASC, b DESC)",
        "SELECT a FROM t1 WHERE a > 1 ORDER BY a ASC", "CREATE TABLE kas(k INTEGER PRIMARY KEY ASC, v)", "INSERT INTO kas VALUES (2, 'b'), (1, 'a')", "SELECT k FROM kas ORDER BY k ASC",
        "SELECT group_concat(a ORDER BY a ASC) FROM t1", "SELECT a, rank() OVER (ORDER BY c ASC) FROM t1 ORDER BY a", "CREATE TABLE kas2(a, b, PRIMARY KEY (a ASC, b))", "SELECT b FROM t2 ORDER BY b COLLATE NOCASE ASC, a"],
    "keyword:HAVING": [
        "SELECT a % 2, count(*) FROM t1 GROUP BY 1 HAVING count(*) > 2 ORDER BY 1", "SELECT b, count(*) FROM t2 GROUP BY b HAVING count(*) > 1", "SELECT c, sum(a) FROM t2 GROUP BY c HAVING sum(a) > 3 ORDER BY 1",
        "SELECT a % 3 AS g FROM t1 GROUP BY g HAVING g IS NOT NULL ORDER BY g", "SELECT count(*) FROM t1 HAVING count(*) > 0", "SELECT count(*) FROM t1 HAVING count(*) > 100",
        "SELECT a FROM t1 GROUP BY a HAVING max(c) > 2 ORDER BY a", "SELECT b, avg(c) FROM t1 GROUP BY b HAVING avg(c) BETWEEN 0 AND 5 ORDER BY b", "SELECT a % 2, max(b) FROM t1 GROUP BY 1 HAVING max(b) LIKE 't%'",
        "SELECT a FROM t1 GROUP BY a HAVING a IN (SELECT a FROM t2) ORDER BY 1", "SELECT a FROM t1 HAVING a > 1", "SELECT b FROM t2 GROUP BY b HAVING sum(c) IS NULL"],
    "keyword:LAST": [
        "SELECT a FROM t1 ORDER BY a NULLS LAST", "SELECT a FROM t1 ORDER BY a DESC NULLS LAST", "SELECT c FROM t1 ORDER BY c ASC NULLS LAST", "SELECT b FROM t1 ORDER BY b NULLS LAST, a",
        "SELECT a, c FROM t1 ORDER BY c NULLS LAST, a NULLS LAST", "SELECT group_concat(a, ',' ORDER BY a NULLS LAST) FROM t1", "SELECT a, row_number() OVER (ORDER BY c NULLS LAST) FROM t1 ORDER BY a",
        "SELECT a FROM t1 UNION SELECT NULL ORDER BY 1 NULLS LAST", "SELECT last_value(a) OVER (ORDER BY a NULLS LAST ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) FROM t1 LIMIT 1",
        "SELECT c FROM t2 ORDER BY c DESC NULLS LAST", "SELECT a FROM t1 ORDER BY a LAST"],
    "keyword:OFFSET": [
        "SELECT a FROM t1 ORDER BY a LIMIT 2 OFFSET 1", "SELECT a FROM t1 ORDER BY a LIMIT 100 OFFSET 5", "SELECT a FROM t1 ORDER BY a LIMIT -1 OFFSET 3", "SELECT a FROM t1 ORDER BY a LIMIT 2 OFFSET -1",
        "SELECT a FROM t1 ORDER BY a LIMIT 1 OFFSET 100", "SELECT b FROM t2 ORDER BY a LIMIT 1 OFFSET (SELECT count(*) FROM t2) - 1", "SELECT a FROM t1 UNION SELECT a FROM t2 ORDER BY 1 LIMIT 3 OFFSET 2",
        "SELECT a FROM t1 ORDER BY a LIMIT 2 OFFSET 1 + 1", "SELECT a FROM t1 ORDER BY a LIMIT 3, 2", "SELECT a FROM t1 ORDER BY a OFFSET 2", "SELECT a FROM t1 ORDER BY a LIMIT 1 OFFSET 'x'",
        "SELECT count(*) FROM (SELECT a FROM t1 LIMIT 3 OFFSET 2)"],
    "keyword:WINDOW": [
        "SELECT a, sum(a) OVER w FROM t1 WINDOW w AS (ORDER BY a) ORDER BY a", "SELECT a, rank() OVER w, dense_rank() OVER w FROM t1 WINDOW w AS (ORDER BY a % 3) ORDER BY a",
        "SELECT a, count(*) OVER w2 FROM t1 WINDOW w1 AS (PARTITION BY a % 2), w2 AS (w1 ORDER BY a) ORDER BY a", "SELECT a, max(c) OVER (w ROWS UNBOUNDED PRECEDING) FROM t1 WINDOW w AS (ORDER BY a) ORDER BY a",
        "SELECT a, lag(a) OVER w FROM t1 WINDOW w AS (ORDER BY a DESC) ORDER BY a", "SELECT a FROM t1 WINDOW w AS (ORDER BY a) ORDER BY a", "SELECT a, sum(a) OVER w FROM t1 WINDOW w AS (w ORDER BY a)",
        "SELECT a, sum(a) OVER w FROM t1 WINDOW v AS (ORDER BY a)", "SELECT a, avg(a) OVER w FROM t1 WINDOW w AS (PARTITION BY a % 2 ORDER BY a ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) ORDER BY a",
        "SELECT a % 2, sum(a), sum(sum(a)) OVER w FROM t1 GROUP BY 1 WINDOW w AS (ORDER BY a % 2) ORDER BY 1", "SELECT a, ntile(3) OVER w FROM t1 WINDOW w AS () ORDER BY a"],
    "keyword:ANALYZE": [
        "ANALYZE", "ANALYZE main", "ANALYZE t1", "ANALYZE main.t2", "CREATE INDEX kan ON t1(b)", "ANALYZE kan", "SELECT tbl, idx FROM sqlite_stat1 ORDER BY 1, 2", "ANALYZE temp", "ANALYZE nope",
        "ANALYZE sqlite_schema", "PRAGMA analysis_limit = 10", "ANALYZE t1", "SELECT count(*) FROM sqlite_stat1"],
    "keyword:RELEASE": [
        "SAVEPOINT r1", "RELEASE r1", "SAVEPOINT r2", "SAVEPOINT r3", "RELEASE SAVEPOINT r3", "RELEASE r2", "RELEASE r2", "SAVEPOINT r4", "INSERT INTO t2 VALUES (64, 'r', 1)", "RELEASE SAVEPOINT r4",
        "SELECT count(*) FROM t2 WHERE a = 64", "BEGIN", "SAVEPOINT r5", "RELEASE r5", "COMMIT", "RELEASE nope"],
    "keyword:VIRTUAL": [
        "CREATE VIRTUAL TABLE kvt1 USING fts5(x)", "INSERT INTO kvt1 VALUES ('alpha beta'), ('gamma')", "SELECT x FROM kvt1 WHERE kvt1 MATCH 'beta'", "CREATE VIRTUAL TABLE kvt2 USING rtree(id, a, b)",
        "INSERT INTO kvt2 VALUES (1, 0, 5)", "SELECT id FROM kvt2 WHERE a < 1", "CREATE VIRTUAL TABLE IF NOT EXISTS kvt1 USING fts5(y)", "CREATE TABLE kvg(a, b AS (a + 1) VIRTUAL)", "INSERT INTO kvg(a) VALUES (1)",
        "SELECT b FROM kvg", "CREATE VIRTUAL TABLE kvt3 USING nope(x)", "CREATE VIRTUAL TABLE temp.kvt4 USING fts4(z)", "DROP TABLE kvt1", "SELECT count(*) FROM sqlite_schema WHERE sql LIKE '%VIRTUAL%'"],
    "syntax:raise-function": [
        "CREATE TABLE rf(a)", "CREATE TRIGGER rf1 BEFORE INSERT ON rf WHEN NEW.a < 0 BEGIN SELECT RAISE(ABORT, 'negative'); END", "CREATE TRIGGER rf2 BEFORE UPDATE ON rf BEGIN SELECT RAISE(IGNORE) WHERE NEW.a = 0; END",
        "CREATE TRIGGER rf3 BEFORE DELETE ON rf WHEN OLD.a = 7 BEGIN SELECT RAISE(FAIL, 'keep seven'); END", "INSERT INTO rf VALUES (-1)", "INSERT INTO rf VALUES (7), (8)", "UPDATE rf SET a = 0",
        "SELECT a FROM rf ORDER BY a", "DELETE FROM rf", "SELECT a FROM rf ORDER BY a", "SELECT RAISE(IGNORE)", "SELECT RAISE(ROLLBACK, 'x')", "CREATE TRIGGER rf4 BEFORE INSERT ON rf BEGIN SELECT RAISE(ABORT, NEW.a); END",
        "INSERT INTO rf VALUES (3)"],
    "syntax:commit-stmt": [
        "BEGIN", "COMMIT", "BEGIN", "END", "BEGIN", "COMMIT TRANSACTION", "BEGIN", "END TRANSACTION", "COMMIT", "END", "BEGIN", "INSERT INTO t2 VALUES (65, 'k', 1)", "COMMIT TRANSACTION", "SELECT count(*) FROM t2 WHERE a = 65",
        "COMMIT nope", "END nope"],
    "syntax:window-defn": [
        "SELECT a, sum(a) OVER w FROM t1 WINDOW w AS (ORDER BY a) ORDER BY a", "SELECT a, sum(a) OVER w FROM t1 WINDOW w AS (PARTITION BY a % 2) ORDER BY a",
        "SELECT a, sum(a) OVER w FROM t1 WINDOW w AS (PARTITION BY a % 2 ORDER BY a ROWS BETWEEN 1 PRECEDING AND CURRENT ROW) ORDER BY a", "SELECT a, sum(a) OVER w2 FROM t1 WINDOW w1 AS (ORDER BY a), w2 AS (w1) ORDER BY a",
        "SELECT a, sum(a) OVER (w1 ROWS 2 PRECEDING) FROM t1 WINDOW w1 AS (ORDER BY a) ORDER BY a", "SELECT a, count(*) OVER w FROM t1 WINDOW w AS () ORDER BY a",
        "SELECT a, rank() OVER w FROM t1 WINDOW w AS (ORDER BY c DESC NULLS LAST) ORDER BY a", "SELECT a, sum(a) OVER w FROM t1 WINDOW w AS (ORDER BY a GROUPS 1 PRECEDING EXCLUDE TIES) ORDER BY a",
        "SELECT a, sum(a) OVER w2 FROM t1 WINDOW w1 AS (ORDER BY a ROWS 1 PRECEDING), w2 AS (w1)", "SELECT a, sum(a) OVER w FROM t1 WINDOW w AS (PARTITION BY a), w AS (ORDER BY a)",
        "SELECT a, first_value(b) OVER w FROM t1 WINDOW w AS (ORDER BY b COLLATE NOCASE) ORDER BY a"],
}


main()
