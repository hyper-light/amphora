#!/usr/bin/env bash
# The mixed log (decisions 37 and 39, what Amphora ships) against the page-a-frame log
# (AMPHORA_PAGE_LOG=1), in one binary, interleaved: each round runs each statement on both logs back
# to back, the order turned each round (ABBA), so both see the same load. Each run is its own process
# on a fresh database, so a run's counters are its statement's. Where the host counts them, each
# process's instructions retired go beside its record (`perf stat` on Linux, `/usr/bin/time -l` on
# macOS); they count the load too, which is the same rows on both logs. Records go to $AB_OUT as JSON
# lines; a table of the best of the rounds follows (bench/log_ab.py).
#
# usage: bench/log_ab.sh <rounds> <statement[,statement...]> [suite arguments...]
#   e.g. bench/log_ab.sh 3 insert_batch_100,delete_range_insert --items 200000 --clients 1,32 --seconds 5 --warmup 1
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
rounds="$1"
statements="$2"
shift 2
bin="${CARGO_TARGET_DIR:-target}/release/amphora-bench"
data="${AMPHORA_BENCH_DATA:-$HOME/.cache/amphora-bench-data}/log-ab-$$"
out="${AB_OUT:-records/$(date -u +%Y-%m-%d)-$(hostname -s)-log-ab.jsonl}"
mkdir -p "$data" "$(dirname -- "$out")"
trap 'rm -rf -- "${data:?}"' EXIT
commit=$(git rev-parse HEAD 2>/dev/null || echo unknown)
# One run: $1 the log (pages or mixed), $2 the statement, the rest the suite's arguments.
one() {
  local log="$1" stmt="$2" counts="$data/counts" line instr
  shift 2
  rm -rf -- "${data:?}/db"
  mkdir -p "$data/db"
  local env=(env)
  [[ "$log" == pages ]] && env+=(AMPHORA_PAGE_LOG=1)
  if [[ "$(uname -s)" == Darwin ]]; then
    line=$("${env[@]}" /usr/bin/time -l -o "$counts" "$bin" --suite --target amphora --dir "$data/db" --only "$stmt" "$@" 2>/dev/null)
    instr=$(awk '/instructions retired/ { print $1 }' "$counts")
  elif command -v perf >/dev/null; then
    line=$("${env[@]}" perf stat -x, -e instructions:u -o "$counts" "$bin" --suite --target amphora --dir "$data/db" --only "$stmt" "$@" 2>/dev/null)
    instr=$(awk -F, '/instructions/ { print $1 }' "$counts")
  else
    line=$("${env[@]}" "$bin" --suite --target amphora --dir "$data/db" --only "$stmt" "$@" 2>/dev/null)
    instr=""
  fi
  python3 -c 'import json, sys
r = json.loads(sys.argv[1]); r.update(log=sys.argv[2], round=int(sys.argv[3]), commit=sys.argv[4], host=sys.argv[5])
r["instructions"] = int(sys.argv[6]) if sys.argv[6].isdigit() else None
print(json.dumps(r))' "$line" "$log" "$round" "$commit" "$(hostname -s)" "${instr:-}" >>"$out"
}
for round in $(seq 1 "$rounds"); do
  if ((round % 2)); then order="pages mixed"; else order="mixed pages"; fi
  for stmt in ${statements//,/ }; do
    for log in $order; do
      echo "== round $round $stmt $log $(date -u +%T)" >&2
      one "$log" "$stmt" "$@"
    done
  done
done
python3 log_ab.py "$out"
