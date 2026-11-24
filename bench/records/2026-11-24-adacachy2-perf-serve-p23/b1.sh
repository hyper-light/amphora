#!/bin/bash
# Lane P23 (serve) b1, adacachy2: V13's real-time cell (32 spread writers, both classes, lane P22's flags) on main's parent
# before the session worker's landing, M (6d8e7dcd), and on main, H (3a58a13f, whose crates are d0c5722d's; the release
# binaries are lane P22's, built in r349), arms alternated; then rounds of the same cell with each member's threads' CPU and
# run-queue wait read while it runs (cell23.py). Prints the load and the CPU pressure beside every cell.
set -u
H=d0c5722de0fb1085bd052ce1655856c13517af53; M=6d8e7dcd38be29214187a06bbfabdf9b054acb2a
R=${R:-5} W=${W:-2} PORT=${PORT:-31740}
A=$HOME/.cache/amphora-perf/serve-p7-remote; BIN=$HOME/.cache/amphora-perf-bin/serve-p22
L=$HOME/.cache/amphora-perf/${LDIR:-serve-p23-b1}; mkdir -p "$L/cells" "$L/watch"
ld() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu-some $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/cpu) io-some $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/io)"; }
echo "start $(date -u +%T) $(ld)"; df -h ~ | tail -1
for s in $H $M; do [ -x "$BIN/$s/bench" ] || { echo "NO BIN $s"; exit 9; }; done
ss -ltn 2>/dev/null | grep -E ":(3174[0-9])" && echo "PORT BUSY"
git -C "$A" show $M:bench/records/2026-11-23-adacachy2-perf-commits-v13/cell13.py > "$L/cell13.py" || exit 9
git -C "$A" show ${S:-origin/perf/serve-p23}:bench/records/2026-11-24-adacachy2-perf-serve-p23/cell23.py > "$L/cell23.py" || exit 9
for r in $(seq 1 $R); do
  if [ $((r % 2)) -eq 1 ]; then arms="$H $M"; else arms="$M $H"; fi
  for sha in $arms; do for cls in persisted replicated; do
    echo "cell r$r ${sha:0:8} $cls $(date -u +%T) $(ld) $(nice -n 10 python3 -I "$L/cell13.py" "$BIN/$sha/bench" $PORT "$L/cells/${sha:0:8}-$cls-32-r$r" 32 spread "$cls" 2>&1 | tail -3 | tr '\n' ' ')"
  done; done
done
echo "end cells $(date -u +%T) $(ld)"
python3 -I - "$L/cells" <<'PY'
import json, glob, os, sys, collections, statistics
tot = collections.defaultdict(lambda: dict(cells=0, writes=0, refused=0, misses=0, writers_missed=0, rate=[], p50=[], p99=[], load=[]))
for f in sorted(glob.glob(os.path.join(sys.argv[1], "*", "rec.jsonl"))):
    name = os.path.basename(os.path.dirname(f)); sha, cls = name.split("-")[0], name.split("-")[1]
    r = json.loads(open(f).read().strip().splitlines()[-1])
    k = (sha, cls); t = tot[k]; t["cells"] += 1
    n = sum(b["latency_ms"]["n"] for b in r["by_node"]); t["writes"] += n
    t["refused"] += sum(b["refused"] for b in r["by_node"]); t["misses"] += r["deadline_misses"]; t["writers_missed"] += r["writers_missed"]
    t["rate"].append(round(r["writes_per_s"], 1)); t["p50"].append(round(r["latency_ms"]["p50"], 1)); t["p99"].append(round(r["latency_ms"]["p99"], 1)); t["load"].append(r.get("load1"))
for k in sorted(tot):
    t = tot[k]; print("SUM", k, t, "median p50", statistics.median(t["p50"]), "median rate", statistics.median(t["rate"]))
PY
for r in $(seq 1 $W); do
  if [ $((r % 2)) -eq 1 ]; then arms="$H $M"; else arms="$M $H"; fi
  for sha in $arms; do for cls in persisted replicated; do
    echo "watch r$r ${sha:0:8} $cls $(date -u +%T) $(ld) $(nice -n 10 python3 -I "$L/cell23.py" "$BIN/$sha/bench" $PORT "$L/watch/${sha:0:8}-$cls-32-r$r" 32 spread "$cls" 2>&1 | tail -3 | tr '\n' ' ')"
  done; done
done
echo "end $(date -u +%T) $(ld)"
