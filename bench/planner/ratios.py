#!/usr/bin/env python3
"""Amphora's time over each competitor's, query by query, losses first, from `amphora-bench --planner`
records (bench lane M, 2026-11-23).

A query and engine's time is the median of its timed runs that finished, leaving out the run that
wrote its answer under --answers (its time holds the writing of its rows). A ratio is the base's time
over the competitor's: above 1 the base is slower by that factor. A run that timed out or failed is
shown as such and left out of the geometric mean, which is over the queries both finished; the
count of those left out is printed beside it.

Ladder records (`--cores`) give, for each core count, the base's and each competitor's median and
the ratio, and the geometric mean of the ratio at that count.

usage: python3 -I bench/planner/ratios.py <records.jsonl> [--base amphora]
"""

import json
import math
import statistics
import sys


def median_ms(runs):
    ms = [r["ms"] for r in runs if r.get("status") == "done" and not r.get("captured")]
    if not ms:
        ms = [r["ms"] for r in runs if r.get("status") == "done"]
    return statistics.median(ms) if ms else None


def why_not(runs):
    for r in runs:
        if r.get("status") != "done":
            return r.get("status", "?") + (": " + r["error"][:60] if r.get("error") else "")
    return "no runs"


def geomean(xs):
    return math.exp(sum(math.log(x) for x in xs) / len(xs)) if xs else None


def fmt(ms):
    return f"{ms:.1f}" if ms is not None else "-"


def table(rows, base, others, title):
    # rows: key -> engine -> (median ms or None, why)
    print(f"## {title}\n")
    print("| query | " + base + " ms | " + " | ".join(f"{o} ms | ratio" for o in others) + " |")
    print("|---|---:|" + "---:|---:|" * len(others))

    def worst(key):
        b = rows[key].get(base, (None, ""))[0]
        if b is None:
            return math.inf
        rs = [b / rows[key][o][0] for o in others if rows[key].get(o, (None,))[0]]
        return max(rs) if rs else 0.0

    for key in sorted(rows, key=worst, reverse=True):
        b, bwhy = rows[key].get(base, (None, "absent"))
        cells = [fmt(b) if b is not None else bwhy]
        for o in others:
            m, why = rows[key].get(o, (None, "absent"))
            cells.append(fmt(m) if m is not None else why)
            cells.append(f"{b / m:.2f}" if b is not None and m else "-")
        print(f"| {key} | " + " | ".join(cells) + " |")
    print()
    for o in others:
        pairs = [(rows[k][base][0], rows[k][o][0]) for k in rows if rows[k].get(base, (None,))[0] and rows[k].get(o, (None,))[0]]
        left = len(rows) - len(pairs)
        g = geomean([b / m for b, m in pairs])
        print(f"- {base} / {o}: geometric mean {g:.2f} over {len(pairs)} queries ({left} left out: a timeout, failure or absence)" if g else f"- {base} / {o}: no query both finished")
    print()


def main(argv):
    if len(argv) < 2:
        sys.exit(__doc__)
    path, base = argv[1], "amphora"
    if "--base" in argv:
        base = argv[argv.index("--base") + 1]
    query, ladder, engines, header = {}, {}, [], None
    with open(path) as f:
        for line in f:
            r = json.loads(line)
            kind = r.get("record")
            if kind == "run":
                header = r
            elif kind == "query":
                m = median_ms(r.get("runs", []))
                query.setdefault(r["query"], {})[r["target"]] = (m, why_not(r.get("runs", [])) if m is None else "")
                if r["target"] not in engines:
                    engines.append(r["target"])
            elif kind == "scaling":
                m = median_ms(r.get("runs", []))
                ladder.setdefault(r["cores"], {}).setdefault(f'{r["query"]} {r["variant"]}', {})[r["target"]] = (m, why_not(r.get("runs", [])) if m is None else "")
                if r["target"] not in engines:
                    engines.append(r["target"])
    if header:
        print(f'commit {header.get("commit", "?")[:10]}, {header.get("bench")}, runs {header.get("runs")}, timeout {header.get("timeout_s")} s, cores {header.get("cores")}\n')
        for e in header.get("engines", []):
            print(f"- {json.dumps(e, sort_keys=True)}")
        print()
    others = [e for e in engines if e != base]
    if query:
        table(query, base, others, f"{base} over each competitor, median of timed runs, losses first")
    for cores in sorted(ladder):
        table(ladder[cores], base, others, f"{base} over each competitor at {cores} cores")


if __name__ == "__main__":
    main(sys.argv)
