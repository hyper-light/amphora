import os, sys
out = sys.argv[1]; os.makedirs(out, exist_ok=True)
Q = ["CREATE TABLE q(k TEXT, v INT)", "CREATE TABLE s(k TEXT, v INT)", "INSERT INTO q VALUES ('a', 1), ('a', 2), ('b', 3)", "INSERT INTO s VALUES ('a', 5), ('c', 6)"]
P = {"DL": (Q, ["SELECT q.k, count(*) FROM q GROUP BY q.k HAVING EXISTS (SELECT 1 FROM (SELECT a3.k AS k FROM q AS a3 WHERE a3.k = q.k) AS a4)"]),
 "DW2": (Q, ["SELECT q.k FROM q GROUP BY q.k HAVING EXISTS (SELECT 1 FROM s WHERE EXISTS (SELECT 1 FROM s AS s2 WHERE s2.k = q.k))", "SELECT q.k FROM q GROUP BY q.k HAVING EXISTS (SELECT 1 FROM s WHERE EXISTS (SELECT 1 FROM s AS s2 WHERE s2.k = s.k AND s2.k = q.k))",
  "SELECT q.k FROM q GROUP BY q.k HAVING EXISTS (SELECT 1 FROM s WHERE s.k = q.k AND EXISTS (SELECT 1 FROM s AS s2 WHERE s2.k = s.k))", "SELECT q.k FROM q GROUP BY q.k HAVING count(*) > 0 AND EXISTS (SELECT 1 FROM s WHERE (SELECT s.k = q.k))",
  "SELECT q.k, (SELECT count(*) FROM s WHERE EXISTS (SELECT 1 FROM s AS s2 WHERE s2.k = s.k AND s2.k = q.k)) FROM q GROUP BY q.k", "SELECT q.k FROM q GROUP BY q.k ORDER BY (SELECT count(*) FROM s WHERE EXISTS (SELECT 1 FROM s AS s2 WHERE s2.k = s.k AND s2.k = q.k)) DESC, q.k",
  "SELECT q.k FROM q WHERE EXISTS (SELECT 1 FROM s WHERE EXISTS (SELECT 1 FROM s AS s2 WHERE s2.k = s.k AND s2.k = q.k))", "SELECT DISTINCT q.k FROM q WHERE (SELECT count(*) FROM s WHERE (SELECT q.k || s.k) = 'aa') = 1"])}
for n, (st, qs) in P.items():
    with open(f"{out}/{n}.md", "w") as f:
        f.write(f"# diff Wrong: {n}\n\nplain\n\n## Statements\n\n```sql\n" + "".join(s + ";\n" for s in st) + "```\n\n## Queries\n\n```sql\n" + "".join(q + ";\n" for q in qs) + "```\n\n## What differed\n\n```\n```\n")
