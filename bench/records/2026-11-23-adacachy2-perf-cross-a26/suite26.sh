#!/usr/bin/env bash
# Lane A26 on adacachy2: the replicated suites of two lib test binaries, the order alternated. usage: suite26.sh OUT MAINBIN FIXBIN ROUNDS FIRST
set -uo pipefail
OUT=${1:?}; M=${2:?}; F=${3:?}; ROUNDS=${4:?}; FIRST=${5:?}; T=${JOBS:-2}
mkdir -p "$OUT"; cd "$HOME/.cache/amphora-perf/cross-a22-wt/crates/amphora" || exit 2
[ -x "$M" ] && [ -x "$F" ] || { echo "no binary"; exit 3; }
stamp() { echo "[$(date -u +%H:%M:%SZ)] $* load=$(cut -d' ' -f1-3 /proc/loadavg) cpu=$(awk 'NR==1{print $2}' /proc/pressure/cpu) io=$(awk 'NR==1{print $2}' /proc/pressure/io)"; }
for r in $(seq "$FIRST" $(( FIRST + ROUNDS - 1 ))); do
  if [ $((r % 2)) -eq 1 ]; then order="main fix"; else order="fix main"; fi
  for arm in $order; do
    bin=$M; [ "$arm" = fix ] && bin=$F
    s=$(date +%s)
    timeout 1500 nice -n 10 "$bin" --test-threads="$T" database::replicated:: replication:: > "$OUT/r$r-$arm.log" 2>&1; rc=$?
    stamp "r$r $arm rc=$rc $(( $(date +%s) - s ))s: $(grep 'test result' "$OUT/r$r-$arm.log" | tail -1)"
    grep -E '^test .* FAILED$' "$OUT/r$r-$arm.log" | head -14
  done
done
stamp end
