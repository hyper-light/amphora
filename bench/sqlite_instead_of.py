"""INSTEAD OF triggers (lane C28; lang_createtrigger.html, section 3). Views over a table, joins, aggregates,
DISTINCT, compounds, CTEs, windows, VALUES and other views, with and without a column list, in main and in
temp; INSTEAD OF INSERT, DELETE, UPDATE and UPDATE OF triggers on them, TEMP ones too, with and without
WHEN, whose bodies write a log, tables under every conflict clause, another view and the view itself, or
raise; fired by INSERT (one row, several, a SELECT, DEFAULT VALUES, a column list, the rowid named), REPLACE,
UPDATE (row values, subqueries, FROM, an alias), DELETE, under every conflict clause, with RETURNING, alone
or inside a transaction; and the statements SQLite refuses. Writes each statement with SQLite's answer (the
sqlite3 library Python links, printed first) to crates/engine/tests/fixtures/instead_of.tsv, which
tests/windows_and_triggers.rs holds Amphora to.

Not drawn, each of another cause: a view's column named with its database (`WHERE main.v.k = 1`), a view with
a column list over VALUES or `*`, a column that is a scalar subquery (none is what any query of a view takes as
SQLite does), and changes() after a statement refused as it ran or beside WITHOUT ROWID tables.
Not drawn, SQLite's against its documents: RETURNING where no trigger is of the statement's event but one is
of another (SQLite takes the statement and does nothing; lang_createview.html: a view is read-only without
one); an UPDATE FROM of a view SQLite doesn't flatten (the columns the statement doesn't name are NULL there); an
UPDATE's or a DELETE's WHERE that reads the view by its alias. SQLite refuses it (it stores
the view's rows with the view's own name, `sqlite3MaterializeView`); lang_update.html and lang_delete.html
give the target an alias, so Amphora takes it (`an_alias_names_the_view_everywhere`).

Each line is `db <name>`, `sql <statement>` (the database's tables, rows, views and triggers) or `q
<statement> <answer>`: the answer's rows sorted, each row's values joined by commas, a NULL written NULL,
the rows joined by semicolons, or ERROR~ and SQLite's words for a statement it refuses.
usage: python3 bench/sqlite_instead_of.py [databases [seed]] > crates/engine/tests/fixtures/instead_of.tsv"""
import random
import sqlite3
import sys

# A column of a table that isn't there: Amphora names the table it hasn't, in any statement.
WORDS_NOT_HELD = {"INSERT INTO v AS q VALUES (1, 2, 3) RETURNING q.a", "UPDATE v SET a = 1 RETURNING old.a", "UPDATE v AS q SET a = 1 RETURNING q.a", "UPDATE v AS q SET a = v.b", "DELETE FROM v AS q WHERE v.a = 1", "INSERT INTO u VALUES ('o', 52)"}
CONFLICTS = ["ABORT", "FAIL", "IGNORE", "REPLACE", "ROLLBACK"]
BASE = [
    "CREATE TABLE t(k INTEGER PRIMARY KEY, a INT, b TEXT, c REAL)",
    "CREATE TABLE u(k TEXT PRIMARY KEY, n INT UNIQUE)",
    "CREATE TABLE lg(ev, x, y, z)",
    "INSERT INTO t VALUES (1, 10, 'x', 1.5), (2, 20, 'y', 2.5), (3, 30, 'x', NULL)",
    "INSERT INTO u VALUES ('a', 1), ('b', 2), ('z', 7)",
]
LOOK = "SELECT 't', quote(k), quote(a), quote(b), quote(c) FROM t UNION ALL SELECT 'u', quote(k), quote(n), '', '' FROM u UNION ALL SELECT 'lg', quote(ev), quote(x), quote(y), quote(z) FROM lg"
LOG = "SELECT rowid, quote(ev), quote(x), quote(y), quote(z) FROM lg"
COUNTS = "SELECT changes(), total_changes(), last_insert_rowid()"
VIEW = "CREATE VIEW v AS SELECT k, a, b FROM t"
INSERTS = "CREATE TRIGGER gi INSTEAD OF INSERT ON v BEGIN INSERT INTO lg VALUES ('I', quote(new.k), quote(new.a), quote(new.b)); END"
UPDATES = "CREATE TRIGGER gu INSTEAD OF UPDATE ON v BEGIN INSERT INTO lg VALUES ('U', quote(old.k) || quote(old.a) || quote(old.b), quote(new.k) || quote(new.a), quote(new.b)); END"
DELETES = "CREATE TRIGGER gd INSTEAD OF DELETE ON v BEGIN INSERT INTO lg VALUES ('D', quote(old.k), quote(old.a), quote(old.b)); END"
ALL = [VIEW, INSERTS, UPDATES, DELETES]

# The forms the documents name, each in its own words: what is made, what is asked.
FORMS = [
    ("refused: a view with no trigger", [VIEW], ["INSERT INTO v VALUES (1, 2, 3)", "UPDATE v SET a = 1", "DELETE FROM v", "REPLACE INTO v VALUES (1, 2, 3)", "INSERT INTO v SELECT * FROM v", "UPDATE v SET zz = 1", "DELETE FROM v WHERE zz"]),
    ("refused: a view whose trigger is of another event", [VIEW, DELETES], ["INSERT INTO v VALUES (1, 2, 3)", "UPDATE v SET a = 1", "DELETE FROM v WHERE k = 1", LOOK]),
    (
        "refused: creating one",
        [VIEW, "CREATE TEMP VIEW tv AS SELECT k FROM t"],
        [
            "CREATE TRIGGER g INSTEAD OF INSERT ON t BEGIN SELECT 1; END",
            "CREATE TRIGGER g INSTEAD OF INSERT ON main.t BEGIN SELECT 1; END",
            "CREATE TEMP TRIGGER g INSTEAD OF INSERT ON main.t BEGIN SELECT 1; END",
            "CREATE TEMP TRIGGER g INSTEAD OF DELETE ON t BEGIN SELECT 1; END",
            "CREATE TRIGGER g BEFORE INSERT ON v BEGIN SELECT 1; END",
            "CREATE TRIGGER g AFTER UPDATE OF a ON v BEGIN SELECT 1; END",
            "CREATE TRIGGER g DELETE ON v BEGIN SELECT 1; END",
            "CREATE TRIGGER g INSERT ON main.v BEGIN SELECT 1; END",
            "CREATE TEMP TRIGGER g AFTER INSERT ON main.v BEGIN SELECT 1; END",
            "CREATE TEMP TRIGGER g BEFORE INSERT ON v BEGIN SELECT 1; END",
            "CREATE TRIGGER g BEFORE INSERT ON tv BEGIN SELECT 1; END",
            "CREATE TRIGGER g INSTEAD OF INSERT ON nope BEGIN SELECT 1; END",
            "CREATE TEMP TRIGGER g INSTEAD OF INSERT ON nope BEGIN SELECT 1; END",
            "SELECT name FROM sqlite_schema WHERE type = 'trigger'",
            "CREATE TRIGGER g INSTEAD OF INSERT ON v BEGIN SELECT 1; END",
            "CREATE TRIGGER g INSTEAD OF DELETE ON v BEGIN SELECT 1; END",
            "CREATE TRIGGER IF NOT EXISTS g INSTEAD OF DELETE ON v BEGIN SELECT 1; END",
            "DELETE FROM v",
            "DROP TRIGGER g",
            "DROP TRIGGER g",
            "DROP TRIGGER IF EXISTS g",
            "INSERT INTO v VALUES (1, 2, 3)",
        ],
    ),
    (
        "refused: a column that isn't there",
        [VIEW, "CREATE TRIGGER g1 INSTEAD OF INSERT ON v BEGIN INSERT INTO lg VALUES (new.zz, 0, 0, 0); END", "CREATE TRIGGER g2 INSTEAD OF UPDATE ON v BEGIN INSERT INTO lg VALUES (old.zz, 0, 0, 0); END", "CREATE TRIGGER g3 INSTEAD OF DELETE ON v BEGIN INSERT INTO lg VALUES (new.a, 0, 0, 0); END"],
        ["INSERT INTO v VALUES (1, 2, 3)", "UPDATE v SET a = 1", "DELETE FROM v", LOOK],
    ),
    (
        "refused: a row a trigger can't name",
        [VIEW, "CREATE TRIGGER g1 INSTEAD OF INSERT ON v BEGIN INSERT INTO lg VALUES (old.a, 0, 0, 0); END", "CREATE TRIGGER g2 INSTEAD OF UPDATE ON v BEGIN INSERT INTO lg VALUES (old.rowid, 0, 0, 0); END", "CREATE TRIGGER g3 INSTEAD OF DELETE ON v WHEN old.oid BEGIN SELECT 1; END"],
        ["INSERT INTO v VALUES (1, 2, 3)", "UPDATE v SET a = 1", "DELETE FROM v", LOOK],
    ),
    (
        "refused: statements",
        ALL,
        [
            "INSERT INTO v VALUES (1)",
            "INSERT INTO v VALUES (1, 2, 3, 4)",
            "INSERT INTO v(a) VALUES (1, 2)",
            "INSERT INTO v(a, zz) VALUES (1, 2)",
            "INSERT INTO main.v(zz) VALUES (1)",
            "INSERT INTO main.v VALUES (1)",
            "INSERT INTO v AS q(zz) VALUES (1)",
            "INSERT INTO v SELECT 1, 2",
            "INSERT INTO v(rowid, a) VALUES ('x', 8)",
            "INSERT INTO v(rowid, a) SELECT 7.5, 5",
            "INSERT INTO v VALUES (1, 2, 3) ON CONFLICT DO NOTHING",
            "INSERT INTO v VALUES (1, 2, 3) ON CONFLICT(k) DO UPDATE SET a = 1",
            "INSERT INTO v VALUES (1, 2, 3) RETURNING rowid",
            "INSERT INTO v AS q VALUES (1, 2, 3) RETURNING q.a",
            "UPDATE v SET zz = 5",
            "UPDATE v SET a = zz",
            "UPDATE v SET a = rowid",
            "UPDATE v SET a = 1 WHERE rowid = 1",
            "UPDATE v SET rowid = 'x'",
            "UPDATE v SET rowid = NULL",
            "UPDATE v SET (a, b) = (1, 2, 3)",
            "UPDATE v SET (a, b) = (SELECT 1)",
            "UPDATE v SET a = 1 RETURNING rowid",
            "UPDATE v SET a = 1 RETURNING old.a",
            "UPDATE v AS q SET a = 1 RETURNING q.a",
            "UPDATE v AS q SET a = v.b",
            "UPDATE v INDEXED BY i SET a = 1",
            "UPDATE v SET a = 1 FROM v",
            "DELETE FROM v WHERE zz = 1",
            "DELETE FROM v WHERE rowid = 1",
            "DELETE FROM v AS q WHERE v.a = 1",
            "DELETE FROM v INDEXED BY i",
            "DELETE FROM v RETURNING oid",
            LOOK,
        ],
    ),
    (
        "insert",
        ALL,
        [
            "INSERT INTO v VALUES (1, 2, 3)",
            "INSERT INTO v VALUES ('1', 2.0, x'01'), (NULL, 'q', 4)",
            "INSERT INTO v(b, k) VALUES ('p', '7')",
            "INSERT INTO v(a, a) VALUES (1, 2)",
            "INSERT INTO v DEFAULT VALUES",
            "INSERT INTO v SELECT k, a, b FROM t",
            "INSERT INTO v SELECT k, a, b FROM t UNION ALL SELECT 9, n, k FROM u",
            "INSERT INTO v SELECT * FROM v WHERE a > 10 ORDER BY k DESC",
            "WITH q(x) AS (VALUES (5), (6)) INSERT INTO v(a) SELECT x FROM q",
            "INSERT INTO v(a) WITH q(x) AS (VALUES (5), (6)) SELECT x + 1 FROM q",
            "INSERT INTO v(a, b) VALUES ((SELECT max(k) FROM t), (SELECT count(*) FROM lg))",
            "INSERT INTO v(rowid, a) VALUES (7, 8), (NULL, 9), ('5', 10)",
            "INSERT INTO v(rowid, oid, a) VALUES ('x', 2, 3)",
            "REPLACE INTO v VALUES (5, 6, 7)",
            "INSERT OR IGNORE INTO main.v VALUES (5, 6, 7)",
            "INSERT INTO v AS q VALUES (1, 2, 3)",
            LOG,
            COUNTS,
            "INSERT INTO v VALUES (1, '2', 3.0) RETURNING *",
            "INSERT INTO v(a) VALUES (1), (2) RETURNING a, b, a + 1 AS s, typeof(b)",
            "INSERT INTO v DEFAULT VALUES RETURNING k IS NULL",
            "INSERT INTO v SELECT k, a, b FROM t RETURNING v.a, (SELECT count(*) FROM lg), (SELECT count(*) FROM v)",
            "INSERT INTO v SELECT k, a, b FROM t WHERE k > 1 RETURNING (SELECT n FROM u WHERE u.n = v.k)",
            "INSERT INTO v SELECT k, a, b FROM t WHERE k > 99 RETURNING *",
            LOG,
            COUNTS,
        ],
    ),
    (
        "update",
        ALL,
        [
            "UPDATE v SET a = 7",
            "UPDATE v SET a = a + 1, b = k WHERE k < 3",
            "UPDATE v SET a = '8', b = 9 WHERE k = 1",
            "UPDATE v SET a = 1, a = 2 WHERE k = 1",
            "UPDATE v SET (a, b) = (b, a) WHERE k = 2",
            "UPDATE v SET (a, b) = (SELECT n, k FROM u WHERE u.n = v.k)",
            "UPDATE v SET a = (SELECT n FROM u WHERE u.n = v.k), b = (SELECT max(k) FROM u)",
            "UPDATE v SET a = (SELECT count(*) FROM lg)",
            "UPDATE v SET b = (SELECT count(*) FROM lg WHERE lg.x <> v.k) WHERE k IN (SELECT n FROM u)",
            "UPDATE v SET rowid = 5 WHERE k = 1",
            "UPDATE v SET oid = '6', a = 1 WHERE k = 1",
            "UPDATE v AS q SET a = q.b || q.k WHERE a = 10",
            "UPDATE main.v SET a = main.v.b WHERE v.k = 1",
            "UPDATE v NOT INDEXED SET a = 3 WHERE k = 3",
            "UPDATE OR REPLACE v SET a = 3 WHERE b = 'x'",
            "WITH q(x, y) AS (VALUES (1, 'one'), (2, 'two')) UPDATE v SET b = (SELECT y FROM q WHERE x = v.k) WHERE k IN (SELECT x FROM q)",
            "UPDATE v SET a = 1 WHERE k > 99",
            LOG,
            COUNTS,
            "UPDATE v SET a = 5 RETURNING *",
            "UPDATE v SET a = '5' WHERE k = 2 RETURNING k, typeof(a), a || b AS ab, v.b",
            "UPDATE v SET b = 5 RETURNING b, (SELECT count(*) FROM v WHERE v.k < 3), (SELECT max(n) FROM u WHERE n < v.k)",
            "WITH q(x) AS (VALUES (2)) UPDATE v SET a = 1 WHERE k = (SELECT x FROM q) RETURNING k + (SELECT x FROM q)",
            "UPDATE v SET a = 1 WHERE k > 99 RETURNING a",
            LOG,
            COUNTS,
        ],
    ),
    (
        "update from",
        ALL,
        [
            "UPDATE v SET a = n FROM u WHERE v.k = u.n",
            "UPDATE v SET a = u.n, b = u.k FROM u",
            "UPDATE v AS q SET b = u.k FROM u WHERE q.k = u.n",
            "UPDATE v SET (a, b) = (u.n, u.k) FROM u WHERE v.k = u.n",
            "UPDATE v SET (a, b) = (SELECT k + u.n, b FROM t WHERE t.k = v.k) FROM u WHERE u.n = v.k",
            "UPDATE v SET a = (SELECT max(n) FROM u AS w WHERE w.n < p.n) FROM u AS p WHERE p.n = v.k",
            "UPDATE v SET a = w.k FROM u JOIN t AS w ON w.k = u.n WHERE v.k = w.k",
            "UPDATE v SET a = w.a FROM v AS w WHERE w.k = v.k + 1",
            "UPDATE v SET a = q.s FROM (SELECT n + 1 AS s, n FROM u) AS q WHERE q.n = v.k",
            "WITH q(x) AS (SELECT n FROM u) UPDATE v SET a = x FROM q WHERE x = v.k",
            "UPDATE v SET rowid = u.n FROM u WHERE v.k = u.n",
            "UPDATE v SET a = u.n FROM u WHERE v.k = u.n RETURNING *",
            "UPDATE v SET a = u.k FROM u WHERE v.k = u.n RETURNING a, (SELECT count(*) FROM t)",
            LOG,
            COUNTS,
        ],
    ),
    (
        "delete",
        ALL,
        [
            "DELETE FROM v",
            "DELETE FROM v WHERE a > 10 AND b = 'x'",
            "DELETE FROM v WHERE k IN (SELECT n FROM u)",
            "DELETE FROM v WHERE EXISTS (SELECT 1 FROM u WHERE u.n = v.k)",
            "DELETE FROM main.v WHERE v.k = 1",
            "DELETE FROM v AS q WHERE k = 1",
            "DELETE FROM v NOT INDEXED WHERE k = 2",
            "WITH q(x) AS (VALUES (1)) DELETE FROM v WHERE k = (SELECT x FROM q)",
            "DELETE FROM v WHERE k > 99",
            LOG,
            COUNTS,
            "DELETE FROM v RETURNING *",
            "DELETE FROM v WHERE k = 2 RETURNING a, b || k, (SELECT count(*) FROM u WHERE n > v.k), (SELECT count(*) FROM v)",
            "WITH q(x) AS (VALUES (1)) DELETE FROM v WHERE k = 1 RETURNING (SELECT x FROM q)",
            "DELETE FROM v WHERE k > 99 RETURNING a",
            LOG,
            COUNTS,
        ],
    ),
    (
        "the document's example",
        [
            "CREATE TABLE customer(cust_id INTEGER PRIMARY KEY, cust_name TEXT, cust_addr TEXT)",
            "CREATE VIEW customer_address AS SELECT cust_id, cust_addr FROM customer",
            "CREATE TRIGGER cust_addr_chng INSTEAD OF UPDATE OF cust_addr ON customer_address BEGIN UPDATE customer SET cust_addr=NEW.cust_addr WHERE cust_id=NEW.cust_id; END",
            "INSERT INTO customer VALUES (1, 'Jack', '1 Main St.'), (2, 'Jill', '2 Main St.')",
        ],
        ["UPDATE customer_address SET cust_addr = '9 Main St.' WHERE cust_id = 2", COUNTS, "SELECT * FROM customer", "UPDATE customer_address SET cust_id = 9", "UPDATE customer_address SET cust_addr = cust_addr || '!', cust_id = cust_id", "SELECT * FROM customer", "INSERT INTO customer_address VALUES (1, 2)"],
    ),
    (
        "what count_changes counts",
        [VIEW, INSERTS, "CREATE TRIGGER gu INSTEAD OF UPDATE ON v WHEN new.a > 1 BEGIN UPDATE t SET a = new.a WHERE k < 3; END", "CREATE TRIGGER gd INSTEAD OF DELETE ON v BEGIN SELECT RAISE(IGNORE) WHERE old.k = 2; DELETE FROM u; END"],
        ["PRAGMA count_changes = 1", "INSERT INTO v VALUES (1, 2, 3), (4, 5, 6)", "INSERT INTO v DEFAULT VALUES", "UPDATE v SET a = 1", "UPDATE v SET a = 2 WHERE k = 3", "DELETE FROM v", "DELETE FROM v WHERE k > 99", "INSERT INTO v VALUES (1, 2, 3) RETURNING a", "INSERT INTO lg VALUES (1, 2, 3, 4)", "PRAGMA count_changes = 0", COUNTS],
    ),
    (
        "changes and last_insert_rowid in and after a trigger",
        [VIEW, "CREATE TRIGGER gi INSTEAD OF INSERT ON v BEGIN INSERT INTO t(a) VALUES (new.a), (new.a + 1); INSERT INTO lg VALUES ('c', changes(), last_insert_rowid(), NULL); UPDATE t SET b = 'w'; INSERT INTO lg VALUES ('c', changes(), last_insert_rowid(), NULL); END"],
        ["INSERT INTO u VALUES ('q', 99)", COUNTS, "INSERT INTO v(a) VALUES (100)", COUNTS, "INSERT INTO v(a) VALUES (200), (300)", COUNTS, "INSERT INTO u VALUES ('r', 98)", COUNTS, LOG],
    ),
    (
        "a view's columns convert what an UPDATE sets, and nothing an INSERT gives",
        [
            "CREATE TABLE w(i INTEGER, r REAL, x TEXT, b BLOB, n NUMERIC, q)",
            "INSERT INTO w VALUES ('1', '2', '3', '4', '5', '6')",
            "CREATE VIEW v AS SELECT i, r, x, b, n, q, i + 1 AS e, CAST(q AS TEXT) AS c, (i) AS p, +i AS s FROM w",
            "CREATE VIEW m AS SELECT i, x, r FROM w UNION ALL SELECT q, 1, i FROM w",
            "CREATE VIEW g AS SELECT x AS i, count(*) AS x, sum(i) AS r FROM w GROUP BY x",
        ]
        + [f"CREATE TRIGGER {v}{e} INSTEAD OF {op} ON {v} BEGIN INSERT INTO lg VALUES ('{v}{e}', {types}, NULL, NULL); END" for v, types in (("v", " || ".join(f"typeof(new.{c})" for c in "irxbnqecps")), ("m", "typeof(new.i) || typeof(new.x) || typeof(new.r)"), ("g", "typeof(new.i) || typeof(new.x) || typeof(new.r)")) for e, op in (("i", "INSERT"), ("u", "UPDATE"))],
        [
            "UPDATE v SET i = '7', r = '8', x = 9, b = 10, n = '11', q = '12', e = '13', c = 14, p = '15', s = '16' RETURNING *",
            "INSERT INTO v VALUES ('7', '8', 9, 10, '11', '12', '13', 14, '15', '16') RETURNING *",
            "UPDATE v SET r = 5, i = 2.0, n = '3.0', x = 1.5 RETURNING r, typeof(r), typeof(i), typeof(n), typeof(x)",
            "UPDATE m SET i = '7', x = 8, r = '9' RETURNING typeof(i), typeof(x), typeof(r)",
            "INSERT INTO m VALUES ('7', 8, '9') RETURNING typeof(i), typeof(x), typeof(r)",
            "UPDATE g SET i = 7, x = '8', r = '9' RETURNING typeof(i), typeof(x), typeof(r)",
            "INSERT INTO g VALUES (7, '8', '9') RETURNING typeof(i), typeof(x), typeof(r)",
            LOG,
        ],
    ),
    (
        "new and old compare as a trigger's columns do",
        ["CREATE TABLE w(i INTEGER, x TEXT COLLATE NOCASE, b BLOB)", "INSERT INTO w VALUES (1, 'Ab', 2)", "CREATE VIEW v AS SELECT i, x, b, x || '' AS e FROM w"]
        + [f"CREATE TRIGGER g{n} INSTEAD OF UPDATE ON v WHEN {when} BEGIN INSERT INTO lg VALUES ('{n}', 1, 2, 3); END" for n, when in enumerate(["old.i = '1'", "old.x = 'aB'", "old.x IN ('AB')", "new.b = '2'", "new.e = 'aB'", "new.x BETWEEN 'a' AND 'b'", "old.i || '' = 1", "(SELECT count(*) FROM w WHERE w.x = new.x) = 1", "CASE new.x WHEN 'AB' THEN 1 END"])]
        + ["CREATE TRIGGER h INSTEAD OF UPDATE ON v BEGIN INSERT INTO lg SELECT 'h', old.x = 'AB', old.i = '1', new.b = '2'; INSERT INTO lg SELECT 's', i, x, b FROM w WHERE x = new.x ORDER BY new.x; END"],
        ["UPDATE v SET b = 2", LOG],
    ),
    (
        "update of",
        [VIEW] + [f"CREATE TRIGGER g{n} INSTEAD OF UPDATE OF {columns} ON v BEGIN INSERT INTO lg VALUES ('{n}', new.k, new.a, new.b); END" for n, columns in enumerate(["a", "B, k", "zz", "a, zz", "rowid"])],
        ["UPDATE v SET a = 1 WHERE k = 1", "UPDATE v SET b = 1 WHERE k = 1", "UPDATE v SET K = 1 WHERE k = 1", "UPDATE v SET a = 1, b = 2 WHERE k = 1", "UPDATE v SET rowid = 1 WHERE k = 1", "UPDATE v SET oid = 1 WHERE k = 1", LOG],
    ),
    ("update of a column alone", [VIEW, "CREATE TRIGGER g INSTEAD OF UPDATE OF zz ON v BEGIN SELECT 1; END"], ["UPDATE v SET a = 1", "UPDATE v SET zz = 1"]),
    (
        "several triggers, their order",
        [VIEW] + [f"CREATE {temp}TRIGGER g{n} INSTEAD OF INSERT ON {on}v FOR EACH ROW BEGIN INSERT INTO lg VALUES ('{n}', new.k, (SELECT count(*) FROM lg), NULL); END" for n, (temp, on) in enumerate([("", ""), ("TEMP ", "main."), ("", "main."), ("TEMP ", "")])],
        ["INSERT INTO v VALUES (1, 2, 3), (4, 5, 6)", LOG, "DROP TRIGGER g1", "INSERT INTO v VALUES (7, 8, 9)", LOG, "SELECT name FROM sqlite_temp_schema ORDER BY name", "DROP VIEW v", "SELECT name FROM sqlite_schema WHERE type <> 'index' ORDER BY name", "SELECT name FROM sqlite_temp_schema ORDER BY name", VIEW, "INSERT INTO v VALUES (1, 2, 3)"],
    ),
    (
        "temp views and triggers",
        ["CREATE TEMP VIEW tv AS SELECT k, a FROM t", "CREATE TRIGGER g INSTEAD OF INSERT ON tv BEGIN INSERT INTO lg VALUES ('tv', new.k, new.a, NULL); INSERT INTO t(a) VALUES (new.a); END", VIEW, "CREATE TEMP VIEW v AS SELECT k + 100 AS k, a, b FROM t", "CREATE TEMP TRIGGER g2 INSTEAD OF DELETE ON v BEGIN INSERT INTO lg VALUES ('temp', old.k, NULL, NULL); END", "CREATE TRIGGER g3 INSTEAD OF DELETE ON main.v BEGIN INSERT INTO lg VALUES ('main', old.k, NULL, NULL); END"],
        ["INSERT INTO tv VALUES (1, 2)", "DELETE FROM v WHERE k % 100 = 1", "DELETE FROM main.v WHERE k = 1", "DELETE FROM temp.v WHERE k = 101", LOOK, "SELECT name, tbl_name FROM sqlite_temp_schema ORDER BY name", "DROP VIEW temp.v", "DELETE FROM v WHERE k = 1", "SELECT name, tbl_name FROM sqlite_temp_schema ORDER BY name", "SELECT name FROM sqlite_schema WHERE type = 'trigger' ORDER BY name", LOG],
    ),
    (
        "raise",
        [VIEW, "CREATE TRIGGER gi INSTEAD OF INSERT ON v BEGIN INSERT INTO lg VALUES ('I', new.k, NULL, NULL); SELECT CASE new.k WHEN 2 THEN RAISE(IGNORE) WHEN 3 THEN RAISE(FAIL, 'fail') WHEN 4 THEN RAISE(ABORT, 'abort') WHEN 5 THEN RAISE(ROLLBACK, 'rollback') END; INSERT INTO lg VALUES ('J', new.k, NULL, NULL); END", "CREATE TRIGGER gj INSTEAD OF INSERT ON v BEGIN INSERT INTO lg VALUES ('K', new.k, NULL, NULL); END"],
        ["INSERT INTO v(k) VALUES (1), (2), (6) RETURNING k", LOG, "INSERT INTO v(k) VALUES (1), (3), (6) RETURNING k", LOG, "INSERT INTO v(k) VALUES (1), (4), (6)", LOG, "BEGIN", "INSERT INTO v(k) VALUES (7)", "INSERT INTO v(k) VALUES (1), (4), (6)", LOG, "INSERT INTO v(k) VALUES (1), (3), (6)", LOG, "INSERT INTO v(k) VALUES (1), (5), (6)", LOG, "COMMIT", LOG],
    ),
    (
        "a trigger that writes its own view",
        [VIEW, "CREATE TRIGGER g INSTEAD OF INSERT ON v BEGIN INSERT INTO lg VALUES ('v', new.k, (SELECT count(*) FROM lg), NULL); INSERT INTO v VALUES (new.k + 1, 0, 0); INSERT INTO lg VALUES ('w', new.k, changes(), NULL); END", "CREATE TRIGGER h INSTEAD OF DELETE ON v BEGIN INSERT INTO lg VALUES ('d', old.k, NULL, NULL); INSERT INTO v VALUES (old.k * 10, 0, 0); END"],
        ["INSERT INTO v VALUES (1, 2, 3)", LOG, COUNTS, "DELETE FROM v WHERE k < 3", LOG],
    ),
    (
        "recursive triggers",
        [
            VIEW,
            "CREATE VIEW o AS SELECT k, a FROM t",
            "CREATE TRIGGER g INSTEAD OF INSERT ON v WHEN new.k < 6 BEGIN INSERT INTO lg VALUES ('v', new.k, (SELECT count(*) FROM lg), changes()); INSERT INTO v VALUES (new.k + 1, new.a, new.b); INSERT INTO lg VALUES ('w', new.k, NULL, changes()); END",
            "CREATE TRIGGER h INSTEAD OF UPDATE ON v WHEN new.a < 14 BEGIN UPDATE t SET a = new.a WHERE k = old.k; UPDATE v SET a = new.a + 1 WHERE k = old.k; INSERT INTO lg VALUES ('h', old.a, new.a, NULL); END",
            "CREATE TRIGGER i INSTEAD OF DELETE ON v BEGIN INSERT INTO lg VALUES ('i', old.k, NULL, NULL); DELETE FROM t WHERE k = old.k; DELETE FROM o WHERE k > old.k; END",
            "CREATE TRIGGER j INSTEAD OF DELETE ON o BEGIN INSERT INTO lg VALUES ('j', old.k, NULL, NULL); DELETE FROM v WHERE k = old.k; END",
        ],
        sum((["PRAGMA recursive_triggers = " + on, "BEGIN", "INSERT INTO v VALUES (1, 0, 0), (4, 0, 0)", "UPDATE v SET a = a + 1 WHERE k = 1 RETURNING a", LOOK, COUNTS, "DELETE FROM v WHERE k = 1 RETURNING k", LOOK, "ROLLBACK"] for on in ("OFF", "ON", "OFF")), []),
    ),
    (
        "recursive triggers without end",
        ["CREATE VIEW e AS SELECT k FROM t", "CREATE TRIGGER ei INSTEAD OF INSERT ON e BEGIN INSERT INTO u VALUES (new.k, new.k); INSERT INTO e VALUES (new.k + 1); END"],
        ["INSERT INTO e VALUES (100)", "SELECT count(*) FROM u", "PRAGMA recursive_triggers = ON", "INSERT INTO e VALUES (200)", "SELECT count(*) FROM u", "PRAGMA recursive_triggers = OFF", "INSERT INTO e VALUES (200)", "SELECT count(*) FROM u"],
    ),
    (
        "views over views, each with its triggers",
        [VIEW, "CREATE VIEW o(p, q) AS SELECT k, a FROM v WHERE a > 5", "CREATE TRIGGER gv INSTEAD OF UPDATE ON v BEGIN UPDATE t SET a = new.a, b = new.b WHERE k = old.k; END", "CREATE TRIGGER go INSTEAD OF UPDATE ON o BEGIN UPDATE v SET a = new.q WHERE k = old.p; INSERT INTO lg VALUES ('o', old.p, new.p, changes()); END", "CREATE TRIGGER gp INSTEAD OF DELETE ON o BEGIN UPDATE o SET q = 0 WHERE p = old.p; END"],
        ["UPDATE o SET q = q + 1 WHERE p > 1", LOOK, COUNTS, "DELETE FROM o WHERE p = 2 RETURNING *", LOOK, "UPDATE o SET q = q + 1 RETURNING p, q", LOOK, COUNTS],
    ),
    (
        "a table's triggers write a view",
        [VIEW, INSERTS, "CREATE TRIGGER ta AFTER INSERT ON u BEGIN INSERT INTO v VALUES (new.n, new.k, 'ta'); DELETE FROM v WHERE k = 1; END", DELETES],
        ["INSERT INTO u VALUES ('m', 50), ('n', 51)", LOG, COUNTS, "DROP VIEW v", "INSERT INTO u VALUES ('o', 52)", "SELECT count(*) FROM u"],
    ),
    (
        "a trigger changes what the statement reads",
        [VIEW, "CREATE TRIGGER gd INSTEAD OF DELETE ON v BEGIN DELETE FROM t WHERE k <> old.k; INSERT INTO lg VALUES ('D', old.k, (SELECT count(*) FROM t), NULL); END", "CREATE TRIGGER gu INSTEAD OF UPDATE ON v BEGIN UPDATE t SET a = a + 1; INSERT INTO lg VALUES ('U', old.k, old.a, new.a); END", "CREATE TRIGGER gi INSTEAD OF INSERT ON v BEGIN INSERT INTO t(a) VALUES (new.a); END"],
        ["UPDATE v SET a = (SELECT sum(a) FROM t WHERE k <= v.k)", LOG, "INSERT INTO v SELECT k, a, b FROM v", LOOK, "DELETE FROM v WHERE k IN (SELECT k FROM t) RETURNING k", LOOK],
    ),
    (
        "the statement's clause is its triggers' statements'",
        [VIEW, "CREATE TRIGGER g INSTEAD OF INSERT ON v BEGIN INSERT INTO lg VALUES ('b', new.a, NULL, NULL); INSERT OR IGNORE INTO u VALUES (new.b, new.a); INSERT INTO lg VALUES ('a', new.a, NULL, NULL); END", "CREATE TRIGGER h INSTEAD OF UPDATE ON v BEGIN INSERT INTO lg VALUES ('b', new.a, NULL, NULL); UPDATE u SET n = new.a WHERE k = new.b; END", "CREATE TRIGGER i INSTEAD OF DELETE ON v BEGIN INSERT OR REPLACE INTO u VALUES (old.b, 2); END"],
        sum(([f"INSERT{c} INTO v VALUES (1, 50, 'p'), (2, 1, 'q'), (3, 51, 'r')", LOOK, f"UPDATE{c} v SET a = 2, b = 'a' WHERE k < 3", LOOK] for c in ["", " OR IGNORE", " OR FAIL", " OR REPLACE", " OR ABORT", " OR ROLLBACK"]), []) + ["DELETE FROM v WHERE k < 3", LOOK],
    ),
    (
        "in a transaction and a savepoint",
        [VIEW, "CREATE TRIGGER g INSTEAD OF INSERT ON v BEGIN INSERT INTO t(k, a) VALUES (new.k, new.a); END"],
        ["BEGIN", "INSERT INTO v VALUES (10, 1, 1)", "INSERT INTO v VALUES (11, 1, 1), (1, 1, 1), (12, 1, 1)", LOOK, "SAVEPOINT s", "INSERT INTO v VALUES (13, 1, 1)", "ROLLBACK TO s", LOOK, "RELEASE s", "ROLLBACK", LOOK, "INSERT INTO v VALUES (10, 1, 1)", "INSERT INTO v VALUES (11, 1, 1), (1, 1, 1), (12, 1, 1)", LOOK],
    ),
    (
        "views of every kind",
        [
            "CREATE VIEW v1 AS SELECT t.k, u.n AS a, u.k AS b FROM t JOIN u ON u.n = t.k",
            "CREATE VIEW v2 AS SELECT t.k, u.n AS a, u.k AS b FROM t LEFT JOIN u ON u.n = t.k",
            "CREATE VIEW v3 AS SELECT b AS k, count(*) AS a, max(k) AS b FROM t GROUP BY b",
            "CREATE VIEW v4 AS SELECT k, a, b FROM t UNION ALL SELECT k, n, 'u' FROM u",
            "CREATE VIEW v5 AS SELECT b AS k, 1 AS a, 2 AS b FROM t UNION SELECT k, 1, 2 FROM u",
            "CREATE VIEW v6 AS WITH q(x, y) AS (SELECT k, a FROM t WHERE k > 1) SELECT x AS k, y AS a, x + y AS b FROM q",
            "CREATE VIEW v7 AS WITH RECURSIVE q(x) AS (VALUES (1) UNION ALL SELECT x + 1 FROM q WHERE x < 4) SELECT x AS k, x * x AS a, 'r' AS b FROM q",
            "CREATE VIEW v8 AS SELECT DISTINCT b AS k, 1 AS a, b FROM t",
            "CREATE VIEW v9 AS SELECT column1 AS k, column2 AS a, column3 AS b FROM (VALUES (1, 2, 'x'), (3, 4, 'y'))",
            "CREATE VIEW v10 AS SELECT k, sum(a) OVER (ORDER BY k) AS a, b FROM t",
            "CREATE VIEW v11 AS SELECT k, a, b FROM t ORDER BY a DESC LIMIT 2",
            "CREATE VIEW v12 AS SELECT * FROM (SELECT k, a, c AS b FROM t)",
            "CREATE VIEW v13 AS SELECT k, a, b FROM v4 WHERE a < 25",
            "CREATE VIEW v14 AS SELECT max(k) AS k, sum(a) AS a, 'all' AS b FROM t",
            "CREATE VIEW v15 AS SELECT k, CAST((SELECT n FROM u WHERE n = t.k) AS INT) AS a, EXISTS (SELECT 1 FROM u WHERE n = t.k) AS b FROM t",
            "CREATE VIEW v16 AS SELECT x.k, y.a, x.b FROM t AS x FULL JOIN t AS y ON y.k = x.k + 1",
        ]
        + [f"CREATE TRIGGER g{n}{e} INSTEAD OF {op} ON v{n} BEGIN INSERT INTO lg VALUES ('v{n}{e}', {', '.join(f'quote({row}.{c})' for c in 'kab')}); END" for n in range(1, 17) for e, op, row in (("i", "INSERT", "new"), ("u", "UPDATE", "new"), ("w", "UPDATE", "old"), ("d", "DELETE", "old"))],
        sum(([f"INSERT INTO v{n} VALUES ('1', 2, x'33')", f"UPDATE v{n} SET a = '5', b = k", f"UPDATE v{n} SET k = a || b WHERE a > 1 RETURNING k, typeof(k), a, b", f"DELETE FROM v{n} WHERE k IS NOT NULL RETURNING *", f"DELETE FROM v{n}", LOOK, "DELETE FROM lg"] for n in range(1, 17)), []),
    ),
]


class Draw:
    def __init__(self, rng):
        self.rng = rng
        self.tables = [
            rng.choice(["CREATE TABLE t(k INTEGER PRIMARY KEY, a INT, b TEXT, c REAL)", "CREATE TABLE t(k INT PRIMARY KEY, a INT, b TEXT, c REAL) WITHOUT ROWID", "CREATE TABLE t(k INT, a INT CHECK (a <> 4), b TEXT NOT NULL DEFAULT 'd', c REAL UNIQUE)"]),
            rng.choice(["CREATE TABLE u(k TEXT PRIMARY KEY, n INT UNIQUE)", "CREATE TABLE u(k TEXT PRIMARY KEY, n INT UNIQUE) WITHOUT ROWID"]),
            "CREATE TABLE lg(ev, x, y, z)",
            "INSERT INTO t VALUES (1, 10, 'x', 1.5), (2, 20, 'y', 2.5), (3, 30, 'x', 3.5)",
            "INSERT INTO u VALUES ('a', 1), ('b', 2), ('z', 7)",
        ]
        self.keyed = any("WITHOUT ROWID" in table for table in self.tables)
        # The order a join's or a group's rows come in is the planner's: ORDER BY settles it.
        query = self.query()
        self.flat = not any(word in query for word in ("ORDER", "GROUP", "UNION", "DISTINCT", "OVER", "LIMIT", "WITH", "JOIN"))
        self.views = [f"CREATE VIEW v{self.columns()} AS {query}"]
        self.nested = rng.random() < 0.4
        if self.nested:
            self.views.append(rng.choice(["CREATE VIEW o AS SELECT k, a, b FROM v", "CREATE VIEW o AS SELECT b AS k, k AS a, a AS b FROM v WHERE a IS NOT NULL", "CREATE VIEW o(k, a, b) AS SELECT k + 1, a, b FROM v"]))
        self.triggers = [self.trigger(i) for i in range(rng.choice([2, 3, 4, 5]))]

    def columns(self):
        return self.rng.choice(["", "", "(k, a, b)"])

    def query(self):
        return self.rng.choice([
            "SELECT k, a, b FROM t",
            "SELECT k, a, b FROM t",
            "SELECT k, a, b FROM t WHERE a > 5",
            "SELECT k, c AS a, b FROM t",
            "SELECT a AS k, k AS a, b FROM t",
            "SELECT k, a + 1 AS a, upper(b) AS b FROM t",
            "SELECT t.k, u.n AS a, u.k AS b FROM t JOIN u ON u.n = t.k ORDER BY 1",
            "SELECT t.k, u.n AS a, t.b FROM t LEFT JOIN u ON u.n = t.k ORDER BY 1",
            "SELECT b AS k, count(*) AS a, max(a) AS b FROM t GROUP BY b ORDER BY 1",
            "SELECT k, a, b FROM t UNION ALL SELECT n, n + 40, k FROM u ORDER BY 2",
            "SELECT DISTINCT b AS k, length(b) AS a, b FROM t ORDER BY 1",
            "WITH q(x, y) AS (SELECT k, a FROM t) SELECT x AS k, y AS a, x || y AS b FROM q",
            "SELECT k, sum(a) OVER (ORDER BY k) AS a, b FROM t ORDER BY k",
            "SELECT k, a, b FROM t ORDER BY a DESC LIMIT 2",
            "SELECT k, +(SELECT n FROM u WHERE n = t.k) AS a, b FROM t",
        ])

    def view(self):
        return "o" if self.nested and self.rng.random() < 0.4 else "v"

    def value(self):
        return self.rng.choice(["1", "2", "3", "4", "'5'", "'x'", "'a'", "NULL", "2.0", "7"])

    def trigger(self, i):
        rng = self.rng
        view = self.view()
        event = rng.choice(["INSERT", "INSERT", "DELETE", "UPDATE", "UPDATE", "UPDATE OF"])
        text = event
        if event == "UPDATE OF":
            text = "UPDATE OF " + ", ".join(rng.sample(["k", "a", "b", "zz"], rng.choice([1, 2])))
            event = "UPDATE"
        rows = {"INSERT": ["new"], "DELETE": ["old"], "UPDATE": ["old", "new"]}[event]
        read = lambda: f"{rng.choice(rows)}.{rng.choice('kab')}"
        when = rng.choice(["", "", "", f" WHEN {read()} > 2", f" WHEN {read()} IS NOT NULL", f" WHEN {read()} IN (SELECT n FROM u) OR {read()} = 'x'", f" WHEN EXISTS (SELECT 1 FROM u WHERE u.n = {read()})"])
        body = [f"INSERT INTO lg VALUES ('{event[0]}{i}', quote({read()}), typeof({read()}), {read()})"]
        for _ in range(rng.choice([0, 1, 1, 2])):
            drawn = rng.random()
            orc = rng.choice(["", "", " OR IGNORE", " OR REPLACE", " OR ABORT", " OR FAIL", " OR ROLLBACK"])
            if drawn < 0.2:
                body.append(f"INSERT{orc} INTO u VALUES ({read()}, {rng.choice(['1', '2', '8', read()])})")
            elif drawn < 0.35:
                body.append(f"UPDATE{orc} t SET a = {rng.choice(['a + 1', '2', '4', read()])} WHERE k = {rng.choice(['1', read()])}")
            elif drawn < 0.45:
                body.append(f"DELETE FROM u WHERE n = {read()}")
            elif drawn < 0.53:
                body.append(f"INSERT INTO lg SELECT 's', k, n, {read()} FROM u WHERE n < 3")
            elif drawn < 0.6:
                body.append(f"INSERT INTO u VALUES ({read()}, 3) ON CONFLICT DO UPDATE SET n = excluded.n + 10")
            elif drawn < 0.75:
                body.append(f"SELECT RAISE({rng.choice(['IGNORE', 'IGNORE', 'ABORT, ' + repr('no'), 'FAIL, ' + repr('no'), 'ROLLBACK, ' + repr('no')])}) WHERE {read()} = {rng.choice(['2', '3', repr('x'), '20'])}")
            elif drawn < 0.9:
                other = self.view()
                body.append(rng.choice([f"INSERT{orc} INTO {other} VALUES ({read()}, {read()}, 'n')", f"UPDATE{orc} {other} SET a = {read()} WHERE k = {read()}", f"DELETE FROM {other} WHERE a = {read()}"]))
            else:
                body.append(f"SELECT {read()}")
        temp = rng.choice(["", "", "", "TEMP "])
        each = rng.choice(["", " FOR EACH ROW"])
        return f"CREATE {temp}TRIGGER g{i} INSTEAD OF {text} ON {view}{each}{when} BEGIN {'; '.join(body)}; END"

    def statement(self):
        rng = self.rng
        view = self.view()
        orc = rng.choice(["", "", "", ""] + [f" OR {c}" for c in CONFLICTS])
        returning = rng.choice(["", "", "", " RETURNING *", " RETURNING k, a || b AS s, typeof(a)", " RETURNING b, (SELECT count(*) FROM u)"])
        drawn = rng.random()
        if drawn < 0.22:
            rows = ", ".join(f"({self.value()}, {self.value()}, {self.value()})" for _ in range(rng.choice([1, 1, 2, 3])))
            return f"INSERT{orc} INTO {view} VALUES {rows}{returning}"
        if drawn < 0.3:
            columns = rng.sample(["k", "a", "b"], rng.choice([1, 2]))
            return f"INSERT{orc} INTO {view}({', '.join(columns)}) VALUES ({', '.join(self.value() for _ in columns)}){returning}"
        if drawn < 0.4:
            return f"INSERT{orc} INTO {view} " + rng.choice(["SELECT k, a, b FROM t ORDER BY k", "SELECT k, n, 's' FROM u ORDER BY n", "SELECT * FROM v", "DEFAULT VALUES", "SELECT n, n, k FROM u WHERE n > 1 ORDER BY n"]) + returning
        if drawn < 0.72:
            sets = ", ".join(rng.sample([f"a = {rng.choice(['a + 1', '3', repr('4'), 'NULL', 'k', '(SELECT max(n) FROM u)', '(SELECT n FROM u WHERE n = k)'])}", f"b = {rng.choice([repr('x'), '5', 'b || a', 'NULL', '(SELECT count(*) FROM lg)'])}", f"k = {rng.choice(['k + 1', repr('2'), 'b'])}"], rng.choice([1, 1, 2, 3])))
            source = ""
            where = rng.choice(["", " WHERE k = 1", " WHERE a > 10", " WHERE k IN (SELECT n FROM u)", " WHERE b IS NOT NULL", f" WHERE {view}.b = 'x'"])
            if self.flat and rng.random() < 0.12:
                source = " FROM u AS w"
                where = f" WHERE w.n = {view}.k"
                sets = sets.replace("n = k", f"n = {view}.k").replace("a = k", f"a = {view}.k").replace("k = k", f"k = {view}.k").replace("k = b", f"k = {view}.b")
            return f"UPDATE{orc} {view} SET {sets}{source}{where}{returning}"
        where = rng.choice(["", " WHERE k = 1", " WHERE a > 10", " WHERE k IN (SELECT n FROM u)", " WHERE b = 'x'", f" WHERE EXISTS (SELECT 1 FROM u WHERE u.n = {view}.k)"])
        return f"DELETE FROM {view}{where}{returning}"


def text(value):
    if value is None:
        return "NULL"
    if isinstance(value, float):
        return f"{value:.9f}"
    if isinstance(value, bytes):
        return value.decode()
    return str(value)


def answer(db, sql):
    try:
        rows = sorted(",".join(text(value) for value in row) for row in db.execute(sql).fetchall())
    except sqlite3.Error as error:
        return "ERROR" if sql in WORDS_NOT_HELD else f"ERROR~{error}"
    return f"{len(rows)}:{';'.join(rows)}"


def ask(db, sql, out):
    got = answer(db, sql)
    out.append(f"q\t{sql}\t{got}")
    return not got.startswith("ERROR")


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 300
    seed = sys.argv[2] if len(sys.argv) > 2 else "C28"
    print(f"# SQLite {sqlite3.sqlite_version}: bench/sqlite_instead_of.py {count} {seed}")
    out = []
    for name, fill, statements in FORMS:
        db = sqlite3.connect(":memory:", isolation_level=None)
        out.append(f"db\t{name}")
        for statement in BASE + fill:
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
        lines = []
        try:
            for statement in draw.tables + draw.views + draw.triggers:
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
            statement = draw.statement()
            if " RETURNING" in statement:
                try:
                    db.execute("EXPLAIN " + statement.split(" RETURNING")[0])
                except sqlite3.OperationalError:
                    continue
            if ask(db, statement, out) and not draw.keyed:
                ask(db, COUNTS, out)
            ask(db, LOOK, out)
            ask(db, "DELETE FROM lg", out)
        if transaction:
            ask(db, rng.choice(["COMMIT", "ROLLBACK"]), out)
            ask(db, LOOK, out)
        ask(db, "PRAGMA integrity_check", out)
    print("\n".join(out))
    print(f"# {made} drawn, {refused} SQLite refused", file=sys.stderr)


main()
