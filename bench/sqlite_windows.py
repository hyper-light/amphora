"""Oracle finding CO (lane C17): window functions over each other. Every built-in and aggregate window
function, alone, inside an expression and read twice, under one to three windows of a query: frames by
ROWS, RANGE and GROUPS, EXCLUDE, FILTER, PARTITION BY, named and chained windows, keys the planner can't
take for deterministic (date(at)); over a table, a join, a derived table, a CTE, groups and DISTINCT, in
the select list, ORDER BY and a subquery. Writes each query with the answer SQLite itself gives (the
sqlite3 library Python links, printed first) to crates/engine/tests/fixtures/windows.tsv, which
tests/windows_and_triggers.rs holds Amphora to.

Each line is `db <name>`, `sql <statement>` (the database's tables and rows) or `q <query> <answer>`:
the answer's rows sorted, each row's values joined by commas, a NULL written NULL, a REAL by its first
nine digits after the point, the rows joined by semicolons.
usage: python3 bench/sqlite_windows.py [queries per database [seed]] > crates/engine/tests/fixtures/windows.tsv"""
import random
import sqlite3
import sys

TABLES = [
    "CREATE TABLE ev(id INTEGER PRIMARY KEY, at TEXT, g TEXT, v INT, n INT)",
    "CREATE TABLE w(g TEXT PRIMARY KEY, m INT) WITHOUT ROWID",
    "INSERT INTO ev VALUES (1, '2026-01-01 10:00', 'a', 5, 1), (2, '2026-01-01 11:00', 'b', 3, NULL), (3, '2026-01-02 09:00', 'a', 9, 2), (4, '2026-01-02 09:00', NULL, 3, 2), (5, NULL, 'b', NULL, 0), (6, '2026-01-03 00:00', 'A', 5, 7), (7, '2026-01-03 08:00', 'c', -1, NULL)",
    "INSERT INTO w VALUES ('a', 1), ('b', 2), ('d', 3)",
]
INDEXES = ["CREATE INDEX ev_v ON ev(v)", "CREATE INDEX ev_g ON ev(g, v)"]
# What the oracles and wider draws found Amphora failing: each is asked first.
FOUND = [
    "SELECT id, sum(v) OVER (ORDER BY id), lag(v + 1) OVER (ORDER BY date(at), id), coalesce(lag(v + 1) OVER (ORDER BY date(at), id), 0) FROM ev",
    "SELECT id FROM ev ORDER BY sum(v) OVER (ORDER BY id), lag(v + 1) OVER (ORDER BY date(at), id), typeof(lag(v + 1) OVER (ORDER BY date(at), id)) LIMIT 5",
    "SELECT 1 FROM ev ORDER BY NULL, ntile(1) OVER (ORDER BY ev.rowid), lag('x') OVER (ORDER BY v, strftime(v, 1.0), ev.rowid DESC), typeof(lag('x') OVER (ORDER BY v, strftime(v, 1.0), ev.rowid DESC)) LIMIT 2 OFFSET 0",
    "SELECT id, rank() OVER (ORDER BY g), lead(v) OVER (PARTITION BY date(at) ORDER BY id), lead(v) OVER (PARTITION BY date(at) ORDER BY id) - v FROM ev",
    "SELECT id, count(*) OVER (), first_value(v + 1) OVER (ORDER BY date(at), id), nth_value(v + 1, 2) OVER (ORDER BY date(at), id), abs(first_value(v + 1) OVER (ORDER BY date(at), id)), nth_value(v + 1, 2) OVER (ORDER BY date(at), id) + 1 FROM ev",
    "SELECT id, sum(v) OVER (ORDER BY id), first_value(random()) OVER (ORDER BY v, id) = first_value(random()) OVER (ORDER BY v, id) FROM ev",
    "SELECT id, sum(v) OVER (ORDER BY id), json_group_array(date(at)) OVER (ORDER BY v, id), json_group_array(date(at)) OVER (ORDER BY v, id) || 'x' FROM ev",
    "SELECT id, sum(v) OVER (ORDER BY id), count(date(at)) FILTER (WHERE date(at) > '2026-01-01') OVER (ORDER BY g, id), count(date(at)) FILTER (WHERE date(at) > '2026-01-01') OVER (ORDER BY g, id) * 2 FROM ev",
    "SELECT g, sum(count(*)) OVER (ORDER BY g), lag(sum(v)) OVER (ORDER BY date(max(at)), g), coalesce(lag(sum(v)) OVER (ORDER BY date(max(at)), g), 0) FROM ev GROUP BY g",
    "SELECT s.x, s.y FROM (SELECT lag(v) OVER (ORDER BY date(at), id) AS x, coalesce(lag(v) OVER (ORDER BY date(at), id), 0) AS y, sum(v) OVER (ORDER BY id) AS z FROM ev) AS s WHERE s.z > 5",
]


class Draw:
    def __init__(self, rng, columns, key, grouped):
        self.rng = rng
        self.columns = columns  # the integers, texts and dates a call may read
        self.key = key  # what makes a window's order total
        self.grouped = grouped
        self.windows = [self.window() for _ in range(rng.choice([1, 2, 2, 3]))]
        self.calls = []

    def scalar(self):
        rng = self.rng
        ints, texts, dates = self.columns
        drawn = rng.random()
        if drawn < 0.45:
            return rng.choice(ints)
        if drawn < 0.6:
            return f"{rng.choice(ints)} + {rng.choice([1, 2, 10])}"
        if drawn < 0.7:
            return rng.choice(texts)
        if drawn < 0.8 and dates:
            return f"date({rng.choice(dates)})"
        if drawn < 0.88:
            return f"coalesce({rng.choice(ints)}, {rng.choice([0, -3])})"
        if drawn < 0.94:
            return rng.choice(["'x'", "7", "NULL"])
        return f"CASE WHEN {rng.choice(ints)} > 2 THEN {rng.choice(ints)} ELSE {rng.choice(texts)} END"

    def integer(self):
        ints = self.columns[0]
        rng = self.rng
        return rng.choice([rng.choice(ints), f"{rng.choice(ints)} + 1", f"({rng.choice(ints)} % 4)"])

    def term(self):
        rng = self.rng
        ints, texts, dates = self.columns
        drawn = rng.random()
        if drawn < 0.35:
            term = rng.choice(ints)
        elif drawn < 0.5:
            term = rng.choice(texts)
        elif drawn < 0.75 and dates:
            term = rng.choice([f"date({rng.choice(dates)})", f"strftime('%d', {rng.choice(dates)})"])
        elif drawn < 0.85:
            term = f"{rng.choice(ints)} % 3"
        else:
            term = f"lower({rng.choice(texts)})"
        # SQLite's window over a GROUP BY misplaces a NULL group under DESC (the oracles' RW).
        if not self.grouped and rng.random() < 0.25:
            term += rng.choice([" DESC", " DESC", " ASC NULLS LAST", " DESC NULLS FIRST"])
        return term

    def window(self):
        rng = self.rng
        partition = [t.split(" DESC")[0].split(" ASC")[0] for t in (self.term() for _ in range(rng.choice([0, 0, 1, 1, 2])))]
        order = [self.term() for _ in range(rng.choice([0, 1, 1, 2]))]
        total = rng.random() < 0.7
        if total:
            order += [k + rng.choice(["", "", " DESC"] if not self.grouped else [""]) for k in self.key]
        return partition, order, total

    def over(self, at, frame=""):
        partition, order, _ = self.windows[at]
        parts = []
        if partition:
            parts.append("PARTITION BY " + ", ".join(partition))
        if order:
            parts.append("ORDER BY " + ", ".join(order))
        if frame:
            parts.append(frame)
        return "(" + " ".join(parts) + ")"

    def frame(self, at):
        rng = self.rng
        _, order, total = self.windows[at]
        if rng.random() < 0.45:
            return ""
        modes = ["ROWS"] if total else []
        modes += ["GROUPS", "RANGE"] if order else ["RANGE"]
        mode = rng.choice(modes)
        offsets = mode != "RANGE"
        starts = ["UNBOUNDED PRECEDING", "CURRENT ROW"] + (["1 PRECEDING", "2 PRECEDING", "1 FOLLOWING"] if offsets else [])
        ends = ["UNBOUNDED FOLLOWING", "CURRENT ROW"] + (["1 PRECEDING", "1 FOLLOWING", "2 FOLLOWING"] if offsets else [])
        start, end = rng.choice(starts), rng.choice(ends)
        if start.endswith("FOLLOWING") and not end.endswith("FOLLOWING"):
            end = "UNBOUNDED FOLLOWING"
        if start == "CURRENT ROW" and end.endswith("PRECEDING"):
            end = "CURRENT ROW"
        exclude = rng.choice(["", "", "", " EXCLUDE NO OTHERS", " EXCLUDE CURRENT ROW", " EXCLUDE GROUP", " EXCLUDE TIES"])
        if not total and exclude == " EXCLUDE CURRENT ROW":
            exclude = " EXCLUDE GROUP"
        return f"{mode} BETWEEN {start} AND {end}{exclude}"

    def call(self):
        """A window function call: one whose answer ties in the window's order can't change."""
        rng = self.rng
        if self.calls and rng.random() < 0.4:
            return rng.choice(self.calls)
        at = rng.randrange(len(self.windows))
        _, order, total = self.windows[at]
        free = ["rank()", "dense_rank()", "percent_rank()", "cume_dist()"]
        drawn = rng.random()
        if total and drawn < 0.5:
            name = rng.choice(["lag", "lead", "first_value", "last_value", "nth_value", "row_number", "ntile"])
            if name in ("lag", "lead"):
                more = rng.choice(["", "", ", 0", ", 2", ", 1, -9", f", 2, {self.scalar()}"])
                call = f"{name}({self.scalar()}{more})"
                over = self.over(at)
            elif name in ("first_value", "last_value"):
                call, over = f"{name}({self.scalar()})", self.over(at, self.frame(at))
            elif name == "nth_value":
                call, over = f"nth_value({self.scalar()}, {rng.choice([1, 2, 3])})", self.over(at, self.frame(at))
            elif name == "ntile":
                call, over = f"ntile({rng.choice([1, 2, 3, 50])})", self.over(at)
            else:
                call, over = "row_number()", self.over(at)
        elif drawn < 0.65:
            call, over = rng.choice(free), self.over(at)
        else:
            name = rng.choice(["sum", "count", "min", "max", "avg", "total", "count(*)"])
            arg = self.integer() if name in ("sum", "avg", "total") else self.scalar()
            if total and rng.random() < 0.15:
                name, arg = rng.choice([("group_concat", "'z' || " + rng.choice(self.columns[1])), ("json_group_array", self.scalar())])
            call = name if name == "count(*)" else f"{name}({arg})"
            if rng.random() < 0.25:
                call += f" FILTER (WHERE {rng.choice(self.columns[0])} > {rng.choice([0, 2, 4])})"
            over = self.over(at, self.frame(at))
        text = f"{call} OVER {over}"
        self.calls.append(text)
        return text

    def expression(self):
        rng = self.rng
        call = self.call()
        drawn = rng.random()
        if drawn < 0.45:
            return call
        if drawn < 0.6:
            return f"coalesce({call}, 0)"
        if drawn < 0.7:
            return f"typeof({call})"
        if drawn < 0.78:
            return f"CASE WHEN {call} IS NULL THEN 'none' ELSE {self.call()} END"
        if drawn < 0.86:
            return f"{call} = {self.call()}"
        if drawn < 0.93:
            return f"quote({call}) || '|' || quote({self.call()})"
        return f"({call}) IS {self.scalar()}"


def named(sql, draw):
    """The same query with its first window named, where one is read whole."""
    over = draw.over(0)
    if f"OVER {over}" not in sql or over == "()":
        return None
    out = sql.replace(f"OVER {over}", "OVER wn")
    at = len(out)
    for word in (" ORDER BY oq", " LIMIT "):
        found = out.rfind(word)
        if found >= 0:
            at = min(at, found)
    return out[:at] + f" WINDOW wn AS {over}" + out[at:]


def query(rng):
    shape = rng.random()
    grouped = shape < 0.2
    if grouped:
        key = rng.choice(["g", "v", "date(at)"])
        columns = (["count(*)", "sum(v)", "max(n)", "min(id)"], [f"coalesce({key}, '-')"], ["max(at)"])
        draw = Draw(rng, columns, ["min(id)"], True)
        select = ", ".join([key] + [draw.expression() for _ in range(rng.choice([1, 2, 3]))])
        having = rng.choice(["", "", " HAVING count(*) > 1"])
        return f"SELECT {select} FROM ev GROUP BY {key}{having}", draw
    columns = (["v", "n", "id"], ["g", "at"], ["at"])
    draw = Draw(rng, columns, ["id"], False)
    expressions = [draw.expression() for _ in range(rng.choice([1, 2, 2, 3, 4]))]
    if shape < 0.5:
        source = "ev"
    elif shape < 0.6:
        source = rng.choice(["(SELECT * FROM ev WHERE id <> 3) AS ev", "(SELECT id, at, g, v, n FROM ev UNION ALL SELECT id + 100, at, g, n, v FROM ev) AS ev"])
    elif shape < 0.72:
        columns = (["ev.v", "w.m", "ev.id"], ["ev.g", "w.g"], ["ev.at"])
        draw = Draw(rng, columns, ["ev.id", "w.m"], False)
        expressions = [draw.expression() for _ in range(rng.choice([1, 2, 3]))]
        join = rng.choice(["JOIN w ON w.g = ev.g", "LEFT JOIN w ON w.g = ev.g", "LEFT JOIN w ON w.m > ev.n", "FULL JOIN w ON w.g = ev.g", "JOIN w"])
        return f"SELECT ev.id, w.m, {', '.join(expressions)} FROM ev {join}", draw
    elif shape < 0.8:
        inner = ", ".join(f"{e} AS c{i}" for i, e in enumerate(expressions))
        outer = rng.choice([", ".join(f"c{i}" for i in range(len(expressions))), "max(c0), count(*)", "id, c0"])
        where = rng.choice(["", " WHERE c0 IS NOT NULL", " WHERE id > 2"])
        return f"SELECT {outer} FROM (SELECT id, {inner} FROM ev) AS s{where}", draw
    elif shape < 0.86:
        return f"WITH s AS (SELECT id, {', '.join(f'{e} AS c{i}' for i, e in enumerate(expressions))} FROM ev) SELECT a.id, a.c0, b.c0 FROM s AS a JOIN s AS b ON b.id = a.id + 1", draw
    elif shape < 0.92:
        return f"SELECT DISTINCT {', '.join(expressions)} FROM ev", draw
    else:
        order = ", ".join(draw.expression() + rng.choice(["", " DESC"]) for _ in range(rng.choice([1, 2, 3])))
        limit = rng.choice(["", " LIMIT 3", " LIMIT 4 OFFSET 2"])
        return f"SELECT id{''.join(', ' + e for e in expressions[:2])} FROM ev ORDER BY {order}, id{limit}", draw
    where = rng.choice(["", "", "", " WHERE v > 0", " WHERE date(at) > '2026-01-01'"])
    return f"SELECT {', '.join(expressions)} FROM {source}{where}", draw


def text(value):
    if value is None:
        return "NULL"
    if isinstance(value, float):
        return f"{value:.9f}"
    return str(value)


def answer(db, sql):
    rows = sorted(",".join(text(value) for value in row) for row in db.execute(sql).fetchall())
    return f"{len(rows)}:{';'.join(rows)}"


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 400
    seed = sys.argv[2] if len(sys.argv) > 2 else "CO"
    print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_windows.py {count} {seed}")
    for name, statements in (("plain", TABLES), ("indexed", TABLES + INDEXES)):
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
            sql, draw = query(rng)
            if rng.random() < 0.15:
                sql = named(sql, draw) or sql
            if sql in seen:
                continue
            seen.add(sql)
            try:
                got = answer(db, sql)
            except sqlite3.Error:
                refused += 1
                continue
            drawn += 1
            print(f"q\t{sql}\t{got}")
        print(f"# {name}: {drawn} drawn, {refused} SQLite refused", file=sys.stderr)


main()
