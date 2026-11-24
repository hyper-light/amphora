#!/usr/bin/env python3
"""Lane C27: plain forms of the run's other findings, as finding files `--replay` reads. usage: plain28.py OUT_DIR"""
import os, sys
out = sys.argv[1]; os.makedirs(out, exist_ok=True)
Q = ["CREATE TABLE q(k TEXT, v INT)", "CREATE TABLE s(k TEXT, v INT)", "INSERT INTO q VALUES ('a', 1), ('a', 2), ('b', 3)", "INSERT INTO s VALUES ('a', 5), ('c', 6)"]
P = {
 "aggregate_explicit_collate": (["CREATE TABLE c(a TEXT, n TEXT COLLATE NOCASE)", "INSERT INTO c VALUES ('ABC', 'ABC'), ('a ', 'a ')"], [
  "SELECT min(quote(a COLLATE NOCASE)) FROM c", "SELECT min(quote(a COLLATE NOCASE)) FROM c GROUP BY NULL", "SELECT min(quote(n COLLATE NOCASE)) FROM c", "SELECT min(quote(n COLLATE NOCASE)) FROM c GROUP BY NULL",
  "WITH w AS (SELECT min(quote(n COLLATE NOCASE)) AS m FROM c GROUP BY NULL) SELECT m FROM w", "WITH w AS (SELECT NULL, min(quote(n COLLATE NOCASE)) AS m FROM c GROUP BY NULL) SELECT m FROM w", "SELECT m FROM (SELECT min(quote(a COLLATE NOCASE)) AS m FROM c GROUP BY 1=1)",
  "SELECT min(a COLLATE NOCASE) FROM c GROUP BY NULL", "SELECT max(a COLLATE NOCASE), min(a || '' COLLATE NOCASE) FROM c GROUP BY NULL", "SELECT min(quote(a COLLATE NOCASE)), max(quote(a COLLATE NOCASE)) FROM c GROUP BY length(a) > 9",
  "SELECT count(DISTINCT quote(a COLLATE NOCASE)) FROM c GROUP BY NULL", "SELECT min(quote(a COLLATE NOCASE)) OVER () FROM c", "SELECT group_concat(DISTINCT lower(a COLLATE RTRIM)) FROM c GROUP BY NULL"]),
 "cast_numeric_text": (["CREATE TABLE d(a TEXT)", "INSERT INTO d VALUES ('-92233720368547758081'), ('-92233720368547758081.5')"], [
  "SELECT CAST(a AS NUMERIC), typeof(CAST(a AS NUMERIC)) FROM d", "SELECT count(*) FROM (SELECT DISTINCT CAST(a AS NUMERIC) FROM d)", "SELECT CAST('-92233720368547758081' AS NUMERIC) = CAST('-92233720368547758081.5' AS NUMERIC)",
  "SELECT CAST(a AS REAL) - -9.2233720368547758e19, CAST(a AS NUMERIC) - -9.2233720368547758e19 FROM d", "SELECT NULL FROM (SELECT CAST(a AS NUMERIC) AS c FROM d) GROUP BY c", "SELECT NULL, c FROM (SELECT CAST(a AS NUMERIC) AS c FROM d) GROUP BY NULL, c",
  "SELECT count(*) FROM d GROUP BY CAST(a AS NUMERIC)", "SELECT '-92233720368547758081' + 0 = '-92233720368547758081.5' + 0"]),
 "having_nested_subquery": (Q, [
  "SELECT q.k, count(*) FROM q GROUP BY q.k HAVING EXISTS (SELECT 1 FROM s WHERE s.k = q.k)", "SELECT q.k, count(*) FROM q GROUP BY q.k HAVING (SELECT count(*) FROM s WHERE s.k = q.k) = 1",
  "SELECT q.k, count(*) FROM q GROUP BY q.k HAVING EXISTS (SELECT 1 FROM s WHERE (SELECT count(*) FROM s AS s2 WHERE s2.k = q.k) = 1)", "SELECT q.k, count(*) FROM q GROUP BY q.k HAVING 5 IN (SELECT s.v FROM s WHERE (SELECT max(s2.v) FROM s AS s2 WHERE s2.k = q.k))",
  "SELECT q.k, count(*) FROM q GROUP BY q.k HAVING (SELECT count(*) FROM s WHERE s.v > (SELECT min(s2.v) FROM s AS s2 WHERE q.k <> s.k)) > 0", "SELECT q.k FROM q GROUP BY q.k HAVING NULL NOT IN (SELECT s.v FROM s WHERE (SELECT min(s2.v) FROM s AS s2 WHERE q.k IS NOT s.k))",
  "SELECT q.k, (SELECT count(*) FROM s WHERE (SELECT count(*) FROM s AS s2 WHERE s2.k = q.k) = 1) FROM q GROUP BY q.k", "SELECT q.k, count(*) FROM q WHERE EXISTS (SELECT 1 FROM s WHERE (SELECT count(*) FROM s AS s2 WHERE s2.k = q.k) = 1) GROUP BY q.k",
  "SELECT q.k FROM q GROUP BY q.k ORDER BY (SELECT count(*) FROM s WHERE (SELECT count(*) FROM s AS s2 WHERE s2.k = q.k) = 1), q.k"]),
 "json5_quotes": (["CREATE TABLE j(a TEXT)", "INSERT INTO j VALUES ('''{\"a\":1}'''), ('''x\"y''')"], [
  "SELECT json(a) FROM j", "SELECT hex(jsonb(a)) FROM j", "SELECT json(jsonb(a)) FROM j", "SELECT json_valid(json(a)) FROM j", "SELECT json_extract(a, '$') FROM j", "SELECT json_array(json(a)) FROM j", "SELECT hex(jsonb_set(a)) FROM j", "SELECT json_set('{}', '$.k', json(a)) FROM j",
  "SELECT json_extract(json_set('{}', '$.k', json(a)), '$.k') FROM j", "SELECT json('{''a\"b'':1}')", "SELECT json_extract('{''a\"b'':1}', '$.\"a\\\"b\"')", "SELECT json_type(a) FROM j", "SELECT json_quote(json(a)) FROM j", "SELECT json_group_array(json(a)) FROM j", "SELECT value FROM j, json_each(json_array(json(j.a)))"]),
 "json_patch_hex": ([], [
  "SELECT json_patch(100, '0x10')", "SELECT hex(jsonb(json_patch(100, '0x10')))", "SELECT hex(jsonb_replace(json_patch(100, '0x10')))", "SELECT hex(jsonb('16'))", "SELECT hex(jsonb('0x10'))", "SELECT json_patch(100, '0x10') || ''", "SELECT hex(jsonb(json_patch(100, '0x10') || ''))", "SELECT hex(jsonb_patch(100, '0x10'))"]),
 "in_subquery_outer_call": (["CREATE TABLE t0(c0 TEXT)", "CREATE TABLE t1(a TEXT, b TEXT)", "INSERT INTO t1 VALUES ('{', 'x')"], [
  "SELECT a FROM t1 WHERE NOT (NULL IN (SELECT s.c0 FROM t0 AS s WHERE json_set(t1.a, t1.a, 'x') BETWEEN 2.0 AND s.c0))", "SELECT a FROM t1 WHERE NULL IN (SELECT s.c0 FROM t0 AS s WHERE s.c0 > json_extract(t1.a, '$.x')) IS NOT 1",
  "SELECT a FROM t1 WHERE a NOT IN (SELECT s.c0 FROM t0 AS s WHERE json_extract(t1.a, '$.x') BETWEEN 2.0 AND s.c0)", "SELECT a, (SELECT count(*) FROM t0 AS s WHERE json_extract(t1.a, '$.x') BETWEEN 1 AND s.c0) FROM t1",
  "SELECT a FROM t1 WHERE NOT EXISTS (SELECT 1 FROM t0 AS s WHERE json_extract(t1.a, '$.x') BETWEEN 1 AND s.c0)"]),
 "view_unread_columns": (["CREATE TABLE d(id INTEGER PRIMARY KEY, body TEXT)", "INSERT INTO d VALUES (1, '{\"a\":1}'), (2, 'no json')", "CREATE VIEW dv AS SELECT DISTINCT id, json_extract(body, '$.a') AS a FROM d", "CREATE VIEW uv AS SELECT id, json_extract(body, '$.a') AS a FROM d UNION SELECT 9, 'z'", "CREATE VIEW iv AS SELECT id, json_extract(body, '$.a') AS a FROM d WHERE id < 0 INTERSECT SELECT id, body FROM d"], [
  "SELECT count(*) FROM dv", "SELECT id FROM dv", "SELECT count(*) FROM uv", "SELECT id FROM iv", "SELECT count(*) FROM iv WHERE id = 1", "SELECT d.id FROM d WHERE NULL IN (SELECT a FROM dv WHERE dv.id = d.id + 100)", "SELECT d.id FROM d WHERE (SELECT min(a) FROM dv WHERE dv.id > 100) ISNULL"]),
 "row_value_refusals": (["CREATE TABLE t(a, b, c)", "INSERT INTO t VALUES (1, 2, 3), (4, 5, 6)"], [
  "SELECT c FROM t WHERE (a, b) > (lower('X'), json_extract('[\"a\"]', '$[0]'))", "SELECT ('a', 1) BETWEEN ('a', 0) AND ('a', 2)", "SELECT c FROM t WHERE (a, CASE WHEN b THEN 0 ELSE 1 END) BETWEEN (a, x'') AND (-1, c)", "SELECT c FROM t WHERE (a, b) BETWEEN (0, 0) AND (2, 2)",
  "SELECT c FROM t WHERE (a, b, c) BETWEEN (1, 2, 3) AND (abs(-4), 5, json_extract('6', '$'))", "SELECT c, count(*) FROM t WHERE (NULL, NULL) < (SELECT max(a), count(b) FROM t) GROUP BY c HAVING 1", "SELECT c FROM t WHERE (NULL, NULL, '1') NOT IN (SELECT a, b, c FROM t) GROUP BY c HAVING NOT 1",
  "SELECT (a, b) = (1, 2) FROM t", "SELECT c FROM t WHERE (1, 2) < (SELECT max(a), count(b) FROM t)", "SELECT 1.5, NULL FROM t WHERE (NULL, NULL) < (SELECT max(a), count(b) FROM t) GROUP BY 1.5"]),
 "compound_in_from_where": (["CREATE TABLE t1(c0 BLOB, c1 BOOLEAN)", "CREATE TABLE t2(c0, c1 INTEGER)", "INSERT INTO t2 VALUES (255, 0.5), (1000000000000000.0, 1.5)"], [
  "SELECT count(*) FROM (SELECT quote(c0) AS c FROM t1 UNION ALL SELECT quote(b.c1) FROM t2 AS a LEFT JOIN t2 AS b ON a.c0 = b.c0)"]),
}
for n, (st, qs) in P.items():
    with open(f"{out}/{n}.md", "w") as f:
        f.write(f"# diff Wrong: {n}\n\nplain\n\n## Statements\n\n```sql\n" + "".join(s + ";\n" for s in st) + "```\n\n## Queries\n\n```sql\n" + "".join(q + ";\n" for q in qs) + "```\n\n## What differed\n\n```\n```\n")
print(len(P))
