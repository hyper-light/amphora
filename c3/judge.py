"""Lane C3: the probe's statements run in SQLite over the probe's own rows; every answer compared.
usage: judge.py <dir with data.sql, queries.tsv, answers.tsv>"""
import sqlite3, sys, collections
d = sys.argv[1]
db = sqlite3.connect(":memory:")
db.executescript(open(f"{d}/data.sql").read())
def show(v):
    return "" if v is None else str(v)
sqls = dict(line.rstrip("\n").split("\t", 1) for line in open(f"{d}/queries.tsv"))
want = {n: ";".join(sorted(",".join(show(v) for v in row) for row in db.execute(sql))) for n, sql in sqls.items()}
print("SQLite", sqlite3.sqlite_version)
for n, w in want.items():
    print(f"q{n} SQLite answers {w}")
total = collections.Counter(); bad = []
shape = {}
for line in open(f"{d}/answers.tsv"):
    analyzed, n, setting, bushy, in_runs, got = line.rstrip("\n").split("\t")
    total[(analyzed, setting, got == want[n])] += 1
    shape[(analyzed, n, setting)] = (bushy, in_runs)
    if got != want[n]:
        bad.append((analyzed, n, setting, bushy, in_runs, got, want[n]))
for (analyzed, setting, ok), count in sorted(total.items()):
    print(f"analyzed={analyzed} {setting}: {'equal to' if ok else 'DIFFERENT FROM'} SQLite's answer: {count}")
for analyzed in ("true", "false"):
    for setting in ("by_cost_first", "by_cost_second"):
        print(f"analyzed={analyzed} {setting} (bushy,in_runs) per query:", [shape[(analyzed, n, setting)] for n in sqls])
print("WRONG ANSWERS:", len(bad))
for b in bad[:80]:
    print("  ", b)
