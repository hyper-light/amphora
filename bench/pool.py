#!/usr/bin/env python3
"""Pools the commit rung's records (`--repl commit --record FILE`) into one histogram per file.

usage: pool.py FILE.jsonl...

Each FILE holds one arm's runs, a JSON record a line. Each run's leader is the member with the most
acknowledged writes (the bench's `leader` names who leads at the end, which differs when the lead moved;
those runs are counted and kept). The leader's `buckets_us` ([least µs, count] pairs) are summed across
the file's runs, and the quantiles read off the sum the way the bench's `Hist::quantile` reads one run:
the least value of the bucket holding sample ceil(n q), capped at the largest sample.
"""
import json
import math
import sys


def quantile(buckets, n, top, q):
    want = max(1, math.ceil(n * q))
    seen = 0
    for least in sorted(buckets):
        seen += buckets[least]
        if seen >= want:
            return min(least, top)
    return top


def pool(path):
    buckets, n, top, refused, runs, moved, missed = {}, 0, 0, 0, 0, 0, 0
    with open(path) as f:
        for line in f:
            if not line.strip():
                continue
            rec = json.loads(line)
            nodes = rec.get("by_node") or []
            if not nodes or "buckets_us" not in nodes[0]:
                sys.exit(f"{path}: a record without by_node buckets_us (a bench before W6)")
            lead = max(range(len(nodes)), key=lambda i: (nodes[i]["latency_ms"] or {}).get("n", 0))
            node = nodes[lead]
            for least, count in node["buckets_us"]:
                buckets[least] = buckets.get(least, 0) + count
            got = sum(c for _, c in node["buckets_us"])
            n += got
            top = max(top, node["max_us"])
            refused += node["refused"]
            runs += 1
            moved += int(rec.get("leader") != lead)
            missed += rec.get("deadline_misses", 0)
    ms = lambda us: us / 1000.0
    return {
        "file": path, "runs": runs, "lead_moved_runs": moved, "leader_writes": n,
        "p50_ms": ms(quantile(buckets, n, top, 0.5)), "p99_ms": ms(quantile(buckets, n, top, 0.99)),
        "p999_ms": ms(quantile(buckets, n, top, 0.999)), "max_ms": ms(top),
        "leader_refused_per_write": round(refused / n, 3) if n else None, "deadline_misses": missed,
    }


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    for p in sys.argv[1:]:
        print(json.dumps(pool(p)))
