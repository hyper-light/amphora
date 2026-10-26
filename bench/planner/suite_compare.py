#!/usr/bin/env python3
"""Two builds' bench suite runs in turns (bench/planner/suite-turns.sh) compared: each statement's
throughput at each client count, best of the turns of each build that began at load under the
limit, and after over before. Writes Markdown to stdout; with --json, the rows as JSON.

usage: bench/planner/suite_compare.py <turns.jsonl> <before> <after> [--max-load 2] [--json out.json]
"""

import json
import sys
from collections import defaultdict


def main(argv):
    path, before, after = argv[0], argv[1], argv[2]
    max_load = float(argv[argv.index("--max-load") + 1]) if "--max-load" in argv else 2.0
    json_out = argv[argv.index("--json") + 1] if "--json" in argv else None
    best = defaultdict(dict)
    what = {}
    runs = defaultdict(lambda: [0, 0])
    with open(path) as f:
        for line in f:
            r = json.loads(line)
            counted = r["load_before"] < max_load
            runs[r["build"]][0 if counted else 1] += 1
            if not counted:
                continue
            for res in r["report"]["results"]:
                key = (res["op"], res["clients"])
                what[key] = res["what"]
                if res.get("errors"):
                    continue
                best[r["build"]][key] = max(best[r["build"]].get(key, 0.0), res["ops_per_s"])
    rows = []
    for key in sorted(set(best[before]) & set(best[after])):
        b, a = best[before][key], best[after][key]
        rows.append({"op": key[0], "clients": key[1], "before": b, "after": a, "ratio": a / b if b else None})
    for build in (before, after):
        print(f"- {build}: {runs[build][0]} runs began at load under {max_load}, {runs[build][1]} past it (left out)")
    ratios = sorted(r["ratio"] for r in rows if r["ratio"])
    print(f"- statements and client counts compared: {len(rows)}; after/before median {ratios[len(ratios) // 2]:.4f}, "
          f"lowest {ratios[0]:.4f}, highest {ratios[-1]:.4f}; within 2%: {sum(1 for x in ratios if abs(x - 1) <= 0.02)} of {len(ratios)}")
    print("\n| statement | clients | before /s | after /s | after/before |\n|---|---:|---:|---:|---:|")
    for r in rows:
        print(f"| {r['op']} | {r['clients']} | {r['before']:.1f} | {r['after']:.1f} | {r['ratio']:.4f} |")
    if json_out:
        with open(json_out, "w") as f:
            json.dump({"before": before, "after": after, "max_load": max_load, "rows": rows}, f, indent=1)


if __name__ == "__main__":
    main(sys.argv[1:])
