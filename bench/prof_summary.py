#!/usr/bin/env python3
"""Each profiled statement's counters, per statement (G1 steps 4 and 5): cycles, instructions and
IPC, L1 data, TLB and cache misses, branch misses, the top-down split of dispatch slots (frontend,
backend, bad speculation, retiring: AMD's PipelineL1 metrics) and the kernel's share of the CPU,
for each engine, from bench/corpus-style profile directories (prof-run.sh's files).

usage: python3 bench/prof_summary.py <core dir> <mem and top-down dir>
  core dir: <target>-<statement>-<clients>-core.{csv,json}; the other: -mem and -topdown runs.
"""
import json
import os
import re
import sys

CORE, OTHER = sys.argv[1], sys.argv[2]


def counts(path):
    out = {}
    if not os.path.exists(path):
        return out
    for line in open(path):
        f = line.rstrip("\n").split(",")
        if len(f) < 3 or not f[0] or f[0].startswith("#"):
            continue
        try:
            out[f[2].split(":")[0]] = float(f[0])
        except ValueError:
            pass
        if len(f) > 6 and f[6].strip():
            m = re.match(r"%slots\s+(\w+)", f[6].strip())
            if m and f[5]:
                out[m.group(1)] = float(f[5])
    return out


def run(path):
    try:
        r = json.loads(open(path).read().strip().splitlines()[-1])
        x = r["results"][0]
        return {"n": x["ops_per_s"] * r["seconds"], "rate": x["ops_per_s"], "user": x.get("cpu_user_s"), "sys": x.get("cpu_sys_s")}
    except (OSError, ValueError, IndexError, KeyError):
        return None


def main():
    keys = sorted({re.match(r"(amphora|sqlite)-(.+)-(1|32)-core\.csv$", f).groups() for f in os.listdir(CORE) if re.match(r"(amphora|sqlite)-(.+)-(1|32)-core\.csv$", f)},
                  key=lambda k: (int(k[2]), k[1], k[0]))
    print("| statement | clients | engine | statements/s | cycles/stmt | instructions/stmt | IPC | L1d misses/stmt | dTLB misses/stmt | cache misses/stmt | branch misses | frontend | backend | retiring | kernel share | bound by |")
    print("|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|")
    for eng, stmt, clients in keys:
        base = f"{eng}-{stmt}-{clients}"
        c, cr = counts(f"{CORE}/{base}-core.csv"), run(f"{CORE}/{base}-core.json")
        m, mr = counts(f"{OTHER}/{base}-mem.csv"), run(f"{OTHER}/{base}-mem.json")
        t = counts(f"{OTHER}/{base}-topdown.csv")
        if not cr or not cr["n"]:
            continue
        n = cr["n"]
        cyc, ins = c.get("cycles", 0) / n, c.get("instructions", 0) / n
        l1 = c.get("l1-dcache-load-misses", 0) / n
        dtlb = m.get("dtlb-load-misses", 0) / mr["n"] if mr and mr["n"] else float("nan")
        llc = m.get("cache-misses", 0) / mr["n"] if mr and mr["n"] else float("nan")
        br = 100 * c.get("branch-misses", 0) / c["branches"] if c.get("branches") else float("nan")
        fe, be, ret = t.get("frontend_bound"), t.get("backend_bound"), t.get("retiring")
        kern = 100 * cr["sys"] / (cr["user"] + cr["sys"]) if cr.get("user") else float("nan")
        bound = "-"
        if fe is not None and be is not None and ret is not None:
            bound = "backend stalls" if be >= max(fe, ret) else ("frontend" if fe >= max(be, ret) else "instructions")
        f = lambda v, d=0: "-" if v is None or v != v else f"{v:,.{d}f}"
        print(f"| {stmt} | {clients} | {eng} | {f(cr['rate'])} | {f(cyc)} | {f(ins)} | {f(ins / cyc if cyc else None, 2)} | {f(l1, 1)} | {f(dtlb, 2)} | {f(llc, 1)} | {f(br, 2)}% | {f(fe, 1)}% | {f(be, 1)}% | {f(ret, 1)}% | {f(kern)}% | {bound} |")


main()
