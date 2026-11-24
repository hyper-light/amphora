#!/usr/bin/env python3
"""crates/engine/tests/fixtures/window_columns.tsv: the columns a SELECT with window functions hands the query around it,
as SQLite has them (AC-T1.8.6; lane C25).

A case is one inner SELECT, of up to three windows (inline and named; calls alone, twice, inside expressions) beside what
its rows hold: plain columns, the rowid, expressions, aggregates of its groups, scalar subqueries, EXISTS and IN, each
named with AS or left to its text; over a table, a join, groups, DISTINCT, with and without ORDER BY and LIMIT. It is read
through `*` and `name.*` in FROM, a CTE, a view, a compound's arm, a join and another SELECT, by its columns' names, and
by CREATE TABLE AS and INSERT. Beside each: the rows (sorted, but where an ORDER BY of every column sorts them).

usage: bench/sqlite_window_columns.py COUNT SEED > crates/engine/tests/fixtures/window_columns.tsv   (SQLite 3.54.0 in Python)
"""
import random
import sqlite3
import struct
import sys

SETUP = [
    "CREATE TABLE t(a INTEGER PRIMARY KEY, g INT, v INT, x TEXT)",
    "CREATE TABLE s(b INT, h INT)",
    "CREATE TABLE w(k TEXT PRIMARY KEY, m INT) WITHOUT ROWID",
    "INSERT INTO t VALUES (1, 1, 25, 'a'), (2, 1, NULL, 'b'), (3, 2, 2, 'A'), (4, NULL, 25, NULL), (5, 2, -3, 'b'), (6, 3, 7, 'c')",
    "INSERT INTO s VALUES (3, 1), (5, 2), (5, 3), (NULL, 2)",
    "INSERT INTO w VALUES ('a', 1), ('b', 2), ('d', 3)",
]


class Inner:
    """A SELECT with windows, and how many columns it has."""

    def __init__(self, rng, width=None, alias=None):
        self.rng = rng
        pick = rng.choice
        self.shape = pick(["table"] * 5 + ["join", "join", "grouped", "grouped", "keyed"])
        if self.shape == "grouped":
            self.ints, self.key, self.source, self.tail = ["g", "count(*)", "sum(v)", "max(a)"], "g", "t", " GROUP BY g"
        elif self.shape == "join":
            self.ints, self.key, self.source, self.tail = ["t.a", "g", "v", "b", "h", "s.h"], "a, b, h", pick(["t JOIN s ON s.h = t.g", "t LEFT JOIN s ON s.h = t.g", "t, s"]), ""
        elif self.shape == "keyed":
            self.ints, self.key, self.source, self.tail = ["m", "m + 1"], "k", "w", ""
        else:
            self.ints, self.key, self.source, self.tail = ["a", "g", "v", "t.a", "t.v"], "a", "t", pick(["", "", " WHERE a > 1"])
        self.windows = []
        for _ in range(pick([0, 1, 2, 2, 2, 3, 3])):
            over = pick(["", f"ORDER BY {self.key}", f"ORDER BY {self.ints[0]} DESC, {self.key}", f"PARTITION BY {self.ints[0]} ORDER BY {self.key}", f"PARTITION BY {self.ints[0]}",
                         f"ORDER BY {self.key} ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING", f"ORDER BY {self.key} ROWS BETWEEN 1 PRECEDING AND CURRENT ROW"])
            self.windows.append(over)
        self.named = []
        if self.windows and rng.random() < 0.3:
            self.named = [(f"w{i}", over) for i, over in enumerate(self.windows)]
        distinct = "DISTINCT " if rng.random() < 0.12 else ""
        columns = [self.column(i) for i in range(width or rng.randint(1, 6))]
        if width is None and self.shape in ("table", "keyed") and rng.random() < 0.12:
            columns.append("*")
        rng.shuffle(columns)
        self.names = [c.rsplit(" AS ", 1)[1] if " AS y" in c else c for c in columns]
        clause = " WINDOW " + ", ".join(f"{n} AS ({o})" for n, o in self.named) if self.named else ""
        self.columns = columns
        self.body = f"SELECT {distinct}{', '.join(columns)} FROM {self.source}{self.tail}{clause}"
        self.distinct = bool(distinct)

    def width(self):
        return sum(1 if c != "*" else {"table": 4, "keyed": 2}[self.shape] for c in self.columns)

    def call(self):
        rng = self.rng
        at = rng.randrange(len(self.windows))
        over = self.named[at][0] if self.named else f"({self.windows[at]})"
        arg = rng.choice(self.ints)
        return rng.choice([f"sum({arg})", f"min({arg})", f"max({arg})", f"count({arg})", f"count(*)", f"total({arg})"]
                          + ([f"row_number()", f"rank()", f"lag({arg})", f"first_value({arg})"] if self.key in self.windows[at] else [])) + f" OVER {over}"

    def column(self, i):
        rng = self.rng
        drawn = rng.random()
        if self.windows and drawn < 0.5:
            text = self.call()
            if drawn < 0.12:
                text = rng.choice([f"{text} + 1", f"({text})", f"coalesce({text}, {self.call()})", f"{text} || '-' || {self.call()}", f"CASE WHEN {text} > 2 THEN {self.call()} END", f"{text} COLLATE NOCASE"])
        elif drawn < 0.75:
            text = rng.choice(self.ints)
            if rng.random() < 0.3:
                text = rng.choice([f"{text} + 1", f"-{text}", f"typeof({text})", f"{text} IS NULL", "7", "'k'"])
            elif self.shape in ("table", "join") and rng.random() < 0.15:
                text = "t.rowid"
        else:
            outer = rng.choice(self.ints)
            text = rng.choice(["(SELECT max(b) FROM s)", "EXISTS (SELECT 1 FROM s WHERE b > 4)", f"{outer} IN (SELECT b FROM s)", "(SELECT count(*) FROM w) + 1", f"{outer} NOT IN (SELECT m FROM w)", f"{outer} BETWEEN 1 AND (SELECT max(b) FROM s)"]
                              + ([f"(SELECT max(b) FROM s WHERE s.b >= {outer})", f"EXISTS (SELECT 1 FROM s AS z WHERE z.b = {outer})"] if self.shape == "table" else []))
        return f"{text} AS y{i}" if rng.random() < 0.4 else text


def question(rng):
    """The statements that make what the query reads, and the query; whether its rows are in order."""
    inner = Inner(rng)
    n = inner.width()
    made = []
    body = inner.body
    if rng.random() < 0.15 and not inner.distinct:
        body += f" ORDER BY {', '.join(str(i + 1) for i in range(n))}" + rng.choice(["", " LIMIT 4"])
    if rng.random() < 0.2 and "ORDER BY 1" not in body:
        other = Inner(rng, width=n)
        op = rng.choice(["UNION ALL", "UNION ALL", "UNION", "EXCEPT", "INTERSECT"])
        body = f"{body} {op} {other.body}" if rng.random() < 0.7 else f"{other.body} {op} {body}"
    form = rng.choice(["from"] * 4 + ["qualified", "cte", "cte", "materialized", "view", "view", "nested", "beside", "join", "left join", "named", "count", "table as", "insert", "over again", "twice", "top", "where", "exists", "using"])
    source = f"({body})"
    if form == "view":
        made.append(f"CREATE VIEW vw AS {body}")
        source = "vw"
    query = {
        "from": f"SELECT * FROM {source}", "view": f"SELECT * FROM {source}", "qualified": f"SELECT q.* FROM {source} AS q",
        "cte": f"WITH c AS {source} SELECT * FROM c", "materialized": f"WITH c AS MATERIALIZED {source} SELECT * FROM c", "nested": f"SELECT * FROM (SELECT * FROM {source})",
        "beside": f"SELECT *, 7 AS seven FROM {source}", "join": f"SELECT * FROM {source} AS q JOIN s ON s.h = 2", "left join": f"SELECT q.*, s.h FROM s LEFT JOIN {source} AS q ON s.b = 5",
        "count": f"SELECT count(*) FROM {source}", "over again": f"SELECT *, count(*) OVER () FROM {source}", "twice": f"WITH c AS {source} SELECT * FROM c AS one JOIN (SELECT count(*) AS n FROM c) AS two",
        "top": body, "where": f"SELECT a FROM t WHERE a IN (SELECT count(*) FROM {source})", "exists": f"SELECT * FROM {source} WHERE EXISTS (SELECT * FROM {source})",
        "using": f"SELECT * FROM {source} NATURAL JOIN (SELECT 1 AS one)", "table as": "SELECT * FROM made", "insert": "SELECT * FROM made", "named": "",
    }[form]
    if form == "table as":
        made.append(f"CREATE TABLE made AS SELECT * FROM {source}")
    if form == "insert":
        made.append(f"CREATE TABLE made({', '.join(f'm{i}' for i in range(n))})")
        made.append(f"INSERT INTO made SELECT * FROM {source}")
    if form == "named":
        name = rng.choice([x for x in inner.names if x != "*"] or ["1"])
        name = name.split(".")[-1] if name in ("t.a", "t.v", "s.h", "t.rowid") else name
        query = 'SELECT "' + name + f'" FROM {source}'
    ordered = False
    if form in ("from", "view", "cte", "qualified", "nested") and rng.random() < 0.4:
        query += " ORDER BY " + ", ".join(f"{i + 1}{rng.choice(['', ' DESC'])}" for i in range(n))
        ordered = True
    return made, query, ordered


def cell(v):
    if v is None:
        return "NULL"
    if isinstance(v, float):
        return "r" + struct.pack(">d", v).hex()
    return f"'{v}'" if isinstance(v, str) else str(v)


def main():
    count, seed = int(sys.argv[1]), int(sys.argv[2])
    rng = random.Random(seed)
    print(f"# bench/sqlite_window_columns.py {count} {seed}, SQLite {sqlite3.sqlite_version}")
    for sql in SETUP:
        print(f"sql\t{sql}")
    made_count = 0
    while made_count < count:
        made, query, ordered = question(rng)
        conn = sqlite3.connect(":memory:", isolation_level=None)
        for sql in SETUP:
            conn.execute(sql)
        try:
            for sql in made:
                conn.execute(sql)
            rows = [",".join(cell(v) for v in row) for row in conn.execute(query).fetchall()]
        except sqlite3.Error:
            continue
        if len(rows) > 40:
            continue
        made_count += 1
        print("case")
        for sql in made:
            print(f"make\t{sql}")
        print(f"{'order' if ordered else 'rows'}\t{query}\t{' / '.join(rows if ordered else sorted(rows))}")


main()
