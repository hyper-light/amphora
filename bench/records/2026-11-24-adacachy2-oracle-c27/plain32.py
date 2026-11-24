import os, sys
out = sys.argv[1]; os.makedirs(out, exist_ok=True)
Q = ["CREATE TABLE q(k TEXT, v INT)", "CREATE TABLE s(k TEXT, v INT)", "INSERT INTO q VALUES ('a', 1), ('a', 2), ('b', 3)", "INSERT INTO s VALUES ('a', 5), ('c', 6)"]
P = {"DW_value": (Q, ["SELECT q.k, (SELECT group_concat((SELECT quote(q.k) || s.k)) FROM s) FROM q GROUP BY q.k", "SELECT q.k, (SELECT max((SELECT quote(q.k) || s.k || quote(count(*)) FROM s AS s2 WHERE s2.k = s.k)) FROM s) FROM q GROUP BY q.k", "SELECT q.k, (SELECT count(*) FROM s WHERE s.k = q.k UNION ALL SELECT 9 LIMIT 1) FROM q GROUP BY q.k", "SELECT q.k FROM q GROUP BY q.k HAVING EXISTS (SELECT 1 FROM s WHERE s.k = q.k UNION SELECT 1 FROM s WHERE s.k = q.k)", "SELECT q.k FROM q GROUP BY q.k HAVING EXISTS (SELECT 1 FROM s WHERE EXISTS (SELECT 1 FROM s AS s2 WHERE s2.k = q.k UNION SELECT 1 FROM s AS s3 WHERE s3.k = q.k))"])}
for n, (st, qs) in P.items():
    with open(f"{out}/{n}.md", "w") as f:
        f.write(f"# diff Wrong: {n}\n\nplain\n\n## Statements\n\n```sql\n" + "".join(s + ";\n" for s in st) + "```\n\n## Queries\n\n```sql\n" + "".join(q + ";\n" for q in qs) + "```\n\n## What differed\n\n```\n```\n")
