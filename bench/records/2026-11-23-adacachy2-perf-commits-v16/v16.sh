#!/usr/bin/env bash
# Lane V16: build RED and FIX lib tests in turn in the lane's one target dir, the focused tests on
# each, then (ROUNDS > 0) clippy on FIX and the replicated suites ROUNDS rounds, arm order
# alternated. Runs from the repository it sits in. usage: v16.sh RED FIX ROUNDS OUT THREADS
set -uo pipefail
RED=${1:?}; FIX=${2:?}; ROUNDS=${3:-3}; OUT=${4:?}; T=${5:-8}
load() { if [ -r /proc/loadavg ]; then cut -d' ' -f1-3 /proc/loadavg; else sysctl -n vm.loadavg; fi; }
B=$HOME/.cache/amphora-perf-bin/commits; mkdir -p "$B" "$OUT"
for sha in "$RED" "$FIX"; do
  git checkout -q --detach "$sha" || exit 3
  s=$(date +%s); out=$(nice -n 10 cargo test -p amphora --lib --locked --no-run 2>&1)
  bin=$(echo "$out" | grep -o '/[^ )]*/debug/deps/amphora-[0-9a-f]*' | tail -1)
  echo "== built $sha in $(( $(date +%s) - s ))s load=$(load)"
  [ -x "$bin" ] || { echo "$out" | grep -E "^(error|warning)" -A 14 | head -90; exit 4; }
  cp "$bin" "$B/v16-tests-$sha"
done
for sha in "$RED" "$FIX"; do
  echo "== focused $sha $(date -u +%T) load=$(load)"
  "$B/v16-tests-$sha" --test-threads="$T" v15_a_write_its_deadline database::slot:: v14_a_slow a_slot_holder_whose_write_fails 2>&1 \
    | grep -E 'FAILED$|test result|panicked at|left:|right:' | head -40
done
[ "$ROUNDS" -gt 0 ] || exit 0
git checkout -q --detach "$FIX"
echo "== clippy $FIX $(date -u +%T)"; nice -n 10 cargo clippy -p amphora --all-targets --locked -- -D warnings 2>&1 | grep -E "^(error|warning)" -A 10 | head -60; echo "clippy exit ${PIPESTATUS[0]}"
for r in $(seq 1 "$ROUNDS"); do
  if [ $((r % 2)) -eq 1 ]; then arms="$FIX $RED"; else arms="$RED $FIX"; fi
  for sha in $arms; do
    s=$(date +%s); out=$("$B/v16-tests-$sha" --test-threads="$T" database::replicated:: replication:: 2>&1)
    echo "$out" > "$OUT/suite-r$r-$sha.log"
    echo "== suite r$r $sha $(( $(date +%s) - s ))s load=$(load): $(echo "$out" | grep 'test result' | tail -1)"
    echo "$out" | grep -E '^test .* FAILED$' | head -20
  done
done
