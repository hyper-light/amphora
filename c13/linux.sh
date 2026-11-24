#!/usr/bin/env bash
# Lane C13's Linux driver (not for main). usage: linux.sh SHA|- STEP...
set -uo pipefail
SHA=$1; shift
export CARGO_HOME=$HOME/.cache/cargo-home-perf-planner-c13 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/planner-c13 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-8}
O=$HOME/.cache/amphora-perf/planner-c13-out; mkdir -p "$O"
load() { cut -d' ' -f1-3 /proc/loadavg; }
cd "$HOME/.cache/amphora-perf/planner-c13-remote" || exit 2
if [ "$SHA" != - ]; then
  git fetch -q origin 2>&1 | tail -2
  git checkout -q -f --detach "$SHA" || exit 3
  cp "$O/c13_sql.rs" crates/engine/tests/c13_sql.rs 2>/dev/null
fi
echo "== $(hostname) $(date -u +%T) tree $(git rev-parse --short=8 HEAD) load $(load)"
D=$CARGO_TARGET_DIR/debug/deps
bin() { ls -t "$D"/$1-* | grep -v '\.d$' | head -1; }
for step in "$@"; do
  s=$(date +%s)
  case $step in
    build:*)  # build:TEST,TEST
      tests=""; IFS=, read -ra names <<< "${step#build:}"; for n in "${names[@]}"; do tests="$tests --test $n"; done
      timeout 900 nice -n 10 cargo test -p amphora-engine --locked --features simulator $tests --no-run 2>&1 | grep -E "^(error|warning)|Executable|Finished" -A 8 | head -80
      echo "== built in $(( $(date +%s) - s ))s" ;;
    sweep)  # the sweep's test, and the same reading its fixture from $C13_FIXTURE
      sed -e 's#include_str!("fixtures/on_ctes.tsv")#Box::leak(std::fs::read_to_string(std::env::var("C13_FIXTURE").unwrap()).unwrap().into_boxed_str())#' crates/engine/tests/outer_join_subqueries.rs > crates/engine/tests/c13_sweep.rs
      timeout 900 nice -n 10 cargo test -p amphora-engine --locked --features simulator --test c13_sweep --test outer_join_subqueries --no-run 2>&1 | grep -E "^(error|warning)|Executable|Finished" -A 8 | head -60
      echo "== built in $(( $(date +%s) - s ))s" ;;
    wide:*)  # wide:COUNT:SEED
      IFS=: read -r _ count seed <<< "$step"
      python3 bench/sqlite_on_ctes.py "$count" "$seed" > "$O/wide-$seed.tsv" 2> "$O/wide-$seed.err"
      C13_FIXTURE=$O/wide-$seed.tsv timeout ${WIDE_TIMEOUT:-420} nice -n 10 "$(bin c13_sweep)" ${WIDE_FILTER:-on_clauses} > "$O/wide-$seed.log" 2>&1; rc=$?
      echo "== wide $count $seed rc=$rc $(( $(date +%s) - s ))s load $(load)"
      grep -E "answers differ|test result" "$O/wide-$seed.log" | cut -c1-200
      echo "panics: $(grep -c '^    Amphora PANIC' "$O/wide-$seed.log") errors: $(grep -c '^    Amphora ERROR' "$O/wide-$seed.log") wrong: $(grep -c '^    Amphora [0-9]' "$O/wide-$seed.log")"
      grep -E '^    Amphora (PANIC|ERROR)' "$O/wide-$seed.log" | sort | uniq -c | sort -rn | head -5 ;;
    wides:*)  # wides:COUNT:SEED SEED ... each in a process of its own
      IFS=: read -r _ count seeds <<< "$step"
      for seed in $seeds; do JOBS=1 bash "$0" - "wide:$count:$seed" > "$O/wide-run-$seed.txt" 2>&1 & done; wait
      for seed in $seeds; do grep -E "^== wide|panics|PANIC|Amphora Err|differ|ERROR" "$O/wide-run-$seed.txt" | cut -c1-200; done ;;
    sql:*)
      C13_SQL=$O/${step#sql:} timeout 120 "$(bin c13_sql)" --nocapture 2>"$O/sql.err" | cut -c1-600
      grep -E "panicked|required" "$O/sql.err" | sort | uniq -c | head ;;
    test:*)  # test:NAME the built binary
      timeout 900 nice -n 10 "$(bin "${step#test:}")" > "$O/test-${step#test:}.log" 2>&1; rc=$?
      echo "== test ${step#test:} rc=$rc $(( $(date +%s) - s ))s"; grep -E "test result|panicked|answers differ|FAILED|failed$" "$O/test-${step#test:}.log" | head -12 ;;
  esac
done
echo "== done $(date -u +%T) load $(load)"
