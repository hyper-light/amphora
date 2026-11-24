#!/usr/bin/env bash
# Lane C10's final checks on Linux (not for main). usage: final.sh SHA STEP...
set -uo pipefail
SHA=$1; shift
export CARGO_HOME=$HOME/.cache/cargo-home-perf-planner-c10 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/planner-c10 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-4}
O=$HOME/.cache/amphora-perf/planner-c10-out; mkdir -p "$O"
cd "$HOME/.cache/amphora-perf/planner-c10-remote" || exit 2
git fetch -q origin 2>&1 | tail -2
rm -f crates/engine/tests/c10_sql.rs crates/engine/tests/c10_sweep.rs
git checkout -q --detach "$SHA" || exit 3
D=$CARGO_TARGET_DIR/debug/deps
bin() { ls -t "$D"/$1-* | grep -v '\.d$' | head -1; }
echo "== $(hostname) $(date -u +%T) tree $(git rev-parse --short=8 HEAD) load $(cut -d' ' -f1-3 /proc/loadavg)"
run() { # run NAME BINARY ARGS...
  local name=$1 b=$2; shift 2; local s=$(date +%s)
  timeout 900 nice -n 10 "$b" --test-threads=4 "$@" > "$O/t-$name.log" 2>&1; local rc=$?
  echo "== $name rc=$rc $(( $(date +%s) - s ))s $(grep 'test result' "$O/t-$name.log" | tail -1 | cut -c1-110)"
  grep -E '^test .* FAILED$' "$O/t-$name.log" | head; [ $rc = 0 ] || grep -E "panicked at|answers differ" -A 6 "$O/t-$name.log" | head -30
}
for step in "$@"; do
  s=$(date +%s)
  case $step in
    build) timeout 1200 nice -n 10 cargo test -p amphora-engine --locked --features simulator --lib --test oracle_findings --test outer_join_subqueries --test in_subquery --test join_methods --test adaptive_joins --test unnesting --test hash_semi_join --test planner --no-run > "$O/build.log" 2>&1; echo "== build rc=$? $(( $(date +%s) - s ))s"; grep -E "^(error|warning)" -A 12 "$O/build.log" | head -60 ;;
    mine) run oracle_findings "$(bin oracle_findings)"; run outer_join_subqueries "$(bin outer_join_subqueries)" ;;
    suites)
      for t in in_subquery join_methods adaptive_joins unnesting hash_semi_join planner; do run $t "$(bin $t)"; done
      run lib-subquery "$(bin amphora_engine)" subquery
      run lib-optimizer "$(bin amphora_engine)" optimizer:: --skip t_pl_8_13
      run lib-c6-small "$(bin amphora_engine)" t_pl_8_13_outer_joins_answer_as_sqlite_small ;;
    clippy) timeout 1200 nice -n 10 cargo clippy -p amphora-engine --all-targets --locked -- -D warnings > "$O/clippy.log" 2>&1; echo "== clippy rc=$? $(( $(date +%s) - s ))s $(grep -c 'Checking amphora-engine ' "$O/clippy.log") $(grep 'Checking amphora-engine ' "$O/clippy.log" | head -1)"; grep -E "^(error|warning)" -A 14 "$O/clippy.log" | head -60 ;;
    production) timeout 1500 nice -n 10 bash scripts/check-production.sh > "$O/production.log" 2>&1; echo "== check-production rc=$? $(( $(date +%s) - s ))s"; tail -8 "$O/production.log" ;;
    fmt) cargo fmt --all --check > "$O/fmt.log" 2>&1; echo "== fmt rc=$?"; head -20 "$O/fmt.log" ;;
    wait) wait ;;
  esac
done
echo "== done $(date -u +%T) load $(cut -d' ' -f1-3 /proc/loadavg)"
