#!/usr/bin/env python3
"""Lane S9: the compact medians table and the report's headline lists, from s9_tables.py's numbers. usage: s9_compact.py ROUNDS_DIR"""
import importlib.util, os, sys
spec = importlib.util.spec_from_file_location("t", os.path.join(os.path.dirname(os.path.abspath(__file__)), "s9_tables.py"))
src = open(spec.origin).read()
src = src[:src.rindex("\n\nmain()")]
ns = {"__name__": "t", "sys": sys}
exec(compile(src, "s9_tables.py", "exec"), ns)
load, col, med, f, ratio, rt, OPS, CLIENTS, WRITES = (ns[k] for k in ("load", "col", "med", "f", "ratio", "rt", "OPS", "CLIENTS", "WRITES"))
cells, passes = load(sys.argv[1])
g = lambda op, c, arm, k="ops": col(cells.get((op, c, arm)), k)
rounds = {a: len({p["round"] for p in passes if p["arm"] == a}) for a in ("A", "B", "Bd", "C", "S", "T", "Sf", "Tf")}
print("rounds per arm:", rounds)
print("| statement | cl | A | B | B/A (range verdict) | Bd | C | S | T | Sf | Tf | B/best(S,T) | B/best(Sf,Tf) writes |")
print("|---|---|---|---|---|---|---|---|---|---|---|---|---|")
for op in OPS:
    for c in CLIENTS:
        m = {a: med(g(op, c, a)) for a in ("A", "B", "Bd", "C", "S", "T", "Sf", "Tf")}
        ab = ratio(g(op, c, "A"), g(op, c, "B"))
        comp = {t: g(op, c, t) for t in ("S", "T") if g(op, c, t)}
        fair = {t: g(op, c, t) for t in ("Sf", "Tf") if g(op, c, t)}
        bb = max(comp, key=lambda t: med(comp[t]), default=None)
        fb = max(fair, key=lambda t: med(fair[t]), default=None)
        v = {"ahead": "+", "behind": "-", "no measured difference": "="}.get(ab["v"] if ab else "", "?")
        r = "-" if not ab else ("inf" if ab["r"] == float("inf") else f"{ab['r']:.2f}x") + " " + v
        print(f"| {op} | {c} | " + " | ".join(f(m[a]) for a in ("A", "B")) + f" | {r} | " + " | ".join(f(m[a]) for a in ("Bd", "C", "S", "T", "Sf", "Tf")) +
              f" | {'-' if not bb or not med(comp[bb]) else f'{m[chr(66)] / med(comp[bb]):.2f}x {bb}'} | {'-' if not fb or not med(fair[fb]) or op not in WRITES else f'{m[chr(66)] / med(fair[fb]):.2f}x {fb}'} |")
