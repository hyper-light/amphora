#!/usr/bin/env python3
"""Each corpus record translated once per dialect (G2 step 4), at corpus build time, by sqlglot (the
translator sources.json pins), so the differential harness (crates/corpus, amphora_corpus::dialect)
runs every engine on SQL it speaks and never translates as it runs.

For every `.test` file under the pool (symbolic links followed: the pool links sqllogictest's
checkout), each record's SQL, in SQLite's dialect as the corpus holds it, is written in each dialect
to <pool>/translations/<dialect>/<file>.jsonl, a JSON line a record in file order:
  {"line": n, "sql": "..."}              its text in the dialect
  {"line": n, "untranslatable": "..."}   sqlglot can't say it there, and why
The harness classifies an untranslatable record as such, never as a disagreement. Engines that speak
SQLite's dialect (SQLite, Amphora, Turso) take the records as written and get no translation; the
postgres dialect is OrioleDB's and CedarDB's too.

usage: dialects.py <pool dir> [<dialect> ...]   (default: every dialect below)
"""
import json
import logging
import os
import re
import sys

import slt

DIALECTS = ("duckdb", "postgres", "mysql", "clickhouse")

logging.getLogger("sqlglot").setLevel(logging.ERROR)  # its "falling back to a Command" notes


def translate(sql, dialect):
    """`sql`, in SQLite's dialect, in `dialect`: (text, None), or (None, why) when sqlglot can't say
    it there. The one translation the corpus's tools share (probe.py's too)."""
    if dialect == "sqlite":
        return sql, None
    import sqlglot

    try:
        out = sqlglot.transpile(sql, read="sqlite", write=dialect, unsupported_level=sqlglot.ErrorLevel.RAISE)
    except Exception as e:  # sqlglot can't say it in this dialect
        return None, f"untranslatable: {type(e).__name__}: {str(e)[:200]}"
    if dialect == "clickhouse":
        # A primary key orders a MergeTree table, and its columns can't be Nullable there.
        out = [re.sub(r"Nullable\(([A-Za-z0-9]+(?:\([0-9, ]+\))?)\)(\s+PRIMARY KEY)", r"\1\2", o) for o in out]
    if dialect == "mysql":
        # SQLite's TEXT column is MySQL's VARCHAR: MySQL's TEXT takes no DEFAULT and no index
        # without a prefix length, which SQLite's text has no need of.
        out = [re.sub(r"\bTEXT\b", "VARCHAR(255)", o) if re.match(r"\s*(CREATE\s+(TEMPORARY\s+)?TABLE|ALTER\s+TABLE)", o, re.I) else o for o in out]
    if not out:
        return None, "untranslatable: sqlglot wrote nothing"
    return ";\n".join(out), None


def files(pool):
    for root, _dirs, names in os.walk(pool, followlinks=True):
        if os.path.relpath(root, pool).split(os.sep)[0] == "translations":
            continue
        for n in sorted(names):
            if n.endswith(".test"):
                yield os.path.relpath(os.path.join(root, n), pool)


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    pool = sys.argv[1]
    dialects = sys.argv[2:] or DIALECTS
    counts = {d: [0, 0] for d in dialects}
    for rel in files(pool):
        records = [r for kind, r in slt.parse(os.path.join(pool, rel)) if kind == "record"]
        for d in dialects:
            out = os.path.join(pool, "translations", d, rel + ".jsonl")
            os.makedirs(os.path.dirname(out), exist_ok=True)
            with open(out, "w", encoding="utf-8") as f:
                for r in records:
                    sql, why = translate(r.sql, d)
                    counts[d][why is not None] += 1
                    f.write(json.dumps({"line": r.line, "sql": sql} if why is None else {"line": r.line, "untranslatable": why}) + "\n")
    for d, (ok, no) in counts.items():
        print(f"{d}: {ok} records translated, {no} untranslatable")


if __name__ == "__main__":
    main()
