#!/usr/bin/env python3
"""Lane T21: the markdown table of the three arms over the 22 queries (from sweep.sh's outputs), and the plans."""
import glob, itertools, json, os, re, statistics, sys
L = sys.argv[1]
ARMS = "ABC"
def details(plan):
    out = []
    def walk(x):
        if isinstance(x, dict):
            if "detail" in x: out.append(str(x["detail"]))
            for k, v in x.items():
                if k != "detail": walk(v)
        elif isinstance(x, list):
            for v in x: walk(v)
    walk(plan)
    return " / ".join(out) if out else str(plan)[:200]
def ranksum_p(a, b):
    if not a or not b: return float("nan")
    pool = sorted(a + b)
    def rank(v):
        idx = [i for i, x in enumerate(pool) if x == v]
        return (idx[0] + idx[-1]) / 2 + 1
    ranks = [rank(v) for v in a + b]
    n = len(a); obs = sum(ranks[:n]); mean = n * (len(pool) + 1) / 2
    combos = list(itertools.combinations(range(len(pool)), n))
    hits = sum(1 for c in combos if abs(sum(ranks[i] for i in c) - mean) >= abs(obs - mean) - 1e-9)
    return hits / len(combos)
times, shapes, loads, bad, counts = {}, {}, {}, {}, {}
for f in sorted(glob.glob(os.path.join(L, "out", "[ABC]-r*.jsonl"))):
    arm = os.path.basename(f)[0]
    for line in open(f):
        if not line.strip(): continue
        r = json.loads(line)
        if r.get("record") != "query": continue
        q = r["query"]; k = (arm, q)
        shapes.setdefault(k, set()).add(details(r.get("plan")))
        counts.setdefault(k, r.get("counts"))
        for x in r.get("runs", []):
            if "plan" in x: shapes[k].add(details(x["plan"]))
            if x.get("status") == "done": times.setdefault(k, []).append(float(x["ms"]))
            else: bad[k] = bad.get(k, 0) + 1
            if isinstance(x.get("load"), (int, float)): loads.setdefault(k, []).append(x["load"])
filters, rej = {}, {}
for f in sorted(glob.glob(os.path.join(L, "traced", "[ABC]-q*.log"))):
    arm, q = os.path.basename(f)[:-4].split("-", 1)
    s = set()
    for line in open(f, errors="replace"):
        if "T19: the star filters' rows this run" in line:
            m = re.search(r"filters=(\[[^\]]*\])", line); s.add(m.group(1) if m else "?")
            p, rj = re.search(r"probes=(\d+)", line), re.search(r"rejects=(\d+)", line)
            rej[(arm, q)] = (int(p.group(1)) if p else -1, int(rj.group(1)) if rj else -1)
    filters[(arm, q)] = sorted(s)
def fmt(v): return f"{v:,.0f}" if v >= 100 else f"{v:.1f}"
def cell(k):
    t = times.get(k, [])
    return f"{fmt(statistics.median(t))} ({fmt(min(t))} to {fmt(max(t))})" if t else "none"
qs = sorted({q for (_, q) in times}, key=lambda q: int(q[1:]))
def filt(a, q):
    s = filters.get((a, q), [])
    r = rej.get((a, q))
    return ", ".join(x.strip("[]").replace('"', "") for x in s) if s else "none"
print("| query | plan A = B | plan B = C | filters A | filters B | filters C | A ms | B ms | C ms | p B vs A | p C vs B | load range |")
print("|---|---|---|---|---|---|---|---|---|---|---|---|")
for q in qs:
    ab = "yes" if shapes.get(("A", q)) == shapes.get(("B", q)) else "NO"
    bc = "yes" if shapes.get(("B", q)) == shapes.get(("C", q)) else "NO"
    pba = ranksum_p(times.get(("B", q), []), times.get(("A", q), []))
    pcb = ranksum_p(times.get(("C", q), []), times.get(("B", q), []))
    ld = loads.get(("A", q), []) + loads.get(("B", q), []) + loads.get(("C", q), [])
    print(f"| {q} | {ab} | {bc} | {filt('A', q)} | {filt('B', q)} | {filt('C', q)} | {cell(('A', q))} | {cell(('B', q))} | {cell(('C', q))} | {pba:.3f} | {pcb:.3f} | {min(ld):.1f} to {max(ld):.1f} |")
print()
print(f"failed runs: {sum(bad.values())} of {sum(len(v) for v in times.values()) + sum(bad.values())}; counted-run rows per operator identical B vs C: {sum(1 for q in qs if counts.get(('B', q)) == counts.get(('C', q)))} of {len(qs)} queries")
print()
print("The plans (B's; A's too where they differ; C's are B's on every query):")
print()
for q in qs:
    for a in "BA":
        for d in sorted(shapes.get((a, q), set())):
            if a == "A" and shapes.get(("A", q)) == shapes.get(("B", q)): continue
            print(f"- {q} {a}: {d}")
