#!/usr/bin/env python3
"""Lane Q4: q4_ways.sh's runs.jsonl as a markdown table: per statement and client count, each way's
median statements a second over its rounds with the range, each way over locked, and errors summed
(printed in the cell when any). Then the load before and after each run.
usage: q4_table.py runs.jsonl"""
import collections, json, statistics, sys

ways = ["locked", "filtered", "published", "optimistic"]
rows = collections.defaultdict(list)
errors = collections.Counter()
order = []
loads = []
for line in open(sys.argv[1]):
    r = json.loads(line)
    if r["way"] not in ways or not r["tag"].startswith(r["way"] + "-"):
        continue
    loads.append((r["tag"], r.get("state_before", ""), " ".join(r.get("load_after", []))))
    for x in r["report"].get("results", []):
        key = (x["op"], x["clients"])
        if key not in order:
            order.append(key)
        rows[key + (r["way"],)].append(x.get("ops_per_s") or 0)
        errors[key + (r["way"],)] += x.get("errors") or 0


def med(k):
    v = rows.get(k)
    return statistics.median(v) if v else None


def cell(k):
    v = rows.get(k)
    if not v:
        return "-"
    e = f", {errors[k]} errors" if errors[k] else ""
    return f"{statistics.median(v):,.0f} ({min(v):,.0f}-{max(v):,.0f}){e}"


print("| statement | clients | " + " | ".join(ways) + " | " + " | ".join(f"{w} / locked" for w in ways[1:]) + " |")
print("|---" * (2 + len(ways) + len(ways) - 1) + "|")
for op, c in order:
    base = med((op, c, "locked"))
    ratios = [f"{med((op, c, w)) / base:.2f}x" if base and med((op, c, w)) else "-" for w in ways[1:]]
    print(f"| {op} | {c} | " + " | ".join(cell((op, c, w)) for w in ways) + " | " + " | ".join(ratios) + " |")
print(f"\nRounds per way: {len(rows.get(order[0] + ('locked',), [])) if order else 0}. Errors in all: {sum(errors.values())}.")
print("Load (1/5/15-minute) and CPU/I-O pressure before each run, and the load after it:\n")
for tag, before, after in loads:
    print(f"- {tag}: before `{before}`, after `{after}`")
