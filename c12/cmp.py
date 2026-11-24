"""Compares c12_sql's output with SQLite's answers. usage: cmp.py FILE.sql OUTPUT"""
import sqlite3, sys
text = open(sys.argv[1]).read()
conn = sqlite3.connect(":memory:")
want = {}
for sql in [s.strip() for s in text.split(";\n") if s.strip()]:
    if any(sql.startswith(w) for w in ("CREATE", "INSERT", "DROP", "DELETE")):
        conn.execute(sql)
    elif not sql.startswith("EXPLAIN"):
        try:
            rows = conn.execute(sql).fetchall()
            lines = sorted(",".join("NULL" if v is None else str(v) for v in r) for r in rows)
            want[sql] = f"{len(lines)}:" + ";".join(lines)
        except sqlite3.Error as error:
            want[sql] = f"ERROR {error}"
got = None
plan = {}
ok = {}
bad = []
cur = None
for line in open(sys.argv[2]):
    line = line.rstrip("\n")
    if line.startswith("--- ["):
        name, sql = line[5:].split("] ", 1)
        cur = (name, sql)
    elif cur and cur[1] in want:
        if line != want[cur[1]]:
            bad.append((cur[0], cur[1], line, want[cur[1]]))
        else:
            ok[cur[0]] = ok.get(cur[0], 0) + 1
        cur = None
print("SQLite", sqlite3.sqlite_version, "right:", ok, "wrong:", len(bad))
for name, sql, g, w in bad:
    print(f"[{name}] {sql}\n    SQLite  {w}\n    Amphora {g}")
