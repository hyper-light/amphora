#!/usr/bin/env bash
# Lane V13: the red commit's and the fix's amphora lib tests, built in turn in one target dir; the
# focused tests on each, clippy on the fix, then the replicated suites ROUNDS rounds, the arm order
# alternated (the red commit's product is main's). usage: tests13.sh RED FIX [ROUNDS] (in a clone,
# CARGO_* set; THREADS defaults to 4)
set -uo pipefail
RED=${1:?}; FIX=${2:?}; ROUNDS=${3:-3}; T=${THREADS:-4}
B=$HOME/.cache/amphora-perf-bin/commits; mkdir -p "$B"
for sha in "$RED" "$FIX"; do
  git checkout -q --detach "$sha" || exit 3
  s=$(date +%s); out=$(cargo test -p amphora --lib --locked --no-run 2>&1)
  bin=$(echo "$out" | grep -o '/[^ )]*/debug/deps/amphora-[0-9a-f]*' | tail -1)
  echo "== built $sha in $(( $(date +%s) - s ))s: $bin"
  [ -x "$bin" ] || { echo "$out" | tail -20; exit 4; }
  cp "$bin" "$B/v13-tests-$sha"
done
for sha in "$RED" "$FIX"; do
  echo "== focused $sha $(date -u +%T) load=$(cut -d' ' -f1-3 /proc/loadavg)"
  "$B/v13-tests-$sha" --test-threads="$T" database::slot:: v13_thirty_two v7_a_hundred_and_twenty_eight 2>&1 \
    | grep -E 'FAILED$|test result|stale-base refusals|panicked at|left:|right:' | head -40
done
git checkout -q --detach "$FIX"
echo "== clippy $FIX"; cargo clippy -p amphora --all-targets --locked -- -D warnings 2>&1 | tail -3; echo "clippy exit ${PIPESTATUS[0]}"
for r in $(seq 1 "$ROUNDS"); do
  if [ $((r % 2)) -eq 1 ]; then arms="$FIX $RED"; else arms="$RED $FIX"; fi
  for sha in $arms; do
    s=$(date +%s); out=$("$B/v13-tests-$sha" --test-threads="$T" database::replicated:: replication:: 2>&1)
    echo "== suite r$r $sha $(( $(date +%s) - s ))s load=$(cut -d' ' -f1-3 /proc/loadavg): $(echo "$out" | grep 'test result' | tail -1)"
    echo "$out" | grep -E '^test .* FAILED$' | head -20
  done
done
