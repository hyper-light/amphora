"""AC-T1.8.6 (lane C15, the oracles' finding CT): comparisons of row values, `=` to `IS NOT`, BETWEEN, an
IN list and an IN subquery, each negated too, whose fields are columns of every type and collation,
literals, calls, subqueries and NULLs on both sides, as a value, in a WHERE, in an ON and in a correlated
subquery, over tables with indexes the row can use and without, and the answers SQLite itself gives (the
sqlite3 library Python links, printed first). Writes crates/engine/tests/fixtures/row_values.tsv, which
tests/row_values.rs holds Amphora to.

Each line is `db <name>`, `sql <statement>` (the database's rows) or `q <kind> <query> <answer>`: the kind
is the comparison, where it stands and the forms of LEFT_OUT it has; the answer is `<rows>:<the FNV-1a 64 of
the rows, sorted, each a line, its values joined by commas>`. A query SQLite refuses is not written, nor one
it answers otherwise without the indexes (a row IN a list it reads from a `r DESC, i` index, at times).
usage: python3 bench/sqlite_row_values.py [queries a database] [seed] [all] > crates/engine/tests/fixtures/row_values.tsv
           (all: the forms of LEFT_OUT too, which Amphora doesn't answer as SQLite yet)
       python3 bench/sqlite_row_values.py --rows '<database>' '<query>'   (SQLite's rows of one query)"""
import collections
import random
import sqlite3
import sys

COLUMNS = {"i": "INT", "r": "REAL", "x": "TEXT", "n": "NUMERIC", "b": "BLOB", "u": "",
           "c": "TEXT COLLATE NOCASE", "d": "TEXT COLLATE RTRIM"}
INTS, REALS = ["-1", "0", "1", "2", "3", "7"], ["-0.5", "1.0", "1.5", "2.0", "7.0"]
TEXTS = ["''", "'1'", "'2'", "'1.0'", "'a'", "'A'", "'a '", "'b'", "'B '", "'7'"]
BLOBS = ["x''", "x'31'", "x'61'", "x'62'"]
POOLS = {"i": INTS, "r": REALS + INTS, "x": TEXTS, "n": INTS + REALS + TEXTS, "b": BLOBS + TEXTS,
         "u": INTS + REALS + TEXTS + BLOBS, "c": TEXTS, "d": TEXTS}
INDEXES = ["i, x", "x, i, c", "c, i", "n, r", "u, b", "d, x", "r DESC, i"]

# The forms Amphora doesn't answer as SQLite yet, each a cause of its own
# (bench/records/2026-11-24-adacachy2-perf-expr-c15.md): none is in the fixture until it does.
LEFT_OUT = {
    "between as a value": "refused: insufficient registers (the oracles' finding CM)",
    "collation": "a BINARY column before one of another collation, which compares by the other's",
    "row subquery": "a field beside a row subquery's, which loses the subquery's affinity and collation",
    "list rows": "a list of rows, not all literals: SQLite compares each by the affinity of one of them",
    "list subquery": "a scalar subquery in an IN subquery's columns, which loses its affinity",
    "two tables": "a range of a row of two tables' columns in a join (the oracles' finding DB)",
}

# A field of a row: its SQL, the column it is (under + or CAST too), whether it has its own COLLATE, and
# whether it is a literal.
Field = collections.namedtuple("Field", "sql column collate literal", defaults=(None, False, False))


def fill(rng, name, rows):
    """`rows` rows, each column's values of its own pool or, at times, of any other's, and one in five NULL."""
    out = []
    for at in range(1, rows + 1):
        values = [str(at)]
        for column in COLUMNS:
            pool = POOLS[column if rng.random() < 0.85 else "u"]
            values.append("NULL" if rng.random() < 0.2 else rng.choice(pool))
        out.append(f"({', '.join(values)})")
    return f"INSERT OR IGNORE INTO {name} VALUES {', '.join(out)}"


def tables(rng, kind):
    """t and s, which `kind` says how to index: not at all, by seven indexes of rows, or s by its key."""
    columns = ", ".join(f"{name} {typed}".rstrip() for name, typed in COLUMNS.items())
    out = [f"CREATE TABLE t(id INTEGER PRIMARY KEY, {columns})"]
    if kind == "keyed":
        out.append(f"CREATE TABLE s(id INT, {columns}, PRIMARY KEY (x, i), UNIQUE (c, n)) WITHOUT ROWID")
        out.append("CREATE UNIQUE INDEX t_u ON t(u, i, id)")
        out.append("CREATE INDEX t_ix ON t(i, x) WHERE i IS NOT NULL")
    else:
        out.append(f"CREATE TABLE s(id INTEGER PRIMARY KEY, {columns})")
    if kind == "indexed":
        for name in "ts":
            for at, indexed in enumerate(INDEXES):
                out.append(f"CREATE INDEX {name}_{at} ON {name}({indexed})")
    return out + [fill(rng, "t", 24), fill(rng, "s", 14)]


class Draw:
    """One query, drawn part by part."""

    def __init__(self, rng):
        self.rng, self.n, self.forms = rng, 0, set()

    def literal(self, column):
        rng = self.rng
        if rng.random() < 0.2:
            return "NULL"
        return rng.choice(POOLS[column if rng.random() < 0.85 else "u"])

    def field(self, column, aliases, plain=False):
        """A field that compares with `column`: a column of one of `aliases`, or written over one; a literal."""
        rng = self.rng
        kind = rng.random()
        if not aliases or kind < 0.22:
            return Field(self.literal(column), literal=True)
        alias = rng.choice(aliases)
        at = f"{alias}.{column}"
        if plain or kind < 0.78:
            return Field(at, column)
        self.n += 1
        m = f"m{self.n}"
        return rng.choice([
            Field(f"+{at}", column), Field(f"({at} COLLATE NOCASE)", collate=True),
            Field(f"({at} COLLATE BINARY)", collate=True), Field(f"CAST({at} AS TEXT)", column),
            Field(f"CAST({at} AS INTEGER)", column), Field(f"lower({at})"),
            Field(f"coalesce({at}, {self.literal(column)})"), Field(f"({at} || '')"), Field(f"({at} + 0)"),
            Field(f"(SELECT max({m}.{column}) FROM s {m})"),
            Field(f"(SELECT {m}.{column} FROM s {m} WHERE {m}.id = {alias}.id)"),
        ])

    def columns(self, count):
        """The columns a row names: at times an index's own, in its order."""
        rng = self.rng
        if rng.random() < 0.4:
            named = [part.split()[0] for part in rng.choice(INDEXES).split(", ")]
            if len(named) >= count:
                return named[:count]
        return [rng.choice(list(COLUMNS)) for _ in range(count)]

    def row(self, columns, aliases):
        rng = self.rng
        # a row's fields are of one table mostly, as an index serves it
        if len(aliases) > 1 and rng.random() < 0.6:
            aliases = [rng.choice(aliases)]
        return [self.field(column if rng.random() < 0.85 else rng.choice(list(COLUMNS)), aliases) for column in columns]

    def collations(self, left, right):
        """Notes a field of `left` that is a BINARY column beside one of `right` of another collation."""
        for one, other in zip(left, right):
            if one.column in set(COLUMNS) - {"c", "d"} and other.column in {"c", "d"} and not (one.collate or other.collate):
                self.forms.add("collation")

    def listed(self, columns, aliases, not_):
        """`(...) [NOT] IN (SELECT ...)`: the list's rows of s or t, all or some, correlated or not."""
        rng = self.rng
        self.n += 1
        q = f"q{self.n}"
        fields = [self.field(column, [q]) if rng.random() < 0.3 else Field(f"{q}.{column}", column) for column in columns]
        if any("SELECT" in field.sql for field in fields):
            self.forms.add("list subquery")
        where = []
        if rng.random() < 0.35:
            column = rng.choice(list(COLUMNS))
            where.append(f"{q}.{column} {rng.choice(['=', '<', '>', '<>'])} {self.literal(column)}")
        if aliases and rng.random() < 0.35:
            column = rng.choice(list(COLUMNS))
            where.append(f"{q}.{column} {rng.choice(['=', '<', '>=', 'IS'])} {rng.choice(aliases)}.{column}")
        of = rng.choice([f"s {q}", f"s {q}", f"t {q}"])
        tail = rng.choice(["", "", "", " UNION ALL SELECT " + ", ".join(self.literal(column) for column in columns),
                           f" ORDER BY {q}.id LIMIT {rng.choice([0, 1, 3])}"])
        return (f"{not_}IN (SELECT {written(fields)} FROM {of}"
                f"{' WHERE ' + ' AND '.join(where) if where else ''}{tail})")

    def comparison(self, aliases, inner=()):
        """A comparison of two rows or more: its left side of `aliases`, its right of them or of `inner`."""
        rng = self.rng
        right = list(inner or aliases) if rng.random() < 0.65 else []
        of = list(aliases) + list(inner) if inner and rng.random() < 0.3 else list(aliases)
        if rng.random() < 0.3:
            # an index's own columns, as it can serve them
            columns, alias = [part.split()[0] for part in rng.choice(INDEXES).split(", ")], rng.choice(aliases)
            left = [Field(f"{alias}.{column}", column) for column in columns]
        else:
            columns = self.columns(rng.choice([2, 2, 2, 3, 3]))
            left = self.row(columns, of if rng.random() < 0.93 else [])
        kind = rng.random()
        spans = len({field.sql.split(".")[0].strip("(+") for field in left if field.column}) > 1
        if kind < 0.34:
            op = rng.choice(["=", "=", "<>", "<", "<=", ">", ">=", "IS", "IS NOT"])
            if rng.random() < 0.12:
                self.n += 1
                q = f"q{self.n}"
                if any(field.column != column or field.sql.startswith(("+", "CAST")) for field, column in zip(left, columns)):
                    self.forms.add("row subquery")
                fields = ", ".join(f"{q}.{column}" for column in columns)
                return (f"{op} (SELECT)", f"({written(left)}) {op} (SELECT {fields} FROM s {q} "
                                          f"WHERE {q}.id = {rng.randrange(1, 16)})")
            if spans and op in ("<", "<=", ">", ">="):
                self.forms.add("two tables")
            other = self.row(columns, right)
            self.collations(left, other)
            return op, f"({written(left)}) {op} ({written(other)})"
        if kind < 0.50:
            not_ = rng.choice(["", "", "NOT "])
            low, high = self.row(columns, right), self.row(columns, right)
            self.collations(left, low)
            self.collations(left, high)
            if spans:
                self.forms.add("two tables")
            return f"{not_}BETWEEN", f"({written(left)}) {not_}BETWEEN ({written(low)}) AND ({written(high)})"
        not_ = rng.choice(["", "NOT "])
        if kind < 0.64:
            rows = [self.row(columns, right if rng.random() < 0.4 else []) for _ in range(rng.choice([1, 2, 3]))]
            for row in rows:
                self.collations(left, row)
            if len(rows) > 1 and not all(field.literal for row in rows for field in row):
                self.forms.add("list rows")
            return f"{not_}IN (list)", f"({written(left)}) {not_}IN ({', '.join(f'({written(row)})' for row in rows)})"
        return f"{not_}IN (SELECT)", f"({written(left)}) {self.listed(columns, list(aliases) + list(inner), not_)}"

    def query(self):
        rng = self.rng
        self.n, self.forms = 0, set()
        place = rng.choice(["value", "value", "where", "where", "not", "null", "join", "left join", "on and where",
                            "exists", "correlated", "pairs", "or", "and", "case", "count"])
        if place in ("join", "left join", "on and where"):
            kind, test = self.comparison(["t0"], ["t1"])
            if rng.random() < 0.3:
                test = f"{test} AND t0.{(c := rng.choice(list(COLUMNS)))} = t1.{c}"
            joined = "LEFT JOIN" if place == "left join" else "JOIN"
            if place == "on and where":
                other = self.comparison(["t0"], ["t0"])[1]
                sql = f"SELECT t0.id, t1.id FROM t t0 JOIN s t1 ON {other} WHERE {test}"
            else:
                first, second = rng.choice([("t t0", "s t1"), ("t t0", "s t1"), ("s t0", "t t1")])
                sql = f"SELECT t0.id, t1.id FROM {first} {joined} {second} ON {test}"
        elif place in ("exists", "correlated"):
            kind, test = self.comparison(["t0"], ["t1"])
            sql = (f"SELECT t0.id FROM t t0 WHERE {rng.choice(['', 'NOT '])}EXISTS (SELECT 1 FROM s t1 WHERE {test})"
                   if place == "exists" else
                   f"SELECT t0.id, (SELECT count(*) FROM s t1 WHERE {test}) FROM t t0")
        elif place == "pairs":
            kind, test = self.comparison(["t0", "t1"])
            sql = f"SELECT t0.id, t1.id, {test} FROM t t0, s t1"
        else:
            kind, test = self.comparison(["t0"])
            of = rng.choice(["t", "t", "s"])
            sql = {"value": f"SELECT t0.id, {test} FROM {of} t0",
                   "where": f"SELECT t0.id FROM {of} t0 WHERE {test}",
                   "not": f"SELECT t0.id FROM {of} t0 WHERE NOT {test}",
                   "null": f"SELECT t0.id FROM {of} t0 WHERE ({test}) IS NULL",
                   "or": f"SELECT t0.id FROM {of} t0 WHERE {test} OR t0.id < {rng.randrange(1, 8)}",
                   "and": f"SELECT t0.id FROM {of} t0 WHERE {test} AND t0.id > {rng.randrange(1, 8)}",
                   "case": f"SELECT t0.id, CASE WHEN {test} THEN 1 WHEN t0.id > 4 THEN 2 END FROM {of} t0",
                   "count": f"SELECT count(*), sum({test}) FROM {of} t0"}[place]
        if "BETWEEN" in kind and place in ("value", "pairs", "case", "count"):
            self.forms.add("between as a value")
        return kind, place, sql, sorted(self.forms)


def written(row):
    return ", ".join(field.sql for field in row)


def sums(rows):
    lines = sorted(",".join("NULL" if v is None else str(v) for v in row) for row in rows)
    total = 0xCBF29CE484222325
    for byte in "\n".join(lines).encode():
        total = ((total ^ byte) * 0x100000001B3) % (1 << 64)
    return f"{len(lines)}:{total:016x}"


def filled(statements):
    db = sqlite3.connect(":memory:")
    for sql in statements:
        db.execute(sql)
    return db


def database(name, seed):
    statements = tables(random.Random(f"{name} {seed}"), name)
    return filled(statements), statements


def main():
    if sys.argv[1:2] == ["--rows"]:
        db, _ = database(sys.argv[2], 1)
        for row in db.execute(sys.argv[3]):
            print(",".join("NULL" if v is None else str(v) for v in row))
        return
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 500
    seed = int(sys.argv[2]) if len(sys.argv) > 2 else 1
    every = sys.argv[3:4] == ["all"]
    print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_row_values.py {count} {seed}{' all' if every else ''}")
    for name in ("plain", "indexed", "keyed"):
        db, statements = database(name, seed)
        bare = filled(sql for sql in statements if " INDEX " not in sql)
        print(f"db\t{name}")
        for sql in statements:
            print(f"sql\t{sql}")
        rng = random.Random(f"queries {name} {seed}")
        draw, seen, refused, kinds = Draw(rng), set(), collections.Counter(), collections.Counter()
        while len(seen) < count:
            kind, place, sql, forms = draw.query()
            if sql in seen or (forms and not every):
                continue
            try:
                answer = sums(db.execute(sql).fetchall())
            except sqlite3.Error as error:
                refused[str(error)] += 1
                continue
            if answer != sums(bare.execute(sql).fetchall()):
                refused["its indexes change its answer"] += 1
                continue
            seen.add(sql)
            kinds[kind] += 1
            print(f"q\t{'; '.join([f'{kind}, {place}'] + forms)}\t{sql}\t{answer}")
        print(f"{name}: not written {dict(refused)}; {dict(kinds)}", file=sys.stderr)


if __name__ == "__main__":
    main()
