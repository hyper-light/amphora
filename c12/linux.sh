#!/usr/bin/env bash
# Lane C12's Linux driver (not for main). usage: linux.sh SHA STEP...
set -uo pipefail
SHA=$1; shift
mkdir -p "$HOME/.cache/cargo-home-perf-planner-c12"
for d in registry git bin; do [ -e "$HOME/.cache/cargo-home-perf-planner-c12/$d" ] || ln -s "$HOME/.cargo/$d" "$HOME/.cache/cargo-home-perf-planner-c12/$d"; done
export CARGO_HOME=$HOME/.cache/cargo-home-perf-planner-c12 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=${TARGET:-$HOME/.cache/amphora-perf-target/planner-c12} CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-8}
T=${THREADS:-8}
O=$HOME/.cache/amphora-perf/planner-c12-out; mkdir -p "$O"
R=${REMOTE:-$HOME/.cache/amphora-perf/planner-c12-remote}
[ -d "$R" ] || git clone -q git@github.com:hyper-light/amphora.git "$R"
load() { echo "$(cut -d' ' -f1-3 /proc/loadavg)"; }
cd "$R" || exit 2
if [ "$SHA" != - ]; then
git fetch -q origin 2>&1 | tail -2
git checkout -q -f --detach "$SHA" || exit 3
git show origin/diag/planner-c12:c12/c12_sql.rs > crates/engine/tests/c12_sql.rs
for f in $(git ls-tree --name-only origin/diag/planner-c12 c12/ | grep -E 'sql$|tsv$|py$'); do git show origin/diag/planner-c12:$f > $O/$(basename $f); done
fi
echo "== $(hostname) $(date -u +%T) tree $(git rev-parse --short=8 HEAD) load $(load)"
D=$CARGO_TARGET_DIR/debug/deps
bin() { ls -t "$D"/$1-* | grep -v '\.d$' | head -1; }
for step in "$@"; do
  s=$(date +%s)
  case $step in
    build:*)  # build:TEST TEST ...
      IFS=: read -r _ tests <<< "$step"; args=""; for t in $tests; do args="$args --test $t"; done
      timeout 1500 nice -n 10 cargo test -p amphora-engine --locked --features simulator $args --no-run > "$O/build.log" 2>&1; rc=$?
      grep -E "^(error|warning: unused)|Executable|Finished" -A 8 "$O/build.log" | head -60
      echo "== built rc=$rc in $(( $(date +%s) - s ))s load $(load)" ;;
    sql:*)
      C12_SQL=$O/${step#sql:} timeout 300 "$(bin c12_sql)" --nocapture 2>"$O/sql.err" | cut -c1-${CUT:-400}
      grep -E "panicked|required" "$O/sql.err" | sort | uniq -c | head
      echo "== sql ${step#sql:} $(( $(date +%s) - s ))s" ;;
    sweeps)  # the two sweeps reading their fixtures from $C12_FIXTURE and $C12_FIXTURE_CH, built beside the landed ones
      sed -e 's#include_str!("fixtures/join_where_subqueries.tsv")#Box::leak(std::fs::read_to_string(std::env::var("C12_FIXTURE").unwrap()).unwrap().into_boxed_str())#' -e 's#include_str!("fixtures/outer_join_subqueries.tsv")#Box::leak(std::fs::read_to_string(std::env::var("C12_FIXTURE_CH").unwrap()).unwrap().into_boxed_str())#' crates/engine/tests/outer_join_subqueries.rs > crates/engine/tests/c12_sweep.rs
      timeout 1200 nice -n 10 cargo test -p amphora-engine --locked --features simulator --test c12_sql --test c12_sweep --test outer_join_subqueries --no-run 2>&1 | grep -E "^(error|warning)|Executable|Finished" -A 8 | head -60
      mkdir -p $HOME/.cache/amphora-perf-bin/planner-c12/$(git rev-parse --short=8 HEAD)
      for b in c12_sql c12_sweep outer_join_subqueries; do cp "$(bin $b)" $HOME/.cache/amphora-perf-bin/planner-c12/$(git rev-parse --short=8 HEAD)/$b; done
      echo "== built in $(( $(date +%s) - s ))s load $(load)" ;;
    wide:*|chwide:*)  # wide:COUNT:SEED
      IFS=: read -r kind count seed <<< "$step"
      B=$HOME/.cache/amphora-perf-bin/planner-c12/${ARM:-$(git rev-parse --short=8 HEAD)}
      if [ $kind = wide ]; then gen=bench/sqlite_join_where_subqueries.py; test=joins_filtered; var=C12_FIXTURE; else gen=bench/sqlite_outer_join_subqueries.py; test=outer_joins_with; var=C12_FIXTURE_CH; fi
      python3 $gen "$count" "$seed" > "$O/$kind-$seed.tsv" 2> "$O/$kind-$seed.err"
      env $var=$O/$kind-$seed.tsv timeout ${WIDE_TIMEOUT:-600} nice -n 10 $B/c12_sweep $test > "$O/$kind-$seed.log" 2>&1; rc=$?
      echo "== $kind $count $seed rc=$rc $(( $(date +%s) - s ))s load $(load)"
      grep -E "answers differ|test result" "$O/$kind-$seed.log" | cut -c1-200
      echo "panics: $(grep -c '^    Amphora PANIC' "$O/$kind-$seed.log") errors: $(grep -c '^    Amphora ERROR' "$O/$kind-$seed.log") wrong: $(grep -c '^    Amphora [0-9]' "$O/$kind-$seed.log")"
      grep -E '^    Amphora (PANIC|ERROR)' "$O/$kind-$seed.log" | sort | uniq -c | sort -rn | head -5 ;;
    wides:*|chwides:*)  # wides:COUNT:SEED SEED ... each in a process of its own
      IFS=: read -r kind count seeds <<< "$step"; one=${kind%s}
      for seed in $seeds; do JOBS=1 bash "$0" - "$one:$count:$seed" > "$O/$one-run-$seed.txt" 2>&1 & done; wait
      for seed in $seeds; do grep -E "^== $one|^panics|PANIC|Amphora ERROR|differ" "$O/$one-run-$seed.txt" | cut -c1-220; done ;;
    first:*)  # first:KIND:SEED N: the first N differences of a wide run
      IFS=: read -r _ kind seed count <<< "$step"
      grep -E "^(plain|indexed) Settings" -A 2 "$O/$kind-$seed.log" | head -${count:-30} | cut -c1-900 ;;
    t:*)  # t:TEST [filter]: an integration test's run
      IFS=: read -r _ name filter <<< "$step"
      timeout 1200 nice -n 10 "$(bin $name)" --test-threads=$T $filter > "$O/t-$name.log" 2>&1; rc=$?
      echo "== $name $filter rc=$rc $(( $(date +%s) - s ))s load $(load)"
      grep -E "^test result|FAILED|panicked|answers differ" "$O/t-$name.log" | cut -c1-300 | head -${LINES_MAX:-20} ;;
    *) echo "unknown step $step" ;;
  esac
done
echo "== done $(date -u +%T) load $(load)"
