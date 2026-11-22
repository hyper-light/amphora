#!/usr/bin/env python3
"""The baselines' numbers from their records (goal 63c98840, bench/baselines/README.md).

usage: python3 -I bench/baselines/check.py <part> <records.jsonl>...
  plan     JOB's planning time on Amphora: each query's least prepare_ms over its runs, the median,
           the 99th percentile (nearest rank) and the largest; exits 1 naming each query over 20 ms
           (the goal's bound) or with no planning time.
  search   JOB's join search time within planning (search_ms), the same figures, and its share of
           planning.
  leader   single-row reads (the suite, one client): each class's p50 and p99 on every engine, the
           leader (the row store with the least p50, Amphora left out) and Amphora's p50 over the
           leader's.
  scan     scans and aggregates per row: the suite's whole-table statements' p50 over the table's
           rows, and TPC-H's queries in the planner bench's records over lineitem's rows, in ns a row,
           each engine beside the others.
Every summary starts with each record's machine, commit and load, and the client and platform it
names, as the records hold them. One engine is set beside another only from the same run (the same
file), the same client and the same platform (Ada's fair comparisons, 2026-11-21): a record that
names no client or platform is shown but set beside nothing.
"""

import json
import sys

PLAN_BOUND_MS = 20.0  # the goal's bound: "every JOB query is planned within 20 ms"
ROW_STORES = {"amphora", "sqlite", "turso", "mvcc", "postgres", "orioledb", "cedardb", "mysql"}
SCANS = ("filter_no_index", "like_substring", "table_count", "table_group_by")


def read(paths):
    for path in paths:
        with open(path) as f:
            for line in f:
                line = line.strip()
                if line:
                    r = json.loads(line)
                    r["_file"] = path
                    yield r


def nearest_rank(values, q):
    """The q-th quantile of `values`, by nearest rank."""
    s = sorted(values)
    if not s:
        return None
    k = max(1, -(-len(s) * q // 100))
    return s[int(k) - 1]


def runs_of(records):
    """{file: (client, platform)} from the planner bench's run records: its query and load records
    take their run's."""
    out = {}
    for r in records:
        if r.get("record") == "run":
            out[r["_file"]] = (r.get("client"), r.get("platform"))
    return out


def group(r, runs):
    """The run, client and platform a record's number came from, or None where it doesn't say them:
    numbers are set beside each other only within one group."""
    client, plat = r.get("client"), r.get("platform")
    if client is None and plat is None:
        client, plat = runs.get(r["_file"], (None, None))
    if not isinstance(client, dict) or not isinstance(plat, dict):
        return None
    return (r["_file"], client.get("program"), client.get("commit"), plat.get("host"), plat.get("os"), plat.get("arch"), plat.get("kernel"),
            plat.get("disk"), plat.get("fs"), plat.get("file_attrs"), plat.get("engine_runs"), plat.get("container") is not None)


def platform_text(plat):
    if not isinstance(plat, dict):
        return "no platform recorded"
    box = f"in container {plat['container']}" if plat.get("container") else "no container"
    return f"{plat.get('engine_runs')}, {box}, {plat.get('os')}/{plat.get('arch')} {plat.get('kernel')}, {plat.get('fs')} on {plat.get('disk')} ({plat.get('data_dir')})"


def client_text(client):
    if not isinstance(client, dict):
        return "no client recorded"
    return f"client {client.get('program')} {client.get('mode') or ''} at {client.get('commit')}".replace("  ", " ")


def where(records):
    """Each run record's machine, commit and load: the planner bench's `run` records and the
    suite's records (bench/run.sh's fields)."""
    out = []
    for r in records:
        if r.get("record") == "run":
            host = r.get("host") or {}
            out.append(f"{host.get('host')} ({host.get('cpu')}, {r.get('cores')} cores), commit {r.get('commit')}, {r.get('dirty_files')} files dirty, max load {r.get('max_load')}; "
                       f"{client_text(r.get('client'))}; {platform_text(r.get('platform'))}")
        elif "report" in r and "host" in r:
            out.append(f"{r.get('host')} ({r.get('cpu')}, {r.get('cores')} cores) {r.get('target') or r['report'].get('target', '')}, commit {r.get('commit')}, load {r.get('load_before')} to {r.get('load_after')}; "
                       f"{client_text(r.get('client'))}; {platform_text(r.get('platform'))}")
    return out


def least(runs, field):
    vals = [r[field] for r in runs if r.get("status") == "done" and isinstance(r.get(field), (int, float))]
    return min(vals) if vals else None


def per_query(records, field):
    """{query: least `field` over its runs} for Amphora's query records, and the queries without."""
    got, missing = {}, []
    for r in records:
        if r.get("record") != "query" or r.get("target") != "amphora":
            continue
        v = least(r.get("runs", []), field)
        if v is None:
            missing.append(r.get("query"))
        else:
            got[r.get("query")] = v
    return got, missing


def figures(values):
    v = list(values)
    return f"{len(v)} queries: median {nearest_rank(v, 50):.3f} ms, p99 {nearest_rank(v, 99):.3f} ms, largest {max(v):.3f} ms"


def plan(records):
    got, missing = per_query(records, "prepare_ms")
    if not got:
        print("check.py plan: no Amphora query records with a planning time")
        return 1
    print(f"planning: {figures(got.values())}")
    over = sorted((q for q, v in got.items() if v > PLAN_BOUND_MS), key=lambda q: -got[q])
    for q in over:
        print(f"over {PLAN_BOUND_MS:g} ms: {q} {got[q]:.3f} ms")
    for q in missing:
        print(f"no planning time: {q}")
    print(f"check.py plan: {len(got) - len(over)} of {len(got) + len(missing)} queries planned within {PLAN_BOUND_MS:g} ms")
    return 1 if over or missing else 0


def search(records):
    got, missing = per_query(records, "search_ms")
    planned, _ = per_query(records, "prepare_ms")
    if not got:
        print("check.py search: no Amphora query records with a search time (search_ms)")
        return 1
    print(f"join search: {figures(got.values())}")
    total_search = sum(got.values())
    total_plan = sum(planned.get(q, 0.0) for q in got)
    if total_plan > 0:
        print(f"join search's share of planning, over every query: {total_search / total_plan:.3f}")
    for q, v in sorted(got.items(), key=lambda kv: -kv[1])[:10]:
        print(f"  {q}: search {v:.3f} ms of planning {planned.get(q, float('nan')):.3f} ms")
    for q in missing:
        print(f"no search time: {q}")
    return 0


def suite_results(records):
    """(record, target, items, result) for every suite record's result."""
    for r in records:
        report = r.get("report")
        if not isinstance(report, dict):
            continue
        for res in report.get("results", []):
            yield r, r.get("target") or report.get("target"), report.get("items"), res


def leader(records):
    runs = runs_of(records)
    by_op = {}
    for r, target, _, res in suite_results(records):
        if res.get("clients") != 1 or res.get("kind") != "Read" or res.get("errors"):
            continue
        ms = res.get("ms") or {}
        if ms.get("p50") is None:
            continue
        by_op.setdefault(res["op"], []).append((target, group(r, runs), ms))
    if not by_op:
        print("check.py leader: no one-client read results")
        return 1
    for op, rows in sorted(by_op.items()):
        print(f"{op}: " + ", ".join(f"{t} p50 {m['p50']:.4f} p99 {m['p99']:.4f} ms" for t, _, m in sorted(rows, key=lambda x: x[0])))
        ours = [(g, m) for t, g, m in rows if t == "amphora"]
        for g, m in ours:
            if g is None:
                print("  Amphora's record names no client or platform: set beside nothing")
                continue
            # The leader: the fastest other row store in Amphora's own run, client and platform.
            rivals = [(t, mm) for t, gg, mm in rows if gg == g and t in ROW_STORES and t != "amphora"]
            if not rivals:
                print("  no other row store ran in Amphora's run on its client and platform: no leader")
                continue
            best, lead = min(rivals, key=lambda x: x[1]["p50"])
            ratio = m["p50"] / lead["p50"] if lead["p50"] > 0 else float("inf")
            print(f"  leader {best}; Amphora's p50 over the leader's: {ratio:.3f}")
        apart = sorted({t for t, gg, _ in rows if t in ROW_STORES and t != "amphora" and all(gg is None or gg != g for g, _ in ours)})
        if apart:
            print(f"  not set beside Amphora (another run, client or platform): {', '.join(apart)}")
    return 0


def scan(records):
    runs = runs_of(records)
    lines = []
    for r, target, items, res in suite_results(records):
        if res.get("clients") != 1 or res.get("op") not in SCANS or not items:
            continue
        p50 = (res.get("ms") or {}).get("p50")
        if p50 is not None:
            lines.append((res["op"], target, p50 * 1e6 / items, f"{items} rows; {platform_text(r.get('platform'))}"))
    lineitem = {}
    for r in records:
        if r.get("record") == "load" and r.get("table") == "lineitem" and r.get("rows"):
            lineitem[r.get("target")] = r["rows"]
    rows = max(lineitem.values(), default=None)
    for r in records:
        if r.get("record") != "query" or r.get("bench") != "tpch" or not rows:
            continue
        best = least(r.get("runs", []), "ms")
        if best is not None:
            plat = runs.get(r["_file"], (None, None))[1]
            lines.append((f"tpch {r.get('query')}", r.get("target"), best * 1e6 / rows, f"lineitem {rows} rows; {platform_text(plat)}"))
    if not lines:
        print("check.py scan: no whole-table results")
        return 1
    for what, target, ns, over in sorted(lines):
        print(f"{what} {target}: {ns:.2f} ns a row ({over})")
    return 0


PARTS = {"plan": plan, "search": search, "leader": leader, "scan": scan}


def main(argv):
    if len(argv) < 2 or argv[0] not in PARTS:
        print(__doc__, file=sys.stderr)
        return 2
    records = list(read(argv[1:]))
    for line in where(records):
        print(f"ran on {line}")
    return PARTS[argv[0]](records)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
