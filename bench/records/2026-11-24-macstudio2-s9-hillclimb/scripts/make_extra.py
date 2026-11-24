#!/usr/bin/env python3
"""Lane S9: the README's section on the switch ablation, the profiles and the many-connections runs, from the files after.sh made.
usage: make_extra.py DATA_DIR"""
import glob, json, os, re, statistics, sys
d = sys.argv[1]
print("## B with one default put back at a time (`ablate.sh`; one pass each, 200,000 items, after the rounds)\n")
rows = {}
p = os.path.join(d, "ablate.jsonl")
if os.path.exists(p):
    for l in open(p):
        r = json.loads(l)
        for op, c, o, p50, e in r["results"]:
            rows.setdefault((op, c), {}).setdefault(r["label"], []).append(o)
    labels = list(dict.fromkeys(k for v in rows.values() for k in v))
    print("| statement | clients | " + " | ".join(labels) + " |")
    print("|---|---|" + "---|" * len(labels))
    for (op, c), v in sorted(rows.items()):
        print(f"| {op} | {c} | " + " | ".join(f"{statistics.median(v[x]):,.0f}" if x in v else "-" for x in labels) + " |")
    print("\nops/s. `default` is B as it ships; `shared=0` is AMPHORA_SHARED_PAGES=0, `page_path=today`, `checkout=pool`, `reader_pages=2000` (shared pages on, 2,000 private pages a reader), `writer_pages=1024`; `all_old` is Bd.\n")
else:
    print("not run.\n")
print("## Profiles (samply, no root, 2,000 Hz, on the 4 s window of a run that reused a database; shares are on-CPU samples, and a share is not a cause)\n")
for f in sorted(glob.glob(os.path.join(d, "prof", "*.txt")) + glob.glob(os.path.join(d, "prof-*.txt"))):
    t = open(f).read()
    print(f"### {os.path.basename(f)}\n\n```")
    lines = t.splitlines()
    out, n = [], 0
    for l in lines:
        if l.startswith("Top"):
            n = 0
        if l.startswith("  "):
            n += 1
            if n > 11:
                continue
        out.append(l[:190])
    print("\n".join(out))
    print("```\n")
print("## Many connections (`many.py`: the three statements below, each on its own, at each client count; a fresh process and load per row)\n")
mf = os.path.join(d, "many.jsonl")
if os.path.exists(mf):
    print("| arm | clients | peak RSS MiB | harness heap peak MiB | statement: ops/s (p50 / p99 ms; errors) |")
    print("|---|---|---|---|---|")
    for l in open(mf):
        r = json.loads(l)
        cell = "; ".join(f"{x[0]} {x[1]:,} ({x[2]} / {x[3]}; {x[4]})" for x in r.get("results", [])) or r.get("error", "failed")[:200]
        print(f"| {r['arm']} | {r['clients']} | {r['max_rss_mb']:,} | {r.get('heap_peak_mb')} | {cell} |")
else:
    print("not run.")
print()
