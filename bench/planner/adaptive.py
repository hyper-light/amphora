#!/usr/bin/env python3
"""PL.13 (lane replan-6; goal "Queries fix a bad plan while they run", AC-PL.13.1): JOB's 20
slowest queries with adaptive re-planning off (`amphora`, the control) and on (`amphora-adaptive`),
beside PostgreSQL, from records `amphora-bench --planner --bench job` wrote: one driver program,
the same runs, deadline and timing method for every engine, on one machine (BRIEF "Fair
comparisons"). The 20 slowest are the control's, by its median warm time; a query timed out on the
control counts as slowest. Each query's warm time is the median of its timed runs; the speed-up is
the geometric mean of control / adaptive over the 20. The goal's bar is 1.27 (27% faster).

Cold and warm (lane replan-9): a warm run follows a run that warmed it (Amphora's counted run, in
which the adaptive engine learns its corrections; the other engines' warm-up run). A cold run is a
query's first on an engine opened by a process of its own (bench/planner/cold.sh: PostgreSQL
restarted, the page cache dropped where the host allows, no correction learned): the `cold`
records. Both are shown for every engine, and the speed-up both ways, so what the adaptive engine
gains by learning is the gap between its cold and warm speed-ups.

    python3 bench/planner/adaptive.py job.jsonl [job-cold.jsonl ...] [--top 20] [--json out.json]
"""
import argparse
import json
import math
import statistics
import sys

CONTROL, ADAPTIVE, POSTGRES = "amphora", "amphora-adaptive", "postgres"
TARGETS = (CONTROL, ADAPTIVE, POSTGRES)


def median_ms(runs):
    done = [r["ms"] for r in runs if r.get("status") == "done" and "ms" in r]
    if not done or len(done) != len(runs):
        return None
    return statistics.median(done)


def read(paths):
    """Each query's warm and cold time on each engine (None: timed out or failed), and each
    pass's machine. A query and engine's last record of each kind is its record."""
    warm, cold, about = {}, {}, []
    for path in paths:
        with open(path) as f:
            for line in f:
                rec = json.loads(line)
                kind = rec.get("record")
                if kind == "run":
                    about.append({k: rec.get(k) for k in ("host", "cores", "commit", "date", "phase")})
                elif kind == "query" and rec.get("bench") == "job":
                    warm.setdefault(rec["query"], {})[rec["target"]] = median_ms(rec.get("runs", []))
                elif kind == "cold" and rec.get("bench") == "job":
                    cold.setdefault(rec["query"], {})[rec["target"]] = median_ms([rec.get("run", {})])
    return warm, cold, about


def geomean(ratios):
    return math.exp(sum(map(math.log, ratios)) / len(ratios)) if ratios else None


def compare(warm, cold, top):
    """The control's `top` slowest warm queries, each engine's cold and warm time, and the
    speed-up off/on warm and cold, with their geometric means."""
    control = {q: t.get(CONTROL) for q, t in warm.items() if CONTROL in t}
    slowest = sorted(control, key=lambda q: math.inf if control[q] is None else control[q], reverse=True)[:top]
    rows, ratios = [], {"warm": [], "cold": []}
    for q in slowest:
        row = {"query": q}
        for phase, times in (("warm", warm), ("cold", cold)):
            t = times.get(q, {})
            for target in TARGETS:
                row[f"{target} {phase}"] = t.get(target)
            c, ad = t.get(CONTROL), t.get(ADAPTIVE)
            row[f"speedup {phase}"] = c / ad if c and ad else None
            if row[f"speedup {phase}"]:
                ratios[phase].append(row[f"speedup {phase}"])
        rows.append(row)
    return rows, {phase: geomean(r) for phase, r in ratios.items()}, {phase: len(r) for phase, r in ratios.items()}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("records", nargs="+")
    ap.add_argument("--top", type=int, default=20)
    ap.add_argument("--json")
    a = ap.parse_args()
    warm, cold, about = read(a.records)
    if not any(CONTROL in t for t in warm.values()):
        sys.exit(f"no {CONTROL} records")
    rows, geo, n = compare(warm, cold, a.top)
    fmt = lambda v: "-" if v is None else f"{v:.1f}"
    cols = [f"{t} {p}" for t in TARGETS for p in ("cold", "warm")] + ["speedup cold", "speedup warm"]
    heads = ["off cold", "off warm", "on cold", "on warm", "pg cold", "pg warm", "off/on cold", "off/on warm"]
    print(f"{'query':8} " + " ".join(f"{h:>12}" for h in heads))
    for r in rows:
        print(f"{r['query']:8} " + " ".join(f"{fmt(r[c]):>12}" for c in cols))
    for phase in ("cold", "warm"):
        print(f"geometric mean off/on {phase} over {n[phase]} of {len(rows)}: {fmt(geo[phase])} (the goal's bar: 1.27)")
    print("ms; - is timed out, failed or not run. client and platform: amphora-bench --planner, one thread per engine, on", about[:1])
    if a.json:
        with open(a.json, "w") as f:
            json.dump({"about": about, "queries": rows, "geomean_speedup": geo, "bar": 1.27}, f, indent=1)


if __name__ == "__main__":
    main()
