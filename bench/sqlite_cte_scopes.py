"""Oracle findings DH and DI (lane C18): WITH clauses inside one another, whose CTEs take the names of outer ones, of
tables and of views, are read twice, by nothing, through views and views' own CTEs, by CTEs of an inner
WITH clause, recursively, from every clause of a query, with the rows of the queries around them, and in an INSERT, an UPDATE and a
DELETE. Writes each statement with the answer, or the error, SQLite itself gives (the sqlite3 library Python links,
printed first) to crates/engine/tests/fixtures/cte_scopes.tsv, which tests/cte_scopes.rs holds Amphora to: `q` a query and
its rows sorted, `w` a write and the rows its table then holds; an answer that begins with `error:` is SQLite's error.
Each CTE's column v holds a number of its own, so that a name read at a wrong place shows.
usage: python3 bench/sqlite_cte_scopes.py [statements per database [seed]] > crates/engine/tests/fixtures/cte_scopes.tsv"""
import random
import sqlite3
import sys

TABLES = [
    "CREATE TABLE t(k INT, v INT)",
    "CREATE TABLE q(k INT, v INT)",
    "CREATE TABLE f(k INT, v INT)",
    "CREATE TABLE log(k INT, v INT)",
    "INSERT INTO t VALUES (1, 10), (2, 20), (3, NULL)",
    "INSERT INTO q VALUES (2, 21), (2, 22), (3, 30), (NULL, 5)",
    "INSERT INTO f VALUES (1, 41), (4, 42)",
    "INSERT INTO log VALUES (1, 1), (2, 2), (3, 3)",
    "CREATE VIEW g AS SELECT k, v FROM q WHERE k > 1",
    "CREATE VIEW vf AS SELECT k, v FROM f",
    "CREATE VIEW vc AS WITH c AS (SELECT k + 1 AS k, v FROM t), d AS (SELECT k, v FROM c) SELECT x.k, d.v FROM c AS x JOIN d ON x.k = d.k",
    "CREATE VIEW vt AS SELECT k, v FROM t WHERE k IN (SELECT k FROM f) OR EXISTS (SELECT 1 FROM g WHERE g.k = t.k)",
]
INDEXES = ["CREATE INDEX t_k ON t(k)", "CREATE INDEX q_kv ON q(k, v)", "CREATE INDEX f_k ON f(k)"]
# f is a table and g a view: the CTEs of their names hide them.
NAMES = ["c", "d", "f", "g"]
SOURCES = ["t", "t", "q", "q", "f", "g", "vf", "vc", "vt"]
# The reductions: each is asked first.
FOUND = [
    # DH
    "WITH d AS (SELECT k FROM q) SELECT t.k FROM t WHERE EXISTS (WITH d AS (SELECT k FROM t WHERE k = 1) SELECT 1 FROM d AS s WHERE t.k IN (SELECT e.k FROM d AS e))",
    "WITH d AS (SELECT k FROM q) SELECT d.k FROM d WHERE EXISTS (WITH d AS (SELECT k FROM t) SELECT 1 FROM d AS s WHERE d.k IN (SELECT e.k FROM d AS e))",
    "WITH d AS (SELECT k FROM q) SELECT t.k FROM t WHERE EXISTS (WITH d AS (SELECT 1 AS k) SELECT 1 FROM (SELECT k FROM d) AS s WHERE s.k = t.k)",
    # DI
    "WITH d AS (SELECT k FROM nosuch) SELECT t.k FROM t WHERE EXISTS (WITH x AS (SELECT k FROM d) SELECT 1)",
    "WITH d AS (SELECT k FROM nosuch) SELECT t.k FROM t WHERE t.k IN (WITH x AS (SELECT k FROM d), y AS (SELECT 1 AS k) SELECT k FROM y)",
    "WITH d AS (SELECT k FROM nosuch) SELECT * FROM (WITH d AS (SELECT 1 AS k) SELECT k FROM d)",
    "WITH d AS (SELECT k FROM nosuch), y AS (WITH x AS (SELECT k FROM d) SELECT 7 AS k) SELECT k FROM y",
    "WITH a AS (WITH b AS (SELECT 1 AS k) SELECT k FROM b), b AS (SELECT k FROM a) SELECT * FROM b",
    "WITH a AS (WITH z AS (SELECT k FROM b) SELECT k FROM z), b AS (SELECT k FROM a) SELECT * FROM b",
    # An outer CTE read by two CTEs of an inner WITH clause.
    "WITH c AS (SELECT k FROM q) SELECT t.k, (WITH a AS (SELECT k FROM c), b AS (SELECT k FROM c) SELECT count(*) FROM a, b) FROM t",
    "WITH RECURSIVE c(k) AS (SELECT 1 UNION ALL SELECT k + 1 FROM c WHERE k < 3) SELECT (WITH a AS (SELECT k FROM c), b AS (SELECT k FROM c) SELECT count(*) FROM a, b)",
    "WITH c AS (SELECT k FROM q), x AS (WITH a AS (SELECT k FROM c), b AS (SELECT k FROM c) SELECT a.k AS ak, b.k AS bk FROM a, b) SELECT * FROM x",
    "WITH c AS (SELECT k FROM q), x AS (WITH z AS (SELECT k FROM c) SELECT c.k AS ck, z.k AS zk FROM c, z) SELECT * FROM x",
    # A view's body reads no CTE of the query that reads the view.
    "WITH q AS (SELECT 1 AS k, 1 AS v) SELECT k FROM t WHERE k IN (SELECT k FROM g) AND k IN (SELECT k + 1 FROM q)",
    # A CTE's body reads the CTEs around its definition, not those around its use.
    "WITH d AS (SELECT k FROM q), c AS (SELECT k FROM d) SELECT (WITH d AS (SELECT k FROM c) SELECT max(k) FROM d)",
    "SELECT d.k FROM t AS d WHERE EXISTS (WITH d AS (SELECT q.k FROM q WHERE q.k = d.k) SELECT 1 FROM d)",
    # A CTE that reads a row of the query around it, read twice.
    "SELECT t.k, (WITH c AS (SELECT q.k FROM q WHERE q.k >= t.k), a AS (SELECT x.k AS xk, y.k AS yk FROM c x, c y) SELECT count(*) FROM a) FROM t",
    "WITH c AS (SELECT 1 AS k) SELECT t.k FROM t WHERE (SELECT max(k) FROM c) IN (SELECT k FROM t)",
]


class Draw:
    def __init__(self, rng):
        self.rng = rng
        self.held = None

    def start(self):
        self.aliases = 0
        self.marks = 0
        # An outer CTE whose body reads a table that isn't there.
        self.bad = False
        # Whether the statement may hold a fault, which it then holds once: SQLite and Amphora meet several in orders of
        # their own.
        self.fault = self.rng.random() < 0.4
        self.held = None
        # The names a CTE's body doesn't read by chance: its own, and those of its WITH clause it reads none of.
        self.hidden = []
        # The CTE whose body is drawn next.
        self.body_of = None

    def alias(self):
        self.aliases += 1
        return f"a{self.aliases}"

    def mark(self):
        self.marks += 1
        return str(100 + self.marks)

    def column(self, scope):
        rng = self.rng
        at = scope[-1] if rng.random() < 0.6 else rng.choice(scope)
        return f"{at}.{rng.choice('kkkv')}"

    def value(self, scope):
        rng = self.rng
        return self.column(scope) if rng.random() < 0.6 else str(rng.choice([0, 1, 2, 3]))

    def plain(self, scope):
        rng = self.rng
        drawn = rng.random()
        if drawn < 0.6:
            return f"{self.column(scope)} {rng.choice(['=', '=', '<', '>=', '<>', 'IS'])} {self.value(scope)}"
        if drawn < 0.75:
            return f"{self.column(scope)} IS {rng.choice(['', 'NOT '])}NULL"
        if drawn < 0.85:
            return f"{self.column(scope)} IN ({self.value(scope)}, 2)"
        return f"({self.plain(scope)} OR {self.plain(scope)})"

    def source(self, ctes):
        rng = self.rng
        drawn = rng.random()
        hidden = set().union(*self.hidden) - set(ctes)
        unknown = [name for name in ("c", "d") if name not in ctes and name not in hidden]
        if drawn < 0.04 and unknown and self.take_fault("unknown"):
            self.unknown = rng.choice(unknown)
            return self.unknown
        if ctes and drawn < 0.8:
            return rng.choice(ctes)
        return rng.choice([source for source in SOURCES if source not in hidden])

    def take_fault(self, kind):
        taken, self.fault = self.fault, False
        if taken:
            self.held = kind
        return taken

    def cte(self, name, ctes, scope, depth):
        """The text of one CTE of a WITH clause, and whether it reads itself."""
        rng = self.rng
        hint = rng.choice(["", "", "", "", "MATERIALIZED ", "NOT MATERIALIZED "])
        drawn = rng.random()
        one, two = self.source(ctes), self.source(ctes)
        if drawn < 0.16:
            return f"{name} AS {hint}(SELECT k, {self.mark()} AS v FROM {one})"
        if drawn < 0.22:
            return f"{name}(k, v) AS {hint}(SELECT {rng.choice([1, 2, 3])}, {self.mark()})"
        if drawn < 0.3:
            return f"{name}(k, v) AS (SELECT {rng.choice([0, 1])}, {self.mark()} {rng.choice(['UNION', 'UNION ALL'])} SELECT k + 1, v FROM {name} WHERE k < {rng.choice([2, 3])})"
        if drawn < 0.36:
            return f"{name}(k, v) AS (SELECT k, {self.mark()} FROM {one} WHERE k < 3 UNION SELECT r.k + 1, r.v FROM {name} AS r JOIN {two} AS s ON s.k = r.k WHERE r.k < 4)"
        if drawn < 0.4:
            return f"{name}(k, v) AS (SELECT 1, {self.mark()} UNION SELECT k + 1, v FROM {name} WHERE k < 4 AND k IN (SELECT k FROM {two}))"
        if drawn < 0.47:
            return f"{name} AS {hint}(SELECT k, v FROM {one} {rng.choice(['UNION', 'UNION ALL', 'EXCEPT', 'INTERSECT'])} SELECT k, {rng.choice(['v', self.mark()])} FROM {two})"
        if drawn < 0.53:
            return f"{name}(k, v) AS {hint}(SELECT k, count(*) + {self.mark()} FROM {one} GROUP BY k)"
        if drawn < 0.58:
            return f"{name} AS {hint}(SELECT k, v FROM {one} ORDER BY k, v LIMIT {rng.choice([1, 2, 3])})"
        if drawn < 0.68:
            return f"{name} AS {hint}(SELECT x.k, y.v FROM {one} AS x {rng.choice(['JOIN', 'LEFT JOIN'])} {one if rng.random() < 0.6 else two} AS y ON x.k = y.k)"
        if drawn < 0.76:
            kind = rng.choice(["own", "table", "column", "count", "twice"])
            if self.take_fault(kind):
                return {
                    "own": f"{name} AS (SELECT k, v FROM {name})",
                    "table": f"{name} AS (SELECT k, v FROM nosuch)",
                    "column": f"{name} AS (SELECT nosuch AS k, v FROM t)",
                    "count": f"{name}(k) AS (SELECT k, v FROM {one})",
                    "twice": f"{name}(k, v) AS (SELECT 1, 1 UNION ALL SELECT x.k, y.v FROM {name} AS x JOIN {name} AS y ON x.k = y.k)",
                }[kind]
        self.body_of = name
        return f"{name} AS {hint}({self.select(ctes, scope, depth - 1, 'pair')})"

    def with_clause(self, ctes, scope, depth):
        """A WITH clause and the names its query reads: its own, and those around it that it doesn't hide."""
        rng = self.rng
        names = rng.sample(NAMES, rng.choice([1, 1, 2, 2, 3]))
        around = [name for name in ctes if name not in names]
        parts = []
        for at, name in enumerate(names):
            later = names[at + 1:] and rng.random() < 0.25 and self.take_fault("later")
            reads = around + names[:at] + (names[at + 1:] if later else [])
            self.hidden.append(set(names) - set(reads))
            parts.append(self.cte(name, reads, scope, depth))
            self.hidden.pop()
        if rng.random() < 0.35:
            # One that nothing reads, most often, whose body would be an error.
            parts.append(f"u AS (SELECT k, v FROM {'bad' if self.bad else rng.choice(['nosuch', 'u', 'c', 'd'])})")
            if rng.random() < 0.1 and self.take_fault("unread"):
                names = names + ["u"]
        rng.shuffle(parts)
        recursive = "RECURSIVE " if rng.random() < 0.3 else ""
        return f"WITH {recursive}{', '.join(parts)} ", around + names

    def item(self, ctes, scope, depth, taken):
        """One source of a FROM clause, and the name its columns are read by."""
        rng = self.rng
        drawn = rng.random()
        source = self.source(ctes)
        if drawn < 0.35 and source not in taken and source not in scope:
            return source, source
        name = self.alias()
        if drawn < 0.75 or depth <= 0:
            return f"{source} AS {name}", name
        if drawn < 0.85:
            return f"(SELECT k, v FROM {source} WHERE {rng.choice(['k > 1', 'v IS NOT NULL', '1'])}) AS {name}", name
        return f"({self.select(ctes, [], depth - 1, 'pair')}) AS {name}", name

    def select(self, ctes, scope, depth, use, may_define=True):
        rng = self.rng
        before = ""
        body_of, self.body_of = self.body_of, None
        if may_define and rng.random() < 0.4 and depth >= 0:
            before, ctes = self.with_clause(ctes, scope, depth)
        text, name = self.item(ctes, scope, depth, [])
        names = [name]
        inner = scope + [name]
        if rng.random() < 0.35:
            other, name = self.item(ctes, scope, depth, names)
            names.append(name)
            inner = scope + names
            on = f"{names[0]}.k = {names[1]}.k"
            if depth > 0 and rng.random() < 0.3:
                on = f"{on} AND {self.subquery(ctes, inner, depth - 1)}" if rng.random() < 0.5 else self.subquery(ctes, inner, depth - 1)
            text += f" {rng.choice(['JOIN', 'JOIN', 'LEFT JOIN', 'RIGHT JOIN', 'FULL JOIN'])} {other} ON {on}"
        terms = []
        if rng.random() < 0.45:
            terms.append(self.plain(inner))
        if depth > 0 and rng.random() < 0.55:
            terms.append(self.subquery(ctes, inner, depth - 1))
        where = f" WHERE {' AND '.join(terms)}" if terms else ""
        first = names[0]
        if use == "exists":
            return f"{before}SELECT 1 FROM {text}{where}"
        if use == "scalar":
            result = rng.choice(["count(*)", f"max({first}.v)", f"sum({first}.k)", f"min({names[-1]}.v)"])
            return f"{before}SELECT {result} FROM {text}{where}"
        if use == "list":
            return f"{before}SELECT {rng.choice(names)}.k FROM {text}{where}"
        value = f"{names[-1]}.v"
        if depth > 0 and rng.random() < 0.25:
            value = f"({self.select(ctes, inner, depth - 1, 'scalar')})"
        sql = f"{before}SELECT {first}.k AS k, {value} AS v FROM {text}{where}"
        if rng.random() < 0.1:
            # A UNION's arm that names the CTE whose body it is reads the CTE in SQLite, its own WITH clause's CTE of the name
            # or not: a finding of its own (the lane's record).
            source = self.source([name for name in ctes if name != body_of])
            if source != body_of:
                sql += f" {rng.choice(['UNION', 'UNION ALL', 'EXCEPT'])} SELECT k, v FROM {source}"
        return sql

    def subquery(self, ctes, scope, depth):
        rng = self.rng
        drawn = rng.random()
        if drawn < 0.3:
            return f"{self.value(scope)} IN ({self.select(ctes, scope, depth, 'list')})"
        if drawn < 0.38:
            return f"{self.value(scope)} NOT IN ({self.select(ctes, scope, depth, 'list')})"
        if drawn < 0.56:
            return f"EXISTS ({self.select(ctes, scope, depth, 'exists')})"
        if drawn < 0.64:
            return f"NOT EXISTS ({self.select(ctes, scope, depth, 'exists')})"
        if drawn < 0.7:
            return f"({self.select(ctes, scope, depth, 'scalar')}) IN ({self.select(ctes, scope, depth, 'list')})"
        return f"({self.select(ctes, scope, depth, 'scalar')}) {rng.choice(['=', '<', '>=', 'IS NOT'])} {self.value(scope)}"

    def query(self, ctes, depth, may_define):
        """A statement's own query over `ctes`, its clauses reading them."""
        rng = self.rng
        drawn = rng.random()
        if drawn < 0.6:
            return self.select(ctes, [], depth, "pair", may_define)
        source = self.source(ctes)
        name = source if rng.random() < 0.5 else self.alias()
        text = source if name == source else f"{source} AS {name}"
        scalar = f"({self.select(ctes, [name], depth - 1, 'scalar')})"
        if drawn < 0.68:
            # Its subquery reads nothing of the grouped row: that a FROM clause inside it loses that row is a finding of its own,
            # which holds no CTE (the lane's record).
            return f"SELECT {name}.k, count(*) FROM {text} GROUP BY {name}.k HAVING {name}.k IN ({self.select(ctes, [], depth - 1, 'list')})"
        if drawn < 0.74:
            return f"SELECT {name}.k, {name}.v FROM {text} ORDER BY {scalar}, {name}.k, {name}.v"
        if drawn < 0.8:
            return f"SELECT {name}.k, {name}.v FROM {text} ORDER BY {name}.k, {name}.v LIMIT (SELECT count(*) FROM {self.source(ctes)})"
        if drawn < 0.86:
            return f"SELECT {name}.k, sum({name}.v) OVER (ORDER BY {name}.k, {name}.v), {scalar} FROM {text}"
        if drawn < 0.92:
            return f"SELECT j.value, {name}.k FROM json_each((SELECT json_group_array(k) FROM {self.source(ctes)})) AS j LEFT JOIN {text} ON {name}.k = j.value AND {self.subquery(ctes, [name], depth - 1)}"
        return f"SELECT k, v FROM {self.source(ctes)} {rng.choice(['UNION', 'UNION ALL', 'EXCEPT', 'INTERSECT'])} {self.compound_arm(ctes, depth)}"

    def compound_arm(self, ctes, depth):
        rng = self.rng
        name = self.alias()
        return f"SELECT {name}.k, {name}.v FROM {self.source(ctes)} AS {name} WHERE {self.subquery(ctes, [name], depth - 1)}"

    def top(self, depth):
        before, ctes = "", []
        if self.rng.random() < 0.85:
            before, ctes = self.with_clause([], [], depth)
            if self.bad:
                head, rest = before.split(" ", 1) if not before.startswith("WITH RECURSIVE ") else ("WITH RECURSIVE", before[len("WITH RECURSIVE "):])
                before = f"{head} bad AS (SELECT k, v FROM nosuch), {rest}"
        return before, ctes

    def read(self):
        self.start()
        rng = self.rng
        self.bad = rng.random() < 0.3
        depth = rng.choice([1, 1, 2, 2, 3])
        before, ctes = self.top(depth)
        return before + self.query(ctes, depth, not before)

    def write(self):
        self.start()
        rng = self.rng
        self.bad = rng.random() < 0.3
        depth = rng.choice([1, 1, 2])
        before, ctes = self.top(depth)
        drawn = rng.random()
        if drawn < 0.3:
            return f"{before}INSERT INTO log {self.select(ctes, [], depth, 'pair')}"
        if drawn < 0.4:
            return f"{before}INSERT INTO log VALUES (({self.select(ctes, [], depth, 'scalar')}), {self.mark()})"
        if drawn < 0.6:
            return f"{before}UPDATE log SET v = ({self.select(ctes, ['log'], depth, 'scalar')}) WHERE {self.subquery(ctes, ['log'], depth)}"
        if drawn < 0.75:
            name = self.alias()
            return f"{before}UPDATE log SET v = {name}.k FROM (SELECT DISTINCT k FROM {self.source(ctes)}) AS {name} WHERE {name}.k = log.k AND {self.subquery(ctes, ['log'], depth)}"
        return f"{before}DELETE FROM log WHERE {self.subquery(ctes, ['log'], depth)}"


def written(value):
    return "NULL" if value is None else str(value)


def lines(rows):
    out = sorted(",".join(written(value) for value in row) for row in rows)
    return f"{len(out)}:" + ";".join(out)


def ask(conn, sql, write):
    """SQLite's answer to `sql`; None where it takes more than 400,000 steps of its machine."""
    conn.set_progress_handler(lambda: 1, 400_000)
    try:
        if not write:
            return lines(conn.execute(sql).fetchall())
        conn.execute("BEGIN")
        try:
            conn.execute(sql)
            return lines(conn.execute("SELECT k, v FROM log").fetchall())
        finally:
            conn.execute("ROLLBACK")
    except sqlite3.OperationalError as error:
        return None if "interrupted" in str(error) else f"error: {error}"
    except sqlite3.Error as error:
        return f"error: {error}"
    finally:
        conn.set_progress_handler(None, 0)


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 300
    seed = sys.argv[2] if len(sys.argv) > 2 else "C18"
    print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_cte_scopes.py {count} {seed}")
    for name, statements, write in [
        ("reads plain", TABLES, False),
        ("reads indexed", TABLES + INDEXES, False),
        ("writes", TABLES + INDEXES[:1], True),
    ]:
        conn = sqlite3.connect(":memory:", isolation_level=None)
        print(f"db\t{name}")
        for statement in statements:
            conn.execute(statement)
            print(f"sql\t{statement}")
        draw = Draw(random.Random(f"{seed} {name}"))
        seen, made, refused, found = set(), 0, 0, [] if write else list(FOUND)
        while made < count:
            sql = found.pop(0) if found else draw.write() if write else draw.read()
            if sql in seen or len(sql) > 900:
                continue
            seen.add(sql)
            answer = ask(conn, sql, write)
            if answer is None or len(answer) > 900 or "\n" in answer:
                continue
            # At most one statement in five that SQLite refuses, each for the fault it was drawn with alone.
            if answer.startswith("error:"):
                expected = {
                    "own": ["circular reference"],
                    "later": ["circular reference"],
                    "twice": ["multiple references to recursive table"],
                    "table": ["no such table"],
                    "unknown": [f"no such table: {getattr(draw, 'unknown', None)}"],
                    "unread": ["no such table: nosuch", "circular reference: u", "no such table: c", "no such table: d"],
                    "column": ["no such column"],
                    "count": ["table "],
                }.get(draw.held, [])
                asked = sql in FOUND or any(answer.startswith(f"error: {text}") for text in expected)
                if not asked or (refused + 1) * 5 > count:
                    continue
                refused += 1
            print(f"{'w' if write else 'q'}\t{sql}\t{answer}")
            made += 1


main()
