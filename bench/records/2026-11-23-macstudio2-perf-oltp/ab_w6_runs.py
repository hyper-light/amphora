"""Lane W6 (oltp): one row a run of ab-w5.sh: writes a second, the run's mean flush (the store's
flush_ms over all its flushes, the quantity W5's bound in ab_w5_quiet.py reads), rows a flush (the
arm's load-only run subtracted, as ab_w5_stats.py does), and the 1-minute load average and the CPU
all processes took (ps, 100 a core) before and after the run, from ab-w5.sh's log.

python3 -I ab_w6_runs.py <ab-w5.sh's log> <its output folder>
"""
import json
import os
import re
import sys


def stats(path):
    try:
        return json.load(open(path)).get("stats") or {}
    except (OSError, ValueError):
        return {}


log, folder = sys.argv[1], sys.argv[2]
print("| run | arm | statement | clients | writes/s | mean flush ms | rows/flush | load before, after | CPU % before, after |")
print("|---|---|---|---|---|---|---|---|---|")
for text in open(log):
    m = re.match(r"(r\d+)-(\w+?)-(insert_one|update_plain)-(\d+) ops/s (\S+) .* load (\S+) \S+ \S+ cpu_pct (\d+) ncpu \d+ \| after load (\S+) \S+ \S+ cpu_pct (\d+)", text)
    if not m:
        continue
    r, arm, op, c, ops, l0, c0, l1, c1 = m.groups()
    st, base = stats(os.path.join(folder, f"{r}-{arm}-{op}-{c}.json")), stats(os.path.join(folder, f"load-{arm}.json"))
    f, fb = st.get("flushes") or 0, (st.get("flushes") or 0) - (base.get("flushes") or 0)
    mean = f"{(st.get('flush_ms') or 0) / f:.2f}" if f else "-"
    rows = f"{((st.get('rows') or 0) - (base.get('rows') or 0)) / fb:.1f}" if fb > 0 else "-"
    print(f"| {r} | {arm} | {op} | {c} | {ops} | {mean} | {rows} | {l0}, {l1} | {c0}, {c1} |")
