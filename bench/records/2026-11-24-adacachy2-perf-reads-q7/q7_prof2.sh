#!/usr/bin/env bash
# Lane Q7: a second profile of each arm at 32 and 128 clients (B first this time), because three of
# q7_ab.sh's four profile runs had slow flushes and B's at 32 clients wrote 541 events (a disk stall,
# a read-only phase). Each run's mixed-phase flushes and ms a flush printed beside it; then
# q6_split.py, q7_limit.py and q7_leaf.py on the new profiles.
# usage: A=<sha> B=<sha> q7_prof2.sh
set -uo pipefail
A=${A:?} B=${B:?}
L=~/.cache/analytics-q7 BIN=~/.cache/amphora-perf-bin/analytics TOOLS=~/.cache/analytics-q7/tools
export XDG_CACHE_HOME=$L/xdg-ab
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
source <(sed -n '/^prof() {/,/^}/p' "$TOOLS/q6_profile.sh")
for n in 32 128; do for arm in "$B" "$A"; do
  prof "ab2-${arm:0:8}-$n" "$BIN/amphora-bench-$arm" "$n" | grep -v '^{' 
  python3 -I -c '
import json, sys
x = json.loads(open(sys.argv[1]).read().strip().splitlines()[-1])
m, p = x["stats_after_mixed"], x["stats_after_preload"]
f, fm = m["flushes"] - p["flushes"], m["flush_ms"] - p["flush_ms"]
print("reads/s %.0f events/s %.0f events_written %d cores %.1f mixed flushes %d ms a flush %.0f" % (x["read_ms"]["n"] / x["seconds"], x["write_events_per_s"], x["events_written"], (x.get("mixed_cpu_s") or 0) / x["seconds"], f, fm / f if f else float("nan")))' "$L/k0/ab2-${arm:0:8}-$n.out"
done; done
python3 -I "$TOOLS/q6_split.py" "$L"/prof/ab2-*.folded
python3 -I "$TOOLS/q7_limit.py" --top 15 "$L"/prof/ab2-*.folded
python3 -I "$TOOLS/q7_leaf.py" '^atomic_|^_mm_pause' 12 7 "$L"/prof/ab2-*.folded
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
