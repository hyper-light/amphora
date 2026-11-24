"""Foreign keys and triggers leave what SQLite leaves (lane C26, from C22's sqlite_trigger_conflicts.py).
Foreign keys under every action (NO ACTION, RESTRICT, CASCADE, SET NULL, SET DEFAULT, on DELETE and on
UPDATE), immediate and deferred, of one column and of two, to a rowid, a primary key and a UNIQUE key, from
and to WITHOUT ROWID tables, from a table to itself, two tables to each other, and three tables deep; defaults
that are the key that goes, another and none; NULL keys; triggers of every time and event on all of them, under
every conflict clause, that write each of them, themselves among them; PRAGMA foreign_keys and
recursive_triggers on and off; alone, inside a transaction, and inside savepoints. Writes each statement with
whether SQLite itself takes it (its words, when it doesn't) and what every table and the triggers' log hold after
it (the sqlite3 library Python links, printed first) to crates/engine/tests/fixtures/foreign_keys.tsv, which
tests/windows_and_triggers.rs holds Amphora to.

The rules (sqlite.org/foreignkeys.html, sqlite.org/lang_createtrigger.html, sqlite.org/pragma.html): an
immediate foreign key is checked where its statement ends and a deferred one where its transaction does, whatever
programs run between; RESTRICT is checked at once; with recursive_triggers a trigger may fire itself and REPLACE
fires the DELETE triggers of the rows it removes; a trigger's statement sees its rows in the order of their key.

Each line is `db <name>`, `sql <statement>` (the database's tables, rows and triggers) or `q <statement>
<answer>`: the answer's rows sorted, each row's values joined by commas, a NULL written NULL, the rows joined by
semicolons, or ERROR for a statement SQLite refuses, with its words when a constraint refused it.
usage: python3 bench/sqlite_foreign_keys.py [databases [seed]] > crates/engine/tests/fixtures/foreign_keys.tsv"""
import os
import random
import re
import sqlite3
import sys

CONFLICTS = ["ABORT", "FAIL", "IGNORE", "REPLACE", "ROLLBACK"]
ORS = [""] + [f" OR {c}" for c in CONFLICTS]
ACTIONS = ["NO ACTION", "RESTRICT", "CASCADE", "SET NULL", "SET DEFAULT"]
DEFERRED = " DEFERRABLE INITIALLY DEFERRED"
LOOK = "SELECT * FROM look"
RECURSIVE = bool(os.environ.get("C26_RECURSIVE"))


def look(tables):
    """Every table's rows, four columns wide."""
    parts = [f"SELECT '{name}', {', '.join([f'quote({c})' for c in columns] + [repr('')] * (4 - len(columns)))} FROM {name}" for name, columns in tables]
    return "CREATE VIEW look AS " + " UNION ALL ".join(parts)


def found():
    """What the lanes found Amphora failing, each in its own form."""
    on = ["PRAGMA foreign_keys = ON"]
    out = []
    # C22: ON DELETE SET DEFAULT whose default is the key that goes
    for keyed in ["", " WITHOUT ROWID"]:
        for kind, key, other in [("TEXT", "'b'", "'c'"), ("INTEGER", "2", "3"), ("INT", "2", "3")]:
            for late in ["", DEFERRED]:
                for default in [key, other, "NULL", "'zz'" if kind == "TEXT" else "99"]:
                    asks = [f"DELETE FROM u WHERE k = {key}", "SELECT * FROM u", "SELECT * FROM w", f"UPDATE u SET k = {'9' if kind != 'TEXT' else repr('q')} WHERE k = {key}", "SELECT * FROM u", "SELECT * FROM w", f"REPLACE INTO u VALUES ({key}, 9)", "SELECT * FROM u", "SELECT * FROM w", f"UPDATE OR REPLACE u SET k = {key} WHERE k = {other}", "SELECT * FROM u", "SELECT * FROM w"]
                    out.append((f"SET DEFAULT of a key that goes: {kind}{keyed}{late} {default}", on + [f"CREATE TABLE u(k {kind} PRIMARY KEY, n){keyed}", f"CREATE TABLE w(k {kind} DEFAULT {default} REFERENCES u(k) ON DELETE SET DEFAULT ON UPDATE SET DEFAULT{late}, m)", f"INSERT INTO u VALUES ({key}, 1), ({other}, 2)", f"INSERT INTO w VALUES ({key}, 1), ({key}, 2), (NULL, 3)"], asks + ["BEGIN"] + asks + [f"INSERT OR IGNORE INTO u VALUES ({key}, 7)", "COMMIT", "SELECT * FROM u", "SELECT * FROM w", "ROLLBACK"]))
    # C22: a violation a trigger leaves that a later one, or none, mends
    for time in ["BEFORE", "AFTER"]:
        for late in ["", DEFERRED]:
            for mends in ["INSERT OR IGNORE INTO u VALUES ('q')", "DELETE FROM w WHERE k = 'q'", "UPDATE w SET k = NULL", "SELECT 1"]:
                fill = on + ["CREATE TABLE u(k TEXT PRIMARY KEY)", f"CREATE TABLE w(k TEXT REFERENCES u(k){late})", "CREATE TABLE t(a UNIQUE)", "CREATE TABLE lg(x)", f"CREATE TRIGGER h {time} INSERT ON t BEGIN INSERT INTO lg VALUES ('h'); {mends}; END", f"CREATE TRIGGER g {time} INSERT ON t BEGIN INSERT INTO lg VALUES ('g'); INSERT INTO w VALUES ('q'); END"]
                asks = []
                for clause in ORS:
                    for statement in [f"INSERT{clause} INTO t VALUES ('a{clause}')", f"INSERT{clause} INTO t VALUES ('b{clause}'), ('b{clause}')"]:
                        asks += [statement, "SELECT * FROM t", "SELECT * FROM u", "SELECT * FROM w", "SELECT * FROM lg", "BEGIN", statement.replace("'a", "'c").replace("'b", "'d"), "SELECT * FROM t", "SELECT * FROM w", "SELECT * FROM lg", "COMMIT", "ROLLBACK"]
                out.append((f"a violation a program leaves: {time}{late} {mends}", fill, asks))
    # a program's FAIL keeps nothing while a key is broken
    for late in ["", DEFERRED]:
        for fails in ["SELECT RAISE(FAIL, 'no') WHERE new.a = 2", "INSERT OR FAIL INTO v SELECT 1 WHERE new.a = 2"]:
            out.append((f"FAIL beside a broken key{late}: {fails}", on + ["CREATE TABLE u(k INTEGER PRIMARY KEY)", f"CREATE TABLE w(k INT REFERENCES u(k){late})", "CREATE TABLE v(n UNIQUE)", "INSERT INTO v VALUES (1)", "CREATE TABLE t(a)", f"CREATE TRIGGER g AFTER INSERT ON t BEGIN INSERT INTO w SELECT 7 WHERE new.a = 1; {fails}; END"], [a for s in ["INSERT INTO t VALUES (1), (2), (3)", "INSERT INTO t VALUES (4), (2), (3)", "INSERT OR FAIL INTO t VALUES (1), (2)"] for a in [s, "SELECT * FROM t", "SELECT * FROM w", "BEGIN", s, "SELECT * FROM t", "SELECT * FROM w", "INSERT OR IGNORE INTO u VALUES (7)", "COMMIT", "ROLLBACK", "DELETE FROM w", "DELETE FROM t", "DELETE FROM u"]]))
    # C25: a trigger's RAISE(FAIL) after its statement broke a key; rows that mend each other; OR REPLACE beside an upsert
    for late in ["", DEFERRED]:
        for action in ["FAIL", "ABORT", "ROLLBACK", "IGNORE"]:
            raises = "IGNORE" if action == "IGNORE" else f"{action}, 'no'"
            out.append((f"RAISE({action}) after a row broke a key{late}", on + ["CREATE TABLE p(k INTEGER PRIMARY KEY)", f"CREATE TABLE t(id INTEGER PRIMARY KEY, f INT REFERENCES p(k){late})", "INSERT INTO p VALUES (1)", "INSERT INTO t VALUES (1, 1), (2, 1), (3, 1)", f"CREATE TRIGGER g AFTER UPDATE ON t WHEN new.id = 2 BEGIN SELECT RAISE({raises}); END"], ["UPDATE t SET f = 8", "SELECT * FROM t", "BEGIN", "UPDATE t SET f = 8", "SELECT * FROM t", "INSERT OR IGNORE INTO p VALUES (8)", "UPDATE t SET f = 8", "SELECT * FROM t", "COMMIT", "ROLLBACK", "SELECT * FROM t"]))
        out.append((f"rows that mend each other, of a table that references itself{late}", on + [f"CREATE TABLE emp(id INT PRIMARY KEY, boss INT REFERENCES emp(id){late})"], ["INSERT INTO emp VALUES (2, 1), (1, NULL)", "SELECT * FROM emp", "INSERT INTO emp VALUES (4, 3)", "INSERT INTO emp SELECT 4, 3 UNION ALL SELECT 3, 4", "SELECT * FROM emp", "INSERT INTO emp VALUES (9, 8), (7, NULL)", "SELECT * FROM emp"]))
        for keyed in ["", " WITHOUT ROWID"]:
            for action in ACTIONS:
                asks = [a for s in ["INSERT OR REPLACE INTO u VALUES ('a', 2) ON CONFLICT(k) DO UPDATE SET n = excluded.n", "INSERT OR REPLACE INTO u VALUES ('c', 2) ON CONFLICT(k) DO UPDATE SET n = 9", "INSERT OR REPLACE INTO u VALUES ('d', 7), ('a', 7) ON CONFLICT(k) DO UPDATE SET k = 'b'", "INSERT OR REPLACE INTO w VALUES ('x', 1) ON CONFLICT(m) DO UPDATE SET k = 'a'", "INSERT OR REPLACE INTO u VALUES ('b', 1) ON CONFLICT(n) DO UPDATE SET k = 'z'", "REPLACE INTO u VALUES ('a', 1)", "INSERT INTO u VALUES ('b', 5) ON CONFLICT(k) DO UPDATE SET k = 'a'"] for a in [s, "SELECT * FROM u", "SELECT * FROM w"]]
                out.append((f"OR REPLACE beside an upsert{keyed}{late} {action}", on + [f"CREATE TABLE u(k TEXT PRIMARY KEY, n INT UNIQUE){keyed}", f"CREATE TABLE w(k TEXT DEFAULT 'b' REFERENCES u(k) ON DELETE {action} ON UPDATE {action}{late}, m INT PRIMARY KEY)", "INSERT INTO u VALUES ('a', 1), ('b', 2)", "INSERT INTO w VALUES ('a', 1), ('b', 2), ('b', 3)"], asks + ["BEGIN"] + asks + ["COMMIT", "SELECT * FROM u", "SELECT * FROM w", "ROLLBACK"]))
    # rows of one statement that mend each other, of a table that references itself
    for key, other in [("INTEGER", ""), ("INT", ""), ("TEXT", ""), ("INT", " WITHOUT ROWID"), ("TEXT COLLATE NOCASE", "")]:
        for late in ["", DEFERRED]:
            for action in ACTIONS:
                asks = []
                for s in ["INSERT INTO t VALUES (1, 2), (2, 3), (3, 1)", "INSERT INTO t VALUES (4, 5)", "INSERT INTO t VALUES (6, 6)", "UPDATE t SET id = id + 10", "UPDATE t SET id = id + 1, pid = pid + 1", "UPDATE t SET pid = id", "UPDATE t SET id = 50 WHERE id = 1", "UPDATE t SET id = 60, pid = 60 WHERE id = 6", "UPDATE t SET id = 61 WHERE id = 60", "REPLACE INTO t VALUES (1, 2)", "UPDATE OR REPLACE t SET id = 2 WHERE id = 1", "DELETE FROM t WHERE id = 2", "DELETE FROM t WHERE id = 60", "INSERT INTO t VALUES (2, 2) ON CONFLICT(id) DO UPDATE SET id = 70, pid = 70", "INSERT INTO t VALUES (3, 3) ON CONFLICT(id) DO UPDATE SET id = 71", "DELETE FROM t"]:
                    asks += [s, "SELECT * FROM t"]
                out.append((f"a table that references itself: {key}{other}{late} {action}", on + [f"CREATE TABLE t(id {key} PRIMARY KEY, pid {key.replace('INTEGER', 'INT')} DEFAULT 1 REFERENCES t(id) ON DELETE {action} ON UPDATE {action}{late}){other}", "INSERT INTO t VALUES (1, 1), (2, 1), (3, 2)" if not late else "INSERT INTO t VALUES (1, 1)"], asks + ["BEGIN"] + asks + ["COMMIT", "SELECT * FROM t", "ROLLBACK", "SELECT * FROM t"]))
    # rows of a statement that mend each other, of two tables
    for late in ["", DEFERRED]:
        for action in ACTIONS:
            asks = []
            for s in ["UPDATE p SET k = k + 1", "UPDATE p SET k = k + 1 WHERE k = 3", "UPDATE p SET k = 4 - k", "UPDATE OR REPLACE p SET k = 2 WHERE k = 1", "REPLACE INTO p VALUES (2), (3)", "DELETE FROM p WHERE k = 2", "DELETE FROM p", "INSERT INTO p VALUES (1), (2), (3)", "INSERT INTO c VALUES (7), (3)", "DELETE FROM c WHERE k = 7", "UPDATE c SET k = 2 WHERE k = 3"]:
                asks += [s, "SELECT * FROM p", "SELECT * FROM c"]
            out.append((f"rows that mend each other{late} {action}", on + ["CREATE TABLE p(k INT PRIMARY KEY)", f"CREATE TABLE c(k INT DEFAULT 2 REFERENCES p(k) ON DELETE {action} ON UPDATE {action}{late})", "INSERT INTO p VALUES (1), (2), (3)", "INSERT INTO c VALUES (1), (2), (3), (3)"], asks + ["BEGIN"] + asks + ["COMMIT", "SELECT * FROM p", "SELECT * FROM c", "ROLLBACK"]))
    # C22: recursive_triggers
    for recursive in ["ON", "OFF"] if RECURSIVE else []:
        pragma = [f"PRAGMA recursive_triggers = {recursive}"]
        log = "CREATE TABLE lg(x, y)"
        out.append((f"recursive_triggers {recursive}: an INSERT that fires itself", [log, "CREATE TABLE t(k INTEGER PRIMARY KEY, n INT)", "CREATE TRIGGER g AFTER INSERT ON t WHEN new.n < 4 BEGIN INSERT INTO lg VALUES ('g', new.n); INSERT INTO t(n) VALUES (new.n + 1); INSERT INTO lg VALUES ('z', new.n); END"] + pragma, ["INSERT INTO t VALUES (100, 1)", "SELECT * FROM t", "SELECT rowid, * FROM lg", "PRAGMA recursive_triggers", "PRAGMA recursive_triggers = OFF", "INSERT INTO t VALUES (200, 1)", "SELECT * FROM t", "PRAGMA recursive_triggers = ON", "INSERT INTO t VALUES (300, 1)", "SELECT * FROM t", "SELECT rowid, * FROM lg", "PRAGMA recursive_triggers"]))
        for keyed in ["", " WITHOUT ROWID"]:
            out.append((f"recursive_triggers {recursive}: UPDATE and DELETE that fire themselves{keyed}", [log, f"CREATE TABLE t(k INT PRIMARY KEY, n INT){keyed}", "INSERT INTO t VALUES (1, 5), (2, 5), (3, 5), (4, 5)", "CREATE TRIGGER g BEFORE UPDATE ON t WHEN new.n > 2 BEGIN INSERT INTO lg VALUES (old.k || 'b', new.n); UPDATE t SET n = new.n - 1 WHERE k = new.k + 1; END", "CREATE TRIGGER h AFTER UPDATE ON t WHEN new.n > 1 BEGIN INSERT INTO lg VALUES (old.k || 'a', new.n); UPDATE t SET n = new.n - 2 WHERE k = new.k - 1; END", "CREATE TRIGGER i AFTER DELETE ON t BEGIN INSERT INTO lg VALUES (old.k || 'd', old.n); DELETE FROM t WHERE k = old.k + 1; END"] + pragma, ["UPDATE t SET n = 4 WHERE k = 2", "SELECT * FROM t", "SELECT rowid, * FROM lg", "UPDATE t SET n = n + 3", "SELECT * FROM t", "SELECT rowid, * FROM lg", "DELETE FROM t WHERE k = 2", "SELECT * FROM t", "SELECT rowid, * FROM lg"]))
            for clause in ["", " ON CONFLICT REPLACE"]:
                out.append((f"recursive_triggers {recursive}: what REPLACE removes{keyed}{clause}", [log, f"CREATE TABLE t(k INT PRIMARY KEY{clause}, n INT UNIQUE{clause}, m INT){keyed}", "INSERT INTO t VALUES (1, 1, 1), (2, 2, 2), (3, 3, 3)", "CREATE TRIGGER g BEFORE DELETE ON t BEGIN INSERT INTO lg VALUES (old.k || 'b', old.n); END", "CREATE TRIGGER h AFTER DELETE ON t BEGIN INSERT INTO lg VALUES (old.k || 'a', (SELECT count(*) FROM t)); END", "CREATE TRIGGER i AFTER INSERT ON t BEGIN INSERT INTO lg VALUES (new.k || 'i', new.n); END", "CREATE TRIGGER j AFTER UPDATE ON t BEGIN INSERT INTO lg VALUES (new.k || 'u', new.n); END"] + pragma, [a for s in ["REPLACE INTO t VALUES (1, 9, 9)", "REPLACE INTO t VALUES (2, 3, 8)", "INSERT INTO t VALUES (1, 4, 4)", "UPDATE OR REPLACE t SET n = 9 WHERE k = 2", "UPDATE t SET k = 7 WHERE k = 1", "REPLACE INTO t VALUES (7, 7, 7), (7, 8, 8), (8, 8, 8)", "INSERT OR REPLACE INTO t VALUES (8, 1, 1) ON CONFLICT(k) DO UPDATE SET m = 5"] for a in [s, "SELECT * FROM t", "SELECT rowid, * FROM lg"]]))
                out.append((f"recursive_triggers {recursive}: what REPLACE removes writes the row again{keyed}{clause}", [log, f"CREATE TABLE t(k INT PRIMARY KEY{clause}, n INT UNIQUE{clause}){keyed}", "INSERT INTO t VALUES (1, 1), (2, 2), (3, 3)", "CREATE TRIGGER h AFTER DELETE ON t WHEN old.k = 2 BEGIN INSERT INTO lg VALUES (old.k, old.n); INSERT INTO t VALUES (old.k, 77); END", "CREATE TRIGGER i BEFORE DELETE ON t WHEN old.k = 3 BEGIN INSERT INTO lg VALUES (old.k, old.n); DELETE FROM t WHERE k = 3; END"] + pragma, [a for s in ["REPLACE INTO t VALUES (2, 9)", "REPLACE INTO t VALUES (9, 3)", "UPDATE OR REPLACE t SET k = 2 WHERE k = 1"] for a in [s, "SELECT * FROM t", "SELECT rowid, * FROM lg"]]))
        out.append((f"recursive_triggers {recursive}: two triggers that fire each other", [log, "CREATE TABLE t(n INT)", "CREATE TABLE s(n INT)", "CREATE TRIGGER g AFTER INSERT ON t WHEN new.n < 6 BEGIN INSERT INTO lg VALUES ('t', new.n); INSERT INTO s VALUES (new.n + 1); END", "CREATE TRIGGER h AFTER INSERT ON s WHEN new.n < 6 BEGIN INSERT INTO lg VALUES ('s', new.n); INSERT INTO t VALUES (new.n + 1); END"] + pragma, ["INSERT INTO t VALUES (1), (2)", "SELECT * FROM t", "SELECT * FROM s", "SELECT rowid, * FROM lg", "INSERT INTO s VALUES (3)", "SELECT rowid, * FROM lg"]))
        out.append((f"recursive_triggers {recursive}: a foreign key's action fires what the trigger is in", on + [log, "CREATE TABLE p(k INT PRIMARY KEY)", "CREATE TABLE c(k INT PRIMARY KEY, pk INT REFERENCES p(k) ON DELETE CASCADE)", "INSERT INTO p VALUES (1), (2), (3)", "INSERT INTO c VALUES (1, 1), (2, 2), (3, 3), (4, 3)", "CREATE TRIGGER g AFTER DELETE ON c BEGIN INSERT INTO lg VALUES (old.k, old.pk); DELETE FROM p WHERE k = old.pk + 1; END"] + pragma, ["DELETE FROM p WHERE k = 1", "SELECT * FROM p", "SELECT * FROM c", "SELECT rowid, * FROM lg"]))
        for depth in [20, 1500]:
            out.append((f"recursive_triggers {recursive}: {depth} deep", ["CREATE TABLE t(n INT)", f"CREATE TRIGGER g AFTER INSERT ON t WHEN new.n < {depth} BEGIN INSERT INTO t VALUES (new.n + 1); END"] + pragma, ["INSERT INTO t VALUES (1)", "SELECT count(*), max(n) FROM t", "DELETE FROM t", f"INSERT OR IGNORE INTO t VALUES ({depth - 10})", "SELECT count(*), max(n) FROM t"]))
    # C22: the order a WITHOUT ROWID table's rows go in
    for keyed in ["", " WITHOUT ROWID"]:
        for primary, rows in [("k TEXT PRIMARY KEY", "('d', 1, 9), ('a', 4, 7), ('c', 2, 8), ('b', 3, 6)"), ("k TEXT, PRIMARY KEY (k DESC)", "('d', 1, 9), ('a', 4, 7), ('c', 2, 8), ('b', 3, 6)"), ("k INT, PRIMARY KEY (a, k)", "(4, 1, 9), (1, 4, 7), (3, 2, 8), (2, 3, 6)")]:
            columns = primary.split(", PRIMARY")[0]
            rest = primary[len(columns):]
            for index in ["CREATE INDEX tb ON t(b)", "CREATE INDEX ta ON t(a)", "CREATE INDEX tab ON t(a, b)", "CREATE INDEX tabk ON t(b, a, k)"]:
                for where in ["", " WHERE a > 1", " WHERE b > 6", " WHERE b > 6 AND a > 1"]:
                    out.append((f"the order rows go in{keyed}: {primary}; {index}{where}", ["CREATE TABLE lg(x, y)", f"CREATE TABLE t({columns}, a INT, b INT{rest}){keyed}", index, f"INSERT INTO t VALUES {rows}", "CREATE TRIGGER g BEFORE DELETE ON t BEGIN INSERT INTO lg VALUES (old.k, (SELECT group_concat(k) FROM (SELECT k FROM t ORDER BY k))); END", "CREATE TRIGGER h AFTER UPDATE ON t BEGIN INSERT INTO lg VALUES (new.k, new.b); UPDATE t SET b = b + 1 WHERE k > new.k; END"], [a for s in [f"UPDATE t SET b = b + 10{where}", f"UPDATE t SET a = a + 1{where}", f"DELETE FROM t{where}"] for a in [s, "SELECT * FROM t", "SELECT rowid, * FROM lg", "DELETE FROM lg"]]))
    return out


class Draw:
    """A database of a parent p, its child c, its child's child g, and t, which references itself."""

    def __init__(self, rng):
        self.rng = rng
        pick = rng.choice
        self.kind = pick(["INT", "TEXT", "TEXT", "INTEGER"])
        self.domain = ["1", "2", "3", "4"] if self.kind != "TEXT" else ["'a'", "'b'", "'c'", "'d'"]
        action = lambda: pick(ACTIONS)
        clauses = lambda: f" ON DELETE {action()} ON UPDATE {action()}{pick(['', '', DEFERRED])}"
        default = lambda: pick(["", f" DEFAULT {pick(self.domain)}", f" DEFAULT {pick(self.domain)}", " DEFAULT NULL", " DEFAULT 99"])
        keyed = lambda: pick(["", "", " WITHOUT ROWID"])
        on = lambda: pick(["", "", "", pick([f" ON CONFLICT {c}" for c in CONFLICTS])])
        kind, loose = self.kind, "INT" if self.kind == "INTEGER" else self.kind
        self.pair = rng.random() < 0.3
        if self.pair:
            keys = pick(["PRIMARY KEY (k, m)", "PRIMARY KEY (m, k)", "UNIQUE (k, m)"]) + on()
            p = f"CREATE TABLE p(k {loose}, m {loose}, n INT{pick(['', ' UNIQUE' + on()])}, {keys})" + (keyed() if "PRIMARY" in keys else "")
        else:
            p = pick([f"CREATE TABLE p(k {kind} PRIMARY KEY{on()}, m {loose}, n INT{pick(['', ' UNIQUE' + on()])}){keyed()}", f"CREATE TABLE p(m {loose}, k {loose} UNIQUE{on()}, n INT)"])
            if "m " in p.split("(")[1][:3]:
                p = p.replace("m " + loose + ", k " + loose + " UNIQUE", "k " + loose + " UNIQUE").replace(", n INT)", f", m {loose}, n INT)")
        reference = "FOREIGN KEY (k, m) REFERENCES p(k, m)" if self.pair else "FOREIGN KEY (k) REFERENCES p(k)"
        unique = pick(["", "", f", UNIQUE (k, m){on()}", f", UNIQUE (k){on()}"])
        c = f"CREATE TABLE c(k {loose}{default()}, m {loose}{default()}{pick(['', '', ' NOT NULL'])}, id {kind} PRIMARY KEY{on()}{unique}, {reference}{clauses()}){keyed()}"
        self.grand = pick(["id", "id", "k"]) if unique.startswith(", UNIQUE (k)") else "id"
        g = f"CREATE TABLE g(k {loose}{default()} REFERENCES c({self.grand}){clauses()}, n INT{pick(['', ' UNIQUE' + on(), ' NOT NULL'])}, id {kind} PRIMARY KEY)"
        t = f"CREATE TABLE t(id {kind} PRIMARY KEY{on()}, k {loose}{default()} REFERENCES t(id){clauses()}, n {loose}{pick(['', f' REFERENCES p(k){clauses()}' if not self.pair else ''])}){keyed()}"
        self.columns = {"p": ["k", "m", "n"], "c": ["k", "m", "id"], "g": ["k", "n", "id"], "t": ["id", "k", "n"]}
        self.tables = [p, c, g, t, "CREATE TABLE lg(ev, x, y)", look([("p", ["k", "m", "n"]), ("c", ["k", "m", "id"]), ("g", ["k", "n", "id"]), ("t", ["id", "k", "n"]), ("lg", ["rowid", "ev", "x", "y"])])]
        for index in ["CREATE INDEX ck ON c(k)", "CREATE INDEX ckm ON c(k, m)", "CREATE INDEX gk ON g(k)", "CREATE INDEX tk ON t(k)"]:
            if rng.random() < 0.2:
                self.tables.append(index)
        self.recursive = RECURSIVE and rng.random() < 0.5
        self.enforced = rng.random() < 0.93
        self.triggers = [self.trigger(f"{table}{i}", table) for table in "pcgt" for i in range(pick([0, 0, 1, 1, 2]))]

    def value(self, column):
        rng = self.rng
        if column == "n":
            return rng.choice(["1", "2", "3", "4", "5", "NULL"]) if rng.random() < 0.6 else rng.choice(self.domain)
        return rng.choice(self.domain * 4 + ["NULL"] + (["99"] if self.kind != "TEXT" else ["'zz'", "'A'"]))

    def key(self):
        return self.rng.choice(self.domain)

    def trigger(self, name, table):
        rng = self.rng
        pick = rng.choice
        columns = self.columns[table]
        time = pick(["BEFORE", "AFTER"])
        event = pick(["INSERT", "DELETE", "DELETE", "UPDATE", "UPDATE"])
        text = event if event != "UPDATE" or rng.random() < 0.6 else f"UPDATE OF {pick(columns)}"
        rows = {"INSERT": ["new"], "DELETE": ["old"], "UPDATE": ["old", "new"]}[event]
        read = lambda: f"{pick(rows)}.{pick(columns)}"
        # no trigger writes its own table without a guard that ends: a level's log is a row
        body = [f"INSERT INTO lg VALUES ('{name}{time[0]}{event[0]}', {read()}, {read()})"]
        for _ in range(pick([0, 1, 1, 2])):
            # what becomes of a row a BEFORE trigger changes is undefined (lang_createtrigger.html)
            other = pick("pcgt" if time == "AFTER" or event == "INSERT" else "pcgt".replace(table, ""))
            there = self.columns[other]
            orc = pick(["", "", "", pick(ORS)])
            drawn = rng.random()
            few = " AND (SELECT count(*) FROM lg) < 24"
            put = lambda: pick([self.key(), read(), read(), "NULL"])
            where = f" WHERE {pick([c for c in there if c != 'n'])} = {pick([read(), self.key()])}{few}"
            if drawn < 0.35:
                body.append(f"DELETE FROM {other}{where}")
            elif drawn < 0.65:
                body.append(f"UPDATE{orc} {other} SET {pick(there)} = {put()}{where}")
            elif drawn < 0.92:
                body.append(f"INSERT{orc} INTO {other} SELECT {put()}, {put()}, {put()} WHERE 1{few}")
            else:
                body.append(f"SELECT RAISE({pick(['IGNORE', 'ABORT, ' + repr('no'), 'FAIL, ' + repr('no'), 'ROLLBACK, ' + repr('no')])}) WHERE {read()} = {self.key()}")
        if rng.random() < 0.5:
            body.append(f"INSERT INTO lg VALUES ('{name}z', {read()}, NULL)")
        when = pick(["", "", "", f" WHEN {read()} IS NOT NULL", f" WHEN {read()} > {self.key()}"])
        return f"CREATE TRIGGER {name} {time} {text} ON {table}{when} BEGIN {'; '.join(body)}; END"

    def row(self, table):
        return f"({', '.join(self.value(c) for c in self.columns[table])})"

    def statement(self):
        rng = self.rng
        pick = rng.choice
        table = pick("ppcct" + "gt")
        columns = self.columns[table]
        orc = pick(["", "", pick(ORS), pick(ORS)])
        drawn = rng.random()
        where = pick(["", f" WHERE {pick([c for c in columns if c != 'n'])} = {self.key()}", f" WHERE +{pick(columns)} {pick(['>', '<', 'IS NOT'])} {self.key()}", f" WHERE +{pick(columns)} = {self.key()}"])
        if drawn < 0.3:
            return f"INSERT{orc} INTO {table} VALUES {', '.join(self.row(table) for _ in range(pick([1, 1, 2, 3])))}"
        if drawn < 0.55:
            if not self.enforced:
                where = f" WHERE {'k' if table == 'p' else 'id'} = {self.key()}" + (f" AND m = {self.key()}" if table == "p" and self.pair else "")
            sets = ", ".join(f"{c} = {pick([self.value(c), self.value(c), pick(columns)])}" for c in rng.sample(columns, pick([1, 1, 2])))
            return f"UPDATE{orc} {table} SET {sets}{where}"
        if drawn < 0.85:
            return f"DELETE FROM {table}{where}"
        target = {"p": "(k, m)" if self.pair else "(k)", "c": "(id)", "g": "(id)", "t": "(id)"}[table]
        column = pick(columns)
        return f"INSERT{orc} INTO {table} VALUES {self.row(table)} ON CONFLICT{target} {pick([f'DO UPDATE SET {column} = excluded.{column}', f'DO UPDATE SET {column} = {self.value(column)}', 'DO NOTHING'])}"


def text(value):
    if value is None:
        return "NULL"
    if isinstance(value, float):
        return f"{value:.9f}"
    return str(value)


def answer(db, sql):
    try:
        rows = sorted(",".join(text(value) for value in row) for row in db.execute(sql).fetchall())
    except sqlite3.IntegrityError as error:
        return f"ERROR {error}"
    except sqlite3.Error:
        return "ERROR"
    return f"{len(rows)}:{';'.join(rows)}"


class Wrong(Exception):
    """SQLite itself leaves what its documentation says it doesn't."""


def deferred(db, table, fkid):
    """Whether a foreign key of a table, by PRAGMA foreign_key_check's number, is deferred: they are numbered from
    the last declared."""
    sql = db.execute("SELECT sql FROM sqlite_schema WHERE name = ?", (table,)).fetchone()[0]
    clauses = re.findall(r"REFERENCES [^,]*(?:\([^)]*\))?[^,]*", sql)
    return DEFERRED.strip() in list(reversed(clauses))[fkid]


def ask(db, sql, out):
    """Writes what SQLite answers, unless it is what its documentation rules out (foreignkeys.html): a row with no
    parent it keeps, or a COMMIT it refuses with none. 3.54.0 looks no parent up for a row written after its
    statement coded a SET NULL action of the same key (isSetNullAction), and counts the deferred key of a row it
    refuses for another (no statement journal undoes it)."""
    got = answer(db, sql)
    if db.execute("PRAGMA foreign_keys").fetchone()[0]:
        broken = db.execute("PRAGMA foreign_key_check").fetchall()
        if sql.split()[0] in ("COMMIT", "RELEASE") and got == "ERROR FOREIGN KEY constraint failed" and not broken:
            raise Wrong(sql)
        if any(not db.in_transaction or not deferred(db, row[0], row[3]) for row in broken):
            raise Wrong(sql)
    out.append(f"q\t{sql}\t{got}")


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 300
    seed = sys.argv[2] if len(sys.argv) > 2 else "C26"
    only = sys.argv[3] if len(sys.argv) > 3 else None
    print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_foreign_keys.py {count} {seed}")
    out = []
    wrong = []
    for name, fill, statements in found():
        if only == "drawn":
            break
        db = sqlite3.connect(":memory:", isolation_level=None)
        out.append(f"db\t{name}")
        for statement in fill:
            db.execute(statement)
            out.append(f"sql\t{statement}")
        try:
            for statement in statements:
                ask(db, statement, out)
        except Wrong as statement:
            wrong.append(f"{name}: {statement}")
            continue
        ask(db, "PRAGMA integrity_check", out)
    made = refused = 0
    while made < count:
        rng = random.Random(f"{seed}-{made}-{refused}")
        draw = Draw(rng)
        db = sqlite3.connect(":memory:", isolation_level=None)
        fill = ["PRAGMA foreign_keys = ON"] if draw.enforced else []
        fill += draw.tables
        fill += [f"INSERT INTO {table} VALUES {draw.row(table)}" for table, many in [("p", 4), ("c", 5), ("g", 3), ("t", 5)] for _ in range(many)]
        lines = []
        try:
            for statement in fill:
                try:
                    db.execute(statement)
                except sqlite3.IntegrityError:
                    continue
                lines.append(f"sql\t{statement}")
                if draw.enforced and db.execute("PRAGMA foreign_key_check").fetchall():
                    raise sqlite3.Error(statement)
            for statement in draw.triggers + ([f"PRAGMA recursive_triggers = {'ON' if draw.recursive else 'OFF'}"]):
                db.execute(statement)
                lines.append(f"sql\t{statement}")
        except sqlite3.Error as error:
            refused += 1
            continue
        out.append(f"db\tdrawn {made}")
        out += lines
        made += 1
        try:
            scope = rng.choice(["", "", "BEGIN", "BEGIN", "SAVEPOINT"])
            if scope == "BEGIN":
                ask(db, "BEGIN", out)
            for _ in range(rng.choice([5, 6, 7])):
                if scope == "SAVEPOINT":
                    ask(db, "SAVEPOINT s", out)
                ask(db, draw.statement(), out)
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
        except Wrong as statement:
            wrong.append(f"drawn {made - 1}: {statement}")
    print("\n".join(out))
    print(f"# {made} drawn, {refused} SQLite refused; {len(wrong)} databases end where SQLite breaks its own documentation:", file=sys.stderr)
    print("\n".join(f"#   {line}" for line in wrong), file=sys.stderr)


main()
