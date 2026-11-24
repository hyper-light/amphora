"""Oracle findings CO and CM (lane C13): joins of every kind whose ON clause holds subqueries that read CTEs
(WITH, a WITH of the subquery's own, recursive, one CTE over another, materialized or not, in a
compound, a derived table, a parenthesized join or a table function's argument), and joins of every
kind with a WITHOUT ROWID table, with and without secondary indexes, on either side. Writes each query
with the answer SQLite itself gives (the sqlite3 library Python links, printed first) to
crates/engine/tests/fixtures/on_ctes.tsv, which tests/outer_join_subqueries.rs holds Amphora to, in
the form bench/sqlite_outer_join_subqueries.py describes.
usage: python3 bench/sqlite_on_ctes.py [queries per database [seed]] > crates/engine/tests/fixtures/on_ctes.tsv"""
import random
import sqlite3
import sys

TABLES = [
    "CREATE TABLE p(id INTEGER PRIMARY KEY, k INT, v INT, s TEXT)",
    "CREATE TABLE q(k INT, v INT, s TEXT COLLATE NOCASE)",
    "CREATE TABLE e(k INT, v INT, s TEXT)",
    "CREATE TABLE w(k INT PRIMARY KEY, v INT, s TEXT) WITHOUT ROWID",
    "CREATE TABLE u(v INT, s TEXT COLLATE NOCASE, k INT, PRIMARY KEY (s, k DESC)) WITHOUT ROWID",
    "CREATE TABLE z(k INT PRIMARY KEY, v INT, s TEXT) WITHOUT ROWID",
    "INSERT INTO p VALUES (1, 1, 2, '1'), (2, 2, 3, 'b'), (3, NULL, 1, 'B'), (4, 4, NULL, NULL), (5, 1, 0, '2')",
    "INSERT INTO q VALUES (2, 2, 'B'), (2, 0, 'a'), (5, 1, '1'), (NULL, 4, NULL), (1, NULL, 'A')",
    "INSERT INTO w VALUES (1, 1, 'a'), (2, 0, '2'), (3, 1, 'B'), (6, NULL, NULL), (7, 2, 'a')",
    "INSERT INTO u VALUES (1, 'a', 1), (2, 'a', 2), (1, 'B', 2), (NULL, 'b', 4), (0, '1', 6), (1, 'c', 3)",
]
INDEXES = [
    "CREATE INDEX p_k ON p(k)",
    "CREATE INDEX q_kv ON q(k, v)",
    "CREATE INDEX w_v ON w(v)",
    "CREATE INDEX w_sv ON w(s, v DESC)",
    "CREATE INDEX u_v ON u(v)",
    "CREATE UNIQUE INDEX u_sv ON u(s COLLATE BINARY, v, k)",
    "CREATE INDEX z_v ON z(v)",
]
# Partial and expression indexes of a WITHOUT ROWID table, and one whose terms are its key's, the other way round.
MORE_INDEXES = [
    "CREATE INDEX w_part ON w(v) WHERE v > 0",
    "CREATE INDEX w_vk ON w(v, k DESC)",
    "CREATE INDEX u_ks ON u(k, s)",
    "CREATE INDEX u_part ON u(v DESC) WHERE v IS NOT NULL",
    "CREATE INDEX u_expr ON u(k + v)",
]
ROWID_TABLES = ["p", "q", "e"]
KEYED_TABLES = ["w", "w", "u", "u", "z"]
SOURCES = ROWID_TABLES + ["w", "u", "z"]
JOINS = ["LEFT JOIN", "LEFT JOIN", "RIGHT JOIN", "RIGHT JOIN", "FULL JOIN", "FULL JOIN", "FULL JOIN", "JOIN", "CROSS JOIN"]
# The reductions: each is asked first.
FOUND = [
    "WITH c AS (SELECT k, v FROM q) SELECT a.k, b.k FROM p AS a JOIN p AS b ON a.k IN (SELECT c.k FROM c)",
    "WITH c AS (SELECT k, v FROM q) SELECT a.k, b.k FROM p AS a FULL JOIN p AS b ON b.k IN (SELECT c.k FROM c WHERE c.v = a.v)",
    "WITH RECURSIVE c(x) AS (SELECT 2 UNION ALL SELECT x + 1 FROM c WHERE x < 4) SELECT a.k, b.k FROM p AS a LEFT JOIN p AS b ON b.k IN (SELECT x FROM c)",
    "WITH c AS (SELECT k FROM q) SELECT x.value FROM json_each((SELECT json_group_array(k) FROM c)) AS x",
    "SELECT count(*) FROM p AS a0 FULL JOIN w AS a2 ON a0.k = a2.v",
    "SELECT a0.k, a2.v FROM p AS a0 RIGHT JOIN w AS a2 ON a0.k = a2.v AND a0.v = 2",
    "SELECT a0.k, a2.v, a2.k FROM p AS a0 FULL JOIN w AS a2 ON a0.k = a2.v WHERE a2.v >= 0",
    "SELECT a0.k, a2.v FROM p AS a0 FULL JOIN u AS a2 ON a0.k = a2.v",
]


class Draw:
    def __init__(self, rng):
        self.rng = rng

    def start(self):
        self.made = 0
        # The CTEs in reach: their names; and the aliases whose `s` is a column of a table.
        self.ctes = []
        self.texts = set()

    def alias(self):
        self.made += 1
        return f"s{self.made}"

    def column(self, scope):
        rng = self.rng
        at = scope[-1] if rng.random() < 0.45 else rng.choice(scope)
        return f"{at}.s" if at in self.texts and rng.random() < 0.1 else f"{at}.{rng.choice('kkv')}"

    def value(self, scope):
        rng = self.rng
        drawn = rng.random()
        if drawn < 0.65:
            return self.column(scope)
        if drawn < 0.85:
            return str(rng.choice([0, 1, 2, 3]))
        if drawn < 0.88:
            return "NULL"
        return f"({self.column(scope)} {rng.choice(['+', '-'])} {rng.choice(['1', self.column(scope)])})"

    def plain(self, scope):
        rng = self.rng
        drawn = rng.random()
        if drawn < 0.6:
            return f"{self.column(scope)} {rng.choice(['=', '=', '=', '<', '>=', '<>', 'IS'])} {self.value(scope)}"
        if drawn < 0.7:
            return f"{self.column(scope)} IS {rng.choice(['', 'NOT '])}NULL"
        if drawn < 0.8:
            return f"{self.column(scope)} IN ({self.value(scope)}, {rng.choice([0, 1, 2])})"
        if drawn < 0.86:
            return f"{self.column(scope)} BETWEEN {rng.choice([0, 1])} AND {self.value(scope)}"
        if drawn < 0.95:
            return f"({self.plain(scope)} OR {self.plain(scope)})"
        return rng.choice(["1", "0", "NULL", self.column(scope)])

    def with_clause(self, names, inner=False):
        """A WITH clause that defines `names`; each may read those before it, and those in reach."""
        rng = self.rng
        parts, recursive = [], False
        for name in names:
            hint = rng.choice(["", "", "", "MATERIALIZED ", "NOT MATERIALIZED "])
            over = rng.choice(self.ctes) if self.ctes and rng.random() < 0.45 else rng.choice(SOURCES)
            drawn = rng.random()
            if drawn < 0.3:
                body = f"{name} AS {hint}(SELECT k, v, s FROM {over} WHERE {rng.choice(['v > 0', 'k IS NOT NULL', '1', 'v < 3'])})"
            elif drawn < 0.45:
                body = f"{name}(k, v, s) AS {hint}(SELECT k, count(*), max(v) FROM {over} GROUP BY k)"
            elif drawn < 0.6:
                recursive = True
                body = f"{name}(k, v, s) AS (SELECT {rng.choice([0, 1])}, 1, 2 {rng.choice(['UNION', 'UNION ALL'])} SELECT k + 1, v + k, s FROM {name} WHERE k < {rng.choice([2, 3])})"
            elif drawn < 0.7:
                recursive = True
                body = f"{name}(k, v, s) AS (SELECT k, v, 1 FROM {over} WHERE k = 1 UNION SELECT t.k, t.v, 1 FROM {name} AS r JOIN {rng.choice(SOURCES)} AS t ON t.k = r.k + 1)"
            elif drawn < 0.82:
                body = f"{name} AS {hint}(SELECT k, v, 0 AS s FROM {over} {rng.choice(['UNION', 'UNION ALL', 'EXCEPT'])} SELECT v, k, 0 FROM {rng.choice(SOURCES)})"
            elif drawn < 0.9 and self.ctes:
                on = self.on_subquery(["t1", "t2"], 1)
                body = f"{name} AS {hint}(SELECT t1.k, t2.v, 0 AS s FROM {rng.choice(SOURCES)} AS t1 {rng.choice(['LEFT JOIN', 'JOIN', 'FULL JOIN'])} {rng.choice(SOURCES)} AS t2 ON {on})"
            else:
                body = f"{name} AS {hint}(SELECT k, v, 0 AS s FROM {over} ORDER BY k, v LIMIT {rng.choice([1, 2, 4])})"
            parts.append(body)
            self.ctes.append(name)
        return f"WITH {'RECURSIVE ' if recursive else ''}{', '.join(parts)} "

    def source(self):
        """What a subquery in an ON clause reads: a CTE, most often."""
        rng = self.rng
        return rng.choice(self.ctes) if self.ctes and rng.random() < 0.85 else rng.choice(SOURCES)

    def select(self, scope, depth, use):
        rng = self.rng
        name = self.alias()
        before, kept = "", list(self.ctes)
        if rng.random() < 0.18:
            # A WITH of the subquery's own, which may take the name of one in reach.
            own = rng.choice(kept) if kept and rng.random() < 0.3 else f"d{self.made}"
            before = self.with_clause([own])
        inner = scope + [name]
        text = f"{self.source()} AS {name}"
        if rng.random() < 0.18:
            other = self.alias()
            inner = scope + [name, other]
            on = f"{name}.k = {other}.k" if depth == 0 or rng.random() < 0.5 else self.on_subquery(inner, depth - 1)
            text += f" {rng.choice(JOINS)} {self.source()} AS {other} ON {on}"
        terms = []
        if rng.random() < 0.6:
            terms.append(self.plain(inner))
        if depth > 0 and rng.random() < 0.45:
            terms.append(self.on_subquery(inner, depth - 1))
        self.ctes = kept
        where = f" WHERE {' AND '.join(terms)}" if terms else ""
        if use == "exists":
            return f"{before}SELECT 1 FROM {text}{where}"
        if use == "scalar":
            result = rng.choice([f"count({name}.k)", "count(*)", f"max({name}.v)", f"min({name}.k)", f"sum({name}.v)"])
            return f"{before}SELECT {result} FROM {text}{where}"
        result = rng.choice([f"{name}.k", f"{name}.k", f"{name}.v", f"({name}.k + 1)"])
        return f"{before}SELECT {result} FROM {text}{where}"

    def on_subquery(self, scope, depth):
        rng = self.rng
        drawn = rng.random()
        if drawn < 0.34:
            return f"{self.value(scope)} IN ({self.select(scope, depth, 'list')})"
        if drawn < 0.44:
            return f"{self.value(scope)} NOT IN ({self.select(scope, depth, 'list')})"
        if drawn < 0.6:
            return f"EXISTS ({self.select(scope, depth, 'exists')})"
        if drawn < 0.68:
            return f"NOT EXISTS ({self.select(scope, depth, 'exists')})"
        if drawn < 0.84:
            return f"({self.select(scope, depth, 'scalar')}) {rng.choice(['=', '<', '>=', 'IS'])} {self.value(scope)}"
        if drawn < 0.92:
            return f"({self.on_subquery(scope, depth)} {rng.choice(['OR', 'AND'])} {self.plain(scope)})"
        return f"NOT ({self.on_subquery(scope, depth)})"

    def joined(self, alias, tables):
        """A source of the FROM clause, and the text of it."""
        rng = self.rng
        drawn = rng.random()
        if self.ctes and drawn < 0.18:
            return f"{rng.choice(self.ctes)} AS {alias}"
        if self.ctes and drawn < 0.26:
            return f"(SELECT k, v, s FROM {rng.choice(self.ctes)} WHERE {rng.choice(['v > 0', 'k IS NOT NULL'])}) AS {alias}"
        self.texts.add(alias)
        return f"{rng.choice(tables)} AS {alias}"

    def from_clause(self, aliases, depth):
        """The FROM clause over `aliases`, each join's ON clause holding a subquery most often."""
        rng = self.rng
        text = self.joined(aliases[0], SOURCES)
        for n in range(1, len(aliases)):
            join = rng.choice(JOINS)
            scope = aliases[:n + 1]
            on = self.on_subquery(scope, depth)
            drawn = rng.random()
            if drawn < 0.3:
                on = f"{aliases[rng.randrange(n)]}.k = {aliases[n]}.{rng.choice('kv')} AND {on}"
            elif drawn < 0.36:
                on = f"({aliases[rng.randrange(n)]}.k = {aliases[n]}.k OR {on})"
            elif drawn < 0.5:
                on = self.plain(scope)
            text += f" {join} {self.joined(aliases[n], SOURCES)} ON {on}"
        return text

    def cte_query(self):
        rng = self.rng
        self.start()
        names = ["c", "d", "f"][:rng.choice([1, 1, 2, 2, 3])]
        before = self.with_clause(names)
        depth = rng.choice([0, 1, 1, 2])
        aliases = [f"a{n}" for n in range(rng.choice([2, 2, 2, 3]))]
        drawn = rng.random()
        if drawn < 0.08:
            # A parenthesized join, whose ON reads the CTEs, joined to one more table.
            inner, outer = aliases[:2], ["a0", "a1", "a9"]
            self.texts.add("a9")
            text = (f"{rng.choice(SOURCES)} AS a9 {rng.choice(['LEFT JOIN', 'JOIN', 'RIGHT JOIN', 'FULL JOIN'])} "
                    f"({self.from_clause(inner, depth)}) ON {self.on_subquery(outer, depth)}")
            aliases = outer
        elif drawn < 0.14:
            # A table function's argument reads a CTE.
            text = (f"json_each((SELECT json_group_array(k) FROM {rng.choice(self.ctes)})) AS j {rng.choice(['LEFT JOIN', 'JOIN'])} "
                    f"{rng.choice(SOURCES)} AS a0 ON a0.k = j.key + 1 AND {self.on_subquery(['a0'], depth)}")
            aliases = ["a0"]
        else:
            text = self.from_clause(aliases, depth)
        columns = [f"{a}.{c}" for a in aliases for c in ("k", "v")]
        select = ", ".join(columns)
        if rng.random() < 0.12:
            select = "count(*), " + ", ".join(f"count({c}), sum({c})" for c in columns[:4])
        where = f" WHERE {self.plain(aliases)}" if rng.random() < 0.35 else ""
        sql = f"SELECT {select} FROM {text}{where}"
        drawn = rng.random()
        if drawn < 0.06:
            other = [f"a{n}" for n in range(2)]
            arm = ", ".join(f"{other[n % 2]}.k" for n in range(select.count(',') + 1))
            self.texts = set()
            sql += f" {rng.choice(['UNION ALL', 'UNION', 'EXCEPT'])} SELECT {arm} FROM {self.from_clause(other, depth)}"
        elif drawn < 0.12:
            return f"{before}SELECT * FROM ({sql}) WHERE 1"
        elif drawn < 0.18:
            self.texts = set()
            return f"SELECT t.k, t.v FROM {rng.choice(SOURCES)} AS t WHERE EXISTS ({before}{sql})"
        return before + sql

    def keyed_query(self):
        """A join of any kind with a WITHOUT ROWID table right or left of it."""
        rng = self.rng
        self.start()
        aliases = [f"a{n}" for n in range(rng.choice([2, 2, 2, 3]))]
        keyed = rng.randrange(len(aliases))
        self.texts.update(aliases)
        text = ""
        for n, alias in enumerate(aliases):
            table = rng.choice(KEYED_TABLES) if n == keyed or rng.random() < 0.35 else rng.choice(ROWID_TABLES)
            source = f"{table} AS {alias}"
            if n == 0:
                text = source
                continue
            scope = aliases[:n + 1]
            drawn = rng.random()
            if drawn < 0.06:
                on = rng.choice(["1", "0", "NULL"])
            elif drawn < 0.75:
                on = f"{aliases[rng.randrange(n)]}.{rng.choice('kvv')} = {alias}.{rng.choice('kvvs')}"
                if rng.random() < 0.4:
                    on += f" AND {self.plain(scope)}"
            else:
                on = self.plain(scope)
            text += f" {rng.choice(JOINS[:8])} {source} ON {on}"
        drawn = rng.random()
        at = aliases[keyed]
        if drawn < 0.15:
            select = "count(*)"
        elif drawn < 0.35:
            select = f"{at}.v"
        elif drawn < 0.5:
            select = f"{at}.v, {at}.k"
        elif drawn < 0.6:
            select = ", ".join(f"{a}.v" for a in aliases)
        elif drawn < 0.66:
            select = f"{at}.v, count(*)"
        elif drawn < 0.72:
            select = f"DISTINCT {at}.v"
        else:
            select = ", ".join(f"{a}.{c}" for a in aliases for c in ("k", "v", "s"))
        sql = f"SELECT {select} FROM {text}"
        terms = []
        if rng.random() < 0.5:
            terms.append(rng.choice([
                f"{at}.v {rng.choice(['=', '>', '>=', '<', 'IS'])} {rng.choice([0, 1, 2])}",
                f"{at}.v IN ({rng.choice([0, 1])}, 2)",
                f"{at}.v = {rng.choice([1, 2])} OR {at}.k = {rng.choice([2, 3, 6])}",
                f"({at}.v = 1 OR {at}.s = 'a')",
                f"{at}.k {rng.choice(['=', '>', '<'])} {rng.choice([1, 2, 3])}",
                f"{at}.s = {rng.choice(['1', chr(39) + 'a' + chr(39), chr(39) + 'B' + chr(39)])}",
                f"{at}.k + {at}.v = {rng.choice([2, 4])}",
                f"{at}.v IN (SELECT k FROM {rng.choice(SOURCES)})",
                f"{at}.v IS NOT NULL",
            ]))
            if " OR " in terms[0] and not terms[0].startswith("("):
                terms[0] = f"({terms[0]})"
        if rng.random() < 0.25:
            terms.append(self.plain(aliases))
        if terms:
            sql += f" WHERE {' AND '.join(terms)}"
        if select.endswith("count(*)") and "," in select:
            sql += f" GROUP BY {at}.v"
        return sql


def written(value):
    return "NULL" if value is None else str(value)


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 300
    seed = sys.argv[2] if len(sys.argv) > 2 else "CO"
    print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_on_ctes.py {count} {seed}")
    databases = [
        ("ctes plain", TABLES, "cte"),
        ("ctes indexed", TABLES + INDEXES, "cte"),
        ("keyed plain", TABLES, "keyed"),
        ("keyed indexed", TABLES + INDEXES, "keyed"),
        ("keyed more", TABLES + MORE_INDEXES, "keyed"),
    ]
    for name, statements, kind in databases:
        conn = sqlite3.connect(":memory:")
        print(f"db\t{name}")
        for statement in statements:
            conn.execute(statement)
            print(f"sql\t{statement}")
        draw = Draw(random.Random(f"{seed} {name}"))
        seen, made, found = set(), 0, list(FOUND)
        while made < count:
            sql = found.pop(0) if found else draw.cte_query() if kind == "cte" else draw.keyed_query()
            if sql in seen or len(sql) > 1400:
                continue
            seen.add(sql)
            try:
                rows = conn.execute(sql).fetchall()
            except sqlite3.Error as error:
                print(f"# SQLite refuses {sql}: {error}", file=sys.stderr)
                continue
            lines = sorted(",".join(written(value) for value in row) for row in rows)
            answer = f"{len(lines)}:" + ";".join(lines)
            if len(answer) > 4000:
                continue
            print(f"q\t{sql}\t{answer}")
            made += 1


main()
