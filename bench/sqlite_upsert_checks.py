#!/usr/bin/env python3
"""crates/engine/tests/fixtures/upsert_checks.tsv: generated UPSERTs whose new row breaks a constraint, the triggers
that run before it is found to, and what SQLite leaves behind (AC-T1.8.6; lane C25).

A case is one table (with a rowid, an alias of it or not, or WITHOUT ROWID; STRICT or not) whose columns are NOT NULL,
CHECKed (one column's and two columns' checks), UNIQUE, a foreign key's child and its parent, each constraint with a
conflict clause or without; its triggers, BEFORE and AFTER each event, which log NEW and OLD and some of which RAISE or
write the row they fire for; an index, plain, partial or on an expression; then UPSERTs of one row and several, under
each OR clause, with one ON CONFLICT or two, beside UPDATEs, plain INSERTs and DELETEs. The refusal's text is held: it
names the constraint SQLite met first. After each refused statement a pragma's table function is read, and
at the end the rows of every table and the integrity check.

Left out: which of two UNIQUE constraints a row meets first (no document says; a refusal is held as UNIQUE's), an
expression that raises (SQLite evaluates a constant one before the statement's first row), what Amphora has no table
function for (pragma_foreign_key_check, pragma_integrity_check('t')) or lists otherwise (pragma_index_list of a WITHOUT
ROWID table), a STRICT table's CHECK that compares TEXT with a number (Amphora refuses the table), a generated column
(production doesn't make one), and two foreign key causes of plain INSERTs: a row with no parent (refused at once, SQLite
at the statement's end) and OR REPLACE of a parent's row on a WITHOUT ROWID table (refused, SQLite takes the new row).

usage: bench/sqlite_upsert_checks.py COUNT SEED > crates/engine/tests/fixtures/upsert_checks.tsv   (SQLite 3.54.0 in Python)
"""
import random
import sqlite3
import struct
import sys

CLAUSES = ["", "", "", "", " ON CONFLICT ABORT", " ON CONFLICT FAIL", " ON CONFLICT IGNORE", " ON CONFLICT REPLACE", " ON CONFLICT ROLLBACK"]
FUNCTIONS = ["pragma_integrity_check", "pragma_integrity_check", "pragma_integrity_check", "pragma_quick_check", "pragma_table_info('t')",
             "pragma_foreign_key_list('ch')", "pragma_table_xinfo('t')"]


class Case:
    def __init__(self, rng):
        self.rng = rng
        pick = rng.choice
        self.strict = rng.random() < 0.4
        self.without_rowid = rng.random() < 0.35
        self.fk = rng.random() < 0.3
        self.unchecked = False
        self.ddl = []
        if self.fk:
            self.ddl.append("CREATE TABLE p(x INT PRIMARY KEY)")
        key = pick(["INTEGER PRIMARY KEY"] * 4 + ["INT PRIMARY KEY", "TEXT PRIMARY KEY", "INT UNIQUE"])
        if self.without_rowid:
            key = pick(["INTEGER PRIMARY KEY", "INT PRIMARY KEY", "TEXT PRIMARY KEY"])
        self.key = key
        self.alias = key == "INTEGER PRIMARY KEY" and not self.without_rowid
        ty = "INT" if self.strict else pick(["INT", "INT", "", "NUMERIC"])
        self.ty = ty
        defs = [f"id {key}"]
        self.cols = ["id"]

        def column(name, text):
            defs.append(" ".join(text.split()))
            self.cols.append(name)

        clause = lambda: pick(CLAUSES) if rng.random() < 0.5 else ""
        column("n", f"n {ty} NOT NULL{clause()}" + (" DEFAULT 4" if rng.random() < 0.4 else ""))
        column("c", f"c {ty} CHECK (c > 0)")
        if rng.random() < 0.6:
            column("d", f"d {ty}" + pick(["", "", f" NOT NULL{clause()}", " CHECK (d <> 7)", " CHECK (d IS NULL OR d < 50)"]))
        column("u", f"u {ty} UNIQUE{clause()}")
        if self.fk:
            column("f", f"f {ty} " + pick(["", "NOT NULL "]) + "REFERENCES p(x)" + pick(["", " ON UPDATE CASCADE", " ON DELETE CASCADE", " DEFERRABLE INITIALLY DEFERRED"]))
        others = [x for x in self.cols if x not in ("id", "f")]
        if rng.random() < 0.5:
            a, b = rng.sample(others, 2)
            defs.append(pick(["", "CONSTRAINT pair "]) + f"CHECK ({a} {pick(['<', '<>', '<='])} {b} + {pick(['0', '20', '20'])})")
        if rng.random() < 0.15:
            defs.append(pick((["CHECK (id <> 9)"] if "TEXT" not in key else []) + ["CHECK (length(id) < 3)"] + ([] if self.without_rowid else ["CHECK (rowid < 90)"])))
        options = ["STRICT"] * self.strict + ["WITHOUT ROWID"] * self.without_rowid
        self.ddl.append(f"CREATE TABLE t({', '.join(defs)}) {', '.join(options)}".strip())
        if self.fk:
            self.ddl.append("CREATE TABLE ch(y INT REFERENCES t(u)" + pick(["", " ON UPDATE CASCADE", " ON UPDATE SET NULL", " ON UPDATE RESTRICT"]) + pick(["", " ON DELETE CASCADE", " ON DELETE SET NULL"]) + ")")
        if rng.random() < 0.35:
            self.ddl.append(pick([f"CREATE INDEX i ON t({pick(others)})", f"CREATE INDEX i ON t({pick(others)} + {pick(others)})", f"CREATE INDEX i ON t({pick(others)}) WHERE {pick(others)} > 2",
                                  f"CREATE UNIQUE INDEX i ON t({pick(others)}, {pick(others + ['id'])})"]))
        self.ddl.append("CREATE TABLE lg(e, " + ", ".join(f"v{i}" for i in range(len(self.cols) * 2)) + ")")
        for at, event in enumerate(rng.sample(["BEFORE UPDATE"] * 4 + ["BEFORE UPDATE OF", "AFTER UPDATE", "BEFORE INSERT", "AFTER INSERT", "BEFORE DELETE", "AFTER DELETE"], rng.randint(1, 4))):
            self.ddl.append(self.trigger(at, event))

    def trigger(self, at, event):
        rng = self.rng
        new, old = "DELETE" not in event, "INSERT" not in event
        seen = [f"new.{x}" for x in self.cols if new] + [f"old.{x}" for x in self.cols if old]
        # before an INSERT without its rowid, new.rowid is -1 (SQLite's documents call it undefined)
        if event == "BEFORE INSERT" and self.alias:
            seen.remove("new.id")
        values = [f"quote({row}.{x})" if on and f"{row}.{x}" in seen else "0" for row, on in (("new", new), ("old", old)) for x in self.cols]
        of = " " + ", ".join(rng.sample(self.cols, rng.randint(1, 2))) if event.endswith("OF") else ""
        when = rng.choice([""] * 4 + [f" WHEN {rng.choice(seen)} {rng.choice(['IS NULL', '> 1', 'IS NOT NULL'])}"])
        act = ""
        if rng.random() < 0.55:
            how = rng.choice(["IGNORE", "IGNORE", "ABORT, 'no'", "ABORT, 'no'", "FAIL, 'no'", "ROLLBACK, 'no'"])
            act = f" SELECT RAISE({how}) WHERE {rng.choice(seen)} {rng.choice(['IS NULL', 'IS NOT NULL', '< 1', '> 1', '= 5'])};"
        elif event.startswith(("BEFORE UPDATE", "AFTER UPDATE")) and rng.random() < 0.4:
            x = rng.choice([x for x in self.cols if x not in ("id", "u", "f")])
            act = rng.choice([f" UPDATE t SET {x} = {rng.choice(['3', '30', 'old.' + x])} WHERE id = old.id AND {x} IS NOT {rng.choice(['3', '30'])};",
                              f" UPDATE OR IGNORE t SET {x} = 2 WHERE id = old.id;"] + [" DELETE FROM t WHERE id = old.id;"] * (rng.random() < 0.3))
        return f"CREATE TRIGGER g{at} {event}{of} ON t{when} BEGIN INSERT INTO lg VALUES ('{event}', {', '.join(values)});{act} END"

    def value(self, col, bad=0.25, inserted=False):
        rng = self.rng
        if col == "id":
            return rng.choice(["1", "2", "3", "'2'", "9", "95"] + ["NULL"] * (rng.random() < 0.2)) if "TEXT" not in self.key else rng.choice(["'a'", "'b'", "'c'", "'long'"])
        if col == "f":
            return rng.choice(["1", "1", "2", "NULL"] + ["8"] * (not inserted))
        if col == "u":
            return rng.choice(["1", "2", "3", "4", "5", "NULL"])
        if rng.random() < bad:
            return rng.choice(["NULL", "NULL", "-1", "0", "7", "60", "'x'", "1.5"] if rng.random() < 0.9 else ["x'41'"])
        return rng.choice(["1", "2", "3", "5", "10", "'6'", "8.0"])

    def set_value(self, col):
        rng = self.rng
        if rng.random() < 0.35 and col not in ("id",):
            return rng.choice([f"excluded.{col}", f"excluded.{rng.choice(self.cols)}", f"{col} + 1", f"t.{col} - 5", f"t.{rng.choice(self.cols)}", f"nullif(excluded.{col}, {rng.choice(['1', '5'])})", "(SELECT max(u) FROM t AS x)",
                               f"(SELECT excluded.{rng.choice(self.cols)})"])
        return self.value(col, 0.45)

    def sets(self):
        rng = self.rng
        cols = rng.sample(self.cols[1:], rng.choice([1, 1, 2, 2, 3]))
        if rng.random() < 0.12:
            cols.append("id")
        if rng.random() < 0.06:
            cols.append(cols[0])  # the last assignment is the one
        if len(cols) == 2 and cols[0] != cols[1] and rng.random() < 0.15:
            return f"({cols[0]}, {cols[1]}) = ({self.set_value(cols[0])}, {self.set_value(cols[1])})"
        return ", ".join(f"{x} = {self.set_value(x)}" for x in cols)

    def upsert(self):
        rng = self.rng
        rows = ", ".join("(" + ", ".join(self.value(x, 0.08, True) for x in self.cols) + ")" for _ in range(rng.choice([1, 1, 2, 3])))
        head = rng.choice(["INSERT"] * 5 + ["INSERT OR ABORT", "INSERT OR FAIL", "INSERT OR IGNORE", "INSERT OR ROLLBACK"] + ["INSERT OR REPLACE"] * (not (self.fk and self.without_rowid)))
        where = lambda: rng.choice([""] * 4 + [f" WHERE {rng.choice(['t.', 'excluded.'])}{rng.choice(self.cols)} {rng.choice(['IS NOT NULL', '> 1', '< 5'])}"])
        arms = [f" ON CONFLICT(id) DO UPDATE SET {self.sets()}{where()}", f" ON CONFLICT(u) DO UPDATE SET {self.sets()}{where()}"]
        rng.shuffle(arms)
        arms = arms[: rng.choice([1, 2, 2])]
        if rng.random() < 0.1:
            arms[-1] = rng.choice([" ON CONFLICT DO NOTHING", f" ON CONFLICT DO UPDATE SET {self.sets()}"])
        returning = rng.choice([""] * 6 + [" RETURNING " + ", ".join(self.cols)])
        return f"{head} INTO t VALUES {rows}{''.join(arms)}{returning}"

    def statement(self):
        rng = self.rng
        drawn = rng.random()
        if drawn < 0.75:
            return self.upsert()
        where = f" WHERE id = {self.value('id')}" if rng.random() < 0.5 or self.without_rowid else ""
        if drawn < 0.9:
            cols = rng.sample(self.cols[1:], rng.choice([1, 2]))
            return f"UPDATE{rng.choice([''] * 3 + [' OR IGNORE', ' OR FAIL', ' OR REPLACE'])} t SET " + ", ".join(f"{x} = {self.value(x, 0.4)}" for x in cols) + where
        if drawn < 0.95:
            return f"DELETE FROM t{where}"
        return f"INSERT{rng.choice(['', ' OR IGNORE'] + [' OR REPLACE'] * (not (self.fk and self.without_rowid)))} INTO t VALUES (" + ", ".join(self.value(x, 0.25, True) for x in self.cols) + ")"

    def text(self):
        rng = self.rng
        out = [("sql", s) for s in self.ddl]
        if self.fk:
            out.append(("do", "PRAGMA foreign_keys = ON"))
            out.append(("do", "INSERT INTO p VALUES (1), (2), (3)"))
        keys = ["1", "2", "3"] if "TEXT" not in self.key else ["'a'", "'b'", "'c'"]
        for at, key in enumerate(keys[: rng.choice([2, 3])]):
            good = {"id": key, "u": str(at + 1), "f": "1", "n": str(at + 1), "c": str(at + 2), "d": str(at + 3)}
            out.append(("do", f"INSERT INTO t VALUES ({', '.join(good[x] for x in self.cols)})"))
        if self.fk:
            out.append(("do", "INSERT INTO ch VALUES (1), (NULL)"))
        if rng.random() < 0.06:
            out += [("do", "PRAGMA ignore_check_constraints = ON"), ("do", "UPDATE t SET c = -4 WHERE u = 1"), ("do", "PRAGMA ignore_check_constraints = OFF")]
            self.unchecked = True  # integrity_check names a row for each CHECK it breaks, SQLite's once
        for _ in range(rng.choice([1, 2, 2, 3])):
            out.append(("do", self.statement()))
        return out

    def last(self):
        rows = [("rows", f"SELECT {'' if self.without_rowid else 'rowid, '}{', '.join(self.cols)} FROM t"), ("rows", "SELECT rowid, * FROM lg")]
        if self.fk:
            rows += [("rows", "SELECT * FROM p"), ("rows", "SELECT rowid, * FROM ch")]
        return rows if self.unchecked else rows + [("rows", "SELECT * FROM pragma_integrity_check")]


def cell(v):
    if v is None:
        return "NULL"
    if isinstance(v, float):
        return "r" + struct.pack(">d", v).hex()
    if isinstance(v, bytes):
        return f"x'{v.hex()}'"
    return f"'{v}'" if isinstance(v, str) else str(v)


def answer(case, rng):
    """SQLite's outcome of each statement and its rows; None when a CREATE is refused."""
    conn = sqlite3.connect(":memory:", isolation_level=None)
    out = []

    def ask(kind, sql):
        try:
            rows = conn.execute(sql).fetchall()
        except sqlite3.Error as error:
            if kind == "sql":
                return False
            text = str(error)
            out.append((kind, sql, "refused: " + ("UNIQUE constraint failed: " if text.startswith("UNIQUE") else text)))
            return kind == "do"
        if kind == "sql":
            out.append((kind, sql))
        elif kind == "rows" or rows:
            out.append(("rows" if kind == "rows" else "do", sql, " / ".join(sorted(",".join(cell(v) for v in row) for row in rows)) if kind == "rows" else "ok"))
        else:
            out.append((kind, sql, "ok"))
        return True

    for kind, sql in case.text():
        if not ask(kind, sql):
            if kind == "sql":
                return None
        if out[-1][-1].startswith("refused") and kind == "do":
            ask("rows", "SELECT * FROM " + (rng.choice(FUNCTIONS) if not case.unchecked else "pragma_table_info('t')"))
    for kind, sql in case.last():
        ask(kind, sql)
    return out


def main():
    count, seed = int(sys.argv[1]), int(sys.argv[2])
    rng = random.Random(seed)
    made, lines = 0, []
    while made < count:
        case = Case(rng)
        out = answer(case, rng)
        if out is None or not any(x[0] == "do" and "ON CONFLICT" in x[1] and "DO UPDATE" in x[1] for x in out):
            continue
        made += 1
        lines.append("case")
        lines.extend("\t".join(line) for line in out)
    print(f"# bench/sqlite_upsert_checks.py {count} {seed}, SQLite {sqlite3.sqlite_version}: a case's statements, each with its outcome, and the rows it leaves.")
    print("\n".join(lines))


main()
