"""Lane R1: each RealSums arm's answers at 1, 4, 8 and 16 workers, held bit for bit against Serial at
one worker, against SQLite's own (the bench's sqlite target, same data and statement), and against
bench/real_sums_ref.py's SQLite, PostgreSQL 19 and exact answers. -0.0 and +0.0 differ; NULL, NaN
and an error are values of their own.

usage: python3 -I bench/real_sums_answers.py <out dir of bench/real_sums_ab.sh> [scale factor]
"""
import json
import math
import os
import struct
import sys

OUT = sys.argv[1]
SF = sys.argv[2] if len(sys.argv) > 2 else "1"
ARMS = ("serial", "exact", "roworder")
CORES = (1, 4, 8, 16)


def value(text):
    if text == "\\N" or text == "NULL":
        return None
    try:
        return float(text)
    except ValueError:
        return text


def key(x):
    if x is None:
        return "NULL"
    if isinstance(x, str):
        return x
    if math.isnan(x):
        return "NaN"
    return struct.pack(">d", x).hex()


def ulps(a, b):
    if not all(isinstance(x, float) and math.isfinite(x) for x in (a, b)) or (a < 0) != (b < 0):
        return None
    ia, ib = (struct.unpack(">q", struct.pack(">d", abs(x)))[0] for x in (a, b))
    return abs(ia - ib)


def answer(root, query, engine):
    path = os.path.join(root, "tpch-sf" + SF, query, engine + ".tsv")
    try:
        with open(path) as f:
            lines = f.read().splitlines()
    except FileNotFoundError:
        return None
    if lines and lines[0].startswith("\\!"):
        return [["REFUSED: " + lines[0][2:]]]
    return [[value(v) for v in line.split("\t")] for line in lines]


def show(x):
    if x is None:
        return "NULL"
    if isinstance(x, float):
        return repr(x)
    return str(x)


def diff(a, b):
    """Cells of a that differ from b by bits, and the most ulps among the finite ones."""
    if a is None or b is None:
        return None, None
    if len(a) != len(b) or any(len(x) != len(y) for x, y in zip(a, b)):
        return "shape", None
    n, most = 0, 0
    for ra, rb in zip(a, b):
        for x, y in zip(ra, rb):
            if key(x) != key(y):
                n += 1
                u = ulps(x, y)
                most = max(most, u) if u is not None else most
    return n, most


def refs():
    """ref.json and any later ref-*.json (lane R2's q09 and TPC-H q01, q06), merged."""
    answers, tpch_refs = {}, {}
    for name in sorted(os.listdir(OUT)):
        if name.startswith("ref") and name.endswith(".json"):
            got = json.load(open(os.path.join(OUT, name)))
            answers.update(got.get("answers", {}))
            tpch_refs.update(got.get("tpch", {}))
    return answers, tpch_refs


REF, TPCH_REF = refs()


def vs_ref(a, q, alg):
    """Cells of answer a that differ by bits from the reference's `alg` for TPC-H query q, and the most ulps."""
    groups = TPCH_REF.get("tpch-" + q)
    if a is None or groups is None:
        return None, None
    if len(a) != len(groups):
        return "shape", None
    n, most = 0, 0
    for row, group in zip(a, groups):
        first = len(group["group"])
        for i, cell in enumerate(group["cells"]):
            x, y = row[first + i], value(cell[alg])
            if key(x) != key(y):
                n += 1
                u = ulps(x, y)
                most = max(most, u) if u is not None else most
    return n, most


def hard():
    ref = REF
    root = os.path.join(OUT, "answers-hard")
    print("## Hard cases over lineitem (bench/planner/real_sums/queries), one row each\n")
    print("| query | column | Serial (1 worker) | Exact at 1/4/8/16 | RowOrder at 1/4/8/16 | SQLite 3.53.2 (bench) | SQLite ref (func.c) | PostgreSQL 19 ref (float.c) | exact ref |")
    print("|---|---|---|---|---|---|---|---|---|")
    for q in sorted(ref) or ["q0%d" % i for i in range(1, 10)]:
        base = answer(os.path.join(root, "serial-c1"), q, "amphora")
        lite = answer(os.path.join(root, "sqlite"), q, "sqlite")
        cols = len(base[0]) if base else len(ref.get(q, []))
        for c in range(cols):
            meta = ref.get(q, [{}] * cols)[c] if q in ref else {}
            name = "%s(%s)" % (meta.get("agg", "?"), meta.get("input", "?"))
            b = base[0][c] if base else None
            cells = []
            for arm in ARMS[1:]:
                vals = []
                for n in CORES:
                    a = answer(os.path.join(root, "%s-c%d" % (arm, n)), q, "amphora")
                    vals.append(a[0][c] if a else "missing")
                same = [key(v) == key(b) for v in vals]
                if all(same):
                    cells.append("= Serial at all")
                elif len({key(v) for v in vals}) == 1:
                    cells.append(show(vals[0]) + " at all")
                else:
                    cells.append(", ".join("%d: %s" % (n, show(v)) for n, v in zip(CORES, vals)))
            s = lite[0][c] if lite else "missing"
            refs = [meta.get(k, "?") for k in ("sqlite", "pg", "exact")]
            print("| %s | %s | %s | %s | %s | %s | %s | %s | %s |" % (q, name, show(b) if base else "missing", cells[0], cells[1], show(s), *refs))
    # Serial at every worker count, too.
    for q in sorted(ref):
        base = answer(os.path.join(root, "serial-c1"), q, "amphora")
        for n in CORES[1:]:
            d, _ = diff(answer(os.path.join(root, "serial-c%d" % n), q, "amphora"), base)
            if d:
                print("\nSerial at %d workers differs from Serial at 1 on %s: %s cells" % (n, q, d))


def tpch(sub, queries, cores):
    root = os.path.join(OUT, sub)
    if not os.path.isdir(root):
        return
    print("\n## TPC-H answers (%s): cells differing by bits (most ulps)\n" % sub)
    print("| query | arm | workers | vs Serial at %d | vs SQLite 3.53.2 | vs SQLite ref (func.c) | vs PostgreSQL 19 ref (float.c) | vs exact ref |" % cores[0])
    print("|---|---|---|---|---|---|---|---|")
    for q in queries:
        base = answer(os.path.join(root, "serial-c%d" % cores[0]), q, "amphora")
        lite = answer(os.path.join(root, "sqlite"), q, "sqlite")
        for arm in ARMS:
            for n in cores:
                a = answer(os.path.join(root, "%s-c%d" % (arm, n)), q, "amphora")
                if a is None:
                    print("| %s | %s | %d | missing | | | | |" % (q, arm, n))
                    continue
                d1, u1 = diff(a, base)
                d2, u2 = diff(a, lite)
                rs = ["%s (%s)" % vs_ref(a, q, alg) if ("tpch-" + q) in TPCH_REF else "no ref" for alg in ("sqlite", "pg", "exact")]
                print("| %s | %s | %d | %s | %s | %s | %s | %s |" % (q, arm, n, "%s (%s)" % (d1, u1), "%s (%s)" % (d2, u2), *rs))
        if lite and ("tpch-" + q) in TPCH_REF:
            print("| %s | SQLite 3.53.2 | 1 | | | %s | %s | %s |" % (q, *("%s (%s)" % vs_ref(lite, q, alg) for alg in ("sqlite", "pg", "exact"))))


hard()
tpch("answers-tpch", ["q01", "q06"], CORES)
QB = ["q03", "q05", "q07", "q08", "q09", "q10", "q11", "q14", "q15", "q17", "q19", "q22"]
tpch("answers-b", QB, CORES if os.path.isdir(os.path.join(OUT, "answers-b", "serial-c1")) else (16,))
