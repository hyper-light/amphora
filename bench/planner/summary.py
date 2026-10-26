#!/usr/bin/env python3
"""The planner bench's records (`amphora-bench --planner`, bench/planner/README.md) summarized:
for each query, Amphora's estimated and actual rows at every join prefix and their q-error, the
q-error of its largest intermediate result (Han et al., PVLDB 15(4), 2021: q-errors over all
sub-plans mislead, the largest results' matter), its planning time, and every engine's run time,
best of its runs; then the q-error's median, 90th and 95th percentile and maximum by the number
of joins in the prefix. Writes Markdown to stdout and, with --json, the same numbers as JSON.

q-error is max(est / act, act / est) with both taken as at least 1 row (Moerkotte, Neumann and
Steidl, PVLDB 2(1), 2009). A prefix's actual rows are its counter's rows over its query's loops
(a correlated subquery's calls), as its estimate is a call's.

usage: bench/planner/summary.py <records.jsonl>... [--json out.json]
"""

import json
import math
import sys
from collections import defaultdict


def q_error(est, act):
    est = max(est, 1.0)
    act = max(act, 1.0)
    return max(est / act, act / est)


def percentile(values, p):
    """The nearest-rank percentile: the smallest value at least p% of the values are at or under."""
    if not values:
        return None
    ordered = sorted(values)
    rank = max(1, math.ceil(p / 100 * len(ordered)))
    return ordered[rank - 1]


def prefixes(record):
    """Each plan's join prefixes: (plan, position, table, estimate, actual) and the prefixes the
    counters could not give, with why."""
    counts = record.get("counts") or []
    loops = {c["plan"]: c["rows"] for c in counts if c["kind"] == "loops"}
    by_plan = defaultdict(dict)
    builds = defaultdict(dict)
    out, missing = [], []
    for c in counts:
        if c["kind"] != "loops" and c["position"] is None:
            missing.append({"plan": c["plan"], "position": None, "why": f"no estimate for {c['table']}'s {c['kind']}"})
        if c["position"] is None:
            continue
        if c["kind"] == "prefix":
            by_plan[c["plan"]][c["position"]] = c
        elif c["kind"] == "hash_build":
            builds[c["plan"]][c["position"]] = c
    for plan in sorted(set(by_plan) | set(builds)):
        positions = set(by_plan[plan]) | set(builds[plan])
        calls = loops.get(plan, 0)
        for position in range(max(positions) + 1):
            c = by_plan[plan].get(position)
            if c is None and position == 0 and 0 in builds[plan]:
                # The first table hashed: its rows in the hash table are the prefix's.
                c = builds[plan][0]
                actual = c["rows"]
            elif c is None:
                why = "a hash join's build side past the first table: the prefix through it is not produced" if position in builds[plan] else "no counter"
                missing.append({"plan": plan, "position": position, "why": why})
                continue
            elif calls == 0:
                missing.append({"plan": plan, "position": position, "why": "its query never ran"})
                continue
            else:
                actual = c["rows"] / calls
            out.append({"plan": plan, "position": position, "joins": position, "table": c["table"],
                        "est": c["est_output"], "act": actual, "calls": calls,
                        "q": q_error(c["est_output"], actual)})
    return out, missing


def best(record, key="ms"):
    times = [r[key] for r in record.get("runs", []) if r.get("status") == "done"]
    return min(times) if times else None


def status(record):
    for r in record.get("runs", []):
        if r.get("status") != "done":
            return r.get("status") + (": " + r["error"] if r.get("error") else "")
    return "done"


def norm(first):
    """A first row for comparing engines: numbers to 2 places, text trimmed."""
    if first is None:
        return None
    cells = []
    for cell in first.split(" | "):
        cell = cell.strip().strip('"')
        for wrap in ("Decimal(", "Double(", "Int(", "BigInt(", "HugeInt(", "Float(", "Text(", "Date32(", "Text(\""):
            if cell.startswith(wrap) and cell.endswith(")"):
                cell = cell[len(wrap):-1].strip('"')
        try:
            cells.append(f"{float(cell):.2f}")
        except ValueError:
            cells.append(cell)
    return " | ".join(cells)


def main(argv):
    paths = [a for a in argv if not a.startswith("--")]
    json_out = argv[argv.index("--json") + 1] if "--json" in argv else None
    if json_out in paths:
        paths.remove(json_out)
    runs, queries = [], {}
    for path in paths:
        with open(path) as f:
            label = None
            for line in f:
                r = json.loads(line)
                if r["record"] == "run":
                    runs.append(r)
                    # TPC-H's scale factor is the run's: its queries are named by it.
                    label = r["bench"] + (f" SF {r['sf']:g}" if r.get("sf") else "")
                elif r["record"] == "query":
                    # The last record of a query and engine is its record.
                    queries[(label or r["bench"], r["query"], r["target"])] = r
    benches = sorted({k[0] for k in queries})
    targets = sorted({k[2] for k in queries}, key=lambda t: ["amphora", "sqlite", "duckdb", "duckdb-mt", "postgres"].index(t) if t in ["amphora", "sqlite", "duckdb", "duckdb-mt", "postgres"] else 9)
    summary = {"runs": runs, "benches": {}}
    for r in runs:
        h = r["host"]
        print(f"- {r['date']} {h['host']}: {h['cpu']}, {r['cores']} logical cores, {h['mem_gib']} GiB, {h['disk']} ({h['fs']}), {h['kernel']}; "
              f"commit {r['commit']} ({r['dirty_files']} files dirty); {r['bench']}" + (f" SF {r['sf']}" if r.get("sf") else "")
              + f", best of {r['runs']}, each run begun at load under {r['max_load']} ({h['load_kind']})")
    for bench in benches:
        names = sorted({k[1] for k in queries if k[0] == bench}, key=lambda n: (int("".join(c for c in n if c.isdigit()) or 0), n))
        by_joins = defaultdict(list)
        largest = []
        rows, complete, refusals, disagreements, eqp_mismatches = [], 0, [], [], []
        for name in names:
            a = queries.get((bench, name, "amphora"))
            line = {"query": name}
            if a is not None:
                pre, missing = prefixes(a)
                counted = a.get("counted", {})
                if counted.get("status") != "done":
                    refusals.append({"query": name, "why": f"the counted run {counted.get('status')}: {counted.get('error', '')}".strip(": ")})
                elif missing:
                    refusals.append({"query": name, "why": "; ".join(f"plan {m['plan']} position {m['position']}: {m['why']}" for m in missing)})
                elif pre:
                    complete += 1
                for p in pre:
                    by_joins[p["joins"]].append(p["q"])
                tables = defaultdict(int)
                for p in pre:
                    tables[p["plan"]] = max(tables[p["plan"]], p["position"] + 1)
                eqp = {n["op"]["estimate"]["output_rows"] for n in (a.get("plan") or {}).get("nodes", []) if n.get("op", {}).get("estimate")}
                unmatched = [c for c in a.get("counts", []) if c["est_output"] is not None and c["est_output"] not in eqp]
                if unmatched:
                    eqp_mismatches.append({"query": name, "counters": len(unmatched)})
                big = max(pre, key=lambda p: p["act"] * p["calls"], default=None)
                if big is not None:
                    largest.append(big["q"])
                line.update({
                    "tables": max(tables.values(), default=0),
                    "prefixes": [{k: p[k] for k in ("plan", "position", "table", "est", "act", "q")} for p in pre],
                    "missing": missing,
                    "max_q": max((p["q"] for p in pre), default=None),
                    "largest_q": big["q"] if big else None,
                    "largest": {k: big[k] for k in ("plan", "position", "table", "est", "act")} if big else None,
                    "plan_ms": best(a, "prepare_ms"),
                })
            line["ms"] = {t: best(queries[(bench, name, t)]) for t in targets if (bench, name, t) in queries}
            line["status"] = {t: status(queries[(bench, name, t)]) for t in targets if (bench, name, t) in queries}
            results = {}
            for t in targets:
                r = queries.get((bench, name, t))
                done = [x for x in (r or {}).get("runs", []) if x.get("status") == "done"]
                if done:
                    results[t] = (done[0]["rows"], norm(done[0].get("first")))
            if len({v[0] for v in results.values()}) > 1:
                disagreements.append({"query": name, "rows": {t: v[0] for t, v in results.items()}})
            rows.append(line)
        dist = {}
        for joins in sorted(by_joins):
            qs = by_joins[joins]
            dist[joins] = {"prefixes": len(qs), "median": percentile(qs, 50), "p90": percentile(qs, 90), "p95": percentile(qs, 95), "max": max(qs)}
        every = [q for qs in by_joins.values() for q in qs]
        summary["benches"][bench] = {"queries": len(names), "complete": complete, "refusals": refusals, "q_error_by_joins": dist,
                                     "q_error_all": {"prefixes": len(every), "median": percentile(every, 50), "p90": percentile(every, 90), "p95": percentile(every, 95), "max": max(every, default=None)},
                                     "largest_intermediate_q_error": {"queries": len(largest), "median": percentile(largest, 50), "p90": percentile(largest, 90), "p95": percentile(largest, 95), "max": max(largest, default=None)},
                                     "row_count_disagreements": disagreements, "estimates_not_in_explain": eqp_mismatches, "per_query": rows}
        print(f"\n## {bench}: {len(names)} queries, {complete} with estimated and actual rows at every join prefix, {len(refusals)} refused\n")
        for r in refusals:
            print(f"- refused {r['query']}: {r['why']}")
        for m in eqp_mismatches:
            print(f"- {m['query']}: {m['counters']} counted estimates not in EXPLAIN QUERY PLAN FORMAT=JSON's")
        for d in disagreements:
            print(f"- engines disagree on {d['query']}'s rows: {d['rows']}")
        print("\n| joins in the prefix | prefixes | median q-error | p90 | p95 | max |\n|---:|---:|---:|---:|---:|---:|")
        fmt = lambda v: "" if v is None else (f"{v:.3g}" if v < 1e4 else f"{v:.3e}")
        for joins, d in dist.items():
            print(f"| {joins} | {d['prefixes']} | {fmt(d['median'])} | {fmt(d['p90'])} | {fmt(d['p95'])} | {fmt(d['max'])} |")
        a = summary["benches"][bench]["q_error_all"]
        print(f"| all | {a['prefixes']} | {fmt(a['median'])} | {fmt(a['p90'])} | {fmt(a['p95'])} | {fmt(a['max'])} |")
        l = summary["benches"][bench]["largest_intermediate_q_error"]
        print(f"\nThe largest intermediate result's q-error, a query each ({l['queries']}): median {fmt(l['median'])}, p90 {fmt(l['p90'])}, p95 {fmt(l['p95'])}, max {fmt(l['max'])}.\n")
        head = " | ".join(f"{t} ms" for t in targets)
        print(f"| query | tables | max q-error | largest result's q-error | Amphora plan ms | {head} |\n|---|---:|---:|---:|---:|" + "---:|" * len(targets))
        for line in rows:
            cells = " | ".join(fmt(line["ms"].get(t)) if line["ms"].get(t) is not None else line["status"].get(t, "")[:40] for t in targets)
            print(f"| {line['query']} | {line.get('tables', '')} | {fmt(line.get('max_q'))} | {fmt(line.get('largest_q'))} | {fmt(line.get('plan_ms'))} | {cells} |")
        totals = {t: sum(l["ms"][t] for l in rows if l["ms"].get(t) is not None) for t in targets}
        done = {t: sum(1 for l in rows if l["ms"].get(t) is not None) for t in targets}
        print("| all (sum of those run) | | | | | " + " | ".join(f"{totals[t]:.0f} ({done[t]} run)" for t in targets) + " |")
        # Each engine against Amphora, on the queries both ran to the end: the geometric mean of
        # its time over Amphora's, and on how many it was the faster.
        versus = {}
        for t in targets:
            pairs = [(l["ms"]["amphora"], l["ms"][t]) for l in rows if l["ms"].get("amphora") and l["ms"].get(t)]
            if t == "amphora" or not pairs:
                continue
            versus[t] = {"queries": len(pairs), "geomean_over_amphora": math.exp(sum(math.log(b / a) for a, b in pairs) / len(pairs)),
                         "faster_than_amphora": sum(1 for a, b in pairs if b < a)}
        summary["benches"][bench]["versus_amphora"] = versus
        print("\n| engine | queries both ran | its time over Amphora's (geometric mean) | queries it was faster on |\n|---|---:|---:|---:|")
        for t, v in versus.items():
            print(f"| {t} | {v['queries']} | {v['geomean_over_amphora']:.3f} | {v['faster_than_amphora']} |")
    if json_out:
        with open(json_out, "w") as f:
            json.dump(summary, f, indent=1)


if __name__ == "__main__":
    main(sys.argv[1:])
