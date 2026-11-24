#!/usr/bin/env python3
"""Lane C27: the findings where SQLite's rows change without its ORDER BY: Amphora's rows with the ORDER BY against SQLite's without it (and Amphora's without it), at the four settings.
usage: variants27.py BENCH LIB REPLAY.json SELF.json JOBS OUT.json"""
import collections, concurrent.futures as cf, json, os, re, subprocess, sys, tempfile
bench, lib, rj, sj, jobs, out = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], int(sys.argv[5]), sys.argv[6]
rs = {r["id"]: r for r in json.load(open(rj))}; se = json.load(open(sj))
ids = [k for k, v in se.items() if any(w.get("order") is False for w in v)]
def fold(txt):
    if txt.startswith("error"): return txt
    lines = txt.split("\n")[1:]
    f = []
    for l in lines:
        l = l.strip()
        l = re.sub(r"'((?:[^']|'')*)'", lambda m: "'" + m.group(1).rstrip(" ").lower() + "'", l)
        l = re.sub(r"(?<![\w'.])(-?\d+)\.0(?![\d\w])", r"\1", l)
        f.append(l)
    return sorted(f)
def one(k):
    r = rs[k]; sql = [w["order_sql"] for w in se[k] if w.get("order") is False][0]
    q = [x.strip().rstrip(";") for x in r["queries"].split(";\n") if x.strip()]
    with tempfile.NamedTemporaryFile("w", suffix=".md", delete=False, dir=os.path.dirname(out)) as f:
        f.write("# diff Wrong: variant\n\n## Statements\n\n```sql\n" + r["stmts"] + "\n```\n\n## Queries\n\n```sql\n" + q[0] + ";\n" + sql + ";\n```\n\n## What differed\n\n```\n```\n")
    p = subprocess.run([bench, "--oracle", "--sqlite", lib, "--replay", f.name], capture_output=True, text=True, timeout=90, errors="replace")
    os.unlink(f.name)
    s = []
    for b in re.split(r"\n(?=#### )", "\n" + p.stdout):
        a = re.findall(r"== (same|DIFFERS) (.*?)\n-- SQLite:  (.*?)\n-- Amphora: (.*?)(?=\n-- Amphora's plan|\n== |\Z)", b, re.S)
        if len(a) == 2: s.append(a)
    ok = len(s) == 4 and all(fold(a[0][3]) == fold(a[1][2]) and fold(a[1][3]) == fold(a[1][2]) for a in s)
    return k, ok, p.returncode, s[:1], bool(re.search(r"ORDER BY[^)]*\(?\d+ COLLATE", q[0].split("ORDER BY")[-1] and q[0][q[0].rfind("ORDER BY"):]))
with cf.ThreadPoolExecutor(jobs) as ex: R = list(ex.map(one, ids))
bad = [x for x in R if not x[1]]
print("variants", len(R), "Amphora's rows are SQLite's own without the ORDER BY, at all four settings:", len(R) - len(bad), "not:", len(bad), "outermost ORDER BY takes a COLLATE:", sum(1 for x in R if x[4]))
for k, ok, rc, s, o in bad[:12]:
    print("--", k, "rc", rc, "collate", o)
    for a in s:
        for v, q, x, y in a: print("    Q:", " ".join(q.split())[:260]); print("    S:", " / ".join(x.split("\n"))[:200]); print("    A:", " / ".join(y.split("\n"))[:200])
json.dump([[k, ok, rc] for k, ok, rc, s, o in R], open(out, "w"))
