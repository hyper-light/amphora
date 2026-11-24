#!/usr/bin/env python3
"""Lane C11: USING's merged column, row BETWEEN, INSERT OR IGNORE, the compound's collation, in plain SQL. usage: plain4.py OUT_DIR"""
import os, sys
out = sys.argv[1]; os.makedirs(out, exist_ok=True)
P = {
 "using_full_affinity": (["CREATE TABLE a(k TEXT, x INT)", "CREATE TABLE b(k REAL, y INT)", "INSERT INTO a VALUES ('1', 1)", "INSERT INTO b VALUES (2.0, 2)"],
   ["SELECT y FROM a FULL JOIN b USING (k) WHERE k = 2", "SELECT y, k = 2 FROM a RIGHT JOIN b USING (k)", "SELECT y FROM a NATURAL FULL JOIN b WHERE k = 2", "SELECT y FROM a FULL JOIN b USING (k) WHERE coalesce(a.k, b.k) = 2", "SELECT k, typeof(k) FROM a FULL JOIN b USING (k)"]),
 "using_full_int_text": (["CREATE TABLE a(id INTEGER, x TEXT)", "CREATE TABLE b(id TEXT, y TEXT)", "INSERT INTO a VALUES (1, 'a')", "INSERT INTO b VALUES ('2', 'b')"],
   ["SELECT y FROM a FULL JOIN b USING (id) WHERE id = '2'", "SELECT y FROM a FULL JOIN b USING (id) WHERE id = 2", "SELECT y FROM a FULL JOIN b USING (id) WHERE id > 1"]),
 "row_between": (["CREATE TABLE t(a NUMERIC, b NUMERIC)", "INSERT INTO t VALUES (10, 10), (10, 'a'), ('a', 10)"],
   ["SELECT a, b FROM t WHERE (a, b) BETWEEN (-2.5, 100) AND (10, '1e3')", "SELECT a, b, (a, b) <= (10, '1e3') FROM t", "SELECT a, b, (a, b) >= (-2.5, 100) FROM t", "SELECT a, b, (a, b) BETWEEN (1, 1) AND (10, 20) FROM t", "SELECT a, b FROM t WHERE (a, b) BETWEEN (1, 1) AND (10, 20)"]),
 "row_between2": (["CREATE TABLE ev(day INT, hour INT)", "INSERT INTO ev VALUES (1, 5), (1, 20), (2, 3), (3, 1)"],
   ["SELECT day, hour FROM ev WHERE (day, hour) BETWEEN (1, 10) AND (2, 12)", "SELECT e.day, f.hour FROM ev e JOIN ev f ON ((e.day, f.hour) BETWEEN (1, 10) AND (2, 4))", "SELECT day, hour FROM ev WHERE (day, hour) >= (1, 10) AND (day, hour) <= (2, 12)", "SELECT day, hour FROM ev WHERE (day, hour) NOT BETWEEN (1, 10) AND (2, 12)"]),
 "or_ignore_strict_pk": (["CREATE TABLE t(k TEXT PRIMARY KEY, v INT) STRICT", "INSERT OR IGNORE INTO t VALUES ('a', 1), (NULL, 2), ('b', 3)"], ["SELECT * FROM t"]),
 "or_ignore_wr_pk": (["CREATE TABLE t(k TEXT PRIMARY KEY, v INT) WITHOUT ROWID", "INSERT OR IGNORE INTO t VALUES ('a', 1), (NULL, 2), ('b', 3)"], ["SELECT * FROM t"]),
 "or_ignore_wr_composite": (["CREATE TABLE t(k TEXT, n INT, v INT, PRIMARY KEY(k, n)) WITHOUT ROWID", "INSERT OR IGNORE INTO t VALUES ('a', 1, 1), (NULL, 2, 2), ('b', NULL, 3)"], ["SELECT * FROM t"]),
 "or_replace_wr_pk": (["CREATE TABLE t(k TEXT PRIMARY KEY, v INT) WITHOUT ROWID", "INSERT OR REPLACE INTO t VALUES ('a', 1)", "INSERT OR IGNORE INTO t VALUES (NULL, 2)"], ["SELECT * FROM t"]),
 "compound_collation2": (["CREATE TABLE t(a TEXT COLLATE NOCASE)", "CREATE TABLE u(a TEXT)", "INSERT INTO t VALUES ('x')", "INSERT INTO u VALUES ('abc'), ('ABC')"],
   ["SELECT a FROM t UNION SELECT a FROM u", "SELECT a FROM t GROUP BY a UNION SELECT a FROM u UNION SELECT 'z'", "SELECT a FROM u EXCEPT SELECT a FROM t", "SELECT a FROM u UNION SELECT a FROM t", "SELECT a FROM u EXCEPT SELECT 'q' COLLATE NOCASE", "SELECT a FROM t UNION ALL SELECT a FROM u UNION SELECT 'ABC'"]),
 "correlated_full_join_panic": (["CREATE TABLE o(k TEXT, n INT)", "CREATE TABLE w(k TEXT)", "INSERT INTO w VALUES ('a')"],
   ["SELECT d.c0 FROM (SELECT (CASE WHEN (b.n NOT IN (SELECT s.k FROM w AS s WHERE (b.k IS '1e3'))) THEN b.k ELSE b.n END) AS c0 FROM o AS b FULL JOIN w AS c ON NULL GROUP BY (CASE WHEN (b.n NOT IN (SELECT s.k FROM w AS s WHERE (b.k IS '1e3'))) THEN b.k ELSE b.n END)) AS d",
    "SELECT b.n NOT IN (SELECT s.k FROM w AS s WHERE (b.k IS '1e3')) FROM o AS b FULL JOIN w AS c ON NULL", "SELECT count(*) FROM o AS b FULL JOIN w AS c ON NULL GROUP BY b.n NOT IN (SELECT s.k FROM w AS s WHERE (b.k IS '1e3'))"]),
}
for n, (st, qs) in P.items():
    with open(f"{out}/{n}.md", "w") as f:
        f.write(f"# diff Wrong: {n}\n\nplain\n\n## Statements\n\n```sql\n" + "".join(s + ";\n" for s in st) + "```\n\n## Queries\n\n```sql\n" + "".join(q + ";\n" for q in qs) + "```\n\n## What differed\n\n```\n```\n")
