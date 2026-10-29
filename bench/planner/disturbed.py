#!/usr/bin/env python3
"""Which query records of a planner bench pass ran while other work held the machine's cores, so
they can be run again on a quiet machine (bench/records/2026-10-26-jupiter-planner-baseline.md).

Inputs: a monitor's samples, a line each 10 s, `<utc> total=<cores> freeGB=<gb> <name>:<pid>:<cores>
...` (every process using a fifth of a core or more over the interval); the passes' log lines, each
with the UTC time it appeared (`<utc> <query> <engine>: <ms> ms`, tail's `==> <pass>.log <==`
headers, and `<utc> == <pass>` where a pass or one of its rerun groups starts); and the pass's name.
A record's runs happen between the log line before it and its own line: it is disturbed when a
sample in that span shows other processes (all but amphora-bench, and Docker's VM for PostgreSQL's
records) using more than LIMIT cores, or the span overlaps a window of other work the monitor can't
see (`--windows`, such as a disk-heavy delete).

A pass stopped part way and started again from its beginning under its name (a pass timed by one
process) keeps the stopped run's lines too: `--since` counts only the records after the restart.

usage: bench/planner/disturbed.py <monitor.log> <passes-ts.log> <pass> [<limit cores, 2>] [--windows a/b,...] [--since utc] [--json out.json]
"""

import json
import re
import sys
from datetime import datetime, timedelta


def t(s):
    return datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ")


def mine(name, target):
    """The bench's own processes: amphora-bench, and Docker's VM while PostgreSQL's queries run."""
    return name == "amphora-bench" or (target == "postgres" and name.lower().startswith("vmmem"))


def main(argv):
    mon_path, ts_path, name = argv[0], argv[1], argv[2]
    limit = float(argv[3]) if len(argv) > 3 and not argv[3].startswith("--") else 2.0
    json_out = argv[argv.index("--json") + 1] if "--json" in argv else None
    since = t(argv[argv.index("--since") + 1]) if "--since" in argv else None
    windows = []
    if "--windows" in argv:
        for w in argv[argv.index("--windows") + 1].split(","):
            a, b = w.split("/")
            windows.append((t(a), t(b)))
    samples = []
    for line in open(mon_path, errors="replace"):
        parts = line.split()
        if len(parts) < 3 or not parts[1].startswith("total="):
            continue
        procs = []
        for p in parts[3:]:
            m = re.match(r"(.+):(\d+):([0-9.]+)$", p)
            if m:
                procs.append((m.group(1), float(m.group(3))))
        samples.append((t(parts[0]), float(parts[1][len("total="):]), procs))
    # The pass's records by the time their log lines appeared; a start opens the next record's span.
    events, current = [], None
    for line in open(ts_path, errors="replace"):
        parts = line.rstrip("\n").split(" ", 1)
        if len(parts) < 2:
            continue
        stamp, rest = parts
        if rest.startswith("==> "):
            current = rest[4:].rstrip(" <=").rsplit("/", 1)[-1].removesuffix(".log")
        elif rest.startswith("== "):
            # A start names its pass too: tail following one log writes no `==>` header.
            current = (rest[3:].split() or [current])[0]
            if current == name:
                events.append((t(stamp), None, None, None))
        else:
            m = re.match(r"(\S+) (\S+): (.+)$", rest)
            if m and current == name:
                events.append((t(stamp), m.group(1), m.group(2), m.group(3)))
    out, prev = [], None
    for when, query, target, result in events:
        if query is None:
            prev = when
            continue
        lo, hi = (prev or when - timedelta(minutes=5)), when + timedelta(seconds=10)
        worst, who = 0.0, []
        for at, total, procs in samples:
            if not (lo < at <= hi):
                continue
            other = total - sum(c for n, c in procs if mine(n, target))
            if other > worst:
                worst = other
                who = sorted(((n, c) for n, c in procs if not mine(n, target)), key=lambda x: -x[1])[:3]
        overlaps = [f"{a:%H:%M}-{b:%H:%M}" for a, b in windows if a < when and b > lo]
        if overlaps:
            who = who + [("window " + " ".join(overlaps), 0.0)]
        if since is None or when >= since:
            out.append({"query": query, "target": target, "result": result, "from": lo.isoformat() + "Z", "to": when.isoformat() + "Z",
                        "worst_other_cores": round(worst, 2), "who": who, "disturbed": worst > limit or bool(overlaps)})
        prev = when
    bad = [r for r in out if r["disturbed"]]
    print(f"{name}: {len(out)} records, {len(bad)} with other work above {limit} cores or in a window")
    for r in bad:
        print(f"  {r['query']} {r['target']}: {r['worst_other_cores']} cores ({', '.join(f'{n} {c:.1f}' for n, c in r['who'])}), {r['from']} to {r['to']}")
    if json_out:
        with open(json_out, "w") as f:
            json.dump(out, f, indent=1)


if __name__ == "__main__":
    main(sys.argv[1:])
