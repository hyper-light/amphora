#!/usr/bin/env python3
"""Lane T20: the three arms' 22 queries at SF1. Per query: whether the plan differs (A against B, B against C), the
star filters each arm's runs checked (the traced pass), and each arm's timed runs (median, range, the runs' load),
with an exact rank-sum p for B against A and C against B. Then, for every query whose plan or filters differ, each
arm's plan and B's priced reads."""
import glob, itertools, json, os, re, statistics, sys

L = sys.argv[1]
ARMS = "ABC"

def details(plan):
    out = []
    def walk(x):
        if isinstance(x, dict):
            if "detail" in x:
                out.append(str(x["detail"]))
            for k, v in x.items():
                if k != "detail":
                    walk(v)
        elif isinstance(x, list):
            for v in x:
                walk(v)
    walk(plan)
    return " / ".join(out) if out else str(plan)[:200]

times, shapes, loads, status, pinned = {}, {}, {}, {}, {}
for f in sorted(glob.glob(os.path.join(L, "out", "[ABC]-r*.jsonl"))):
    arm = os.path.basename(f)[0]
    for line in open(f):
        if not line.strip():
            continue
        r = json.loads(line)
        if r.get("record") == "run":
            pinned.setdefault(arm, set()).add(json.dumps(r.get("pinned_weights"), sort_keys=True))
        if r.get("record") != "query":
            continue
        k = (arm, r["query"])
        shapes.setdefault(k, {}).setdefault(details(r.get("plan")), 0)
        shapes[k][details(r.get("plan"))] += 1
        for x in r.get("runs", []):
            status.setdefault(k, []).append(x.get("status"))
            if x.get("status") == "done":
                times.setdefault(k, []).append(float(x["ms"]))
            loads.setdefault(k, []).append(x.get("load"))
            if "plan" in x:
                d = details(x["plan"])
                shapes[k][d] = shapes[k].get(d, 0) + 1

filters, priced, tshape = {}, {}, {}
for f in sorted(glob.glob(os.path.join(L, "traced", "[ABC]-q*.log"))):
    arm, q = os.path.basename(f)[:-4].split("-", 1)
    fl, pr = [], set()
    for line in open(f, errors="replace"):
        if "T19: the star filters' rows this run" in line:
            m = re.search(r"filters=(\[[^\]]*\])", line)
            p = re.search(r"probes=(\d+)", line)
            rj = re.search(r"rejects=(\d+)", line)
            fl.append((m.group(1) if m else "?", int(p.group(1)) if p else -1, int(rj.group(1)) if rj else -1))
        elif "T19: a read priced with the star filter it gives" in line:
            g = {k: re.search(k + r"=([^ ]+)", line) for k in ("rhs_table", "input_cardinality", "pass", "read_cost", "filtered_cost")}
            pr.add(tuple((k, v.group(1) if v else "?") for k, v in g.items()))
    filters[(arm, q)] = fl
    priced[(arm, q)] = pr
for f in sorted(glob.glob(os.path.join(L, "traced", "[ABC]-q*.jsonl"))):
    arm, q = os.path.basename(f)[:-6].split("-", 1)
    for line in open(f):
        r = json.loads(line) if line.strip() else {}
        if r.get("record") == "query":
            tshape[(arm, q)] = {details(r.get("plan"))} | {details(x.get("plan")) for x in r.get("runs", []) if "plan" in x}

def ranksum_p(a, b):
    """Exact two-sided p of the rank-sum statistic (ties by mid-rank), all splits enumerated."""
    if not a or not b:
        return float("nan")
    pool = sorted(a + b)
    def rank(v):
        idx = [i for i, x in enumerate(pool) if x == v]
        return (idx[0] + idx[-1]) / 2 + 1
    ranks = [rank(v) for v in a + b]
    n = len(a)
    obs = sum(ranks[:n])
    mean = n * (len(pool) + 1) / 2
    combos = list(itertools.combinations(range(len(pool)), n))
    hits = sum(1 for c in combos if abs(sum(ranks[i] for i in c) - mean) >= abs(obs - mean) - 1e-9)
    return hits / len(combos)

def fmt(v):
    return f"{v:,.0f}" if v >= 100 else f"{v:.1f}"

print("pinned weights per arm:", {a: len(v) for a, v in pinned.items()}, "distinct sets:", len(set().union(*pinned.values())) if pinned else 0)
queries = sorted({q for (_, q) in times} | {q for (_, q) in shapes}, key=lambda q: int(q[1:]))
changed = []
print("q   | plans A=B B=C | filters A ; B ; C | A ms med (range) | B | C | p(B vs A) p(C vs B) | load range | failed")
for q in queries:
    sh = {a: set(shapes.get((a, q), {})) for a in ARMS}
    fs = {a: sorted({x[0] for x in filters.get((a, q), [])}) for a in ARMS}
    ab = "same" if sh["A"] == sh["B"] else "DIFF"
    bc = "same" if sh["B"] == sh["C"] else "DIFF"
    cell = []
    for a in ARMS:
        t = times.get((a, q), [])
        cell.append(f"{fmt(statistics.median(t))} ({fmt(min(t))}-{fmt(max(t))}) n{len(t)}" if t else "none")
    pba = ranksum_p(times.get(("B", q), []), times.get(("A", q), []))
    pcb = ranksum_p(times.get(("C", q), []), times.get(("B", q), []))
    ld = [x for a in ARMS for x in loads.get((a, q), []) if isinstance(x, (int, float))]
    bad = sum(1 for a in ARMS for s in status.get((a, q), []) if s != "done")
    unstable = [a for a in ARMS if len(sh[a]) > 1]
    print(f"{q} | {ab} {bc}{' UNSTABLE ' + ''.join(unstable) if unstable else ''} | {' '.join(fs['A']) or '-'} ; {' '.join(fs['B']) or '-'} ; {' '.join(fs['C']) or '-'} | {cell[0]} | {cell[1]} | {cell[2]} | {pba:.3f} {pcb:.3f} | {min(ld):.1f}-{max(ld):.1f} | {bad}" if ld else f"{q} | {ab} {bc} | ... | {cell} | {bad}")
    if ab == "DIFF" or bc == "DIFF" or fs["A"] != fs["B"] or fs["B"] != fs["C"] or unstable:
        changed.append(q)
print()
for q in changed:
    print(f"== {q}")
    for a in ARMS:
        for d, n in sorted(shapes.get((a, q), {}).items(), key=lambda x: -x[1]):
            print(f"  {a} x{n}: {d[:400]}")
        tr = tshape.get((a, q))
        if tr and tr != set(shapes.get((a, q), {})):
            print(f"  {a} traced plan(s): {[d[:300] for d in tr]}")
        for fl in sorted(set(filters.get((a, q), [])))[:4]:
            print(f"  {a} filters {fl[0]} probes={fl[1]} rejects={fl[2]}")
    for a in "BC":
        pr = sorted(priced.get((a, q), set()))
        print(f"  {a} priced reads ({len(pr)} distinct):")
        for p in pr[:10]:
            print("    " + " ".join(f"{k}={v}" for k, v in p))
print()
print("== plans of every query (B; A and C when they differ)")
for q in queries:
    for a in ARMS:
        ds = sorted(shapes.get((a, q), {}).items(), key=lambda x: -x[1])
        if a != "B" and set(shapes.get((a, q), {})) == set(shapes.get(("B", q), {})):
            continue
        for d, n in ds:
            print(f"{q} {a} x{n}: {d[:500]}")
