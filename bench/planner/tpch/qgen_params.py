"""TPC-H's substitution parameters, as qgen draws them, bound to the prepared templates (PL.12).

`bench/planner/tpch/prepared/qNN.sql` is each of TPC-H's 22 queries with its substitution
parameters as `?N`. This runs qgen (electrum/tpch-dbgen, the revision `dbgen.sh` builds) with `-l`,
which logs each query's parameters as it draws them (one line a query: its number, then each
parameter, tab-separated), once for each seed, and turns each logged line into the values the
template binds: the parameter itself, or what TPC-H 3.0.1 §2.4 makes of it (a date plus its
interval, a LIKE pattern around a word, a discount's range). Each output line is JSON:
`{"query": "q06", "seed": 3, "params": ["1994-01-01", "1995-01-01", 0.05, 0.07, 24]}`.

usage: python3 -I bench/planner/tpch/qgen_params.py <dbgen dir> <scale factor> <first seed> <seeds> <out.jsonl>
"""

import datetime
import json
import os
import subprocess
import sys
import tempfile
from decimal import Decimal


def months(date, n):
    """`date` plus `n` months: every date TPC-H adds months to is a month's first day."""
    d = datetime.date.fromisoformat(date)
    m = d.month - 1 + n
    return d.replace(year=d.year + m // 12, month=m % 12 + 1).isoformat()


def days(date, n):
    return (datetime.date.fromisoformat(date) + datetime.timedelta(days=n)).isoformat()


def number(text):
    """An integer parameter as an int, a decimal one as a float."""
    return int(text) if text.lstrip("-").isdigit() else float(text)


def bound(q, p):
    """The values query `q`'s template binds, in `?N` order, from qgen's parameters `p` (TPC-H
    3.0.1 §2.4.q.3, each query's substitution parameters, and §2.4.q.2's text around them)."""
    if q == 1:
        return [days("1998-12-01", -int(p[0]))]
    if q == 2:
        return [int(p[0]), "%" + p[1], p[2]]
    if q == 3:
        return [p[0], p[1]]
    if q == 4:
        return [p[0], months(p[0], 3)]
    if q == 5:
        return [p[0], p[1], months(p[1], 12)]
    if q == 6:
        # The discount's range is ±0.01 around it, kept to its two places.
        d = Decimal(p[1])
        return [p[0], months(p[0], 12), float(d - Decimal("0.01")), float(d + Decimal("0.01")), number(p[2])]
    if q in (7, 8, 17):
        return list(p)
    if q == 9:
        return ["%" + p[0] + "%"]
    if q == 10:
        return [p[0], months(p[0], 3)]
    if q == 11:
        return [p[0], float(p[1])]
    if q == 12:
        return [p[0], p[1], p[2], months(p[2], 12)]
    if q == 13:
        return ["%" + p[0] + "%" + p[1] + "%"]
    if q == 14:
        return [p[0], months(p[0], 1)]
    if q == 15:
        return [p[0], months(p[0], 3)]
    if q == 16:
        return [p[0], p[1] + "%"] + [int(s) for s in p[2:10]]
    if q == 18:
        return [number(p[0])]
    if q == 19:
        return p[0:3] + [number(x) for x in p[3:6]]
    if q == 20:
        return [p[0] + "%", p[1], months(p[1], 12), p[2]]
    if q == 21:
        return [p[0]]
    if q == 22:
        return list(p[0:7])
    raise ValueError(f"qgen logged query {q}: TPC-H has 22")


# The number of values each template binds, checked against what `bound` makes.
ARITY = {1: 1, 2: 3, 3: 2, 4: 2, 5: 3, 6: 5, 7: 2, 8: 3, 9: 1, 10: 2, 11: 2, 12: 4, 13: 1, 14: 2,
         15: 2, 16: 10, 17: 2, 18: 1, 19: 6, 20: 4, 21: 1, 22: 7}


def parse(log):
    """qgen's `-l` log: each line's query number and its parameters."""
    out = []
    for line in log.splitlines():
        if not line.strip():
            continue
        fields = line.split("\t")
        out.append((int(fields[0]), fields[1:]))
    return out


def draw(dbgen, sf, seed):
    """qgen's parameters for the 22 queries under `seed`, as (query, bound values)."""
    with tempfile.TemporaryDirectory(dir=os.path.expanduser("~/.cache")) as tmp:
        log = os.path.join(tmp, "params.log")
        env = dict(os.environ, DSS_QUERY=os.path.join(dbgen, "queries"), DSS_CONFIG=dbgen)
        subprocess.run([os.path.join(dbgen, "qgen"), "-s", str(sf), "-r", str(seed), "-l", log]
                       + [str(n) for n in range(1, 23)],
                       cwd=dbgen, env=env, check=True, stdout=subprocess.DEVNULL)
        with open(log) as f:
            logged = parse(f.read())
    rows = []
    for q, p in logged:
        values = bound(q, p)
        if len(values) != ARITY[q]:
            raise ValueError(f"q{q:02} seed {seed}: {len(values)} values from {p}, its template binds {ARITY[q]}")
        rows.append((q, values))
    return rows


def main(argv):
    if len(argv) != 6:
        sys.exit(__doc__)
    dbgen, sf, first, seeds, out = argv[1], argv[2], int(argv[3]), int(argv[4]), argv[5]
    with open(out, "w") as f:
        for seed in range(first, first + seeds):
            for q, values in draw(dbgen, sf, seed):
                f.write(json.dumps({"query": f"q{q:02}", "seed": seed, "params": values}) + "\n")


if __name__ == "__main__":
    main(sys.argv)
