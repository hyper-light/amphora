"""Oracle finding DC (lane C17): a cursor read where it was never opened. Correlated IN, NOT IN, EXISTS and
scalar subqueries in the select list, GROUP BY, HAVING, ORDER BY and aggregates of grouped and aggregate
queries over LEFT, RIGHT and FULL JOINs; and row-value ranges of one and of several tables' columns, with
the automatic indexes their joins build. Writes each query with the answer SQLite itself gives (the
sqlite3 library Python links, printed first) to crates/engine/tests/fixtures/unread_cursors.tsv, which
tests/windows_and_triggers.rs holds Amphora to.

Each line is `db <name>`, `sql <statement>` (the database's tables and rows) or `q <query> <answer>`:
the answer's rows sorted, each row's values joined by commas, a NULL written NULL, the rows joined by
semicolons.
usage: python3 bench/sqlite_unread_cursors.py [queries per database [seed]] > crates/engine/tests/fixtures/unread_cursors.tsv"""
import random
import sqlite3
import sys

TABLES = [
    "CREATE TABLE o(k INT, n INT)",
    "CREATE TABLE p(k INT, n INT)",
    "CREATE TABLE q(id INTEGER PRIMARY KEY, k INT, n INT)",
    "CREATE TABLE w(k INT PRIMARY KEY, n INT) WITHOUT ROWID",
    "INSERT INTO p VALUES (1, 1), (2, 5), (2, 7), (NULL, 3), (4, NULL)",
    "INSERT INTO q VALUES (1, 2, 5), (2, 3, 1), (3, NULL, 2), (4, 2, 9)",
    "INSERT INTO w VALUES (1, 4), (2, 1), (5, NULL)",
]
INDEXES = ["CREATE INDEX p_k ON p(k)", "CREATE INDEX q_kn ON q(k, n)"]
ROWS = ["INSERT INTO o VALUES (2, 1), (9, 2)"]
SOURCES = ["o", "p", "q", "w"]
# What the oracles found Amphora failing: each is asked first.
FOUND = [
    "SELECT b.n NOT IN (SELECT s.k FROM p AS s WHERE b.k IS 5) FROM o AS b FULL JOIN w AS c ON NULL GROUP BY b.n NOT IN (SELECT s.k FROM p AS s WHERE b.k IS 5)",
    "SELECT d.c0 FROM (SELECT (CASE WHEN (b.n NOT IN (SELECT s.k FROM w AS s WHERE (b.k IS '1e3'))) THEN b.k ELSE b.n END) AS c0 FROM o AS b FULL JOIN w AS c ON NULL GROUP BY (CASE WHEN (b.n NOT IN (SELECT s.k FROM w AS s WHERE (b.k IS '1e3'))) THEN b.k ELSE b.n END)) AS d",
    "SELECT a1.c0 FROM w AS a0, (SELECT CAST((b.k NOT IN (SELECT s.k FROM o AS s WHERE (c.n NOT IN (3)))) AS INTEGER) AS c0 FROM o AS c FULL JOIN w AS b GROUP BY CAST((b.k NOT IN (SELECT s.k FROM o AS s WHERE (c.n NOT IN (3)))) AS INTEGER)) AS a1",
    "SELECT EXISTS (SELECT 1 FROM w AS s WHERE b.k IS NULL) FROM o AS b FULL JOIN w AS c ON NULL GROUP BY EXISTS (SELECT 1 FROM w AS s WHERE b.k IS NULL)",
    "SELECT 1 FROM p AS x JOIN w AS y ON (CASE WHEN x.k <= NULL THEN NULL END) JOIN p AS z ON ((x.k, NULL, z.n) BETWEEN (1, 1, x.k) AND (7, 10, 41)) WHERE z.n >= 1",
    "SELECT e.k, f.n FROM p e JOIN p f ON ((e.k, f.n) BETWEEN (1, 10) AND (2, 4))",
]


class Draw:
    def __init__(self, rng):
        self.rng = rng
        self.made = 0

    def column(self, scope):
        return f"{self.rng.choice(scope)}.{self.rng.choice('kkn')}"

    def subquery(self, scope):
        """A subquery's value: correlated with the FROM's tables, most often by one column."""
        rng = self.rng
        self.made += 1
        s = f"s{self.made}"
        table = rng.choice(SOURCES)
        outer = self.column(scope)
        where = rng.choice([f"{s}.k = {outer}", f"{s}.n < {outer}", f"{outer} IS NULL", f"{outer} IS NOT NULL", f"{outer} > 1", f"{s}.k IS {outer}", f"{s}.k = {outer} AND {s}.n <> {self.column(scope)}", "1"])
        drawn = rng.random()
        if drawn < 0.4:
            return f"{self.column(scope)} {rng.choice(['IN', 'NOT IN'])} (SELECT {s}.{rng.choice('kn')} FROM {table} AS {s} WHERE {where})"
        if drawn < 0.65:
            return f"{rng.choice(['', 'NOT '])}EXISTS (SELECT 1 FROM {table} AS {s} WHERE {where})"
        agg = rng.choice(["count(*)", f"max({s}.n)", f"min({s}.k)", f"sum({s}.n)"])
        return f"(SELECT {agg} FROM {table} AS {s} WHERE {where})"

    def source(self, joins):
        rng = self.rng
        tables = rng.choice([2, 2, 2, 3])
        scope = [f"a{i}" for i in range(tables)]
        sql = f"{rng.choice(SOURCES)} AS a0"
        for i in range(1, tables):
            join = rng.choice(joins)
            on = rng.choice([f"a{i}.k = a{i - 1}.k", f"a{i}.k = a{i - 1}.n", "NULL", f"a{i}.n > a{i - 1}.n", f"a{i}.k IS NULL"])
            sql += f" {join} {rng.choice(SOURCES)} AS a{i} ON {on}"
        return sql, scope

    def grouped(self):
        """A grouped or aggregate query whose bare columns are its group's keys, or of one row."""
        rng = self.rng
        source, scope = self.source(["LEFT JOIN", "RIGHT JOIN", "FULL JOIN", "FULL JOIN", "JOIN"])
        # SQLite takes a bare column from any row of its group; every column a subquery reads is a key.
        keys = []
        for a in scope:
            keys += [f"{a}.k", f"{a}.n"]
        shape = rng.random()
        if shape < 0.45:
            sub = self.subquery(scope)
            outputs = [sub, rng.choice(["count(*)", f"max({self.column(scope)})", f"sum({self.subquery(scope)})"])]
            return f"SELECT {', '.join(outputs)} FROM {source} GROUP BY {', '.join(keys)}"
        if shape < 0.65:
            sub = self.subquery(scope)
            return f"SELECT {rng.choice(['count(*)', 'count(*), ' + sub])} FROM {source} GROUP BY {sub}"
        if shape < 0.8:
            sub = self.subquery(scope)
            return f"SELECT {', '.join(keys)}, count(*) FROM {source} GROUP BY {', '.join(keys)} HAVING {rng.choice(['', 'NOT '])}({sub})"
        if shape < 0.9:
            sub = self.subquery(scope)
            return f"SELECT count(*), sum({self.subquery(scope)}), count(*) FILTER (WHERE {sub}) FROM {source}"
        sub = self.subquery(scope)
        return f"SELECT {', '.join(keys[:2])} FROM {source} GROUP BY {', '.join(keys)} ORDER BY {sub}, {', '.join(keys)} LIMIT {rng.choice([2, 5])}"

    def row_value(self):
        rng = self.rng
        source, scope = self.source(["JOIN", "JOIN", "JOIN", "LEFT JOIN", "FULL JOIN", "RIGHT JOIN"])
        element = lambda: rng.choice([self.column(scope), self.column(scope), str(rng.choice([1, 2, 5, 10])), "NULL"])
        width = rng.choice([2, 2, 3])
        # Amphora refuses a literal first in a row compared by BETWEEN (the oracles' CM).
        row = [self.column(scope)] + [element() for _ in range(width - 1)]
        low = [element() for _ in range(width)]
        high = [element() for _ in range(width)]
        join = lambda r: f"({', '.join(r)})"
        if rng.random() < 0.5:
            test = f"{join(row)} {rng.choice(['', 'NOT '])}BETWEEN {join(low)} AND {join(high)}"
        else:
            test = f"{join(row)} {rng.choice(['<', '<=', '>', '>=', '=', '<>'])} {join(high)}"
        more = rng.choice(["", "", f" AND {self.column(scope)} >= {rng.choice([1, 3])}", f" AND {self.column(scope)} = {self.column(scope)}"])
        columns = ", ".join(f"{a}.k, {a}.n" for a in scope)
        return f"SELECT {columns} FROM {source} WHERE {test}{more}"


def text(value):
    return "NULL" if value is None else str(value)


def answer(db, sql):
    rows = sorted(",".join(text(value) for value in row) for row in db.execute(sql).fetchall())
    return f"{len(rows)}:{';'.join(rows)}"


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 300
    seed = sys.argv[2] if len(sys.argv) > 2 else "DC"
    print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_unread_cursors.py {count} {seed}")
    for name, statements in (("plain", TABLES), ("rows", TABLES + ROWS), ("indexed", TABLES + ROWS + INDEXES)):
        db = sqlite3.connect(":memory:")
        print(f"db\t{name}")
        for statement in statements:
            db.execute(statement)
            print(f"sql\t{statement}")
        for sql in FOUND:
            print(f"q\t{sql}\t{answer(db, sql)}")
        rng = random.Random(f"{seed}-{name}")
        seen, drawn, refused = set(FOUND), 0, 0
        while drawn < count:
            draw = Draw(rng)
            sql = draw.grouped() if rng.random() < 0.6 else draw.row_value()
            if sql in seen:
                continue
            seen.add(sql)
            try:
                got = answer(db, sql)
            except sqlite3.Error:
                refused += 1
                continue
            if len(got) > 2000:
                continue
            drawn += 1
            print(f"q\t{sql}\t{got}")
        print(f"# {name}: {drawn} drawn, {refused} SQLite refused", file=sys.stderr)


main()
