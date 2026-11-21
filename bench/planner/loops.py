#!/usr/bin/env python3
"""Count the plans that read a table whole beneath loops that can give more than one row (PL.6,
docs/research/planner.md rank 6; docs/design/planner.md, "Decision (lane pl6)").

Such a table is read again for each row of the loops before it, whatever their estimate said. The
count reads each Amphora query's EXPLAIN QUERY PLAN FORMAT=JSON in the planner bench's records
(`amphora-bench --planner`): in each query block (the steps with one parent), a step that reads a
B-tree table whole (a `scan` of a table or of an index, or a `hash_join`, whose probe scans its
table) counts when a table access comes before it in the block and any of those before it is not a
lookup by rowid (`rowid_eq`, at most one row a row). A hash join's build table (the step's
`build`) is listed in its block after the probe, but read once, before the first probe: its scan
neither counts nor stands before a later step. A lookup by every column of a unique index
gives at most one row too, but the JSON doesn't say an index is unique, so it counts here: the count
is never below the planner's own. Step 3's records count 30 of JOB's 113 plans and none of TPC-H's.

usage: python3 -I bench/planner/loops.py <records.jsonl>...
Prints each plan with such a read, the read and the steps before it, then
`loops.py: <file>: N of M plans read a table whole beneath loops that can give more than one row`.
"""

import json
import sys

ACCESS = {"scan", "search", "multi_index", "hash_join", "index_method"}


def whole_reads_in_loops(plan):
    """The steps of `plan` that read a table whole beneath loops that can give more than one row,
    each with the details of the table accesses before it in its block."""
    blocks = {}
    for node in plan.get("nodes", []):
        blocks.setdefault(node.get("parent"), []).append(node)
    found = []
    for nodes in blocks.values():
        builds = {n["op"]["build"] for n in nodes if n.get("op", {}).get("type") == "hash_join" and "build" in n["op"]}
        before = []
        for node in nodes:
            op = node.get("op", {})
            kind = op.get("type")
            if kind not in ACCESS:
                continue
            if kind == "scan" and op.get("alias", op.get("table")) in builds:
                continue
            whole = (kind == "scan" and op.get("source") == "table") or kind == "hash_join"
            if whole and before and not all(b["op"].get("search_kind") == "rowid_eq" for b in before):
                found.append((node["detail"], [b["detail"] for b in before]))
            before.append(node)
    return found


def count(path):
    """(plans with such a read, plans) over the Amphora query records of `path`, printing each."""
    with_loops = plans = 0
    with open(path) as records:
        for line in records:
            record = json.loads(line)
            if record.get("record") != "query" or record.get("target") != "amphora" or not record.get("plan"):
                continue
            plans += 1
            found = whole_reads_in_loops(record["plan"])
            if found:
                with_loops += 1
                for detail, before in found:
                    print(f"{record.get('query')}: {detail} beneath {' / '.join(before)}")
    return with_loops, plans


def main(paths):
    if not paths:
        print(__doc__, file=sys.stderr)
        return 2
    for path in paths:
        with_loops, plans = count(path)
        print(f"loops.py: {path}: {with_loops} of {plans} plans read a table whole beneath loops that can give more than one row")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
