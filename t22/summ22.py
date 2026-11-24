#!/usr/bin/env python3
"""Lane T22: q10, arm A against arm B, paired by round. A process's value is its 5 timed runs' median (the counted run is
not timed). Per round r, A-r and B-r ran back to back (A first in odd rounds). Reported: each arm's medians of process
medians, range, the pooled and per-arm spread, B/A's per-round ratio, an exact sign test and Wilcoxon's signed rank (both
two-sided, exact by enumeration for n <= 20, otherwise normal), a bootstrap interval of the median ratio, and the same
on user plus system CPU time of the whole process (load noise hits wall time first)."""
import glob, itertools, json, math, os, random, re, statistics, sys

L = sys.argv[1]
proc, cpu, loads, plans, rowsets, status, run_ms, counts = {}, {}, {}, {}, {}, {}, {}, {}
for f in sorted(glob.glob(os.path.join(L, "out", "[AB]-r*.jsonl"))):
    arm, r = re.match(r"([AB])-r(\d+)\.jsonl", os.path.basename(f)).groups()
    r = int(r)
    for line in open(f):
        if not line.strip():
            continue
        rec = json.loads(line)
        if rec.get("record") != "query":
            continue
        ms = [float(x["ms"]) for x in rec["runs"] if x.get("status") == "done"]
        status.setdefault(arm, []).extend(x.get("status") for x in rec["runs"])
        if ms:
            proc[(arm, r)] = statistics.median(ms)
            run_ms[(arm, r)] = ms
        loads[(arm, r)] = [x.get("load") for x in rec["runs"] if isinstance(x.get("load"), (int, float))]
        counts[arm] = [(c.get("kind"), c.get("table"), c.get("est_output"), c.get("rows")) for c in rec.get("counts", [])]
        ds = re.findall(r"'detail': '([^']*)'", str(rec.get("plan")))
        plans.setdefault(arm, {}).setdefault(" / ".join(ds), 0)
        plans[arm][" / ".join(ds)] += 1
for f in glob.glob(os.path.join(L, "run.txt")):
    pass
txt = open(os.path.join(L, "run-ab.txt")).read() if os.path.exists(os.path.join(L, "run-ab.txt")) else ""
for m in re.finditer(r"=== ([AB]) r(\d+) .*?\ncpu_s ([0-9.]+) ([0-9.]+) wall_s ([0-9.]+)", txt):
    cpu[(m.group(1), int(m.group(2)))] = float(m.group(3)) + float(m.group(4))

rounds = sorted({r for (_, r) in proc if ("A", r) in proc and ("B", r) in proc})

def signed_rank(d):
    d = [x for x in d if x != 0]
    n = len(d)
    if n == 0:
        return float("nan")
    order = sorted(range(n), key=lambda i: abs(d[i]))
    rank = [0.0] * n
    i = 0
    while i < n:
        j = i
        while j + 1 < n and abs(abs(d[order[j + 1]]) - abs(d[order[i]])) < 1e-12:
            j += 1
        for k in range(i, j + 1):
            rank[order[k]] = (i + j) / 2 + 1
        i = j + 1
    w = sum(rank[i] for i in range(n) if d[i] > 0)
    mean = sum(rank) / 2
    if n <= 20:
        hits = sum(1 for s in itertools.product((0, 1), repeat=n) if abs(sum(rank[i] for i in range(n) if s[i]) - mean) >= abs(w - mean) - 1e-9)
        return hits / 2 ** n
    var = sum(x * x for x in rank) / 4
    z = (abs(w - mean) - 0.5) / math.sqrt(var)
    return math.erfc(z / math.sqrt(2))

def sign_p(d):
    d = [x for x in d if x != 0]
    n, k = len(d), sum(1 for x in d if x > 0)
    tail = sum(math.comb(n, i) for i in range(0, min(k, n - k) + 1)) / 2 ** n
    return min(1.0, 2 * tail)

def boot(d, reps=4000):
    rng = random.Random(7)
    ms = sorted(statistics.median([rng.choice(d) for _ in d]) for _ in range(reps))
    return ms[int(0.025 * reps)], ms[int(0.975 * reps)]

for name, vals in (("wall, the median of a process's 5 timed runs (ms)", proc), ("CPU of the whole process, user + system (s)", cpu)):
    a = [vals[("A", r)] for r in rounds if ("A", r) in vals and ("B", r) in vals]
    b = [vals[("B", r)] for r in rounds if ("A", r) in vals and ("B", r) in vals]
    if not a:
        continue
    lr = [math.log(y / x) for x, y in zip(a, b)]
    lo, hi = boot(lr)
    sd = statistics.stdev(lr) if len(lr) > 1 else float("nan")
    print(f"{name}: n rounds {len(a)}")
    print(f"  A median {statistics.median(a):.1f} ({min(a):.1f} to {max(a):.1f}); B median {statistics.median(b):.1f} ({min(b):.1f} to {max(b):.1f})")
    print(f"  B/A per round: median ratio {math.exp(statistics.median(lr)):.3f}, bootstrap 95% {math.exp(lo):.3f} to {math.exp(hi):.3f}; sd of ln(B/A) {sd:.3f}; B slower in {sum(1 for x in lr if x > 0)} of {len(lr)}; sign test p {sign_p(lr):.4f}; signed rank p {signed_rank(lr):.4f}")
    print(f"  per-arm sd of ln: A {statistics.stdev(map(math.log, a)):.3f} B {statistics.stdev(map(math.log, b)):.3f}")
    print("  rounds needed for a difference of ln(1467/1157)=0.237 at alpha 0.05 power 0.9: %.1f" % (((1.96 + 1.2816) ** 2) * sd * sd / 0.237 ** 2))
    print("  per round (A, B): " + " ".join(f"{x:.0f}/{y:.0f}" for x, y in zip(a, b)))
print("statuses", {a: {s: v.count(s) for s in set(v)} for a, v in status.items()})
for a in "AB":
    ld = [x for (arm, _), v in loads.items() if arm == a for x in v]
    if ld:
        print(f"load during {a}'s runs: median {statistics.median(ld):.1f}, {min(ld):.1f} to {max(ld):.1f}")
    print(a, "plans", {k[:330]: v for k, v in plans.get(a, {}).items()})
    print(a, "counted rows", counts.get(a))
