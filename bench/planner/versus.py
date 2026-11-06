#!/usr/bin/env python3
"""A planner change against its parent, from `record.sh`'s passes of each on one machine
(docs/research/planner.md section 9: a change is measured on itself and on its parent). For each
benchmark both have, by `summary.py`'s definitions (q-error, nearest-rank percentiles, each query's
best run and the record `summary.py --best` takes over the passes given):

- the prefixes estimated below one row while counting 1,000 or more, and in how many queries
  (AC-PL.1.1);
- the median q-error at each count of joins and over every prefix, and the largest intermediate
  results' median, p90 and p95 (AC-PL.1.2);
- the first tables' q-error p90 and p95 (AC-PL.2.1, AC-PL.4.2);
- the median planning time (AC-PL.4.3);
- Amphora's time on the change over the parent's, geometric mean over the queries both finished,
  and on how many the change was the faster (AC-PL.1.3, AC-PL.2.3).

Whether the answers are the same is `answers.py`'s check (record.sh's ANSWERS), not this one's: a
change of plan counts other prefixes.

usage: python3 -I bench/planner/versus.py --change <jsonl>... --parent <jsonl>... [--json out.json]
"""

import importlib.util
import json
import math
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("summary", HERE / "summary.py")
summary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(summary)


def load(paths):
    """Each (benchmark, query)'s Amphora record, the fastest over the passes given."""
    queries = {}
    for path in paths:
        label = None
        with open(path) as f:
            for line in f:
                r = json.loads(line)
                if r["record"] == "run":
                    label = r["bench"] + (f" SF {r['sf']:g}" if r.get("sf") else "")
                elif r["record"] == "query" and r["target"] == "amphora":
                    key = (label or r["bench"], r["query"])
                    old = queries.get(key)
                    if old is not None and summary.best(old) is not None and (summary.best(r) is None or summary.best(old) <= summary.best(r)):
                        continue
                    queries[key] = r
    return queries


def measures(queries, bench):
    rows = {q: r for (b, q), r in queries.items() if b == bench}
    pre = {q: summary.prefixes(r)[0] for q, r in rows.items()}
    every = [p for ps in pre.values() for p in ps]
    by_joins = {}
    for p in every:
        by_joins.setdefault(p["joins"], []).append(p["q"])
    largest = [max(ps, key=lambda p: p["act"] * p["calls"])["q"] for ps in pre.values() if ps]
    first = [p["q"] for ps in pre.values() for p in ps if p["plan"] == 0 and p["position"] == 0]
    below = [(q, p) for q, ps in pre.items() for p in ps if p["est"] < 1 and p["act"] >= 1000]
    plan_ms = [summary.best(r, "prepare_ms") for r in rows.values() if summary.best(r, "prepare_ms") is not None]
    return {
        "queries": len(rows),
        "prefixes": len(every),
        "below_one_counting_1000": len(below),
        "below_one_counting_1000_queries": len({q for q, _ in below}),
        "median_q_by_joins": {j: summary.percentile(v, 50) for j, v in sorted(by_joins.items())},
        "median_q_all": summary.percentile([p["q"] for p in every], 50),
        "largest_q": {s: summary.percentile(largest, n) for s, n in (("median", 50), ("p90", 90), ("p95", 95))},
        "first_q": {"p90": summary.percentile(first, 90), "p95": summary.percentile(first, 95)},
        "plan_ms_median": summary.percentile(plan_ms, 50),
        "ms": {q: summary.best(r) for q, r in rows.items()},
    }


def main(argv):
    if "--change" not in argv or "--parent" not in argv:
        print(__doc__, file=sys.stderr)
        return 2
    out = None
    if "--json" in argv:
        i = argv.index("--json")
        out = argv[i + 1]
        argv = argv[:i] + argv[i + 2 :]
    c, p = argv.index("--change"), argv.index("--parent")
    change_paths = argv[c + 1 : p] if c < p else argv[c + 1 :]
    parent_paths = argv[p + 1 : c] if p < c else argv[p + 1 :]
    change, parent = load(change_paths), load(parent_paths)
    report = {}
    for bench in sorted({b for b, _ in change} & {b for b, _ in parent}):
        a, b = measures(change, bench), measures(parent, bench)
        both = [q for q in a["ms"] if a["ms"][q] and b["ms"].get(q)]
        ratio = math.exp(sum(math.log(a["ms"][q] / b["ms"][q]) for q in both) / len(both)) if both else None
        report[bench] = {
            "change": {k: v for k, v in a.items() if k != "ms"},
            "parent": {k: v for k, v in b.items() if k != "ms"},
            "time_over_parent_geomean": ratio,
            "queries_timed": len(both),
            "change_faster": sum(1 for q in both if a["ms"][q] < b["ms"][q]),
        }
        print(f"## {bench}")
        for name in ("queries", "prefixes", "below_one_counting_1000", "below_one_counting_1000_queries", "median_q_all", "plan_ms_median"):
            print(f"{name}: change {a[name]}, parent {b[name]}")
        print(f"median q by joins: change {a['median_q_by_joins']}")
        print(f"                   parent {b['median_q_by_joins']}")
        print(f"largest q: change {a['largest_q']}, parent {b['largest_q']}")
        print(f"first q: change {a['first_q']}, parent {b['first_q']}")
        print(f"time over parent (geomean of {len(both)}): {ratio}, change faster on {report[bench]['change_faster']}")
    if out:
        with open(out, "w") as f:
            json.dump(report, f, indent=1, default=str)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
