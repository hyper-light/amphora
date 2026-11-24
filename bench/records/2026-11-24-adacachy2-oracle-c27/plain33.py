import os, sys
out = sys.argv[1]; os.makedirs(out, exist_ok=True)
P = {"IN_affinity_compound": (["CREATE TABLE i(n INTEGER)", "CREATE TABLE tx(s TEXT)", "INSERT INTO i VALUES (1), (2)", "INSERT INTO tx VALUES ('1'), ('3')"], ["SELECT n, n IN (SELECT s FROM tx) FROM i", "SELECT n, n IN (SELECT s FROM tx EXCEPT SELECT 'z') FROM i", "SELECT n, n IN (SELECT s FROM tx UNION SELECT 'z') FROM i", "SELECT n, n IN (SELECT s FROM tx INTERSECT SELECT s FROM tx) FROM i", "SELECT n, n IN (SELECT s FROM tx UNION ALL SELECT 'z') FROM i", "SELECT n, n IN (SELECT s FROM tx UNION SELECT s FROM tx EXCEPT SELECT '3') FROM i", "SELECT n, n NOT IN (SELECT s FROM tx EXCEPT SELECT 'z') FROM i"]),
 "compound_correlated": (["CREATE TABLE q(k TEXT)", "CREATE TABLE s(k TEXT)", "INSERT INTO q VALUES ('a'), ('b')", "INSERT INTO s VALUES ('a')"], ["SELECT k FROM q WHERE EXISTS (SELECT 1 FROM s WHERE s.k = q.k UNION SELECT 1 FROM s WHERE s.k = q.k)", "SELECT k, (SELECT k FROM s WHERE s.k = q.k UNION ALL SELECT 'z' LIMIT 1) FROM q", "SELECT k FROM q WHERE k IN (SELECT s.k FROM s WHERE s.k = q.k UNION SELECT 'z')"])}
for n, (st, qs) in P.items():
    with open(f"{out}/{n}.md", "w") as f:
        f.write(f"# diff Wrong: {n}\n\nplain\n\n## Statements\n\n```sql\n" + "".join(s + ";\n" for s in st) + "```\n\n## Queries\n\n```sql\n" + "".join(q + ";\n" for q in qs) + "```\n\n## What differed\n\n```\n```\n")
