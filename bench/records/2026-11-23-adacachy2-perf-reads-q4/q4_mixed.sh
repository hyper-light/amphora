#!/usr/bin/env bash
# Lane Q4: decision 40's ways on kiln's conversation workload (K0's mixed phase: 32 writers commit
# events durably while 32 readers read sessions' latest pages), where the log holds the pages read
# and every commit moves the snapshot's last frame on. One fresh store a run, the ways' order
# rotated each round, the load recorded before each run.
# usage: BIN=<amphora-bench> q4_mixed.sh
set -uo pipefail
L=~/.cache/analytics-q4 ROUNDS=${ROUNDS:-2}
WAYS=(locked published optimistic filtered)
export XDG_CACHE_HOME=$L/xdg
mkdir -p "$L/k0" && : > "$L/k0.jsonl"
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
for round in $(seq 1 "$ROUNDS"); do
  shift_by=$(( (round - 1) % ${#WAYS[@]} ))
  order=("${WAYS[@]:$shift_by}" "${WAYS[@]:0:$shift_by}")
  for way in "${order[@]}"; do
    dir=$L/k0/$way-$round; rm -rf "$dir"
    before=$(state)
    echo "=== $way-$round $(date -u +%H:%M:%SZ) $before"
    AMPHORA_LOG_READ_INDEX=$way nice -n 10 "$BIN" --target amphora --dir "$dir" --sessions 1000 --preload 50 --writers 32 --readers 32 --seconds 8 --warmup 2 --searches 1 2>>"$L/k0.err" | tail -1 | python3 -I -c '
import json, sys
r = json.loads(sys.stdin.read()); s = lambda k: {x: (r.get(k) or {}).get(x) for x in ("n", "p50", "p99")}
print(json.dumps({"tag": sys.argv[1], "state_before": sys.argv[2], "failures": r.get("failures"), "unanswered": r.get("unanswered"), "write_events_per_s": r.get("write_events_per_s"), "write_ms": s("write_ms"), "read_ms": s("read_ms"), "mixed_cpu_s": r.get("mixed_cpu_s")}))' "$way-$round" "$before" | tee -a "$L/k0.jsonl"
    rm -rf "$dir"
  done
done
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
