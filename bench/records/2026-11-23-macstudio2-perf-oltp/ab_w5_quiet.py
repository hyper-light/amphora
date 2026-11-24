"""Lane W5 (oltp): ab-w5.sh's runs from several folders, keeping only runs whose flushes ran at the
disk's quiet cost: a run's mean flush (the store's flush_ms over its flushes) at most W4's largest
quiet mean on this machine, 5.40 ms (this record, W4's table: insert_one at 32 clients, arm A). A
flush is one F_FULLFSYNC whose cost W4 measured the same in both arms (4.55 to 4.93 ms medians), so
a slower one was slowed by other I/O on the disk. Then ab_w5_stats.py's table and rank-sum tests.

python3 -I ab_w5_quiet.py "<arms>" <folder> [<folder> ...]
"""
import glob
import json
import os
import shutil
import sys

QUIET_MS = 5.40
arms, folders = sys.argv[1], sys.argv[2:]
out = os.path.join(os.path.dirname(os.path.abspath(folders[0])), "out-w5-quiet")
os.makedirs(out, exist_ok=True)
for f in glob.glob(os.path.join(out, "*.json")):
    os.remove(f)
kept, dropped = 0, 0
for i, folder in enumerate(folders):
    for p in sorted(glob.glob(os.path.join(folder, "*.json"))):
        name = os.path.basename(p)
        try:
            st = json.load(open(p)).get("stats") or {}
        except (OSError, ValueError):
            continue
        if name.startswith("load-"):
            if i == 0:
                shutil.copy(p, os.path.join(out, name))
            continue
        flushes = st.get("flushes") or 0
        if flushes and (st.get("flush_ms") or 0) / flushes <= QUIET_MS:
            r, rest = name.split("-", 1)
            shutil.copy(p, os.path.join(out, f"r{i * 100 + int(r[1:])}-{rest}"))
            kept += 1
        else:
            dropped += 1
print(f"runs kept {kept}, dropped {dropped} (mean flush over {QUIET_MS} ms)")
os.execv(sys.executable, [sys.executable, "-I", os.path.join(os.path.dirname(os.path.abspath(__file__)), "ab_w5_stats.py"), out, arms])
