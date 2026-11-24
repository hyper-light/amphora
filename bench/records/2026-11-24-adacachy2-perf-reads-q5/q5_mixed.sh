#!/usr/bin/env bash
# Lane Q5: decision 40's four ways (AMPHORA_LOG_READ_INDEX) in K0's mixed phase: N writers commit
# events durably while N readers read sessions' latest pages (N = 1, 32, 128), so the log holds the
# pages read and every commit moves the readers' snapshot on. One binary, so one CPU cost
# calibration for every way (RULES 5e).
#   1. profiles (AMPHORA_PROFILE, 997 Hz), two rounds: the ways in order, then reversed;
#   2. ROUNDS timed rounds, the ways' order rotated each round;
#   3. each timed run's answers after its mixed phase (AMPHORA_ANSWERS) held to SQLite's on the
#      same files (q5_answers.py).
# The load average and CPU and I/O pressure are recorded before every run.
# usage: BIN=<amphora-bench> TOOLS=<dir with q4_shares.py, q5_answers.py, q5_table.py> q5_mixed.sh
set -uo pipefail
L=~/.cache/analytics-q5 ROUNDS=${ROUNDS:-3} MIXED=${MIXED_SECONDS:-6}
WAYS=(locked filtered published optimistic)
CLIENTS=(1 32 128)
export XDG_CACHE_HOME=$L/xdg
rm -rf "$L/k0" "$L/prof" && mkdir -p "$L/k0" "$L/prof" "$XDG_CACHE_HOME" || exit 1
: > "$L/k0.jsonl"; : > "$L/answers.jsonl"
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
run() { # tag way clients [profile prefix]
  local tag=$1 way=$2 n=$3 prof=${4:-} dir=$L/k0/$1 before rc
  rm -rf "$dir"; before=$(state)
  echo "=== $tag $(date -u +%H:%M:%SZ) $before"
  env AMPHORA_LOG_READ_INDEX="$way" ${prof:+AMPHORA_PROFILE=$prof} AMPHORA_ANSWERS="$dir.answers" nice -n 10 "$BIN" --target amphora --dir "$dir" \
    --sessions 1000 --preload 50 --writers "$n" --readers "$n" --seconds "$MIXED" --warmup 1 --searches 3 > "$dir.out" 2>>"$L/err.log"; rc=$?
  python3 -I -c '
import json, sys
lines = open(sys.argv[5]).read().strip().splitlines()
try:
    r = json.loads(lines[-1])
except Exception:
    r = {"error": "no report"}
print(json.dumps({"tag": sys.argv[1], "way": sys.argv[2], "clients": int(sys.argv[3]), "state_before": sys.argv[4], "exit": int(sys.argv[6]), "report": r}))' "$tag" "$way" "$n" "$before" "$dir.out" "$rc" >> "$L/k0.jsonl"
  [ "$rc" = 0 ] || { echo "exit $rc"; tail -3 "$L/err.log"; }
  if [ -z "$prof" ] && [ "$rc" = 0 ]; then
    python3 -I "$TOOLS/q5_answers.py" "$tag" "$dir.answers" "$dir/bench-0.db" >> "$L/answers.jsonl"
  fi
  rm -rf "$dir" "$dir.answers"
}
for n in "${CLIENTS[@]}"; do
  for round in 1 2; do
    if [ "$round" = 1 ]; then order=("${WAYS[@]}"); else order=(optimistic published filtered locked); fi
    for way in "${order[@]}"; do
      run "prof-$way-$round-$n" "$way" "$n" "$L/prof/$way-$round-amphora-mixed-$n"
    done
  done
done
python3 -I "$TOOLS/q4_shares.py" "$L/prof"
for round in $(seq 1 "$ROUNDS"); do
  shift_by=$(( (round - 1) % ${#WAYS[@]} ))
  order=("${WAYS[@]:$shift_by}" "${WAYS[@]:0:$shift_by}")
  for n in "${CLIENTS[@]}"; do
    for way in "${order[@]}"; do
      run "$way-$round-$n" "$way" "$n"
    done
  done
done
python3 -I "$TOOLS/q5_table.py" "$L/k0.jsonl" "$L/answers.jsonl"
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
