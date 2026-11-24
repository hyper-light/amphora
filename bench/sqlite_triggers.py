"""Oracle finding CP (lane C17): triggers on rowid and WITHOUT ROWID tables. BEFORE and AFTER INSERT, DELETE,
UPDATE and UPDATE OF, with and without WHEN, their bodies writing to a log, to another table under every
conflict clause, to the table itself, or raising; fired by INSERT, REPLACE, UPDATE, DELETE and UPSERT
under every conflict clause, with UNIQUE, CHECK and NOT NULL constraints, secondary, partial and
expression indexes, and RETURNING, alone or inside a transaction. Writes each statement with whether
SQLite itself takes it and what the tables hold after it (the sqlite3 library Python links, printed
first) to crates/engine/tests/fixtures/triggers.tsv, which tests/windows_and_triggers.rs holds Amphora
to.

Not drawn, each a difference of another cause this sweep found or the oracles had (lane C17's record): FAIL
and ROLLBACK in a trigger's own statements, and an OR clause before DO UPDATE (a constraint that fails in a
trigger's statement is taken by the clause of the statement that fired it, not by its own); UNIQUE ON
CONFLICT (which of two constraints is met first); NOCASE columns (the oracles' CX); text inserted in an
INTEGER PRIMARY KEY under OR ROLLBACK.

Each line is `db <name>`, `sql <statement>` (the database's tables, rows and triggers) or `q <statement>
<answer>`: the answer's rows sorted, each row's values joined by commas, a NULL written NULL, the rows
joined by semicolons, or ERROR for a statement SQLite refuses.
usage: python3 bench/sqlite_triggers.py [databases [seed]] > crates/engine/tests/fixtures/triggers.tsv"""
import random
import sqlite3
import sys

CONFLICTS = ["ABORT", "FAIL", "IGNORE", "REPLACE", "ROLLBACK"]
# What the oracles and wider draws found Amphora failing: each is asked first.
FOUND = [
    (
        "update of, without rowid",
        [
            "CREATE TABLE t(k TEXT PRIMARY KEY, a INT, b TEXT, c INT) WITHOUT ROWID",
            "CREATE TABLE lg(ev, x, y, z)",
            "CREATE TRIGGER g AFTER UPDATE OF b ON t BEGIN INSERT INTO lg VALUES ('g', old.k, old.b, new.b); END",
            "INSERT INTO t VALUES ('a', 1, 'x', 1)",
        ],
        ["UPDATE t SET b = 'b' WHERE k = 'a'", "UPDATE t SET a = 2 WHERE k = 'a'", "UPDATE t SET k = 'b' WHERE b = 'b'"],
    ),
    (
        "replace fires no delete trigger, without rowid",
        [
            "CREATE TABLE t(k TEXT PRIMARY KEY, a INT, b TEXT, c INT) WITHOUT ROWID",
            "CREATE TABLE lg(ev, x, y, z)",
            "CREATE TRIGGER g AFTER DELETE ON t BEGIN INSERT INTO lg VALUES ('g', old.k, old.b, NULL); END",
            "INSERT INTO t VALUES ('a', 1, 'x', 1)",
        ],
        ["INSERT OR REPLACE INTO t VALUES ('a', 2, 'y', 2)", "REPLACE INTO t VALUES ('a', 3, 'z', 3)", "DELETE FROM t WHERE k = 'a'"],
    ),
    (
        "no rowid to name, without rowid",
        [
            "CREATE TABLE t(k TEXT PRIMARY KEY, a INT, b TEXT, c INT) WITHOUT ROWID",
            "CREATE TABLE lg(ev, x, y, z)",
            "CREATE TRIGGER g AFTER INSERT ON t WHEN new.a > 1 BEGIN INSERT INTO lg VALUES ('g', new.rowid, NULL, NULL); END",
        ],
        ["INSERT INTO t VALUES ('a', 1, 'x', 1)", "INSERT INTO t VALUES ('b', 2, 'x', 1)"],
    ),
    (
        "a row goes in its place after an upsert's other constraint replaced one",
        [
            "CREATE TABLE t(k TEXT, a INT, b TEXT, c INT UNIQUE ON CONFLICT REPLACE, PRIMARY KEY (k, a))",
            "CREATE TABLE lg(ev, x, y, z)",
            "INSERT INTO t VALUES ('e', 1, 'x', 2), ('e', 3, 'y', 5), ('d', 3, 'z', 1), ('a', 60, 'x', 3)",
        ],
        ["INSERT INTO t VALUES ('c', 60, 'x', 5) ON CONFLICT(k, a) DO NOTHING", "SELECT rowid FROM t WHERE rowid >= 3", "UPDATE t SET b = 'w'", "SELECT count(*) FROM t WHERE rowid IN (1, 3, 4, 5)", "PRAGMA integrity_check"],
    ),
    (
        "the table is left as a BEFORE trigger wrote it, without rowid",
        [
            "CREATE TABLE t(k TEXT PRIMARY KEY, a INT, b TEXT, c INT) WITHOUT ROWID",
            "CREATE INDEX tc ON t(c)",
            "CREATE TABLE lg(ev, x, y, z)",
            "CREATE TRIGGER g BEFORE UPDATE OF a ON t BEGIN UPDATE t SET c = 100 WHERE k = old.k; END",
            "INSERT INTO t VALUES ('a', 1, 'x', 2), ('b', 2, 'y', 4)",
        ],
        ["UPDATE t SET a = 10", "SELECT k FROM t INDEXED BY tc WHERE c = 100", "SELECT k FROM t INDEXED BY tc WHERE c IN (2, 4)"],
    ),
]


class Draw:
    def __init__(self, rng):
        self.rng = rng
        rng_choice = rng.choice
        self.keyed = rng.random() < 0.6
        null = rng_choice(["", "", " NOT NULL", " NOT NULL DEFAULT 5", " NOT NULL ON CONFLICT REPLACE DEFAULT 6", " NOT NULL ON CONFLICT IGNORE"])
        check = rng_choice(["", "", "", " CHECK (a < 50)", " CHECK (a <> 4)"])
        unique = rng_choice([" UNIQUE", " UNIQUE", ""])
        self.unique = bool(unique)
        key = rng_choice(["k TEXT PRIMARY KEY, a INT{n}{c}, b TEXT, c INT{u}", "k TEXT, a INT{n}{c}, b TEXT, c INT{u}, PRIMARY KEY (k, a)", "b TEXT, k INT, a INT{n}{c}, c INT{u}, PRIMARY KEY (a DESC, k)"] + ([] if self.keyed else ["k INTEGER PRIMARY KEY, a INT{n}{c}, b TEXT, c INT{u}"]))
        self.target = "(k, a)" if "PRIMARY KEY (k, a)" in key else "(a, k)" if "PRIMARY KEY (a" in key else "(k)"
        self.number_key = "k INT" in key
        self.order = "b, k, a, c" if key.startswith("b TEXT") else "k, a, b, c"
        self.tables = [f"CREATE TABLE t({key.format(n=null, c=check, u=unique)}){' WITHOUT ROWID' if self.keyed else ''}"]
        self.tables.append(rng_choice(["CREATE TABLE u(k TEXT PRIMARY KEY, n INT UNIQUE) WITHOUT ROWID", "CREATE TABLE u(k TEXT PRIMARY KEY, n INT UNIQUE)"]))
        self.tables.append("CREATE TABLE lg(ev, x, y, z)")
        indexes = ["CREATE INDEX tb ON t(b)", "CREATE INDEX tca ON t(c, a)", "CREATE INDEX tp ON t(b) WHERE a > 2", "CREATE INDEX te ON t(a + c)", "CREATE UNIQUE INDEX tub ON t(b) WHERE b > 'x'"]
        self.indexed = rng.random() < 0.5
        if self.indexed:
            self.tables += rng.sample(indexes, rng.choice([1, 1, 2, 3]))
        self.triggers = [self.trigger(i) for i in range(rng.choice([1, 2, 2, 3, 4]))]

    def key(self):
        rng = self.rng
        return str(rng.choice([1, 2, 3, 4, 7])) if self.number_key else rng.choice(["'a'", "'b'", "'c'", "'d'", "'B'", "'e'"])

    def row(self):
        rng = self.rng
        values = {"k": self.key(), "a": rng.choice(["1", "2", "3", "4", "'5'", "NULL", "60"]), "b": rng.choice(["'x'", "'y'", "'z'", "NULL", "7"]), "c": rng.choice(["1", "2", "3", "4", "5", "6", "NULL"])}
        return "(" + ", ".join(values[name] for name in self.order.split(", ")) + ")"

    def trigger(self, i):
        rng = self.rng
        time = rng.choice(["BEFORE", "AFTER"])
        event = rng.choice(["INSERT", "DELETE", "UPDATE", "UPDATE OF", "UPDATE OF"])
        of = ""
        if event == "UPDATE OF":
            of = " " + ", ".join(rng.sample(["k", "a", "b", "c"], rng.choice([1, 1, 2])))
            event = "UPDATE"
            text = f"UPDATE OF{of}"
        else:
            text = event
        rows = {"INSERT": ["new"], "DELETE": ["old"], "UPDATE": ["old", "new"]}[event]
        read = lambda: f"{rng.choice(rows)}.{rng.choice('kabc')}"
        when = rng.choice(["", "", "", f" WHEN {read()} > 2", f" WHEN {read()} IS NOT NULL", f" WHEN {read()} IN (SELECT k FROM u) OR {read()} = 'x'", f" WHEN EXISTS (SELECT 1 FROM u WHERE u.n = {read()})"])
        body = [f"INSERT INTO lg VALUES ('{time[0]}{event[0]}{i}', {read()}, {read()}, {read()})"]
        for _ in range(rng.choice([0, 1, 1, 2])):
            drawn = rng.random()
            orc = rng.choice(["", "", " OR IGNORE", " OR REPLACE", " OR ABORT"])
            if drawn < 0.25:
                body.append(f"INSERT{orc} INTO u VALUES ({read()}, {rng.choice(['1', '2', '8', read()])})")
            elif drawn < 0.4:
                body.append(f"UPDATE{orc} u SET n = {rng.choice(['n + 1', '2', read()])} WHERE k = {rng.choice([repr('a'), read()])}")
            elif drawn < 0.5:
                body.append(f"DELETE FROM u WHERE k = {read()}")
            elif drawn < 0.58:
                body.append(f"INSERT INTO lg SELECT 's', k, n, {read()} FROM u WHERE n < 3")
            elif drawn < 0.64:
                body.append(f"INSERT INTO u VALUES ({read()}, 3) ON CONFLICT(k) DO UPDATE SET n = excluded.n + 10")
            elif drawn < 0.7:
                body.append("INSERT INTO lg SELECT 'n', count(*), max(c), NULL FROM t")
            elif drawn < 0.85:
                body.append(f"SELECT RAISE({rng.choice(['IGNORE', 'IGNORE', 'ABORT, ' + repr('no'), 'FAIL, ' + repr('no'), 'ROLLBACK, ' + repr('no')])}) WHERE {read()} = {rng.choice(['2', '3', repr('x'), repr('a')])}")
            elif time == "AFTER" and event != "INSERT":
                body.append(rng.choice([f"UPDATE t SET b = 'w' WHERE c = {read()}", f"DELETE FROM t WHERE c = {read()} AND k <> {rows[0]}.k"]))
            else:
                body.append(f"SELECT {read()}")
        name = f"g{i}"
        return f"CREATE TRIGGER {name} {time} {text} ON t{when} BEGIN {'; '.join(body)}; END"

    def statement(self):
        rng = self.rng
        orc = rng.choice(["", "", ""] + [f" OR {c}" for c in CONFLICTS])
        drawn = rng.random()
        if drawn < 0.3:
            rows = ", ".join(self.row() for _ in range(rng.choice([1, 1, 2, 3])))
            returning = rng.choice(["", "", "", " RETURNING k, a, b, c"])
            return f"INSERT{orc} INTO t VALUES {rows}{returning}"
        if drawn < 0.36 and not self.number_key:
            return f"INSERT{orc} INTO t(k, a, b, c) SELECT k, n, 'x', n + 1 FROM u ORDER BY k"
        if drawn < 0.5:
            sets = ", ".join(rng.sample([f"b = {rng.choice(['excluded.b', repr('q'), 't.b || excluded.b'])}", f"c = {rng.choice(['excluded.c', 't.c + 1', '3'])}", "a = excluded.a", "a = t.a + 1"], rng.choice([1, 1, 2])))
            where = rng.choice(["", "", f" WHERE t.a < {rng.choice([2, 3, 9])}"])
            target = rng.choice([self.target, self.target, "(c)" if self.unique else self.target])
            do = rng.choice([f"DO UPDATE SET {sets}{where}", f"DO UPDATE SET {sets}{where}", "DO NOTHING"])
            rows = ", ".join(self.row() for _ in range(rng.choice([1, 2])))
            return f"INSERT{orc if do == 'DO NOTHING' else ''} INTO t VALUES {rows} ON CONFLICT{target} {do}"
        if drawn < 0.8:
            sets = ", ".join(rng.sample([f"a = {rng.choice(['a + 1', '3', '4', 'NULL', 'c'])}", f"b = {rng.choice([repr('x'), repr('q'), 'b || a', 'NULL'])}", f"c = {rng.choice(['c + 1', '2', 'c', 'a'])}", f"k = {rng.choice([self.key(), 'k', self.key()])}"], rng.choice([1, 1, 2, 3])))
            where = self.where(["", f" WHERE +c > {rng.choice([1, 2, 3])}", " WHERE +b IS NOT NULL", " WHERE +a IN (SELECT n FROM u)"])
            returning = rng.choice(["", "", "", " RETURNING k, a, b, c"])
            return f"UPDATE{orc} t SET {sets}{where}{returning}"
        where = self.where([f" WHERE +c > {rng.choice([2, 3])}", " WHERE +b = 'x'", "", " WHERE +a IN (SELECT n FROM u)"])
        returning = rng.choice(["", "", "", " RETURNING k, a, b, c"])
        return f"DELETE FROM t{where}{returning}"

    def where(self, scans):
        """The rows a statement is on. The order a scan visits rows in, which conflicts and triggers show, is
        the planner's to choose where a table has an index: there, one row by its key."""
        rng = self.rng
        if not self.indexed and not self.unique and rng.random() < 0.7:
            return rng.choice(scans)
        if self.target == "(k)":
            return f" WHERE k = {self.key()}"
        return f" WHERE k = {self.key()} AND a = {rng.choice([1, 2, 3, 5])}"


def text(value):
    if value is None:
        return "NULL"
    if isinstance(value, float):
        return f"{value:.9f}"
    return str(value)


def answer(db, sql):
    try:
        rows = sorted(",".join(text(value) for value in row) for row in db.execute(sql).fetchall())
    except sqlite3.Error:
        return "ERROR"
    return f"{len(rows)}:{';'.join(rows)}"


def ask(db, sql, out):
    out.append(f"q\t{sql}\t{answer(db, sql)}")


LOOK = ["SELECT 't', quote(k), quote(a), quote(b), quote(c) FROM t UNION ALL SELECT 'u', quote(k), quote(n), '', '' FROM u UNION ALL SELECT 'lg', quote(ev), quote(x), quote(y), quote(z) FROM lg", "DELETE FROM lg"]
LOOK_FOUND = ["SELECT 't', quote(k), quote(a), quote(b), quote(c) FROM t UNION ALL SELECT 'lg', quote(ev), quote(x), quote(y), quote(z) FROM lg", "DELETE FROM lg"]


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 300
    seed = sys.argv[2] if len(sys.argv) > 2 else "CP"
    print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_triggers.py {count} {seed}")
    out = []
    for name, fill, statements in FOUND:
        db = sqlite3.connect(":memory:", isolation_level=None)
        out.append(f"db\t{name}")
        for statement in fill:
            db.execute(statement)
            out.append(f"sql\t{statement}")
        for statement in statements:
            ask(db, statement, out)
            for look in LOOK_FOUND:
                ask(db, look, out)
    made = refused = 0
    while made < count:
        rng = random.Random(f"{seed}-{made}-{refused}")
        draw = Draw(rng)
        db = sqlite3.connect(":memory:", isolation_level=None)
        fill = draw.tables + ["INSERT INTO u VALUES ('a', 1), ('b', 2), ('z', 7)"]
        for _ in range(rng.choice([3, 4, 5])):
            fill.append(f"INSERT OR IGNORE INTO t VALUES {draw.row()}")
        lines = []
        try:
            for statement in fill:
                try:
                    db.execute(statement)
                except sqlite3.IntegrityError:
                    continue
                lines.append(f"sql\t{statement}")
            for statement in draw.triggers:
                db.execute(statement)
                lines.append(f"sql\t{statement}")
        except sqlite3.Error:
            refused += 1
            continue
        out.append(f"db\tdrawn {made}")
        out += lines
        made += 1
        transaction = rng.random() < 0.3
        if transaction:
            ask(db, "BEGIN", out)
        for _ in range(rng.choice([4, 5, 6])):
            ask(db, draw.statement(), out)
            for look in LOOK:
                ask(db, look, out)
        if transaction:
            ask(db, "COMMIT", out)
            ask(db, LOOK[0], out)
        ask(db, "PRAGMA integrity_check", out)
    print("\n".join(out))
    print(f"# {made} drawn, {refused} SQLite refused", file=sys.stderr)


main()
