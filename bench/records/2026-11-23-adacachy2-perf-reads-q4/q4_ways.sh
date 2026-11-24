#!/usr/bin/env bash
# Lane Q4: decision 40's four ways (AMPHORA_LOG_READ_INDEX = locked, filtered, published, optimistic)
# on one binary, point_pk, point_secondary and point_text at 1, 32 and 128 clients, on one fresh
# 200,000-item load:
#   1. profiles (AMPHORA_PROFILE, 997 Hz), two rounds, the ways in order then reversed;
#   2. statements a second, ROUNDS rounds, the ways' order rotated each round;
#   3. each way's answers held to SQLite's on the same rows (bench/suite_answers.py).
# The load average and CPU/I-O pressure are recorded before and after every run. One binary, so one
# CPU cost calibration for every way (RULES 5e).
# usage: BIN=<amphora-bench> TOOLS=<dir with q4_shares.py, q4_table.py, suite_answers.py> q4_ways.sh
set -uo pipefail
L=~/.cache/analytics-q4 DATA=~/.cache/analytics-q4/data ROUNDS=${ROUNDS:-5}
ONLY='point_pk$,point_secondary$,point_text$'
ARGS=(--suite --items 200000 --clients 1,32,128 --seconds 1 --warmup 0.5 --only "$ONLY")
WAYS=(locked filtered published optimistic)
export XDG_CACHE_HOME=$L/xdg
rm -rf "$DATA" "$L/prof" && mkdir -p "$L/prof" "$DATA" "$XDG_CACHE_HOME" || exit 1
: > "$L/runs.jsonl"; : > "$L/prof.jsonl"
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
record() {
  python3 -I -c '
import json, sys
lines = sys.stdin.read().strip().splitlines()
r = json.loads(lines[-1]) if lines else {"error": "no output"}
print(json.dumps({"tag": sys.argv[1], "way": sys.argv[2], "state_before": sys.argv[3], "load_after": open("/proc/loadavg").read().split()[:3], "report": r}))' "$@"
}
run() { # out tag way target [extra arguments]
  local out=$1 tag=$2 way=$3 t=$4; shift 4
  local before; before=$(state)
  echo "=== $tag $(date -u +%H:%M:%SZ) $before"
  AMPHORA_LOG_READ_INDEX=$way nice -n 10 "$BIN" "${ARGS[@]}" --target "$t" --dir "$DATA/$t" "$@" 2>>"$L/err.log" | record "$tag" "$way" "$before" >> "$out"
}
run "$L/runs.jsonl" load-amphora locked amphora --clients 1 --seconds 0.2 --warmup 0
run "$L/runs.jsonl" load-sqlite locked sqlite --clients 1 --seconds 0.2 --warmup 0
run "$L/runs.jsonl" sqlite-ref locked sqlite --reuse
for round in 1 2; do
  if [ "$round" = 1 ]; then order=("${WAYS[@]}"); else order=(optimistic published filtered locked); fi
  for way in "${order[@]}"; do
    AMPHORA_PROFILE=$L/prof/$way-$round run "$L/prof.jsonl" "prof-$way-$round" "$way" amphora --reuse
  done
done
python3 -I "$TOOLS/q4_shares.py" "$L/prof"
for round in $(seq 1 "$ROUNDS"); do
  shift_by=$(( (round - 1) % ${#WAYS[@]} ))
  order=("${WAYS[@]:$shift_by}" "${WAYS[@]:0:$shift_by}")
  for way in "${order[@]}"; do
    run "$L/runs.jsonl" "$way-$round" "$way" amphora --reuse
  done
done
python3 -I "$TOOLS/q4_table.py" "$L/runs.jsonl"
for way in "${WAYS[@]}"; do
  python3 -I - "$L/runs.jsonl" "$way" "$L/answers-$way.jsonl" <<'PY'
import json, sys
runs, way, out = sys.argv[1], sys.argv[2], sys.argv[3]
last = {}
for line in open(runs):
    r = json.loads(line)
    if r["tag"] == "sqlite-ref" or (r["way"] == way and r["tag"].startswith(way + "-")):
        last["sqlite" if r["tag"] == "sqlite-ref" else "amphora"] = r["report"]
with open(out, "w") as f:
    for rep in last.values():
        f.write(json.dumps({"report": rep}) + "\n")
PY
  echo "--- answers, $way vs sqlite"
  python3 -I "$TOOLS/suite_answers.py" "$L/answers-$way.jsonl" sqlite; echo "suite_answers exit $?"
done
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
