"""Lane W3 (oltp): one line per suite run, and the A/B's summary.

python3 -I ab_w3.py line <run.json> <label> <load state>
python3 -I ab_w3.py summary <folder of r<round>-<arm>-<op>-<clients>.json and load-<arm>.json>
Rows and commits a flush subtract the arm's load-only run, so they cover a run's warm-up and timed
window alone.
"""
import glob
import json
import os
import re
import statistics
import sys


def read(path):
    r = json.load(open(path))
    xs = r.get("results") or []
    return (xs[0] if xs else {}), (r.get("stats") or {})


def line(path, label, state):
    x, st = read(path)
    ms = x.get("ms") or {}
    print(label, "ops/s", x.get("ops_per_s"), "p50", ms.get("p50"), "p99", ms.get("p99"), "n", ms.get("n"),
          "errors", x.get("errors"), "retries", x.get("retries"), "commits", st.get("commits"),
          "flushes", st.get("flushes"), "rows", st.get("rows"), state)


def spread(v):
    v = [a for a in v if a is not None]
    if not v:
        return "-"
    return f"{statistics.median(v):.3f} ({min(v):.3f} to {max(v):.3f})"


def summary(folder):
    load = {}
    for arm in ("A", "B"):
        p = os.path.join(folder, f"load-{arm}.json")
        if os.path.exists(p):
            load[arm] = read(p)[1]
    runs = {}
    for p in sorted(glob.glob(os.path.join(folder, "r*-*.json"))):
        m = re.match(r"r(\d+)-(A|B|sqlite)-(\w+?)-(\d+)\.json$", os.path.basename(p))
        if m:
            runs.setdefault((m[2], m[3], int(m[4])), []).append((int(m[1]),) + read(p))
    print("load-only runs (rows, commits, flushes):", {a: (s.get("rows"), s.get("commits"), s.get("flushes")) for a, s in load.items()})
    for (arm, op, c), rs in sorted(runs.items(), key=lambda k: (k[0][1], k[0][2], k[0][0])):
        ops = [x.get("ops_per_s") for _, x, _ in rs]
        p50 = [(x.get("ms") or {}).get("p50") for _, x, _ in rs]
        p99 = [(x.get("ms") or {}).get("p99") for _, x, _ in rs]
        errs = sum((x.get("errors") or 0) for _, x, _ in rs)
        retries = sum((x.get("retries") or 0) for _, x, _ in rs)
        per_flush, commits_per_flush = [], []
        base = load.get(arm)
        for _, _, st in rs:
            if base and st.get("flushes") is not None:
                f = st["flushes"] - base.get("flushes", 0)
                if f > 0:
                    per_flush.append((st["rows"] - base.get("rows", 0)) / f)
                    commits_per_flush.append((st["commits"] - base.get("commits", 0)) / f)
        print(f"SUMMARY {op} clients {c} {arm}: runs {len(rs)} ops/s {spread(ops)} p50 ms {spread(p50)} p99 ms {spread(p99)} "
              f"errors {errs} retries {retries} rows/flush {spread(per_flush)} commits/flush {spread(commits_per_flush)}")
    sq = {r: (x.get("ms") or {}).get("p50") for r, x, _ in runs.get(("sqlite", "insert_one", 1), [])}
    for arm in ("A", "B"):
        gaps = [(x.get("ms") or {}).get("p50") - sq[r] for r, x, _ in runs.get((arm, "insert_one", 1), []) if r in sq]
        print(f"GAP insert_one 1 client, {arm} p50 less SQLite's in the same round, ms: {spread(gaps)} {[round(g, 3) for g in gaps]}")


if sys.argv[1] == "line":
    line(sys.argv[2], sys.argv[3], " ".join(sys.argv[4:]))
else:
    summary(sys.argv[2])
