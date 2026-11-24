import os, sys
out = sys.argv[1]; os.makedirs(out, exist_ok=True)
P = {"EA_strict_type_beside_a_taken_rowid": (["CREATE TABLE t0(id INTEGER PRIMARY KEY, b BLOB) STRICT", "INSERT INTO t0 VALUES (1, x'00')", "INSERT OR IGNORE INTO t0 VALUES (1, 'x')", "INSERT INTO t0 VALUES (1, 'x') ON CONFLICT DO NOTHING"], ["SELECT * FROM t0"]),
 "EA_unique": (["CREATE TABLE t0(id INT UNIQUE, b BLOB) STRICT", "INSERT INTO t0 VALUES (1, x'00')", "INSERT OR IGNORE INTO t0 VALUES (1, 'x')"], ["SELECT * FROM t0"])}
for n, (st, qs) in P.items():
    with open(f"{out}/{n}.md", "w") as f:
        f.write(f"# diff Wrong: {n}\n\nplain\n\n## Statements\n\n```sql\n" + "".join(s + ";\n" for s in st) + "```\n\n## Queries\n\n```sql\n" + "".join(q + ";\n" for q in qs) + "```\n\n## What differed\n\n```\n```\n")
