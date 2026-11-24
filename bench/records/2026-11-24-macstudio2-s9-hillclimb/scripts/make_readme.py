#!/usr/bin/env python3
"""Lane S9: assembles README.md from the rounds' numbers and the static text. usage: make_readme.py ROUNDS_DIR HEAD.md STATIC.md TAIL.md > README.md"""
import os, sys
here = os.path.dirname(os.path.abspath(__file__))
src = open(os.path.join(here, "s9_tables.py")).read()
src = src[:src.rindex("\n\nmain()")]
ns = {"__name__": "t", "sys": sys}
exec(compile(src, "s9_tables.py", "exec"), ns)
load, col, med, f, mr, ratio, rt, OPS, CLIENTS, WRITES, verdict = (ns[k] for k in ("load", "col", "med", "f", "mr", "ratio", "rt", "OPS", "CLIENTS", "WRITES", "verdict"))
cells, passes = load(sys.argv[1])
g = lambda op, c, arm, k="ops": col(cells.get((op, c, arm)), k)
nr = {a: len({p["round"] for p in passes if p["arm"] == a}) for a in ("A", "B", "Bd", "C", "S", "T", "Sf", "Tf")}
rows = []
for op in OPS:
    for c in CLIENTS:
        a, b, bd, cc = (g(op, c, x) for x in ("A", "B", "Bd", "C"))
        comp = {t: g(op, c, t) for t in ("S", "T") if g(op, c, t)}
        fair = {t: g(op, c, t) for t in ("Sf", "Tf") if g(op, c, t)}
        best = max(comp, key=lambda t: med(comp[t]), default=None)
        fb = max(fair, key=lambda t: med(fair[t]), default=None)
        wr = op in WRITES
        use, who = (fair.get(fb), fb) if wr else (comp.get(best), best)
        rows.append({"op": op, "c": c, "a": a, "b": b, "bd": bd, "cc": cc, "ab": ratio(a, b), "bd_a": ratio(a, bd), "wr": wr,
                     "who": who, "bu": ratio(use, b) if use else None, "use": use, "au": ratio(use, a) if use else None})
w = print
w(open(sys.argv[2]).read().rstrip("\n") + "\n")
w("## Losses first\n")
los = sorted((r for r in rows if r["ab"] and r["ab"]["v"] == "behind"), key=lambda r: r["ab"]["r"])
w(f"### B slower than A: {len(los)} of {sum(1 for r in rows if r['ab'])} cells have B's every round below A's every round (A and B: {nr['A']} and {nr['B']} rounds)\n")
for r in los:
    w(f"- {r['op']} at {r['c']} clients: {rt(r['ab'])}. A {mr(r['a'])}, B {mr(r['b'])} ops/s, Bd (B with A's defaults, {nr['Bd']} rounds) {mr(r['bd'])}. p50 {mr(g(r['op'], r['c'], 'A', 'p50'))} -> {mr(g(r['op'], r['c'], 'B', 'p50'))} ms, p99 {mr(g(r['op'], r['c'], 'A', 'p99'))} -> {mr(g(r['op'], r['c'], 'B', 'p99'))} ms.")
if not los:
    w("- none")
w(f"\n### B behind the best competitor (reads: SQLite and Turso, {nr['S']} and {nr['T']} rounds; writes: SQLite and Turso with F_FULLFSYNC, {nr['Sf']} round each, so Amphora's own durability)\n")
far = sorted((r for r in rows if r["bu"] and r["bu"]["r"] < 1), key=lambda r: r["bu"]["r"])
for r in far[:20]:
    w(f"- {r['op']} at {r['c']} clients: B is {rt(r['bu'])} of {r['who']} ({10 / r['bu']['r']:.1f}x short of 10x); B {mr(r['b'])}, {r['who']} {mr(r['use'])} ops/s; verdict {r['bu']['v']}.")
if not far:
    w("- none")
w("\n### Cells where B reaches 10x the best competitor of the same durability\n")
ten = [r for r in rows if r["bu"] and r["bu"]["r"] >= 10]
w(f"{len(ten)} of {sum(1 for r in rows if r['bu'])}: " + ", ".join(f"{r["op"]}@{r["c"]} {"none answered" if r["bu"]["r"] == float("inf") else format(r["bu"]["r"], ".1f") + "x"}" for r in sorted(ten, key=lambda r: -r['bu']['r'])) + ".\n")
w("### Memory, a whole pass (ru_maxrss, MiB; median (range))\n")
for arm in ("A", "B", "Bd", "C", "S", "T", "Sf", "Tf"):
    ps = [p for p in passes if p["arm"] == arm]
    if ps:
        w(f"- {arm}: max RSS {mr([p['max_rss_mb'] for p in ps])}, harness heap peak {mr([p['heap_peak_mb'] for p in ps])}, CPU user/sys s {mr([p['cpu_user_s'] for p in ps])} / {mr([p['cpu_sys_s'] for p in ps])}.")
w("")
w(open(sys.argv[3]).read().rstrip("\n") + "\n")
w(open(sys.argv[4]).read().rstrip("\n") + "\n")
