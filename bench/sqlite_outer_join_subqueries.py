"""Oracle finding CH (lane C10): LEFT, RIGHT and FULL JOINs whose WHERE, ON or select list holds IN, NOT
IN, EXISTS and scalar subqueries, one inside another, correlated with any table of the FROM or of a
subquery around them, or with none; over tables with and without indexes. Writes each query with the
answer SQLite itself gives (the sqlite3 library Python links, printed first) to
crates/engine/tests/fixtures/outer_join_subqueries.tsv, which tests/outer_join_subqueries.rs holds
Amphora to.

Each line is `db <name>`, `sql <statement>` (the database's tables and rows) or `q <query> <answer>`:
the answer's rows sorted, each row's values joined by commas, a NULL written NULL, the rows joined by
semicolons.
usage: python3 bench/sqlite_outer_join_subqueries.py [queries per database [seed]] > crates/engine/tests/fixtures/outer_join_subqueries.tsv"""
import random
import sqlite3
import sys

TABLES = [
    "CREATE TABLE p(id INTEGER PRIMARY KEY, k INT, v INT, s TEXT)",
    "CREATE TABLE q(id INTEGER PRIMARY KEY, k INT, v INT, s TEXT COLLATE NOCASE)",
    "CREATE TABLE n(k INT, v INT, s TEXT)",
    "CREATE TABLE e(k INT, v INT, s TEXT)",
    "CREATE TABLE w(k INT PRIMARY KEY, v INT, s TEXT) WITHOUT ROWID",
    "INSERT INTO p VALUES (1, 1, 2, '1'), (2, 2, 3, 'b'), (3, NULL, 1, 'B'), (4, 4, NULL, NULL), (5, 1, 0, '2')",
    "INSERT INTO q VALUES (1, 2, 2, 'B'), (2, 2, 0, 'a'), (3, 5, 1, '1'), (4, NULL, 4, NULL)",
    "INSERT INTO n VALUES (1, 2, '2'), (NULL, 1, 'b'), (2, 2, NULL), (3, 0, 'A'), (1, NULL, '1')",
    "INSERT INTO w VALUES (1, 1, 'a'), (2, 0, '2'), (6, NULL, NULL)",
]
INDEXES = [
    "CREATE INDEX p_k ON p(k)",
    "CREATE INDEX q_k ON q(k, v)",
    "CREATE UNIQUE INDEX q_s ON q(s)",
    "CREATE INDEX n_kv ON n(k, v)",
    "CREATE INDEX n_v ON n(v)",
    "CREATE INDEX w_v ON w(v)",
    "CREATE INDEX e_k ON e(k)",
]
SOURCES = ["p", "q", "n", "e", "w"]


class Draw:
    def __init__(self, rng):
        self.rng = rng
        self.made = 0
        self.ctes = False

    def alias(self):
        self.made += 1
        return f"s{self.made}"

    def joined(self, join):
        """A table on `join`'s right. Amphora refuses a WITHOUT ROWID table with an index there in a
        RIGHT or FULL JOIN, with a subquery or without (finding CM, open), so none is drawn."""
        keeps_right = join in ("RIGHT JOIN", "FULL JOIN")
        return self.rng.choice([table for table in SOURCES if table != "w" or not keeps_right])

    def column(self, scope, text=False):
        """A column of one of the aliases in reach, the innermost likeliest."""
        rng = self.rng
        at = scope[-1] if rng.random() < 0.45 else rng.choice(scope)
        return f"{at}.{'s' if text or rng.random() < 0.08 else rng.choice('kkv')}"

    def value(self, scope):
        rng = self.rng
        drawn = rng.random()
        if drawn < 0.6:
            return self.column(scope)
        if drawn < 0.75:
            return str(rng.choice([0, 1, 2, 3]))
        if drawn < 0.8:
            return "NULL"
        return f"({self.column(scope)} {rng.choice(['+', '-', '*'])} {self.column(scope)})"

    def plain(self, scope):
        """A condition with no subquery."""
        rng = self.rng
        drawn = rng.random()
        if drawn < 0.6:
            return f"{self.column(scope)} {rng.choice(['=', '=', '=', '<', '<=', '<>', 'IS'])} {self.value(scope)}"
        if drawn < 0.7:
            return f"{self.column(scope)} IS {rng.choice(['', 'NOT '])}NULL"
        if drawn < 0.78:
            return f"{self.column(scope)} IN ({self.value(scope)}, 2)"
        if drawn < 0.86:
            return f"({self.plain(scope)} OR {self.plain(scope)})"
        if drawn < 0.92:
            return self.column(scope)
        return rng.choice(["NULL", "1", "0", "1 AND " + self.column(scope)])

    def condition(self, scope, depth):
        """A condition that may hold subqueries, `depth` of them one inside another at most."""
        rng = self.rng
        if depth == 0 or rng.random() < 0.25:
            return self.plain(scope)
        return self.subquery_condition(scope, depth)

    def subquery_condition(self, scope, depth):
        rng = self.rng
        drawn = rng.random()
        if drawn < 0.22:
            return f"{self.value(scope)} IN ({self.select(scope, depth, 'list')})"
        if drawn < 0.38:
            return f"{self.value(scope)} NOT IN ({self.select(scope, depth, 'list')})"
        if drawn < 0.5:
            return f"EXISTS ({self.select(scope, depth, 'exists')})"
        if drawn < 0.6:
            return f"NOT EXISTS ({self.select(scope, depth, 'exists')})"
        if drawn < 0.7:
            return f"({self.select(scope, depth, 'scalar')})"
        if drawn < 0.8:
            return f"({self.select(scope, depth, 'scalar')}) {rng.choice(['=', '<', '>=', 'IS NOT'])} {self.value(scope)}"
        if drawn < 0.88:
            return f"({self.subquery_condition(scope, depth)} {rng.choice(['OR', 'AND'])} {self.plain(scope)})"
        if drawn < 0.94:
            return f"NOT ({self.subquery_condition(scope, depth)})"
        return f"({self.subquery_condition(scope, depth)}) IS {rng.choice(['', 'NOT '])}NULL"

    def select(self, scope, depth, use):
        rng = self.rng
        name = self.alias()
        inner = scope + [name]
        source = f"{rng.choice(SOURCES)} AS {name}"
        if rng.random() < 0.12:
            other = self.alias()
            inner = scope + [name, other]
            join = rng.choice(["JOIN", "LEFT JOIN", "RIGHT JOIN", "FULL JOIN"])
            source += f" {join} {self.joined(join)} AS {other} ON {name}.k = {other}.k"
        elif rng.random() < 0.06:
            source = f"(SELECT k, v, s FROM {rng.choice(SOURCES)} WHERE {rng.choice(['v > 0', 'k IS NOT NULL', '1'])}) AS {name}"
        elif rng.random() < 0.04:
            source = f"(SELECT value AS k, key AS v, type AS s FROM json_each(json_array({self.value(scope)}, {self.value(scope)}, 2))) AS {name}"
        elif self.ctes and rng.random() < 0.5:
            source = f"c AS {name}"
        where = [self.condition(inner, depth - 1)]
        if rng.random() < 0.35:
            where.append(self.plain(inner))
        where = " AND ".join(where)
        if use == "exists":
            return f"SELECT 1 FROM {source} WHERE {where}"
        if use == "scalar":
            result = rng.choice([f"count({name}.k)", "count(*)", f"max({name}.v)", f"min({name}.k)", f"sum({name}.v)",
                                 f"{name}.v"])
            if result == f"{name}.v":
                return f"SELECT {result} FROM {source} WHERE {where} ORDER BY 1 LIMIT 1"
            return f"SELECT {result} FROM {source} WHERE {where}"
        result = rng.choice([f"{name}.k", f"{name}.k", f"{name}.v", f"{name}.s", f"({name}.k + {name}.v)"])
        text = f"SELECT {result} FROM {source} WHERE {where}"
        drawn = rng.random()
        if drawn < 0.06:
            return f"{text} UNION ALL SELECT v FROM {rng.choice(SOURCES)}"
        if drawn < 0.12:
            return f"{text} GROUP BY 1 HAVING count(*) > 0"
        return text

    def query(self):
        rng = self.rng
        self.made = 0
        self.ctes = rng.random() < 0.08
        count = rng.choice([2, 2, 2, 3, 3])
        aliases = [f"a{n}" for n in range(count)]
        depth = rng.choice([1, 2, 2, 3])
        text = f"{rng.choice(SOURCES)} AS a0"
        outer = False
        for n in range(1, count):
            joins = ["LEFT JOIN", "RIGHT JOIN", "RIGHT JOIN", "FULL JOIN", "FULL JOIN", "FULL JOIN"]
            if n < count - 1 or outer:
                joins += ["JOIN", "CROSS JOIN"]
            join = rng.choice(joins)
            outer = outer or join not in ("JOIN", "CROSS JOIN")
            scope = aliases[:n + 1]
            drawn = rng.random()
            if drawn < 0.15:
                on = rng.choice(["NULL", "1", "0"])
            elif drawn < 0.75:
                on = f"{aliases[rng.randrange(n)]}.k = {aliases[n]}.{rng.choice('kv')}"
            else:
                on = self.condition(scope, depth)
            text += f" {join} {self.joined(join)} AS {aliases[n]} ON {on}"
        columns = [f"{a}.{c}" for a in aliases for c in ("k", "v")]
        if rng.random() < 0.12:
            columns.append(f"({self.select(aliases, depth, 'scalar')})")
        select = ", ".join(columns)
        if rng.random() < 0.15:
            select = "count(*), " + ", ".join(f"count({c}), sum({c})" for c in columns[:4])
        where = [self.subquery_condition(aliases, depth)]
        if rng.random() < 0.3:
            where.append(self.condition(aliases, depth))
        if rng.random() < 0.2:
            where.append(self.plain(aliases))
        before = ""
        if self.ctes:
            source, hint = rng.choice(SOURCES), rng.choice(["", "", "MATERIALIZED ", "NOT MATERIALIZED "])
            before = f"WITH c AS {hint}(SELECT k, v, s FROM {source} WHERE {rng.choice(['v > 0', 'k IS NOT NULL', '1'])}) "
        return f"{before}SELECT {select} FROM {text} WHERE {' AND '.join(where)}"


def written(value):
    return "NULL" if value is None else str(value)


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 400
    seed = sys.argv[2] if len(sys.argv) > 2 else "CH"
    print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_outer_join_subqueries.py {count} {seed}")
    for name, statements in [("plain", TABLES), ("indexed", TABLES + INDEXES)]:
        conn = sqlite3.connect(":memory:")
        print(f"db\t{name}")
        for statement in statements:
            conn.execute(statement)
            print(f"sql\t{statement}")
        draw = Draw(random.Random(f"{seed} {name}"))
        seen, made = set(), 0
        while made < count:
            sql = draw.query()
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
