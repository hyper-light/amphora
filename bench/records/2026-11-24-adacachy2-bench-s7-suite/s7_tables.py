#!/usr/bin/env python3
"""Lane S7 tables: the in-process SQL suite's rounds, Amphora against Turso and SQLite, beside S4's table.

usage: s7_tables.py ROUNDS_DIR S4_RECORDS.jsonl > TABLES.md
  ROUNDS_DIR holds round1.jsonl .. roundN.jsonl (bench/suite.sh records, one line a target a round).
Median (min-max) over rounds. A cell with fewer than 1,000 samples in some round has its p99.9 withheld
(S4's rule: bench/records/2026-11-23-adacachy2-suite-inproc/table.py `tail`).
"""
import glob
import json
import os
import re
import statistics
import sys

TARGETS = ["amphora", "turso", "sqlite"]
OPS_ORDER = ["point_pk", "point_secondary", "point_text", "json_field", "blob_read", "insert_one", "insert_batch_100",
             "update_plain", "update_indexed", "upsert", "delete_insert", "txn_read_write", "table_count",
             "table_group_by", "filter_no_index", "aggregates_range", "join_three"]
MIN_SAMPLES = 1000


def rows_of(path):
    """(op, clients, target) -> one result dict of one target's pass, plus a list of failures."""
    out, failed, passes = {}, [], []
    for line in open(path):
        try:
            r = json.loads(line)
        except ValueError:
            continue
        rep = r.get("report") or {}
        t = rep.get("target") or r.get("target")
        if r.get("failed"):
            failed.append((t, str(r.get("error"))[-300:]))
            continue
        passes.append((t, r.get("load_before"), r.get("load_after")))
        for x in rep.get("results") or []:
            out[(x["op"], x["clients"], t)] = x
    return out, failed, passes


def med(xs):
    return statistics.median(xs)


def f(v, digits=None):
    """A number for the table: ops/s with separators, latencies with three significant digits."""
    if v is None:
        return "-"
    a = abs(v)
    if digits is not None:
        return f"{v:.{digits}f}"
    if a >= 1000:
        return f"{v:,.0f}"
    if a >= 100:
        return f"{v:.0f}"
    if a >= 1:
        return f"{v:.2f}"
    return f"{v:.3f}"


def m1(xs):
    """The median of the values there are, or a dash."""
    xs = [x for x in xs if x is not None]
    return f(med(xs)) if xs else "-"


def mr(xs):
    """median (min-max) of a list."""
    xs = [x for x in xs if x is not None]
    if not xs:
        return "-"
    return f"{f(med(xs))} ({f(min(xs))}-{f(max(xs))})"


class Cells:
    """For each (op, clients, target): the list of per-round (ops_s, p50, p99, p999, n, errors, retries, error)."""

    def __init__(self):
        self.d = {}
        self.failed = []

    def add(self, key, x):
        ms = x.get("ms") or {}
        self.d.setdefault(key, []).append({
            "ops": x.get("ops_per_s"), "p50": ms.get("p50"), "p99": ms.get("p99"),
            "p999": ms.get("p999", ms.get("p99.9")), "n": ms.get("n"), "max": ms.get("max"),
            "errors": x.get("errors") or 0, "retries": x.get("retries") or 0, "error": x.get("error"),
            "unsupported": bool(x.get("unsupported")), "longest": x.get("longest_ms"),
            "rows": x.get("rows_per_op")})


def load_rounds(paths):
    cells, failed, rounds_seen = Cells(), [], []
    for i, p in enumerate(paths, 1):
        rows, fl, passes = rows_of(p)
        rounds_seen.append((os.path.basename(p), passes))
        for t, e in fl:
            failed.append((os.path.basename(p), t, e))
        for k, x in rows.items():
            cells.add(k, x)
    return cells, failed, rounds_seen


def stats_of(runs):
    """One cell's numbers over rounds; None where no round has them."""
    if not runs:
        return None
    ok = [r for r in runs if not r["unsupported"]]
    if not ok:
        return {"unsupported": True}
    ops = [r["ops"] for r in ok if r["ops"] is not None]
    n = [r["n"] or 0 for r in ok]
    return {"unsupported": False, "rounds": len(ok), "ops": ops,
            "p50": [r["p50"] for r in ok], "p99": [r["p99"] for r in ok], "p999": [r["p999"] for r in ok],
            "nmin": min(n) if n else 0, "errors": sum(r["errors"] for r in ok), "retries": sum(r["retries"] for r in ok),
            "err": next((r["error"] for r in ok if r["error"]), None),
            "longest": max([r["longest"] or 0 for r in ok] or [0])}


def opskey(op):
    return OPS_ORDER.index(op) if op in OPS_ORDER else 99


def compare(amph, other):
    """Amphora's ops/s over another's (median over rounds), and how the two ranges relate. The ratio is inf when
    the other answered nothing in its window and 0.0 when Amphora did."""
    a, b = [x for x in amph["ops"] if x is not None], [x for x in other["ops"] if x is not None]
    ratio = float("inf") if med(b) == 0 else med(a) / med(b)
    sep = "ahead" if min(a) > max(b) else "behind" if max(a) < min(b) else "overlap"
    return ratio, sep


def ratio_text(r):
    return "inf (none answered in the window)" if r == float("inf") else f"{r:.2f}x"


def slower_text(r):
    return "Amphora answered none" if r == 0 else "-" if r == float("inf") else f"{1 / r:.1f}x slower" if r < 1 else f"{r:.1f}x faster"


def main():
    rounds_dir, s4_path = sys.argv[1], sys.argv[2]
    paths = sorted(glob.glob(os.path.join(rounds_dir, "round[0-9]*.jsonl")), key=lambda p: int(re.search(r"round(\d+)\.jsonl", p).group(1)))
    if not paths:
        sys.exit(f"no rounds in {rounds_dir}")
    cells, failed, seen = load_rounds(paths)
    s4rows, s4failed, _ = rows_of(s4_path) if False else (None, None, None)
    out = []
    w = out.append
    # ---- header
    w("## 1. Every cell, one row a target (median (min-max) over rounds)")
    w("")
    w(f"Rounds: {len(paths)} ({', '.join(os.path.basename(p) for p in paths)}). Latencies in ms, closed loop, 1 s window after 0.5 s warm-up. "
      f"The p99.9 is withheld when some round has fewer than {MIN_SAMPLES} samples. 'best/this' = the best target's median ops/s over this one's.")
    w("")
    w("| op | clients | target | min samples | ops/s | p50 | p99 | p99.9 | errors | retries | best/this ops/s |")
    w("|---|---|---|---|---|---|---|---|---|---|---|")
    keys = sorted({(op, c) for (op, c, _) in cells.d}, key=lambda k: (opskey(k[0]), k[1]))
    err_list = []
    best_by = {}
    for op, c in keys:
        st = {t: stats_of(cells.d.get((op, c, t))) for t in TARGETS}
        meds = {t: med([x for x in s["ops"] if x is not None]) for t, s in st.items() if s and not s.get("unsupported") and s["ops"]}
        best = max(meds.values()) if meds else 0
        best_by[(op, c)] = st
        for t in TARGETS:
            s = st[t]
            if s is None:
                continue
            if s.get("unsupported"):
                w(f"| {op} | {c} | {t} | | unsupported | | | | | | |")
                continue
            p999 = mr(s["p999"]) if s["nmin"] >= MIN_SAMPLES else f"withheld (min samples {s['nmin']} < {MIN_SAMPLES})"
            bt = f"{best / meds[t]:.2f}" if meds.get(t) else ""
            mark = "" if s["nmin"] >= MIN_SAMPLES else " [<1000]"
            w(f"| {op} | {c} | {t} | {s['nmin']}{mark} | {mr(s['ops'])} | {mr(s['p50'])} | {mr(s['p99'])} | {p999} | {s['errors']} | {s['retries']} | {bt} |")
            if s["errors"]:
                err_list.append((op, c, t, s["errors"], s["retries"], s["err"]))
    # ---- errors
    w("")
    w("### Cells where a target erred or refused")
    w("")
    if not err_list and not failed:
        w("None: no target reported an error, no target failed, in any round.")
    for (name, t, e) in failed:
        w(f"- FAILED {name} {t}: {e!r}")
    for op, c, t, n, r, e in err_list:
        w(f"- {op}, {c} clients, {t}: {n} errors over the rounds (and {r} retries), first: {str(e)[:200]!r}")
    # ---- summary
    w("")
    w("## 2. Amphora against the best SQLite-family competitor")
    w("")
    w("ratio = Amphora's median ops/s over that competitor's. 'ahead' = Amphora's slowest round beats the competitor's fastest; "
      "'behind' = the reverse; 'overlap' = the two ranges of rounds touch, so no measured difference.")
    w("")
    w("| op | clients | Amphora ops/s | Turso ops/s | SQLite ops/s | best competitor | Amphora/best | range verdict | Amphora p99 ms | best p99 ms | samples (Amphora/best, min) |")
    w("|---|---|---|---|---|---|---|---|---|---|---|")
    ratios = []
    for op, c in keys:
        st = best_by[(op, c)]
        a = st["amphora"]
        comp = {t: st[t] for t in ("turso", "sqlite") if st[t] and not st[t].get("unsupported") and st[t]["ops"]}
        if not a or a.get("unsupported") or not a["ops"]:
            w(f"| {op} | {c} | - | | | | Amphora did not answer | | | | |")
            continue
        if not comp:
            w(f"| {op} | {c} | {mr(a['ops'])} | | | none answered | | | | | |")
            continue
        best_t = max(comp, key=lambda t: med([x for x in comp[t]["ops"] if x is not None]))
        ratio, sep = compare(a, comp[best_t])
        ratios.append((ratio, op, c, best_t, sep, a, comp[best_t]))
        v = {"ahead": "ahead", "behind": "behind", "overlap": "overlap (no measured difference)"}[sep]
        w(f"| {op} | {c} | {mr(a['ops'])} | {mr(st['turso']['ops']) if st['turso'] and not st['turso'].get('unsupported') else '-'} | "
          f"{mr(st['sqlite']['ops']) if st['sqlite'] and not st['sqlite'].get('unsupported') else '-'} | {best_t} | {ratio_text(ratio)} | {v} | "
          f"{mr(a['p99'])} | {mr(comp[best_t]['p99'])} | {a['nmin']}/{comp[best_t]['nmin']}{' [<1000]' if min(a['nmin'], comp[best_t]['nmin']) < MIN_SAMPLES else ''} |")
    # ---- counts
    w("")
    ahead = sum(1 for x in ratios if x[4] == "ahead")
    behind = sum(1 for x in ratios if x[4] == "behind")
    over = sum(1 for x in ratios if x[4] == "overlap")
    w(f"Counts over {len(ratios)} cells: ahead {ahead}, behind {behind}, overlapping ranges {over}. "
      f"By median ratio alone: {sum(1 for x in ratios if x[0] > 1)} above 1x, {sum(1 for x in ratios if x[0] < 1)} below.")
    # ---- by family
    w("")
    w("### By family and client count: geometric mean of Amphora/best (cells where either side answered nothing left out, and counted)")
    w("")
    fam = {"point reads": ["point_pk", "point_secondary", "point_text", "json_field", "blob_read"],
           "OLTP writes": ["insert_one", "insert_batch_100", "update_plain", "update_indexed", "upsert", "delete_insert", "txn_read_write"],
           "analytics": ["table_count", "table_group_by", "filter_no_index", "aggregates_range", "join_three"]}
    w("| family | " + " | ".join(f"{c} client{'s' if c != 1 else ''}" for c in sorted({c for _, c in keys})) + " |")
    w("|---" * (1 + len({c for _, c in keys})) + "|")
    for name, ops in fam.items():
        row = []
        for c in sorted({c for _, c in keys}):
            rs = [(r, t) for r, op, cc, t, *_ in ratios if op in ops and cc == c]
            vals = [r for r, _ in rs if 0 < r < float("inf")]
            skip = len(rs) - len(vals)
            import math
            gm = math.exp(sum(math.log(v) for v in vals) / len(vals)) if vals else None
            row.append(f"{gm:.2f}x over {len(vals)} ops" + (f", {skip} left out" if skip else "") if gm is not None else "no cell")
        w(f"| {name} | " + " | ".join(row) + " |")
    # ---- gaps
    w("")
    w("## 3. The ten largest gaps (Amphora furthest behind the best competitor)")
    w("")
    w("| rank | op | clients | Amphora/best | best competitor | Amphora ops/s (median, range) | best ops/s (median, range) | Amphora p50 / p99 ms | best p50 / p99 ms | range verdict | min samples |")
    w("|---|---|---|---|---|---|---|---|---|---|---|")
    for i, (ratio, op, c, t, sep, a, b) in enumerate(sorted(ratios, key=lambda x: x[0])[:10], 1):
        w(f"| {i} | {op} | {c} | {ratio_text(ratio)} ({slower_text(ratio)}) | {t} | {mr(a['ops'])} | {mr(b['ops'])} | {m1(a['p50'])} / {m1(a['p99'])} | {m1(b['p50'])} / {m1(b['p99'])} | {sep} | {min(a['nmin'], b['nmin'])}{' [<1000]' if min(a['nmin'], b['nmin']) < MIN_SAMPLES else ''} |")
    # ---- beside S4's table
    if s4_path and os.path.exists(s4_path):
        s4 = Cells()
        passes = []
        for line in open(s4_path):
            try:
                r = json.loads(line)
            except ValueError:
                continue
            rep = r.get("report") or {}
            t = rep.get("target") or r.get("target")
            if r.get("failed"):
                continue
            passes.append((t, r.get("load_before"), r.get("load_after")))
            for x in rep.get("results") or []:
                s4.add((x["op"], x["clients"], t), x)
        w("")
        w("## 4. Beside S4's table (bench/records/2026-11-23-adacachy2-suite-inproc)")
        w("")
        w("S4's load averages (before/after each target's pass, 1-minute, 32 cores): " +
          "; ".join(f"{t} " + ", ".join(f"{a}/{b}" for tt, a, b in passes if tt == t) for t in TARGETS))
        w("")
        w("Rule (from the brief): now's median inside S4's min-max is 'no measured change'; outside it, the direction and the ratio of the two medians "
          "(now/then). S4's ranges are five noisy rounds, so a cell marked 'changed' is bigger than S4's noise, not necessarily a real change.")
        w("")
        w("| op | clients | target | S4 ops/s (median, range) | now ops/s (median, range) | now/S4 | verdict |")
        w("|---|---|---|---|---|---|---|")
        changed = []
        tally = {}
        for op, c in keys:
            for t in TARGETS:
                a, b = stats_of(s4.d.get((op, c, t))), best_by[(op, c)].get(t)
                if not a or not b or a.get("unsupported") or b.get("unsupported"):
                    continue
                ao = [x for x in a["ops"] if x is not None]
                bo = [x for x in b["ops"] if x is not None]
                if not ao or not bo:
                    continue
                m4, m7 = med(ao), med(bo)
                inside = min(ao) <= m7 <= max(ao)
                ratio = float("inf") if m4 == 0 else m7 / m4
                if inside:
                    v = "no measured change"
                elif m7 > max(ao):
                    v = f"faster than S4's fastest round ({ratio:.2f}x)" if ratio != float("inf") else "S4 answered none; now answers"
                else:
                    v = f"slower than S4's slowest round ({ratio:.2f}x)"
                tally.setdefault(t, {}).setdefault("inside" if inside else ("up" if m7 > max(ao) else "down"), []).append((op, c, ratio))
                r_txt = "inf" if ratio == float("inf") else f"{ratio:.2f}x"
                w(f"| {op} | {c} | {t} | {mr(ao)} | {mr(bo)} | {r_txt} | {v} |")
                if not inside:
                    changed.append((t, op, c, ratio))
        w("")
        for t in TARGETS:
            g = tally.get(t, {})
            w(f"- {t}: {len(g.get('inside', []))} cells no measured change, {len(g.get('up', []))} above S4's range, {len(g.get('down', []))} below.")
    print("\n".join(out))


if __name__ == "__main__":
    main()
