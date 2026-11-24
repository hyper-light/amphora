#!/usr/bin/env python3
"""Lane Q3: an alternated A/B's runs.jsonl (q2_ab.sh's or q3_ab.sh's) as a markdown table: per
statement and client count, each tag's median statements a second with its range over the rounds,
B over A, and B over the best competitor. Errors are summed per cell; any is printed in the cell.
usage: q3_table.py runs.jsonl"""
import collections, json, statistics, sys

rows = collections.defaultdict(list)
errors = collections.Counter()
order = []
loads = []
for line in open(sys.argv[1]):
    r = json.loads(line)
    tag = r["tag"].split("-")[0]
    if tag in ("calib", "load"):
        continue
    loads.append((r["tag"], r.get("state_before") or "", " ".join(r.get("load", []))))
    for x in r.get("results", []):
        key = (x["op"], x["clients"])
        if key not in order:
            order.append(key)
        rows[key + (tag,)].append(x.get("ops_per_s") or 0)
        errors[key + (tag,)] += x.get("errors") or 0
tags = [t for t in ("A", "B", "turso", "sqlite", "postgres") if any(k[2] == t for k in rows)]
comp = [t for t in tags if t not in ("A", "B")]


def med(k):
    v = rows.get(k)
    return statistics.median(v) if v else None


def cell(k):
    v = rows.get(k)
    if not v:
        return "-"
    e = f", {errors[k]} errors" if errors[k] else ""
    return f"{statistics.median(v):,.0f} ({min(v):,.0f}-{max(v):,.0f}){e}"


print("| statement | clients | " + " | ".join(tags) + " | B / A | B / best competitor |")
print("|---" * (len(tags) + 4) + "|")
for op, c in order:
    a, b = med((op, c, "A")), med((op, c, "B"))
    best = max(((med((op, c, t)) or 0, t) for t in comp), default=(0, "-"))
    ba = f"{b / a:.2f}x" if a and b else "-"
    bc = f"{b / best[0]:.2f}x of {best[1]}" if b and best[0] else "-"
    n = len(rows.get((op, c, "A"), []))
    print(f"| {op} | {c} | " + " | ".join(cell((op, c, t)) for t in tags) + f" | {ba} | {bc} |")
print()
print(f"Rounds: {n}. Load before each run (1/5/15-minute) and CPU/I-O pressure:")
print()
for tag, before, after in loads:
    print(f"- {tag}: before `{before}`, after `{after}`")
