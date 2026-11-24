#!/usr/bin/env python3
"""crates/engine/tests/fixtures/refused_queries.tsv: generated queries of the shapes Amphora refused, with SQLite's rows
(AC-T1.8.6; bench/records/2026-11-24-adacachy2-oracle-c11's setup refusals).

- A compound SELECT's ORDER BY terms: a position, a name or an expression, under any parentheses, signs and COLLATEs.
- A window function's arguments, its FILTER, PARTITION BY and ORDER BY that hold subqueries, correlated or not.
- A view's, a derived table's or a CTE's columns that fail on some row and that nothing reads.

usage: bench/sqlite_refused_queries.py COUNT SEED > crates/engine/tests/fixtures/refused_queries.tsv   (SQLite 3.54.0)

A compound's values are distinct under every collation, so that the order is one; its columns declare no collation (which
arm's collation sorts is CY). No correlated subquery is a compound (refused anywhere), no query has two of lag, lead,
first_value, last_value and nth_value (CO), and no `SELECT *` is over two windows (it has a column too few). No view is
DISTINCT or grouped: each of its columns counts, and which rows reach it is the push-down's.
"""
import random
import sqlite3
import struct
import sys

TEXTS = ["'apple'", "'Banana'", "'cherry'", "'Date'", "'egg'", "'Fig'", "'_x'", "'Zed'", "'10'", "x'41'", "NULL", "3", "2.5"]
COLLATIONS = ["NOCASE", "BINARY", "RTRIM"]


def compound(rng):
    texts = rng.sample(TEXTS, 8)
    ints = rng.sample(range(-9, 60), 8)
    cells = [f"({t}, {n})" for t, n in zip(texts, ints)]
    setup = ["CREATE TABLE t(a, b INT)", "CREATE TABLE u(a, c INT)", "INSERT INTO t VALUES " + ", ".join(cells[:4]), "INSERT INTO u VALUES " + ", ".join(cells[4:7])]
    op = rng.choice(["UNION", "UNION ALL", "EXCEPT", "INTERSECT", "UNION", "UNION ALL"])
    alias = rng.choice(["", "", " AS n"])
    query = f"SELECT a{alias}, b FROM t {op} SELECT a, c FROM u"
    if rng.random() < 0.4:
        query += f" {rng.choice(['UNION', 'UNION ALL'])} SELECT {texts[7]}, {ints[7]}"

    def term():
        x = rng.choice(["1"] * 8 + ["2"] * 5 + ["a", "b", "n", "t.a", "c", "u.a", "\"a\"", "[b]", "0", "3", "-1", "+1", "+2", "- -1", "-(-2)", "0x1", "0x02", "1.0", "'a'", "nosuch",
                        "a || ''", "1 + 1", "99999999999", "NULL", "upper(a)"])
        for _ in range(rng.choice([0, 0, 1, 1, 1, 2, 3])):
            wrap = rng.choice(["paren", "paren", "collate", "sign"])
            if wrap == "paren":
                x = f"({x})"
            elif wrap == "collate":
                x = f"{x} COLLATE {rng.choice(COLLATIONS)}"
            elif rng.random() < 0.3:
                x = f"+({x})"
        return x + rng.choice(["", "", "", " DESC", " ASC", " DESC NULLS LAST", " NULLS LAST"])

    query += " ORDER BY " + ", ".join(term() for _ in range(rng.choice([1, 1, 1, 2])))
    if rng.random() < 0.15:
        query += " LIMIT 3"
    return setup, [("ordered", query)]


def window(rng):
    n = rng.randint(3, 6)
    setup = ["CREATE TABLE t(a INTEGER PRIMARY KEY, g INT, v)", "CREATE TABLE s(b INT, w)",
             "INSERT INTO t VALUES " + ", ".join(f"({i}, {rng.randint(1, 2)}, {rng.choice(['1', '2', '2.5', 'NULL', chr(39) + 'x' + chr(39)])})" for i in range(1, n + 1)),
             "INSERT INTO s VALUES " + ", ".join(f"({rng.choice(['1', '2', '3', '5', 'NULL'])}, {rng.choice(['1', '10', chr(39) + 'y' + chr(39)])})" for _ in range(rng.randint(1, 4)))]
    if rng.random() < 0.3:
        setup.append("CREATE INDEX i ON s(b)")

    def sub():
        return rng.choice(["(SELECT max(b) FROM s)", "(SELECT min(b) FROM s WHERE b > t.a)", "(SELECT count(*) FROM s WHERE s.b <= t.g)", "t.a IN (SELECT b FROM s)",
                           "t.g NOT IN (SELECT b FROM s WHERE b IS NOT NULL)", "EXISTS (SELECT 1 FROM s WHERE b = t.a)", "NOT EXISTS (SELECT 1 FROM s WHERE b > t.a)",
                           "CASE WHEN t.a IN (SELECT b FROM s) THEN t.a ELSE 0 END", "coalesce((SELECT b FROM s WHERE b = t.a), -1)", "(SELECT count(*) FROM t AS x WHERE x.a < t.a) + t.a",
                           "(t.a, t.g) IN (SELECT b, 1 FROM s)", "(SELECT (SELECT max(b) FROM s) + t.a)", "t.v = (SELECT max(w) FROM s)", "(SELECT t.a)", "length((SELECT max(w) FROM s))",
                           "iif((SELECT count(*) FROM s) > 2, t.a, t.v)"])

    def bad():
        return rng.choice(["(SELECT 1, 2)", "t.a IN (SELECT b, w FROM s)", "(SELECT nosuch FROM s)", "(SELECT b, w FROM s) = 1"])

    def arg():
        return bad() if rng.random() < 0.02 else sub() if rng.random() < 0.8 else rng.choice(["t.a", "t.v", "t.a * 2"])

    def over():
        part = rng.choice(["", "", "PARTITION BY t.g ", f"PARTITION BY {sub()} "])
        order = rng.choice(["ORDER BY t.a", "ORDER BY t.a DESC", f"ORDER BY {sub()}, t.a"])
        frame = rng.choice(["", "", "", " ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING", " ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING", " ROWS 1 PRECEDING"])
        return f"({part}{order}{frame})"

    def aggregate():
        f = rng.choice(["sum", "count", "max", "min", "total", "avg", "group_concat", "json_group_array", "sum", "max"])
        args = arg() if f != "group_concat" or rng.random() < 0.5 else f"{arg()}, (SELECT 'x')"
        filt = f" FILTER (WHERE {sub()})" if rng.random() < 0.2 else ""
        return f"{f}({args}){filt}"

    def ranked():
        return rng.choice([f"lag({arg()})", f"lead({arg()}, 1)", f"lag({arg()}, 1, {arg()})", f"first_value({arg()})", f"last_value({arg()})", f"nth_value({arg()}, 2)"])

    used = []
    for _ in range(rng.choice([1, 1, 1, 2])):
        used.append(aggregate() if used or rng.random() < 0.75 else ranked())
    shape = rng.random()
    if shape < 0.12:
        calls = ", ".join(f"sum(sum({arg()})) OVER (ORDER BY t.g)" for _ in used)
        query = f"SELECT t.g, {calls} FROM t GROUP BY t.g"
    elif shape < 0.25 and len(used) == 1:
        query = f"SELECT t.a, {used[0]} OVER w FROM t WINDOW w AS {over()}"
    else:
        query = f"SELECT t.a, {', '.join(f'{u} OVER {over()}' for u in used)} FROM t"
        if rng.random() < 0.25:
            query += f" WHERE {sub()}"
    outer = rng.random()
    if outer < 0.1 and len(used) == 1:
        query = f"SELECT * FROM ({query})"
    elif outer < 0.18:
        query = f"SELECT count(*) FROM ({query}) AS q"
    elif outer < 0.25:
        query = f"{query} UNION ALL SELECT {', '.join(['0'] * (len(used) + 1))}"
    return setup, [("rows", query)]


def derived(rng):
    """A view's, a derived table's or a CTE's columns that fail on some row, most of them read by nothing."""
    bodies = rng.sample(["'{\"a\":1}'", "'{\"a\":\"x\"}'", "'not json'", "'[1,2]'", "NULL", "'{'"], rng.randint(2, 4))
    setup = ["CREATE TABLE doc(id INTEGER PRIMARY KEY, body TEXT, k INT)", "CREATE TABLE o(id INTEGER PRIMARY KEY, j INT)",
             "INSERT INTO doc VALUES " + ", ".join(f"({i}, {b}, {rng.choice(['1', '2', '3', 'NULL'])})" for i, b in enumerate(bodies, 1)),
             "INSERT INTO o VALUES " + ", ".join(f"({i}, {rng.randint(1, 3)})" for i in range(1, 5))]
    good = ["id", "k", "d", "e", "m"]
    texts = {"id": "id", "k": "k", "d": "k * 2 AS d", "e": "coalesce(k, 0) AS e", "m": "'m' || id AS m", "a": "json_extract(body, '$.a') AS a", "b": "json(body) AS b",
             "c": "abs(-9223372036854775808 + coalesce(k, 0) * 0) AS c", "n": "json_array_length(body) + 1 AS n", "q": "(SELECT json(body)) AS q"}
    bad = rng.sample(["a", "b", "c", "n", "q"], rng.choice([1, 1, 2]))
    cols = ["id"] + rng.sample(good[1:], rng.randint(1, 3)) + bad
    if rng.random() < 0.3:
        rng.shuffle(cols)
    body = f"SELECT {', '.join(texts[c] for c in cols)} FROM doc" + rng.choice(["", "", "", " WHERE id > 1", " WHERE k IS NOT NULL", " ORDER BY id", " ORDER BY k DESC, id", " ORDER BY id LIMIT 2"])
    shape = rng.random()
    if shape < 0.15 and "ORDER" not in body:
        body += f" UNION ALL SELECT {', '.join(texts[c] for c in cols)} FROM doc WHERE id < 3"
    names = [c for c in cols]
    for _ in range(rng.choice([0, 0, 0, 1, 2])):
        inner = f"({body})" if rng.random() < 0.5 else None
        if inner is None:
            setup.append(f"CREATE VIEW v{len(setup)} AS {body}")
            inner = f"v{len(setup) - 1}"
        names = rng.choice([names, names, [c for c in names if c == "id" or rng.random() < 0.8]])
        body = f"SELECT {'*' if rng.random() < 0.3 and len(names) == len(cols) else ', '.join(names)} FROM {inner}" + rng.choice(["", "", " WHERE id < 9"])
    how = rng.random()
    if how < 0.5:
        setup.append(f"CREATE VIEW v AS {body}")
        src, head = "v", ""
    elif how < 0.8:
        src, head = f"({body}) AS v", ""
    else:
        src, head = "v", f"WITH v AS ({body}) "
    reads = [c for c in names if c not in bad] or ["id"]
    c, d = rng.choice(reads), rng.choice(reads)
    if rng.random() < 0.08:
        c = rng.choice(names)
    once = [
        f"SELECT count(*) FROM {src}", f"SELECT {c} FROM {src}", f"SELECT v.{c}, v.{d} FROM {src}", f"SELECT {c}, count(*) FROM {src} GROUP BY {c}", f"SELECT max({c}), min({d}) FROM {src}",
        f"SELECT {c} FROM {src} WHERE id > 1", f"SELECT v.{c}, o.j FROM {src} JOIN o ON o.id = v.id", f"SELECT o.id, v.{c} FROM o LEFT JOIN {src} ON v.id = o.j",
        f"SELECT o.id FROM o WHERE o.id IN (SELECT id FROM {src})", f"SELECT o.id FROM o WHERE EXISTS (SELECT 1 FROM {src} WHERE v.id = o.id)", f"SELECT o.id, (SELECT max({c}) FROM {src}) FROM o",
        f"SELECT o.id, (SELECT count(*) FROM {src} WHERE v.id <= o.j) FROM o", f"SELECT DISTINCT {c} FROM {src}", f"SELECT {c} FROM {src} UNION SELECT j FROM o", f"SELECT {c} FROM {src} ORDER BY {d}, id",
        f"SELECT id, sum({c}) OVER (ORDER BY id) FROM {src}", f"SELECT * FROM (SELECT {c}, id FROM {src}) WHERE id < 3", f"SELECT {c} FROM {src} JOIN o USING (id)", f"SELECT o.j FROM o NATURAL JOIN (SELECT id, {c} AS zz FROM {src})",
        f"SELECT j FROM {src} JOIN o ON v.{c} = o.j", f"SELECT v.id FROM {src} RIGHT JOIN o ON o.id = v.id", f"SELECT o.id, v.{c} FROM o FULL JOIN {src} ON v.id = o.id", f"SELECT x.id FROM doc AS x, {src} WHERE x.id = v.id AND v.{c} IS NOT NULL",
        f"SELECT {c} FROM {src} LIMIT 2", f"SELECT id FROM {src} GROUP BY id HAVING max({c}) IS NOT NULL", f"SELECT CASE WHEN {c} IS NULL THEN {d} ELSE id END FROM {src}", f"SELECT rowid FROM o WHERE j NOT IN (SELECT {c} FROM {src} WHERE {c} IS NOT NULL)",
    ]
    return setup, [("rows" if c not in bad else "reads a failing column", head + rng.choice(once))]


def cell(v):
    if v is None:
        return "NULL"
    if isinstance(v, float):
        return "r" + struct.pack(">d", v).hex()
    if isinstance(v, bytes):
        return f"x'{v.hex()}'"
    return f"'{v}'" if isinstance(v, str) else str(v)


def main():
    count, seed = int(sys.argv[1]), int(sys.argv[2])
    rng = random.Random(seed)
    lines = []
    made = 0
    while made < count:
        setup, queries = (window, compound, derived)[made % 3](rng)
        conn = sqlite3.connect(":memory:", isolation_level=None)
        for sql in setup:
            conn.execute(sql)
        for kind, sql in queries:
            try:
                rows = [",".join(cell(v) for v in row) for row in conn.execute(sql).fetchall()]
                got = " / ".join(rows if kind == "ordered" else sorted(rows))
            except sqlite3.Error as error:
                got = f"refused: {error}"
            if made % 3 == 2 and kind == "rows" and got.startswith("refused"):
                continue  # SQLite evaluates a column nothing reads where it neither flattens nor prunes: no document says it does
            made += 1
            lines.append("case")
            lines.extend(f"sql\t{sql}" for sql in setup)
            lines.append(f"{'ordered' if kind == 'ordered' else 'rows'}\t{sql}\t{got}")
    print(f"# bench/sqlite_refused_queries.py {count} {seed}, SQLite {sqlite3.sqlite_version}: a case's statements, and each query's rows.")
    print("\n".join(lines))


main()
