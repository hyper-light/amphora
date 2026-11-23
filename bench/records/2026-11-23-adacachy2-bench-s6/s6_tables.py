import glob, json, os, re, statistics, sys

# Lane S6: tables per query type from one run's outputs. usage: s6_tables.py OUT_DIR
O = sys.argv[1]
AMPH = ("amphora", "amphora-procs-persisted", "amphora-procs-replicated")

phases = []
for line in open(os.path.join(O, "phases.log")):
    m = re.match(r"(\S+) (.*?) load=(\S+) (\S+) (\S+)", line)
    if m:
        phases.append((m.group(1), m.group(2), float(m.group(3))))


def load_range(start, end):
    xs = [l for t, what, l in phases if start <= t <= end]
    return f"{min(xs):.0f}-{max(xs):.0f}" if xs else "?"


def phase_window(name):
    s = [t for t, what, l in phases if what == f"{name}-start"]
    e = [t for t, what, l in phases if what.startswith(f"{name}-end")]
    return (s[0] if s else "", e[0] if e else "9999")


def f(x):
    if x is None:
        return "-"
    return f"{x:,.0f}" if abs(x) >= 100 else (f"{x:,.2f}" if abs(x) >= 1 else f"{x:.3f}")


def ratio(a_speed, c_speed):
    if not a_speed or not c_speed:
        return "-"
    r = a_speed / c_speed
    return f"{r:.2f}x faster" if r >= 1 else f"{1 / r:.2f}x slower"


failures, notes = [], []

# Replicated commits.
print("## Replicated commits (bench/repl_ladder.sh, 3 members on one host, writers 1/32/128)\n")
s, e = phase_window("ladder")
print(f"Load (1-min average) over the ladder: {load_range(s, e)} on 32 cores.\n")
recs = {}
for p in glob.glob(os.path.join(O, "repl", "*-commit.jsonl")):
    for line in open(p):
        try:
            r = json.loads(line)
        except ValueError:
            continue
        recs[(r.get("system"), r.get("writers"))] = r
print("| writers | competitor | column | competitor p50 / p99 ms | competitor commits/s | Amphora persisted p50 / p99 ms | Amphora persisted commits/s | Amphora against it (commits/s) | Amphora replicated commits/s | refused |")
print("|---|---|---|---|---|---|---|---|---|---|")
for w in sorted({k[1] for k in recs if k[1] is not None}):
    a = recs.get(("amphora-procs-persisted", w)) or {}
    ar = recs.get(("amphora-procs-replicated", w)) or {}
    al = a.get("latency_ms") or {}
    for (sysname, ww), r in sorted(recs.items(), key=lambda kv: str(kv[0][0])):
        if ww != w or sysname in AMPH:
            continue
        l = r.get("latency_ms") or {}
        col = (r.get("about") or {}).get("column") or ""
        ref = "; ".join(str(x.get("refused"))[:80] for x in (a, ar, r) if x.get("refused"))
        print(f"| {w} | {sysname} | {col} | {f(l.get('p50'))} / {f(l.get('p99'))} | {f(r.get('writes_per_s'))} | {f(al.get('p50'))} / {f(al.get('p99'))} | {f(a.get('writes_per_s'))} | {ratio(a.get('writes_per_s'), r.get('writes_per_s'))} | {f(ar.get('writes_per_s'))} | {ref} |")
    for x in (a, ar):
        if x.get("refused") or (x and not x.get("writes_per_s")):
            failures.append(f"replicated commits: {x.get('system')} at {w} writers: {str(x.get('refused') or 'no commits')[:200]}")
print()
for line in open(os.path.join(O, "repl.err")) if os.path.exists(os.path.join(O, "repl.err")) else []:
    if line.startswith(("FAILED", "refused")):
        failures.append(f"replicated commits ladder: {line.strip()[:200]}")

# The suite.
suite = {}
loads = {}
for name in ("suite-inproc.jsonl", "suite-pg.jsonl"):
    p = os.path.join(O, name)
    if not os.path.exists(p):
        notes.append(f"{name}: no file")
        continue
    for line in open(p):
        try:
            r = json.loads(line)
        except ValueError:
            continue
        if r.get("failed"):
            failures.append(f"suite target {r.get('target')} failed: {str(r.get('error'))[-300:]!r}")
            continue
        rep = r.get("report") or {}
        t = rep.get("target")
        loads[t] = (r.get("load_before"), r.get("load_after"))
        for x in rep.get("results") or []:
            suite[(x.get("op"), x.get("clients"), t)] = x

KINDS = [
    ("Point reads", ["point_pk", "point_secondary", "point_text", "json_field", "blob_read"]),
    ("OLTP writes", ["insert_one", "insert_batch_100", "update_plain", "update_indexed", "upsert", "delete_insert", "txn_read_write"]),
    ("Analytics in the suite (scans, aggregates, joins)", ["table_count", "table_group_by", "filter_no_index", "aggregates_range", "join_three"]),
]
targets = sorted({k[2] for k in suite})
comps = [t for t in targets if t != "amphora"]
print("Load (1-min average) before/after each suite target's pass: " + "; ".join(f"{t}: {a}/{b}" for t, (a, b) in sorted(loads.items())) + "\n")
for title, ops in KINDS:
    print(f"## {title} (bench/suite.sh, 200,000 items, 1 s window after 0.5 s warm-up)\n")
    print("| statement | clients | competitor | competitor p50 / p99 ms | competitor ops/s | Amphora p50 / p99 ms | Amphora ops/s | Amphora against it (ops/s) | rows/op Amphora vs it | load before/after (Amphora; it) |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for op in ops:
        for c in sorted({k[1] for k in suite if k[0] == op}):
            a = suite.get((op, c, "amphora"))
            am = (a or {}).get("ms") or {}
            for t in comps:
                x = suite.get((op, c, t))
                if not x:
                    continue
                xm = x.get("ms") or {}
                ra, rx = (a or {}).get("rows_per_op"), x.get("rows_per_op")
                la, lx = loads.get("amphora", (None, None)), loads.get(t, (None, None))
                print(f"| {op} | {c} | {t} | {f(xm.get('p50'))} / {f(xm.get('p99'))} | {f(x.get('ops_per_s'))} | {f(am.get('p50'))} / {f(am.get('p99'))} | {f((a or {}).get('ops_per_s'))} | {ratio((a or {}).get('ops_per_s'), x.get('ops_per_s'))} | {f(ra)} vs {f(rx)} | {la[0]}/{la[1]}; {lx[0]}/{lx[1]} |")
                if t == "sqlite" and ra is not None and rx is not None and op not in ("insert_one", "insert_batch_100") and abs(ra - rx) > 0.01 * max(abs(rx), 1):
                    failures.append(f"suite {op} at {c} clients: Amphora returned {ra:.3f} rows/op, SQLite {rx:.3f} (possible wrong answer)")
            if a and (a.get("errors") or a.get("error")):
                failures.append(f"suite {op} at {c} clients: Amphora {a.get('errors')} errors: {str(a.get('error'))[:200]!r}")
            if a is None and suite:
                failures.append(f"suite {op} at {c} clients: no Amphora result")
    print()

# TPC-H.
print("## Analytics: TPC-H SF1, 22 queries, one thread each (amphora-bench --planner)\n")
s, e = phase_window("tpch")
print(f"Load (1-min average) over the TPC-H pass: {load_range(s, e)} on 32 cores. Each query: the counted warm-up, then up to 3 timed runs; p50 = median of the timed runs, p99 column = the slowest of them (3 runs carry no p99).\n")
q = {}
p = os.path.join(O, "tpch.jsonl")
if os.path.exists(p):
    for line in open(p):
        try:
            r = json.loads(line)
        except ValueError:
            continue
        if r.get("record") != "query":
            continue
        t = r.get("target") or r.get("engine")
        runs = r.get("runs") or []
        ok = [x.get("ms") for x in runs if x.get("status") == "done" and x.get("ms") is not None]
        bad = [x.get("status") for x in runs if x.get("status") != "done"]
        cnt = (r.get("counted") or {}).get("status")
        q[(r.get("query"), t)] = (ok, bad, cnt, [x.get("load") for x in runs])
        if t == "amphora" and (bad or cnt not in ("done", None)):
            failures.append(f"TPC-H {r.get('query')}: Amphora runs {bad or ''} counted {cnt}")
print("| query | competitor | competitor ms p50 / slowest | Amphora ms p50 / slowest | Amphora against it | load at Amphora's runs |")
print("|---|---|---|---|---|---|")
for qq in sorted({k[0] for k in q}):
    a = q.get((qq, "amphora"))
    for t in ("sqlite", "duckdb"):
        x = q.get((qq, t))
        if not x:
            continue
        am = statistics.median(a[0]) if a and a[0] else None
        xm = statistics.median(x[0]) if x[0] else None
        lo = [l for l in (a[3] if a else []) if isinstance(l, (int, float))]
        print(f"| {qq} | {t} | {f(xm)} / {f(max(x[0]) if x[0] else None)} | {f(am)} / {f(max(a[0]) if a and a[0] else None)} | {ratio(1 / am if am else None, 1 / xm if xm else None)} | {f(min(lo)) if lo else '-'}-{f(max(lo)) if lo else '-'} |")
print()
ap = os.path.join(O, "tpch-answers.txt")
if os.path.exists(ap):
    txt = open(ap).read().strip().splitlines()
    print("answers.py (last lines):\n\n```\n" + "\n".join(txt[-12:]) + "\n```\n")

print("## Failures seen\n")
for x in failures:
    print(f"- {x}")
for x in notes:
    print(f"- note: {x}")
