"""Lane R1: the timed rounds of bench/real_sums_ab.sh, pooled per phase, arm, query and worker count.
p50, p99 and p99.9 are nearest-rank over every timed run of every round; with n runs, a percentile
past 1 - 1/n is the slowest run, and the table says how many runs each rests on. Failed or timed
out runs are counted, never timed.

usage: python3 -I bench/real_sums_stats.py <out dir of bench/real_sums_ab.sh>
"""
import glob
import json
import math
import os
import re
import sys

OUT = sys.argv[1]
runs = {}
bad = {}
loads = {}
for path in sorted(glob.glob(os.path.join(OUT, "timed-*-r*.jsonl"))):
    m = re.match(r"(timed-[ab])-(\w+)-r(\d+)\.jsonl", os.path.basename(path))
    if not m:
        continue
    phase, arm = m.group(1), m.group(2)
    for line in open(path):
        rec = json.loads(line)
        if rec.get("record") != "scaling":
            continue
        k = (phase, rec["query"], rec["cores"], arm)
        for r in rec["runs"]:
            if r.get("status") == "done":
                runs.setdefault(k, []).append(r["ms"])
            else:
                bad[k] = bad.get(k, 0) + 1
            if r.get("load") is not None:
                loads.setdefault(k, []).append(r["load"])


def pct(xs, p):
    xs = sorted(xs)
    return xs[max(0, math.ceil(p * len(xs)) - 1)]


def cell(k):
    xs = runs.get(k)
    return pct(xs, 0.5) if xs else None


print("| phase | query | workers | arm | runs | failed | p50 ms | p99 ms | p99.9 ms | min-max ms | load | p50 vs Serial same workers | p50 vs Serial 1 worker | p50 vs DuckDB same workers |")
print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
for k in sorted(set(runs) | set(bad), key=lambda k: (k[0], k[1], k[2], ("serial", "exact", "roworder", "duckdb").index(k[3]))):
    phase, q, n, arm = k
    xs = runs.get(k, [])
    p50 = cell(k)
    ser, ser1, duck = cell((phase, q, n, "serial")), cell((phase, q, 1, "serial")), cell((phase, q, n, "duckdb"))
    ratio = lambda a, b: "%.2fx" % (a / b) if a and b else "-"
    ld = loads.get(k, [])
    print("| %s | %s | %d | %s | %d | %d | %s | %s | %s | %s | %s | %s | %s | %s |" % (
        phase, q, n, arm, len(xs), bad.get(k, 0),
        "%.1f" % p50 if xs else "-", "%.1f" % pct(xs, 0.99) if xs else "-", "%.1f" % pct(xs, 0.999) if xs else "-",
        "%.1f-%.1f" % (min(xs), max(xs)) if xs else "-", "%.1f-%.1f" % (min(ld), max(ld)) if ld else "-",
        ratio(ser, p50) if arm != "serial" else "1.00x", ratio(ser1, p50), ratio(duck, p50) if arm != "duckdb" else "-"))
