#!/usr/bin/env python3
"""The suite's records (bench/suite.sh) as tables: each statement's throughput and p99 at each
client count for every target, Amphora's ratio to the best of the rest, and its losses first.

usage: python3 bench/suite_table.py records/<date>-<host>-suite.jsonl [more.jsonl...]
"""
import json
import sys
from collections import defaultdict

runs = {}
for path in sys.argv[1:]:
    for line in open(path):
        r = json.loads(line)
        rep = r.get("report", r)
        if r.get("failed") or not rep.get("suite"):
            print(f"failed: {r.get('target')}: {r.get('error', '')[:200]}")
            continue
        runs[rep["target"]] = (r, rep)

cells = defaultdict(dict)  # (op, clients) -> target -> result
kinds, whats = {}, {}
for target, (r, rep) in runs.items():
    for x in rep["results"]:
        cells[(x["op"], x["clients"])][target] = x
        kinds[x["op"]], whats[x["op"]] = x["kind"], x["what"]

targets = sorted(runs, key=lambda t: (t != "amphora", t))
ops = list(dict.fromkeys(op for op, _ in cells))
clients = sorted({c for _, c in cells})


def value(x):
    if x is None:
        return None, "–"
    if x.get("unsupported"):
        return None, "unsupported"
    if x.get("errors") and not x.get("ms"):
        return None, "failed: " + (x.get("error") or "")[:40]
    ms = x.get("ms") or {}
    return x.get("ops_per_s"), f"{x.get('ops_per_s', 0):,.0f}/s p99 {ms.get('p99', '–')}"


for t, (r, rep) in runs.items():
    mem = r.get("server_mem_held_peak_mb") or rep.get("rss_max_mb")
    print(f"- {t}: load {r.get('load_before')}–{r.get('load_after')}, items {rep['items']}, load {rep['load_s']:.1f} s, memory {mem} MB, commit {str(r.get('commit'))[:7]}")
print()
losses = []
for c in clients:
    print(f"### {c} client{'s' if c != 1 else ''}\n")
    print("| statement | " + " | ".join(targets) + " | Amphora / best other |")
    print("|---" * (len(targets) + 2) + "|")
    for op in ops:
        row = cells.get((op, c), {})
        vals = {t: value(row.get(t)) for t in targets}
        amph = vals.get("amphora", (None, ""))[0]
        others = [(v[0], t) for t, v in vals.items() if t != "amphora" and v[0]]
        ratio = ""
        if amph and others:
            best, who = max(others)
            ratio = f"{amph / best:.2f}x ({who})"
            losses.append((amph / best, op, c, who))
        elif others and not amph:
            ratio = "Amphora can't"
            losses.append((0.0, op, c, max(others)[1]))
        print(f"| {op} | " + " | ".join(vals[t][1] for t in targets) + f" | {ratio} |")
    print()

print("### Where Amphora loses, worst first (its throughput over the best other's)\n")
for ratio, op, c, who in sorted(losses):
    if ratio >= 1.0:
        break
    print(f"- {op} ({kinds[op]}, {c} clients): {ratio:.2f}x of {who} — {whats[op]}")
