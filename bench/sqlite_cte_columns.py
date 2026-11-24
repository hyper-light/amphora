"""AC-T1.8.6 (lane C16, the oracles' finding CU): a column of a recursive or ordinary CTE, a derived table
or a compound, made from a column of each type affinity and collation, a CAST, a literal or an
expression, and compared with a column of every type holding values of every kind (=, <, IN, join keys,
ORDER BY, GROUP BY), and the answers SQLite itself gives (the sqlite3 library Python links, printed
first). Writes crates/engine/tests/fixtures/cte_columns.tsv, which tests/cte_columns.rs holds Amphora to.

Each line is `sql <statement>` (the database's rows), `source <name> <expression>` (the column),
`shape <name> <query>` (@E@ the column's expression, @TAIL@ the rest), `tail <name> <text>` or
`q <shape> <source> <tail> <answer>`: the answer is `<rows>:<the FNV-1a 64 of the rows, each a line, its
values joined by commas, sorted but for a tail named ordered...>`. A `step` shape's tail is inside the
recursive arm; the other shapes' tails are around the CTE, the derived table or the compound.

The documents' exceptions, where they and SQLite 3.54.0 part or they leave the answer open:
- A column made of a scalar subquery. datatype3.html 3.2 gives an expression that is no column, CAST or
  COLLATE no affinity; SQLite gives a subquery its own column's. The document's answer is held to: SQLite
  is asked of the subquery inside a CASE, which has none in either (ASKED).
- A compound's column whose arms' affinities differ (the `mixed` shapes). datatype3.html 3.3.1: "it is
  indeterminate which of the SELECT statements will be used to determine affinity", and it "might be
  different at different times within the same query". Amphora combines the arms' affinities as SQLite
  does where SQLite reads the column, and the queries are kept but for two ways SQLite reads another:
  where it moves the WHERE into the arms, each comparing by its own affinity (told by SQLite's answer
  differing once the compound is MATERIALIZED), and where one arm alone has REAL affinity, whose column's
  integers SQLite makes REALs as it reads them.
- A UNION, EXCEPT or INTERSECT whose arms' collations differ (the oracles' CY): none is drawn.
- Which of a group's or a DISTINCT's equal values is returned (a NOCASE group of 'ABC' and 'abc'): the
  tails return counts and ids.
- A REAL's text (the oracles' CK, floatingpoint.html): the tails return ids, types and truths.
usage: python3 bench/sqlite_cte_columns.py > crates/engine/tests/fixtures/cte_columns.tsv
       python3 bench/sqlite_cte_columns.py --sql '<shape>' '<source>' '<tail>'   (the query, and SQLite's rows)"""
import collections
import random
import sqlite3
import sys

# name, declaration
COLUMNS = [("i", "INTEGER"), ("r", "REAL"), ("t", "TEXT"), ("n", "NUMERIC"), ("b", "BLOB"), ("x", ""),
           ("tn", "TEXT COLLATE NOCASE"), ("tr", "TEXT COLLATE RTRIM"), ("xn", "COLLATE NOCASE")]
NAMES = [name for name, _ in COLUMNS]
VALUES_S = ["NULL", "1", "2", "-1", "1.5", "'1'", "'2'", "'1.0'", "'1.5'", "' 1'", "'abc'", "'ABC'",
            "'abc '", "''", "x'31'", "x'616263'", "0", "'0x1'", "'1e0'"]
VALUES_O = ["NULL", "1", "2.0", "1.5", "'1'", "'01'", "'1.0'", "'abc'", "'Abc'", "'ABC  '", "x'31'", "''",
            "3", "'1.5'", "2"]


def filled():
    declared = ", ".join(f"{name} {declaration}".strip() for name, declaration in COLUMNS)
    out = []
    for table, values in [("s", VALUES_S), ("o", VALUES_O), ("u", VALUES_O)]:
        out.append(f"CREATE TABLE {table}(id INTEGER PRIMARY KEY, {declared})")
        rows = ", ".join("(" + ", ".join([str(at + 1)] + [value] * len(COLUMNS)) + ")"
                         for at, value in enumerate(values))
        out.append(f"INSERT INTO {table} VALUES {rows}")
    out += [f"CREATE INDEX o_{name} ON o({name})" for name in NAMES]
    return out


# The column under test, an expression over s.
SOURCES = {name: f"s.{name}" for name in NAMES}
SOURCES |= {
    "id": "s.id",
    "plus_i": "+s.i", "plus_t": "+s.t", "plus_tn": "+s.tn",
    "cast_int": "CAST(s.x AS INTEGER)", "cast_real": "CAST(s.x AS REAL)", "cast_text": "CAST(s.x AS TEXT)",
    "cast_num": "CAST(s.x AS NUMERIC)", "cast_blob": "CAST(s.x AS BLOB)",
    "cast_tn": "CAST(s.tn AS TEXT)",
    "one": "1", "one_text": "'1'", "real": "1.5", "blob": "x'31'", "null": "NULL", "word": "'abc'",
    "add": "s.i + 1", "concat": "s.t || ''", "lower": "lower(s.t)", "coalesce": "coalesce(s.i, s.t)",
    "case": "CASE WHEN s.id > 0 THEN s.t END", "negative": "-s.r", "compare": "s.i = 1",
    "t_nocase": "s.t COLLATE NOCASE", "x_nocase": "s.x COLLATE NOCASE", "word_nocase": "'abc' COLLATE NOCASE",
    "tn_binary": "s.tn COLLATE BINARY", "tn_rtrim": "s.tn COLLATE RTRIM", "paren": "(s.t)",
    "scalar": "(SELECT y.t FROM s AS y WHERE y.id = s.id)",
    "scalar_int": "(SELECT y.i FROM s AS y WHERE y.id = s.id)",
}
# What SQLite is asked of where the document's answer isn't its own.
ASKED = {name: f"CASE WHEN s.id > 0 THEN {SOURCES[name]} END" for name in ("scalar", "scalar_int")}
REALS = {"r", "cast_real"}  # the sources and the other arms with REAL affinity
# What a second arm holds that the first does not (the arms' affinities may differ).
OTHERS = {"i": "o.i", "r": "o.r", "t": "o.t", "n": "o.n", "b": "o.b", "x": "o.x", "tn": "o.tn", "one": "1",
          "word": "'abc'", "cast_text": "CAST(o.x AS TEXT)", "cast_int": "CAST(o.x AS INTEGER)", "null": "NULL",
          "add": "o.i + 1"}

LIMIT = "w.d < 1"
# Every shape gives w(id, a, d); the step shapes' tails are the recursive arm's FROM and WHERE.
SHAPES = {
    "cte": "WITH w(id, a, d) AS (SELECT s.id, @E@, 0 FROM s) @TAIL@",
    "cte_mat": "WITH w(id, a, d) AS MATERIALIZED (SELECT s.id, @E@, 0 FROM s) @TAIL@",
    "cte_not_mat": "WITH w(id, a, d) AS NOT MATERIALIZED (SELECT s.id, @E@, 0 FROM s) @TAIL@",
    "cte_of_cte": "WITH v(id, a) AS (SELECT s.id, @E@ FROM s), w(id, a, d) AS (SELECT id, a, 0 FROM v) @TAIL@",
    "derived": "@TAIL:(SELECT s.id, @E@ AS a, 0 AS d FROM s) AS w@",
    "derived_limit": "@TAIL:(SELECT s.id, @E@ AS a, 0 AS d FROM s ORDER BY s.id LIMIT 100) AS w@",
    "union_all": "@TAIL:(SELECT s.id, @E@ AS a, 0 AS d FROM s WHERE s.id < 9 UNION ALL "
                 "SELECT s.id, @E@, 0 FROM s WHERE s.id >= 9) AS w@",
    "union": "@TAIL:(SELECT s.id, @E@ AS a, 0 AS d FROM s UNION SELECT s.id, @E@, 0 FROM s) AS w@",
    "except": "@TAIL:(SELECT s.id, @E@ AS a, 0 AS d FROM s EXCEPT SELECT s.id, @E@, 0 FROM s WHERE s.id = 3) AS w@",
    "intersect": "@TAIL:(SELECT s.id, @E@ AS a, 0 AS d FROM s INTERSECT SELECT s.id, @E@, 0 FROM s) AS w@",
    "rec": f"WITH RECURSIVE w(id, a, d) AS (SELECT s.id, @E@, 0 FROM s UNION ALL "
           f"SELECT w.id + 100, w.a, w.d + 1 FROM w WHERE {LIMIT}) @TAIL@",
    "rec_mat": f"WITH RECURSIVE w(id, a, d) AS MATERIALIZED (SELECT s.id, @E@, 0 FROM s UNION ALL "
               f"SELECT w.id + 100, w.a, w.d + 1 FROM w WHERE {LIMIT}) @TAIL@",
    "rec_union": f"WITH RECURSIVE w(id, a, d) AS (SELECT s.id, @E@, 0 FROM s UNION "
                 f"SELECT w.id + 100, w.a, w.d + 1 FROM w WHERE {LIMIT}) @TAIL@",
    "rec_column_order": f"WITH RECURSIVE w(id, d, a) AS (SELECT s.id, 0, @E@ FROM s UNION ALL "
                        f"SELECT w.id + 100, w.d + 1, w.a FROM w WHERE {LIMIT}) @TAIL@",
    "rec_two_anchors": f"WITH RECURSIVE w(id, a, d) AS (SELECT s.id, @E@, 0 FROM s WHERE s.id < 9 UNION ALL "
                       f"SELECT s.id, @E@, 0 FROM s WHERE s.id >= 9 UNION ALL "
                       f"SELECT w.id + 100, w.a, w.d + 1 FROM w WHERE {LIMIT}) @TAIL@",
    "step": f"WITH RECURSIVE w(id, a, d) AS (SELECT s.id, @E@, 0 FROM s UNION ALL "
            f"SELECT w.id * 100 + o.id, w.a, w.d + 1 FROM w JOIN o ON @TAIL@ WHERE {LIMIT}) "
            f"SELECT id, d, typeof(a) FROM w",
    "step_scan": f"WITH RECURSIVE w(id, a, d) AS (SELECT s.id, @E@, 0 FROM s UNION ALL "
                 f"SELECT w.id * 100 + o.id, w.a, w.d + 1 FROM u AS o JOIN w ON @TAIL@ WHERE {LIMIT}) "
                 f"SELECT id, d, typeof(a) FROM w",
    "step_left": f"WITH RECURSIVE w(id, a, d) AS (SELECT s.id, @E@, 0 FROM s UNION ALL "
                 f"SELECT w.id * 100 + coalesce(o.id, 0), w.a, w.d + 1 FROM w LEFT JOIN o ON @TAIL@ WHERE {LIMIT}) "
                 f"SELECT id, d, typeof(a) FROM w",
    "step_where": f"WITH RECURSIVE w(id, a, d) AS (SELECT s.id, @E@, 0 FROM s UNION ALL "
                  f"SELECT w.id * 100 + o.id, w.a, w.d + 1 FROM w, o WHERE @TAIL@ AND {LIMIT}) "
                  f"SELECT id, d, typeof(a) FROM w",
    "step_exists": f"WITH RECURSIVE w(id, a, d) AS (SELECT s.id, @E@, 0 FROM s UNION ALL "
                   f"SELECT w.id + 100, w.a, w.d + 1 FROM w WHERE {LIMIT} AND EXISTS "
                   f"(SELECT 1 FROM o WHERE @TAIL@)) SELECT id, d, typeof(a) FROM w",
    "step_case": f"WITH RECURSIVE w(id, a, d, c) AS (SELECT s.id, @E@, 0, 0 FROM s UNION ALL "
                 f"SELECT w.id * 100 + o.id, w.a, w.d + 1, CASE WHEN @TAIL@ THEN 1 ELSE 2 END FROM w, o WHERE {LIMIT}) "
                 f"SELECT id, d, c FROM w",
    "step_union": f"WITH RECURSIVE w(id, a, d) AS (SELECT s.id, @E@, 0 FROM s UNION "
                  f"SELECT w.id * 100 + o.id, w.a, w.d + 1 FROM w JOIN o ON @TAIL@ WHERE {LIMIT}) "
                  f"SELECT id, d, typeof(a) FROM w",
}
STEPS = [name for name in SHAPES if name.startswith("step")]
OUTER = [name for name in SHAPES if not name.startswith("step")]

LITERALS = {"one": "1", "one_text": "'1'", "real": "1.5", "word": "'abc'", "upper": "'ABC'", "blob": "x'31'",
            "one_real_text": "'1.0'", "empty": "''"}
COMPARED = {name: f"o.{name}" for name in NAMES} | {"id": "o.id"}


def step_tails():
    """Comparisons of w.a with a column of o or a literal."""
    out = {}
    for name, other in COMPARED.items():
        out[f"eq_{name}"] = f"{other} = w.a"
        out[f"rev_eq_{name}"] = f"w.a = {other}"
        out[f"lt_{name}"] = f"w.a < {other}"
        out[f"ge_{name}"] = f"{other} >= w.a AND o.id < 9"
        out[f"is_{name}"] = f"w.a IS {other}"
        out[f"between_{name}"] = f"w.a BETWEEN {other} AND {other}"
        out[f"in_{name}"] = f"w.a IN (SELECT y.{name} FROM o AS y WHERE y.id < 9) AND o.id = 1"
        out[f"not_in_{name}"] = f"w.a NOT IN (SELECT y.{name} FROM o AS y WHERE y.id BETWEEN 2 AND 9) AND o.id = 1"
        out[f"in_list_{name}"] = f"{other} IN (w.a, 3)"
        out[f"in_list2_{name}"] = f"w.a IN ({other}, 3)"
        out[f"plus_{name}"] = f"+w.a = {other}"
        out[f"both_{name}"] = f"{other} = w.a AND o.id = w.id"
        out[f"or_{name}"] = f"({other} = w.a OR o.id = w.id)"
        out[f"eq_plus_{name}"] = f"+{other} = w.a"
        out[f"scalar_{name}"] = f"o.id = (SELECT min(y.id) FROM o AS y WHERE y.{name} = w.a)"
    for name, other in LITERALS.items():
        out[f"lit_eq_{name}"] = f"w.a = {other} AND o.id < 3"
        out[f"lit_lt_{name}"] = f"w.a < {other} AND o.id < 3"
        out[f"lit_in_{name}"] = f"w.a IN ({other}, 3) AND o.id < 3"
    out["nocase"] = "o.t = w.a COLLATE NOCASE"
    out["row"] = "(o.t, o.id) = (w.a, w.id)"
    return out


def outer_tails():
    out = {}
    for name, other in COMPARED.items():
        out[f"join_{name}"] = f"SELECT w.id, o.id FROM @W@ JOIN o ON o.{name} = w.a"
        out[f"join_scan_{name}"] = f"SELECT w.id, o.id FROM @W@ JOIN u AS o ON w.a = o.{name}"
        out[f"left_{name}"] = f"SELECT w.id, o.id FROM @W@ LEFT JOIN o ON o.{name} = w.a"
        out[f"left_from_{name}"] = f"SELECT w.id, o.id FROM u AS o LEFT JOIN @W@ ON o.{name} = w.a"
        out[f"lt_{name}"] = f"SELECT w.id, o.id FROM @W@ JOIN o ON w.a < o.{name}"
        out[f"where_{name}"] = f"SELECT w.id, o.id FROM @W@, o WHERE o.{name} = w.a AND o.id < 12"
        out[f"is_{name}"] = f"SELECT w.id, o.id FROM @W@, o WHERE o.{name} IS w.a"
        out[f"in_{name}"] = f"SELECT w.id, w.a IN (SELECT o.{name} FROM o WHERE o.id < 9) FROM @W@"
        out[f"of_{name}"] = f"SELECT o.id FROM o WHERE o.{name} IN (SELECT w.a FROM @W@)"
        out[f"not_of_{name}"] = f"SELECT o.id FROM o WHERE o.{name} NOT IN (SELECT w.a FROM @W@ WHERE w.a NOT NULL)"
        out[f"exists_{name}"] = f"SELECT o.id FROM o WHERE EXISTS (SELECT 1 FROM @W@ WHERE w.a = o.{name})"
        out[f"case_{name}"] = f"SELECT w.id, o.id, w.a = o.{name}, w.a < o.{name} FROM @W@, o"
        out[f"row_{name}"] = f"SELECT w.id, o.id FROM @W@, o WHERE (w.a, w.id) = (o.{name}, o.id)"
    for name, other in LITERALS.items():
        out[f"lit_{name}"] = f"SELECT w.id FROM @W@ WHERE w.a = {other}"
        out[f"lit_lt_{name}"] = f"SELECT w.id FROM @W@ WHERE w.a < {other}"
        out[f"lit_in_{name}"] = f"SELECT w.id FROM @W@ WHERE w.a IN ({other}, 3)"
        out[f"lit_case_{name}"] = f"SELECT w.id, w.a = {other}, {other} > w.a, w.a IS {other} FROM @W@"
    out["ordered"] = "SELECT w.id FROM @W@ ORDER BY w.a, w.id"
    out["ordered_desc"] = "SELECT w.id FROM @W@ ORDER BY w.a DESC, w.id"
    out["group"] = "SELECT count(*), min(w.id), max(w.id) FROM @W@ GROUP BY w.a"
    out["group_having"] = "SELECT count(*), min(w.id) FROM @W@ GROUP BY w.a HAVING w.a = '1' OR w.a = 1"
    out["distinct"] = "SELECT count(*) FROM (SELECT DISTINCT w.a FROM @W@)"
    out["count_distinct"] = "SELECT count(DISTINCT w.a) FROM @W@"
    out["window"] = "SELECT w.id, count(*) OVER (PARTITION BY w.a) FROM @W@"
    out["rank"] = "SELECT w.id, rank() OVER (ORDER BY w.a) FROM @W@"
    out["max"] = "SELECT w.id FROM @W@ WHERE w.a = (SELECT max(y.t) FROM o AS y)"
    out["types"] = "SELECT w.id, typeof(w.a) FROM @W@"
    out["self"] = "SELECT v1.id, w.id FROM (SELECT o.id, o.t AS a FROM o) AS v1 JOIN @W@ ON v1.a = w.a"
    return out


def joined(shape, source, tail):
    text = SHAPES[shape]
    if "@TAIL:" in text:
        at = text.index("@TAIL:")
        text = tail.replace("@W@", text[at + len("@TAIL:"):-1])
    else:
        text = text.replace("@TAIL@", tail.replace("@W@", "w"))
    return text.replace("@E@", source)


# Arms whose affinities differ: datatype3.html 3.3.1 leaves open which arm's a compound's column has, and
# Amphora types the column as SQLite 3.54.0 does.
for other, text in OTHERS.items():
    SHAPES[f"mixed_all_{other}"] = (f"@TAIL:(SELECT s.id, @E@ AS a, 0 AS d FROM s UNION ALL "
                                    f"SELECT o.id + 100, {text}, 0 FROM o) AS w@")
    SHAPES[f"mixed_rec_{other}"] = (f"WITH RECURSIVE w(id, a, d) AS (SELECT s.id, @E@, 0 FROM s UNION ALL "
                                    f"SELECT w.id * 100 + o.id, {text}, w.d + 1 FROM w, o WHERE {LIMIT} AND o.id < 6) @TAIL@")
    SHAPES[f"mixed_step_{other}"] = (f"WITH RECURSIVE w(id, a, d) AS (SELECT s.id, @E@, 0 FROM s UNION ALL "
                                     f"SELECT w.id * 100 + o.id, {text}, w.d + 1 FROM w JOIN o ON @TAIL@ WHERE {LIMIT}) "
                                     f"SELECT id, d, typeof(a) FROM w")
MIXED = [name for name in SHAPES if name.startswith("mixed")]


def fnv(rows):
    total = 0xCBF29CE484222325
    for byte in "\n".join(rows).encode():
        total = ((total ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return f"{total:016x}"


def answered(db, sql, ordered):
    rows = [",".join("NULL" if v is None else str(v) for v in row) for row in db.execute(sql)]
    return rows if ordered else sorted(rows)


def opened():
    db = sqlite3.connect(":memory:")
    for sql in filled():
        db.execute(sql)
    return db


def drawn():
    """Every source under every shape, each with a draw of the tails; every tail under each shape at least
    once."""
    rng = random.Random("amphora cte columns")
    steps, outers = step_tails(), outer_tails()
    seen = set()
    for shapes, tails, count in [(STEPS, steps, 7), (OUTER, outers, 4)]:
        for shape in shapes:
            names = list(tails)
            for source in SOURCES:
                for tail in rng.sample(names, count):
                    seen.add((shape, source, tail))
            for tail in names:
                seen.add((shape, rng.choice(list(SOURCES)), tail))
    for shape in MIXED:
        tails = steps if shape.startswith("mixed_step") else outers
        for source in SOURCES:
            for tail in rng.sample(list(tails), 1):
                seen.add((shape, source, tail))
    return sorted(seen), steps, outers


def text_of(shape, source, tail, steps, outers):
    """The query Amphora answers, and the one SQLite is asked."""
    tails = steps if shape in STEPS or shape.startswith("mixed_step") else outers
    return (joined(shape, SOURCES[source], tails[tail]),
            joined(shape, ASKED.get(source, SOURCES[source]), tails[tail]))


def materialized(shape, source, tail, outers):
    """The query with its compound a MATERIALIZED CTE."""
    compound = SHAPES[shape].removeprefix("@TAIL:").removesuffix(" AS w@").replace("@E@", SOURCES[source])
    return f"WITH w AS MATERIALIZED {compound} {outers[tail].replace('@W@', 'w')}"


def open_to_choice(db, shape, source, tail, outers):
    """Whether 3.3.1 leaves the answer open in a way SQLite's own answer shows."""
    if not shape.startswith("mixed") or source in ASKED:
        return source in ASKED and shape.startswith("mixed")
    if (source in REALS) != (shape.rsplit("_", 1)[1] in REALS):
        return True
    if not shape.startswith("mixed_all"):
        return False
    ordered = tail.startswith("ordered")
    sql = joined(shape, SOURCES[source], outers[tail])
    return answered(db, sql, ordered) != answered(db, materialized(shape, source, tail, outers), ordered)


def main():
    db = opened()
    print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_cte_columns.py")
    for sql in filled():
        print(f"sql\t{sql}")
    for name, text in SOURCES.items():
        print(f"source\t{name}\t{text}")
    for name, text in SHAPES.items():
        print(f"shape\t{name}\t{text}")
    queries, steps, outers = drawn()
    for kind, tails in [("step", steps), ("outer", outers)]:
        for name, text in tails.items():
            print(f"tail\t{kind}\t{name}\t{text}")
    count = collections.Counter()
    for shape, source, tail in queries:
        if open_to_choice(db, shape, source, tail, outers):
            count["open"] += 1
            continue
        got = answered(db, text_of(shape, source, tail, steps, outers)[1], tail.startswith("ordered"))
        count["kept"] += 1
        count["empty"] += not got
        count["asked"] += source in ASKED
        print(f"q\t{shape}\t{source}\t{tail}\t{len(got)}:{fnv(got)}")
    print(f"# {count['kept']} queries, {count['empty']} of no row; {count['asked']} of a scalar subquery, the document's "
          f"answer; {count['open']} drawn and left out, open by 3.3.1")


if sys.argv[1:2] == ["--sql"]:
    sql, asked = text_of(*sys.argv[2:5], step_tails(), outer_tails())
    print(sql)
    for line in answered(opened(), asked, sys.argv[4].startswith("ordered")):
        print(line)
else:
    main()
