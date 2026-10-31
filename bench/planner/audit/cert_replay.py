#!/usr/bin/env python3
"""Every CERT finding the oracles recorded, replayed on a build (the planner audit's row 5 and 12):
each finding's statements, then EXPLAIN QUERY PLAN FORMAT=JSON of its query and of its more
restrictive form, through `amphora-bench --eqp`. The estimate is the oracle's
(bench/src/oracle/checks.rs `estimate_any`): the plan's output_rows, else the last top-level
table's. A finding still fails when the more restrictive form is estimated at more rows
(`no_more`). Prints one line a finding, its cause from the campaign's causes.tsv, then the totals.

python3 bench/planner/audit/cert_replay.py BENCH_BINARY RECORDS_DIR SCRATCH_DIR
"""

import json
import pathlib
import subprocess
import sys


def blocks(text):
    parts = text.split("```sql\n")[1:]
    return [p.split("```")[0] for p in parts]


def statements(block):
    return [s.strip() for s in block.split(";\n") if s.strip()]


def estimate(plan_text):
    j = json.loads(plan_text)
    if isinstance(j.get("output_rows"), (int, float)):
        return float(j["output_rows"])
    tops = [n["op"].get("estimate", {}).get("output_rows") for n in j.get("nodes", []) if n.get("parent") is None]
    tops = [t for t in tops if isinstance(t, (int, float))]
    return float(tops[-1]) if tops else None


def plans(output):
    """The JSON plans the script printed, in order (None for a statement that failed)."""
    out = []
    for chunk in output.split("\n> ")[1:]:
        head, _, body = chunk.partition("\n")
        if not head.startswith("EXPLAIN QUERY PLAN FORMAT=JSON"):
            continue
        body = body.strip()
        out.append(estimate(body[: body.rindex("}") + 1]) if body.startswith("{") else None)
    return out


def no_more(more, base):
    return more <= base * (1.0 + 1e-9) + 1e-9


def main():
    binary, records, scratch = sys.argv[1], pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3])
    scratch.mkdir(parents=True, exist_ok=True)
    causes = {}
    for tsv in sorted(records.glob("*/causes.tsv")):
        for line in tsv.read_text().splitlines():
            f = line.split("\t")
            if len(f) >= 4:
                causes[(tsv.parent.name, f[0])] = f[3]
    totals = {}
    for path in sorted(records.glob("*/findings/cert-*.md")):
        campaign = path.parent.parent.name
        b = blocks(path.read_text())
        stmts, queries = statements(b[0]), statements(b[1]) if len(b) > 1 else []
        cause = causes.get((campaign, path.stem), "?")
        if len(queries) != 2:
            verdict, eq, er = "malformed", None, None
        else:
            script = scratch / "cert.sql"
            body = [s.replace("\n", " ") + ";" for s in stmts]
            body += ["EXPLAIN QUERY PLAN FORMAT=JSON " + q.replace("\n", " ") + ";" for q in queries]
            script.write_text("\n".join(body) + "\n")
            run = subprocess.run([binary, "--eqp", "--script", str(script), "--dir", str(scratch / "db")], capture_output=True, text=True, timeout=120)
            found = plans(run.stdout)
            eq, er = (found + [None, None])[:2]
            if eq is None or er is None:
                verdict = "no estimate"
            else:
                verdict = "passes" if no_more(er, eq) else "fails"
        totals[(cause, verdict)] = totals.get((cause, verdict), 0) + 1
        print(f"{campaign}\t{path.stem}\t{cause}\t{eq}\t{er}\t{verdict}")
    print()
    for (cause, verdict), n in sorted(totals.items()):
        print(f"cause {cause}: {n} {verdict}")


if __name__ == "__main__":
    main()
