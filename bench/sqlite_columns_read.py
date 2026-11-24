"""Oracle findings CQ, CP, CX, CR and DE (lane C21): what a column read through something else is. A table
function's argument that holds a correlated subquery, in every join (CQ); a table function's column that
declares no type, compared with columns of every affinity and collation (CP); a trigger's new.c and old.c,
of every collation and affinity, in WHEN and in bodies, under every conflict clause (CX); a subquery whose
IN reads a table an EXISTS around it was joined by (CR); a column in an aggregate's argument, under every
grouping (DE). Writes each query with the answer SQLite itself gives (the sqlite3 library Python links,
printed first) to crates/engine/tests/fixtures/columns_read.tsv, which tests/columns_read.rs holds Amphora to.

Each line is `db <class> <name>`, `sql <statement>` (the database's tables, rows, triggers and writes; one
SQLite refuses is not written) or `q <query> <answer>`: the answer's rows sorted, each row's values joined
by commas (a NULL written NULL, a text in quotes, a blob x''), the rows joined by semicolons.

Not drawn, and why:
- json_each's type, fullkey, path, id and parent beside an operand whose affinity they would change or be
  changed by: json1.html 4.10 declares them TEXT and INTEGER, SQLite 3.54.0 declares no column of it.
- key, value and atom, which json1.html 4.10 declares ANY, are compared as SQLite compares them, with no
  affinity (stricttables.html: a column of type ANY takes any value as it is).
- generate_series: an extension, which the SQLite library Python links doesn't have.
- A bare column's value, a text of a group or a tie's value: any of its rows may give it (lower(), rtrim()).
- new.rowid in a BEFORE INSERT that gives no rowid: undefined (lang_createtrigger.html).
- A trigger on a WITHOUT ROWID table: Amphora refuses it (lane C11's record).
- A subquery in a window function's argument: Amphora refuses it (finding CS).
- json_tree right of a RIGHT or FULL JOIN: its root's rowid, 0, reads NULL in Amphora (finding DK).
- A subquery in an UPSERT's DO UPDATE: Amphora refuses it ("Scalar subquery is not supported in this context").
usage: python3 bench/sqlite_columns_read.py [seed] > crates/engine/tests/fixtures/columns_read.tsv"""
import random
import sqlite3
import sys


def written(value):
    if value is None:
        return "NULL"
    if isinstance(value, float):
        return repr(value)
    if isinstance(value, bytes):
        return "x'%s'" % value.hex()
    if isinstance(value, str):
        return "'%s'" % value
    return str(value)


class Database:
    """A database of the fixture: what SQLite takes of `statements`, and the queries it answers."""

    def __init__(self, kind, name, statements):
        self.con = sqlite3.connect(":memory:", isolation_level=None)
        self.lines = [f"db\t{kind}\t{name}"]
        self.queries = set()
        self.refused = 0
        for statement in statements:
            self.run(statement)

    def run(self, statement):
        try:
            self.con.execute(statement)
        except sqlite3.Error:
            self.refused += 1
            return False
        self.lines.append(f"sql\t{statement}")
        return True

    def ask(self, query):
        if query in self.queries:
            return
        self.queries.add(query)
        try:
            rows = [",".join(written(v) for v in row) for row in self.con.execute(query).fetchall()]
        except sqlite3.Error:
            self.refused += 1
            return
        rows.sort()
        answer = f"{len(rows)}:{';'.join(rows)}"
        if "\t" in answer or "\n" in answer or len(answer) > 4000:
            return
        self.lines.append(f"q\t{query}\t{answer}")

    def asked(self):
        return sum(line.startswith("q\t") for line in self.lines)

    def print(self):
        print("\n".join(self.lines))


# CQ ------------------------------------------------------------------------------------------------
ARGUMENT_TABLES = [
    "CREATE TABLE a(k INT, v INT)",
    "INSERT INTO a VALUES (1, 7), (2, 8), (3, NULL), (NULL, 7)",
    "CREATE TABLE b(k INT, v INT)",
    "INSERT INTO b VALUES (1, 1), (7, 2), (8, 2), (NULL, 3)",
    "CREATE TABLE t(k INT, j TEXT)",
    "INSERT INTO t VALUES (1, '[7]'), (2, '[8, 7]'), (3, '[]'), (NULL, '[7, 9]'), (4, '{\"x\": 7}')",
    "CREATE TABLE r(x INT)",
    "INSERT INTO r VALUES (1), (5), (2)",
    "CREATE TABLE l(y INT)",
    "INSERT INTO l VALUES (5), (2)",
    "CREATE TABLE z(k INT, v INT)",
]
ARGUMENT_INDEXES = [
    "CREATE INDEX a_k ON a(k)",
    "CREATE INDEX b_k ON b(k, v)",
    "CREATE UNIQUE INDEX t_k ON t(k)",
    "CREATE INDEX r_x ON r(x)",
    "CREATE INDEX l_y ON l(y)",
]


def argument(rng, left):
    """A table function's argument: a subquery that reads `left`, or one that reads nothing outside it."""
    k = f"{left}.k"
    v = f"{left}.v"
    join = "EXISTS (SELECT 1 FROM r JOIN l ON r.x = l.y)"
    nested = "t.k IN (SELECT r.x FROM r JOIN l ON r.x = l.y)"
    return rng.choice([
        f"(SELECT t.j FROM t WHERE t.k = {k})",
        f"(SELECT t.j FROM t WHERE t.k IS {k})",
        f"(SELECT t.j FROM t WHERE t.k = {k} AND {join})",
        f"(SELECT max(t.j) FROM t WHERE {k} IS NULL OR t.k <= {k})",
        f"(SELECT json_array(min(t.k), max(t.k), count(*) + 6) FROM t WHERE t.k >= {k})",
        f"(SELECT json_array(max(t.k), count(*), 7) FROM t WHERE {nested} OR t.k IS {k})",
        f"coalesce((SELECT t.j FROM t WHERE t.k = {k}), '[7, 1]')",
        f"json_array({k}, (SELECT count(*) FROM t WHERE t.k < {k}), {v})",
        f"CASE WHEN EXISTS (SELECT 1 FROM t WHERE t.k = {v} - 6) THEN '[7, 2]' ELSE '[1]' END",
        f"CASE WHEN {k} IN (SELECT t.k FROM t) THEN '[7]' ELSE '[2, 7]' END",
        f"CASE WHEN {k} NOT IN (SELECT t.k FROM t WHERE t.k <> {v} - 6 AND t.k IS NOT NULL) THEN '[7]' ELSE '[8]' END",
        f"CASE WHEN ({k}, {v}) IN (SELECT t.k, t.k + 6 FROM t) THEN '[7]' ELSE '[1, 8]' END",
        "(SELECT json_array(min(r.x), max(r.x), 7) FROM r JOIN l ON r.x = l.y)",
        f"(SELECT json_array(min(t.k), 7) FROM t WHERE {nested})",
        f"json_array({k}, {v}, 7)",
        "'[7, 1, null]'",
    ])


def argument_query(rng):
    left = rng.choice(["a", "a", "a", "z", "b"])
    joins = ["JOIN", "LEFT JOIN", "RIGHT JOIN", "FULL JOIN", "FULL JOIN", "RIGHT JOIN"]
    tables = f"{left} AS x"
    names = ["x"]
    if rng.random() < 0.35:
        tables += f" {rng.choice(['JOIN', 'LEFT JOIN', 'FULL JOIN', 'RIGHT JOIN'])} {rng.choice('abz')} AS y ON {rng.choice(['x.k = y.k', 'x.v = y.k', 'y.k < x.k', '1'])}"
        names.append("y")
    reads = rng.choice(names)
    join = rng.choice(joins)
    function = rng.choice(["json_each", "json_each", "json_each", "json_tree"] if "RIGHT" not in join and "FULL" not in join else ["json_each"])
    on = rng.choice(["je.value = 7", f"je.value = {reads}.v", "je.key = 0", f"je.value >= {reads}.k", "1", "NULL", "je.atom IS NOT NULL",
                     f"je.value IN (SELECT r.x FROM r WHERE r.x <> {reads}.k)"])
    tables += f" {join} {function}({argument(rng, reads)}) AS je ON {on}"
    values = [f"{name}.k" for name in names] + ["je.value"]
    if rng.random() < 0.3:
        second = rng.choice([argument(rng, reads), "json_array(je.value, 7)", "(SELECT json_array(min(t.k), max(t.k)) FROM t WHERE t.k < je.value)"])
        tables += f" {rng.choice(joins)} json_each({second}) AS jf ON {rng.choice(['jf.value = je.value', 'jf.value = 7', '1', 'jf.key = je.key'])}"
        values.append("jf.value")
    if rng.random() < 0.3:
        tables += f" {rng.choice(['JOIN', 'LEFT JOIN', 'RIGHT JOIN', 'FULL JOIN'])} b AS w ON {rng.choice(['w.k = je.value', 'w.v = x.k', 'w.k IS x.k'])}"
        values.append("w.k")
    where = ""
    if rng.random() < 0.4:
        where = " WHERE " + rng.choice([
            "je.value IS NOT NULL", "x.k IS NULL OR x.k < 3", "je.value = 7 OR x.v = 8",
            "je.value NOT IN (SELECT r.x FROM r)", "EXISTS (SELECT 1 FROM t WHERE t.k = x.k)",
            "(SELECT count(*) FROM t WHERE t.k <= x.k) < 3", "coalesce(je.value, 0) <> 1",
        ])
    shape = rng.random()
    if shape < 0.6:
        return f"SELECT {', '.join(values)} FROM {tables}{where}"
    if shape < 0.7:
        return f"SELECT count(*), sum(je.value), count(x.k) FROM {tables}{where}"
    if shape < 0.8:
        return f"SELECT x.k, count(*), sum(je.value) FROM {tables}{where} GROUP BY x.k"
    if shape < 0.86:
        return f"SELECT DISTINCT {values[0]}, je.value FROM {tables}{where}"
    if shape < 0.92:
        return f"SELECT {values[0]}, je.value, count(*) OVER (PARTITION BY x.k) FROM {tables}{where}"
    if shape < 0.96:
        return f"SELECT q.k, (SELECT count(*) FROM {tables}{where}) FROM a AS q"
    return f"SELECT q.k FROM a AS q WHERE EXISTS (SELECT 1 FROM {tables} WHERE je.value = q.v)"


def arguments(seed):
    out = []
    for name, indexes, count in (("plain", [], 400), ("indexed", ARGUMENT_INDEXES, 300)):
        database = Database("cq", name, ARGUMENT_TABLES + indexes)
        rng = random.Random(f"{seed} cq {name}")
        for query in ARGUMENT_FOUND:
            database.ask(query)
        while database.asked() < count:
            database.ask(argument_query(rng))
        out.append(database)
    return out


ARGUMENT_FOUND = [
    "SELECT a.k, je.value FROM a FULL JOIN json_each((SELECT t.j FROM t WHERE t.k = a.k AND EXISTS (SELECT 1 FROM r JOIN l ON r.x = l.y))) AS je ON je.value = 7",
    "SELECT a.k, je.value FROM a RIGHT JOIN json_each((SELECT t.j FROM t WHERE t.k = a.k)) AS je ON je.value = 7",
    "SELECT z.k, je.value FROM z FULL JOIN json_each((SELECT json_array(max(t.k), count(*)) FROM t WHERE t.k IN (SELECT r.x FROM r JOIN l ON r.x = l.y) OR t.k IS z.k)) AS je",
    "SELECT z.k, je.value FROM z RIGHT JOIN json_each((SELECT json_array(min(r.x), max(r.x)) FROM r JOIN l ON r.x = l.y WHERE z.k IS NULL)) AS je",
    "SELECT count(*), sum(je.value) FROM a FULL JOIN json_each((SELECT t.j FROM t WHERE t.k IS a.k)) AS je ON je.value = 7",
    "SELECT a.k, je.value, b.k FROM a JOIN json_each((SELECT t.j FROM t WHERE t.k = a.k)) AS je RIGHT JOIN b ON b.k = je.value",
    "SELECT a.k, b.k, je.value FROM a RIGHT JOIN b ON a.k = b.k LEFT JOIN json_each((SELECT t.j FROM t WHERE t.k IS a.k)) AS je",
]


# CP ------------------------------------------------------------------------------------------------
TYPED = ["i", "r", "t", "n", "b", "u", "nc", "rt"]
UNTYPED_TABLES = [
    "CREATE TABLE p(id INTEGER PRIMARY KEY, i INTEGER, r REAL, t TEXT, n NUMERIC, b BLOB, u, nc TEXT COLLATE NOCASE, rt TEXT COLLATE RTRIM)",
    "INSERT INTO p VALUES (1, 1, 1.0, '1', '1', '1', '1', '1', '1 '), (2, 2, 2.5, '2', 2, 2, 2, 'A', 'a  '), (3, 0, 0.0, 'a', 'a', x'61', 'a', 'a', 'a'), (4, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL)",
    "INSERT INTO p VALUES (5, 'x', 'x', 'x', 'x', 'x', 'x', 'X', 'x '), (6, 10, 10, '10', '10', '10', 10, '10', '10'), (7, '1.5', 1.5, '1.5', 1.5, x'31', 1.5, '1.5', '1.5')",
    "CREATE TABLE d(id INTEGER PRIMARY KEY, j TEXT)",
    "INSERT INTO d VALUES (1, '[1, \"1\", \"a\", 2.5, null, true, \"A\", \"a \"]'), (2, '{\"1\": 1, \"a\": \"A\", \"b\": [1, 2], \"x\": \"x\"}'), (3, '[]'), (4, NULL), (5, '\"1\"'), (6, '[10, \"10\", 0, \"x\", 1.5, \"1.5\"]')",
]
UNTYPED_INDEXES = [f"CREATE INDEX p_{column} ON p({column})" for column in TYPED]


def untyped_query(rng):
    source, columns = rng.choice([
        ("json_each(d.j)", ["key", "value", "atom"]), ("json_each(d.j)", ["key", "value", "value"]),
        ("json_tree(d.j)", ["key", "atom"]),
    ])
    literal_source = rng.random() < 0.45
    if literal_source:
        doc = rng.choice(["'[1, \"1\", \"a\", 2.5, null, true, \"A\", \"a \"]'", "'[5, 6, 7]'", "'{\"1\": 1, \"a\": \"A\", \"x\": \"x\"}'", "'[10, \"10\", 0, \"x\", 1.5, \"1.5\"]'"])
        source = source.replace("d.j", doc)
    if literal_source and rng.random() < 0.15:
        source, columns = "pragma_table_info('p')", ["cid", "name", "pk"]
    column = f"j.{rng.choice(columns)}"
    typed = f"p.{rng.choice(TYPED)}"
    other = rng.choice([typed] * 8 + [f"+{typed}", f"CAST({typed} AS TEXT)", f"({typed} || '')", "'1'", "1", "'a'", "2.5", "'A'", f"{typed} COLLATE NOCASE", f"{typed} COLLATE RTRIM"])
    operator = rng.choice(["=", "=", "=", "<", ">", ">=", "<=", "IS", "<>", "IS NOT"])
    lhs, rhs = (column, other) if rng.random() < 0.5 else (other, column)
    test = rng.choice([
        f"{lhs} {operator} {rhs}", f"{lhs} {operator} {rhs}", f"{lhs} {operator} {rhs}", f"{lhs} {operator} {rhs}",
        f"{lhs} IN ({rhs}, '1', 2)", f"{lhs} NOT IN ({rhs}, 'a')", f"{lhs} BETWEEN {rhs} AND {rhs}",
        f"({lhs}, 1) = ({rhs}, 1)", f"({lhs}, 1) < ({rhs}, 2)", f"CASE {lhs} WHEN {rhs} THEN 1 ELSE 0 END",
        f"nullif({lhs}, {rhs}) IS NULL", f"max({lhs}, {rhs}) = {lhs}",
    ])
    if literal_source:
        tables = rng.choice([f"p, {source} AS j", f"p JOIN {source} AS j", f"{source} AS j, p", f"{source} AS j JOIN p"])
    else:
        tables = rng.choice([f"p, d, {source} AS j", f"d, {source} AS j, p", f"d JOIN {source} AS j JOIN p", f"p JOIN d ON d.id < 4 JOIN {source} AS j"])
    inner = f"SELECT {column} FROM {'' if literal_source else 'd, '}{source} AS j"
    name = "c"
    shape = rng.randrange(13)
    if shape <= 2:
        return f"SELECT p.id, {column} FROM {tables} WHERE {test}"
    if shape == 3:
        return f"SELECT p.id, {column}, {test} FROM {tables}"
    if shape == 4:
        return f"SELECT p.id, {column} FROM p LEFT JOIN {source} AS j ON {test}" if literal_source else f"SELECT p.id, {column} FROM p LEFT JOIN (d JOIN {source} AS j) ON {test}"
    if shape == 5:
        return f"SELECT p.id FROM p WHERE {typed} {rng.choice(['IN', 'NOT IN'])} ({inner}{' WHERE ' + column + ' IS NOT NULL' if rng.random() < 0.5 else ''})"
    if shape == 6:
        return f"SELECT {column} FROM {'' if literal_source else 'd, '}{source} AS j WHERE {column} {rng.choice(['IN', 'NOT IN'])} (SELECT {typed} FROM p{' WHERE ' + typed + ' IS NOT NULL' if rng.random() < 0.6 else ''})"
    if shape == 7:
        return f"SELECT p.id FROM p WHERE {rng.choice(['', 'NOT '])}EXISTS ({inner} WHERE {test})"
    if shape == 8:
        return f"SELECT p.id, (SELECT count(*) FROM ({inner}) AS s WHERE s.{column[2:]} {operator} {typed}) FROM p"
    if shape == 9:
        return f"WITH {name}(v) AS {rng.choice(['', 'MATERIALIZED ', 'NOT MATERIALIZED '])}({inner}) SELECT p.id, {name}.v FROM p, {name} WHERE {rng.choice([f'{name}.v {operator} {typed}', f'{typed} {operator} {name}.v'])}"
    if shape == 10:
        return f"SELECT count(*), sum({test}) FROM {tables}"
    if shape == 11:
        return f"SELECT p.id, count(*) FROM {tables} WHERE {test} GROUP BY p.id"
    return f"SELECT p.id, s.{column[2:]} FROM p JOIN ({inner} {rng.choice(['ORDER BY 1 LIMIT 100', 'LIMIT 100', ''])}) AS s ON {rng.choice([f's.{column[2:]} {operator} {typed}', f'{typed} {operator} s.{column[2:]}'])}"


UNTYPED_FOUND = [
    "SELECT p.t, j.key FROM p, json_each('[5,6,7]') AS j WHERE p.t = j.key",
    "SELECT p.t FROM p WHERE p.t NOT IN (SELECT key FROM json_each('[5,6,7]'))",
    "SELECT p.t, j.value FROM p LEFT JOIN json_each('[1,\"a\",2]') AS j ON j.value = p.t",
    "SELECT j.key FROM json_each('[5,6,7]') AS j WHERE j.key = '1'",
    "SELECT p.b, j.value FROM p, json_each('[1,\"1\",\"2\",2.5,\"a\"]') AS j WHERE p.b = j.value",
]


def untyped(seed):
    out = []
    for name, indexes, count in (("plain", [], 900), ("indexed", UNTYPED_INDEXES, 500)):
        database = Database("cp", name, UNTYPED_TABLES + indexes)
        rng = random.Random(f"{seed} cp {name}")
        for query in UNTYPED_FOUND:
            database.ask(query)
        while database.asked() < count:
            database.ask(untyped_query(rng))
        out.append(database)
    return out


# CX ------------------------------------------------------------------------------------------------
ROW_COLUMNS = ["i", "r", "t", "n", "b", "u", "nc", "rt", "tn"]
ROW_TABLE = "i INTEGER, r REAL, t TEXT, n NUMERIC, b BLOB, u, nc TEXT COLLATE NOCASE, rt TEXT COLLATE RTRIM, tn COLLATE NOCASE"
STRICT_TABLE = "i INTEGER, r REAL, t TEXT, n ANY, b BLOB, u ANY, nc TEXT COLLATE NOCASE, rt TEXT COLLATE RTRIM, tn TEXT COLLATE NOCASE"
ROWS = [
    "1, 1.0, '1', '1', '1', '1', 'ADMIN', 'x  ', 'Aa'", "2, 2.5, 'admin', 2, 2, 2, 'admin', 'x', 'aA'",
    "0, 0.0, 'x', 'a', x'61', 'a', 'Abc', 'abc ', 'x '", "NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL",
    "10, 10, 'ADMIN', '10', '10', 10, '10', '10 ', '10'", "7, 1.5, 'abc', 1.5, x'31', 1.5, 'ABC', 'ABC', 'abc'",
]
STRICT_ROWS = [
    "1, 1.0, '1', '1', x'31', '1', 'ADMIN', 'x  ', 'Aa'", "2, 2.5, 'admin', 2, x'32', 2, 'admin', 'x', 'aA'",
    "0, 0.0, 'x', 'a', x'61', 'a', 'Abc', 'abc ', 'x '", "NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL",
    "10, 10, 'ADMIN', '10', x'3130', 10, '10', '10 ', '10'", "7, 1.5, 'abc', 1.5, x'31', 1.5, 'ABC', 'ABC', 'abc'",
]
M_TABLE = [
    "CREATE TABLE m(id INTEGER PRIMARY KEY, t TEXT, nc TEXT COLLATE NOCASE, rt TEXT COLLATE RTRIM, i INTEGER, u)",
    "INSERT INTO m VALUES (1, 'admin', 'ADMIN', 'x', 1, '1'), (2, 'abc', 'x', 'abc  ', 2, 1), (3, 'ADMIN', 'Abc', 'ABC', 10, 'x'), (4, NULL, NULL, NULL, NULL, NULL)",
]
LITERALS = ["'admin'", "'ADMIN'", "'x'", "'x '", "'abc'", "'1'", "1", "10", "'10'", "2.5", "x'61'", "NULL", "0"]


def row_value(rng, rows):
    row = rng.choice(rows)
    column = rng.choice(["nc", "nc", "nc", "rt", "rt", "tn", "t", "t"] + ROW_COLUMNS + ["id", "rowid"])
    return f"{row}.{column}"


def row_test(rng, rows, depth=1, subqueries=True):
    x = row_value(rng, rows)
    y = rng.choice(LITERALS * 2 + [row_value(rng, rows)] * 8 + [f"(SELECT m.{rng.choice(['t', 'nc', 'rt'])} FROM m WHERE m.id = {rng.choice('123')})"] * (4 if subqueries else 0))
    if rng.random() < 0.2:
        y = f"{y} COLLATE {rng.choice(['NOCASE', 'RTRIM', 'BINARY'])}"
    if rng.random() < 0.1:
        x = rng.choice([f"+{x}", f"CAST({x} AS TEXT)", f"({x} || '')", f"{x} COLLATE {rng.choice(['NOCASE', 'RTRIM', 'BINARY'])}", f"lower({x})"])
    operator = rng.choice(["=", "=", "=", "<", ">", "<=", ">=", "<>", "IS", "IS NOT"])
    m = f"m.{rng.choice(['t', 't', 'nc', 'rt', 'i', 'u'])}"
    forms = [
        f"{x} {operator} {y}", f"{y} {operator} {x}", f"{x} {operator} {y}", f"{y} {operator} {x}",
        f"{x} IN ({y}, 'abc')", f"{x} NOT IN ({y}, 'x')", f"{y} IN ({x}, 'zz')",
        f"{x} {rng.choice(['IN', 'NOT IN'])} (SELECT {m} FROM m WHERE {m} IS NOT NULL)",
        f"{x} IN (SELECT {m} FROM m WHERE {m} <> {row_value(rng, rows)})",
        f"EXISTS (SELECT 1 FROM m WHERE {x} {operator} {m})", f"EXISTS (SELECT 1 FROM m WHERE {m} {operator} {x})",
        f"NOT EXISTS (SELECT 1 FROM m WHERE {m} = {x} AND {row_value(rng, rows)} IS NOT NULL)",
        f"(SELECT count(*) FROM m WHERE {x} {operator} {m})", f"(SELECT count(*) FROM m WHERE {m} {operator} {x})",
        f"(SELECT m.id FROM m WHERE {m} = {x} ORDER BY m.id LIMIT 1)",
        f"{x} BETWEEN {y} AND 'b'", f"'a' BETWEEN {x} AND {y}", f"CASE {x} WHEN {y} THEN 1 ELSE 0 END", f"CASE WHEN {x} = {y} THEN 'yes' END",
        f"nullif({x}, {y}) IS NULL", f"max({x}, {y}) IS {x}", f"min({y}, {x}) IS {y}",
        f"({x}, 1) = ({y}, 1)", f"({x}, {row_value(rng, rows)}) < ({y}, 'b')", f"({x}, 1) IN (SELECT {m}, 1 FROM m)",
        f"(SELECT count(DISTINCT v) FROM (SELECT {x} AS v UNION ALL SELECT {y}))", f"(SELECT count(*) FROM (SELECT {x} UNION SELECT {y}))",
        f"(SELECT group_concat(v) FROM (SELECT 'b' AS v WHERE {x} = {y}))",
        f"(WITH c(v) AS (SELECT {x}) SELECT count(*) FROM c WHERE c.v = {y})", f"(SELECT count(*) FROM (SELECT {x} AS v) AS s WHERE s.v {operator} {y})",
    ]
    test = rng.choice([form for form in forms if subqueries or "SELECT" not in form])
    if depth and rng.random() < 0.15:
        test = f"({test} {rng.choice(['AND', 'OR'])} {row_test(rng, rows, 0, subqueries)})"
    return test


def trigger(rng, number):
    event = rng.choice(["INSERT", "INSERT", "UPDATE", "UPDATE OF nc, t, i", "DELETE"])
    rows = {"INSERT": ["new"], "DELETE": ["old"]}.get(event, ["new", "old"])
    time = rng.choice(["AFTER", "AFTER", "BEFORE"])
    when = f" WHEN {row_test(rng, rows)}" if rng.random() < 0.6 else ""
    values = ", ".join(row_test(rng, rows) for _ in range(3))
    body = rng.choice([
        f"INSERT INTO lg VALUES ('g{number}', {values});",
        f"INSERT INTO lg VALUES ('g{number}', {values});",
        f"INSERT INTO lg VALUES ('g{number}', {values});",
        f"INSERT INTO lg SELECT 'g{number}', m.id, {row_test(rng, rows)}, 0 FROM m WHERE {row_value(rng, rows)} {rng.choice(['=', '<', '>='])} m.{rng.choice(['t', 'nc', 'rt'])};",
        f"UPDATE m2 SET hits = hits + 1 WHERE m2.{rng.choice(['t', 'nc', 'rt'])} = {row_value(rng, rows)}; INSERT INTO lg VALUES ('g{number}', {values});",
        f"UPDATE m2 SET hits = hits + 1 WHERE {row_value(rng, rows)} = m2.{rng.choice(['t', 'nc', 'rt'])};",
        f"DELETE FROM m2 WHERE {row_value(rng, rows)} = m2.{rng.choice(['t', 'nc', 'rt'])} AND m2.id > 2;",
        f"INSERT OR {rng.choice(['IGNORE', 'REPLACE'])} INTO kn VALUES ({row_value(rng, rows)}, 'g{number}');",
        f"INSERT INTO kn VALUES ({row_value(rng, rows)}, 'g{number}') ON CONFLICT(name) DO UPDATE SET who = excluded.who || {row_test(rng, rows, 1, False)} WHERE kn.name = {row_value(rng, rows)};",
        f"INSERT INTO kn VALUES (coalesce({row_value(rng, rows)}, 'none'), 'g{number}') ON CONFLICT DO NOTHING;",
        f"UPDATE OR {rng.choice(['IGNORE', 'REPLACE'])} kn SET name = {row_value(rng, rows)} WHERE {row_value(rng, rows)} = name;",
        f"SELECT CASE WHEN {row_test(rng, rows)} THEN RAISE(IGNORE) END; INSERT INTO lg VALUES ('g{number}', {values});",
    ])
    return f"CREATE TRIGGER g{number} {time} {event} ON t FOR EACH ROW{when} BEGIN {body} END"


def writes(rng, strict):
    rows = STRICT_ROWS if strict else ROWS
    at = lambda: rng.randrange(1, 9)
    out = [f"INSERT INTO t VALUES (1, {rows[0]}), (2, {rows[1]}), (3, {rows[2]})"]
    for _ in range(7):
        row = rng.choice(rows)
        out.append(rng.choice([
            f"INSERT INTO t VALUES ({at() + 10}, {row})",
            f"INSERT OR REPLACE INTO t VALUES ({at()}, {row})",
            f"INSERT OR IGNORE INTO t VALUES ({at()}, {row})",
            f"REPLACE INTO t VALUES ({at()}, {row})",
            f"INSERT INTO t VALUES ({at()}, {row}) ON CONFLICT(id) DO UPDATE SET nc = excluded.nc, t = excluded.t, i = excluded.i + 1",
            f"INSERT INTO t VALUES ({at()}, {row}) ON CONFLICT(id) DO NOTHING",
            f"UPDATE t SET nc = {rng.choice(LITERALS[:7])} WHERE id = {at()}",
            f"UPDATE t SET t = {rng.choice(LITERALS[:7])}, rt = 'x '",
            f"UPDATE OR REPLACE t SET id = {at()}, nc = 'ADMIN' WHERE id = {at()}",
            f"UPDATE OR IGNORE t SET id = {at()}, i = 5 WHERE id = {at()}",
            f"UPDATE t SET i = i + 1, nc = upper(nc) WHERE id < 4",
            f"DELETE FROM t WHERE id = {at()}",
            "DELETE FROM t WHERE nc = 'admin'",
        ]))
    return out


ROW_FOUND = [
    "CREATE TRIGGER f1 AFTER INSERT ON t WHEN new.nc = 'admin' BEGIN INSERT INTO lg VALUES ('f1', new.nc = 'admin', new.nc IN ('admin'), new.rt = 'x'); END",
    "CREATE TRIGGER f2 AFTER UPDATE ON t WHEN old.nc = new.t BEGIN INSERT INTO lg VALUES ('f2', old.nc = new.t, new.t = old.nc, old.id = '1'); END",
    "CREATE TRIGGER f3 AFTER INSERT ON t WHEN EXISTS (SELECT 1 FROM m WHERE new.nc = m.t) BEGIN INSERT INTO lg VALUES ('f3', nullif(new.nc, 'admin'), new.id = '1', new.i = '1'); END",
]


def triggers(seed):
    out = []
    for number in range(160):
        rng = random.Random(f"{seed} cx {number}")
        strict = number % 4 == 3
        table = STRICT_TABLE if strict else ROW_TABLE
        statements = [f"CREATE TABLE t(id INTEGER PRIMARY KEY, {table}){' STRICT' if strict else ''}"] + M_TABLE + [
            "CREATE TABLE m2(id INTEGER PRIMARY KEY, t TEXT, nc TEXT COLLATE NOCASE, rt TEXT COLLATE RTRIM, hits INTEGER DEFAULT 0)",
            "INSERT INTO m2(id, t, nc, rt) VALUES (1, 'admin', 'ADMIN', 'x'), (2, 'abc', 'x', 'abc  '), (3, 'ADMIN', 'Abc', 'ABC')",
            f"CREATE TABLE kn(name {rng.choice(['TEXT', 'TEXT COLLATE NOCASE', 'TEXT COLLATE RTRIM', ''])} UNIQUE, who)",
            "INSERT INTO kn VALUES ('admin', 'start'), ('x', 'start')",
            "CREATE TABLE lg(g, a, b, c)",
        ]
        if number % 2:
            statements += ["CREATE INDEX m_t ON m(t)", "CREATE INDEX m_nc ON m(nc)", "CREATE INDEX t_nc ON t(nc)"]
        database = Database("cx", f"{'strict' if strict else 'rows'}{number}", statements)
        for statement in (ROW_FOUND if number < 8 else []):
            database.run(statement)
        made = 0
        while made < 8:
            made += database.run(trigger(rng, made))
        for statement in writes(rng, strict):
            database.run(statement)
        database.ask("SELECT * FROM lg")
        database.ask("SELECT * FROM t")
        database.ask("SELECT * FROM m2")
        database.ask("SELECT * FROM kn")
        out.append(database)
    return out


# CR ------------------------------------------------------------------------------------------------
NESTED_TABLES = [
    "CREATE TABLE p(id INTEGER PRIMARY KEY, k INT, v INT, s TEXT)",
    "CREATE TABLE q(id INTEGER PRIMARY KEY, k INT, v INT, s TEXT COLLATE NOCASE)",
    "CREATE TABLE n(k INT, v INT, s TEXT)",
    "CREATE TABLE e(k INT, v INT, s TEXT)",
    "CREATE TABLE w(k INT PRIMARY KEY, v INT, s TEXT) WITHOUT ROWID",
    "INSERT INTO p VALUES (1, 1, 2, '1'), (2, 2, 3, 'b'), (3, NULL, 1, 'B'), (4, 4, NULL, NULL), (5, 1, 0, '2')",
    "INSERT INTO q VALUES (1, 2, 2, 'B'), (2, 2, 0, 'a'), (3, 5, 1, '1'), (4, NULL, 4, NULL), (5, 1, 1, 'A')",
    "INSERT INTO n VALUES (1, 2, '2'), (NULL, 1, 'b'), (2, 2, NULL), (3, 0, 'A'), (1, NULL, '1')",
    "INSERT INTO w VALUES (1, 1, 'a'), (2, 0, '2'), (6, NULL, NULL)",
]
NESTED_INDEXES = ["CREATE INDEX p_k ON p(k)", "CREATE INDEX q_k ON q(k, v)", "CREATE INDEX n_v ON n(v)", "CREATE INDEX w_v ON w(v)", "CREATE INDEX e_k ON e(k)"]


def nested_query(rng):
    source = lambda: rng.choice(["p", "q", "n", "w", "p", "q", "n", "e"])
    k = lambda alias: f"{alias}.{rng.choice('kkv')}"
    tables = f"{source()} AS a0"
    names = ["a0"]
    if rng.random() < 0.5:
        tables += f" {rng.choice(['JOIN', 'LEFT JOIN'])} {source()} AS a1 ON {k('a0')} = {k('a1')}"
        names.append("a1")
    outer = rng.choice(names)
    correlated = rng.choice([f"{k(outer)} = {k('s2')}", f"{k('s2')} = {k(outer)}", f"{k(outer)} = s2.v AND s2.v IN (2, 1)", f"{k('s1')} = {k('s2')}", f"{k(outer)} = {k('s2')} AND {k('s1')} <> {k('s2')}", f"{k(outer)} < {k('s2')}", "1"])
    inner = rng.choice([
        f"{k('s1')} {rng.choice(['IN', 'IN', 'NOT IN'])} (SELECT {k('s2')} FROM {source()} AS s2 WHERE {correlated})",
        f"{k('s1')} {rng.choice(['IN', 'IN', 'NOT IN'])} (SELECT {k('s2')} FROM {source()} AS s2 WHERE {correlated})",
        f"{rng.choice(['', 'NOT '])}EXISTS (SELECT 1 FROM {source()} AS s2 WHERE {correlated} AND {k('s2')} = {k('s1')})",
        f"{k('s1')} = (SELECT max({k('s2')}) FROM {source()} AS s2 WHERE {correlated})",
        f"{k('s1')} > (SELECT count(*) FROM {source()} AS s2 WHERE {correlated})",
        f"({k('s1')}, 1) IN (SELECT {k('s2')}, 1 FROM {source()} AS s2 WHERE {correlated})",
        f"s1.s IN (SELECT s2.s FROM {source()} AS s2 WHERE {correlated})",
        f"{k('s1')} IN (SELECT {k('s2')} FROM {source()} AS s2 JOIN {source()} AS s3 ON {k('s2')} = {k('s3')} WHERE {correlated})",
    ])
    beside = rng.choice(["", "", f" AND {k('s1')} = {k(outer)}", f" AND {k('s1')} IS NOT NULL", f" AND {k('s1')} <= {k(outer)}"])
    test = rng.choice([
        f"{rng.choice(['', 'NOT ', 'NOT '])}EXISTS (SELECT 1 FROM {source()} AS s1 WHERE {inner}{beside})",
        f"{rng.choice(['', 'NOT ', 'NOT '])}EXISTS (SELECT 1 FROM {source()} AS s1 WHERE {inner}{beside})",
        f"{k(outer)} {rng.choice(['IN', 'NOT IN'])} (SELECT {k('s1')} FROM {source()} AS s1 WHERE {inner}{beside})",
        f"{k(outer)} >= (SELECT count(*) FROM {source()} AS s1 WHERE {inner}{beside})",
    ])
    more = f" AND {rng.choice(['', 'NOT '])}EXISTS (SELECT 1 FROM {source()} AS s9 WHERE {k('s9')} = {k(outer)})" if rng.random() < 0.25 else ""
    return f"SELECT {', '.join(f'{name}.k, {name}.v' for name in names)} FROM {tables} WHERE {test}{more}"


NESTED_FOUND = [
    "SELECT a0.k FROM n AS a0 WHERE NOT EXISTS (SELECT 1 FROM e AS s3 WHERE s3.k IN (SELECT s4.s FROM p AS s4 WHERE a0.v = s4.v))",
    "SELECT a0.k FROM n AS a0 WHERE NOT EXISTS (SELECT 1 FROM p AS s3 WHERE s3.k IN (SELECT s4.k FROM p AS s4 WHERE a0.v = s4.v))",
    "SELECT a0.k FROM n AS a0 WHERE EXISTS (SELECT 1 FROM p AS s3 WHERE s3.k IN (SELECT s4.k FROM p AS s4 WHERE a0.v = s4.v))",
    "SELECT a0.k, a0.v, a1.k, a1.v, (SELECT max(s1.v) FROM q AS s1 WHERE s1.k IN (SELECT s2.k FROM p AS s2 WHERE a1.k IS s2.v)) FROM w AS a0 LEFT JOIN p AS a1 ON a0.k = a1.v WHERE NOT EXISTS (SELECT 1 FROM e AS s3 WHERE s3.k IN (SELECT s4.s FROM p AS s4 WHERE a0.v = s4.v AND s4.v IN (2, 2))) AND EXISTS (SELECT 1 FROM (SELECT value AS k, key AS v, type AS s FROM json_each(json_array(NULL, a1.k, 2))) AS s5 WHERE a1.k IN (SELECT s6.v FROM q AS s6 WHERE a1.v IN (1, 2))) AND 1 AND a1.s",
]


def nested(seed):
    out = []
    for name, indexes, count in (("plain", [], 500), ("indexed", NESTED_INDEXES, 400)):
        database = Database("cr", name, NESTED_TABLES + indexes)
        rng = random.Random(f"{seed} cr {name}")
        for query in NESTED_FOUND:
            database.ask(query)
        while database.asked() < count:
            database.ask(nested_query(rng))
        out.append(database)
    return out


# DE ------------------------------------------------------------------------------------------------
GROUP_TABLES = [
    "CREATE TABLE t(id INTEGER PRIMARY KEY, nc TEXT COLLATE NOCASE, rt TEXT COLLATE RTRIM, b TEXT, i INTEGER, u, tn COLLATE NOCASE)",
    "INSERT INTO t VALUES (1, 'ABC', 'abc ', 'abc', 1, 'ABC', 'x'), (2, 'abc', 'abc', 'ABC', 2, 'abc', 'X'), (3, 'x', 'x  ', 'x', 1, 1, 'abc'), (4, 'Abc', 'ABC', 'x ', 2, 'x', NULL)",
    "INSERT INTO t VALUES (5, NULL, NULL, NULL, NULL, NULL, 'ABC'), (6, '1', '1 ', '1', 1, '1', 1), (7, 'X', 'x', 'abc ', 3, 'abc ', 'b'), (8, 'b', 'b', 'B', 3, 'B', 'B')",
    "CREATE TABLE s(id INTEGER PRIMARY KEY, nc TEXT COLLATE NOCASE, b TEXT, i INTEGER)",
    "INSERT INTO s VALUES (1, 'abc', 'ABC', 1), (2, 'X', 'x', 2), (3, 'b', 'abc', 3), (4, NULL, NULL, 1)",
]
GROUP_INDEXES = ["CREATE INDEX t_nc ON t(nc)", "CREATE INDEX t_rt ON t(rt, i)", "CREATE INDEX t_i ON t(i)", "CREATE INDEX s_i ON s(i, nc)"]


def group_column(rng):
    return f"t.{rng.choice(['nc', 'nc', 'nc', 'rt', 'rt', 'tn', 'b', 'u', 'i'])}"


def group_test(rng, subqueries=True):
    x = group_column(rng)
    y = rng.choice(["'abc'", "'ABC'", "'x'", "'x '", "1", "'1'", group_column(rng), group_column(rng), f"{group_column(rng)} COLLATE {rng.choice(['NOCASE', 'RTRIM', 'BINARY'])}"])
    if rng.random() < 0.15:
        x = rng.choice([f"+{x}", f"CAST({x} AS TEXT)", f"({x} || '')", f"{x} COLLATE {rng.choice(['NOCASE', 'RTRIM', 'BINARY'])}"])
    operator = rng.choice(["=", "=", "<", ">", "<=", ">=", "<>", "IS", "IS NOT"])
    plain = [
        f"{x} {operator} {y}", f"{y} {operator} {x}", f"{x} {operator} {y}", f"{x} IN ({y}, 'b')", f"{x} NOT IN ({y}, 'x')",
        f"{x} BETWEEN {y} AND 'b'", f"CASE {x} WHEN {y} THEN 1 ELSE 0 END", f"nullif({x}, {y}) IS NULL",
        f"({x}, 1) = ({y}, 1)", f"max({x}, {y}) IS {x}",
    ]
    return rng.choice(plain if not subqueries else plain + [
        f"{x} IN (SELECT s.nc FROM s)", f"{x} IN (SELECT s.b FROM s WHERE s.b IS NOT NULL)",
        f"EXISTS (SELECT 1 FROM s WHERE s.b = {x})", f"EXISTS (SELECT 1 FROM s WHERE {x} = s.b)", f"(SELECT count(*) FROM s WHERE {x} > s.b)",
    ])


def aggregate(rng):
    test = group_test(rng)
    x = group_column(rng)
    call = rng.choice([
        f"max({test})", f"sum({test})", f"min({test})", f"total({test})", f"count(nullif({test}, 0))", f"max(quote({test}))", f"sum({test} AND {group_test(rng)})",
        f"count(DISTINCT {x})", f"count(DISTINCT {x} COLLATE {rng.choice(['NOCASE', 'RTRIM', 'BINARY'])})", f"lower(rtrim(max({x})))", f"lower(rtrim(min({x})))",
        f"max({x}) = 'abc'", f"min({x}) >= 'ABC'", f"sum(DISTINCT {test})",
    ])
    if rng.random() < 0.25 and not call.startswith(("lower", "max(t", "min(t")):
        call += f" FILTER (WHERE {group_test(rng)})"
    return call


def group_query(rng):
    subquery = rng.choice([
        "t.id IN (SELECT s.id FROM s WHERE s.nc = t.nc)", "t.i IN (SELECT s.i FROM s WHERE s.b = t.b)", "EXISTS (SELECT 1 FROM s WHERE s.i = t.i)",
        "(SELECT max(s.i) FROM s WHERE s.nc = t.nc)", "CASE (3 = t.i) WHEN (t.id IN (SELECT s.id FROM s WHERE s.nc == t.id)) THEN t.i END", "t.i IN (SELECT s.i FROM s)",
        "(SELECT count(*) FROM s WHERE s.b > t.nc)",
    ])
    key = rng.choice([subquery] * 4 + ["t.i", "t.i % 2", "lower(t.nc)", f"t.i, {subquery}", f"{subquery}, lower(rtrim(t.rt))", "length(t.b)", group_test(rng), "t.nc COLLATE BINARY"])
    calls = [aggregate(rng) for _ in range(rng.choice([1, 2, 3]))]
    tables = rng.choice(["t", "t", "t", "t JOIN s ON s.i = t.i", "t LEFT JOIN s ON s.nc = t.nc", "(SELECT * FROM t) AS t", "(SELECT * FROM t ORDER BY id LIMIT 7) AS t"])
    where = f" WHERE {group_test(rng)}" if rng.random() < 0.2 else ""
    shape = rng.random()
    if shape < 0.12:
        return f"SELECT {', '.join(calls)} FROM {tables}{where}"
    if shape < 0.3:
        window = rng.choice(["ORDER BY t.id", f"PARTITION BY {rng.choice(['t.i', 't.nc', subquery])} ORDER BY t.id", "PARTITION BY t.nc", "ORDER BY t.id ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING"])
        test = group_test(rng)
        calls = [f"{rng.choice(['sum', 'max', 'count', 'min'])}({group_test(rng, False)}) OVER ({window})" for _ in range(2)]
        return f"SELECT t.id, {', '.join(calls)}, {test} FROM {rng.choice(['t', '(SELECT * FROM t) AS t'])}{where}"
    having = f" HAVING {rng.choice([aggregate(rng), f'{aggregate(rng)} > 0', f'max({group_test(rng)})'])}" if rng.random() < 0.3 else ""
    order = f" ORDER BY {aggregate(rng)}" if rng.random() < 0.2 else ""
    shown = "" if key == "t.nc COLLATE BINARY" else f"{key}, "
    return f"SELECT {shown}{', '.join(calls)} FROM {tables}{where} GROUP BY {key}{having}{order}"


GROUP_FOUND = [
    "SELECT CASE (3 = t.i) WHEN (t.id IN (SELECT s.id FROM s WHERE s.nc == t.id)) THEN 1 END, max(quote(t.nc IS 'abc')) FROM t GROUP BY CASE (3 = t.i) WHEN (t.id IN (SELECT s.id FROM s WHERE s.nc == t.id)) THEN 1 END",
    "SELECT t.id IN (SELECT s.id FROM s WHERE s.nc = t.nc), sum(t.nc = 'abc') FILTER (WHERE t.nc = 'abc'), max(CASE t.nc WHEN 'abc' THEN 1 ELSE 0 END), sum(t.rt = 'x') FROM t GROUP BY 1",
]


def groups(seed):
    out = []
    for name, indexes, count in (("plain", [], 700), ("indexed", GROUP_INDEXES, 500)):
        database = Database("de", name, GROUP_TABLES + indexes)
        rng = random.Random(f"{seed} de {name}")
        for query in GROUP_FOUND:
            database.ask(query)
        while database.asked() < count:
            database.ask(group_query(rng))
        out.append(database)
    return out


def main():
    seed = sys.argv[1] if len(sys.argv) > 1 else "C21"
    print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_columns_read.py {seed}")
    for databases in (arguments(seed), untyped(seed), triggers(seed), nested(seed), groups(seed)):
        asked = sum(database.asked() for database in databases)
        refused = sum(database.refused for database in databases)
        print(f"# {databases[0].lines[0].split(chr(9))[1]}: {len(databases)} databases, {asked} queries; SQLite refused {refused} drawn", file=sys.stderr)
        for database in databases:
            database.print()


if __name__ == "__main__":
    main()
