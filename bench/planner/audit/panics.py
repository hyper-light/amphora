#!/usr/bin/env python3
"""The planner audit's row 13 (docs/research/planner.md, "The audit"): what the planner's
production code holds that CLAUDE.md section 1 forbids, file by file and lint by lint, from the
count pass of scripts/check-production.sh, read on stdin:

CLIPPY_CONF_DIR="$PWD" cargo clippy -p amphora-engine --locked --no-deps --message-format=json -- \
  -W clippy::panic -W clippy::unwrap_used -W clippy::expect_used -W clippy::unreachable -W clippy::todo \
  -W clippy::unimplemented -W clippy::indexing_slicing -W clippy::arithmetic_side_effects \
  -W clippy::string_slice -W clippy::cast_possible_truncation -W clippy::cast_sign_loss \
  -W clippy::cast_possible_wrap -W clippy::disallowed_macros -W clippy::disallowed_methods \
  | python3 bench/planner/audit/panics.py > bench/planner/audit/panics.out

A lint with no count in these files has no column.

The planner's files: crates/engine/translate/optimizer, crates/engine/stats.rs and statement.rs.
The lines of every unwrap, expect, panic and unreachable are listed.
"""

import collections
import json
import sys

LINED = ("clippy::unwrap_used", "clippy::expect_used", "clippy::panic", "clippy::unreachable")


def planner(path):
    return "translate/optimizer/" in path or path.endswith("engine/stats.rs") or path.endswith("engine/statement.rs")


counts = collections.Counter()
lines = collections.defaultdict(set)
seen = set()
for raw in sys.stdin:
    try:
        m = json.loads(raw)
    except ValueError:
        continue
    if m.get("reason") != "compiler-message":
        continue
    msg = m["message"]
    code = (msg.get("code") or {}).get("code") or ""
    if not code.startswith("clippy::"):
        continue
    for s in msg["spans"]:
        if not s["is_primary"] or not planner(s["file_name"]):
            continue
        name = s["file_name"].split("crates/engine/")[-1]
        key = (name, s["line_start"], s["column_start"], code)
        if key in seen:
            continue
        seen.add(key)
        counts[(name, code)] += 1
        if code in LINED:
            lines[(name, code)].add(s["line_start"])
files = sorted({f for f, _ in counts})
codes = sorted({c for _, c in counts})
print("file\t" + "\t".join(c.removeprefix("clippy::") for c in codes))
for f in files:
    print(f + "\t" + "\t".join(str(counts[(f, c)]) for c in codes))
print("total\t" + "\t".join(str(sum(counts[(f, c)] for f in files)) for c in codes))
print()
for (f, c), ls in sorted(lines.items()):
    print(f"{f}\t{c.removeprefix('clippy::')}\t" + ", ".join(str(x) for x in sorted(ls)))
