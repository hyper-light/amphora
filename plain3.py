#!/usr/bin/env python3
"""Lane C11: the window panic and others in plain SQL. usage: plain3.py OUT_DIR"""
import os, sys
out = sys.argv[1]; os.makedirs(out, exist_ok=True)
T = ["CREATE TABLE ev(id INTEGER PRIMARY KEY, at TEXT, v INT)", "INSERT INTO ev VALUES (1, '2026-01-01 10:00', 5), (2, '2026-01-01 11:00', 3), (3, '2026-01-02 09:00', 9)"]
P = {
 "window_date1": (T, ["SELECT id, lag(v + 1) OVER (ORDER BY date(at), id) AS p, typeof(lag(v + 1) OVER (ORDER BY date(at), id)) FROM ev"]),
 "window_date2": (T, ["SELECT id FROM ev ORDER BY lag(v + 1) OVER (ORDER BY date(at), id), typeof(lag(v + 1) OVER (ORDER BY date(at), id))"]),
 "window_date3": (T, ["SELECT id FROM ev ORDER BY sum(v) OVER (ORDER BY id), lag(v + 1) OVER (ORDER BY date(at), id), typeof(lag(v + 1) OVER (ORDER BY date(at), id))"]),
 "window_date4": (T, ["SELECT id FROM ev ORDER BY abs(lag(v + 1) OVER (ORDER BY date(at), id))"]),
 "window_date5": (T, ["SELECT id, sum(v) OVER (ORDER BY id), lag(v + 1) OVER (ORDER BY date(at), id), coalesce(lag(v + 1) OVER (ORDER BY date(at), id), 0) FROM ev"]),
 "window_date6": (T, ["SELECT id, first_value(v + 1) OVER (PARTITION BY date(at) ORDER BY id), first_value(v + 1) OVER (PARTITION BY date(at) ORDER BY id) - v FROM ev"]),
 "window_date7": (T, ["SELECT 1 FROM ev ORDER BY NULL, ntile(1) OVER (ORDER BY ev.rowid), lag('x') OVER (ORDER BY v, strftime(v, 1.0), ev.rowid DESC), typeof(lag('x') OVER (ORDER BY v, strftime(v, 1.0), ev.rowid DESC)) LIMIT 2 OFFSET 0"]),
 "window_date8": (T, ["SELECT 1 FROM ev ORDER BY ntile(1) OVER (ORDER BY id), typeof(lag('x') OVER (ORDER BY strftime(v, 1.0)))"]),
 "window_date9": (T, ["SELECT 1 FROM ev ORDER BY ntile(1) OVER (ORDER BY id), lag('x') OVER (ORDER BY strftime(v, 1.0))"]),
 "row_call": (["CREATE TABLE t(a TEXT, b TEXT)", "INSERT INTO t VALUES ('x', 'y')"], ["SELECT * FROM t WHERE (a, b) = ('x', json_extract('[\"y\"]', '$[0]'))", "SELECT * FROM t WHERE (a, b) < ('x', 'z')", "SELECT * FROM t WHERE (a, b) BETWEEN ('x', 'a') AND ('x', 'z')",
    "SELECT * FROM t WHERE (a, upper(b)) BETWEEN ('x', 'A') AND ('x', 'Z')", "SELECT ('a', 1) BETWEEN ('a', 0) AND ('a', 2)", "SELECT (a, 1) BETWEEN (a, 0) AND (a, 2) FROM t", "SELECT ('k', b) > ('j', 'x') FROM t", "SELECT * FROM t WHERE (a, b) IN (VALUES ('x', 'y'))", "SELECT * FROM t WHERE (a, 5) >= (b, 1)"]),
 "trigger_update_of_rowid": (["CREATE TABLE t(k TEXT PRIMARY KEY, v TEXT)", "CREATE TABLE lg(a)", "CREATE TRIGGER g AFTER UPDATE OF k ON t BEGIN INSERT INTO lg VALUES (old.k); END", "INSERT INTO t VALUES ('a', 'x')", "UPDATE t SET k = 'b' WHERE v = 'x'"], ["SELECT * FROM t", "SELECT * FROM lg"]),
 "trigger_update_of_other_wr": (["CREATE TABLE t(k TEXT PRIMARY KEY, v TEXT) WITHOUT ROWID", "CREATE TABLE lg(a)", "CREATE TRIGGER g AFTER UPDATE OF v ON t BEGIN INSERT INTO lg VALUES (old.k); END", "INSERT INTO t VALUES ('a', 'x')", "UPDATE t SET k = 'b' WHERE v = 'x'"], ["SELECT * FROM t", "SELECT * FROM lg"]),
 "trigger_update_of_v_wr": (["CREATE TABLE t(k TEXT PRIMARY KEY, v TEXT) WITHOUT ROWID", "CREATE TABLE lg(a)", "CREATE TRIGGER g AFTER UPDATE OF v ON t BEGIN INSERT INTO lg VALUES (old.k); END", "INSERT INTO t VALUES ('a', 'x')", "UPDATE t SET v = 'b' WHERE k = 'a'"], ["SELECT * FROM t", "SELECT * FROM lg"]),
}
for n, (st, qs) in P.items():
    with open(f"{out}/{n}.md", "w") as f:
        f.write(f"# diff Wrong: {n}\n\nplain\n\n## Statements\n\n```sql\n" + "".join(s + ";\n" for s in st) + "```\n\n## Queries\n\n```sql\n" + "".join(q + ";\n" for q in qs) + "```\n\n## What differed\n\n```\n```\n")
