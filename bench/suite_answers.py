#!/usr/bin/env python3
"""The suite's answer check: every value of every read/DDL answer in each target's record
(src/suite.rs `answers`, written by suite.sh / comprehensive.sh), held to the reference target's
(SQLite's by default).

Dialect-specific catalog ops (e.g. `catalog_introspect`) are cross-checked against a same-dialect
reference whenever one is present in the JSONL (e.g. `amphora-pg` vs `postgres`), and never
silently skipped without verifying non-empty error-free answers.

Values are compared as text, except that numbers compare by value across dialects
("49.5000" from PostgreSQL's NUMERIC is SQLite's 49.5). Between two SQLite-dialect targets every
value must match exactly. Rows of a statement with no ORDER BY compare as a multiset.

A mismatch prints the statement, its parameters and both answers; an answer that failed prints its
error. Exit status 1 if anything differed or failed, 0 otherwise.

usage: suite_answers.py RECORDS.jsonl [REFERENCE] [--require-pg-ref]
"""
import json
import re
import sys
from decimal import Decimal, InvalidOperation

SQLITE_NUM_FORMAT = {"amphora", "amphora-pg", "turso", "sqlite", "mvcc"}
PG_CATALOG_DIALECT = {"amphora-pg", "postgres", "orioledb", "cedardb"}
SQLITE_CATALOG_DIALECT = {"amphora", "turso", "sqlite", "mvcc"}
DIALECT_SPECIFIC_OPS = {
    "catalog_introspect",
    "catalog_public_relations",
    "agg_ordered_set_stats",
}


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


def any_ordered(stmts):
    return any(ordered(s) for s in stmts)


def show(rows):
    cut = lambda v: v if v is None or len(v) <= 120 else f"{v[:120]}...({len(v)} chars)"
    return json.dumps([[cut(v) for v in r] for r in rows])


def same_catalog_dialect(a, b):
    if a in PG_CATALOG_DIALECT and b in PG_CATALOG_DIALECT:
        return True
    if a in SQLITE_CATALOG_DIALECT and b in SQLITE_CATALOG_DIALECT:
        return True
    return a == b


def compare_answer(name, ref_name, a, r, cross):
    mine, theirs = a["answer"], r["answer"]
    if "error" in mine or "error" in theirs:
        print(
            f"ERROR {name} vs {ref_name}: {a['op']} {a['sql']} params {a['params']}: "
            f"{name}: {mine.get('error', 'answered')} | {ref_name}: {theirs.get('error', 'answered')}"
        )
        return False
    x, y = mine["rows"], theirs["rows"]
    if not any_ordered(a.get("sql", [])):
        key = lambda row: json.dumps(row)
        x, y = sorted(x, key=key), sorted(y, key=key)
    if len(x) == len(y) and all(
        len(p) == len(q) and all(same(u, v, cross) for u, v in zip(p, q))
        for p, q in zip(x, y)
    ):
        return True
    print(f"DIFFERS {name} vs {ref_name}: {a['op']} (client {a['client']})")
    print(f"  {name} sql: {' ; '.join(a['sql'])}")
    print(f"  {ref_name} sql: {' ; '.join(r['sql'])}")
    print(f"  params: {json.dumps(a['params'])}")
    print(f"  {name} ({len(x)} rows): {show(x)}")
    print(f"  {ref_name} ({len(y)} rows): {show(y)}")
    return False


def main():
    args = [a for a in sys.argv[1:] if a != "--require-pg-ref"]
    require_pg_ref = "--require-pg-ref" in sys.argv[1:]
    if len(args) < 1:
        print(__doc__)
        return 2
    path = args[0]
    ref_name = args[1] if len(args) > 1 else "sqlite"
    reports = {}
    for line in open(path):
        try:
            r = json.loads(line)
        except ValueError:
            continue
        rep = r.get("report") if isinstance(r.get("report"), dict) else r
        if isinstance(rep, dict) and isinstance(rep.get("answers"), list) and "target" in rep:
            reports[rep["target"]] = rep
    if ref_name not in reports:
        print(f"no answers from the reference {ref_name} (targets with answers: {sorted(reports)})")
        return 2
    if require_pg_ref and "postgres" not in reports:
        print(f"--require-pg-ref set but no postgres reference found (targets: {sorted(reports)})")
        return 2
    ref_rep = reports[ref_name]
    ref = {(a["op"], json.dumps(a["params"])): a for a in ref_rep["answers"]}
    pg_ref = (
        {(a["op"], json.dumps(a["params"])): a for a in reports["postgres"]["answers"]}
        if "postgres" in reports
        else None
    )
    sqlite_ref = (
        {(a["op"], json.dumps(a["params"])): a for a in reports["sqlite"]["answers"]}
        if "sqlite" in reports
        else None
    )
    ref_results = {
        (r.get("op"), r.get("clients")): r
        for r in ref_rep.get("results", [])
        if isinstance(r, dict) and not r.get("unsupported")
    }
    pg_results = (
        {
            (r.get("op"), r.get("clients")): r
            for r in reports["postgres"].get("results", [])
            if isinstance(r, dict) and not r.get("unsupported")
        }
        if "postgres" in reports
        else {}
    )
    bad = 0
    for name, rep in sorted(reports.items()):
        if name == ref_name:
            continue
        cross = (name in SQLITE_NUM_FORMAT) != (ref_name in SQLITE_NUM_FORMAT)
        compared = differ = 0
        unsupported_ops = {
            r.get("op")
            for r in rep.get("results", [])
            if isinstance(r, dict) and r.get("unsupported")
        }
        seen_keys = set()
        seen_pg_dialect_keys = set()
        for a in rep["answers"]:
            key_pair = (a["op"], json.dumps(a["params"]))
            if a["op"] in DIALECT_SPECIFIC_OPS and not same_catalog_dialect(name, ref_name):
                # Prefer comparing PostgreSQL catalog answers directly against stock `postgres`
                # whenever present, and SQLite catalog answers against stock `sqlite`.
                if name in PG_CATALOG_DIALECT and pg_ref is not None and name != "postgres":
                    seen_pg_dialect_keys.add(key_pair)
                    r = pg_ref.get(key_pair)
                    if r is None:
                        differ += 1
                        print(f"MISSING_PG_REF {name} vs postgres: {a['op']} params {a['params']}: postgres produced no answer")
                        continue
                    compared += 1
                    pg_cross = (name in SQLITE_NUM_FORMAT) != ("postgres" in SQLITE_NUM_FORMAT)
                    if not compare_answer(name, "postgres", a, r, pg_cross):
                        differ += 1
                    continue
                if name in SQLITE_CATALOG_DIALECT and sqlite_ref is not None and name != "sqlite":
                    r = sqlite_ref.get(key_pair)
                    if r is not None:
                        compared += 1
                        sq_cross = (name in SQLITE_NUM_FORMAT) != ("sqlite" in SQLITE_NUM_FORMAT)
                        if not compare_answer(name, "sqlite", a, r, sq_cross):
                            differ += 1
                        continue
                # No same-dialect reference present (or this target IS the alternate reference):
                # still validate error-free, non-empty catalog answers.
                compared += 1
                mine = a["answer"]
                if "error" in mine or not mine.get("rows"):
                    differ += 1
                    print(
                        f"INVALID_CATALOG {name}: {a['op']} returned "
                        f"{mine.get('error', 'empty rows')} with no same-dialect reference"
                    )
                continue
            seen_keys.add(key_pair)
            r = ref.get(key_pair)
            if r is None:
                continue
            compared += 1
            if not compare_answer(name, ref_name, a, r, cross):
                differ += 1
        for (op, params_json), r in ref.items():
            if (op, params_json) in seen_keys or op in unsupported_ops:
                continue
            if op in DIALECT_SPECIFIC_OPS and not same_catalog_dialect(name, ref_name):
                continue
            compared += 1
            differ += 1
            print(f"MISSING {name} vs {ref_name}: {op} params {params_json}: {name} produced no answer")
        if name in PG_CATALOG_DIALECT and pg_ref is not None and name != "postgres":
            for (op, params_json), _ in pg_ref.items():
                if op not in DIALECT_SPECIFIC_OPS or op in unsupported_ops:
                    continue
                if (op, params_json) not in seen_pg_dialect_keys:
                    compared += 1
                    differ += 1
                    print(f"MISSING {name} vs postgres: {op} params {params_json}: {name} produced no answer")

        # Also validate timed results (writes, DDL, reads): errors, 0-op stalls, or missing ops fail.
        cand_results = {
            (r.get("op"), r.get("clients")): r
            for r in rep.get("results", [])
            if isinstance(r, dict) and not r.get("unsupported")
        }
        result_errors = 0
        checked_results = 0
        for (op, clients), cr in sorted(cand_results.items()):
            checked_results += 1
            if cr.get("errors", 0) > 0 or cr.get("error"):
                result_errors += 1
                print(
                    f"RESULT_ERROR {name}: {op} (clients={clients}): "
                    f"errors={cr.get('errors')} ops_per_s={cr.get('ops_per_s')} error={cr.get('error')}"
                )
                continue
            rr = ref_results.get((op, clients))
            if rr is None and name in PG_CATALOG_DIALECT and name != "postgres":
                rr = pg_results.get((op, clients))
            if rr and rr.get("errors", 0) == 0 and rr.get("ops_per_s", 0) > 0 and cr.get("ops_per_s", 0) <= 0:
                result_errors += 1
                print(
                    f"RESULT_ZERO_OPS {name} vs {ref_name}: {op} (clients={clients}): "
                    f"{name} completed 0 ops/s while reference completed {rr.get('ops_per_s')} ops/s"
                )
        for (op, clients), rr in sorted(ref_results.items()):
            if (op, clients) not in cand_results and op not in unsupported_ops:
                checked_results += 1
                result_errors += 1
                print(f"MISSING_RESULT {name} vs {ref_name}: {op} (clients={clients}) omitted by {name}")

        if compared == 0 and checked_results == 0:
            result_errors += 1
            print(f"MISSING_EVIDENCE {name} vs {ref_name}: 0 answers and 0 results compared")
        missing = len(rep["answers"]) - compared
        print(
            f"{name} vs {ref_name}: {compared} answers compared, {differ} differ or failed, "
            f"{checked_results} results checked, {result_errors} result errors"
            + (f", {missing} with no {ref_name} answer to compare" if missing > 0 else "")
        )
        bad += differ + result_errors
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
