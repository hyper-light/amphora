#!/usr/bin/env bash
# Lane C9: builds and runs one arm. usage: arm.sh <sha or ref> <name> [jobs]
set -u
export CARGO_HOME=$HOME/.cache/cargo-home-perf-planner CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0
export CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/planner-c6 CARGO_BUILD_JOBS=${3:-4}
OUT=$HOME/.cache/amphora-perf/planner-c9-out; BIN=$HOME/.cache/amphora-perf-bin/planner-c9/$2
mkdir -p "$OUT" "$BIN"
cd ~/.cache/amphora-perf/planner-c9-remote || exit 2
git checkout -q --detach "$1" -- || exit 2
git checkout -q origin/perf/planner-c9-probe -- crates/engine/tests/c9_probe.rs || exit 2
git log --oneline -1
nice -n 10 timeout 900 cargo test -p amphora-engine --locked --features simulator --test on_terms --test c9_probe --test oracle_findings --no-run > "$OUT/build-$2.log" 2>&1
code=$?; echo "build $code $(date -u +%T)"; [ $code = 0 ] || { tail -30 "$OUT/build-$2.log"; exit 3; }
for t in on_terms c9_probe oracle_findings; do
  f=$(grep -o "/[^ )]*/$t-[0-9a-f]*" "$OUT/build-$2.log" | tail -1); cp "$f" "$BIN/$t" || exit 4
done
git reset -q; rm -f crates/engine/tests/c9_probe.rs
nice -n 10 timeout 300 "$BIN/on_terms" --nocapture --test-threads=3 > "$OUT/on_terms-$2.log" 2>&1; echo "on_terms $? $(date -u +%T)"
grep -E "queries,|test result" "$OUT/on_terms-$2.log" | sort; echo "wrong lines: $(grep -c '^WRONG' "$OUT/on_terms-$2.log")"
