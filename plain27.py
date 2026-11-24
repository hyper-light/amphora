#!/usr/bin/env python3
"""Lane C27: plain forms, as finding files `--replay` reads. usage: plain27.py OUT_DIR"""
import os, sys
out = sys.argv[1]; os.makedirs(out, exist_ok=True)
UT = ["CREATE TABLE u(a TEXT)", "CREATE TABLE t(a TEXT COLLATE NOCASE)", "CREATE TABLE r(a TEXT COLLATE RTRIM)", "CREATE TABLE e(a TEXT)",
      "INSERT INTO u VALUES ('ABC'), ('abc'), ('x')", "INSERT INTO t VALUES ('Q'), ('q'), ('ABC')", "INSERT INTO r VALUES ('m'), ('m '), ('n')"]
P = {
 "compound_two_arms": (UT, [
  "SELECT a FROM u UNION SELECT a FROM t", "SELECT a FROM t UNION SELECT a FROM u", "SELECT a FROM u INTERSECT SELECT a FROM t", "SELECT a FROM t INTERSECT SELECT a FROM u",
  "SELECT a FROM u EXCEPT SELECT a FROM t", "SELECT a FROM t EXCEPT SELECT a FROM u", "SELECT a FROM t EXCEPT SELECT NULL", "SELECT a FROM t UNION SELECT NULL", "SELECT a FROM t UNION SELECT 'zz'",
  "SELECT 'abc' UNION SELECT a FROM t", "SELECT NULL UNION SELECT a FROM t", "SELECT NULL FROM e UNION SELECT a FROM t"]),
 "compound_three_arms": (UT, [
  "SELECT a FROM t UNION SELECT 'zz' UNION SELECT 'y'", "SELECT a FROM t UNION ALL SELECT 'zz' UNION SELECT 'y'", "SELECT a FROM t EXCEPT SELECT NULL UNION SELECT NULL",
  "SELECT a FROM t UNION SELECT NULL EXCEPT SELECT 'y'", "SELECT a FROM t UNION ALL SELECT NULL INTERSECT SELECT a FROM t", "SELECT a FROM t UNION ALL SELECT 'zz' EXCEPT SELECT NULL",
  "SELECT a FROM r UNION ALL SELECT 1 UNION SELECT 'y'", "SELECT 'zz' UNION ALL SELECT a FROM t UNION SELECT 'y'", "SELECT 'zz' UNION SELECT 'y' UNION SELECT a FROM t",
  "SELECT a FROM u UNION SELECT a FROM t UNION SELECT a FROM u", "SELECT a FROM t UNION SELECT a FROM u UNION SELECT a FROM u", "SELECT a FROM t UNION SELECT a FROM t UNION SELECT a FROM u"]),
 "compound_explicit": (UT, [
  "SELECT 'abc' COLLATE NOCASE UNION SELECT a FROM u", "SELECT a FROM u UNION SELECT 'abc' COLLATE NOCASE", "SELECT a COLLATE BINARY FROM t UNION SELECT a FROM t", "SELECT a FROM t UNION SELECT a COLLATE BINARY FROM t",
  "SELECT a FROM u EXCEPT SELECT 'abc' COLLATE NOCASE", "SELECT a FROM t UNION ALL SELECT 'zz' COLLATE BINARY UNION SELECT 'y'", "SELECT NULL COLLATE NOCASE UNION ALL SELECT 'A' UNION SELECT 'a'",
  "SELECT lower('A') COLLATE NOCASE UNION ALL SELECT 'A' UNION SELECT 'a'", "SELECT a COLLATE NOCASE FROM u UNION ALL SELECT NULL UNION SELECT 7"]),
 "function_collate": (UT, [
  "SELECT min(quote(a COLLATE NOCASE)), max(quote(a COLLATE NOCASE)) FROM u", "SELECT DISTINCT quote(a COLLATE NOCASE) FROM u", "SELECT quote(a COLLATE NOCASE) FROM u GROUP BY quote(a COLLATE NOCASE)",
  "SELECT count(*) FROM u WHERE quote(a COLLATE NOCASE) = '''ABC'''", "SELECT count(*) FROM u WHERE (a COLLATE NOCASE) || '' = 'ABC'", "SELECT count(DISTINCT quote(a COLLATE NOCASE)) FROM u",
  "SELECT quote(a COLLATE NOCASE) FROM u UNION SELECT NULL", "SELECT upper(a COLLATE NOCASE) IN ('abc') FROM u", "SELECT a FROM u ORDER BY quote(a COLLATE NOCASE), a",
  "SELECT max(a || ('' COLLATE NOCASE)) FROM u", "SELECT min(a COLLATE NOCASE), max(a COLLATE NOCASE) FROM u"]),
 "window_group_by": (["CREATE TABLE t0(c0 TEXT COLLATE NOCASE, c1 REAL PRIMARY KEY)", "INSERT INTO t0 VALUES (100, 7), (0.5, x'c3bf')"], [
  "SELECT 1, (7 IS a0.c0), NULL, count(NULL) OVER (ORDER BY NULL DESC) FROM t0 AS a0 GROUP BY 1, (7 IS a0.c0)", "SELECT (7 IS a0.c0), count(*) FROM t0 AS a0 GROUP BY (7 IS a0.c0)",
  "SELECT (7 IS a0.c0) AS x, count(*) OVER () FROM t0 AS a0 GROUP BY x"]),
 "compound_order_collate_groupconst": (["CREATE TABLE t(c0 TEXT)"], [
  "SELECT NULL, NULL FROM t UNION SELECT 'A', count(*) FROM t GROUP BY 'A' ORDER BY 1, 2", "SELECT NULL, NULL FROM t UNION SELECT 'A', count(*) FROM t GROUP BY 'A' ORDER BY 1, 2 COLLATE RTRIM",
  "SELECT * FROM (SELECT NULL, NULL FROM t UNION SELECT 'A', count(*) FROM t GROUP BY 'A') ORDER BY 1, 2 COLLATE RTRIM"]),
 "in_subquery_outer_call": (["CREATE TABLE t0(c0 TEXT)", "CREATE TABLE t1(a TEXT, b TEXT)", "INSERT INTO t1 VALUES ('{', 'x')"], [
  "SELECT a FROM t1 WHERE NOT (NULL IN (SELECT s.c0 FROM t0 AS s WHERE json_set(t1.a, t1.a, 'x') BETWEEN 2.0 AND s.c0))", "SELECT a FROM t1 WHERE NOT EXISTS (SELECT 1 FROM t0 AS s WHERE json_extract(t1.a, '$.x') = s.c0)",
  "SELECT a, (SELECT count(*) FROM t0 AS s WHERE json_extract(t1.a, '$.x') = s.c0) FROM t1"]),
}
for n, (st, qs) in P.items():
    with open(f"{out}/{n}.md", "w") as f:
        f.write(f"# diff Wrong: {n}\n\nplain\n\n## Statements\n\n```sql\n" + "".join(s + ";\n" for s in st) + "```\n\n## Queries\n\n```sql\n" + "".join(q + ";\n" for q in qs) + "```\n\n## What differed\n\n```\n```\n")
print(len(P))
