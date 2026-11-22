#!/usr/bin/env python3
"""PL.13 (lane replan-6; goal "Queries fix a bad plan while they run", AC-PL.13.1): JOB's 20
slowest queries with adaptive re-planning off (`amphora`, the control) and on (`amphora-adaptive`),
beside PostgreSQL, from records `amphora-bench --planner --bench job` wrote: one driver program,
the same runs, deadline and timing method for every engine, on one machine (BRIEF "Fair
comparisons"). The 20 slowest are the control's, by its median time; a query timed out on the
control counts as slowest. Each query's time is the median of its runs; the speed-up is the
geometric mean of control / adaptive over the 20. The goal's bar is 1.27 (27% faster).

    python3 bench/planner/adaptive.py job.jsonl [--top 20] [--json out.json]
"""
import argparse
import json
import math
import statistics
import sys

CONTROL, ADAPTIVE, POSTGRES = "amphora", "amphora-adaptive", "postgres"


def median_ms(runs):
    done = [r["ms"] for r in runs if r.get("status") == "done" and "ms" in r]
    if not done or len(done) != len(runs):
        return None
    return statistics.median(done)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("records", nargs="+")
    ap.add_argument("--top", type=int, default=20)
    ap.add_argument("--json")
    a = ap.parse_args()
    times, about = {}, []
    for path in a.records:
        with open(path) as f:
            for line in f:
                rec = json.loads(line)
                if rec.get("record") == "run":
                    about.append({k: rec.get(k) for k in ("host", "cores", "commit", "date")})
                if rec.get("record") != "query" or rec.get("bench") != "job":
                    continue
                times.setdefault(rec["query"], {})[rec["target"]] = median_ms(rec.get("runs", []))
    control = {q: t.get(CONTROL) for q, t in times.items() if CONTROL in t}
    if not control:
        sys.exit(f"no {CONTROL} records")
    slowest = sorted(control, key=lambda q: math.inf if control[q] is None else control[q], reverse=True)[: a.top]
    rows, ratios = [], []
    for q in slowest:
        t = times[q]
        c, ad, pg = t.get(CONTROL), t.get(ADAPTIVE), t.get(POSTGRES)
        ratio = c / ad if c and ad else None
        if ratio:
            ratios.append(ratio)
        rows.append({"query": q, CONTROL: c, ADAPTIVE: ad, POSTGRES: pg, "speedup": ratio})
    fmt = lambda v: "timeout/failed" if v is None else f"{v:.1f}"
    print(f"{'query':8} {'off ms':>14} {'on ms':>14} {'postgres ms':>14} {'off/on':>8}")
    for r in rows:
        print(f"{r['query']:8} {fmt(r[CONTROL]):>14} {fmt(r[ADAPTIVE]):>14} {fmt(r[POSTGRES]):>14} {fmt(r['speedup']):>8}")
    geo = math.exp(sum(map(math.log, ratios)) / len(ratios)) if ratios else None
    print(f"geometric mean off/on over {len(ratios)} of {len(rows)}: {fmt(geo)} (the goal's bar: 1.27)")
    print("client and platform: amphora-bench --planner, one thread per engine, on", about[:1])
    if a.json:
        with open(a.json, "w") as f:
            json.dump({"about": about, "queries": rows, "geomean_speedup": geo, "bar": 1.27}, f, indent=1)


if __name__ == "__main__":
    main()
