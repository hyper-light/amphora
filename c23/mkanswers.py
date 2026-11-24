"""SQLite rows of a probe file: python3 mkanswers.py p.sql > p.sqlite"""
import sqlite3, sys
text = open(sys.argv[1]).read()
conn = sqlite3.connect(":memory:", isolation_level=None)
asked = False
for sql in [s.strip() for s in text.split(";\n") if s.strip()]:
    setup = not asked and sql.startswith(("CREATE", "INSERT", "ANALYZE", "DROP", "DELETE"))
    asked |= not setup
    write = not setup and sql.startswith(("INSERT", "UPDATE", "DELETE"))
    if not setup:
        print("--- " + sql)
    if write:
        conn.execute("BEGIN")
    try:
        rows = conn.execute(sql).fetchall()
        rows = sorted(",".join("NULL" if v is None else str(v) for v in r) for r in rows)
        if not setup:
            print(f"{len(rows)}:" + ";".join(rows))
    except sqlite3.Error as e:
        print("ERROR " + str(e))
    if write:
        conn.execute("ROLLBACK")
