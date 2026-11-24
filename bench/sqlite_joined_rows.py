"""Oracle findings DS and DT (lane C24): which row a join's other parts read. A virtual table's row by its
rowid, 0 and an outer join's NULL row among them, on every side of every join (DS); the row of a table an
EXISTS, NOT EXISTS, IN or NOT IN was joined by, read by a subquery nested in it (DT). Writes each query with
the answer SQLite itself gives to crates/engine/tests/fixtures/joined_rows.tsv, which tests/joined_rows.rs
holds Amphora to: DS's through the sqlite3 shell (the library Python links has no generate_series), DT's
through that library; both print their version first.

Each line is `db <class> <name>`, `sql <statement>` (the database's tables and rows) or `q <query> <answer>`:
the answer's rows sorted, each row's values joined by commas (a NULL written NULL, a text in quotes), the
rows joined by semicolons.

Not drawn, and why:
- json_each's and json_tree's id and parent: json1.html 4.10 promises only that id differs row by row.
- generate_series' value and rowid beside text or a real: the extension reads either as an integer by plan
  (lane C21's CP), so only atoms of documents of integers are compared.
- A function's argument that reads a table to its right, or an argument given in WHERE: SQLite answers by plan.
- A bare column's value and a tie's order: any of its rows may give it.
usage: python3 bench/sqlite_joined_rows.py [seed] > crates/engine/tests/fixtures/joined_rows.tsv"""
import random
import sqlite3
import subprocess
import sys


def written(value):
    if value is None:
        return "NULL"
    if isinstance(value, str):
        return "'%s'" % value
    return str(value)


def kept(answer):
    return "\t" not in answer and len(answer) < 3000


# DS ------------------------------------------------------------------------------------------------
ROWID_TABLES = [
    "CREATE TABLE a(k INT, v INT)",
    "INSERT INTO a VALUES (0, 7), (1, 8), (2, NULL), (NULL, 7), (3, 0)",
    "CREATE TABLE b(id INTEGER PRIMARY KEY, k INT)",
    "INSERT INTO b VALUES (0, 2), (1, NULL), (2, 0), (4, 7), (-1, 1)",
    "CREATE TABLE d(id INTEGER PRIMARY KEY, j TEXT)",
    "INSERT INTO d VALUES (0, '[7, 1]'), (1, '[[1, 2], 3]'), (2, '{\"a\": {\"b\": 1}, \"c\": [2]}'), (3, NULL), (4, '5'), (5, '[]'), (6, '[0, [0, [0]]]')",
    "CREATE TABLE w(k INT PRIMARY KEY, v INT) WITHOUT ROWID",
    "INSERT INTO w VALUES (0, 1), (1, 0), (2, NULL), (7, 2)",
    "CREATE TABLE z(k INT, v INT)",
    "CREATE TABLE ti(x INTEGER, y TEXT NOT NULL, z)",
    "CREATE UNIQUE INDEX ti_y ON ti(y, x)",
]
ROWID_INDEXES = ["CREATE INDEX a_k ON a(k)", "CREATE INDEX b_k ON b(k)", "CREATE INDEX a_v ON a(v, k)", "CREATE INDEX w_v ON w(v)"]
DOCUMENTS = ["'[7, 1]'", "'[[1, 2], 3]'", "'{\"a\": {\"b\": 1}, \"c\": [2]}'", "'5'", "'[]'", "'[0, [0, [0]]]'", "NULL", "'{\"x\": 0}'"]


def virtual(rng, name, left):
    """A virtual table read as `name`, the table it reads its argument from (none: `left` is empty),
    and its columns an integer compares with."""
    k = f"{rng.choice(left)}.{rng.choice('kkv')}" if left else None
    json = rng.choice(["json_each", "json_tree", "json_tree"])
    literal = rng.choice(DOCUMENTS)
    sources = [
        (f"{json}({literal})", ["atom"]),
        (f"{json}({literal})", ["atom"]),
        (f"generate_series({rng.choice(['0', '-1', '1', '-2'])}, {rng.choice(['2', '3', '0'])})", ["value"]),
        (f"generate_series({rng.choice(['2', '3'])}, {rng.choice(['0', '-1'])}, -1)", ["value"]),
        (f"generate_series(-2, 4, {rng.choice(['2', '3'])})", ["value"]),
        ("pragma_table_info('ti')", ["cid", "pk"]),
        ("pragma_index_info('ti_y')", ["seqno", "cid"]),
        ("pragma_index_list('ti')", ["seq"]),
    ]
    if left:
        sources += [
            (f"{json}(json_array({k}, 0, {rng.choice(left)}.v))", ["atom"]),
            (f"{json}((SELECT d.j FROM d WHERE d.id = {k}))", ["atom"]),
            (f"{json}((SELECT d.j FROM d WHERE d.id IS {k}))", ["atom"]),
            (f"generate_series(0, {k})", ["value"]),
            (f"generate_series({k}, 3)", ["value"]),
            (f"generate_series((SELECT min(b.id) FROM b WHERE b.id >= {k}), 2)", ["value"]),
        ]
    source, columns = rng.choice(sources)
    rowid = rng.choice(["rowid", "rowid", "rowid", "oid", "_rowid_"])
    return f"{source} AS {name}", f"{name}.{rowid}", [f"{name}.{column}" for column in columns], "pragma" in source


def rowid_test(rng, rowid, others, plain):
    """A test that reads the rowid, `others` the columns and rowids beside it, `plain` those of tables."""
    other = rng.choice(others + ["0", "1", "2"])
    return rng.choice([
        f"{rowid} = {other}", f"{other} = {rowid}", f"{rowid} = {other} + 1", f"{rowid} IS {other}", f"{rowid} < {other}", f"{other} <= {rowid}",
        f"{rowid} IS NULL", f"{rowid} IS NOT NULL", f"{rowid} IN (0, 2)", f"{rowid} NOT IN (1, {other})", f"{rowid} BETWEEN 0 AND {other}",
        f"{rowid} IN (SELECT b.id FROM b)", f"{rowid} NOT IN (SELECT b.id FROM b WHERE b.id <> {rng.choice(plain + ['0'])})",
        f"EXISTS (SELECT 1 FROM b WHERE b.id = {rowid})", f"NOT EXISTS (SELECT 1 FROM b WHERE b.id = {rowid} AND b.k IS NOT {rng.choice(plain + ['2'])})",
        f"(SELECT b.k FROM b WHERE b.id = {rowid}) IS NOT NULL", f"({rowid}, 1) = ({other}, 1)", f"coalesce({rowid}, -1) >= {other}", f"{rowid} + 0 = {other}",
    ])


def rowid_query(rng):
    base = rng.choice(["a", "a", "a", "z", "b", "w"])
    base_column = lambda table, name: f"{name}.{'id' if table == 'b' and rng.random() < 0.6 else 'k'}"
    tables, plain, values, rowids = "", [], [], []
    count = rng.choice([1, 2, 2, 2, 3, 3])
    for position in range(count):
        name = f"t{position}"
        joins = ["JOIN", "LEFT JOIN", "LEFT JOIN", "RIGHT JOIN", "FULL JOIN", "FULL JOIN", "RIGHT JOIN", "CROSS JOIN"]
        join = rng.choice(joins)
        is_table = rng.random() < (0.55 if position == 0 and count > 1 else 0.25 if count > 1 else 0)
        if is_table:
            table = rng.choice([base, "a", "b"])
            column = base_column(table, name)
            piece, rowid, columns = f"{table} AS {name}", None, [column]
        else:
            left = [n for n in plain if n.split(".")[0] in [p.split(".")[0] for p in plain]]
            # an argument reads an ordinary table to its left that has k and v
            names = sorted({n.split(".")[0] for n in plain if n.endswith(".k")} & correlated_names(tables))
            piece, rowid, columns, _ = virtual(rng, name, names if rng.random() < 0.45 else [])
        if position == 0:
            tables = piece
        else:
            tests = []
            if join != "CROSS JOIN":
                mine = [rowid] if rowid else columns
                before = values + rowids + plain
                tests = [rng.choice([
                    f"{rng.choice(mine)} = {rng.choice(before)}", f"{rng.choice(before)} = {rng.choice(mine)}", f"{rng.choice(columns)} = {rng.choice(before)}", "1",
                    rowid_test(rng, rng.choice(mine), before, plain), rowid_test(rng, rng.choice(before), mine, plain), f"{rng.choice(mine)} <= {rng.choice(before)}",
                ])]
                if rng.random() < 0.2:
                    tests.append(rowid_test(rng, rng.choice(mine), before, plain))
            tables += f" {join} {piece}" + (f" ON {' AND '.join(tests)}" if tests else "")
        if rowid:
            rowids.append(rowid)
            values += columns
        else:
            plain += columns
    shown = rowids + plain + values[:2]
    if not rowids:
        return None
    where = ""
    if rng.random() < 0.45:
        terms = [rowid_test(rng, rng.choice(rowids), shown, plain)]
        if rng.random() < 0.3:
            terms.append(rowid_test(rng, rng.choice(rowids), shown, plain))
        where = " WHERE " + rng.choice([" AND ", " OR "]).join(terms)
    rowid = rng.choice(rowids)
    shape = rng.random()
    if shape < 0.5:
        return f"SELECT {', '.join(shown)} FROM {tables}{where}"
    if shape < 0.58:
        return f"SELECT count({rowid}), min({rowid}), max({rowid}), sum({rowid}), count(*), count(DISTINCT {rowid}) FROM {tables}{where}"
    if shape < 0.66:
        return f"SELECT {rowid}, count(*), count({rng.choice(shown)}) FROM {tables}{where} GROUP BY {rowid}{rng.choice(['', f' HAVING {rowid} IS NOT NULL', f' HAVING count(*) > 1'])}"
    if shape < 0.72:
        return f"SELECT DISTINCT {rowid}, {rng.choice(shown)} FROM {tables}{where}"
    if shape < 0.78:
        return f"SELECT {rowid}, count(*) OVER (PARTITION BY {rowid}), max({rowid}) OVER () FROM {tables}{where}"
    if shape < 0.82:
        return f"SELECT s.r, s.r IS NULL, typeof(s.r) FROM (SELECT {rowid} AS r FROM {tables}{where}) AS s WHERE {rng.choice(['s.r = 0', 's.r IS NULL', 's.r >= 0', '1'])}"
    if shape < 0.86:
        return f"WITH c(r) AS {rng.choice(['', 'MATERIALIZED ', 'NOT MATERIALIZED '])}(SELECT {rowid} FROM {tables}{where}) SELECT a.k, c.r FROM a {rng.choice(['JOIN', 'LEFT JOIN', 'FULL JOIN'])} c ON c.r = a.k"
    if shape < 0.92:
        return f"SELECT a.k FROM a WHERE a.k {rng.choice(['IN', 'NOT IN'])} (SELECT {rowid} FROM {tables}{where})"
    if shape < 0.97:
        return f"SELECT q.k, (SELECT count(*) FROM {tables}{where + ' AND' if where else ' WHERE'} {rowid} = q.k) FROM a AS q"
    return f"SELECT {rowid} FROM {tables}{where} UNION {rng.choice(['', 'ALL '])}SELECT k FROM a"


def correlated_names(tables):
    """The ordinary tables of `tables` that have both k and v (b has no v)."""
    words = tables.replace("(", " ").replace(")", " ").split()
    return {words[i + 2] for i in range(len(words) - 2) if words[i] in ("a", "w", "z") and words[i + 1] == "AS"}


def shell(statements, queries):
    """SQLite's answers through its shell, None where it refuses the query."""
    script = ".mode quote\n.headers off\n" + "".join(f"{s};\n" for s in statements)
    script += "".join(f".print ===\n{q};\n" for q in queries)
    run = subprocess.run(["sqlite3", "-batch", ":memory:"], input=script, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, check=False)
    parts = run.stdout.split("===\n")
    assert len(parts) == len(queries) + 1, f"{len(parts)} answers to {len(queries)} queries"
    assert not parts[0].strip(), parts[0]
    answers = []
    for part in parts[1:]:
        rows = part.split("\n")[:-1]
        answers.append(None if "rror" in part else f"{len(rows)}:{';'.join(sorted(rows))}")
    return answers


def rowids(seed):
    for name, indexes, count in (("plain", [], 900), ("indexed", ROWID_INDEXES, 500)):
        rng = random.Random(f"{seed} ds {name}")
        statements = ROWID_TABLES + indexes
        queries = list(ROWID_FOUND)
        while len(queries) < count * 2:
            query = rowid_query(rng)
            if query and query not in queries:
                queries.append(query)
        print(f"db\tds\t{name}")
        for statement in statements:
            print(f"sql\t{statement}")
        asked = 0
        for query, answer in zip(queries, shell(statements, queries)):
            if answer is None or not kept(answer) or (answer.startswith("0:") and rng.random() < 0.8):
                continue
            if asked < count:
                print(f"q\t{query}\t{answer}")
                asked += 1


ROWID_FOUND = [
    "SELECT rowid FROM json_tree('[7]')",
    "SELECT x.k, je.atom FROM a AS x RIGHT JOIN json_tree('[7,1]') AS je ON 1",
    "SELECT x.k, je.atom FROM a AS x FULL JOIN json_tree('[7,1]') AS je ON je.atom = x.k",
    "SELECT x.k, je.rowid FROM a AS x LEFT JOIN json_each('[7,1]') AS je ON je.value = x.k",
    "SELECT x.k, g.rowid FROM a AS x FULL JOIN generate_series(0, 2) AS g ON g.value = x.k",
    "SELECT x.k, p.rowid, p.cid FROM a AS x LEFT JOIN pragma_table_info('ti') AS p ON p.cid = x.k",
    "SELECT x.k, p.rowid, p.cid FROM a AS x FULL JOIN pragma_table_info('ti') AS p ON p.cid = x.v",
    "SELECT d.id, je.rowid, je.type FROM d FULL JOIN json_tree(d.j) AS je ON je.type = 'integer'",
    "SELECT d.id, je.rowid, je.type FROM d RIGHT JOIN json_tree(d.j) AS je ON je.type = 'integer'",
    "SELECT d.id, je.rowid, je.type FROM d LEFT JOIN json_tree(d.j) AS je ON je.type = 'integer'",
    "SELECT value FROM json_each('[7,1]') WHERE rowid = 0",
    "SELECT value FROM generate_series(0,3) WHERE rowid IN (0, 2)",
    "SELECT je.rowid, jf.rowid FROM json_tree('[7,1]') AS je FULL JOIN json_tree('[7,2]') AS jf ON je.rowid = jf.rowid + 1",
    "SELECT b.id, je.rowid FROM json_tree('[7,1]') AS je LEFT JOIN b ON b.id = je.rowid",
    "SELECT b.id, je.rowid FROM b RIGHT JOIN json_tree('[7,1]') AS je ON b.id = je.rowid",
]


# DT ------------------------------------------------------------------------------------------------
def nested_tables(rng, indexed):
    """Seven tables, each empty or of one of these sizes (the costs move the joins' order), their values
    NULL, repeated or both."""
    statements = []
    for name in ("p", "q", "n", "e", "m", "x"):
        statements.append(f"CREATE TABLE {name}(id INTEGER PRIMARY KEY, k INT, v INT)")
    statements.append("CREATE TABLE w(k INT PRIMARY KEY, v INT) WITHOUT ROWID")
    for name in ("p", "q", "n", "e", "m"):
        values = rng.choice([4, 8, 30, 100])
        value = lambda: "NULL" if rng.random() < 0.1 else str(rng.randrange(values))
        rows = [f"({row + 1}, {value()}, {value()})" for row in range(rng.choice([3, 6, 12, 25, 40]))]
        statements.append(f"INSERT INTO {name} VALUES {', '.join(rows)}")
    rows = [f"({row}, {'NULL' if row % 7 == 3 else row * 3 % 11})" for row in range(rng.choice([3, 6, 12, 25]))]
    statements.append(f"INSERT INTO w VALUES {', '.join(rows)}")
    if indexed:
        for name in ("p", "q", "n", "e", "m", "x"):
            if rng.random() < 0.6:
                statements.append(f"CREATE INDEX {name}_k ON {name}(k{rng.choice(['', ', v'])})")
            if rng.random() < 0.3:
                statements.append(f"CREATE INDEX {name}_v ON {name}(v)")
    return statements


def nested_query(rng):
    source = lambda: rng.choice("ppqqnnmmewx")
    column = lambda alias: f"{alias}.{rng.choice('kkv')}"
    names = ["a0"]
    tables = f"{source()} AS a0"
    for position in range(1, rng.choice([1, 2, 2, 2, 3])):
        tables += f" {rng.choice(['JOIN', 'LEFT JOIN', 'LEFT JOIN'])} {source()} AS a{position} ON {column(rng.choice(names))} = {column(f'a{position}')}"
        names.append(f"a{position}")
    outer = lambda: rng.choice(names)

    def inner(a, s1, s2):
        both = rng.choice(["<", ">=", "<>", "="])
        correlated = rng.choice([
            f"{column(s2)} = {column(a)} AND {column(s2)} {both} {column(s1)}", f"{column(s2)} = {column(s1)} AND {column(s2)} {both} {column(a)}",
            f"{column(s2)} = {column(s1)}", f"{column(s2)} = {column(s1)}", f"{column(a)} < {column(s2)} AND {column(s2)} = {column(s1)}", f"{column(s2)} = {column(a)}",
        ])
        exists = f"{rng.choice(['', 'NOT '])}EXISTS (SELECT 1 FROM {source()} AS {s2} WHERE {correlated})"
        return rng.choice([
            exists, exists, exists, f"({exists} OR {column(s1)} = {rng.randrange(6)})", f"({column(s1)} > {rng.randrange(6)} OR {exists})",
            f"CASE WHEN {exists} THEN {column(s1)} > 1 ELSE {column(s1)} <= 1 END", f"({column(s1)} > 1) = ({exists})",
            f"{column(s1)} {rng.choice(['IN', 'NOT IN'])} (SELECT {column(s2)} FROM {source()} AS {s2} WHERE {correlated})",
            f"{column(s1)} {rng.choice(['=', '<', '>='])} (SELECT {rng.choice(['max', 'min', 'count'])}({column(s2)}) FROM {source()} AS {s2} WHERE {correlated})",
        ])

    def test(s1, s2):
        a = outer()
        beside = rng.choice(["", f" AND {column(s1)} = {column(a)}", f" AND {column(s1)} = {column(a)}", f" AND {column(s1)} <= {column(outer())}", f" AND {column(s1)} = {column(outer())}"])
        exists = f"{rng.choice(['', '', 'NOT '])}EXISTS (SELECT 1 FROM {source()} AS {s1} WHERE {inner(a, s1, s2)}{beside})"
        return rng.choice([exists, exists, f"{column(a)} {rng.choice(['IN', 'IN', 'NOT IN'])} (SELECT {column(s1)} FROM {source()} AS {s1} WHERE {inner(a, s1, s2)}{beside})"])

    def beside(s):
        a = outer()
        return rng.choice([
            f"{rng.choice(['', 'NOT ', 'NOT '])}EXISTS (SELECT 1 FROM {source()} AS {s} WHERE {column(s)} = {column(a)}{rng.choice(['', f' AND {column(s)} > 2'])})",
            f"({rng.choice(['', 'NOT '])}EXISTS (SELECT 1 FROM {source()} AS {s} WHERE {column(s)} = {column(a)}) OR {column(outer())} = {rng.randrange(6)})",
            f"{column(outer())} {rng.choice(['<', '<=', '>', '>=', '='])} (SELECT {rng.choice(['avg', 'max', 'min', 'count', 'sum'])}({column(s)}) FROM {source()} AS {s} WHERE {column(s)} = {column(a)})",
            f"{column(outer())} {rng.choice(['IN', 'NOT IN'])} (SELECT {column(s)} FROM {source()} AS {s} WHERE {column(s)} = {column(a)})",
            f"{column(a)} {rng.choice(['=', '<', 'IS NOT'])} {rng.randrange(5)}",
        ])

    terms = [test("s1", "s2")] + [beside(f"s{8 + position}") for position in range(rng.choice([1, 1, 2]))]
    if rng.random() < 0.2:
        terms.append(test("s5", "s6"))
    rng.shuffle(terms)
    shown = ", ".join(f"{name}.k, {name}.v" for name in names)
    if rng.random() < 0.6:
        shown = f"count(*), sum({names[0]}.k), sum({names[-1]}.v), count({names[0]}.v)"
    return f"SELECT {shown} FROM {tables} WHERE {' AND '.join(terms)}"


def nested(seed):
    for number in range(30):
        rng = random.Random(f"{seed} dt {number}")
        statements = NESTED_FOUND[0][0] if number == 0 else nested_tables(rng, number % 2 == 1)
        con = sqlite3.connect(":memory:", isolation_level=None)
        print(f"db\tdt\t{number}")
        for statement in statements:
            con.execute(statement)
            print(f"sql\t{statement}")
        queries = list(NESTED_FOUND[0][1]) if number == 0 else []
        asked = 0
        for _ in range(600):
            query = queries.pop(0) if queries else nested_query(rng)
            steps = [0]

            def stop():
                steps[0] += 1
                return steps[0] > 40

            con.set_progress_handler(stop, 10000)
            try:
                rows = sorted(",".join(written(v) for v in row) for row in con.execute(query).fetchall())
            except sqlite3.Error:
                continue
            answer = f"{len(rows)}:{';'.join(rows)}"
            if not kept(answer) or (rows in ([], ["0,NULL,NULL,0"]) and rng.random() < 0.9):
                continue
            print(f"q\t{query}\t{answer}")
            asked += 1
            if asked == 50:
                break


NESTED_FOUND = [(
    [
        "CREATE TABLE p(id INTEGER PRIMARY KEY, k INT, v INT, s TEXT)",
        "CREATE TABLE q(id INTEGER PRIMARY KEY, k INT, v INT, s TEXT COLLATE NOCASE)",
        "CREATE TABLE n(k INT, v INT, s TEXT)",
        "CREATE TABLE e(k INT, v INT, s TEXT)",
        "CREATE TABLE m(k INT, v INT, s TEXT)",
        "CREATE TABLE x(k INT, v INT, s TEXT)",
        "CREATE TABLE w(k INT PRIMARY KEY, v INT, s TEXT) WITHOUT ROWID",
        "INSERT INTO p VALUES (1, 1, 2, '1'), (2, 2, 3, 'b'), (3, NULL, 1, 'B'), (4, 4, NULL, NULL), (5, 1, 0, '2')",
        "INSERT INTO q VALUES (1, 2, 2, 'B'), (2, 2, 0, 'a'), (3, 5, 1, '1'), (4, NULL, 4, NULL), (5, 1, 1, 'A')",
        "INSERT INTO n VALUES (1, 2, '2'), (NULL, 1, 'b'), (2, 2, NULL), (3, 0, 'A'), (1, NULL, '1')",
        "INSERT INTO w VALUES (1, 1, 'a'), (2, 0, '2'), (6, NULL, NULL)",
    ],
    [
        "SELECT a0.k, a0.v, a1.k, a1.v FROM p AS a0 LEFT JOIN w AS a1 ON a0.v = a1.k WHERE EXISTS (SELECT 1 FROM n AS s1 WHERE NOT EXISTS (SELECT 1 FROM p AS s2 WHERE a1.v < s2.k AND s2.k = s1.k)) AND NOT EXISTS (SELECT 1 FROM e AS s9 WHERE s9.k = a1.v)",
    ],
)]


def main():
    seed = sys.argv[1] if len(sys.argv) > 1 else "C24"
    library = sqlite3.connect(":memory:").execute("SELECT sqlite_source_id()").fetchone()[0]
    version = subprocess.run(["sqlite3", "--version"], stdout=subprocess.PIPE, text=True, check=True).stdout.strip()
    print(f"# the sqlite3 library Python links: {library}; the shell: {version}; seed {seed}")
    rowids(seed)
    nested(seed)


main()
