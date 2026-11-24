import sqlite3, sys
text = open(sys.argv[1]).read()
for case in text.split("----\n"):
    print("=====")
    c = sqlite3.connect(":memory:", isolation_level=None)
    for sql in [s.strip() for s in case.split(";\n") if s.strip()]:
        if sql.startswith("EXPLAIN"): continue
        try:
            rows = c.execute(sql).fetchall()
            if sql.startswith(("SELECT",)) or rows: print("  ", sql, "\n     ", " / ".join(", ".join("NULL" if v is None else repr(v) for v in r) for r in rows))
            elif not sql.startswith("CREATE"): print("   ok", sql)
        except Exception as e: print("  ", sql, "\n      ERROR", e)
