#!/usr/bin/env python3
"""What SQLite 3.54.0 answers for compounds whose arms' columns have collations of their own (the oracles' finding CY): the fixture of
crates/engine/tests/compound_collations.rs. Two arms of every kind under every operator, three arms, an ORDER BY over a UNION ALL, and a compound in a
FROM clause, a CTE, an IN and an INSERT. Which of several equal texts a compound keeps no page says, so a value is written folded (lower case, no
trailing spaces) and unordered rows sorted. What SQLite does otherwise than lang_select.html is left out here, some held by the page in the test: the
operators group from the left, where SQLite gives an EXCEPT or INTERSECT of two arms with no collation that of a later arm (`certain`) and merges a UNION
of four arms or more two by two; a VALUES of several rows means the SELECTs under UNION ALL, whose literals have no collation, where SQLite reads its
rows as columns; and an INTEGER PRIMARY KEY is a column, where SQLite's rowid has no collation.

usage: bench/sqlite_compound_collations.py [two-arm draws] [three-arm draws] [seed] > crates/engine/tests/fixtures/compound_collations.tsv"""
import itertools, random, sqlite3, sys

assert sqlite3.sqlite_version == "3.54.0", sqlite3.sqlite_version
two, three, seed = (int(a) for a in (sys.argv[1:] + ["700", "320", "27"][len(sys.argv) - 1 :]))
rng = random.Random(seed)
FILL = [
    "CREATE TABLE u(a TEXT)", "CREATE TABLE t(a TEXT COLLATE NOCASE)", "CREATE TABLE r(a TEXT COLLATE RTRIM)", "CREATE TABLE b(a TEXT COLLATE BINARY)",
    "CREATE TABLE n(a)", "CREATE TABLE e(a TEXT COLLATE NOCASE)", "CREATE TABLE k(a TEXT COLLATE NOCASE PRIMARY KEY) WITHOUT ROWID", "CREATE TABLE o(a TEXT COLLATE NOCASE)",
    "INSERT INTO u VALUES ('ABC'), ('abc'), ('x'), ('q '), (NULL)", "INSERT INTO t VALUES ('Q'), ('q'), ('ABC'), ('m ')", "INSERT INTO r VALUES ('m'), ('m '), ('abc'), ('ABC ')",
    "INSERT INTO b VALUES ('aBC'), ('Q'), ('M')", "INSERT INTO n VALUES ('X'), ('abc'), (7)", "INSERT INTO k VALUES ('Abc'), ('Q'), ('z')",
    "CREATE VIEW v AS SELECT a FROM t", "CREATE VIEW f AS SELECT lower(a) AS a FROM r",
]
# an arm, and whether its column has a collation (a COLLATE, or a column, a BINARY one among them)
ARMS = [(f"SELECT a FROM {x}", True) for x in "utrbnekvf"]
ARMS += [(f"SELECT {e} FROM {x}", True) for x in "utr" for e in ("+a", "(a)", "CAST(a AS TEXT)")]
ARMS += [(f"SELECT {e} FROM {x}", False) for x in "utr" for e in ("a || ''", "lower(a)", "max(a)", "coalesce(a, 'Z')")]
ARMS += [("SELECT 'abc'", False), ("SELECT 'Q'", False), ("SELECT NULL", False), ("VALUES ('abc')", False), ("SELECT 'm '", False), ("SELECT 7", False)]
ARMS += [("SELECT a COLLATE NOCASE FROM u", True), ("SELECT a COLLATE BINARY FROM t", True), ("SELECT a COLLATE RTRIM FROM b", True), ("SELECT 'abc' COLLATE NOCASE", True),
         ("SELECT lower(a COLLATE NOCASE) FROM u", True), ("SELECT 'm ' COLLATE RTRIM", True), ("VALUES ('abc' COLLATE NOCASE)", True), ("SELECT (a COLLATE NOCASE) || '' FROM u", True)]
ARMS += [("SELECT (SELECT max(a) FROM t)", False), ("SELECT x FROM (SELECT a AS x FROM t)", True), ("SELECT x FROM (SELECT lower(a) AS x FROM t)", True), ("SELECT x FROM (SELECT a AS x FROM u LIMIT 9)", True),
         ("SELECT a FROM u GROUP BY a", True), ("SELECT DISTINCT a FROM t", True), ("SELECT t.a FROM t JOIN u ON t.a = u.a", True), ("SELECT u.a FROM u LEFT JOIN t ON t.a = u.a", True),
         ("SELECT a FROM u LEFT JOIN t USING (a)", True), ("SELECT first_value(a) OVER (ORDER BY a) FROM t", False), ("SELECT CASE WHEN a = 'q' THEN a ELSE a END FROM t", False), ("SELECT a FROM t WHERE a IN (SELECT a FROM u)", True)]
OPS = ["UNION", "EXCEPT", "INTERSECT"]

con = sqlite3.connect(":memory:")
for s in FILL:
    con.execute(s)

def write(v):
    if v is None:
        return "NULL"
    return v.rstrip(" ").lower() if isinstance(v, str) else str(v)

def answer(sql, ordered=False, dml=False):
    if dml:
        con.execute("DELETE FROM o")
        con.execute(sql)
        got = con.execute("SELECT a FROM o").fetchall()
    else:
        got = con.execute(sql).fetchall()
    rows = [",".join(write(v) for v in r) for r in got]
    return f"{len(rows)}:" + ";".join(rows if ordered else sorted(rows))

def certain(arms, ops):
    """not two arms with no collation told apart by EXCEPT or INTERSECT before an arm that has one"""
    for i in range(1, len(arms)):
        if any(a[1] for a in arms[: i + 1]):
            return True
        if ops[i - 1] in ("EXCEPT", "INTERSECT") and any(a[1] for a in arms[i + 1 :]):
            return False
    return True

def compound(arms, ops):
    return arms[0][0] + "".join(f" {op} {arm[0]}" for op, arm in zip(ops, arms[1:]))

out, seen = [], set()
def add(kind, sql, ordered=False, dml=False):
    if sql in seen:
        return
    seen.add(sql)
    out.append("\t".join([kind, "ordered" if ordered else "dml" if dml else "rows", sql, answer(sql, ordered, dml)]))

def plain(a):  # an arm that ends a compound may not be a bare VALUES list's neighbour in an ORDER BY
    return not a[0].startswith("VALUES")

pairs = list(itertools.product(range(len(ARMS)), range(len(ARMS)), OPS))
rng.shuffle(pairs)
# the plainest first: every two columns under every operator
for x, y in itertools.product("utrbnek", repeat=2):
    for op in OPS:
        add("columns", f"SELECT a FROM {x} {op} SELECT a FROM {y}")
for x, y, op in pairs[:two]:
    add("two", compound([ARMS[x], ARMS[y]], [op]))
while len(out) < two + three:
    k = 3
    arms = [rng.choice(ARMS) for _ in range(k)]
    ops = [rng.choice(OPS + ["UNION", "UNION ALL"]) for _ in range(k - 1)]
    if certain(arms, ops):
        add("three", compound(arms, ops))
for _ in range(three // 2):
    k = rng.choice([2, 3])
    arms = [rng.choice([a for a in ARMS if plain(a)]) for _ in range(k)]
    add("order by", compound(arms, ["UNION ALL"] * (k - 1)) + " ORDER BY 1", ordered=True)
for _ in range(three):
    arms, ops = [rng.choice(ARMS), rng.choice(ARMS)], [rng.choice(OPS)]
    c = compound(arms, ops)
    ctx = rng.choice(["from", "cte", "count", "in", "insert", "not in"])
    if ctx == "from":
        add("from", f"SELECT * FROM ({c}) AS s")
    elif ctx == "cte":
        add("cte", f"WITH c(z) AS ({c}) SELECT z FROM c")
    elif ctx == "count":
        add("count", f"SELECT count(*) FROM ({c}) AS s")
    elif ctx == "insert":
        add("insert", f"INSERT INTO o {c}", dml=True)
    else:
        # an IN's own collation is the left side's: a NOCASE one finds whichever of the equal texts is kept
        add(ctx, f"SELECT a, (a COLLATE NOCASE) {ctx.upper()} ({c}) FROM u")
# an IN's own collation, its left side's, is not the compound's
for x, y in itertools.permutations("utr", 2):
    for op in OPS:
        add("in columns", f"SELECT a, (a COLLATE NOCASE) IN (SELECT a FROM {x} {op} SELECT a FROM {y}) FROM n")
# a recursive CTE tells its rows apart as its arms' compound does
for x, y in itertools.product("utrbk", repeat=2):
    add("recursive", f"WITH RECURSIVE c(z) AS (SELECT a FROM {x} UNION SELECT s.a FROM c JOIN {y} AS s ON s.a = c.z COLLATE NOCASE) SELECT z FROM c")
    add("recursive", f"WITH RECURSIVE c(z, d) AS (SELECT a, 0 FROM {x} UNION SELECT s.a, 0 FROM c, {y} AS s) SELECT z, d FROM c")
    add("recursive", f"WITH RECURSIVE c(z, d) AS (SELECT 'abc', 0 UNION SELECT s.a, d + 1 FROM c, {y} AS s WHERE d < 1 UNION SELECT s.a, d + 1 FROM c, {x} AS s WHERE d < 1) SELECT z, d FROM c")
print("\n".join(out))
print(f"{len(out)} queries", file=sys.stderr)
