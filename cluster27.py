#!/usr/bin/env python3
"""Lane C27: replayed findings clustered by reduced shape; the smallest of each printed. usage: cluster27.py REPLAY.json [TOP]"""
import collections, json, re, sys
rs = json.load(open(sys.argv[1])); top = int(sys.argv[2]) if len(sys.argv) > 2 else 1
self_ = json.load(open(sys.argv[3])) if len(sys.argv) > 3 else {}
only = sys.argv[4] if len(sys.argv) > 4 else ""
def norm(s): return re.sub(r"\d+", "#", s)
def shape(q):
    q = re.sub(r"'(?:[^']|'')*'", "'s'", q); q = re.sub(r"x'[0-9a-f]*'", "x''", q); q = re.sub(r"\b\d+(?:\.\d+)?(?:e[+-]?\d+)?\b", "n", q)
    return re.sub(r"\s+", " ", q)
def first(d): return (d.splitlines() or [""])[0]
by = collections.defaultdict(list)
for r in rs:
    sysok = r["rc"] == 0 and r["differs"] and r.get("system_differs") == []
    if not r["differs"] and r["rc"] == 0: k = "NOT-REPRODUCED " + r["kind"]
    elif sysok: k = "SYSTEM-AGREES-WITH-AMPHORA"
    elif r["kind"] == "Panic" or r["rc"] not in (0, 124): k = "PANIC " + norm(r["panic"])[:170]
    elif r["rc"] == 124: k = "TIMEOUT"
    elif r["kind"] == "Error": k = "ERROR " + norm(first(r["diff"]))[:150]
    elif any(w.get("order") is False for w in self_.get(r["id"], [])): k = "SQLITE-SELF: its rows change without its ORDER BY"
    elif any(not w["opt"] for w in self_.get(r["id"], [])): k = "SQLITE-SELF: its rows change with its optimizations off"
    else:
        f = []
        q = r["queries"] + " " + r["stmts"]
        for n, rx in (("trigger", r"CREATE TRIGGER"), ("view", r"CREATE VIEW"), ("recursive", r"WITH RECURSIVE"), ("window", r"OVER ?\("), ("upsert", r"ON CONFLICT"), ("fulljoin", r"FULL JOIN|RIGHT JOIN"), ("compound", r"UNION|INTERSECT|EXCEPT"), ("rowvalue", r"\([^()]*,[^()]*\) (=|<|>|IS|IN|NOT IN|BETWEEN)"), ("jsonb", r"jsonb"), ("strict", r"STRICT"), ("withoutrowid", r"WITHOUT ROWID")):
            if re.search(rx, q): f.append(n)
        k = "WRONG " + r["oracle"] + " " + "+".join(f)
    by[k].append(r)
for k, v in sorted(by.items(), key=lambda x: (-len(x[1]), x[0])):
    v.sort(key=lambda r: len(r["stmts"]) + len(r["queries"]))
    if only and only not in k: continue
    print(f"\n##### {k}: {len(v)}")
    for r in v[:top]:
        print("  --", r["id"], r["kind"], "differs at", r["differs"], "rc", r["rc"], "system", r.get("system_differs"))
        print("    " + "\n    ".join(x[:420] for x in r["stmts"].splitlines()))
        print("    Q: " + r["queries"][:900].replace("\n", "\n       "))
        print("    " + "\n    ".join(r["diff"].splitlines()[:8])[:500])
