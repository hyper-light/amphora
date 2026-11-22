#!/usr/bin/env python3
"""The table of bench/log_ab.sh's records: for each statement and client count, each log's best
throughput across the rounds, the mixed log's ratio to the page-a-frame log's, each log's least CPU a
statement in the timed run (the process's user and system time over the statements it ran), and each
log's fewest instructions retired (the whole process, load included: a timed run's count grows with
the statements it ran, so it compares logs only beside their throughput). Prints markdown.

usage: python3 bench/log_ab.py runs.jsonl
"""
import json
import sys
from collections import defaultdict

best = defaultdict(float)
fewest = {}
cpu = {}
cells = set()
for line in open(sys.argv[1]):
    r = json.loads(line)
    for res in r.get("results", []):
        if res.get("unsupported"):
            continue
        cell = (res["op"], res["clients"])
        cells.add(cell)
        key = cell + (r["log"],)
        best[key] = max(best[key], res["ops_per_s"])
        n = res["ops_per_s"] * r["seconds"]
        if n and res.get("cpu_user_s") is not None:
            us = (res["cpu_user_s"] + (res.get("cpu_sys_s") or 0.0)) * 1e6 / n
            cpu[key] = min(cpu.get(key, us), us)
        if r.get("instructions") is not None:
            fewest[key] = min(fewest.get(key, r["instructions"]), r["instructions"])
print("| statement | clients | pages /s | mixed /s | mixed / pages | pages CPU us | mixed CPU us | pages instr | mixed instr |")
print("|---|---|---|---|---|---|---|---|---|")
for op, clients in sorted(cells):
    p, m = best[(op, clients, "pages")], best[(op, clients, "mixed")]
    ratio = f"{m / p:.3f}" if p else "-"
    pi, mi = fewest.get((op, clients, "pages")), fewest.get((op, clients, "mixed"))
    pc, mc = cpu.get((op, clients, "pages")), cpu.get((op, clients, "mixed"))
    show = lambda x: f"{x:,.0f}" if x is not None else "-"
    print(f"| {op} | {clients} | {p:,.1f} | {m:,.1f} | {ratio} | {show(pc)} | {show(mc)} | {pi if pi is not None else '-'} | {mi if mi is not None else '-'} |")
