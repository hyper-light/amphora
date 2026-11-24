"""Lane R1: the reference answers for bench/planner/real_sums/queries, from lineitem.tbl in row order.

Each input column gets three algorithms, each copied from its source:
  sqlite  sumStep / sumFinalize / avgFinalize / totalFinalize, Kahan-Babushka-Neumaier
          (sqlite.org/src func.c trunk, lines 1925-2100); a NaN result reads NULL.
  pg      PostgreSQL 19.0: sum(float8) folds float8pl from the first input, no initial value
          (pg_aggregate.dat line 72; float.h float8_pl_safe, an infinite result from finite
          inputs is an overflow error); avg(float8) is float8_accum from {0,0,0}, Youngs-Cramer
          with its overflow checks, then Sx / N (float.c 3090-3160, 3254-3270). Inputs as float8.
  exact   the exact sum of the inputs, rounded once (to nearest, ties to even); AVG divides that.

TPC-H q01 and q06 (bench/planner/tpch/queries) get the same three, cell by cell, over the rows
their WHERE keeps, grouped as q01 groups and in q01's ORDER BY.

usage: python3 -I bench/real_sums_ref.py <dir with lineitem.tbl> [query,...] > ref.json
  the queries (q01..q09 of real_sums, tpch-q01, tpch-q06) default to all of them
"""
import json
import math
import sys
from fractions import Fraction

INF = float("inf")


def kbn(values):
    r_sum = r_err = 0.0
    i_sum, cnt, approx = 0, 0, False

    def step(r):
        nonlocal r_sum, r_err
        s = r_sum
        t = s + r
        if abs(s) > abs(r):
            r_err += (s - t) + r
        else:
            r_err += (r - t) + s
        r_sum = t

    def step_int(v):
        if v <= -4503599627370496 or v >= 4503599627370496:
            sm = abs(v) % 16384 * (1 if v >= 0 else -1)
            step(float(v - sm))
            step(float(sm))
        else:
            step(float(v))

    def init(v):
        nonlocal r_sum, r_err
        if v <= -4503599627370496 or v >= 4503599627370496:
            sm = abs(v) % 16384 * (1 if v >= 0 else -1)
            r_sum, r_err = float(v - sm), float(sm)
        else:
            r_sum, r_err = float(v), 0.0

    for v in values:
        cnt += 1
        if not approx:
            if isinstance(v, float):
                init(i_sum)
                approx = True
                step(v)
            else:
                i_sum += v
        elif isinstance(v, int):
            step_int(v)
        else:
            step(v)
    r = r_sum + r_err if math.isfinite(r_err) else r_sum
    null = lambda x: None if math.isnan(x) else x
    return {"sum": null(r), "avg": null(r / cnt), "total": null(r)}


def pg(values):
    state = None
    overflow_sum = False
    n, sx, sxx = 0.0, 0.0, 0.0
    overflow_avg = False
    for v in values:
        v = float(v)
        if state is None:
            state = v
        elif not overflow_sum:
            r = state + v
            if math.isinf(r) and not math.isinf(state) and not math.isinf(v):
                overflow_sum = True
            state = r
        if overflow_avg:
            continue
        old_n, old_sx = n, sx
        n += 1.0
        sx += v
        if old_n > 0.0:
            tmp = v * n - sx
            sxx += tmp * tmp / (n * old_n)
            if math.isinf(sx) or math.isinf(sxx):
                if not math.isinf(old_sx) and not math.isinf(v):
                    overflow_avg = True
                sxx = math.nan
        elif math.isnan(v) or math.isinf(v):
            sxx = math.nan
    return {
        "sum": "ERROR: value out of range: overflow" if overflow_sum else state,
        "avg": "ERROR: value out of range: overflow" if overflow_avg else sx / n,
    }


def exact(values):
    pos = neg = False
    total = Fraction(0)
    scale = 1 << 1074
    acc = 0
    cnt = 0
    for v in values:
        cnt += 1
        if isinstance(v, float) and math.isinf(v):
            pos, neg = pos or v > 0, neg or v < 0
            continue
        if isinstance(v, int):
            acc += v * scale
        else:
            num, den = v.as_integer_ratio()
            acc += num * (scale // den)
    if pos and neg:
        return {"sum": None, "avg": None, "total": None}
    if pos or neg:
        r = INF if pos else -INF
        return {"sum": r, "avg": r, "total": r}
    total = Fraction(acc, scale)
    try:
        r = float(total)
    except OverflowError:
        r = INF if total > 0 else -INF
    if r == 0.0:
        r = 0.0
    return {"sum": r, "avg": r / cnt, "total": r}


def columns(rows):
    ln = lambda r: r[1]
    pick = lambda r, table, other: table.get(ln(r), other)
    return {
        "q01": [("sum", "disc"), ("avg", "disc"), ("total", "disc"), ("sum", "net"), ("avg", "price")],
        "q02": [("sum", "cancel"), ("avg", "cancel"), ("total", "cancel")],
        "q03": [("sum", "tenths"), ("avg", "tenths"), ("total", "tenths")],
        "q04": [("sum", "mixed"), ("avg", "mixed"), ("total", "mixed")],
        "q05": [("sum", "negzero"), ("avg", "negzero"), ("total", "negzero"), ("sum", "zeros")],
        "q06": [("sum", "oneinf"), ("avg", "oneinf"), ("sum", "bothinf"), ("total", "bothinf")],
        "q07": [("sum", "big"), ("avg", "big"), ("total", "big"), ("sum", "bigpairs")],
        "q08": [("sum", "ints"), ("avg", "ints"), ("total", "ints")],
        "q09": [("sum", "kbnloss"), ("avg", "kbnloss"), ("total", "kbnloss")],
    }, {
        "disc": lambda r: r[3],
        "net": lambda r: r[2] * (1 - r[3]),
        "price": lambda r: r[2],
        "cancel": lambda r: pick(r, {1: 1e16, 2: -1e16}, 1.0),
        "tenths": lambda r: pick(r, {1: 0.1, 2: 0.2, 3: -0.3}, 0.0),
        "mixed": lambda r: pick(r, {1: 1e300, 2: 1e-300, 3: -1e300}, 5e-324),
        "negzero": lambda r: -0.0,
        "zeros": lambda r: pick(r, {1: 0.0}, -0.0),
        "oneinf": lambda r: INF if (r[0] == 1 and r[1] == 1) else r[3],
        "bothinf": lambda r: pick(r, {1: INF, 2: -INF}, 1.0),
        "big": lambda r: 1e308,
        "bigpairs": lambda r: pick(r, {1: 1e308, 2: 1e308, 3: -1e308, 4: -1e308}, 0.0),
        "ints": lambda r: pick(r, {1: 9007199254740993}, 0.5),
        "kbnloss": lambda r: {1: 1e36, 1500000: 1e18, 3000000: 1.0, 4500000: -1e36, 6000000: -1e18}.get(r[0], 0.0) if r[1] == 1 else 0.0,
    }


def tpch(rows, only):
    """TPC-H q01 and q06 as bench/planner/tpch/queries states them, each REAL aggregate three ways."""
    out = {}
    if "tpch-q01" in only:
        groups = {}
        for r in rows:
            if r[7] <= "1998-09-02":
                groups.setdefault((r[5], r[6]), []).append(r)
        cells = []
        for g in sorted(groups):
            rs = groups[g]
            net = [r[2] * (1 - r[3]) for r in rs]
            charge = [r[2] * (1 - r[3]) * (1 + r[4]) for r in rs]
            row = []
            for agg, values in (("sum", [r[8] for r in rs]), ("sum", [r[2] for r in rs]), ("sum", net), ("sum", charge), ("avg", [r[8] for r in rs]), ("avg", [r[2] for r in rs]), ("avg", [r[3] for r in rs])):
                row.append({alg: text(fn(values).get(agg, "n/a")) for alg, fn in (("sqlite", kbn), ("pg", pg), ("exact", exact))})
            cells.append({"group": list(g), "count": len(rs), "cells": row})
        out["tpch-q01"] = cells
    if "tpch-q06" in only:
        values = [r[2] * r[3] for r in rows if "1994-01-01" <= r[7] < "1995-01-01" and 0.05 <= r[3] <= 0.07 and r[8] < 24]
        out["tpch-q06"] = [{"group": [], "count": len(values), "cells": [{alg: text(fn(values)["sum"]) for alg, fn in (("sqlite", kbn), ("pg", pg), ("exact", exact))}]}]
    return out


def text(v):
    if v is None:
        return "NULL"
    if isinstance(v, str):
        return v
    return repr(v)


def main():
    rows = []
    with open(sys.argv[1] + "/lineitem.tbl") as f:
        for line in f:
            p = line.split("|")
            rows.append((int(p[0]), int(p[3]), float(p[5]), float(p[6]), float(p[7]), p[8], p[9], p[10], float(p[4])))
    queries, inputs = columns(rows)
    only = set(sys.argv[2].split(",")) if len(sys.argv) > 2 else set(queries) | {"tpch-q01", "tpch-q06"}
    queries = {q: cols for q, cols in queries.items() if q in only}
    done = {}
    for name in {inp for cols in queries.values() for _, inp in cols}:
        values = [inputs[name](r) for r in rows]
        done[name] = {"sqlite": kbn(values), "pg": pg(values), "exact": exact(values), "inputs other than 0.0": sum(1 for v in values if v != 0.0)}
    out = {}
    for q, cols in queries.items():
        out[q] = [dict({alg: text(done[inp][alg].get(agg, "n/a")) for alg in ("sqlite", "pg", "exact")}, agg=agg, input=inp, nonzero=done[inp]["inputs other than 0.0"]) for agg, inp in cols]
    json.dump({"rows": len(rows), "answers": out, "tpch": tpch(rows, only)}, sys.stdout, indent=1)


main()
