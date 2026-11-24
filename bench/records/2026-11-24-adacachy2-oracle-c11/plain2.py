#!/usr/bin/env python3
"""Lane C11: more causes in plain SQL. usage: plain2.py OUT_DIR"""
import os, sys
out = sys.argv[1]; os.makedirs(out, exist_ok=True)
E = ["CREATE TABLE edge(src TEXT, dst TEXT)", "INSERT INTO edge VALUES ('1', '2'), ('2', '3')"]
T = ["CREATE TABLE t(id INTEGER PRIMARY KEY, v INT)", "INSERT INTO t VALUES (1, 5), (2, 3), (3, 9)"]
P = {
 "recursive_text_join2": (E, ["WITH RECURSIVE r(n, d) AS (SELECT 1, 0 UNION ALL SELECT r.n + 1, r.d + 1 FROM r JOIN edge ON edge.src = r.n WHERE r.d < 5) SELECT n, d FROM r",
    "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT r.n + 1 FROM edge JOIN r ON edge.src = r.n) SELECT n FROM r", "WITH r(n) AS (SELECT 1 UNION ALL SELECT 2) SELECT edge.dst FROM r JOIN edge ON edge.src = r.n",
    "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r WHERE n < 3) SELECT r.n, edge.dst FROM r JOIN edge ON edge.src = r.n"]),
 "window_order_by2": (T, ["SELECT id FROM t ORDER BY ntile(2) OVER (ORDER BY id), lag(v + 1) OVER (ORDER BY v, id), typeof(lag(v + 1) OVER (ORDER BY v, id)) LIMIT 2"]),
 "window_order_by3": (T, ["SELECT ntile(2) OVER (ORDER BY id), lag(v + 1) OVER (ORDER BY v, id), typeof(lag(v + 1) OVER (ORDER BY v, id)) FROM t"]),
 "window_order_by4": (T, ["SELECT id FROM t ORDER BY sum(v) OVER (ORDER BY id), first_value(v + 1) OVER (ORDER BY v)"]),
 "window_order_by5": (T, ["SELECT id, lag(v + 1) OVER (ORDER BY v, id) AS p, sum(v) OVER (ORDER BY id) AS s FROM t ORDER BY s, p, coalesce(lag(v + 1) OVER (ORDER BY v, id), 0)"]),
 "window_order_by6": (T, ["SELECT id FROM t ORDER BY count(*) OVER (ORDER BY id), abs(lag(v + 1) OVER (ORDER BY v, id))"]),
 "row_misused2": (["CREATE TABLE a(x INT, y INT)", "CREATE TABLE b(x INT, y INT)", "INSERT INTO a VALUES (1, 2), (3, 4)", "INSERT INTO b VALUES (1, 2)"],
   ["SELECT a.x FROM a LEFT JOIN b ON (a.x, NULL) <> (SELECT max(x), count(y) FROM b) UNION SELECT 1", "SELECT a.x FROM a WHERE (1 = a.x AND ((a.x, a.y) < (SELECT max(x), max(y) FROM b) OR a.y > 3))",
    "SELECT a.x FROM a LEFT JOIN b ON (a.x, a.y) < (SELECT max(x), max(y) FROM b) WHERE a.x > 0", "SELECT a.x FROM a WHERE NOT ((a.x, a.y) = (SELECT max(x), max(y) FROM b) OR a.y > 3)", "SELECT a.x FROM a, b WHERE a.x = b.x AND (a.x, b.y) >= (SELECT max(x), max(y) FROM b)",
    "SELECT a.x FROM a WHERE ((a.x, a.y) < (SELECT max(x), max(y) FROM b)) IS NULL"]),
 "full_join_without_rowid2": (["CREATE TABLE w(k TEXT PRIMARY KEY, u INT UNIQUE) WITHOUT ROWID", "CREATE TABLE o(k TEXT)", "INSERT INTO w VALUES ('a', 1), ('b', 2)", "INSERT INTO o VALUES ('a'), ('c')"],
   ["SELECT count(*) FROM o FULL JOIN w", "SELECT count(*) FROM o FULL JOIN w ON 1", "SELECT count(*) FROM o RIGHT JOIN w ON o.k > 'b'", "SELECT count(*) FROM o LEFT JOIN w ON o.k > 'b'", "SELECT o.k, w.u FROM o FULL JOIN w ON w.u > 1", "SELECT count(*) FROM w AS a FULL JOIN w AS b"]),
 "upsert_strict_text_pk": (["CREATE TABLE kv(id INTEGER PRIMARY KEY, name TEXT UNIQUE, n INT) STRICT", "INSERT INTO kv VALUES (1, 'a', 0)", "INSERT INTO kv VALUES (7, 'a', 5) ON CONFLICT(name) DO UPDATE SET id = excluded.id, n = excluded.n"], ["SELECT * FROM kv"]),
 "upsert_strict_rowid": (["CREATE TABLE kv(id INTEGER PRIMARY KEY, n INT) STRICT", "INSERT INTO kv VALUES (1, 0)", "INSERT INTO kv VALUES (1, 5) ON CONFLICT(id) DO UPDATE SET n = excluded.n + excluded.rowid"], ["SELECT * FROM kv"]),
 "trigger_strict_after": (["CREATE TABLE price(id INTEGER PRIMARY KEY, amount REAL) STRICT", "CREATE TABLE audit(amount)", "CREATE TRIGGER g AFTER INSERT ON price BEGIN INSERT INTO audit VALUES (new.amount); END", "INSERT INTO price VALUES (1, 100)"], ["SELECT amount, typeof(amount) FROM audit"]),
 "trigger_plain_before": (["CREATE TABLE price(id INTEGER PRIMARY KEY, amount REAL)", "CREATE TABLE audit(amount)", "CREATE TRIGGER g BEFORE INSERT ON price BEGIN INSERT INTO audit VALUES (new.amount); END", "INSERT INTO price VALUES (1, 100)"], ["SELECT amount, typeof(amount) FROM audit"]),
 "trigger_collation_old": (["CREATE TABLE u(id INTEGER PRIMARY KEY, name TEXT COLLATE NOCASE)", "CREATE TABLE lg(a)", "INSERT INTO u VALUES (1, 'ADMIN')", "CREATE TRIGGER g AFTER DELETE ON u WHEN old.name = 'admin' BEGIN INSERT INTO lg VALUES (old.name); END", "DELETE FROM u WHERE id = 1"], ["SELECT * FROM lg"]),
 "upsert_collation": (["CREATE TABLE u(id INTEGER PRIMARY KEY, name TEXT COLLATE NOCASE, hit INT)", "INSERT INTO u VALUES (1, 'ADMIN', 0)", "INSERT INTO u VALUES (1, 'Admin', 1) ON CONFLICT(id) DO UPDATE SET hit = (excluded.name = u.name)"], ["SELECT * FROM u"]),
 "view_affinity": (["CREATE TABLE a(x REAL)", "INSERT INTO a VALUES (2)", "CREATE VIEW v AS SELECT x FROM a UNION ALL SELECT x FROM a"], ["SELECT x, typeof(x) FROM v", "SELECT * FROM v WHERE x = '2'"]),
}
for n, (st, qs) in P.items():
    with open(f"{out}/{n}.md", "w") as f:
        f.write(f"# diff Wrong: {n}\n\nplain\n\n## Statements\n\n```sql\n" + "".join(s + ";\n" for s in st) + "```\n\n## Queries\n\n```sql\n" + "".join(q + ";\n" for q in qs) + "```\n\n## What differed\n\n```\n```\n")
