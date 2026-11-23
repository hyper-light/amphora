"""Lane V12: ladder.sh's records as one row a cell, each round's value and the median, with the load and
pressure logged at the cell's start. usage: python3 -I ladder_table.py OUT"""
import glob, json, os, re, statistics, sys

out = sys.argv[1]
env = {}
for line in open(os.path.join(out, "cells.log")):
    m = re.match(r"\S+ start r(\d) (\S+) (\d+) (\S+) load=(\S+) cpu_some=(\S+) io_some=(\S+) io_full=(\S+)", line)
    if m:
        env[(int(m[1]), m[2], int(m[3]), m[4])] = (m[5].split(",")[0], m[6][6:], m[7][6:], m[8][6:])
    m = re.match(r"\S+ end r(\d) (\S+) (\d+) (\S+) rc=(\d+)", line)
    if m:
        env[("rc", int(m[1]), m[2], int(m[3]), m[4])] = m[5]
rows = {}
for f in sorted(glob.glob(os.path.join(out, "r*", "*-commit.jsonl"))):
    rnd = int(os.path.basename(os.path.dirname(f))[1:])
    system = os.path.basename(f)[: -len("-commit.jsonl")]
    for line in open(f):
        r = json.loads(line)
        key = (system, r.get("writers"), r.get("route") or "?")
        lat = r.get("latency_ms") or {}
        ser = sum((n.get("refused") or 0) for n in (r.get("by_node") or []))
        rows.setdefault(key, {})[rnd] = {
            "cps": r.get("writes_per_s"), "p50": lat.get("p50"), "p99": lat.get("p99"), "ser": ser,
            "miss": r.get("deadline_misses"), "refused": r.get("refused"), "load1": r.get("load1")}
f2 = lambda v, d=1: "-" if v is None else (f"{v:.{d}f}" if isinstance(v, (int, float)) else str(v))
print("| system | writers | route | commits/s per round | median | p50 ms per round | p99 ms per round | stale-base refusals | deadline misses | run refused | load1, cpu some, io some per round |")
print("|---|---|---|---|---|---|---|---|---|---|---|")
for s in sorted({k[1] for k in env if k[0] != "rc"} | {k[0] for k in rows}):
    for w in (1, 32, 128):
        for rt in ("spread", "leader"):
            per = rows.get((s, w, rt), {})
            cps = [per.get(i, {}).get("cps") for i in (1, 2, 3)]
            med = statistics.median([c for c in cps if c is not None]) if any(c is not None for c in cps) else None
            refused = [str(per[i]["refused"]) for i in (1, 2, 3) if i in per and per[i].get("refused")]
            failed = [f"r{i} rc={env.get(('rc', i, s, w, rt))}" for i in (1, 2, 3) if i not in per]
            ld = ["/".join(env.get((i, s, w, rt), ("?",) * 4)[:3]) for i in (1, 2, 3)]
            print(f"| {s} | {w} | {rt} | {', '.join(f2(c) for c in cps)} | {f2(med)} | "
                  f"{', '.join(f2(per.get(i, {}).get('p50')) for i in (1, 2, 3))} | {', '.join(f2(per.get(i, {}).get('p99')) for i in (1, 2, 3))} | "
                  f"{', '.join(f2(per.get(i, {}).get('ser'), 0) for i in (1, 2, 3))} | {', '.join(f2(per.get(i, {}).get('miss'), 0) for i in (1, 2, 3))} | "
                  f"{'; '.join(refused + failed) or 'none'} | {'; '.join(ld)} |")
