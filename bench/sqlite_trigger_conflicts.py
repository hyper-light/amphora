"""A conflict is judged by the clause of the statement that meets it (lane C22, from C17's
sqlite_triggers.py). Triggers two and three deep on rowid and WITHOUT ROWID tables, every statement in and
outside them under no clause or any of ABORT, FAIL, IGNORE, REPLACE and ROLLBACK; UPSERT beside triggers and
inside them, under a clause too; PRIMARY KEY, UNIQUE, NOT NULL and CHECK constraints, each with no clause or
one of its own; foreign keys that cascade, set null and set default into tables with triggers; alone, inside a
transaction, and inside savepoints rolled back and released. Writes each statement with whether SQLite itself
takes it (its words, when it doesn't) and what every table holds after it (the sqlite3 library Python links,
printed first) to crates/engine/tests/fixtures/trigger_conflicts.tsv, which tests/windows_and_triggers.rs
holds Amphora to.

The rule (sqlite.org/lang_createtrigger.html, sqlite.org/lang_conflict.html): a statement in a trigger is under
the clause of the statement that fired it, when that one has a clause, and under its own when it hasn't. A DO
UPDATE is under ABORT (sqlite.org/lang_upsert.html), and so is what it fires; a DELETE has no clause, and what
it fires keeps its own; a foreign key's action is under ABORT (SQLite's sqlite3FkActions). What fails under FAIL
leaves what its statement wrote before it, under ROLLBACK ends the transaction, under ABORT undoes the statement
the user wrote.

Not drawn, each a difference of another cause (lane C17's and C22's records): NOCASE columns; the order a
statement of several rows visits them in where an index may be chosen. Not held: which constraint refused a
statement beside foreign keys (a trigger's program refuses a violation where it ends, SQLite where the statement
does).

Each line is `db <name>`, `sql <statement>` (the database's tables, rows and triggers) or `q <statement>
<answer>`: the answer's rows sorted, each row's values joined by commas, a NULL written NULL, the rows joined by
semicolons, or ERROR for a statement SQLite refuses, with its words when a constraint refused it.
usage: python3 bench/sqlite_trigger_conflicts.py [databases [seed]] > crates/engine/tests/fixtures/trigger_conflicts.tsv"""
import random
import sqlite3
import sys

CONFLICTS = ["ABORT", "FAIL", "IGNORE", "REPLACE", "ROLLBACK"]
ORS = [""] + [f" OR {c}" for c in CONFLICTS]
ONS = [""] + [f" ON CONFLICT {c}" for c in CONFLICTS]
LOOK = "SELECT * FROM look"
LOOK_FOUND = "CREATE VIEW look AS SELECT 't', quote(k), quote(a), '', '' FROM t UNION ALL SELECT 'u', quote(k), quote(n), '', '' FROM u UNION ALL SELECT 'lg', quote(ev), quote(x), quote(y), quote(z) FROM lg"
LOOK_DRAWN = "CREATE VIEW look AS SELECT 't', quote(k), quote(a), quote(b), quote(c) FROM t UNION ALL SELECT 'u', quote(k), quote(n), '', '' FROM u UNION ALL SELECT 'w', quote(k), quote(n), quote(m), '' FROM w UNION ALL SELECT 'lg', quote(ev), quote(x), quote(y), quote(z) FROM lg"


# What the lanes found Amphora failing, each in its own form: a database's fill, and what's asked of it.
FOUND = [
    (
        "a DO UPDATE's trigger is under ABORT, not its INSERT's IGNORE",
        ["CREATE TABLE t(k TEXT PRIMARY KEY, a INT)", "CREATE TABLE u(k TEXT PRIMARY KEY, n INT UNIQUE)", "INSERT INTO u VALUES ('a', 1), ('b', 2)", "INSERT INTO t VALUES ('x', 1)", "CREATE TRIGGER g AFTER UPDATE ON t BEGIN UPDATE u SET n = 2 WHERE k = 'a'; END"],
        ["INSERT OR IGNORE INTO t VALUES ('x', 5) ON CONFLICT(k) DO UPDATE SET a = excluded.a", "SELECT * FROM t", "SELECT * FROM u"],
    ),
    (
        "a DO UPDATE is under ABORT, not its INSERT's FAIL or ROLLBACK",
        ["CREATE TABLE t(k TEXT PRIMARY KEY, a INT UNIQUE CHECK (a < 50))", "INSERT INTO t VALUES ('x', 1), ('y', 2)"],
        ["BEGIN"] + [ask for clause in CONFLICTS for ask in [f"INSERT OR {clause} INTO t VALUES ('{clause}', {3 + len(clause)}), ('x', 2) ON CONFLICT(k) DO UPDATE SET a = excluded.a", "SELECT * FROM t", f"INSERT OR {clause} INTO t VALUES ('{clause}', {13 + len(clause)}), ('x', 3) ON CONFLICT(k) DO UPDATE SET a = 60", "SELECT * FROM t"]] + ["COMMIT"],
    ),
    (
        "a trigger's statement keeps its own clause beside the next one's",
        ["CREATE TABLE t(k TEXT PRIMARY KEY)", "CREATE TABLE u(n INT UNIQUE)", "INSERT INTO u VALUES (1)", "CREATE TABLE lg(x)"] + [f"CREATE TRIGGER g{clause} AFTER INSERT ON t WHEN new.k = '{clause}' BEGIN INSERT INTO lg VALUES ('{clause}'); INSERT OR {clause} INTO u VALUES (1); INSERT INTO lg VALUES ('after {clause}'); END" for clause in CONFLICTS],
        [ask for clause in CONFLICTS for ask in ["BEGIN", "INSERT INTO lg VALUES ('kept')", f"INSERT INTO t VALUES ('{clause}')", "SELECT * FROM lg", "SELECT * FROM t", "COMMIT", "DELETE FROM lg", f"INSERT INTO t VALUES ('p{clause}'), ('{clause}'), ('q{clause}')", "SELECT * FROM lg", "SELECT * FROM t"]],
    ),
    (
        "a DELETE has no clause: what it fires keeps its own",
        ["CREATE TABLE t(k INT PRIMARY KEY, a)", "CREATE TABLE w(k INT)", "INSERT INTO w VALUES (1)", "CREATE TABLE u(n INT UNIQUE, m)", "INSERT INTO u VALUES (5, 0)", "CREATE TRIGGER g AFTER INSERT ON t BEGIN DELETE FROM w; END", "CREATE TRIGGER h AFTER DELETE ON w BEGIN INSERT INTO u VALUES (5, 1); END"],
        ["INSERT OR IGNORE INTO t VALUES (1, 2)", "SELECT * FROM t", "INSERT OR FAIL INTO t VALUES (1, 2)", "SELECT * FROM t", "INSERT OR REPLACE INTO t VALUES (1, 2)", "SELECT * FROM u", "SELECT * FROM w"],
    ),
    (
        "a foreign key's action is under ABORT, not its table's clause",
        ["PRAGMA foreign_keys = ON", "CREATE TABLE p(k INTEGER PRIMARY KEY)", "INSERT INTO p VALUES (1), (2), (9)", "CREATE TABLE c(k INT DEFAULT 9 UNIQUE ON CONFLICT REPLACE REFERENCES p(k) ON DELETE SET DEFAULT, v)", "INSERT INTO c VALUES (1, 'a'), (2, 'b')"],
        ["DELETE FROM p WHERE k = 1", "SELECT * FROM c", "DELETE FROM p WHERE k = 2", "SELECT * FROM c", "SELECT * FROM p"],
    ),
    (
        "a foreign key's action is under ABORT, and so is what it fires",
        ["PRAGMA foreign_keys = ON", "CREATE TABLE p(k INTEGER PRIMARY KEY)", "INSERT INTO p VALUES (1)", "CREATE TABLE c(k INT REFERENCES p(k) ON DELETE SET NULL, v)", "INSERT INTO c VALUES (1, 'a')", "CREATE TABLE u(n INT UNIQUE)", "INSERT INTO u VALUES (5)", "CREATE TRIGGER g AFTER UPDATE ON c BEGIN INSERT OR IGNORE INTO u VALUES (5); END"],
        ["DELETE FROM p WHERE k = 1", "SELECT * FROM c", "SELECT * FROM p", "SELECT * FROM u"],
    ),
    (
        "NOT NULL has the clause it was declared with",
        ["CREATE TABLE t(a INT NOT NULL ON CONFLICT FAIL, b INT NOT NULL ON CONFLICT ROLLBACK, c INT NOT NULL ON CONFLICT REPLACE DEFAULT 6, d INT NOT NULL ON CONFLICT IGNORE)", "CREATE TABLE lg(x)", "CREATE TRIGGER g BEFORE UPDATE ON t WHEN old.a > 1 BEGIN INSERT INTO lg VALUES (old.a); END"],
        ["INSERT INTO t VALUES (1, 1, NULL, 1), (2, 2, 2, NULL), (NULL, 2, 2, 2), (3, 3, 3, 3)", "SELECT * FROM t", "INSERT INTO t VALUES (2, 2, 2, 2), (3, 3, 3, 3)", "BEGIN", "INSERT INTO t VALUES (7, 7, 7, 7)", "UPDATE t SET a = CASE WHEN a < 3 THEN 9 END", "SELECT * FROM t", "UPDATE t SET c = NULL", "SELECT * FROM t", "UPDATE t SET d = NULL", "SELECT * FROM t", "INSERT INTO t VALUES (8, NULL, 8, 8)", "SELECT * FROM t", "COMMIT", "SELECT * FROM lg"],
    ),
    (
        "a column's own IGNORE beside an upsert",
        ["CREATE TABLE t(k TEXT PRIMARY KEY, c INT UNIQUE ON CONFLICT IGNORE)", "INSERT INTO t VALUES ('a', 1)"],
        ["INSERT INTO t VALUES ('b', 1) ON CONFLICT(k) DO NOTHING", "INSERT INTO t VALUES ('b', 1), ('c', 2) ON CONFLICT(k) DO UPDATE SET c = 9", "SELECT * FROM t"],
    ),
    (
        "a rowid that is no integer is refused under ABORT, whatever the clause",
        ["CREATE TABLE t(k INTEGER PRIMARY KEY, a)", "INSERT INTO t VALUES (1, 11), (2, 'x')"],
        ["BEGIN", "INSERT INTO t VALUES (3, 3)"] + [ask for clause in CONFLICTS for ask in [f"INSERT OR {clause} INTO t VALUES ('x', 1)", f"INSERT OR {clause} INTO t VALUES (5, 5), ('x', 1)", f"UPDATE OR {clause} t SET k = a", "SELECT * FROM t"]] + ["COMMIT", "SELECT * FROM t", "INSERT OR FAIL INTO t VALUES (5, 5), ('x', 1)", "SELECT * FROM t"],
    ),
    (
        "a function's refusal is under ABORT, whatever the clause",
        ["CREATE TABLE s(x)", "INSERT INTO s VALUES ('[1]'), ('['), ('[2]')", "CREATE TABLE t(a UNIQUE)"],
        ["BEGIN"] + [ask for clause in CONFLICTS for ask in [f"INSERT OR {clause} INTO t SELECT json(x) FROM s", "SELECT * FROM t"]] + ["COMMIT", "INSERT OR FAIL INTO t SELECT json(x) FROM s", "SELECT * FROM t"],
    ),
] + [
    (
        f"a WITHOUT ROWID table's key is checked in its place: {table}",
        [f"CREATE TABLE t({table}) WITHOUT ROWID", "INSERT INTO t(k, c) VALUES (1, 1), (2, 2)"],
        ["INSERT INTO t(k, c) VALUES (1, 2)", "INSERT INTO t(k, c) VALUES (3, 3), (1, 2)", "SELECT k, c FROM t", "UPDATE t SET k = 1, c = 1 WHERE k = 2", "SELECT k, c FROM t", "INSERT INTO t(k, c) VALUES (7, 7), (1, 2) ON CONFLICT(c) DO UPDATE SET c = 8", "SELECT k, c FROM t"],
    )
    for key in ["TEXT", "INTEGER", "INT"]
    for mine in ["", " ON CONFLICT IGNORE", " ON CONFLICT FAIL", " ON CONFLICT REPLACE"]
    for other in ["", " ON CONFLICT IGNORE", " ON CONFLICT FAIL", " ON CONFLICT REPLACE"]
    if mine != other
    for table in [f"k {key} PRIMARY KEY{mine}, c INT UNIQUE{other}", f"c INT UNIQUE{other}, k {key} PRIMARY KEY{mine}", f"k {key}, c INT UNIQUE{other}, PRIMARY KEY (k){mine}"]
]


def nested():
    """A trigger's statement that fires another's: every clause on the outermost by every one on each of the two."""
    out = []
    for middle in ORS:
        for inner in ORS:
            out.append(
                (
                    f"nested{middle},{inner}",
                    ["CREATE TABLE t(k TEXT PRIMARY KEY)", "CREATE TABLE m(k TEXT PRIMARY KEY)", "CREATE TABLE u(n INT UNIQUE)", "INSERT INTO u VALUES (1)", "CREATE TABLE lg(x)", f"CREATE TRIGGER g AFTER INSERT ON t BEGIN INSERT INTO lg VALUES ('g'); INSERT{middle} INTO m VALUES (new.k); INSERT INTO lg VALUES ('g after'); END", f"CREATE TRIGGER h AFTER INSERT ON m BEGIN INSERT INTO lg VALUES ('h'); INSERT{inner} INTO u VALUES (1); INSERT INTO lg VALUES ('h after'); END"],
                    [ask for at, outer in enumerate(ORS) for ask in ["BEGIN", "INSERT INTO lg VALUES ('kept')", f"INSERT{outer} INTO t VALUES ('a{at}'), ('b{at}')", "SELECT 't', k FROM t UNION ALL SELECT 'm', k FROM m UNION ALL SELECT 'u', n FROM u UNION ALL SELECT 'lg', x FROM lg", "COMMIT", "DELETE FROM lg"]],
                )
            )
    return out


def ladder():
    """Every clause on the firing statement, by every clause on the trigger's, by every clause on a trigger's it
    fires: what fails is the innermost, with a statement before and after each level."""
    out = []
    for rowid in ["", " WITHOUT ROWID"]:
        for fire in ["INSERT{} INTO t VALUES ('y', 2)", "UPDATE{} t SET a = 7 WHERE k = 'x'", "INSERT{} INTO t VALUES ('x', 5) ON CONFLICT(k) DO UPDATE SET a = excluded.a"]:
            event = "UPDATE" if "SET" in fire else "INSERT"
            for inner in ["UPDATE{} u SET n = 2 WHERE k = 'a'", "INSERT{} INTO u VALUES ('c', 2)", "INSERT{} INTO u VALUES ('a', 2) ON CONFLICT(k) DO UPDATE SET n = excluded.n"]:
                for clause in ORS:
                    out.append(
                        (
                            f"ladder{rowid} {fire.split('{')[0]}{' upsert' if 'DO' in fire else ''} fires {inner}".replace("{}", clause or " "),
                            [
                                f"CREATE TABLE t(k TEXT PRIMARY KEY, a INT){rowid}",
                                f"CREATE TABLE u(k TEXT PRIMARY KEY, n INT UNIQUE){rowid}",
                                "CREATE TABLE lg(ev, x, y, z)",
                                LOOK_FOUND,
                                "INSERT INTO u VALUES ('a', 1), ('b', 2)",
                                "INSERT INTO t VALUES ('x', 1)",
                                f"CREATE TRIGGER g AFTER {event} ON t BEGIN INSERT INTO lg VALUES ('before', new.k, new.a, NULL); {inner.format(clause)}; INSERT INTO lg VALUES ('after', new.k, new.a, NULL); END",
                            ],
                            [statement for outer in ORS for statement in ["BEGIN", "INSERT INTO lg VALUES ('kept', NULL, NULL, NULL)", fire.format(outer), LOOK, "ROLLBACK", fire.format(outer), LOOK, "DELETE FROM lg", "DELETE FROM t WHERE k <> 'x'", "DELETE FROM u", "INSERT INTO u VALUES ('a', 1), ('b', 2)"]],
                        )
                    )
    return out


class Draw:
    def __init__(self, rng):
        self.rng = rng
        pick = rng.choice
        self.keyed = rng.random() < 0.5
        self.linked = rng.random() < 0.4
        on = lambda: pick(["", "", pick(ONS)])
        null = pick(["", "", " NOT NULL", " NOT NULL DEFAULT 5"] + [f" NOT NULL{c} DEFAULT 6" for c in ONS[1:]])
        check = pick(["", "", "", " CHECK (a < 50)", " CHECK (a <> 4)"])
        unique = pick([" UNIQUE" + on(), " UNIQUE" + on(), ""])
        self.unique = bool(unique)
        primary = on()
        shapes = [f"k TEXT PRIMARY KEY{primary}, a INT{null}{check}, b TEXT, c INT{unique}", f"k TEXT, a INT{null}{check}, b TEXT, c INT{unique}, PRIMARY KEY (k, a){primary}"]
        if not self.keyed:
            shapes.append(f"k INTEGER PRIMARY KEY{primary}, a INT{null}{check}, b TEXT, c INT{unique}")
        key = pick(shapes)
        self.target = "(k, a)" if "PRIMARY KEY (k, a)" in key else "(k)"
        self.number_key = "k INTEGER" in key
        self.tables = [f"CREATE TABLE t({key}){' WITHOUT ROWID' if self.keyed else ''}"]
        self.tables.append(f"CREATE TABLE u(k TEXT PRIMARY KEY{on()}, n INT UNIQUE{on()}{pick(['', '', ' NOT NULL', ' CHECK (n < 40)'])}){pick(['', ' WITHOUT ROWID'])}")
        if self.linked:
            action = lambda: pick(["CASCADE", "CASCADE", "SET NULL", "SET DEFAULT", "NO ACTION", "RESTRICT"])
            self.tables.append(f"CREATE TABLE w(k TEXT DEFAULT 'b' REFERENCES u(k) ON DELETE {action()} ON UPDATE {action()}{pick(['', ' DEFERRABLE INITIALLY DEFERRED'])}, n INT UNIQUE{on()}, m INT{pick(['', ' NOT NULL' + on()])}{pick(['', ', UNIQUE (k)' + on()])})")
        else:
            self.tables.append(f"CREATE TABLE w(k TEXT, n INT UNIQUE{on()}, m INT{pick(['', ' NOT NULL' + on()])})")
        self.tables += ["CREATE TABLE lg(ev, x, y, z)", LOOK_DRAWN]
        self.indexed = rng.random() < 0.3
        if self.indexed:
            self.tables += rng.sample(["CREATE INDEX tb ON t(b)", "CREATE INDEX tca ON t(c, a)", "CREATE INDEX tp ON t(b) WHERE a > 2", "CREATE UNIQUE INDEX tub ON t(b) WHERE b > 'x'"], pick([1, 1, 2]))
        self.triggers = [self.trigger(f"g{i}", "t", ["w", "u"]) for i in range(pick([1, 1, 2, 3]))]
        self.triggers += [self.trigger(f"h{i}", "u", ["w"]) for i in range(pick([0, 1, 1, 2]))]
        self.triggers += [self.trigger(f"i{i}", "w", []) for i in range(pick([0, 0, 1]))]

    def key(self):
        rng = self.rng
        return str(rng.choice([1, 2, 3, 4, 7])) if self.number_key else rng.choice(["'a'", "'b'", "'c'", "'d'", "'B'", "'e'"])

    def row(self):
        rng = self.rng
        key = rng.choice([self.key()] * 12 + ["'txt'"]) if self.number_key else self.key()
        return f"({key}, {rng.choice(['1', '2', '3', '4', repr('5'), 'NULL', '60'])}, {rng.choice([repr('x'), repr('y'), repr('z'), 'NULL', '7'])}, {rng.choice(['1', '2', '3', '4', '5', '6', 'NULL'])})"

    def trigger(self, name, table, writes):
        rng = self.rng
        pick = rng.choice
        columns = {"t": "kabc", "u": "kn", "w": "knm"}[table]
        time = pick(["BEFORE", "AFTER"])
        event = pick(["INSERT", "DELETE", "UPDATE", "UPDATE"])
        text = event if event != "UPDATE" or rng.random() < 0.6 else f"UPDATE OF {pick(columns)}"
        rows = {"INSERT": ["new"], "DELETE": ["old"], "UPDATE": ["old", "new"]}[event]
        read = lambda: f"{pick(rows)}.{pick(columns)}"
        when = pick(["", "", "", "", f" WHEN {read()} > 2", f" WHEN {read()} IS NOT NULL"])
        body = [f"INSERT INTO lg VALUES ('{name}{time[0]}{event[0]}', {read()}, {read()}, {read()})"]
        for _ in range(pick([1, 1, 2, 2, 3])):
            drawn = rng.random()
            orc = pick(["", "", pick(ORS), pick(ORS), pick(ORS)])
            if "u" in writes and drawn < 0.25:
                body.append(f"INSERT{orc} INTO u VALUES ({read()}, {pick(['1', '2', '8', '45', 'NULL', read()])})")
            elif "u" in writes and drawn < 0.45:
                body.append(f"UPDATE{orc} u SET {pick(['n = n + 1', 'n = 2', 'n = ' + read(), 'k = ' + read(), 'n = NULL'])} WHERE k = {pick([repr('a'), repr('b'), read()])}")
            elif "u" in writes and drawn < 0.55:
                body.append(f"DELETE FROM u WHERE k = {pick([repr('a'), read()])}")
            elif "u" in writes and drawn < 0.67:
                body.append(f"INSERT{orc} INTO u VALUES ({read()}, {pick(['2', '3', read()])}) ON CONFLICT({pick(['k', 'k', 'n'])}) {pick(['DO UPDATE SET n = excluded.n + 10', 'DO UPDATE SET n = 2', 'DO UPDATE SET k = ' + repr('b'), 'DO NOTHING'])}")
            elif "w" in writes and drawn < 0.75:
                body.append(f"INSERT{orc} INTO w VALUES ({pick([repr('a'), repr('b'), 'NULL'])}, {pick(['1', '2', read()])}, {pick(['1', 'NULL', read()])})")
            elif "w" in writes and drawn < 0.82:
                body.append(f"UPDATE{orc} w SET {pick(['n = 2', 'm = ' + read(), 'n = n + 1', 'm = NULL'])} WHERE n = {pick(['1', '7', read()])}")
            elif drawn < 0.88:
                body.append(f"SELECT RAISE({pick(['IGNORE', 'ABORT, ' + repr('no'), 'FAIL, ' + repr('no'), 'ROLLBACK, ' + repr('no')])}) WHERE {read()} = {pick(['2', '3', repr('x'), repr('a')])}")
            else:
                body.append(f"INSERT INTO lg SELECT '{name}n', count(*), {read()}, NULL FROM {pick(['u', 'w', 't'])}")
        if rng.random() < 0.6:
            body.append(f"INSERT INTO lg VALUES ('{name}z', {read()}, NULL, NULL)")
        return f"CREATE TRIGGER {name} {time} {text} ON {table}{when} BEGIN {'; '.join(body)}; END"

    def statement(self):
        rng = self.rng
        pick = rng.choice
        orc = pick(["", "", pick(ORS), pick(ORS)])
        drawn = rng.random()
        if drawn < 0.3:
            rows = ", ".join(self.row() for _ in range(pick([1, 1, 2, 3])))
            return f"INSERT{orc} INTO t VALUES {rows}{pick(['', '', '', ' RETURNING k, a, b, c'])}"
        if drawn < 0.5:
            sets = ", ".join(rng.sample([f"b = {pick(['excluded.b', repr('q'), 't.b || excluded.b'])}", f"c = {pick(['excluded.c', 't.c + 1', '3'])}", "a = excluded.a", "a = t.a + 1"], pick([1, 1, 2])))
            target = pick([self.target, self.target, "(c)" if self.unique else self.target])
            do = pick([f"DO UPDATE SET {sets}", f"DO UPDATE SET {sets}", f"DO UPDATE SET {sets} WHERE t.a < 9", "DO NOTHING"])
            more = pick(["", "", "", f" ON CONFLICT DO {pick(['NOTHING', 'UPDATE SET b = ' + repr('m')])}"])
            rows = ", ".join(self.row() for _ in range(pick([1, 2, 3])))
            return f"INSERT{orc} INTO t VALUES {rows} ON CONFLICT{target} {do}{more}"
        if drawn < 0.75:
            sets = ", ".join(rng.sample([f"a = {pick(['a + 1', '3', '4', 'NULL', 'c'])}", f"b = {pick([repr('x'), repr('q'), 'b || a', 'NULL'])}", f"c = {pick(['c + 1', '2', 'c', 'a'])}", f"k = {pick([self.key(), 'k', self.key()])}"], pick([1, 1, 2, 3])))
            return f"UPDATE{orc} t SET {sets}{self.where(['', ' WHERE +c > 1', ' WHERE +b IS NOT NULL'])}{pick(['', '', '', ' RETURNING k, a, b, c'])}"
        if drawn < 0.85:
            return f"DELETE FROM t{self.where([' WHERE +c > 2', '', ' WHERE +b = ' + repr('x')])}"
        key = pick(["'a'", "'b'", "'c'"])
        return pick([f"DELETE FROM u WHERE k = {key}", f"UPDATE{orc} u SET k = {pick([repr('e'), repr('b')])} WHERE k = {key}", f"INSERT{orc} INTO u VALUES ({key}, {pick(['1', '2', '9'])})", f"INSERT{orc} INTO u VALUES ({key}, 2) ON CONFLICT(k) DO UPDATE SET n = excluded.n", f"INSERT{orc} INTO w VALUES ({key}, {pick(['1', '2'])}, 1)"])

    def where(self, scans):
        """The rows a statement is on. The order a scan visits rows in, which conflicts and triggers show, is the
        planner's to choose where a table has an index: there, one row by its key."""
        rng = self.rng
        if not self.indexed and not self.unique and self.target == "(k)" and rng.random() < 0.7:
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


def answer(db, sql, words):
    try:
        rows = sorted(",".join(text(value) for value in row) for row in db.execute(sql).fetchall())
    except sqlite3.IntegrityError as error:
        return f"ERROR {error}" if words else "ERROR"
    except sqlite3.Error:
        return "ERROR"
    return f"{len(rows)}:{';'.join(rows)}"


def ask(db, sql, out, words=True):
    out.append(f"q\t{sql}\t{answer(db, sql, words)}")


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 300
    seed = sys.argv[2] if len(sys.argv) > 2 else "C22"
    print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_trigger_conflicts.py {count} {seed}")
    out = []
    for name, fill, statements in FOUND + nested() + ladder():
        db = sqlite3.connect(":memory:", isolation_level=None)
        out.append(f"db\t{name}")
        for statement in fill:
            db.execute(statement)
            out.append(f"sql\t{statement}")
        for statement in statements:
            ask(db, statement, out)
        ask(db, "PRAGMA integrity_check", out)
    made = refused = 0
    while made < count:
        rng = random.Random(f"{seed}-{made}-{refused}")
        draw = Draw(rng)
        db = sqlite3.connect(":memory:", isolation_level=None)
        fill = (["PRAGMA foreign_keys = ON"] if draw.linked else []) + draw.tables + ["INSERT INTO u VALUES ('a', 1), ('b', 2), ('z', 7)", "INSERT INTO w VALUES ('a', 1, 1), ('b', 7, 2)"]
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
        scope = rng.choice(["", "", "BEGIN", "BEGIN", "SAVEPOINT"])
        if scope == "BEGIN":
            ask(db, "BEGIN", out)
        for _ in range(rng.choice([4, 5, 6])):
            if scope == "SAVEPOINT":
                ask(db, "SAVEPOINT s", out)
            ask(db, draw.statement(), out, not draw.linked)
            ask(db, LOOK, out)
            if scope == "SAVEPOINT":
                if rng.random() < 0.5:
                    ask(db, "ROLLBACK TO s", out)
                    ask(db, LOOK, out)
                ask(db, "RELEASE s", out)
            ask(db, "DELETE FROM lg", out)
        if scope == "BEGIN":
            ask(db, rng.choice(["COMMIT", "COMMIT", "ROLLBACK"]), out)
            ask(db, LOOK, out)
        ask(db, "PRAGMA integrity_check", out)
    print("\n".join(out))
    print(f"# {made} drawn, {refused} SQLite refused", file=sys.stderr)


main()
