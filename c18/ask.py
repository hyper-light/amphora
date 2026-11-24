import sqlite3, sys
conn = sqlite3.connect(":memory:")
for sql in [s.strip() for s in open(sys.argv[1]).read().split(";\n")]:
    if not sql: continue
    try:
        rows = conn.execute(sql).fetchall()
    except sqlite3.Error as e:
        print(f"--- {sql}\nERROR {e}"); continue
    if sql.split()[0] in ("CREATE", "INSERT", "ANALYZE"): continue
    lines = sorted(",".join("NULL" if v is None else str(v) for v in row) for row in rows)
    print(f"--- {sql}\n{len(lines)}:{';'.join(lines)}")
