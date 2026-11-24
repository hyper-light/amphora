#!/usr/bin/env python3
"""crates/engine/tests/fixtures/dml_rows.tsv: generated INSERTs, UPSERTs, REPLACEs and UPDATEs, and the rows SQLite
stores after them (AC-T1.8.6; the oracles' findings DA and CV).

A case is one table (a rowid alias in each form it is written, listed in the INSERT or not; a key that is no alias; no
key; WITHOUT ROWID; STRICT or not; each column type; generated columns), its triggers, which log each NEW and OLD value
with its type, rows put in, then statements. `excluded.*` is read for each column, the rowid and its aliases.

usage: bench/sqlite_dml_rows.py COUNT SEED > crates/engine/tests/fixtures/dml_rows.tsv   (SQLite 3.54.0 in Python)

Left out, each a difference of another cause (bench/records/2026-11-24-adacachy2-oracle-c11.md): a comparison in a
trigger of a column with a collation (CX), STRICT beside WITHOUT ROWID (refused), triggers on WITHOUT ROWID tables
(refused), which of two unique constraints an UPSERT with no target meets first (no document says), an INSERT OR IGNORE
of NULL into a STRICT key that is no rowid alias, a subquery in DO UPDATE (refused), and the text of a REAL past 15
digits (CK).
"""
import random
import sqlite3
import struct
import sys

STRICT_TYPES = ["INT", "INTEGER", "REAL", "TEXT", "BLOB", "ANY"]
PLAIN_TYPES = ["INT", "INTEGER", "REAL", "TEXT", "BLOB", "NUMERIC", "", "VARCHAR(10)", "DOUBLE", "BOOLEAN", "DATE"]
# What a STRICT column takes, as its type and as what its affinity converts to it.
TAKES = {
    "INT": ["7", "0", "-3", "100", "'12'", "' 8 '", "3.0", "'4.0'", "9007199254740993", "'1e2'"],
    "REAL": ["7", "0", "-3", "100", "1.5", "-2.25", "'12'", "'2.5'", "' 8 '", "100.0", "'1e2'"],
    "TEXT": ["'a'", "'ABC'", "'7'", "''", "7", "1.5", "100", "-2.25"],
    "BLOB": ["x''", "x'41'", "x'3700'"],
    "ANY": ["7", "1.5", "'7'", "'a'", "x'41'", "100", "'1.0'", "3.0"],
}
TAKES["INTEGER"] = TAKES["INT"]
ANY_VALUE = ["NULL", "0", "1", "-1", "7", "100", "9007199254740993", "0.0", "1.0", "1.5", "-2.25", "100.0", "''", "'7'", "' 8 '", "'7.0'", "'1.5'",
             "'1e2'", "'abc'", "'ABC'", "'x7'", "x''", "x'41'", "x'37'", "TRUE"]
KEYS = ["1", "2", "3", "5", "10", "'2'", "3.0", "' 5 '", "'10'", "NULL"]


class Case:
    def __init__(self, rng):
        self.rng = rng
        pick = rng.choice
        self.strict = rng.random() < 0.55
        self.without_rowid = not self.strict and rng.random() < 0.15
        types = STRICT_TYPES if self.strict else PLAIN_TYPES
        key = pick(["alias"] * 6 + ["alias table"] * 2 + ["alias auto", "int", "text", "none", "desc"])
        if self.without_rowid:
            key = pick(["int", "text", "alias", "two"])
        self.key = key
        n = rng.randint(1, 4)
        self.cols = []  # name, type, written in an INSERT
        defs = []
        for i in range(n):
            ty = pick(types)
            extra = ""
            if rng.random() < 0.15:
                extra += " DEFAULT " + pick(TAKES[ty] if self.strict else ["7", "'1.5'", "'abc'", "(abs(-3))", "3.0"])
            if ty in ("TEXT", "VARCHAR(10)") and rng.random() < 0.25:
                extra += " COLLATE " + pick(["NOCASE", "RTRIM"])
            if rng.random() < 0.1:
                extra += " NOT NULL"
            self.cols.append((f"c{i}", ty))
            defs.append(f"c{i} {ty}{extra}".replace("  ", " "))
        self.unique = []
        if key in ("alias", "alias table", "alias auto", "desc"):
            id_type = "INTEGER"
        elif key == "int":
            id_type = "INT"
        elif key == "text":
            id_type = "TEXT"
        else:
            id_type = None
        self.alias = None
        tail = []
        if id_type:
            at = rng.randint(0, n) if rng.random() < 0.4 else 0
            name = pick(["id", "id", "id", "k", "Key"])
            one = {"alias table": "", "alias auto": " PRIMARY KEY AUTOINCREMENT", "desc": " PRIMARY KEY DESC"}.get(key, " PRIMARY KEY")
            if key == "alias table":
                tail.append(f"PRIMARY KEY({name})")
            self.cols.insert(at, (name, id_type))
            defs.insert(at, f"{name} {id_type}{one}")
            self.unique.append([name])
            self.pk = name
            if key.startswith("alias") and not self.without_rowid:
                self.alias = name
        elif key == "two":
            self.cols.append(("p", pick(types)))
            defs.append(f"p {self.cols[-1][1]}".strip())
            self.cols.append(("q", pick(types)))
            defs.append(f"q {self.cols[-1][1]}".strip())
            tail.append("PRIMARY KEY(p, q)")
            self.unique.append(["p", "q"])
            self.pk = None
        else:
            self.pk = None
        if key == "none" or rng.random() < 0.4:
            ty = pick(["INT", "TEXT", "REAL"] if self.strict else ["INT", "TEXT", "REAL", "NUMERIC", ""])
            self.cols.append(("u", ty))
            defs.append(f"u {ty}".strip() + " UNIQUE" + (" COLLATE NOCASE" if ty == "TEXT" and rng.random() < 0.3 else ""))
            self.unique.append(["u"])
        self.gen = []
        if not self.without_rowid and rng.random() < 0.2:
            over = pick([c for c, _ in self.cols])
            body = pick([f"{over}", f"typeof({over})", f"{over} || 'z'", f"coalesce({over}, 0) + 1"])
            ty = pick(["INT", "REAL", "TEXT", "ANY"] if self.strict else types)
            if self.strict:
                body, ty = pick([(f"typeof({over})", "TEXT"), (f"{over} IS NULL", "INT"), (over, "ANY"), (f"quote({over})", "TEXT")])
            self.gen.append(("g", ty))
            defs.insert(rng.randint(0, len(defs)), f"g {ty} AS ({body}) VIRTUAL".replace("  ", " "))
        order = [d.split()[0] for d in defs]
        self.all = order  # in the table's order
        self.types = dict(self.cols + self.gen)
        self.written = [c for c in order if c != "g"]
        options = "STRICT" if self.strict else "WITHOUT ROWID" if self.without_rowid else ""
        self.ddl = [f"CREATE TABLE t({', '.join(defs + tail)}) {options}".strip()]
        if rng.random() < 0.15 and not self.without_rowid:
            self.ddl.append(f"CREATE INDEX i ON t({pick(self.written)})")
        self.triggers = 0
        if not self.without_rowid and rng.random() < 0.6:
            cells = [f"{w}.{c}" for c in self.all for w in ("X",)]
            self.ddl.append("CREATE TABLE lg(e, " + ", ".join(f"v{i}, w{i}" for i in range(len(cells) + 1)) + ", " + ", ".join(f"o{i}" for i in range(len(cells) + 1)) + ")")
            events = rng.sample(["BEFORE INSERT", "AFTER INSERT", "BEFORE UPDATE", "AFTER UPDATE", "AFTER UPDATE OF", "BEFORE DELETE", "AFTER DELETE"], rng.randint(1, 4))
            for event in events:
                new = event.split()[1] != "DELETE"
                old = event.split()[1] != "INSERT"
                # before an INSERT without its rowid, new.rowid is -1 (SQLite's documents call it undefined)
                names = self.all + ["rowid"]
                values = ", ".join((f"new.{c}, typeof(new.{c})" if new else "0, 0") for c in names)
                olds = ", ".join((f"quote(old.{c})" if old else "0") for c in names)
                of = f" {pick(self.written)}" if event.endswith("OF") else ""
                self.ddl.append(f"CREATE TRIGGER g{self.triggers} {event}{of} ON t BEGIN INSERT INTO lg VALUES ('{event}', {values}, {olds}); END")
                self.triggers += 1

    def value(self, col):
        rng = self.rng
        ty = self.types[col]
        if col == self.pk:
            if self.key in ("alias", "alias table", "alias auto"):
                v = rng.choice(KEYS)
                return "3" if v == "NULL" and self.without_rowid else v
            if self.strict:
                return rng.choice(["1", "2", "3", "'2'", "3.0"] if ty in ("INT", "INTEGER") else ["'a'", "'b'", "1", "'1'"])
            return rng.choice(["1", "2", "'2'", "2.0", "'a'", "x'41'"] if ty in ("INT", "TEXT") else KEYS[:9])
        if col in ("p", "q", "u"):
            return rng.choice(["1", "'1'", "2", "1.0"] if ty not in ("TEXT", "BLOB") or not self.strict else ["'a'", "'A'", "'b'", "1"] if ty == "TEXT" else ["x'41'"])
        if self.strict and rng.random() < 0.93:
            return "NULL" if rng.random() < 0.08 else rng.choice(TAKES[ty])
        return rng.choice(ANY_VALUE)

    def listed(self):
        """The columns an INSERT names: None for all of them in the table's order."""
        rng = self.rng
        if rng.random() < 0.6:
            return None
        named = {self.pk} if self.strict and not self.alias else set()
        cols = [c for c in self.written if c in named or rng.random() < 0.8] or self.written[:]
        if rng.random() < 0.3:
            rng.shuffle(cols)
        if not self.without_rowid and not (self.alias in cols) and "rowid" not in cols and rng.random() < 0.25:
            cols.append(rng.choice(["rowid", "oid", "_rowid_"]))
        return cols

    def rows(self, cols):
        rng = self.rng
        many = rng.choice([1, 1, 1, 2, 3])

        def cell(c):
            return rng.choice(KEYS) if c in ("rowid", "oid", "_rowid_") else self.value(c)

        out = []
        for _ in range(many):
            out.append("(" + ", ".join(cell(c) for c in cols) + ")")
        if rng.random() < 0.15:
            return "SELECT " + " UNION ALL SELECT ".join(row[1:-1] for row in out) + (" WHERE true" if True else "")
        return "VALUES " + ", ".join(out)

    def ref(self):
        rng = self.rng
        names = self.all[:]
        if not self.without_rowid:
            names += ["rowid", rng.choice(["oid", "_rowid_"])]
        if self.pk and rng.random() < 0.35:
            return self.pk
        return rng.choice(names)

    def expr(self):
        rng = self.rng
        e, c = "excluded." + self.ref(), self.ref()
        own = rng.choice([f"t.{c}", c])
        lit = rng.choice(["1", "'7'", "'a'", "1.5", "NULL", "10", "'A'"])
        return rng.choice([
            e, e, e, e, e,
            f"typeof({e})", f"quote({e})", f"coalesce({e}, {own})", f"coalesce({own}, {e})", f"{e} + 1", f"{own} + {e}", f"-{e}",
            f"{e} = {own}", f"{e} IS {own}", f"{e} = {lit}", f"{own} = {e}", f"{e} < {lit}", f"{e} > {own}",
            f"CASE WHEN {e} > 2 THEN {e} ELSE {own} END", f"iif({e} IS NULL, {own}, {e})", f"CAST({e} AS TEXT)", f"hex({e})", f"{e} IN (1, 2, '2', 10)",
            f"max({e}, coalesce({own}, 0))", f"length({e})", lit, own,
        ])

    def sets(self):
        rng = self.rng
        targets = [c for c in self.written if c not in ("p", "q")]
        if rng.random() < 0.85 and len(targets) > 1:
            targets = [c for c in targets if c != self.pk and c != "u"] or targets
        chosen = rng.sample(targets, min(len(targets), rng.choice([1, 1, 2, 3])))
        if len(chosen) == 2 and rng.random() < 0.15:
            return f"({chosen[0]}, {chosen[1]}) = ({self.expr()}, {self.expr()})"
        return ", ".join(f"{c} = {self.expr()}" for c in chosen)

    def on_conflict(self):
        rng = self.rng
        if not self.unique:
            return " ON CONFLICT DO NOTHING"
        target = rng.choice(self.unique)
        text = f" ON CONFLICT({', '.join(target)})" if len(self.unique) > 1 or rng.random() < 0.85 else " ON CONFLICT"
        if rng.random() < 0.15:
            return text + " DO NOTHING"
        text += " DO UPDATE SET " + self.sets()
        if rng.random() < 0.3:
            text += " WHERE " + rng.choice([f"excluded.{self.ref()} {op} {x}" for op, x in (("=", "2"), (">", "1"), ("IS NOT", "NULL"), ("IS", f"t.{self.ref()}"), ("<>", "'a'"))])
        return text

    def statement(self):
        rng = self.rng
        kind = rng.choice(["insert"] * 2 + ["upsert"] * 6 + ["replace", "or ignore", "or replace", "update"])
        if kind == "update":
            cols = rng.sample(self.written, min(len(self.written), rng.choice([1, 1, 2])))
            where = rng.choice(["", f" WHERE {self.written[0]} IS NOT NULL", f" WHERE {rng.choice(self.written)} = {rng.choice(['1', '2', '10'])}"])
            return "UPDATE t SET " + ", ".join(f"{c} = {self.value(c)}" for c in cols) + where
        cols = self.listed()
        head = {"insert": "INSERT", "upsert": rng.choice(["INSERT"] * 8 + ["INSERT OR IGNORE", "INSERT OR REPLACE"]), "replace": "REPLACE", "or ignore": "INSERT OR IGNORE",
                "or replace": "INSERT OR REPLACE"}[kind]
        if cols is None and kind == "insert" and rng.random() < 0.05:
            return "INSERT INTO t DEFAULT VALUES"
        sql = f"{head} INTO t" + (f"({', '.join(cols)})" if cols else "") + " " + self.rows(cols or self.written)
        if kind == "upsert":
            sql += self.on_conflict()
        return sql

    def text(self):
        out = [("sql", s) for s in self.ddl]
        for _ in range(self.rng.choice([1, 1, 2])):
            out.append(("do", "INSERT INTO t VALUES (" + ", ".join(self.value(c) for c in self.written) + ")"))
        for _ in range(self.rng.choice([1, 2, 2, 3])):
            out.append(("do", self.statement()))
        out.append(("rows", "SELECT * FROM t" if self.without_rowid else "SELECT rowid, * FROM t"))
        if self.triggers:
            out.append(("rows", "SELECT rowid, * FROM lg"))
        return out


def cell(v):
    if v is None:
        return "NULL"
    if isinstance(v, float):
        return "r" + struct.pack(">d", v).hex()
    if isinstance(v, bytes):
        return f"x'{v.hex()}'"
    return f"'{v}'" if isinstance(v, str) else str(v)


def answer(case):
    """SQLite's outcome of each statement and its rows; None when a CREATE is refused."""
    conn = sqlite3.connect(":memory:", isolation_level=None)
    out = []
    for kind, sql in case:
        try:
            rows = conn.execute(sql).fetchall()
            got = "ok"
        except sqlite3.Error as error:
            if kind == "sql":
                return None
            rows, got = [], f"refused: {error}"
        if kind == "rows":
            got = " / ".join(sorted(",".join(cell(v) for v in row) for row in rows))
        out.append((kind, sql) if kind == "sql" else (kind, sql, got))
    return out


def main():
    count, seed = int(sys.argv[1]), int(sys.argv[2])
    rng = random.Random(seed)
    print(f"# bench/sqlite_dml_rows.py {count} {seed}, SQLite {sqlite3.sqlite_version}: a case's statements, each with its outcome, and the rows it leaves")
    made = 0
    while made < count:
        case = Case(rng)
        out = answer(case.text())
        if out is None or not any(kind == "do" and got == "ok" for kind, _, got in (x for x in out if len(x) == 3)):
            continue
        made += 1
        print("case")
        for line in out:
            print("\t".join(line))


main()
