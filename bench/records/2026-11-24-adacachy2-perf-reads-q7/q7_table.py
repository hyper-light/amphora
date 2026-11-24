#!/usr/bin/env python3
"""Lane Q7: K0's A/B table from q7_ab.sh's runs (ab.jsonl, ab-answers.jsonl): each run's reads and
events a second, cores, the mixed phase's flushes and ms a flush (stats_after_mixed less
stats_after_preload) and the load before it; then per client count and arm the median and range,
B/A and an exact two-sided rank-sum p; then the answers held to SQLite's.
usage: q7_table.py AB.jsonl ANSWERS.jsonl A B"""
import itertools, json, statistics, sys

runs = [json.loads(l) for l in open(sys.argv[1])]
answers = [json.loads(l) for l in open(sys.argv[2])]
a, b = sys.argv[3], sys.argv[4]
ok = lambda r: r["exit"] == 0 and isinstance(r["report"].get("read_ms"), dict)


def mixed(x, k):
    m, p = x.get("stats_after_mixed") or {}, x.get("stats_after_preload") or {}
    return (m.get(k) or 0) - (p.get(k) or 0)


def ranksum_p(x, y):
    # exact two-sided rank-sum (Mann-Whitney) p: every split of the pooled mid-ranks into |x| and |y|
    if not x or not y:
        return float("nan")
    pooled = x + y
    order = sorted(range(len(pooled)), key=lambda i: pooled[i])
    ranks = [0.0] * len(pooled)
    i = 0
    while i < len(order):
        j = i
        while j + 1 < len(order) and pooled[order[j + 1]] == pooled[order[i]]:
            j += 1
        for k in range(i, j + 1):
            ranks[order[k]] = (i + j) / 2 + 1
        i = j + 1
    k = len(x)
    mid = k * (len(pooled) + 1) / 2
    obs = abs(sum(ranks[:k]) - mid)
    hits = total = 0
    for c in itertools.combinations(ranks, k):
        total += 1
        hits += abs(sum(c) - mid) >= obs - 1e-9
    return hits / total


print("| run | reads/s | events/s | cores | mixed flushes | ms a flush | load before |")
print("|---|---|---|---|---|---|---|")
for r in runs:
    if not ok(r):
        print(f"| {r['tag']} | failed: exit {r['exit']} | | | | | {r['state_before']} |")
        continue
    x = r["report"]
    f, fm = mixed(x, "flushes"), mixed(x, "flush_ms")
    print(f"| {r['tag']} | {x['read_ms']['n'] / x['seconds']:,.0f} | {x['write_events_per_s']:,.0f} | {(x.get('mixed_cpu_s') or 0) / x['seconds']:.1f} | {f} | {fm / f if f else float('nan'):.0f} | {r['state_before'].split(' cpu_psi')[0].replace('load ', '')} |")
print()
print("| clients | arm | runs | reads/s median (range) | events/s median (range) | cores median | read p50 / p99 ms | failures | unanswered | failed runs |")
print("|---|---|---|---|---|---|---|---|---|---|")
got = {}
for n in (1, 32, 128):
    for arm in (a, b):
        rs = [r for r in runs if r["clients"] == n and r["arm"] == arm]
        xs = [r["report"] for r in rs if ok(r)]
        reads = [x["read_ms"]["n"] / x["seconds"] for x in xs]
        events = [x["write_events_per_s"] for x in xs]
        cores = [(x.get("mixed_cpu_s") or 0) / x["seconds"] for x in xs]
        got[(n, arm)] = (reads, events)
        p = lambda k: statistics.median(x["read_ms"][k] for x in xs) if xs else float("nan")
        rng = lambda v: f"{statistics.median(v):,.0f} ({min(v):,.0f}-{max(v):,.0f})" if v else "-"
        print(f"| {n} | {arm[:8]} | {len(xs)} | {rng(reads)} | {rng(events)} | {statistics.median(cores) if cores else float('nan'):.1f} | {p('p50'):.3f} / {p('p99'):.3f} | {sum(x.get('failures', 0) for x in xs)} | {sum(x.get('unanswered', 0) or 0 for x in xs)} | {len(rs) - len(xs)} |")
for n in (1, 32, 128):
    (ra, ea), (rb, eb) = got[(n, a)], got[(n, b)]
    ratio = lambda x, y: statistics.median(y) / statistics.median(x) if x and y and statistics.median(x) else float("nan")
    print(f"{n} clients: B/A reads {ratio(ra, rb):.2f}x (rank-sum p {ranksum_p(ra, rb):.4f}), events {ratio(ea, eb):.2f}x (p {ranksum_p(ea, eb):.4f})")
print(f"answers held to SQLite: {len(answers)} runs, {sum(x['sessions'] for x in answers)} pages, {sum(x['differ'] for x in answers)} differ")
