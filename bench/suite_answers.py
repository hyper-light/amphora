#!/usr/bin/env python3
"""The suite's answer check: every value of every read's answer in each target's record
(src/suite.rs `answers`, written by suite.sh), held to the reference target's (SQLite's by default).

Values are compared as text, except that numbers compare by value across dialects
("49.5000" from PostgreSQL's NUMERIC is SQLite's 49.5). Between two SQLite-dialect targets every
value must match exactly. Rows of a statement with no ORDER BY compare as a multiset.

A mismatch prints the statement, its parameters and both answers; an answer that failed prints its
error. Exit status 1 if anything differed or failed, 0 otherwise.

usage: suite_answers.py RECORDS.jsonl [REFERENCE]
"""
import json
import re
import sys
from decimal import Decimal, InvalidOperation

SQLITE_DIALECT = {"amphora", "turso", "sqlite", "mvcc"}


def number(v):
    try:
        return Decimal(v)
    except (InvalidOperation, TypeError, ValueError):
        return None


def same(a, b, cross):
    if a == b:
        return True
    if not cross or a is None or b is None:
        return False
    x, y = number(a), number(b)
    if x is None or y is None or not x.is_finite() or not y.is_finite():
        return False
    # Across dialects a number is compared as the f64 both sides would read it as.
    return x == y or float(x) == float(y)


def ordered(sql):
    return "ORDER BY" in re.sub(r"OVER\s*\([^)]*\)", "", sql.upper())


def show(rows):
    cut = lambda v: v if v is None or len(v) <= 120 else f"{v[:120]}...({len(v)} chars)"
    return json.dumps([[cut(v) for v in r] for r in rows])


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    ref_name = sys.argv[2] if len(sys.argv) > 2 else "sqlite"
    reports = {}
    for line in open(sys.argv[1]):
        try:
            r = json.loads(line)
        except ValueError:
            continue
        rep = r.get("report")
        if isinstance(rep, dict) and isinstance(rep.get("answers"), list):
            reports[rep["target"]] = rep
    if ref_name not in reports:
        print(f"no answers from the reference {ref_name} (targets with answers: {sorted(reports)})")
        return 2
    ref = {(a["op"], json.dumps(a["params"])): a for a in reports[ref_name]["answers"]}
    bad = 0
    for name, rep in sorted(reports.items()):
        if name == ref_name:
            continue
        cross = (name in SQLITE_DIALECT) != (ref_name in SQLITE_DIALECT)
        compared = differ = 0
        for a in rep["answers"]:
            r = ref.get((a["op"], json.dumps(a["params"])))
            if r is None:
                continue
            compared += 1
            mine, theirs = a["answer"], r["answer"]
            if "error" in mine or "error" in theirs:
                differ += 1
                print(f"ERROR {name} vs {ref_name}: {a['op']} {a['sql']} params {a['params']}: {name}: {mine.get('error', 'answered')} | {ref_name}: {theirs.get('error', 'answered')}")
                continue
            x, y = mine["rows"], theirs["rows"]
            if not ordered(a["sql"][0] if a["sql"] else ""):
                key = lambda row: json.dumps(row)
                x, y = sorted(x, key=key), sorted(y, key=key)
            if len(x) == len(y) and all(len(p) == len(q) and all(same(u, v, cross) for u, v in zip(p, q)) for p, q in zip(x, y)):
                continue
            differ += 1
            print(f"DIFFERS {name} vs {ref_name}: {a['op']} (client {a['client']})")
            print(f"  {name} sql: {' ; '.join(a['sql'])}")
            print(f"  {ref_name} sql: {' ; '.join(r['sql'])}")
            print(f"  params: {json.dumps(a['params'])}")
            print(f"  {name} ({len(x)} rows): {show(x)}")
            print(f"  {ref_name} ({len(y)} rows): {show(y)}")
        missing = len(rep["answers"]) - compared
        print(f"{name} vs {ref_name}: {compared} answers compared, {differ} differ or failed" + (f", {missing} with no {ref_name} answer to compare" if missing else ""))
        bad += differ
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
