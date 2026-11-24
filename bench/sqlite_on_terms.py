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
    ("small", (24, 16, 10), 5, 6, False, 640, 60),
    ("partial", (24, 16, 10), 5, 6, True, 320, 60),
    ("large", (3000, 12, 8), 20, 15, False, 240, 150),
]


class Draw:
    """One query, drawn term by term."""

    def __init__(self, rng, rows, xs, ys, partial, one_a):
        self.rng, self.rows, self.xs, self.ys = rng, rows, xs, ys
        self.partial, self.one_a = partial, one_a
        self.kinds = collections.Counter()

    def values(self, name):
        """A value of each column of one of the table's rows, as SQL writes it."""
        rng = self.rng
        row = rng.randrange(1, self.rows[name] + 1)
        ints = {"id": row, "u": row * 3, "x": rng.randrange(self.xs), "y": rng.randrange(self.ys)}
        return {**{column: str(value) for column, value in ints.items()}, "t": f"'{text_of(row)}'"}

    def comparisons(self, alias, name, columns):
        """One comparison an index can serve on each of `columns`."""
        rng, out = self.rng, []
        for column in columns:
            way = rng.choice(["=", "=", "<", "<=", ">", ">="])
            value = self.values(name)[column]
            out.append(rng.choice([f"{alias}.{column} {way} {value}", f"{alias}.{column} {way} {value}",
                                   f"{value} {way} {alias}.{column}"]))
        return out

    def trap(self, alias, name):
        """ON terms and WHERE terms on `alias` alone, each on an index of its own (CG's shape): the ON's
        say which rows match, the WHERE's which rows there are."""
        rng = self.rng
        columns = rng.sample(["id", "u", "x", "y", "t"], rng.choice([2, 3, 3, 4, 4]))
        split = rng.randrange(1, len(columns))
        where = self.comparisons(alias, name, columns[split:])
        if rng.random() < 0.6:
            where.append(rng.choice([f"{alias}.p = {rng.randrange(4)}", f"{alias}.p IS NOT NULL",
                                     f"{alias}.p < 3", f"{alias}.x IS NOT NULL", f"{alias}.y IS NULL"]))
        return self.comparisons(alias, name, columns[:split]), where

    def proof(self, alias):
        """A term that is a partial index's own."""
        return self.rng.choice([f"{alias}.p = 1", f"{alias}.x IS NOT NULL"])

    def term(self, alias, name):
        """A term on `alias` alone: mostly a comparison an index can serve."""
        rng = self.rng
        if self.partial and rng.random() < 0.3:
            return self.proof(alias)
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

    def proof_query(self):
        """A join of a table with partial indexes over a column one of them has, then an outer join whose
        ON holds a partial index's own term on that table or on the first."""
        rng = self.rng
        names = [rng.choice("abc") for _ in range(3)]
        column, row = rng.choice("xy"), self.values(names[1])
        first = rng.choice([f"t1.{column} = t0.{rng.choice(['id', 'x', 'y', 'p'])}",
                            f"t1.{column} {rng.choice(['=', '<', '>='])} {row[column]}"])
        joined = rng.choice(["JOIN", "JOIN", "JOIN", "LEFT JOIN"])
        kind = rng.choice(["LEFT JOIN", "LEFT JOIN", "LEFT JOIN", "FULL JOIN", "RIGHT JOIN"])
        on = [self.proof(rng.choice(["t1", "t1", "t1", "t0"]))]
        self.kinds[kind, "left-only"] += 1
        if rng.random() < 0.8:
            on.append(self.link("t1", "t2"))
            self.kinds[kind, "mixed"] += 1
        rng.shuffle(on)
        where = [self.term(f"t{at}", names[at]) for at in rng.sample(range(3), rng.choice([0, 0, 1, 2]))]
        sql = (f"SELECT t0.id, t1.id, t2.id FROM {names[0]} t0 {joined} {names[1]} t1 ON {first} "
               f"{kind} {names[2]} t2 ON {' AND '.join(on)}")
        return sql + (" WHERE " + " AND ".join(where) if where else "")

    def query(self):
        rng = self.rng
        if self.partial and rng.random() < 0.4:
            return self.proof_query()
        count = rng.choice([2, 2, 2, 3, 3, 4])
        names = [rng.choice("abc") for _ in range(count)]
        if self.one_a:  # the large table once, its rows of no product with another large one
            names = [rng.choice("bc") for _ in range(count)]
            names[rng.choice([0, 0, rng.randrange(count)])] = "a"
        aliases = [f"t{n}" for n in range(count)]
        text, focus, outer, where = f"{names[0]} t0", None, [], []
        # Half the draws put a trap at one of their joins.
        trapped = rng.randrange(1, count) if rng.random() < 0.5 else None
        for n in range(1, count):
            kind = rng.choice(["LEFT JOIN", "LEFT JOIN", "LEFT JOIN", "RIGHT JOIN", "FULL JOIN", "JOIN"])
            on = []
            if rng.random() < 0.7:
                on.append(self.link(aliases[rng.randrange(n)], aliases[n]))
                self.kinds[kind, "mixed"] += 1
            if n == trapped:
                # On the joined table, half of the RIGHT and FULL JOINs' (a swapped one's left side).
                at = n if kind in ("RIGHT JOIN", "FULL JOIN") and rng.random() < 0.5 else rng.randrange(n)
                terms, over = self.trap(aliases[at], names[at])
                on += terms
                where += over
                self.kinds[kind, "right-only" if at == n else "left-only"] += len(terms)
            if n != trapped and rng.random() < 0.2:
                on += self.comparisons(aliases[n], names[n], rng.sample(["id", "u", "x", "y", "t"], 2))
                self.kinds[kind, "right-only"] += 2
            for chance in (0.0, 0.1) if trapped else (0.75, 0.25):
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
        for _ in range(rng.choice([0, 0, 1]) if trapped else rng.choice([0, 1, 1, 2, 2, 2, 3])):
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
    for name, counts, xs, ys, partial, drawn, thousands in DATABASES:
        db, fill = opened(name)
        print(f"db\t{name}")
        for sql in fill:
            print(f"sql\t{sql}")
        rng = random.Random(f"amphora on terms {name}")
        draw = Draw(rng, dict(zip("abc", counts)), xs, ys, partial, name == "large")
        kept, seen = 0, set()
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
            if sql in seen or (not got and rng.random() > 0.06):
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
