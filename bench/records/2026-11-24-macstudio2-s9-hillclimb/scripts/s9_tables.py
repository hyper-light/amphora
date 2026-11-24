#!/usr/bin/env python3
"""Lane S9's tables: base (A, 8fc85651) against the hillclimb (B, b5dd793c), with B on A's defaults (Bd), main (C), and
SQLite (S) and Turso (T) as the suite builds them; Sf and Tf are the same two with every fsync() made F_FULLFSYNC, as
Amphora's own flush on macOS asks (writes only).

usage: s9_tables.py ROUNDS_DIR > TABLES.md   (ROUNDS_DIR holds round1.jsonl .. roundN.jsonl from rounds.py)

Per cell: median (min-max) over rounds of ops/s, p50, p99, p99.9. Verdicts: "ahead"/"behind" when the two arms' ranges of
rounds don't touch (with 5 against 5 that is an exact two-sided rank-sum p of 0.0079), else "no measured difference".
Intervals: 95% percentile bootstrap of the ratio of medians (10,000 resamples of rounds, seed 20261124).
A p99.9 is withheld when some round has fewer than 1,000 samples (S4's rule)."""
import glob, json, os, random, re, statistics, sys
OPS = ["point_pk", "point_secondary", "point_text", "json_field", "blob_read", "insert_one", "insert_batch_100", "update_plain",
       "update_indexed", "upsert", "delete_insert", "txn_read_write", "table_count", "table_group_by", "filter_no_index",
       "aggregates_range", "join_three"]
WRITES = {"insert_one", "insert_batch_100", "update_plain", "update_indexed", "upsert", "delete_insert", "txn_read_write"}
CLIENTS = [1, 32, 128]
ARMS = ["A", "B", "Bd", "C", "S", "T", "Sf", "Tf"]
MIN_SAMPLES = 1000


def med(xs):
    return statistics.median(xs) if xs else None


def f(v):
    if v is None:
        return "-"
    a = abs(v)
    if a >= 1000:
        return f"{v:,.0f}"
    if a >= 100:
        return f"{v:.0f}"
    if a >= 1:
        return f"{v:.2f}"
    return f"{v:.3f}"


def mr(xs):
    xs = [x for x in xs if x is not None]
    return f"{f(med(xs))} ({f(min(xs))}-{f(max(xs))})" if xs else "-"


def load(d):
    cells, passes = {}, []
    for p in sorted(glob.glob(os.path.join(d, "round[0-9]*.jsonl")), key=lambda p: int(re.search(r"round(\d+)", p).group(1))):
        for line in open(p):
            try:
                r = json.loads(line)
            except ValueError:
                continue
            rep = r.get("report") or {}
            passes.append({k: r.get(k) for k in ("arm", "round", "position", "date", "took_s", "rc", "load_before", "load_after", "others_before", "others_after",
                                                 "max_rss_mb", "cpu_user_s", "cpu_sys_s", "failed", "error", "stderr_tail")} |
                          {k: rep.get(k) for k in ("load_s", "index_s", "settle_s", "heap_peak_mb", "heap_end_mb", "rss_max_mb")})
            for x in rep.get("results") or []:
                ms = x.get("ms") or {}
                cells.setdefault((x["op"], x["clients"], r["arm"]), []).append({
                    "round": r["round"], "ops": x.get("ops_per_s"), "p50": ms.get("p50"), "p99": ms.get("p99"), "p999": ms.get("p999"), "n": ms.get("n") or 0,
                    "max": ms.get("max"), "errors": x.get("errors") or 0, "retries": x.get("retries") or 0, "error": x.get("error"),
                    "rows": x.get("rows_per_op"), "unsupported": bool(x.get("unsupported")), "longest": x.get("longest_ms"), "cpu_u": x.get("cpu_user_s"), "cpu_s": x.get("cpu_sys_s")})
    return cells, passes


def col(runs, k):
    return [r[k] for r in (runs or []) if not r["unsupported"] and r[k] is not None]


def verdict(a, b):
    """b against a on ops/s (higher is better): how the two ranges of rounds sit."""
    if not a or not b:
        return "-"
    return "ahead" if min(b) > max(a) else "behind" if max(b) < min(a) else "no measured difference"


def boot(a, b, n=10000):
    rng = random.Random(20261124)
    out = []
    for _ in range(n):
        x = med([rng.choice(a) for _ in a])
        y = med([rng.choice(b) for _ in b])
        if x:
            out.append(y / x)
    out.sort()
    return (out[int(0.025 * len(out))], out[int(0.975 * len(out))]) if out else (None, None)


def ratio(a, b):
    """The median of b's ops over a's, the 95% interval and the verdict."""
    if not a or not b or (not med(a) and not med(b)):
        return None
    if not med(a):
        return {"r": float("inf"), "lo": float("inf"), "hi": float("inf"), "v": verdict(a, b), "none": "none answered in the window"}
    lo, hi = boot(a, b)
    return {"r": med(b) / med(a), "lo": lo, "hi": hi, "v": verdict(a, b)}


def rt(r):
    if r is None:
        return "-"
    if r["r"] == float("inf"):
        return "inf (the other answered none in its window)"
    return f"{r['r']:.2f}x [{r['lo']:.2f}, {r['hi']:.2f}]"


def main():
    cells, passes = load(sys.argv[1])
    rounds = sorted({p["round"] for p in passes})
    w = print
    get = lambda op, c, arm: cells.get((op, c, arm))
    nr = {arm: len({p["round"] for p in passes if p["arm"] == arm}) for arm in ARMS}
    w("# S9: the hillclimb (B, b5dd793c) against its base (A, 8fc85651), main (C) and SQLite (S) and Turso (T)\n")
    w(f"Rounds {rounds}; passes by arm {nr}. Latencies in ms (the harness's own resolution is 1 us), closed loop, 1 s window after 0.5 s warm-up.\n")
    # ---- 1. ops table
    w("## 1. Median ops/s (min-max over rounds)\n")
    w("| statement | clients | A base | B hillclimb | Bd: B, A's defaults | C main | S SQLite | T Turso | Sf SQLite+F_FULLFSYNC | Tf Turso+F_FULLFSYNC |")
    w("|---|---|---|---|---|---|---|---|---|---|")
    for op in OPS:
        for c in CLIENTS:
            w(f"| {op} | {c} | " + " | ".join(mr(col(get(op, c, a), 'ops')) for a in ARMS) + " |")
    # ---- 2. comparisons
    w("\n## 2. B over A, and against the best competitor (ratio of median ops/s [95% bootstrap interval])\n")
    w("'best' is the faster of S and T by median ops/s. For writes on macOS S and T call fsync() without F_FULLFSYNC, so the write rows are also given against Sf and Tf (best of the two), which is the fair comparison; the S/T one isn't.\n")
    w("| statement | clients | B/A | verdict | Bd/A | C/A | C/B | best (S,T) | A/best | B/best | 10x short (B) | fair best (Sf,Tf) | A/fair | B/fair |")
    w("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    rows = []
    for op in OPS:
        for c in CLIENTS:
            a, b, bd, cc = (col(get(op, c, x), "ops") for x in ("A", "B", "Bd", "C"))
            comp = {t: col(get(op, c, t), "ops") for t in ("S", "T")}
            fair = {t: col(get(op, c, t), "ops") for t in ("Sf", "Tf")}
            best = max((t for t in comp if comp[t]), key=lambda t: med(comp[t]), default=None)
            fb = max((t for t in fair if fair[t]), key=lambda t: med(fair[t]), default=None)
            ab = ratio(a, b)
            row = {"op": op, "c": c, "ab": ab, "bd": ratio(a, bd), "ca": ratio(a, cc), "cb": ratio(b, cc), "best": best,
                   "a_best": ratio(comp[best], a) if best else None, "b_best": ratio(comp[best], b) if best else None,
                   "fair": fb, "a_fair": ratio(fair[fb], a) if fb else None, "b_fair": ratio(fair[fb], b) if fb else None,
                   "a": a, "b": b}
            rows.append(row)
            short = "" if not row["b_best"] else "none answered" if row["b_best"]["r"] == 0 else f"{10 / row['b_best']['r']:.1f}x" if row["b_best"]["r"] < 10 else "met"
            w(f"| {op} | {c} | {rt(ab)} | {ab['v'] if ab else '-'} | {rt(row['bd'])} | {rt(row['ca'])} | {rt(row['cb'])} | {best or '-'} | {rt(row['a_best'])} | {rt(row['b_best'])} | {short} | {fb or '-'} | {rt(row['a_fair'])} | {rt(row['b_fair'])} |")
    # ---- 3. latency for A and B
    w("\n## 3. Latency, ms: median (min-max over rounds) of p50, p99 and p99.9 (withheld under 1,000 samples in some round)\n")
    w("| statement | clients | p50 A | p50 B | p50 best | p99 A | p99 B | p99 best | p99.9 A | p99.9 B | p99.9 best | samples min A/B |")
    w("|---|---|---|---|---|---|---|---|---|---|---|---|")
    for row in rows:
        op, c = row["op"], row["c"]
        g = lambda arm, k: col(get(op, c, arm), k) if arm else []
        mins = lambda arm: min([r["n"] for r in get(op, c, arm) or []] or [0])
        t999 = lambda arm: "-" if not arm else (mr(g(arm, "p999")) if mins(arm) >= MIN_SAMPLES else "withheld")
        w(f"| {op} | {c} | {mr(g('A', 'p50'))} | {mr(g('B', 'p50'))} | {mr(g(row['best'], 'p50'))} | {mr(g('A', 'p99'))} | {mr(g('B', 'p99'))} | {mr(g(row['best'], 'p99'))} | "
          f"{t999('A')} | {t999('B')} | {t999(row['best'])} | {mins('A')}/{mins('B')} |")
    # ---- 4. rankings
    w("\n## 4. Rankings\n")
    ranked = [r for r in rows if r["ab"]]
    ahead = sorted((r for r in ranked if r["ab"]["v"] == "ahead"), key=lambda r: -r["ab"]["r"])
    behind = sorted((r for r in ranked if r["ab"]["v"] == "behind"), key=lambda r: r["ab"]["r"])
    same = [r for r in ranked if r["ab"]["v"] == "no measured difference"]
    w(f"{len(ahead)} cells B ahead of A, {len(behind)} cells B behind A, {len(same)} cells no measured difference (ranges of rounds touch), of {len(ranked)}.\n")
    w("### The ten biggest gains (B over A, ranges apart)\n")
    for r in ahead[:10]:
        w(f"- {r['op']} at {r['c']}: {rt(r['ab'])}, A {mr(r['a'])} -> B {mr(r['b'])} ops/s; p50 {mr(col(get(r['op'], r['c'], 'A'), 'p50'))} -> {mr(col(get(r['op'], r['c'], 'B'), 'p50'))} ms")
    w("\n### The ten biggest regressions (B slower than A, ranges apart)\n")
    for r in behind[:10]:
        w(f"- {r['op']} at {r['c']}: {rt(r['ab'])}, A {mr(r['a'])} -> B {mr(r['b'])} ops/s; p50 {mr(col(get(r['op'], r['c'], 'A'), 'p50'))} -> {mr(col(get(r['op'], r['c'], 'B'), 'p50'))} ms; p99 {mr(col(get(r['op'], r['c'], 'A'), 'p99'))} -> {mr(col(get(r['op'], r['c'], 'B'), 'p99'))} ms")
    if not behind:
        w("- none")
    w("\n### B below A by median with touching ranges (not a claim)\n")
    for r in sorted((r for r in same if r["ab"]["r"] < 0.97), key=lambda r: r["ab"]["r"])[:10]:
        w(f"- {r['op']} at {r['c']}: {rt(r['ab'])}, A {mr(r['a'])}, B {mr(r['b'])}")
    w("\n### The ten cells where B is furthest behind the best competitor (reads against S and T; writes against Sf and Tf, same durability)\n")
    far = []
    for r in rows:
        use = r["b_fair"] if r["op"] in WRITES else r["b_best"]
        who = r["fair"] if r["op"] in WRITES else r["best"]
        if use:
            far.append((use["r"], r, use, who))
    for x, r, use, who in sorted(far, key=lambda t: t[0])[:10]:
        w(f"- {r['op']} at {r['c']}: B is {rt(use)} of {who} ({'B answered none' if not x else f'{10 / x:.1f}x short of 10x'}); B {mr(r['b'])} against {mr(col(get(r['op'], r['c'], who), 'ops'))} ops/s; verdict {use['v']}")
    # ---- 5. errors
    w("\n## 5. Errors, retries, failed passes\n")
    bad = 0
    for (op, c, arm), runs in sorted(cells.items()):
        e, t = sum(r["errors"] for r in runs), sum(r["retries"] for r in runs)
        if e or t:
            bad += 1
            w(f"- {op} at {c}, {arm}: errors {e}, retries {t}; first {next((r['error'] for r in runs if r['error']), None)!r}")
    for p in passes:
        if p["failed"] or p["rc"]:
            bad += 1
            w(f"- pass {p['arm']} round {p['round']} rc {p['rc']}: {p['error']!r} {p['stderr_tail']!r}")
    w("\nRows an operation answered (rows_per_op), arm against SQLite (S), cells where the medians differ by more than 1%:")
    diff_rows = 0
    for op in OPS:
        for c in CLIENTS:
            ref = med(col(get(op, c, "S"), "rows"))
            for arm in ("A", "B", "Bd", "C", "T"):
                v = med(col(get(op, c, arm), "rows"))
                if not med(col(get(op, c, arm), "ops")) or not med(col(get(op, c, "S"), "ops")):
                    continue
                if v is not None and ref is not None and abs(v - ref) > 0.01 * max(1.0, abs(ref)):
                    diff_rows += 1
                    w(f"- {op} at {c}: {arm} {v} against S {ref}")
    if not diff_rows:
        w("- none")
    if not bad:
        w("\nNone: 0 errors, 0 retries and no failed pass in any round.")
    # ---- 6. memory
    w("\n## 6. Memory, per pass (a whole pass: load, answers, 17 statements up to 128 clients)\n")
    w("| arm | passes | max RSS MiB (ru_maxrss) | heap peak MiB (harness's allocator) | heap at end MiB | load_s | index_s | settle_s | CPU user s | CPU sys s |")
    w("|---|---|---|---|---|---|---|---|---|---|")
    for arm in ARMS:
        ps = [p for p in passes if p["arm"] == arm]
        if ps:
            g = lambda k: [p[k] for p in ps if p[k] is not None]
            w(f"| {arm} | {len(ps)} | " + " | ".join(mr(g(k)) for k in ("max_rss_mb", "heap_peak_mb", "heap_end_mb", "load_s", "index_s", "settle_s", "cpu_user_s", "cpu_sys_s")) + " |")
    # ---- 7. conditions
    w("\n## 7. Conditions per pass (load 1-min before -> after; other processes' CPU, percent of one core)\n")
    w("| round | position | arm | took s | load before -> after | others before | others after | biggest other before |")
    w("|---|---|---|---|---|---|---|---|")
    for p in passes:
        ob, oa = p["others_before"] or {}, p["others_after"] or {}
        w(f"| {p['round']} | {p['position']} | {p['arm']} | {p['took_s']} | {p['load_before'][0]} -> {p['load_after'][0]} | {ob.get('sum')} | {oa.get('sum')} | {(ob.get('top') or [['', '']])[0]} |")

    # ---- 8. conditions per round
    w("\n## 8. Conditions per round (macOS: load average, CPU idle over one second, free memory, other processes' CPU)\n")
    w("| round | when | t | load 1/5/15 | CPU user/sys/idle % | memory free % | vm pressure level | swap | thermal | other processes' CPU sum (% of a core), biggest |")
    w("|---|---|---|---|---|---|---|---|---|---|")
    logp = os.path.join(sys.argv[1], "rounds.log")
    for line in open(logp) if os.path.exists(logp) else []:
        r = json.loads(line)
        m = r["mem"]
        w(f"| {r['round']} ({','.join(r['order'])}) | {r['label']} | {r['t']} | {'/'.join(str(x) for x in r['load'])} | {r['cpu_user_sys_idle']} | {m['free_pct']} | {m['vm_pressure_level']} | {m['swap']} | {r['therm'][:80]} | {r['others']['sum']}: {r['others']['top'][:3]} |")
    ph = os.path.join(sys.argv[1], "phases.log")
    if os.path.exists(ph):
        sm = [json.loads(l) for l in open(ph)]
        ls, os_ = [x["load"][0] for x in sm], [x["others"]["sum"] for x in sm]
        w(f"\nSampler (every 5 s through the rounds, {len(sm)} samples): load 1-min median {med(ls):.1f} (range {min(ls):.1f}-{max(ls):.1f}); other processes' CPU (sum over the machine, % of one core) median {med(os_):.0f} (range {min(os_):.0f}-{max(os_):.0f}).")
        over = [x for x in sm if x["others"]["sum"] > 200]
        w(f"Samples with other processes over 200% of a core: {len(over)}" + (": " + ", ".join(f"{x['t']} {x['others']['sum']}% {x['others']['top'][:2]}" for x in over[:8]) if over else "") + ".")

main()
