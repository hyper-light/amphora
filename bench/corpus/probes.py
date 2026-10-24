#!/usr/bin/env python3
"""Probes: for each entry of the SQLite language's inventory (inventory.py), one or more small,
deterministic statements in SQLite's dialect that exercise it, run on every engine by probe.py to
say which support it and how their answers compare with SQLite's.

Each probe: an id (`<inventory id>` or `<inventory id>#<variant>`), the statements that set it up
(`setup`, run first, their answers ignored) and the query whose answer is compared. A probe's
setup works on the tables `t` (a INTEGER, b TEXT, c REAL) and `u` (a INTEGER, d TEXT) unless it
makes its own.

usage: python3 probes.py <features.json> <out probes.json>
"""
import json
import sys

BASE = [
    "CREATE TABLE t(a INTEGER, b TEXT, c REAL)",
    "INSERT INTO t VALUES (1, 'one', 1.5), (2, 'two', 2.5), (3, 'three', NULL), (4, NULL, 4.5), (5, 'five', 5.5)",
    "CREATE TABLE u(a INTEGER, d TEXT)",
    "INSERT INTO u VALUES (1, 'x'), (2, 'y'), (2, 'yy'), (6, 'z')",
]

P = []


def probe(pid, query, setup=None, base=True, order=True, params=None, requires=None, expect=None, sqlite_setup=None, compare=None):
    """`order`: the query's rows have a defined order (an ORDER BY, or one row); unordered answers
    are compared as multisets. `params`: values bound to the query (a list for positional, a dict
    for named parameters). `requires`: the compile-time option SQLite documents the entry behind
    (off in default builds). `expect`: "error" when SQLite's answer is a refusal (a STRICT table
    turning a value away): an engine that refuses too answers as SQLite does. `sqlite_setup`: what
    SQLite's dialect needs first to behave as the others do by default (foreign keys enforced), run
    by the SQLite-dialect engines only. `compare`: "nonempty" or "shape" for an answer each
    implementation words its own way (EXPLAIN's program, a build's lists, a file's path)."""
    p = {"id": pid, "setup": (BASE if base else []) + (setup or []), "base_len": len(BASE) if base else 0, "query": query, "ordered": order}
    if expect:
        p["expect"] = expect
    if sqlite_setup:
        p["sqlite_setup"] = sqlite_setup
    if compare:
        p["compare"] = compare
    if params is not None:
        p["params"] = params
    if requires:
        p["requires"] = requires
    P.append(p)


# ---------------------------------------------------------------- core functions
core = {
    "abs": "SELECT abs(-3), abs(2.5), abs(NULL)",
    "changes": "SELECT changes()",
    "char": "SELECT char(72, 105)",
    "coalesce": "SELECT coalesce(NULL, NULL, 3)",
    "concat": "SELECT concat('a', NULL, 'b', 1)",
    "concat_ws": "SELECT concat_ws('-', 'a', NULL, 'b')",
    "format": "SELECT format('%d-%s', 7, 'x')",
    "glob": "SELECT glob('a*', 'abc'), glob('a*', 'bc')",
    "hex": "SELECT hex('AB'), hex(255)",
    "if": "SELECT if(1 > 2, 'a', 'b')",
    "ifnull": "SELECT ifnull(NULL, 'z')",
    "iif": "SELECT iif(1 < 2, 'yes', 'no')",
    "instr": "SELECT instr('hello', 'll')",
    "last_insert_rowid": "SELECT last_insert_rowid() > 0",
    "length": "SELECT length('héllo'), length(x'0102'), length(NULL)",
    "like": "SELECT like('a%', 'abc'), like('a\\%', 'a%', '\\')",
    "likelihood": "SELECT likelihood(1 = 1, 0.5)",
    "likely": "SELECT likely(1 = 1)",
    "load_extension": "SELECT load_extension('/nonexistent/probe-extension')",
    "lower": "SELECT lower('AbC')",
    "ltrim": "SELECT ltrim('  ab'), ltrim('xxab', 'x')",
    "max": "SELECT max(1, 5, 3)",
    "min": "SELECT min(4, 2, 8)",
    "nullif": "SELECT nullif(3, 3), nullif(3, 4)",
    "octet_length": "SELECT octet_length('héllo')",
    "printf": "SELECT printf('%5.2f|%-3s|%03d', 3.14159, 'a', 7)",
    "quote": "SELECT quote('it''s'), quote(NULL), quote(x'ab')",
    "random": "SELECT typeof(random())",
    "randomblob": "SELECT length(randomblob(8))",
    "replace": "SELECT replace('banana', 'an', 'AN')",
    "round": "SELECT round(2.567, 2), round(2.5), round(-2.5)",
    "rtrim": "SELECT rtrim('ab  '), rtrim('abyy', 'y')",
    "sign": "SELECT sign(-7), sign(0), sign(2.5)",
    "soundex": "SELECT soundex('Robert')",
    "sqlite_compileoption_get": "SELECT typeof(sqlite_compileoption_get(0))",
    "sqlite_compileoption_used": "SELECT typeof(sqlite_compileoption_used('THREADSAFE'))",
    "sqlite_offset": "SELECT typeof(sqlite_offset(a)) FROM t LIMIT 1",
    "sqlite_source_id": "SELECT typeof(sqlite_source_id())",
    "sqlite_version": "SELECT typeof(sqlite_version())",
    "substr": "SELECT substr('database', 2, 3), substr('database', -4), substr('database', 5)",
    "substring": "SELECT substring('database', 2, 3)",
    "total_changes": "SELECT typeof(total_changes())",
    "trim": "SELECT trim('  ab  '), trim('xxabxx', 'x')",
    "typeof": "SELECT typeof(1), typeof(1.5), typeof('a'), typeof(x'00'), typeof(NULL)",
    "unhex": "SELECT unhex('4142') = CAST('AB' AS BLOB)",
    "unicode": "SELECT unicode('A')",
    "unistr": "SELECT unistr('\\u0041\\u00e9')",
    "unistr_quote": "SELECT unistr_quote('a''b')",
    "unlikely": "SELECT unlikely(1 = 2)",
    "upper": "SELECT upper('aBc')",
    "zeroblob": "SELECT length(zeroblob(4)), typeof(zeroblob(1))",
}
REQUIRES = {"soundex": "SQLITE_SOUNDEX", "sqlite_offset": "SQLITE_ENABLE_OFFSET_SQL_FUNC"}
for name, q in core.items():
    probe(f"function.core:{name}", q, requires=REQUIRES.get(name), expect="error" if name == "load_extension" else None)

# ---------------------------------------------------------------- math functions
math = {
    "acos": "SELECT round(acos(0.5), 6)", "acosh": "SELECT round(acosh(2), 6)", "asin": "SELECT round(asin(0.5), 6)",
    "asinh": "SELECT round(asinh(1), 6)", "atan": "SELECT round(atan(1), 6)", "atan2": "SELECT round(atan2(1, 2), 6)",
    "atanh": "SELECT round(atanh(0.5), 6)", "ceil": "SELECT ceil(2.1), ceil(-2.1)", "ceiling": "SELECT ceiling(2.1)",
    "cos": "SELECT round(cos(1), 6)", "cosh": "SELECT round(cosh(1), 6)", "degrees": "SELECT round(degrees(pi()), 6)",
    "exp": "SELECT round(exp(1), 6)", "floor": "SELECT floor(2.9), floor(-2.1)", "ln": "SELECT round(ln(10), 6)",
    "log": "SELECT round(log(100), 6), round(log(2, 8), 6)", "log10": "SELECT round(log10(1000), 6)", "log2": "SELECT round(log2(8), 6)",
    "mod": "SELECT mod(7, 3), mod(7.5, 2)", "pi": "SELECT round(pi(), 6)", "pow": "SELECT pow(2, 10)", "power": "SELECT power(3, 3)",
    "radians": "SELECT round(radians(180), 6)", "sin": "SELECT round(sin(1), 6)", "sinh": "SELECT round(sinh(1), 6)",
    "sqrt": "SELECT sqrt(16), sqrt(2.25)", "tan": "SELECT round(tan(1), 6)", "tanh": "SELECT round(tanh(1), 6)",
    "trunc": "SELECT trunc(2.7), trunc(-2.7)",
}
for name, q in math.items():
    probe(f"function.math:{name}", q)

# ---------------------------------------------------------------- date and time functions and modifiers
dt = {
    "date": "SELECT date('2024-02-28', '+1 day'), date(0, 'unixepoch')",
    "time": "SELECT time('2024-01-01 13:45:30'), time('13:45', '+30 minutes')",
    "datetime": "SELECT datetime('2024-01-31 10:00:00', '+1 month'), datetime(1700000000, 'unixepoch')",
    "julianday": "SELECT julianday('2000-01-01 12:00:00')",
    "unixepoch": "SELECT unixepoch('2024-01-01 00:00:00')",
    "strftime": "SELECT strftime('%Y-%m-%d %H:%M %j %w', '2024-03-15 08:05:00')",
    "timediff": "SELECT timediff('2024-03-15', '2024-01-01')",
}
for name, q in dt.items():
    probe(f"function.datetime:{name}", q)
modifiers = {
    "NNN days": "SELECT date('2024-01-01', '+10 days')", "NNN hours": "SELECT datetime('2024-01-01', '+5 hours')",
    "NNN minutes": "SELECT datetime('2024-01-01', '-90 minutes')", "NNN seconds": "SELECT datetime('2024-01-01', '+3600 seconds')",
    "NNN months": "SELECT date('2024-01-31', '+1 months')", "NNN years": "SELECT date('2024-02-29', '+1 years')",
    "start of month": "SELECT date('2024-03-15', 'start of month')", "start of year": "SELECT date('2024-03-15', 'start of year')",
    "start of day": "SELECT datetime('2024-03-15 10:11:12', 'start of day')", "weekday N": "SELECT date('2024-03-15', 'weekday 0')",
    "unixepoch": "SELECT datetime(1710460800, 'unixepoch')", "julianday": "SELECT date(2460385.5, 'julianday')",
    "auto": "SELECT datetime(1710460800, 'auto')", "localtime": "SELECT typeof(datetime('2024-03-15', 'localtime'))",
    "utc": "SELECT typeof(datetime('2024-03-15', 'utc'))", "subsec": "SELECT typeof(datetime('now', 'subsec'))",
    "subsecond": "SELECT typeof(datetime('now', 'subsecond'))", "ceiling": "SELECT date('2024-01-31', '+1 month', 'ceiling')",
    "floor": "SELECT date('2024-01-31', '+1 month', 'floor')",
}
for name, q in modifiers.items():
    probe(f"datetime.modifier:{name}", q)

# ---------------------------------------------------------------- JSON functions
J = "'{\"a\":1,\"b\":[1,2,{\"c\":3}],\"d\":null}'"
js = {
    "json": f"SELECT json(' {{\"x\" : 1}} ')", "json_array": "SELECT json_array(1, 'a', NULL, 2.5)",
    "json_array_insert": "SELECT json_array_insert('[1,3]', '$[1]', 2)", "json_array_length": f"SELECT json_array_length({J}, '$.b')",
    "json_each": f"SELECT key, value, type FROM json_each({J}) ORDER BY key", "json_error_position": "SELECT json_error_position('{\"a\":}')",
    "json_extract": f"SELECT json_extract({J}, '$.b[2].c'), json_extract({J}, '$.a', '$.d')", "json_group_array": "SELECT json_group_array(a) FROM (SELECT a FROM t ORDER BY a)",
    "json_group_object": "SELECT json_group_object(b, a) FROM (SELECT a, b FROM t WHERE b IS NOT NULL ORDER BY a)", "json_insert": f"SELECT json_insert({J}, '$.e', 5, '$.a', 9)",
    "json_object": "SELECT json_object('k', 1, 'v', 'x')", "json_patch": "SELECT json_patch('{\"a\":1,\"b\":2}', '{\"b\":null,\"c\":3}')",
    "json_pretty": "SELECT json_pretty('{\"a\":[1,2]}')", "json_quote": "SELECT json_quote('a\"b'), json_quote(3)",
    "json_remove": f"SELECT json_remove({J}, '$.b', '$.d')", "json_replace": f"SELECT json_replace({J}, '$.a', 7, '$.zz', 1)",
    "json_set": f"SELECT json_set({J}, '$.a', 8, '$.e', 'n')", "json_tree": f"SELECT fullkey, type FROM json_tree({J}) ORDER BY fullkey",
    "json_type": f"SELECT json_type({J}, '$.b'), json_type({J}, '$.d')", "json_valid": "SELECT json_valid('{\"a\":1}'), json_valid('{a:1}'), json_valid('{a:1}', 2)",
    "jsonb": "SELECT json(jsonb('{\"a\":1}'))", "jsonb_array": "SELECT json(jsonb_array(1, 2))",
    "jsonb_array_insert": "SELECT json(jsonb_array_insert('[1,3]', '$[1]', 2))", "jsonb_each": f"SELECT key FROM jsonb_each(jsonb({J})) ORDER BY key",
    "jsonb_extract": f"SELECT json(jsonb_extract(jsonb({J}), '$.b'))", "jsonb_group_array": "SELECT json(jsonb_group_array(a)) FROM (SELECT a FROM t ORDER BY a)",
    "jsonb_group_object": "SELECT json(jsonb_group_object(b, a)) FROM (SELECT a, b FROM t WHERE b IS NOT NULL ORDER BY a)", "jsonb_insert": f"SELECT json(jsonb_insert({J}, '$.e', 5))",
    "jsonb_object": "SELECT json(jsonb_object('k', 1))", "jsonb_patch": "SELECT json(jsonb_patch('{\"a\":1}', '{\"b\":2}'))",
    "jsonb_remove": f"SELECT json(jsonb_remove({J}, '$.b'))", "jsonb_replace": f"SELECT json(jsonb_replace({J}, '$.a', 2))",
    "jsonb_set": f"SELECT json(jsonb_set({J}, '$.a', 3))", "jsonb_tree": f"SELECT count(*) FROM jsonb_tree(jsonb({J}))",
}
for name, q in js.items():
    probe(f"function.json:{name}", q, order=("ORDER BY" in q or "count(" in q or "FROM" not in q or "group_" in q))

# ---------------------------------------------------------------- aggregate functions (with FILTER and ORDER BY in aggregates)
agg = {
    "avg": "SELECT avg(c), avg(a) FROM t", "count": "SELECT count(*), count(b), count(DISTINCT a % 2) FROM t",
    "group_concat": "SELECT group_concat(b, '|' ORDER BY a) FROM t", "string_agg": "SELECT string_agg(b, ',' ORDER BY a DESC) FROM t",
    "max": "SELECT max(c), max(b) FROM t", "min": "SELECT min(c), min(b) FROM t", "sum": "SELECT sum(a), sum(c) FROM t",
    "total": "SELECT total(c), total(NULL) FROM t", "median": "SELECT median(c) FROM t", "percentile": "SELECT percentile(c, 50) FROM t",
    "percentile_cont": "SELECT percentile_cont(c, 0.25) FROM t", "percentile_disc": "SELECT percentile_disc(c, 0.25) FROM t",
}
for name, q in agg.items():
    probe(f"function.aggregate:{name}", q)

# ---------------------------------------------------------------- window functions
W = "OVER (ORDER BY a)"
win = {
    "row_number": f"SELECT a, row_number() {W} FROM t ORDER BY a", "rank": "SELECT a, rank() OVER (ORDER BY a % 2) FROM t ORDER BY a",
    "dense_rank": "SELECT a, dense_rank() OVER (ORDER BY a % 2) FROM t ORDER BY a", "percent_rank": f"SELECT a, percent_rank() {W} FROM t ORDER BY a",
    "cume_dist": f"SELECT a, cume_dist() {W} FROM t ORDER BY a", "ntile": f"SELECT a, ntile(2) {W} FROM t ORDER BY a",
    "lag": f"SELECT a, lag(b) {W}, lag(b, 2, 'none') {W} FROM t ORDER BY a", "lead": f"SELECT a, lead(b) {W} FROM t ORDER BY a",
    "first_value": f"SELECT a, first_value(b) {W} FROM t ORDER BY a", "last_value": "SELECT a, last_value(b) OVER (ORDER BY a ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) FROM t ORDER BY a",
    "nth_value": "SELECT a, nth_value(b, 2) OVER (ORDER BY a ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) FROM t ORDER BY a",
}
for name, q in win.items():
    probe(f"function.window:{name}", q)
frames = {
    "rows": "SELECT a, sum(a) OVER (ORDER BY a ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) FROM t ORDER BY a",
    "range": "SELECT a, sum(a) OVER (ORDER BY a RANGE BETWEEN 1 PRECEDING AND CURRENT ROW) FROM t ORDER BY a",
    "groups": "SELECT a, count(*) OVER (ORDER BY a % 2 GROUPS BETWEEN CURRENT ROW AND 1 FOLLOWING) FROM t ORDER BY a",
    "exclude-current-row": "SELECT a, sum(a) OVER (ORDER BY a ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE CURRENT ROW) FROM t ORDER BY a",
    "exclude-group": "SELECT a, count(*) OVER (ORDER BY a % 2 ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE GROUP) FROM t ORDER BY a",
    "exclude-ties": "SELECT a, count(*) OVER (ORDER BY a % 2 ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE TIES) FROM t ORDER BY a",
    "partition": "SELECT a, sum(a) OVER (PARTITION BY a % 2 ORDER BY a) FROM t ORDER BY a",
    "named-window": "SELECT a, sum(a) OVER w, count(*) OVER w FROM t WINDOW w AS (ORDER BY a) ORDER BY a",
    "window-chaining": "SELECT a, sum(a) OVER (w ROWS 1 PRECEDING) FROM t WINDOW w AS (ORDER BY a) ORDER BY a",
    "aggregate-filter": "SELECT a, count(*) FILTER (WHERE a > 2) OVER (ORDER BY a) FROM t ORDER BY a",
}
for name, q in frames.items():
    probe(f"syntax:frame-spec#{name}", q)

# ---------------------------------------------------------------- operators
ops = {
    "~": "SELECT ~5", "+ (unary)": "SELECT +3, +'x'", "- (unary)": "SELECT -(3), -'4'", "+": "SELECT 1 + 2, 1.5 + 2", "-": "SELECT 5 - 7, 2.5 - 1", "COLLATE": "SELECT 'a' = 'A' COLLATE NOCASE",
    "||": "SELECT 'a' || 'b' || 1", "->": "SELECT '{\"a\":{\"b\":2}}' -> '$.a'", "->>": "SELECT '{\"a\":{\"b\":2}}' ->> '$.a.b'",
    "*": "SELECT 6 * 7", "/": "SELECT 7 / 2, 7.0 / 2", "%": "SELECT 7 % 3", "&": "SELECT 6 & 3", "|": "SELECT 6 | 3",
    "<<": "SELECT 1 << 4", ">>": "SELECT 256 >> 2", "<": "SELECT 1 < 2", ">": "SELECT 2 > 1", "<=": "SELECT 2 <= 2", ">=": "SELECT 2 >= 3",
    "=": "SELECT 1 = 1", "==": "SELECT 1 == 1", "<>": "SELECT 1 <> 2", "!=": "SELECT 1 != 1", "IS": "SELECT NULL IS NULL, 1 IS 1",
    "IS NOT": "SELECT 1 IS NOT NULL", "IS DISTINCT FROM": "SELECT 1 IS DISTINCT FROM NULL", "IS NOT DISTINCT FROM": "SELECT NULL IS NOT DISTINCT FROM NULL",
    "IN": "SELECT 2 IN (1, 2, 3), 5 IN (SELECT a FROM t)", "NOT IN": "SELECT 9 NOT IN (1, 2)", "LIKE": "SELECT 'Abc' LIKE 'a%'",
    "NOT LIKE": "SELECT 'abc' NOT LIKE 'b%'", "GLOB": "SELECT 'abc' GLOB 'a?c'", "NOT GLOB": "SELECT 'abc' NOT GLOB 'A*'",
    "REGEXP": "SELECT 'abc' REGEXP 'b', 'abc' REGEXP '^b'", "NOT REGEXP": "SELECT 'abc' NOT REGEXP 'z'", "ESCAPE": "SELECT 'a%' LIKE 'a!%' ESCAPE '!'",
    "BETWEEN": "SELECT 2 BETWEEN 1 AND 3", "NOT BETWEEN": "SELECT 5 NOT BETWEEN 1 AND 3", "ISNULL": "SELECT NULL ISNULL",
    "NOTNULL": "SELECT 1 NOTNULL", "NOT NULL": "SELECT 1 NOT NULL", "EXISTS": "SELECT EXISTS (SELECT 1 FROM t WHERE a = 3)", "NOT EXISTS": "SELECT NOT EXISTS (SELECT 1 FROM t WHERE a = 9)",
    "CASE": "SELECT CASE WHEN 1 > 2 THEN 'a' WHEN 2 > 1 THEN 'b' ELSE 'c' END, CASE 3 WHEN 3 THEN 'x' END",
}
for name, q in ops.items():
    probe(f"operator:{name}", q)
# NOT, AND and OR on SQLite's integers (engines with a boolean type refuse them), then on booleans.
probe("operator:NOT#1", "SELECT NOT 0, NOT 5")
probe("operator:NOT#2", "SELECT NOT FALSE, NOT TRUE")
probe("operator:AND#1", "SELECT 1 AND 0, 1 AND NULL")
probe("operator:AND#2", "SELECT TRUE AND FALSE, TRUE AND NULL")
probe("operator:OR#1", "SELECT 0 OR NULL, 1 OR NULL")
probe("operator:OR#2", "SELECT FALSE OR NULL, TRUE OR NULL")
# CAST: a plain cast, a real's truncation, and SQLite's reading of text by its numeric prefix.
probe("operator:CAST#1", "SELECT CAST(5 AS TEXT), CAST('7' AS INTEGER), CAST(2 AS REAL)")
probe("operator:CAST#2", "SELECT CAST(3.9 AS INTEGER), CAST(-3.9 AS INTEGER)")
probe("operator:CAST#3", "SELECT CAST('12abc' AS INTEGER), CAST('abc' AS INTEGER)")
# MATCH calls the match() function a virtual table (FTS5 here) supplies; SQLite's own default raises
# an error, so NOT MATCH outside such a table needs an application-defined match().
FTS = ["CREATE VIRTUAL TABLE ft USING fts5(body)", "INSERT INTO ft VALUES ('hello world'), ('goodbye')"]
probe("operator:MATCH", "SELECT body FROM ft WHERE ft MATCH 'hello'", setup=FTS, base=False)
probe("operator:NOT MATCH", "SELECT 'abc' NOT MATCH 'x'", requires="an application-defined match() function")

# ---------------------------------------------------------------- types, affinities, collations
probe("affinity:TEXT", "SELECT typeof(x) FROM (SELECT CAST(12 AS TEXT) AS x)")
probe("affinity:NUMERIC", "SELECT typeof(CAST('12' AS NUMERIC)), typeof(CAST('1.5' AS NUMERIC)), typeof(CAST('abc' AS NUMERIC))")
probe("affinity:INTEGER", "SELECT typeof(v) FROM (SELECT CAST('7' AS INTEGER) AS v)")
probe("affinity:REAL", "SELECT typeof(CAST(7 AS REAL))")
probe("affinity:BLOB", "SELECT typeof(CAST('ab' AS BLOB))")
probe("affinity:column", "SELECT typeof(n), typeof(s), typeof(i) FROM ta", setup=["CREATE TABLE ta(n NUMERIC, s TEXT, i INT)", "INSERT INTO ta VALUES ('5', 5, '7')"], base=False)
probe("collation:BINARY", "SELECT 'a' < 'B' COLLATE BINARY")
probe("collation:NOCASE", "SELECT b FROM t WHERE b = 'ONE' COLLATE NOCASE")
probe("collation:RTRIM", "SELECT 'a  ' = 'a' COLLATE RTRIM")
for cls, lit in [("NULL", "NULL"), ("INTEGER", "1"), ("REAL", "1.5"), ("TEXT", "'t'"), ("BLOB", "x'01'")]:
    probe(f"storage_class:{cls}", f"SELECT typeof({lit})")

# ---------------------------------------------------------------- expression forms with sections of their own
probe("expression:scalar subquery", "SELECT (SELECT max(a) FROM t), (SELECT b FROM t WHERE a = 2)")
probe("expression:correlated subquery", "SELECT a, (SELECT count(*) FROM u WHERE u.a = t.a) FROM t ORDER BY a")
probe("expression:row value", "SELECT (1, 2) = (1, 2), (1, 2) < (1, 3), (2, 'y') IN (SELECT a, d FROM u)")
probe("expression:boolean expression", "SELECT CASE WHEN 'abc' THEN 1 ELSE 0 END, CASE WHEN '1x' THEN 1 ELSE 0 END, CASE WHEN 0.5 THEN 1 ELSE 0 END, NOT 'abc'")

# ---------------------------------------------------------------- the datatype page's rules, each shown on values it decides
R = "CREATE TABLE r"
rules = {
    "rule.affinity:INT gives INTEGER": ([f"{R}(x BIGINT, y MEDIUMINT)", "INSERT INTO r VALUES ('12', '13')"], "SELECT typeof(x), typeof(y) FROM r"),
    "rule.affinity:CHAR, CLOB or TEXT gives TEXT": ([f"{R}(a VARCHAR(10), b CLOB, c NTEXT)", "INSERT INTO r VALUES (12, 13, 14)"], "SELECT typeof(a), typeof(b), typeof(c) FROM r"),
    "rule.affinity:BLOB or no type gives BLOB": ([f"{R}(a BLOB, b)", "INSERT INTO r VALUES ('12', '13')"], "SELECT typeof(a), typeof(b) FROM r"),
    "rule.affinity:REAL, FLOA or DOUB gives REAL": ([f"{R}(a REAL, b FLOAT, c DOUBLE PRECISION)", "INSERT INTO r VALUES ('1', '2', 3)"], "SELECT typeof(a), typeof(b), typeof(c) FROM r"),
    "rule.affinity:otherwise NUMERIC": ([f"{R}(a DECIMAL(10, 5), b BOOLEAN, c DATE)", "INSERT INTO r VALUES ('1.0', '2', '2024-01-01')"], "SELECT typeof(a), a, typeof(b), typeof(c) FROM r"),
    "rule.affinity:the rules apply in order": ([f"{R}(a CHARINT, b FLOATING POINT)", "INSERT INTO r VALUES ('5', '6')"], "SELECT typeof(a), typeof(b) FROM r"),
    "rule.expression-affinity:an IN list has no affinity": ([f"{R}(t TEXT)", "INSERT INTO r VALUES ('1')"], "SELECT 1 IN (SELECT t FROM r), 1 IN ('1')"),
    "rule.expression-affinity:a column keeps its affinity": ([f"{R}(n INTEGER)", "INSERT INTO r VALUES (5)"], "SELECT n = '5', (n) = '5' FROM r"),
    "rule.expression-affinity:an operator removes affinity": ([f"{R}(n INTEGER)", "INSERT INTO r VALUES (5)"], "SELECT +n = '5', n + 0 = '5' FROM r"),
    "rule.expression-affinity:CAST gives its type's affinity": ([], "SELECT CAST(5 AS TEXT) = 5, CAST('5' AS INTEGER) = '5'"),
    "rule.expression-affinity:COLLATE keeps its operand's affinity": ([f"{R}(n INTEGER)", "INSERT INTO r VALUES (5)"], "SELECT (n COLLATE NOCASE) = '5' FROM r"),
    "rule.expression-affinity:otherwise no affinity": ([], "SELECT 5 = '5', '5' = 5"),
    "rule.expression-affinity:view and subquery columns": ([f"{R}(a INT)", "INSERT INTO r VALUES (1)", "CREATE VIEW rv(x, y) AS SELECT a, a + 0 FROM r"], "SELECT x = '1', y = '1' FROM rv"),
    "rule.expression-affinity:compound view columns": ([f"{R}(a INT)", "CREATE TABLE s(b INT)", "INSERT INTO r VALUES (1)", "INSERT INTO s VALUES (2)"], "SELECT count(*) FROM (SELECT a AS c FROM r UNION ALL SELECT b FROM s) WHERE c = '2'"),
    "rule.comparison:NULL is less than any value": ([], "SELECT x FROM (SELECT 1 AS x UNION ALL SELECT NULL UNION ALL SELECT -5) ORDER BY x"),
    "rule.comparison:numbers before text and blobs": ([], "SELECT 1 < '0', 2 < x'01', 99.5 < ''"),
    "rule.comparison:numbers compare numerically": ([], "SELECT 2 < 10, 2.5 < 3, 3 = 3.0"),
    "rule.comparison:text before blobs": ([], "SELECT 'zzz' < x'00'"),
    "rule.comparison:text compares by collation": ([], "SELECT 'a' < 'B', 'a' < 'B' COLLATE NOCASE"),
    "rule.comparison:blobs compare by memcmp": ([], "SELECT x'0102' < x'0201', x'01' < x'0100'"),
    "rule.comparison:numeric affinity goes to the other operand": ([f"{R}(n NUMERIC)", "INSERT INTO r VALUES ('10')"], "SELECT n > '9', n = '10.0' FROM r"),
    "rule.comparison:text affinity goes to an operand with none": ([f"{R}(t TEXT)", "INSERT INTO r VALUES ('10')"], "SELECT t = 10, t > 9 FROM r"),
    "rule.comparison:otherwise compared as is": ([], "SELECT '10' = 10, x'31' = '1'"),
    "rule.comparison:BETWEEN is two comparisons": ([f"{R}(t TEXT)", "INSERT INTO r VALUES ('5')"], "SELECT t BETWEEN 1 AND 9, t BETWEEN 10 AND 20 FROM r"),
    "rule.comparison:IN compares as equality": ([f"{R}(n INTEGER)", "INSERT INTO r VALUES (1)", "CREATE TABLE s(t TEXT)", "INSERT INTO s VALUES ('1')"], "SELECT (SELECT n IN (SELECT t FROM s) FROM r), '1' IN (SELECT n FROM r)"),
    "rule.sort:ORDER BY orders by storage class": ([], "SELECT x FROM (SELECT x'00' AS x UNION ALL SELECT 'b' UNION ALL SELECT 2 UNION ALL SELECT NULL UNION ALL SELECT 1.5 UNION ALL SELECT 'A') ORDER BY x"),
    "rule.sort:GROUP BY keeps classes apart": ([], "SELECT count(*) AS n FROM (SELECT 1 AS x UNION ALL SELECT 1.0 UNION ALL SELECT '1') GROUP BY x ORDER BY n"),
    "rule.sort:compound SELECTs compare as is": ([f"{R}(n INTEGER)", "INSERT INTO r VALUES (1)"], "SELECT count(*) FROM (SELECT n FROM r UNION SELECT '1')"),
    "rule.collation:columns default to BINARY": ([f"{R}(t TEXT)", "INSERT INTO r VALUES ('a')"], "SELECT t = 'A' FROM r"),
    "rule.collation:an explicit COLLATE wins, left first": ([f"{R}(t TEXT COLLATE NOCASE)", "INSERT INTO r VALUES ('a')"], "SELECT t = 'A' COLLATE BINARY, 'A' COLLATE NOCASE = t COLLATE BINARY FROM r"),
    "rule.collation:then a column's collation, left first": ([f"{R}(x TEXT COLLATE NOCASE, y TEXT COLLATE BINARY)", "INSERT INTO r VALUES ('a', 'A')"], "SELECT x = y, y = x FROM r"),
    "rule.collation:otherwise BINARY": ([], "SELECT 'a' = 'A'"),
    "rule.collation:the leftmost COLLATE anywhere wins": ([], "SELECT 'a' COLLATE NOCASE = ('A' COLLATE BINARY), 'x' = ('X' COLLATE NOCASE) || ''"),
    "rule.collation:IN uses the left operand's collation": ([], "SELECT 'a' COLLATE NOCASE IN ('A'), 'a' IN ('A' COLLATE NOCASE)"),
}
for pid, (setup, q) in rules.items():
    probe(pid, q, setup=setup, base=False)
ORDER_COLL = [f"{R}(t TEXT COLLATE NOCASE)", "INSERT INTO r VALUES ('b'), ('A'), ('a'), ('B')"]
probe("rule.collation:ORDER BY uses COLLATE, then the column's#1", "SELECT t FROM r ORDER BY t COLLATE BINARY", setup=ORDER_COLL, base=False)
probe("rule.collation:ORDER BY uses COLLATE, then the column's#2", "SELECT t FROM r ORDER BY t, t COLLATE BINARY", setup=ORDER_COLL, base=False)

# ---------------------------------------------------------------- words the grammar uses that aren't keywords
probe("word:TRUE", "SELECT TRUE, TRUE + 1")
probe("word:FALSE", "SELECT FALSE, FALSE IS FALSE")
probe("word:STRICT#1", "SELECT typeof(k), typeof(v) FROM st", setup=["CREATE TABLE st(k INTEGER, v TEXT) STRICT", "INSERT INTO st VALUES (1, 'x')"], base=False)
probe("word:STRICT#2", "INSERT INTO st VALUES ('abc', 1)", setup=["CREATE TABLE st(k INTEGER, v TEXT) STRICT"], base=False, expect="error")
probe("word:STORED", "SELECT b FROM gs", setup=["CREATE TABLE gs(a INT, b INT AS (a * 2) STORED)", "INSERT INTO gs(a) VALUES (3)"], base=False)
probe("word:ROWID", "SELECT k, v FROM wr", setup=["CREATE TABLE wr(k INT PRIMARY KEY, v TEXT) WITHOUT ROWID", "INSERT INTO wr VALUES (1, 'a')"], base=False)
probe("word:EXPRESSIONS", "REINDEX EXPRESSIONS; SELECT count(*) FROM t WHERE lower(b) = 'one'", setup=["CREATE INDEX ei ON t(lower(b))"])
probe("word:E", "SELECT 1e3, 2.5E-1, 1E+2")

# ---------------------------------------------------------------- keywords no other probe uses (the rest are covered by the probes that use them)
FK = ["CREATE TABLE kp(k INTEGER PRIMARY KEY)", "INSERT INTO kp VALUES (1)"]
probe("keyword:INNER", "SELECT t.a, u.d FROM t INNER JOIN u ON t.a = u.a ORDER BY t.a, u.d")
probe("keyword:CONSTRAINT", "INSERT INTO kc VALUES (1); SELECT a FROM kc", setup=["CREATE TABLE kc(a INT CONSTRAINT pos CHECK (a > 0), CONSTRAINT uq UNIQUE (a))"], base=False)
probe("keyword:FAIL", "INSERT OR FAIL INTO kp VALUES (2); SELECT count(*) FROM kp", setup=FK, base=False)
probe("keyword:NO", "INSERT INTO kn VALUES (1); SELECT count(*) FROM kn", setup=FK + ["CREATE TABLE kn(a INT REFERENCES kp(k) ON DELETE NO ACTION)"], base=False)
probe("keyword:ACTION", "INSERT INTO ka VALUES (1); SELECT count(*) FROM ka", setup=FK + ["CREATE TABLE ka(a INT REFERENCES kp(k) ON UPDATE NO ACTION)"], base=False)
probe("keyword:RESTRICT", "INSERT INTO kr VALUES (1); SELECT count(*) FROM kr", setup=FK + ["CREATE TABLE kr(a INT REFERENCES kp(k) ON DELETE RESTRICT)"], base=False)
probe("keyword:OTHERS", "SELECT a, sum(a) OVER (ORDER BY a ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING EXCLUDE NO OTHERS) FROM t ORDER BY a")

# ---------------------------------------------------------------- statements and clauses (syntax productions), with their keyword paths
S = {
    "select-stmt": ["SELECT a, b FROM t WHERE a > 1 ORDER BY a DESC LIMIT 2 OFFSET 1", "SELECT DISTINCT a % 2 FROM t ORDER BY 1", "SELECT ALL a FROM t ORDER BY a LIMIT 1, 2"],
    "select-core": ["SELECT a % 2 AS g, count(*), sum(a) FROM t GROUP BY g HAVING count(*) > 1 ORDER BY g", "VALUES (1, 'a'), (2, 'b')", "SELECT 1 + 1"],
    "simple-select-stmt": ["SELECT a FROM t ORDER BY a NULLS LAST LIMIT 3"],
    "factored-select-stmt": ["WITH x AS (SELECT a FROM t) SELECT a FROM x ORDER BY a"],
    "compound-select-stmt": ["SELECT a FROM t UNION SELECT a FROM u ORDER BY 1", "SELECT a FROM t UNION ALL SELECT a FROM u ORDER BY 1", "SELECT a FROM t INTERSECT SELECT a FROM u ORDER BY 1", "SELECT a FROM t EXCEPT SELECT a FROM u ORDER BY 1"],
    "compound-operator": ["SELECT 1 UNION ALL SELECT 2 ORDER BY 1"],
    "common-table-expression": ["WITH x(v) AS MATERIALIZED (SELECT a FROM t) SELECT count(*) FROM x", "WITH x(v) AS NOT MATERIALIZED (SELECT a FROM t) SELECT sum(v) FROM x"],
    "recursive-cte": ["WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5) SELECT i FROM n ORDER BY i", "WITH RECURSIVE f(i, v) AS (SELECT 1, 1 UNION ALL SELECT i + 1, v * (i + 1) FROM f WHERE i < 6) SELECT v FROM f ORDER BY i DESC LIMIT 1"],
    "with-clause": ["WITH a1 AS (SELECT 1 AS v), a2 AS (SELECT v + 1 AS v FROM a1) SELECT v FROM a2"],
    "cte-table-name": ["WITH x(p, q) AS (SELECT 1, 2) SELECT p + q FROM x"],
    "join-clause": ["SELECT t.a, u.d FROM t JOIN u ON t.a = u.a ORDER BY t.a, u.d", "SELECT t.a, u.d FROM t LEFT JOIN u ON t.a = u.a ORDER BY t.a, u.d", "SELECT t.a, u.d FROM t RIGHT JOIN u ON t.a = u.a ORDER BY u.a, u.d", "SELECT t.a, u.d FROM t FULL OUTER JOIN u ON t.a = u.a ORDER BY coalesce(t.a, u.a), u.d", "SELECT count(*) FROM t CROSS JOIN u", "SELECT a, d FROM t NATURAL JOIN u ORDER BY a, d"],
    "join-constraint": ["SELECT a, b, d FROM t JOIN u USING (a) ORDER BY a, d"],
    "join-operator": ["SELECT count(*) FROM t, u WHERE t.a = u.a", "SELECT count(*) FROM t LEFT OUTER JOIN u ON t.a = u.a"],
    "table-or-subquery": ["SELECT s.x FROM (SELECT a * 2 AS x FROM t) AS s ORDER BY s.x", "SELECT value FROM json_each('[3,1,2]') ORDER BY value"],
    "result-column": ["SELECT t.*, u.d FROM t JOIN u ON t.a = u.a ORDER BY t.a, u.d", "SELECT * FROM u ORDER BY a, d"],
    "ordering-term": ["SELECT b FROM t ORDER BY b COLLATE NOCASE DESC NULLS FIRST", "SELECT b FROM t ORDER BY b ASC NULLS LAST"],
    "qualified-table-name": ["SELECT a FROM main.t INDEXED BY ti WHERE a > 3 ORDER BY a"],
    "insert-stmt": ["INSERT INTO t(a, b) VALUES (10, 'ten') RETURNING a, b", "INSERT OR IGNORE INTO pk VALUES (1, 'dup')", "INSERT OR REPLACE INTO pk VALUES (1, 'new')", "INSERT INTO t SELECT a + 100, b, c FROM t WHERE a < 3", "INSERT INTO t DEFAULT VALUES", "REPLACE INTO pk VALUES (2, 'two')"],
    "upsert-clause": ["INSERT INTO pk VALUES (1, 'up') ON CONFLICT(k) DO UPDATE SET v = excluded.v || '!' RETURNING k, v", "INSERT INTO pk VALUES (1, 'no') ON CONFLICT DO NOTHING"],
    "returning-clause": ["DELETE FROM t WHERE a = 5 RETURNING *", "UPDATE t SET c = 0 WHERE a = 1 RETURNING a, c * 2 AS dbl"],
    "update-stmt": ["UPDATE t SET b = upper(b), c = c + 1 WHERE a < 3", "UPDATE OR IGNORE t SET a = a WHERE a = 1", "UPDATE t SET (b, c) = ('p', 9.5) WHERE a = 2", "UPDATE t SET b = u.d FROM u WHERE t.a = u.a"],
    "update-stmt-limited": ["UPDATE t SET b = 'lim' WHERE a > 0 ORDER BY a LIMIT 2"],
    "delete-stmt": ["DELETE FROM t WHERE a IN (SELECT a FROM u)"],
    "delete-stmt-limited": ["DELETE FROM t WHERE a > 0 ORDER BY a DESC LIMIT 1"],
    "create-table-stmt": ["CREATE TABLE IF NOT EXISTS ct(a INTEGER PRIMARY KEY, b TEXT NOT NULL DEFAULT 'x', c REAL CHECK (c >= 0), UNIQUE (b))", "CREATE TEMP TABLE ctt AS SELECT a, b FROM t", "CREATE TEMPORARY TABLE ctt2(x INT)"],
    # One clause per probe where SQLite's own syntax sits beside the standard's, so an engine that
    # lacks one isn't counted as lacking the rest.
    "column-def": ["CREATE TABLE cd(a INTEGER PRIMARY KEY AUTOINCREMENT, v TEXT); INSERT INTO cd(v) VALUES ('x'), ('y'); SELECT a, v FROM cd ORDER BY a",
                   "CREATE TABLE cd(b TEXT COLLATE NOCASE); INSERT INTO cd VALUES ('Abc'); SELECT count(*) FROM cd WHERE b = 'aBC'",
                   "CREATE TABLE cd(a INT, c INT GENERATED ALWAYS AS (a * 2) STORED); INSERT INTO cd(a) VALUES (4); SELECT c FROM cd",
                   "CREATE TABLE cd(a INT, d INT AS (a + 1) VIRTUAL); INSERT INTO cd(a) VALUES (4); SELECT d FROM cd"],
    "column-constraint": ["CREATE TABLE cc(a INT PRIMARY KEY DESC ON CONFLICT REPLACE, v TEXT); INSERT INTO cc VALUES (1, 'x'); INSERT INTO cc VALUES (1, 'y'); SELECT v FROM cc",
                          "CREATE TABLE cc(b INT UNIQUE NOT NULL); INSERT INTO cc VALUES (1); SELECT count(*) FROM cc",
                          "CREATE TABLE cc(c INT REFERENCES pk(k) ON DELETE CASCADE); INSERT INTO cc VALUES (1); DELETE FROM pk; SELECT count(*) FROM cc",
                          "CREATE TABLE cc(d INT DEFAULT (1 + 1), e TEXT); INSERT INTO cc(e) VALUES ('x'); SELECT d FROM cc"],
    "table-constraint": ["CREATE TABLE tc(a INT, b INT, PRIMARY KEY (a, b), CHECK (a < b)); INSERT INTO tc VALUES (1, 2); SELECT count(*) FROM tc",
                         "CREATE TABLE tc(a INT, FOREIGN KEY (a) REFERENCES pk(k) DEFERRABLE INITIALLY DEFERRED); INSERT INTO tc VALUES (1); SELECT count(*) FROM tc"],
    "foreign-key-clause": ["CREATE TABLE fk(a INT REFERENCES pk(k) ON UPDATE SET NULL ON DELETE SET DEFAULT MATCH SIMPLE)"],
    "conflict-clause": ["CREATE TABLE cf(a INT UNIQUE ON CONFLICT IGNORE)"],
    "table-options": ["CREATE TABLE wr(k INT PRIMARY KEY, v TEXT) WITHOUT ROWID", "CREATE TABLE st(k INTEGER, v TEXT) STRICT", "CREATE TABLE sw(k INT PRIMARY KEY, v ANY) STRICT, WITHOUT ROWID"],
    "type-name": ["CREATE TABLE tn(a VARCHAR(20), b DECIMAL(10, 2), c DOUBLE PRECISION); INSERT INTO tn VALUES ('x', 1.25, 2.5); SELECT a, b, c FROM tn",
                  "CREATE TABLE tn(d UNSIGNED BIG INT); INSERT INTO tn VALUES (7); SELECT d FROM tn"],
    "create-index-stmt": ["CREATE UNIQUE INDEX IF NOT EXISTS ix ON t(a DESC)", "CREATE INDEX px ON t(a) WHERE b IS NOT NULL", "CREATE INDEX ex ON t(lower(b))",
                          "CREATE INDEX cx ON t(b COLLATE NOCASE)"],
    "indexed-column": ["CREATE INDEX ic ON t(b COLLATE NOCASE ASC)"],
    "create-view-stmt": ["CREATE VIEW IF NOT EXISTS v1(x, y) AS SELECT a, b FROM t", "CREATE TEMP VIEW v2 AS SELECT count(*) FROM t"],
    "create-trigger-stmt": ["CREATE TRIGGER tr AFTER INSERT ON t FOR EACH ROW WHEN new.a > 100 BEGIN INSERT INTO u VALUES (new.a, 'trig'); END", "CREATE TRIGGER tb BEFORE DELETE ON t BEGIN SELECT RAISE(IGNORE); END", "CREATE TRIGGER tu AFTER UPDATE OF b ON t BEGIN UPDATE u SET d = new.b WHERE a = new.a; END", "CREATE TEMP TRIGGER IF NOT EXISTS ti INSTEAD OF INSERT ON v1 BEGIN SELECT 1; END"],
    "raise-function": ["CREATE TRIGGER rf BEFORE INSERT ON pk WHEN new.k < 0 BEGIN SELECT RAISE(ABORT, 'negative'); END"],
    "create-virtual-table-stmt": ["CREATE VIRTUAL TABLE IF NOT EXISTS vt USING fts5(body)", "CREATE VIRTUAL TABLE rt USING rtree(id, x0, x1)"],
    "alter-table-stmt": ["ALTER TABLE t RENAME TO t9", "ALTER TABLE t RENAME COLUMN b TO bb", "ALTER TABLE t ADD COLUMN e TEXT DEFAULT 'e'", "ALTER TABLE t DROP COLUMN c"],
    "drop-table-stmt": ["DROP TABLE IF EXISTS u"],
    "drop-index-stmt": ["DROP INDEX IF EXISTS ti"],
    "drop-view-stmt": ["DROP VIEW IF EXISTS nope"],
    "drop-trigger-stmt": ["DROP TRIGGER IF EXISTS nope"],
    "analyze-stmt": ["ANALYZE", "ANALYZE t"],
    "attach-stmt": ["ATTACH DATABASE ':memory:' AS aux"],
    "detach-stmt": ["DETACH DATABASE aux"],
    "begin-stmt": ["BEGIN DEFERRED TRANSACTION", "BEGIN IMMEDIATE", "BEGIN EXCLUSIVE"],
    "commit-stmt": ["COMMIT", "END TRANSACTION"],
    "rollback-stmt": ["ROLLBACK TO SAVEPOINT sp1"],
    "savepoint-stmt": ["SAVEPOINT sp1"],
    "release-stmt": ["RELEASE SAVEPOINT sp1"],
    "reindex-stmt": ["REINDEX", "REINDEX t"],
    "vacuum-stmt": ["VACUUM"],
    "pragma-stmt": ["PRAGMA table_info(t)", "PRAGMA main.user_version = 3"],
    "pragma-value": ["PRAGMA cache_size = -2000"],
    "expr": ["SELECT (1 + 2) * 3, -a, a IS NOT NULL, b LIKE 't%' FROM t ORDER BY a"],
    "literal-value": ["SELECT 1, 1.5, 'txt', x'0F', NULL, TRUE, FALSE, typeof(CURRENT_DATE), typeof(CURRENT_TIME), typeof(CURRENT_TIMESTAMP)"],
    "numeric-literal": ["SELECT 0x1F, 1e3, .5, 1_000_000, 2.5E-1"],
    "signed-number": ["CREATE TABLE sn(a INT DEFAULT -1, b REAL DEFAULT +2.5)"],
    "function-arguments": ["SELECT count(DISTINCT a), group_concat(DISTINCT b) FROM t", "SELECT count(*) FROM t"],
    "simple-function-invocation": ["SELECT abs(-1)"],
    "aggregate-function-invocation": ["SELECT sum(DISTINCT a % 3) FROM t", "SELECT group_concat(b ORDER BY a DESC) FROM t"],
    "window-function-invocation": ["SELECT a, sum(a) FILTER (WHERE a > 1) OVER (ORDER BY a) FROM t ORDER BY a"],
    "over-clause": ["SELECT a, count(*) OVER (PARTITION BY a % 2) FROM t ORDER BY a"],
    "window-defn": ["SELECT a, max(a) OVER w FROM t WINDOW w AS (PARTITION BY a % 2 ORDER BY a RANGE UNBOUNDED PRECEDING) ORDER BY a"],
    "filter-clause": ["SELECT count(*) FILTER (WHERE b IS NULL) FROM t", "SELECT count(*) FILTER (WHERE a > 2), sum(a) FILTER (WHERE b IS NOT NULL) FROM t"],
    "frame-spec": ["SELECT a, sum(a) OVER (ORDER BY a ROWS UNBOUNDED PRECEDING) FROM t ORDER BY a"],
    "column-name-list": ["UPDATE t SET (a, b) = (SELECT 1, 'z') WHERE a = 4"],
    "comment-syntax": ["SELECT 1 -- a comment", "SELECT /* inline */ 2"],
    "sql-stmt": ["EXPLAIN SELECT 1", "EXPLAIN QUERY PLAN SELECT a FROM t WHERE a = 1"],
    "sql-stmt-list": ["SELECT 1; SELECT 2"],
}
EXTRA_SETUP = {
    "qualified-table-name": ["CREATE INDEX ti ON t(a)"],
    "insert-stmt": ["CREATE TABLE pk(k INTEGER PRIMARY KEY, v TEXT)", "INSERT INTO pk VALUES (1, 'one')"],
    "upsert-clause": ["CREATE TABLE pk(k INTEGER PRIMARY KEY, v TEXT)", "INSERT INTO pk VALUES (1, 'one')"],
    "foreign-key-clause": ["CREATE TABLE pk(k INTEGER PRIMARY KEY, v TEXT)"],
    "column-constraint": ["CREATE TABLE pk(k INTEGER PRIMARY KEY, v TEXT)", "INSERT INTO pk VALUES (1, 'one')"],
    "table-constraint": ["CREATE TABLE pk(k INTEGER PRIMARY KEY, v TEXT)", "INSERT INTO pk VALUES (1, 'one')"],
    "raise-function": ["CREATE TABLE pk(k INTEGER PRIMARY KEY, v TEXT)"],
    "create-trigger-stmt": ["CREATE VIEW v1 AS SELECT a, b FROM t"],
    "drop-index-stmt": ["CREATE INDEX ti ON t(a)"],
    "detach-stmt": ["ATTACH DATABASE ':memory:' AS aux"],
    "commit-stmt": ["BEGIN"],
    "rollback-stmt": ["BEGIN", "SAVEPOINT sp1"],
    "release-stmt": ["SAVEPOINT sp1"],
}
SQLITE_SETUP = {"column-constraint": ["PRAGMA foreign_keys = ON"], "foreign-key-clause": ["PRAGMA foreign_keys = ON"]}
PARAMS = [("?NNN", "SELECT ?1 + 1", [41]), ("?", "SELECT ? * 2", [21]), (":AAAA", "SELECT :v IS NULL", {"v": None}),
          ("@AAAA", "SELECT @w + 0", {"w": 5}), ("$AAAA", "SELECT $x || 'y'", {"x": "x"})]
for name, q, params in PARAMS:
    probe(f"parameter:{name}", q, params=params)
LIMITED = {"update-stmt-limited", "delete-stmt-limited"}
for prod, queries in S.items():
    for i, q in enumerate(queries):
        unordered = q.lstrip().upper().startswith(("SELECT", "WITH", "VALUES")) and "ORDER BY" not in q.upper() and "count(" not in q and "sum(" not in q
        probe(f"syntax:{prod}" + (f"#{i + 1}" if len(queries) > 1 else ""), q, setup=EXTRA_SETUP.get(prod), order=not unordered,
              requires="SQLITE_ENABLE_UPDATE_DELETE_LIMIT" if prod in LIMITED else None, sqlite_setup=SQLITE_SETUP.get(prod),
              compare="nonempty" if prod == "sql-stmt" else None)

# ---------------------------------------------------------------- PRAGMAs: each in its read form (a few take an argument)
PRAGMA_ARGS = {"table_info": "(t)", "table_xinfo": "(t)", "index_list": "(t)", "index_info": "(ti)", "index_xinfo": "(ti)", "foreign_key_list": "(t)",
               "table_list": "", "function_list": "", "collation_list": "", "module_list": "", "pragma_list": "", "integrity_check": "", "quick_check": "",
               "foreign_key_check": "", "optimize": "", "wal_checkpoint": "", "shrink_memory": "", "incremental_vacuum": "", "compile_options": ""}
# What a build lists (its functions, modules, options, collations) or where its file is: an engine
# answers when it answers at all, or in the same shape.
PRAGMA_COMPARE = {"compile_options": "nonempty", "function_list": "nonempty", "module_list": "nonempty", "pragma_list": "nonempty",
                  "collation_list": "nonempty", "database_list": "shape", "data_version": "shape"}
PRAGMA_REQUIRES = {"parser_trace": "SQLITE_DEBUG", "vdbe_addoptrace": "SQLITE_DEBUG", "vdbe_debug": "SQLITE_DEBUG", "vdbe_listing": "SQLITE_DEBUG",
                   "vdbe_trace": "SQLITE_DEBUG", "stats": "a testing build (its output is deliberately undocumented)",
                   "data_store_directory": "Windows's VFS", "legacy_file_format": "nothing: it no longer functions (a no-op)"}


def add_pragmas(features):
    for f in features:
        if f["kind"] != "pragma":
            continue
        name = f["name"]
        setup = ["CREATE INDEX ti ON t(a)"] if name.startswith("index_") else None
        probe(f"pragma:{name}", f"PRAGMA {name}{PRAGMA_ARGS.get(name, '')}", setup=setup, order=False, requires=PRAGMA_REQUIRES.get(name), compare=PRAGMA_COMPARE.get(name))


def split(sql):
    """A probe's statements, split where SQLite's own parser says each one ends (a trigger's
    body keeps its semicolons), so every engine's runner takes them one at a time."""
    import sqlite3
    out, cur = [], ""
    for piece in sql.split(";"):
        cur = piece if not cur else cur + ";" + piece
        if sqlite3.complete_statement(cur + ";"):
            if cur.strip():
                out.append(cur.strip())
            cur = ""
    if cur.strip():
        out.append(cur.strip())
    return out


def main():
    feats = json.load(open(sys.argv[1]))["features"]
    add_pragmas(feats)
    for p in P:
        p["parts"] = split(p["query"])
    ids = {f["id"] for f in feats}
    probed = {p["id"].split("#")[0] for p in P}
    missing = sorted(i for i in ids if i not in probed and not i.startswith("keyword:"))
    import re
    def words(sql):
        return {w.upper() for w in re.findall(r"[A-Za-z_][A-Za-z_0-9]*", re.sub(r"'(?:[^']|'')*'", " ", sql))}
    used = set()
    for p in P:
        for s_ in p["setup"] + [p["query"]]:
            used |= words(s_)
    missing += sorted(f["id"] for f in feats if f["kind"] == "keyword" and f["name"] not in used)
    json.dump({"probes": P, "unprobed": missing}, open(sys.argv[2], "w"), indent=1)
    entries = [i for i in ids if not i.startswith("keyword:")]
    kws = [f["name"] for f in feats if f["kind"] == "keyword"]
    print(f"{len(P)} probes: {len(set(entries) & probed)} of {len(entries)} entries probed, {sum(k in used for k in kws)} of {len(kws)} keywords used by a probe")
    if missing:
        print("not probed:", missing)


main()
