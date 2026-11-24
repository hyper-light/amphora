"""AC-T1.8.6 (lane C9, the oracles' finding CG): inner, left, right and full joins whose ON holds terms
on the left tables alone, on the joined table alone and on both, under WHERE terms on either side, over
columns with a unique index, a plain one and none, all with NULLs, and the answers SQLite itself gives
(the sqlite3 library Python links, printed first). Writes crates/engine/tests/fixtures/on_terms.tsv,
which tests/on_terms.rs holds Amphora to.

Each line is `db <name>`, `sql <statement>` (the database's rows) or `q <query> <answer>`: the answer is
`<rows>:<the FNV-1a 64 of the rows, sorted, each a line, its values joined by commas>`.
usage: python3 bench/sqlite_on_terms.py > crates/engine/tests/fixtures/on_terms.tsv
       python3 bench/sqlite_on_terms.py --rows '<database>' '<query>'   (SQLite's rows of one query)"""
import collections
import random
import sqlite3
import sys

COLUMNS = "id INTEGER PRIMARY KEY, u INT UNIQUE, x INT, y INT, t TEXT UNIQUE, p INT"
TEXT = 10007  # prime, so t's number is a permutation of the ids


def text_of(i):
    return f"{i * 37 % TEXT:05d}"


def table(name, count, xs, ys, partial):
    """`count` rows: u and t unique, x and y of `xs` and `ys` values, p of 4 and in no index, each but p
    NULL in some rows. `partial`: the indexes on x and y hold only some rows."""
    over_x, over_y = (" WHERE p = 1", " WHERE x IS NOT NULL") if partial else ("", "")
    return [
        f"CREATE TABLE {name}({COLUMNS})",
        f"CREATE INDEX {name}_x ON {name}(x){over_x}",
        f"CREATE INDEX {name}_y ON {name}(y){over_y}",
        f"WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i + 1 FROM n WHERE i < {count}) "
        f"INSERT INTO {name} SELECT i, CASE WHEN i % 7 <> 0 THEN i * 3 END, "
        f"CASE WHEN i % 5 <> 0 THEN i % {xs} END, CASE WHEN i % 6 <> 0 THEN i * 7 % {ys} END, "
        f"CASE WHEN i % 9 <> 0 THEN printf('%05d', i * 37 % {TEXT}) END, i % 4 FROM n",
    ]


# name, rows of a, b and c, values of x and of y, partial indexes, queries, steps of 1,000 a draw may take
DATABASES = [
    ("small", (24, 16, 10), 5, 6, False, 560, 60),
    ("partial", (24, 16, 10), 5, 6, True, 200, 60),
    ("large", (3000, 12, 8), 20, 15, False, 200, 150),
]


class Draw:
    """One query, drawn term by term."""

    def __init__(self, rng, rows, xs, ys, one_a):
        self.rng, self.rows, self.xs, self.ys, self.one_a = rng, rows, xs, ys, one_a
        self.kinds = collections.Counter()

    def term(self, alias, name):
        """A term on `alias` alone: mostly a comparison an index can serve."""
        rng = self.rng
        row = rng.randrange(1, self.rows[name] + 1)
        ints = {"id": row, "u": row * 3, "x": rng.randrange(self.xs), "y": rng.randrange(self.ys)}
        column = rng.choice(["id", "u", "u", "x", "x", "y", "y", "t", "t"])
        value = f"'{text_of(row)}'" if column == "t" else str(ints[column])
        at = f"{alias}.{column}"
        kind = rng.random()
        if kind < 0.30:
            return f"{at} = {value}"
        if kind < 0.52:
            return f"{at} {rng.choice(['<', '<=', '>', '>='])} {value}"
        if kind < 0.58:
            return f"{value} {rng.choice(['<', '<=', '>', '>=', '='])} {at}"
        if kind < 0.64:
            return f"{at} IS {rng.choice([value, 'NULL'])}"
        if kind < 0.70:
            return f"{at} {rng.choice(['IS NULL', 'IS NOT NULL', 'IS NULL'])}"
        if kind < 0.76:
            more = f"'{text_of(row % 7 + 1)}'" if column == "t" else str(ints[column] + 1)
            return f"{at} {rng.choice(['IN', 'IN', 'NOT IN'])} ({value}, {more})"
        if kind < 0.81 and column != "t":
            return f"{at} BETWEEN {ints[column] - 2} AND {ints[column] + 2}"
        if kind < 0.89:
            other = rng.choice(["x", "y"])
            return f"({at} = {value} OR {alias}.{other} = {ints[other]})"
        if kind < 0.92:
            return f"({alias}.x = {ints['x']} AND {alias}.y >= {ints['y']} OR {alias}.id = {row})"
        if kind < 0.95:
            return f"{alias}.p = {rng.randrange(4)}"
        return rng.choice([f"{at} <> {value}", f"NOT {at} > {value}", f"{alias}.x + 0 = {ints['x']}",
                           f"{alias}.t GLOB '0*'", f"coalesce({alias}.y, 1) < {ints['y']}"])

    def link(self, left, right):
        return self.rng.choice([f"{right}.id = {left}.id", f"{right}.id = {left}.id", f"{right}.u = {left}.u",
                                f"{right}.x = {left}.x", f"{left}.y = {right}.y", f"{right}.id = {left}.x",
                                f"{right}.t = {left}.t", f"{right}.id < {left}.x", f"{right}.x = {left}.p",
                                f"{left}.id + 1 = {right}.id"])

    def query(self):
        rng = self.rng
        count = rng.choice([2, 2, 2, 3, 3, 4])
        names = [rng.choice("abc") for _ in range(count)]
        if self.one_a:  # the large table once, its rows of no product with another large one
            names = [rng.choice("bc") for _ in range(count)]
            names[rng.choice([0, 0, rng.randrange(count)])] = "a"
        aliases = [f"t{n}" for n in range(count)]
        text, focus, outer = f"{names[0]} t0", None, []
        for n in range(1, count):
            kind = rng.choice(["LEFT JOIN", "LEFT JOIN", "LEFT JOIN", "RIGHT JOIN", "FULL JOIN", "JOIN"])
            on = []
            if rng.random() < 0.7:
                on.append(self.link(aliases[rng.randrange(n)], aliases[n]))
                self.kinds[kind, "mixed"] += 1
            for chance in (0.75, 0.25):
                if rng.random() < chance:
                    at = rng.randrange(n)
                    focus = at if rng.random() < 0.8 else focus
                    on.append(self.term(aliases[at], names[at]))
                    self.kinds[kind, "left-only"] += 1
            if rng.random() < 0.35 or not on:
                on.append(self.term(aliases[n], names[n]))
                self.kinds[kind, "right-only"] += 1
                if kind in ("RIGHT JOIN", "FULL JOIN") and rng.random() < 0.6:
                    focus = n
            rng.shuffle(on)
            if kind != "JOIN":
                outer.append(n)
            text += f" {kind} {names[n]} {aliases[n]} ON {' AND '.join(on)}"
        where = []
        for _ in range(rng.choice([0, 1, 1, 2, 2, 2, 3])):
            at = focus if focus is not None and rng.random() < 0.65 else rng.randrange(count)
            where.append(self.term(aliases[at], names[at]))
        if outer and rng.random() < 0.15:
            where.append(f"t{rng.choice(outer)}.id IS {rng.choice(['NULL', 'NOT NULL'])}")
        if outer and rng.random() < 0.1:
            at, other = rng.choice(outer), rng.randrange(count)
            where.append(f"(t{at}.id IS NULL OR {self.term(aliases[other], names[other])})")
        self.kinds["where", str(len(where))] += 1
        rng.shuffle(where)
        sql = f"SELECT {', '.join(f'{alias}.id' for alias in aliases)} FROM {text}"
        return sql + (" WHERE " + " AND ".join(where) if where else "")


def opened(name):
    _, counts, xs, ys, partial, _, _ = next(d for d in DATABASES if d[0] == name)
    db, fill = sqlite3.connect(":memory:"), []
    for letter, count in zip("abc", counts):
        fill += table(letter, count, xs, ys, partial)
    for sql in fill:
        db.execute(sql)
    return db, fill


def lines(db, sql):
    return sorted(",".join("NULL" if v is None else str(v) for v in row) for row in db.execute(sql))


def fnv(rows):
    total = 0xCBF29CE484222325
    for byte in "\n".join(rows).encode():
        total = ((total ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return f"{total:016x}"


def main():
    print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_on_terms.py")
    for name, counts, xs, ys, _, drawn, thousands in DATABASES:
        db, fill = opened(name)
        print(f"db\t{name}")
        for sql in fill:
            print(f"sql\t{sql}")
        rng = random.Random(f"amphora on terms {name}")
        draw, kept, seen = Draw(rng, dict(zip("abc", counts)), xs, ys, name == "large"), 0, set()
        kinds = collections.Counter()
        while kept < drawn:
            draw.kinds = collections.Counter()
            sql = draw.query()
            # A draw past what a debug build reads in a moment isn't kept.
            steps = [0]

            def step():
                steps[0] += 1
                return 1 if steps[0] > thousands else 0

            db.set_progress_handler(step, 1000)
            try:
                got = lines(db, sql)
            except sqlite3.OperationalError as error:
                assert "interrupted" in str(error), (error, sql)
                continue
            finally:
                db.set_progress_handler(None, 0)
            if sql in seen or (not got and rng.random() > 0.2):
                continue
            seen.add(sql)
            kept += 1
            kinds += draw.kinds
            print(f"q\t{sql}\t{len(got)}:{fnv(got)}")
        for (kind, what), count in sorted(kinds.items()):
            print(f"# {name}: {kind}, {what}: {count}")


if sys.argv[1:2] == ["--rows"]:
    for line in lines(opened(sys.argv[2])[0], sys.argv[3]):
        print(line)
else:
    main()
