import os, sys
out = sys.argv[1]; os.makedirs(out, exist_ok=True)
P = {"ED_upsert_where_collation": (["CREATE TABLE t0(c0 TEXT COLLATE NOCASE PRIMARY KEY, c1 TEXT)", "INSERT INTO t0 VALUES ('abc', 'old')", "INSERT INTO t0 VALUES ('ABC', 'new') ON CONFLICT DO UPDATE SET c1 = excluded.c1 WHERE t0.c0 BETWEEN 'A' AND excluded.c0"], ["SELECT * FROM t0"]),
 "ED_upsert_where_eq": (["CREATE TABLE t0(c0 TEXT COLLATE NOCASE PRIMARY KEY, c1 TEXT)", "INSERT INTO t0 VALUES ('abc', 'old')", "INSERT INTO t0 VALUES ('ABC', 'new') ON CONFLICT DO UPDATE SET c1 = excluded.c1 WHERE t0.c0 = excluded.c0"], ["SELECT * FROM t0"]),
 "ED_upsert_where_le": (["CREATE TABLE t0(c0 TEXT COLLATE NOCASE PRIMARY KEY, c1 TEXT)", "INSERT INTO t0 VALUES ('abc', 'old')", "INSERT INTO t0 VALUES ('ABC', 'new') ON CONFLICT DO UPDATE SET c1 = excluded.c1 WHERE t0.c0 <= excluded.c0"], ["SELECT * FROM t0"]),
 "ED_upsert_set_case": (["CREATE TABLE t0(c0 TEXT COLLATE NOCASE PRIMARY KEY, c1 TEXT)", "INSERT INTO t0 VALUES ('abc', 'old')", "INSERT INTO t0 VALUES ('ABC', 'new') ON CONFLICT DO UPDATE SET c1 = CASE WHEN t0.c0 BETWEEN 'A' AND excluded.c0 THEN 'yes' ELSE 'no' END, c0 = excluded.c0"], ["SELECT * FROM t0"]),
 "ED_upsert_excluded_eq": (["CREATE TABLE t0(c0 TEXT COLLATE NOCASE PRIMARY KEY, c1 TEXT)", "INSERT INTO t0 VALUES ('abc', 'old')", "INSERT INTO t0 VALUES ('ABC', 'new') ON CONFLICT DO UPDATE SET c1 = excluded.c1 WHERE excluded.c0 = 'abc'"], ["SELECT * FROM t0"]),
 "ED_upsert_excluded_between": (["CREATE TABLE t0(c0 TEXT COLLATE NOCASE PRIMARY KEY, c1 TEXT)", "INSERT INTO t0 VALUES ('abc', 'old')", "INSERT INTO t0 VALUES ('ABC', 'new') ON CONFLICT DO UPDATE SET c1 = excluded.c1 WHERE excluded.c0 BETWEEN 'abc' AND 'abd'"], ["SELECT * FROM t0"]),
 "ED_update_where_between": (["CREATE TABLE t0(c0 TEXT COLLATE NOCASE PRIMARY KEY, c1 TEXT)", "INSERT INTO t0 VALUES ('abc', 'ABC')", "UPDATE t0 SET c1 = 'new' WHERE c0 BETWEEN 'A' AND c1"], ["SELECT * FROM t0"])}
for n, (st, qs) in P.items():
    with open(f"{out}/{n}.md", "w") as f:
        f.write(f"# diff Wrong: {n}\n\nplain\n\n## Statements\n\n```sql\n" + "".join(s + ";\n" for s in st) + "```\n\n## Queries\n\n```sql\n" + "".join(q + ";\n" for q in qs) + "```\n\n## What differed\n\n```\n```\n")
