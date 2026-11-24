"""Lane W5 (oltp): ab-w5.sh's table, W4's ab_w4_stats.py over any arms, with rank-sum tests of each
pair named in PAIRS (second against first).

python3 -I ab_w5_stats.py <folder of r<round>-<arm>-<op>-<clients>.json and load-<arm>.json> "<arms>"
Per statement and clients, per arm: median (range) of writes a second, p50 and p99 ms, rows and
commits a flush (the arm's load-only run subtracted, as ab_w3.py does), a flush's mean ms and the
share of the counters' 1.5 s (warm-up and timed window) the flusher spent flushing; then the
Mann-Whitney U of writes a second and of p50 per pair, with its exact two-sided p over every
split of the runs.
"""
import glob
import itertools
import math
import json
import os
import re
import statistics
import sys


def read(path):
    r = json.load(open(path))
    xs = r.get("results") or []
    return (xs[0] if xs else {}), (r.get("stats") or {})


def ranks(values):
    order = sorted(range(len(values)), key=lambda i: values[i])
    out = [0.0] * len(values)
    i = 0
    while i < len(order):
        j = i
        while j + 1 < len(order) and values[order[j + 1]] == values[order[i]]:
            j += 1
        for k in range(i, j + 1):
            out[order[k]] = (i + j) / 2 + 1
        i = j + 1
    return out


def u_of(a, b):
    r = ranks(a + b)
    return sum(r[: len(a)]) - len(a) * (len(a) + 1) / 2


def mann_whitney(a, b):
    if not a or not b:
        return None, None
    u = u_of(a, b)
    pooled = a + b
    if math.comb(len(pooled), len(a)) > 20000:
        # Too many splits to count: the normal approximation (no tie correction), two-sided.
        n1, n2 = len(a), len(b)
        z = (u - n1 * n2 / 2) / math.sqrt(n1 * n2 * (n1 + n2 + 1) / 12)
        return min(u, n1 * n2 - u), min(1.0, math.erfc(abs(z) / math.sqrt(2)))
    us = [u_of([pooled[i] for i in s], [pooled[i] for i in range(len(pooled)) if i not in s])
          for s in map(set, itertools.combinations(range(len(pooled)), len(a)))]
    lo = sum(1 for x in us if x <= u) / len(us)
    hi = sum(1 for x in us if x >= u) / len(us)
    return min(u, len(a) * len(b) - u), min(1.0, 2 * min(lo, hi))


def spread(v, f="{:.3f}"):
    v = [a for a in v if a is not None]
    if not v:
        return "-"
    return f"{f.format(statistics.median(v))} ({f.format(min(v))} to {f.format(max(v))})"


PAIRS = [("M", "D2"), ("T", "D2"), ("M", "D1"), ("T", "D1"), ("T", "M")]


def main(folder, arms):
    load = {a: read(p)[1] for a in arms for p in [os.path.join(folder, f"load-{a}.json")] if os.path.exists(p)}
    runs = {}
    for p in sorted(glob.glob(os.path.join(folder, "r*-*.json"))):
        m = re.match(r"r(\d+)-(" + "|".join(arms) + r")-(\w+?)-(\d+)\.json$", os.path.basename(p))
        if m:
            runs.setdefault((m[3], int(m[4]), m[2]), []).append(read(p))
    print("| statement | clients | arm | runs | writes/s | p50 ms | p99 ms | rows/flush | commits/flush | ms/flush | flushing share | errors |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|")
    cells = sorted({(op, c) for op, c, _ in runs})
    for op, c in cells:
        for arm in arms:
            rs = runs.get((op, c, arm))
            if not rs:
                continue
            base = load.get(arm, {})
            per_flush, commits_per_flush, ms_per_flush, share = [], [], [], []
            for _, st in rs:
                f = (st.get("flushes") or 0) - base.get("flushes", 0)
                if f > 0:
                    per_flush.append((st.get("rows", 0) - base.get("rows", 0)) / f)
                    commits_per_flush.append((st.get("commits", 0) - base.get("commits", 0)) / f)
                    ms = (st.get("flush_ms") or 0) - base.get("flush_ms", 0)
                    ms_per_flush.append(ms / f)
                    share.append(ms / 1500)
            print(f"| {op} | {c} | {arm} | {len(rs)} | {spread([x.get('ops_per_s') for x, _ in rs], '{:.0f}')} | "
                  f"{spread([(x.get('ms') or {}).get('p50') for x, _ in rs])} | {spread([(x.get('ms') or {}).get('p99') for x, _ in rs], '{:.2f}')} | "
                  f"{spread(per_flush, '{:.1f}')} | {spread(commits_per_flush, '{:.2f}')} | {spread(ms_per_flush, '{:.2f}')} | {spread(share, '{:.2f}')} | {sum((x.get('errors') or 0) + (x.get('retries') or 0) for x, _ in rs)} |")
    print()
    for (first, second), (op, c) in itertools.product([p for p in PAIRS if set(p) <= set(arms)], cells):
        a, b = runs.get((op, c, first), []), runs.get((op, c, second), [])
        for what, get in (("writes/s", lambda x: x.get("ops_per_s")), ("p50", lambda x: (x.get("ms") or {}).get("p50"))):
            va = [get(x) for x, _ in a if get(x) is not None]
            vb = [get(x) for x, _ in b if get(x) is not None]
            u, p = mann_whitney(va, vb)
            if u is not None:
                change = (statistics.median(vb) - statistics.median(va)) / statistics.median(va) * 100
                print(f"RANKSUM {second} against {first} {op} {c} clients {what}: {len(va)} and {len(vb)} runs; median {change:+.1f}%; U {u:.1f}, exact two-sided p {p:.3f}")


main(sys.argv[1], sys.argv[2].split())
