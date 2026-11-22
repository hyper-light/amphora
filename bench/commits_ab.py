#!/usr/bin/env python3
"""The table of bench/commits_ab.sh's records (kiln goal ab3aedaf): for each cell, each arm's best across the rounds
and after's ratio to before's. Suite cells (statement, clients): the highest throughput, and the fewest bytes left
in --dir and in its log files. --sql-api cells (shape): the least ns an execute inside the API, its CPU ns where the
host counts them (Linux schedstat), and whole. Prints markdown; the same records always print the same table.

usage: python3 bench/commits_ab.py runs.jsonl
"""
import json
import sys

high, low = {}, {}
cells = set()


def keep(table, key, value, better):
    if value is None:
        return
    table[key] = better(table.get(key, value), value)


for line in open(sys.argv[1]):
    r = json.loads(line)
    arm = r["arm"]
    for res in r.get("results", []):
        if res.get("unsupported"):
            continue
        cell = ("suite", res["op"], res["clients"])
        cells.add(cell)
        keep(high, cell + ("ops/s", arm), res.get("ops_per_s"), max)
        keep(low, cell + ("disk bytes", arm), r.get("disk_bytes_after"), min)
        keep(low, cell + ("log bytes", arm), r.get("log_bytes_after"), min)
    if r.get("record") == "sql_api":
        cell = ("sql_api", r["shape"], r.get("statement", ""))
        cells.add(cell)
        for column in ("inside_ns_per_execute", "inside_cpu_ns_per_execute", "whole_ns_per_execute"):
            keep(low, cell + (column, arm), r.get(column), min)


def show(x):
    return "-" if x is None else f"{x:,.1f}" if isinstance(x, float) else f"{x:,}"


print("| kind | cell | measure | before | after | after / before |")
print("|---|---|---|---|---|---|")
for cell in sorted(cells, key=str):
    for table in (high, low):
        for measure in sorted({k[3] for k in table if k[:3] == cell}):
            b, a = table.get(cell + (measure, "before")), table.get(cell + (measure, "after"))
            ratio = f"{a / b:.3f}" if a is not None and b else "-"
            print(f"| {cell[0]} | {' '.join(str(c) for c in cell[1:] if c != '')} | {measure} | {show(b)} | {show(a)} | {ratio} |")
