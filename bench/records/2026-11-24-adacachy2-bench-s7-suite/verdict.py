#!/usr/bin/env python3
"""Lane S7: the record's verdict bullets, from TABLES.md as s7_tables.py wrote it (numbers copied, never retyped).

usage: verdict.py TABLES.md
"""
import re
import sys

t = open(sys.argv[1]).read()
sec = lambda a, b: t[t.index(a):t.index(b, t.index(a) + 1)] if b else t[t.index(a):]
s2 = sec("## 2.", "## 3.")
s3 = sec("## 3.", "## 4.") if "## 4." in t else sec("## 3.", None)
counts = re.search(r"Counts over.*", s2).group(0)
print("- **Counts (Amphora's throughput against the best competitor's, 17 statements x 1, 32 and 128 clients).** " + counts)
fam = s2[s2.index("### By family"):s2.index("### Order check")]
print("- **By family and client count (geometric mean of Amphora/best).**")
print()
print("\n".join(l for l in fam.splitlines() if l.startswith("|")))
print()
gaps = [l for l in s3.splitlines() if re.match(r"\| \d+ \|", l)]
print("- **The ten largest gaps (Amphora furthest behind):**")
for l in gaps:
    c = [x.strip() for x in l.strip("|").split("|")]
    # rank, op, clients, ratio, competitor, amphora ops, best ops, amph p50/p99, best p50/p99, verdict, samples
    print(f"  {c[0]}. {c[1]} at {c[2]} clients: {c[3]} of {c[4]} ({c[5]} vs {c[6]} ops/s; p50/p99 ms {c[7]} vs {c[8]}; {c[9]}; min samples {c[10]})")
order = s2[s2.index("### Order check"):]
print("- **Order check:** " + "; ".join(f"{m.group(1)} odd/even {m.group(2)} over {m.group(3)} cells" for m in re.finditer(r"\| (\w+) \| ([\d.]+) \| (\d+) \|", order)))
if "## 4." in t:
    s4 = t[t.index("## 4."):]
    print("- **Beside S4's table (per target, cells over the 17 statements x 3 client counts):**")
    for m in re.finditer(r"^- (\w+): (\d+ cells no measured change, \d+ above S4's range, \d+ below\.)", s4, re.M):
        print(f"  - {m.group(1)}: {m.group(2)}")
    if "### The largest changes" in s4:
        lc = s4[s4.index("### The largest changes"):]
        rows = [l for l in lc.splitlines() if l.startswith("| ") and not l.startswith("| target") and "|---" not in l]
        print("- **Largest changes outside S4's range (median of rounds; a Turso cell also changed engine wait, see below):**")
        for l in rows:
            c = [x.strip() for x in l.strip("|").split("|")]
            print(f"  - {c[0]} {c[1]} at {c[2]} clients: {c[3]} -> {c[4]} ops/s ({c[5]}, {c[6]})")
