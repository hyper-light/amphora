#!/usr/bin/env bash
# Lane A24 on adacachy2: the replicated suites (database::replicated:: replication::) ROUNDS rounds a side, the order
# alternated each round, held to $JOBS test threads; a stream START of 2 begins with side B. Each run logs to OUT and
# prints its result, its FAILED tests and the r5 stamps. usage: a24suite.sh ROUNDS OUT SHA_A SHA_B START
set -uo pipefail
ROUNDS=${1:?}; OUT=${2:?}; A=${3:?}; B=${4:?}; START=${5:-1}; T=${JOBS:-8}
BIN=$HOME/.cache/amphora-perf-bin/cross; mkdir -p "$OUT"
stamp() { echo "[$(date -u +%H:%M:%SZ)] $* load=$(cut -d" " -f1-3 /proc/loadavg) cpu=$(awk "NR==1{print \$2}" /proc/pressure/cpu) io=$(awk "NR==1{print \$2}" /proc/pressure/io)"; }
one() {
  local sha=$1 label=$2 s out rc; s=$(date +%s)
  out=$(nice -n 10 "$BIN/a24-tests-$sha" --test-threads="$T" --show-output database::replicated:: replication:: 2>&1); rc=$?
  echo "$out" > "$OUT/$label-$sha.log"
  stamp "$label $sha rc=$rc $(( $(date +%s) - s ))s: $(echo "$out" | grep "^test result" | tail -1 | cut -c1-70)"
  echo "$out" | grep -E "^test .* FAILED$" | sed -E "s/^test (database::)?//; s/ \.\.\. FAILED//" | head -14
  echo "$out" | grep -E "a24-r5" | head -2
}
for r in $(seq 1 "$ROUNDS"); do
  if [ $(( (r + START) % 2 )) -eq 0 ]; then order="$A $B"; else order="$B $A"; fi
  for S in $order; do one "$S" "s$START-r$r"; done
done
stamp "end"
