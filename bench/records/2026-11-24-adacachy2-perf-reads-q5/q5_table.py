#!/usr/bin/env python3
"""Lane Q5: K0's mixed phase per way and client count, median of the timed rounds (range): reads a
second (the readers' reads over the mixed phase), read p50 and p99 (ms), events written a second;
then failures, unanswered writes, failed runs and the answers held to SQLite's.
usage: q5_table.py K0_JSONL ANSWERS_JSONL"""
import collections, json, statistics, sys

ways = ["locked", "filtered", "published", "optimistic"]
cells = collections.defaultdict(lambda: collections.defaultdict(list))
bad = collections.Counter()
for line in open(sys.argv[1]):
    r = json.loads(line)
    if r["tag"].startswith("prof-"):
        continue
    rep = r["report"]
    if r["exit"] != 0 or "read_ms" not in rep:
        bad[(r["way"], r["clients"], "failed runs")] += 1
        continue
    c = cells[(r["clients"], r["way"])]
    c["reads/s"].append(rep["read_ms"]["n"] / rep["seconds"])
    c["read p50"].append(rep["read_ms"]["p50"])
    c["read p99"].append(rep["read_ms"]["p99"])
    c["writes/s"].append(rep["write_events_per_s"])
    bad[(r["way"], r["clients"], "failures")] += rep.get("failures") or 0
    bad[(r["way"], r["clients"], "unanswered")] += rep.get("unanswered") or 0
fmt = lambda xs, f: f"{f(statistics.median(xs))} ({f(min(xs))}-{f(max(xs))})" if xs else "-"
num = lambda x: f"{x:,.0f}"
ms = lambda x: f"{x:.3f}"
for metric, f in [("reads/s", num), ("read p50", ms), ("read p99", ms), ("writes/s", num)]:
    print(f"\n{metric}, median of rounds (range)")
    print("| clients | " + " | ".join(ways) + " | " + " | ".join(f"{w} / locked" for w in ways[1:]) + " |")
    print("|---" * (2 * len(ways)) + "|")
    for n in sorted({k[0] for k in cells}):
        meds = [statistics.median(cells[(n, w)][metric]) if cells[(n, w)][metric] else None for w in ways]
        ratio = [f"{m / meds[0]:.2f}x" if m and meds[0] else "-" for m in meds[1:]]
        print(f"| {n} | " + " | ".join(fmt(cells[(n, w)][metric], f) for w in ways) + " | " + " | ".join(ratio) + " |")
print("\nfailures, unanswered, failed runs:", {f"{k[0]}-{k[1]} {k[2]}": v for k, v in sorted(bad.items()) if v} or "none")
answers = [json.loads(l) for l in open(sys.argv[2])]
print("answers held to SQLite:", len(answers), "runs,", sum(a["sessions"] for a in answers), "pages,", sum(a["differ"] for a in answers), "differ")
for a in answers:
    if a["differ"]:
        print("  differs:", json.dumps(a))
