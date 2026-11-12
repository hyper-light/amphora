#!/usr/bin/env python3
"""The follower-write candidates' A/B tables (goal 9c63f7d4, kiln plan 01a1b8d174dc step 3).

Reads the model's JSON lines (crates/amphora/tests/write_starve_model.rs, `the_ab_sweep`) and prints
Markdown: one table per workload of every candidate at every leader rate and round trip, then a
summary of each candidate's worst case across them all. Usage: write_starve_table.py MODEL.jsonl
"""

import json
import sys


def main(path: str) -> None:
    rows = [json.loads(line) for line in open(path, encoding="utf-8") if line.strip()]
    base = [r for r in rows if r.get("variant", "base") == "base"]
    workloads = list(dict.fromkeys(r["workload"] for r in base))
    candidates = list(dict.fromkeys(r["candidate"] for r in base))
    print("Columns: follower write latency p50 / p99 / max (ms); deadline misses of writes; refusals and")
    print("false refusals (no row conflicted, appends rebased); the leader's own writes a second against")
    print("its rate with no follower; the leader's p99 (ms) against the same; seconds the leader's slot")
    print("was held for the follower, of 2,400 (twenty seeds of 120 s).")
    for workload in workloads:
        print(f"\n### {workload}\n")
        print("| rate/s | rtt ms | cand | p50 | p99 | max | misses | refusals | false (rows) | leader/s (alone) | leader p99 (alone) | slot held s |")
        print("|---|---|---|---|---|---|---|---|---|---|---|---|")
        for r in base:
            if r["workload"] != workload:
                continue
            print(
                f"| {r['rate']:g} | {r['rtt_ms']:g} | {r['candidate']} | {r['p50_ms']:.1f} | {r['p99_ms']:.1f} | "
                f"{r['max_ms']:.1f} | {r['follower_misses']} of {r['follower_done'] + r['follower_misses']} | "
                f"{r['refusals']} | {r['false_refusals_rows']} | {r['leader_per_s']:.1f} ({r['leader_baseline_per_s']:.1f}) | "
                f"{r['leader_p99_ms']:.1f} ({r['leader_baseline_p99_ms']:.1f}) | {r['leader_stall_s']:.1f} |"
            )
    print("\n### Each candidate's worst cell\n")
    print("| cand | cells with a miss | worst p99 ms (where) | worst max ms (where) | least leader share of alone (where) |")
    print("|---|---|---|---|---|")
    for cand in candidates:
        mine = [r for r in base if r["candidate"] == cand]
        missed = sum(1 for r in mine if r["follower_misses"] > 0)
        where = lambda r: f"{r['workload']} {r['rate']:g}/s {r['rtt_ms']:g} ms"
        p99 = max(mine, key=lambda r: r["p99_ms"] if r["follower_misses"] == 0 else float("inf"))
        mx = max(mine, key=lambda r: r["max_ms"] if r["follower_misses"] == 0 else float("inf"))
        share = min(mine, key=lambda r: r["leader_per_s"] / r["leader_baseline_per_s"] if r["leader_baseline_per_s"] else 1)
        share_v = share["leader_per_s"] / share["leader_baseline_per_s"] if share["leader_baseline_per_s"] else 1
        p99_v = "misses" if p99["follower_misses"] else f"{p99['p99_ms']:.1f}"
        mx_v = "misses" if mx["follower_misses"] else f"{mx['max_ms']:.1f}"
        print(f"| {cand} | {missed} of {len(mine)} | {p99_v} ({where(p99)}) | {mx_v} ({where(mx)}) | {share_v:.2f} ({where(share)}) |")
    slow = [r for r in rows if r.get("variant") == "slow"]
    if slow:
        print("\n### Sensitivity: preparations 20 us a page, the follower writing once a second, 10 ms round trip\n")
        print("| workload | rate/s | cand | p50 | p99 | max | misses | leader/s (alone) |")
        print("|---|---|---|---|---|---|---|---|")
        for r in slow:
            print(
                f"| {r['workload']} | {r['rate']:g} | {r['candidate']} | {r['p50_ms']:.1f} | {r['p99_ms']:.1f} | {r['max_ms']:.1f} | "
                f"{r['follower_misses']} of {r['follower_done'] + r['follower_misses']} | {r['leader_per_s']:.1f} ({r['leader_baseline_per_s']:.1f}) |"
            )


if __name__ == "__main__":
    main(sys.argv[1])
