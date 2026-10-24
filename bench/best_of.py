#!/usr/bin/env python3
"""Best of N for a matrix of runs (CLAUDE.md §7): for the suite, each cell's (statement, clients,
target) highest throughput across the runs given, with that run's p99; for K0, each target's best
write rate and its lowest write, read and search latencies across its runs. Amphora's place in
every cell, its ratio to the best competitor and to the best row store, losses first. Prints
markdown; the same records always print the same table.

usage: python3 bench/best_of.py --suite runs.jsonl [more.jsonl ...] [--k0 k0.jsonl ...]
"""
import json
import sys
from collections import defaultdict

ROW_STORES = {"turso", "sqlite", "postgres", "orioledb", "mysql"}


def args():
    groups, cur = {"--suite": [], "--k0": []}, None
    for a in sys.argv[1:]:
        if a in groups:
            cur = a
        elif cur:
            groups[cur].append(a)
        else:
            raise SystemExit(__doc__)
    return groups["--suite"], groups["--k0"]


def runs(paths):
    out = []
    for p in paths:
        for line in open(p):
            r = json.loads(line)
            if not r.get("failed"):
                out.append(r)
    return out


def fmt(x):
    return f"{x:,.0f}" if x >= 100 else f"{x:,.1f}"


def header(rs):
    hosts = sorted({(r.get("host"), r.get("cpu"), r.get("cores"), r.get("disk"), r.get("fs"), r.get("kernel")) for r in rs})
    dates = sorted(r.get("date", "") for r in rs)
    loads = sorted(float(str(r.get(k, "0")).split()[0]) for r in rs for k in ("load_before", "load_after") if r.get(k))
    commits = sorted({f"{r.get('commit', '')[:10]}{' +' + str(r.get('dirty_files')) + ' dirty' if r.get('dirty_files') else ''}" for r in rs})
    lines = [f"- host: {h[0]}, {h[1]} ({h[2]} threads), disk {h[3]} ({h[4]}), kernel {h[5]}" for h in hosts]
    lines.append(f"- runs: {len(rs)}, {dates[0]} to {dates[-1]}" if dates else "- runs: 0")
    if loads:
        lines.append(f"- load average around the runs: {loads[0]:.1f} to {loads[-1]:.1f} (not a quiet machine: CLAUDE.md §7 asks under 2)")
    lines.append(f"- commits: {', '.join(commits)}")
    return lines


def suite(rs):
    best = defaultdict(dict)
    passes = defaultdict(int)
    for r in rs:
        rep = r.get("report", r)
        if not rep.get("suite"):
            continue
        passes[rep["target"]] += 1
        for x in rep["results"]:
            if x.get("unsupported") or not x.get("ops_per_s"):
                continue
            cell = best[(x["op"], x["clients"])]
            old = cell.get(rep["target"])
            if old is None or x["ops_per_s"] > old["ops_per_s"]:
                cell[rep["target"]] = {"ops_per_s": x["ops_per_s"], "p99": (x.get("ms") or {}).get("p99")}
    rows = []
    for (op, clients), cell in best.items():
        a, rest = cell.get("amphora"), {t: v for t, v in cell.items() if t != "amphora"}
        if a is None or not rest:
            continue
        top_t, top = max(rest.items(), key=lambda kv: (kv[1]["ops_per_s"], kv[0]))
        row = {t: v for t, v in rest.items() if t in ROW_STORES}
        row_t, rbest = max(row.items(), key=lambda kv: (kv[1]["ops_per_s"], kv[0])) if row else (None, None)
        rows.append((a["ops_per_s"] / top["ops_per_s"], op, clients, a, 1 + sum(v["ops_per_s"] > a["ops_per_s"] for v in rest.values()), 1 + len(rest), top_t, top, (a["ops_per_s"] / rbest["ops_per_s"]) if rbest else None))
    rows.sort(key=lambda r: (r[0], r[1], r[2]))
    out = ["## Suite: best of each cell, losses first", ""]
    out.append("passes per target: " + ", ".join(f"{t} {n}" for t, n in sorted(passes.items())))
    first = sum(r[4] == 1 for r in rows)
    ahead_rows = sum(r[8] is not None and r[8] > 1 for r in rows)
    out += ["", f"{len(rows)} cells: Amphora first in {first}, ahead of every row store in {ahead_rows}.", ""]
    out.append("| statement | clients | Amphora /s | Amphora p99 ms | place | best other | its /s | Amphora / best | Amphora / best row store |")
    out.append("|---|---:|---:|---:|---|---|---:|---:|---:|")
    for ratio, op, clients, a, place, of, top_t, top, vr in rows:
        p99 = f"{a['p99']:.3f}" if a.get("p99") is not None else "-"
        out.append(f"| {op} | {clients} | {fmt(a['ops_per_s'])} | {p99} | {place}/{of} | {top_t} | {fmt(top['ops_per_s'])} | {ratio:.3f} | {vr:.2f} |" if vr is not None else
                   f"| {op} | {clients} | {fmt(a['ops_per_s'])} | {p99} | {place}/{of} | {top_t} | {fmt(top['ops_per_s'])} | {ratio:.3f} | - |")
    return out, passes


def k0(rs):
    best = {}
    for r in rs:
        rep = r.get("report", r)
        t = rep.get("target")
        if not t or "write_events_per_s" not in rep:
            continue
        b = best.setdefault(t, {"n": 0, "writes": 0.0, "write_p99": float("inf"), "write_p999": float("inf"), "read_p99": float("inf"), "search_p50": float("inf"), "rss": float("inf"), "mem": float("inf")})
        b["n"] += 1
        b["writes"] = max(b["writes"], rep["write_events_per_s"])
        b["write_p99"] = min(b["write_p99"], rep["write_ms"]["p99"])
        b["write_p999"] = min(b["write_p999"], rep["write_ms"]["p999"])
        b["read_p99"] = min(b["read_p99"], rep["read_ms"]["p99"])
        b["search_p50"] = min(b["search_p50"], rep["search_ms"]["p50"])
        b["rss"] = min(b["rss"], rep.get("rss_max_mb") or float("inf"))
        # Memory on equal terms: the bench's process (an embedded engine's whole footprint) plus a
        # server's own held memory (anonymous and shared, its container's cgroup), page cache apart.
        b["mem"] = min(b["mem"], (rep.get("rss_max_mb") or 0.0) + (r.get("server_mem_held_peak_mb") or 0.0))
    out = ["## K0: kiln's conversation workload (1,000 sessions, 32 durable writers, 4 readers, 30 s), best of each metric", ""]
    out.append("| target | runs | writes /s | write p99 ms | write p99.9 ms | history read p99 ms | search p50 ms | memory MB (process + server held) |")
    out.append("|---|---:|---:|---:|---:|---:|---:|---:|")
    for t, b in sorted(best.items(), key=lambda kv: -kv[1]["writes"]):
        out.append(f"| {t} | {b['n']} | {fmt(b['writes'])} | {b['write_p99']:.2f} | {b['write_p999']:.2f} | {b['read_p99']:.3f} | {b['search_p50']:.2f} | {b['mem']:.0f} |")
    return out, best


def main():
    sp, kp = args()
    s, k = runs(sp), runs(kp)
    out = ["# Matrix: best of the runs given", ""] + header(s + k) + [""]
    if s:
        lines, _ = suite(s)
        out += lines + [""]
    if k:
        lines, _ = k0(k)
        out += lines + [""]
    print("\n".join(out))


main()
