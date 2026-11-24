#!/usr/bin/env bash
# Lane C10's Linux driver (not for main). usage: linux.sh SHA STEP...
set -uo pipefail
SHA=$1; shift
export CARGO_HOME=$HOME/.cache/cargo-home-perf-planner-c10 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/planner-c10 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-8}
O=$HOME/.cache/amphora-perf/planner-c10-out; mkdir -p "$O"
load() { echo "$(cut -d' ' -f1-3 /proc/loadavg)"; }
cd "$HOME/.cache/amphora-perf/planner-c10-remote" || exit 2
if [ "$SHA" != - ]; then
git fetch -q origin 2>&1 | tail -2
git checkout -q --detach "$SHA" || exit 3
git show origin/diag/planner-c10:c10/c10_sql.rs > crates/engine/tests/c10_sql.rs
for f in $(git ls-tree --name-only origin/diag/planner-c10 c10/ | grep sql$); do git show origin/diag/planner-c10:$f > $O/$(basename $f); done
fi
echo "== $(hostname) $(date -u +%T) tree $(git rev-parse --short=8 HEAD) load $(load)"
D=$CARGO_TARGET_DIR/debug/deps
bin() { ls -t "$D"/$1-* | grep -v '\.d$' | head -1; }
for step in "$@"; do
  s=$(date +%s)
  case $step in
    sweeps)  # the sweep's test, and the same reading its fixture from $C10_FIXTURE
      sed -e 's#include_str!("fixtures/outer_join_subqueries.tsv").lines()#Box::leak(std::fs::read_to_string(std::env::var("C10_FIXTURE").unwrap()).unwrap().into_boxed_str()).lines()#' crates/engine/tests/outer_join_subqueries.rs > crates/engine/tests/c10_sweep.rs
      timeout 1200 nice -n 10 cargo test -p amphora-engine --locked --features simulator --test c10_sql --test c10_sweep --test outer_join_subqueries --no-run 2>&1 | grep -E "^(error|warning)|Executable|Finished" -A 8 | head -60
      echo "== built in $(( $(date +%s) - s ))s" ;;
    wide:*)  # wide:COUNT:SEED
      IFS=: read -r _ count seed <<< "$step"
      python3 bench/sqlite_outer_join_subqueries.py "$count" "$seed" > "$O/wide-$seed.tsv" 2> "$O/wide-$seed.err"
      C10_FIXTURE=$O/wide-$seed.tsv timeout ${WIDE_TIMEOUT:-600} nice -n 10 "$(bin c10_sweep)" > "$O/wide-$seed.log" 2>&1; rc=$?
      echo "== wide $count $seed rc=$rc $(( $(date +%s) - s ))s load $(load)"
      grep -E "answers differ|test result" "$O/wide-$seed.log" | cut -c1-200
      echo "panics: $(grep -c '^    PANIC' "$O/wide-$seed.log") errors: $(grep -c '^    Amphora Err' "$O/wide-$seed.log") wrong: $(grep -c '^    Amphora Ok' "$O/wide-$seed.log")"
      grep '^    PANIC' "$O/wide-$seed.log" | sort | uniq -c | sort -rn | head -5
      grep '^    Amphora Err' "$O/wide-$seed.log" | sort | uniq -c | sort -rn | head -5 ;;
    fixture)
      timeout 900 nice -n 10 "$(bin outer_join_subqueries)" > "$O/fixture.log" 2>&1; rc=$?
      echo "== fixture rc=$rc $(( $(date +%s) - s ))s load $(load)"
      grep -E "answers differ|test result" "$O/fixture.log" | cut -c1-200
      echo "panics: $(grep -c '^    PANIC' "$O/fixture.log") errors: $(grep -c '^    Amphora Err' "$O/fixture.log") wrong: $(grep -c '^    Amphora Ok' "$O/fixture.log")" ;;
    wides:*)  # wides:COUNT:SEED SEED ... each in a process of its own
      IFS=: read -r _ count seeds <<< "$step"
      for seed in $seeds; do JOBS=1 bash "$0" - "wide:$count:$seed" > "$O/wide-run-$seed.txt" 2>&1 & done; wait
      for seed in $seeds; do grep -E "^== wide|panics|PANIC|Amphora Err|differ" "$O/wide-run-$seed.txt" | cut -c1-200; done ;;
    sql:*)
      C10_SQL=$O/${step#sql:} timeout 120 "$(bin c10_sql)" --nocapture 2>"$O/sql.err" | cut -c1-400
      grep -E "panicked|required" "$O/sql.err" | sort | uniq -c | head ;;
  esac
done
echo "== done $(date -u +%T) load $(load)"
