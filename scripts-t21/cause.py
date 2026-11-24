#!/usr/bin/env python3
"""Lane T21: for each query whose plan or filter set differs between arms, the whole plans the search weighed in
each arm's traced run (the three cheapest, their table orders), the reads it priced with a star filter, the filters
the run checked, and the counted run's estimated and actual rows per operator.

Usage: cause.py DIR [q..]. Without queries: every query whose plan differs A/B/C or whose filters differ."""
import glob, json, os, re, sys

L = sys.argv[1]
ONLY = sys.argv[2:]
ARMS = "ABC"

def num(k, l):
    m = re.search(r"\b%s=(-?[0-9.]+(?:e[-+]?[0-9]+)?)" % k, l)
    return float(m.group(1)) if m else None

def txt(k, l):
    m = re.search(r'\b%s="([^"]*)"' % k, l)
    return m.group(1) if m else None

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

def first_prepare(path):
    lines = open(path, errors="replace").read().splitlines()
    starts = [i for i, l in enumerate(lines) if "the weights this plan is made on" in l]
    if not starts:
        return lines, lines
    end = starts[1] if len(starts) > 1 else len(lines)
    return lines[starts[0]:end], lines

def analyze(arm, q):
    path = os.path.join(L, "traced", f"{arm}-{q}.log")
    if not os.path.exists(path):
        return None
    seg, all_lines = first_prepare(path)
    names = []
    for l in seg:
        m = re.search(r'names=(\[[^\]]*\])', l)
        if m and "join search budget" in l:
            names = re.findall(r'"([^"]*)"', m.group(1))
            break
    plans = {}
    for l in seg:
        if "a whole plan weighed" in l or "a whole bushy plan weighed" in l:
            m = re.search(r"data: \[([^\]]*)\]", l)
            if not m:
                continue
            order = tuple(int(x) for x in re.findall(r"\((\d+),", m.group(1)))
            c = num("cost", l)
            if c is None:
                continue
            name = " > ".join(names[i] if i < len(names) else str(i) for i in order)
            b = "bushy" if "bushy" in l else "left"
            k = (b, name)
            if k not in plans or c < plans[k]:
                plans[k] = c
    best = sorted(plans.items(), key=lambda kv: kv[1])
    priced = set()
    for l in seg:
        if "T19: a read priced with the star filter it gives" in l:
            priced.add((txt("rhs_table", l), num("input_cardinality", l), num("pass", l), num("read_cost", l), num("filtered_cost", l)))
    runs = [l for l in all_lines if "T19: the star filters' rows this run" in l]
    filt = sorted({(re.search(r"filters=(\[[^\]]*\])", l).group(1) if re.search(r"filters=(\[[^\]]*\])", l) else "?", num("probes", l), num("rejects", l)) for l in runs}, key=str)
    return best, sorted(priced, key=str), filt

shape, counts = {}, {}
for f in sorted(glob.glob(os.path.join(L, "out", "[ABC]-r*.jsonl"))):
    arm = os.path.basename(f)[0]
    for line in open(f):
        if not line.strip():
            continue
        r = json.loads(line)
        if r.get("record") != "query":
            continue
        q = r["query"]
        shape.setdefault((arm, q), set()).add(details(r.get("plan")))
        counts.setdefault((arm, q), r.get("counts"))
        for x in r.get("runs", []):
            if "plan" in x:
                shape[(arm, q)].add(details(x["plan"]))

queries = sorted({q for (_, q) in shape} | {os.path.basename(p)[2:-4] for p in glob.glob(os.path.join(L, "traced", "A-q*.log"))}, key=lambda q: (int(re.sub(r"\D", "", q) or 0), q))
if ONLY:
    queries = [q for q in queries if q in ONLY]

for q in queries:
    info = {a: analyze(a, q) for a in ARMS}
    same_plan = shape.get(("A", q)) == shape.get(("B", q)) == shape.get(("C", q))
    filts = {a: (info[a][2] if info[a] else None) for a in ARMS}
    fsets = {a: sorted({x[0] for x in (filts[a] or [])}) for a in ARMS}
    fdiff = not (fsets["A"] == fsets["B"] == fsets["C"])
    if not ONLY and same_plan and not fdiff:
        continue
    print(f"== {q} plans A=B: {shape.get(('A', q)) == shape.get(('B', q))} B=C: {shape.get(('B', q)) == shape.get(('C', q))} filters: {fsets}")
    for a in ARMS:
        for d in sorted(shape.get((a, q), [])):
            print(f"  {a} plan: {d[:350]}")
    for a in ARMS:
        if not info[a]:
            print(f"  {a}: no traced log")
            continue
        best, priced, filt = info[a]
        print(f"  {a} whole plans weighed ({len(best)} orders), cheapest:")
        for (b, name), c in best[:4]:
            print(f"    {c:>14.1f}  {b}  {name[:200]}")
        for p in priced[:8]:
            print(f"    priced read: rhs {p[0]} input {p[1]} pass {p[2]} read {p[3]} filtered {p[4]}")
        for f in filt[:4]:
            print(f"    run filters {f[0]} probes {f[1]} rejects {f[2]}")
    for a in ("A", "B"):
        cs = counts.get((a, q)) or []
        print(f"  {a} counted run, est vs actual rows per operator:")
        for c in cs[:14]:
            print(f"    plan {c.get('plan')} {str(c.get('kind'))[:12]:<12} {str(c.get('table'))[:14]:<14} pos {c.get('position')} est_in {c.get('est_input')} est_out {c.get('est_output')} actual {c.get('rows')}")
