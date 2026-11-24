#!/usr/bin/env bash
# Lane C9: the checks of one tree. usage: check.sh <ref> <name> <jobs> <steps...>
set -u
export CARGO_HOME=$HOME/.cache/cargo-home-perf-planner CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0
export CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/planner-c6 CARGO_BUILD_JOBS=$3
OUT=$HOME/.cache/amphora-perf/planner-c9-out; BIN=$HOME/.cache/amphora-perf-bin/planner-c9/$2; J=$3
mkdir -p "$OUT" "$BIN"
ref=$1 name=$2; shift 3
cd ~/.cache/amphora-perf/planner-c9-remote || exit 2
git reset -q --hard; git checkout -q --detach "$ref" -- || exit 2
git log --oneline -1
for step in "$@"; do
  case $step in
  clippy) nice -n 10 timeout 900 cargo clippy -p amphora-engine --all-targets --locked -- -D warnings > "$OUT/clippy-$name.log" 2>&1; echo "clippy $? $(date -u +%T)"; grep -E "(Checking|Compiling) amphora-engine " "$OUT/clippy-$name.log"; tail -2 "$OUT/clippy-$name.log";;
  production) nice -n 10 timeout 900 bash scripts/check-production.sh > "$OUT/production-$name.log" 2>&1; echo "check-production $? $(date -u +%T)"; tail -3 "$OUT/production-$name.log";;
  build) nice -n 10 timeout 900 cargo test -p amphora-engine --locked --features simulator --lib --test on_terms --test oracle_findings --test join_methods --test adaptive_joins --test planner --test sorted_fetch --no-run > "$OUT/build-$name.log" 2>&1
    echo "build $? $(date -u +%T)"; grep -E "^error|Compiling amphora-engine " "$OUT/build-$name.log"
    for t in amphora_engine on_terms oracle_findings join_methods adaptive_joins planner sorted_fetch; do f=$(grep -o "/[^ )]*/$t-[0-9a-f]*" "$OUT/build-$name.log" | tail -1); cp "$f" "$BIN/$t" || exit 4; done;;
  integration) for t in on_terms oracle_findings join_methods adaptive_joins planner sorted_fetch; do (cd crates/engine && nice -n 10 timeout 600 "$BIN/$t" --test-threads=$J > "$OUT/$t-$name.log" 2>&1); echo "$t $? $(date -u +%T)"; grep -E "test result|FAILED|panicked" "$OUT/$t-$name.log" | head -6; done;;
  optimizer) (cd crates/engine && nice -n 10 timeout 900 "$BIN/amphora_engine" translate::optimizer --skip outer_joins_answer_as_sqlite --test-threads=$J > "$OUT/optimizer-$name.log" 2>&1); echo "optimizer $? $(date -u +%T)"; grep -E "test result|FAILED|panicked" "$OUT/optimizer-$name.log" | head -12;;
  sweep) (cd crates/engine && nice -n 10 timeout 900 "$BIN/amphora_engine" outer_joins_answer_as_sqlite_small --test-threads=$J > "$OUT/sweep-$name.log" 2>&1); echo "C6's small sweep $? $(date -u +%T)"; grep -E "test result|FAILED|panicked|running" "$OUT/sweep-$name.log" | head -12;;
  esac
done
