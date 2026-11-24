"""T-PL.8.13 (lane C3): left, right and full joins among inner ones, over every FROM order drawn,
with the answers SQLite itself gives (the sqlite3 library Python links, printed first). Writes
crates/engine/tests/fixtures/outer_joins.tsv, which bushy_search_tests' sweep holds every plan to.

Each line is `db <name>`, `sql <statement>` (the database's rows), or `q <query> <answer>`.
usage: python3 bench/sqlite_outer_joins.py > crates/engine/tests/fixtures/outer_joins.tsv"""
import random
import sqlite3


def rows_of(name, count, columns):
    return (f"WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < {count}) "
            f"INSERT INTO {name} SELECT {columns} FROM n")


def database(dims, facts, keys, every_null):
    """Two dimensions and two facts. A fact's d is NULL in one row of `every_null` (0: none) and
    past the dimension's ids in some; its k takes `keys` values."""
    d = f"x % {dims + 2 if every_null else dims} + 1"
    if every_null:
        d = f"CASE WHEN x % {every_null} = 0 THEN NULL ELSE {d} END"
    cats = 25 if dims >= 50 else 5
    return [
        "CREATE TABLE d1(id INTEGER PRIMARY KEY, cat INT)",
        "CREATE TABLE d2(id INTEGER PRIMARY KEY, cat INT)",
        "CREATE TABLE f1(id INTEGER PRIMARY KEY, d1 INT, k INT)",
        "CREATE TABLE f2(id INTEGER PRIMARY KEY, d2 INT, k INT)",
        rows_of("d1", dims, f"x, x % {cats}"),
        rows_of("d2", dims, f"x, (x * 3) % {cats}" if every_null else f"x, x % {cats}"),
        rows_of("f1", facts, f"x, {d}, x % {keys}"),
        rows_of("f2", facts, f"x, {d}, (x * 7) % {keys}" if every_null else f"x, {d}, x % {keys}"),
    ]


# The links an ON may use, by the kinds of the earlier table `a` and the later one `b`; `fk` is the
# fact's dimension column.
LINKS = {
    "dd": ["{b}.id = {a}.id * 3", "{b}.id = {a}.id + 5", "{b}.cat = {a}.cat", "{b}.id = {a}.id"],
    "df": ["{b}.{bfk} = {a}.id", "{b}.id = {a}.id * 7", "{b}.k = {a}.cat"],
    "fd": ["{b}.id = {a}.{afk}", "{b}.id = {a}.id % 60", "{b}.id = {a}.k"],
    "ff": ["{b}.id = {a}.id", "{b}.id = {a}.id * 2", "{b}.k = {a}.k", "{b}.{bfk} = {a}.{afk}"],
}
KIND = {"d1": "d", "d2": "d", "f1": "f", "f2": "f"}
FK = {"f1": "d1", "f2": "d2", "d1": None, "d2": None}


def one_table(alias, table, rng):
    if KIND[table] == "d":
        return rng.choice([f"{alias}.cat < 2", f"{alias}.cat <> 3", f"{alias}.id = 4", f"{alias}.cat = 1",
                           f"{alias}.id > 3"])
    return rng.choice([f"{alias}.k < 3", f"{alias}.k = 2", f"{alias}.id % 3 <> 0", f"{alias}.id < 20",
                       f"{alias}.{FK[table]} = 4"])


def query(rng, joins):
    """A FROM of 4 or 5 tables, each joined to an earlier one by `joins`' draw, with terms on one
    table, on two, on the padded side alone, and with ON terms no index reads."""
    count = rng.choice([4, 4, 5])
    tables = [rng.choice(["d1", "d2", "f1", "f2"]) for _ in range(count)]
    if all(KIND[t] == "f" for t in tables):
        tables[0] = "d1"
    aliases = [f"t{n}" for n in range(count)]
    text = f"{tables[0]} {aliases[0]}"
    outer = []
    for n in range(1, count):
        at = rng.randrange(n)
        link = rng.choice(LINKS[KIND[tables[at]] + KIND[tables[n]]]).format(
            a=aliases[at], b=aliases[n], afk=FK[tables[at]], bfk=FK[tables[n]])
        if rng.random() < 0.3:
            link += " AND " + one_table(aliases[n], tables[n], rng)
        if rng.random() < 0.12:
            link += f" AND {aliases[at]}.id + {aliases[n]}.id > 9"
        kind = rng.choice(joins)
        if kind != "JOIN":
            outer.append(n)
        text += f" {kind} {tables[n]} {aliases[n]} ON {link}"
    where = []
    if rng.random() < 0.75:
        at = rng.randrange(count)
        where.append(one_table(aliases[at], tables[at], rng))
    if rng.random() < 0.4:
        at = rng.randrange(count)
        where.append(one_table(aliases[at], tables[at], rng))
    if outer and rng.random() < 0.3:
        where.append(f"{aliases[rng.choice(outer)]}.id IS NULL")
    if outer and rng.random() < 0.35:
        at, other = rng.choice(outer), rng.randrange(count)
        where.append(f"({aliases[at]}.id IS NULL OR {aliases[at]}.id + {aliases[other]}.id > 12)")
    if rng.random() < 0.2:
        at = rng.randrange(count)
        where.append(f"coalesce({aliases[at]}.id, 3) < 30")
    select = ", ".join(f"count({a}.id), sum({a}.id)" for a in aliases)
    mixed = " + ".join(f"coalesce({a}.id, -{n + 1}) * {31 + 6 * n}" for n, a in enumerate(aliases))
    sql = f"SELECT count(*), {select}, sum(({mixed}) % 997) FROM {text}"
    if where:
        sql += " WHERE " + " AND ".join(where)
    return sql


def fact_queries():
    """The shape t_pl_8_13's queries have, over its own rows, and the same with the LEFT JOIN the
    other way round, hung on a fact, and full."""
    select = "SELECT count(*), sum(f1.id), sum(f2.id), count(s.id), sum(s.cat)"
    out = []
    for on_extra, above in [("", ""), (" AND s.cat <> 11 AND s.cat <> 17", ""), ("", " AND s.id IS NULL"),
                            ("", " AND (s.id IS NULL OR s.cat + f2.k = 11)")]:
        out.append(f"{select} FROM d1 LEFT JOIN d2 s ON s.id = d1.id * 12{on_extra} JOIN f1 ON f1.d1 = d1.id "
                   f"JOIN f2 ON f2.k = f1.k JOIN d2 ON f2.d2 = d2.id WHERE d1.cat < 3 AND d2.id = 7{above}")
        out.append(f"{select} FROM d2 LEFT JOIN d1 s ON s.id = d2.id * 6{on_extra} JOIN f2 ON f2.d2 = d2.id "
                   f"JOIN f1 ON f2.k = f1.k JOIN d1 ON f1.d1 = d1.id WHERE d2.cat < 3 AND d1.id = 3{above}")
        out.append(f"{select} FROM d1 JOIN f1 ON f1.d1 = d1.id LEFT JOIN d2 s ON s.id = f1.id % 60{on_extra} "
                   f"JOIN f2 ON f2.k = f1.k JOIN d2 ON f2.d2 = d2.id WHERE d1.cat = 3 AND d2.id = 7{above}")
        out.append(f"{select} FROM d2 s RIGHT JOIN d1 ON s.id = d1.id * 12{on_extra} JOIN f1 ON f1.d1 = d1.id "
                   f"JOIN f2 ON f2.k = f1.k JOIN d2 ON f2.d2 = d2.id WHERE d1.cat < 3 AND d2.id = 7{above}")
        out.append(f"{select} FROM d1 FULL JOIN d2 s ON s.id = d1.id * 12{on_extra} JOIN f1 ON f1.d1 = d1.id "
                   f"JOIN f2 ON f2.k = f1.k JOIN d2 ON f2.d2 = d2.id WHERE d1.cat < 3 AND d2.id = 7{above}")
    return out


def answer(db, sql):
    (row,) = db.execute(sql).fetchall()
    return ",".join("" if v is None else str(v) for v in row)


DATABASES = [
    # name, dims, facts, keys, one d in this many NULL, queries drawn by seed, joins drawn from
    ("small", 12, 60, 10, 11, 160, ["JOIN", "JOIN", "LEFT JOIN"]),
    ("small_any", 12, 60, 10, 11, 160, ["JOIN", "JOIN", "LEFT JOIN", "RIGHT JOIN", "FULL JOIN"]),
    ("middle", 50, 1500, 300, 13, 80, ["JOIN", "JOIN", "LEFT JOIN", "RIGHT JOIN", "FULL JOIN"]),
    ("facts", 50, 12000, 2, 0, 0, []),
]

print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_outer_joins.py")
for name, dims, facts, keys, every_null, drawn, joins in DATABASES:
    db = sqlite3.connect(":memory:")
    print(f"db\t{name}")
    for sql in database(dims, facts, keys, every_null):
        db.execute(sql)
        print(f"sql\t{sql}")
    db.execute("ANALYZE")
    rng = random.Random(f"amphora T-PL.8.13 {name}")
    queries = fact_queries() if not drawn else []
    while len(queries) < drawn:
        sql = query(rng, joins)
        # A draw whose join is past what a debug build reads in a moment isn't kept.
        steps = [0]
        def step():
            steps[0] += 1
            return 1 if steps[0] > 400 else 0
        db.set_progress_handler(step, 10_000)
        try:
            got = answer(db, sql)
        except sqlite3.OperationalError:
            continue
        finally:
            db.set_progress_handler(None, 0)
        # Most draws hold an outer join and give rows.
        plain = "LEFT" not in sql and "RIGHT" not in sql and "FULL" not in sql
        if (got.startswith("0,") or plain) and rng.random() > 0.1:
            continue
        queries.append(sql)
    for sql in queries:
        print(f"q\t{sql}\t{answer(db, sql)}")
