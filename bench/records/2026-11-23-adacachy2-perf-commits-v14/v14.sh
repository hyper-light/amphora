#!/usr/bin/env bash
# Lane V14 on adacachy2: build RED and FIX lib tests in turn in the lane's target dir, the focused
# tests on each, clippy on FIX, the fix's release bench and V13's ladder (3 rounds), then the
# replicated suites ROUNDS rounds, arm order alternated. usage: v14.sh RED FIX ROUNDS OUT
set -uo pipefail
RED=${1:?}; FIX=${2:?}; ROUNDS=${3:-3}; OUT=${4:?}; T=8
R=$HOME/.cache/amphora-perf/commits-remote
cd "$R" || exit 2
git fetch -q origin || exit 3
export CARGO_HOME=$HOME/.cache/cargo-home-perf-commits CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/commits CARGO_BUILD_JOBS=8 CARGO_INCREMENTAL=0
B=$HOME/.cache/amphora-perf-bin/commits; mkdir -p "$B" "$OUT"
for sha in "$RED" "$FIX"; do
  git checkout -q --detach "$sha" || exit 3
  s=$(date +%s); out=$(nice -n 10 cargo test -p amphora --lib --locked --no-run 2>&1)
  bin=$(echo "$out" | grep -o '/[^ )]*/debug/deps/amphora-[0-9a-f]*' | tail -1)
  echo "== built $sha in $(( $(date +%s) - s ))s load=$(cut -d' ' -f1-3 /proc/loadavg)"
  [ -x "$bin" ] || { echo "$out" | grep -E "^(error|warning)" -A 14 | head -90; exit 4; }
  cp "$bin" "$B/v14-tests-$sha"
done
for sha in "$RED" "$FIX"; do
  echo "== focused $sha $(date -u +%T) load=$(cut -d' ' -f1-3 /proc/loadavg)"
  "$B/v14-tests-$sha" --test-threads="$T" database::slot:: v14_a_slow v13_thirty_two v7_a_hundred_and_twenty_eight 2>&1 \
    | grep -E 'FAILED$|test result|stale-base refusals|panicked at|left:|right:|Persisted|Replicated' | head -40
done
git checkout -q --detach "$FIX"
echo "== clippy $FIX $(date -u +%T)"; nice -n 10 cargo clippy -p amphora --all-targets --locked -- -D warnings 2>&1 | grep -E "^(error|warning)" -A 10 | head -60; echo "clippy exit ${PIPESTATUS[0]}"
s=$(date +%s); (cd bench && nice -n 10 cargo build --release --locked 2>&1 | tail -2); echo "== bench built in $(( $(date +%s) - s ))s"
cp "$CARGO_TARGET_DIR/release/amphora-bench" "$B/v14-$FIX" || exit 5
echo "== ladder $(date -u +%T)"
bash bench/records/2026-11-23-adacachy2-perf-commits-v13/ladder13.sh "$B/v14-$FIX" "$OUT/ladder" 3
for r in $(seq 1 "$ROUNDS"); do
  if [ $((r % 2)) -eq 1 ]; then arms="$FIX $RED"; else arms="$RED $FIX"; fi
  for sha in $arms; do
    s=$(date +%s); out=$("$B/v14-tests-$sha" --test-threads="$T" database::replicated:: replication:: 2>&1)
    echo "$out" > "$OUT/suite-r$r-$sha.log"
    echo "== suite r$r $sha $(( $(date +%s) - s ))s load=$(cut -d' ' -f1-3 /proc/loadavg): $(echo "$out" | grep 'test result' | tail -1)"
    echo "$out" | grep -E '^test .* FAILED$' | head -20
  done
done
