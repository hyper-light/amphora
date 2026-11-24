"""Oracle finding CN (lane C12): joins of two to four tables, inner, LEFT, RIGHT and FULL, whose WHERE
(and ON, select list, GROUP BY and ORDER BY) reads scalar, EXISTS, IN and NOT IN subqueries, correlated
with any of the joined tables, with a query around the join, or with none, beside columns of the other
joined tables. Writes each query with the answer SQLite itself gives (the sqlite3 library Python links,
printed first) to crates/engine/tests/fixtures/join_where_subqueries.tsv, which
tests/outer_join_subqueries.rs holds Amphora to under every join method.

The lines are `sqlite_outer_join_subqueries.py`'s: `db <name>`, `sql <statement>`, `q <query> <answer>`.
usage: python3 bench/sqlite_join_where_subqueries.py [queries per database [seed]] > crates/engine/tests/fixtures/join_where_subqueries.tsv"""
import random
import sqlite3
import sys

TABLES = [
    "CREATE TABLE p(id INTEGER PRIMARY KEY, k INT, v INT, s TEXT)",
    "CREATE TABLE q(id INTEGER PRIMARY KEY, k INT, v INT, s TEXT COLLATE NOCASE)",
    "CREATE TABLE n(k INT, v INT, s TEXT)",
    "CREATE TABLE f(k INT, v INT, s TEXT)",
    "CREATE TABLE g(k INT, v INT, s TEXT)",
    "CREATE TABLE e(k INT, v INT, s TEXT)",
    "INSERT INTO p VALUES (1, 1, 2, '1'), (2, 2, 3, 'b'), (3, NULL, 1, 'B'), (4, 4, NULL, NULL), (5, 1, 0, '2')",
    "INSERT INTO q VALUES (1, 2, 2, 'B'), (2, 2, 0, 'a'), (3, 5, 1, '1'), (4, NULL, 4, NULL), (5, 1, 1, 'c')",
    "INSERT INTO n VALUES (1, 2, '2'), (NULL, 1, 'b'), (2, 2, NULL), (3, 0, 'A'), (1, NULL, '1')",
    "INSERT INTO f VALUES (1, 2, '2'), (2, 1, 'b'), (2, 5, NULL)",
    "INSERT INTO g VALUES (0, 1, '0'), (1, 1, 'a'), (2, 3, 'b'), (2, 0, 'B'), (3, 4, NULL), (4, 2, '4'), (5, NULL, 'c'), (NULL, 2, '2')",
]
INDEXES = [
    "CREATE INDEX p_k ON p(k)",
    "CREATE INDEX q_kv ON q(k, v)",
    "CREATE INDEX n_v ON n(v)",
    "CREATE UNIQUE INDEX f_kv ON f(k, v)",
    "CREATE INDEX g_k ON g(k)",
    "CREATE INDEX e_k ON e(k)",
]
SOURCES = ["p", "q", "n", "f", "g", "e"]
# Finding CN's own query, the smallest forms of what Amphora failed on, and what wider draws found.
FOUND = [
    "SELECT a0.k, a1.k, a2.k FROM p AS a0 JOIN q AS a1 ON a0.k = a1.k LEFT JOIN e AS a2 ON a1.k = a2.v WHERE (SELECT min(s1.k) FROM p AS s1 WHERE a0.k < NULL) IS NOT (a2.k - a2.k)",
    "SELECT a0.k, a1.k, a2.k FROM p AS a0 JOIN q AS a1 ON a0.k = a1.k LEFT JOIN e AS a2 ON a1.k = a2.v WHERE a2.k IN (SELECT s1.k FROM n AS s1 WHERE s1.v = a0.k)",
    "SELECT a0.k, a1.k, a2.k FROM p AS a0 JOIN q AS a1 ON a0.k = a1.k LEFT JOIN e AS a2 ON a1.k = a2.v WHERE a2.k NOT IN (SELECT s1.k FROM n AS s1 WHERE s1.v = a0.k)",
    "SELECT a0.k, a1.k, a2.k FROM p AS a0 JOIN q AS a1 ON a0.k = a1.k LEFT JOIN e AS a2 ON a1.k = a2.v WHERE a2.k IS NULL AND (SELECT count(*) FROM n AS s1 WHERE s1.k = a0.k) = 2",
    "SELECT a0.k, a1.k, a2.k FROM p AS a0 JOIN q AS a1 ON a0.k = a1.k JOIN f AS a2 ON a1.k = a2.v WHERE (SELECT a0.k) IS a2.v",
    "SELECT o.k, (SELECT count(*) FROM q AS a0 LEFT JOIN e AS a1 ON a0.k = a1.v WHERE (SELECT o.k) IS NOT a1.k) FROM p AS o",
    "SELECT o.k, (SELECT count(*) FROM q AS a0 LEFT JOIN f AS a1 ON a0.k = a1.v WHERE (SELECT o.k) IS NOT a1.k) FROM p AS o",
    "SELECT o.k FROM p AS o WHERE EXISTS (SELECT 1 FROM q AS a0 LEFT JOIN e AS a1 ON a0.k = a1.v WHERE (SELECT o.k) = a1.k)",
    "SELECT a0.k, a1.k, a2.k FROM q AS a0 JOIN p AS a1 ON a0.k = a1.k LEFT JOIN e AS a2 ON a1.k = a2.k WHERE CASE WHEN NOT EXISTS (SELECT 1 FROM g AS s3 JOIN f AS s4 ON s3.k = s4.v WHERE s3.v IS NOT NULL) THEN a2.v IS a2.k ELSE a0.v < (a0.v - a0.k) END",
    "SELECT a0.k, a1.k, a2.k, a3.k, (SELECT sum(s3.v) FROM q AS s3 LEFT JOIN n AS s4 ON s3.k = s4.k WHERE s3.k IN ((s4.k + s4.v), 2)) FROM n AS a0 JOIN q AS a1 ON a0.v = a1.k AND a0.k IS a0.k JOIN f AS a2 ON a1.k = a2.v LEFT JOIN n AS a3 ON a2.v = a3.k",
    "SELECT a1.k, a3.v FROM n AS a0 JOIN p AS a1 ON a0.k = a1.k LEFT JOIN q AS a2 ON a1.k = a2.k LEFT JOIN g AS a3 ON a2.k = a3.v WHERE coalesce((SELECT count(s1.k) FROM n AS s1 WHERE s1.k IS a3.v), a1.k) = a0.k",
    "SELECT a0.k, count(*), sum(a3.v) FROM q AS a0 JOIN p AS a1 ON a0.k = a1.k JOIN e AS a2 ON a0.v = a2.k LEFT JOIN n AS a3 ON a2.k = a3.v WHERE (EXISTS (SELECT 1 FROM n AS s1 WHERE s1.v < a3.v) AND a1.k <> a0.k) GROUP BY a0.k",
]


class Draw:
    def __init__(self, rng):
        self.rng = rng
        self.made = 0
        self.negated = 0

    def alias(self):
        self.made += 1
        return f"s{self.made}"

    def column(self, scope):
        rng = self.rng
        return f"{rng.choice(scope)}.{'s' if rng.random() < 0.05 else rng.choice('kkv')}"

    def value(self, scope):
        rng = self.rng
        drawn = rng.random()
        if drawn < 0.62:
            return self.column(scope)
        if drawn < 0.76:
            return str(rng.choice([0, 1, 2, 3]))
        if drawn < 0.8:
            return "NULL"
        return f"({self.column(scope)} {rng.choice(['+', '-'])} {self.column(scope)})"

    def plain(self, scope):
        rng = self.rng
        drawn = rng.random()
        if drawn < 0.5:
            return f"{self.column(scope)} {rng.choice(['=', '=', '<', '<=', '<>', 'IS', 'IS NOT'])} {self.value(scope)}"
        if drawn < 0.72:
            return f"{self.column(scope)} IS {rng.choice(['', 'NOT '])}NULL"
        if drawn < 0.8:
            return f"{self.column(scope)} IN ({self.value(scope)}, 2)"
        if drawn < 0.9:
            return f"({self.plain(scope)} OR {self.plain(scope)})"
        return f"({self.column(scope)} + {self.column(scope)}) IS {rng.choice(['', 'NOT '])}NULL"

    def some(self, scope, least, most):
        """Some of the scope's aliases, in their order."""
        count = min(len(scope), self.rng.randint(least, most))
        chosen = set(self.rng.sample(scope, count))
        return [alias for alias in scope if alias in chosen]

    def term(self, scope, depth):
        """A WHERE term: a subquery that reads some of the scope beside columns of some of it."""
        rng = self.rng
        outer = self.some(scope, 0, 2)
        beside = self.some(scope, 1, 2)
        drawn = rng.random()
        if drawn < 0.16:
            return f"{self.value(beside)} IN ({self.select(outer, depth, 'list')})"
        if drawn < 0.32:
            return f"{self.value(beside)} NOT IN ({self.select(outer, depth, 'list')})"
        if drawn < 0.5:
            op = rng.choice(["=", "<", ">=", "<>", "IS", "IS NOT"])
            return f"({self.select(outer, depth, 'scalar')}) {op} {self.value(beside)}"
        if drawn < 0.58:
            return f"(({self.select(outer, depth, 'scalar')}) + {self.column(beside)}) IS {rng.choice(['', 'NOT '])}NULL"
        if drawn < 0.64:
            return f"coalesce(({self.select(outer, depth, 'scalar')}), {self.column(beside)}) {rng.choice(['=', '<', 'IS NOT'])} {self.value(beside)}"
        if drawn < 0.72:
            return f"{rng.choice(['', 'NOT '])}EXISTS ({self.select(outer, depth, 'exists')})"
        if drawn < 0.84:
            return f"({rng.choice(['', 'NOT '])}EXISTS ({self.select(outer, depth, 'exists')}) {rng.choice(['OR', 'AND'])} {self.plain(beside)})"
        if drawn < 0.9:
            return f"CASE WHEN {rng.choice(['', 'NOT '])}EXISTS ({self.select(outer, depth, 'exists')}) THEN {self.plain(beside)} ELSE {self.plain(beside)} END"
        if drawn < 0.95 and not self.negated:
            # A row value's NOT IN, and a NOT over its IN, answer otherwise than SQLite where a value is
            # NULL, with a join or without (this lane's record, not traced), so neither is drawn.
            return f"({self.column(beside)}, {self.column(beside)}) IN ({self.select(outer, depth, 'pairs')})"
        self.negated += 1
        term = self.term(scope, depth)
        self.negated -= 1
        return f"NOT ({term})"

    def select(self, outer, depth, use):
        rng = self.rng
        name = self.alias()
        inner = outer + [name]
        source = f"{rng.choice(SOURCES)} AS {name}"
        if rng.random() < 0.12:
            other = self.alias()
            inner = outer + [name, other]
            source += f" {rng.choice(['JOIN', 'LEFT JOIN'])} {rng.choice(SOURCES)} AS {other} ON {name}.k = {other}.{rng.choice('kv')}"
        where = []
        for alias in outer:
            where.append(f"{name}.{rng.choice('kv')} {rng.choice(['=', '=', '=', '<', '<>', 'IS'])} {alias}.{rng.choice('kkv')}")
        if depth > 1 and rng.random() < 0.3:
            where.append(self.term(inner, depth - 1))
        if not where or rng.random() < 0.3:
            where.append(self.plain(inner))
        where = " AND ".join(where)
        if use == "exists":
            return f"SELECT 1 FROM {source} WHERE {where}"
        if use == "pairs":
            return f"SELECT {name}.k, {name}.v FROM {source} WHERE {where}"
        if use == "scalar":
            if outer and rng.random() < 0.12:
                return f"SELECT {rng.choice(outer)}.{rng.choice('kv')}"
            result = rng.choice([f"count({name}.k)", "count(*)", f"max({name}.v)", f"min({name}.k)", f"sum({name}.v)", f"{name}.v"])
            order = " ORDER BY 1 LIMIT 1" if result == f"{name}.v" else ""
            return f"SELECT {result} FROM {source} WHERE {where}{order}"
        return f"SELECT {rng.choice([f'{name}.k', f'{name}.v', f'({name}.k + {name}.v)'])} FROM {source} WHERE {where}"

    def joined(self, around):
        """A join, its aliases and its WHERE. `around` is the alias of the query around it, if any."""
        rng = self.rng
        count = rng.choice([2, 2, 3, 3, 3, 4])
        aliases = [f"a{n}" for n in range(count)]
        tables = rng.sample(SOURCES, count)
        if rng.random() < 0.1:
            tables[-1] = tables[0]
        text = f"{tables[0]} AS a0"
        rightmost = rng.random() < 0.14
        for n in range(1, count):
            last = n == count - 1
            joins = ["JOIN", "LEFT JOIN", "LEFT JOIN"] if last else ["JOIN", "JOIN", "LEFT JOIN"]
            if rightmost and last:
                joins = ["RIGHT JOIN", "FULL JOIN"]
            on = f"a{rng.randrange(n) if rng.random() < 0.4 else n - 1}.{rng.choice('kkkv')} = a{n}.{rng.choice('kkv')}"
            drawn = rng.random()
            if drawn < 0.12:
                on += f" AND {self.plain(aliases[:n + 1])}"
            elif drawn < 0.2:
                on += f" AND {self.term(aliases[:n + 1], 1)}"
            text += f" {rng.choice(joins)} {tables[n]} AS a{n} ON {on}"
        scope = aliases + ([around] if around else [])
        where = [self.term(scope, rng.choice([1, 1, 2]))]
        if rng.random() < 0.25:
            where.append(self.term(scope, 1))
        if rng.random() < 0.3:
            where.append(self.plain(aliases))
        rng.shuffle(where)
        return text, aliases, " AND ".join(where)

    def query(self):
        rng = self.rng
        self.made = 0
        drawn = rng.random()
        if drawn < 0.12:
            text, aliases, where = self.joined("o")
            result = rng.choice(["count(*)", f"sum({aliases[-1]}.k)", f"max({aliases[0]}.v)", f"count({aliases[-1]}.k)"])
            return f"SELECT o.k, (SELECT {result} FROM {text} WHERE {where}) FROM {rng.choice(['p', 'n', 'g'])} AS o"
        if drawn < 0.24:
            text, aliases, where = self.joined("o")
            if rng.random() < 0.5:
                return f"SELECT o.k, o.v FROM {rng.choice(['p', 'n', 'g'])} AS o WHERE {rng.choice(['', 'NOT '])}EXISTS (SELECT 1 FROM {text} WHERE {where})"
            return f"SELECT o.k, o.v FROM {rng.choice(['p', 'n', 'g'])} AS o WHERE o.k {rng.choice(['IN', 'NOT IN'])} (SELECT {rng.choice(aliases)}.k FROM {text} WHERE {where})"
        text, aliases, where = self.joined(None)
        columns = [f"{alias}.k" for alias in aliases]
        drawn = rng.random()
        if drawn < 0.5:
            return f"SELECT {', '.join(columns)} FROM {text} WHERE {where}"
        if drawn < 0.62:
            return f"SELECT {', '.join(columns)}, ({self.select(self.some(aliases, 0, 2), 1, 'scalar')}) FROM {text} WHERE {where}"
        if drawn < 0.76:
            return f"SELECT count(*), count({columns[-1]}), sum({columns[-1]}), min({columns[0]}) FROM {text} WHERE {where}"
        if drawn < 0.84:
            return f"SELECT {columns[0]}, count(*), sum({aliases[-1]}.v) FROM {text} WHERE {where} GROUP BY {columns[0]}"
        if drawn < 0.9:
            return f"SELECT DISTINCT {', '.join(columns)} FROM {text} WHERE {where}"
        if drawn < 0.95:
            return f"SELECT {', '.join(columns)} FROM {text} WHERE {where} ORDER BY ({self.select(self.some(aliases, 1, 2), 1, 'scalar')}), {', '.join(str(n + 1) for n in range(len(columns)))}"
        return f"SELECT sum(({self.select(self.some(aliases, 1, 2), 1, 'scalar')})), count(*) FROM {text} WHERE {where}"


def written(value):
    return "NULL" if value is None else str(value)


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 250
    seed = sys.argv[2] if len(sys.argv) > 2 else "CN"
    print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_join_where_subqueries.py {count} {seed}")
    for name, statements in [("plain", TABLES), ("indexed", TABLES + INDEXES)]:
        conn = sqlite3.connect(":memory:")
        print(f"db\t{name}")
        for statement in statements:
            conn.execute(statement)
            print(f"sql\t{statement}")
        draw = Draw(random.Random(f"{seed} {name}"))
        seen, made, found = set(), 0, list(FOUND)
        while made < count:
            sql = found.pop(0) if found else draw.query()
            if sql in seen or len(sql) > 1200:
                continue
            seen.add(sql)
            try:
                rows = conn.execute(sql).fetchall()
            except sqlite3.Error as error:
                print(f"# SQLite refuses {sql}: {error}", file=sys.stderr)
                continue
            lines = sorted(",".join(written(value) for value in row) for row in rows)
            answer = f"{len(lines)}:" + ";".join(lines)
            if len(answer) > 3000:
                continue
            print(f"q\t{sql}\t{answer}")
            made += 1


main()
