"""Step 1's record of the owners' stalls (docs/research/owner-stalls.md, kiln goal 6b112f14) as
bench records: one row per run and member, and per host and condition the best, median and max
of each statistic over those rows. Usage: python3 bench/owner_stalls.py > bench/records/<file>.jsonl
"""

import json
import re
import statistics
import sys

RECORD = "docs/research/owner-stalls.md"
SERIES = ["write_ms", "step_gap_ms", "tick_gap_ms"]
QUANTILES = ["p50", "p99", "p99_9", "max"]


def runs(text):
    for block in re.split(r"\n### ", text)[1:]:
        head, _, body = block.partition("\n")
        m = re.match(r"(\w+), (.+?), run (\d+) \((\w+)\)$|(\w+), run (\d+) \((\w+)\)$", head)
        if m.group(1):
            host, condition, run, commit = m.group(1), m.group(2), int(m.group(3)), m.group(4)
        else:
            host, condition, run, commit = m.group(5), "beside its shared work", int(m.group(6)), m.group(7)
        window = re.search(r"(\S+Z) to (\S+Z)", body)
        load = re.search(r"load ([\d.]+)-([\d.]+)(?:, median ([\d.]+))?", body)
        writes = re.search(r"([\d,]+) writes acknowledged,\s+([\d,]+)\s+refused", body)
        for line in body.splitlines():
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if len(cells) != 6 or not cells[0].isdigit():
                continue
            row = {
                "host": host,
                "condition": condition,
                "run": run,
                "commit": commit,
                "start": window.group(1),
                "end": window.group(2),
                "load_min": float(load.group(1)) if load else None,
                "load_max": float(load.group(2)) if load else None,
                "load_median": float(load.group(3)) if load and load.group(3) else None,
                "acknowledged": int(writes.group(1).replace(",", "")),
                "refused": int(writes.group(2).replace(",", "")),
                "member": int(cells[0]),
            }
            for name, cell in zip(SERIES, cells[1:4]):
                row.update({f"{name}_{q}": float(v) for q, v in zip(QUANTILES, cell.split("/"))})
            for follower in re.finditer(r"from (\d+): ([\d./]+) \((\d+)\)", cells[4]):
                key = f"answer_gap_from_{follower.group(1)}_ms"
                row.update({f"{key}_{q}": float(v) for q, v in zip(QUANTILES, follower.group(2).split("/"))})
                row[f"answer_gaps_over_200ms_from_{follower.group(1)}"] = int(follower.group(3))
            row["step_downs"], row["elections"], row["changes"] = map(int, cells[5].split("/"))
            yield row


def main():
    rows = list(runs(open(RECORD).read()))
    for row in rows:
        print(json.dumps({"kind": "run", "source": RECORD, **row}))
    groups = {}
    for row in rows:
        groups.setdefault((row["host"], row["condition"]), []).append(row)
    for (host, condition), group in groups.items():
        keys = [k for k in group[0] if (k.endswith(tuple(QUANTILES)) and not k.startswith("load_")) or k in ("step_downs", "elections", "refused")]
        summary = {"kind": "summary", "source": RECORD, "host": host, "condition": condition,
                   "runs": len({r["run"] for r in group}), "rows": len(group)}
        for k in keys:
            values = [r[k] for r in group if k in r]
            summary[k] = {"best": min(values), "median": statistics.median(values), "max": max(values)}
        print(json.dumps(summary))


if __name__ == "__main__":
    sys.exit(main())
