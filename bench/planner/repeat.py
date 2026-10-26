#!/usr/bin/env python3
"""Two passes of the planner bench on the same build and machine compared (bench/planner/README.md):
Amphora's estimates at every counter, which must be the same (a planner that estimates the same
query twice alike), the rows counted, and each engine's run times: the median over the queries of
each query's best time, which the plan's test holds within 5% between passes.

usage: bench/planner/repeat.py <pass a.jsonl> <pass b.jsonl> [--json out.json]
"""

import json
import statistics
import sys


def records(path):
    out = {}
    with open(path) as f:
        for line in f:
            r = json.loads(line)
            if r["record"] == "query":
                out[(r["bench"], r["query"], r["target"])] = r
    return out


def best(r):
    times = [x["ms"] for x in r.get("runs", []) if x.get("status") == "done"]
    return min(times) if times else None


def main(argv):
    a, b = records(argv[0]), records(argv[1])
    json_out = argv[argv.index("--json") + 1] if "--json" in argv else None
    keys = sorted(set(a) & set(b))
    estimate_diffs, count_diffs, counters = [], [], 0
    for k in keys:
        if k[2] != "amphora":
            continue
        ca, cb = a[k].get("counts", []), b[k].get("counts", [])
        sites = lambda cs: [(c["plan"], c["kind"], c["table"], c["position"]) for c in cs]
        if sites(ca) != sites(cb):
            estimate_diffs.append({"query": k[1], "why": "different counters"})
            continue
        for x, y in zip(ca, cb):
            counters += 1
            if x["est_output"] != y["est_output"] or x["est_input"] != y["est_input"]:
                estimate_diffs.append({"query": k[1], "table": x["table"], "a": x["est_output"], "b": y["est_output"]})
            if x["rows"] != y["rows"]:
                count_diffs.append({"query": k[1], "table": x["table"], "a": x["rows"], "b": y["rows"]})
    engines = {}
    for t in sorted({k[2] for k in keys}):
        pairs = [(best(a[k]), best(b[k])) for k in keys if k[2] == t]
        pairs = [(x, y) for x, y in pairs if x is not None and y is not None]
        if not pairs:
            continue
        ma, mb = statistics.median(x for x, _ in pairs), statistics.median(y for _, y in pairs)
        engines[t] = {"queries": len(pairs), "median_ms_a": ma, "median_ms_b": mb, "b_over_a": mb / ma,
                      "per_query_ratio_median": statistics.median(y / x for x, y in pairs)}
    print(f"- Amphora's counters compared: {counters}; estimates that differ: {len(estimate_diffs)}; rows counted that differ: {len(count_diffs)}")
    for d in estimate_diffs[:20]:
        print(f"  - estimate differs: {d}")
    for d in count_diffs[:20]:
        print(f"  - count differs: {d}")
    print("\n| engine | queries | median best ms, pass a | pass b | b/a | median of b/a by query |\n|---|---:|---:|---:|---:|---:|")
    for t, e in engines.items():
        print(f"| {t} | {e['queries']} | {e['median_ms_a']:.1f} | {e['median_ms_b']:.1f} | {e['b_over_a']:.4f} | {e['per_query_ratio_median']:.4f} |")
    if json_out:
        with open(json_out, "w") as f:
            json.dump({"counters": counters, "estimate_diffs": estimate_diffs, "count_diffs": count_diffs, "engines": engines}, f, indent=1)


if __name__ == "__main__":
    main(sys.argv[1:])
