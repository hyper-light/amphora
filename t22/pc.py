#!/usr/bin/env python3
"""Lane T22: a traced run's VDBE instruction samples (stderr of the bench with AMPHORA_PC_TRACE=1): per instruction, its
share of the timed runs' samples and the milliseconds that share is of the runs' median time, with the instruction's
EXPLAIN line. Usage: pc.py LOG RECORD.jsonl [least share in percent]"""
import json, re, statistics, sys

log, rec = sys.argv[1], sys.argv[2]
least = float(sys.argv[3]) if len(sys.argv) > 3 else 0.4
runs, insns = [], {}
for line in open(log, errors="replace"):
    line = line.rstrip("\n")
    if line.startswith("T22 run total="):
        m = re.match(r"T22 run total=(\d+)(.*)", line)
        total = int(m.group(1))
        hist = {int(a): int(b) for a, b in re.findall(r"(\d+):(\d+)", m.group(2))}
        runs.append((total, hist))
    elif line.startswith("T22 insn "):
        f = line[len("T22 insn "):].split("|")
        if f and f[0].isdigit():
            insns[int(f[0])] = " ".join(f[1:])
ms = []
for line in open(rec):
    r = json.loads(line) if line.strip() else {}
    if r.get("record") == "query":
        ms += [float(x["ms"]) for x in r["runs"] if x.get("status") == "done"]
med = statistics.median(ms) if ms else float("nan")
agg = {}
tot = 0
for total, h in runs:
    tot += total
    for pc, n in h.items():
        agg[pc] = agg.get(pc, 0) + n
print(f"runs traced {len(runs)} samples {tot}; timed runs ms {[round(x) for x in ms]} median {med:.0f}")
print("pc   share%   ms     instruction")
for pc in sorted(insns):
    n = agg.get(pc, 0)
    share = 100.0 * n / tot if tot else 0.0
    if share >= least:
        print(f"{pc:4d} {share:6.2f} {share / 100 * med:7.1f}  {insns[pc][:150]}")
print("sampled pcs below the least share:", sum(1 for p in agg if 100.0 * agg[p] / max(tot, 1) < least), "sum share %.2f" % sum(100.0 * n / max(tot, 1) for p, n in agg.items() if 100.0 * n / max(tot, 1) < least))
