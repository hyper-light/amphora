#!/usr/bin/env bash
# Lane C18's Linux driver (not for main). usage: linux.sh SHA|- STEP...
set -uo pipefail
SHA=$1; shift
export CARGO_HOME=$HOME/.cache/cargo-home-perf-planner-c13 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/planner-c13 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-8}
O=$HOME/.cache/amphora-perf/planner-c18-out; mkdir -p "$O"
load() { cut -d' ' -f1-3 /proc/loadavg; }
R=$HOME/.cache/amphora-perf/planner-c18-remote
[ -d "$R" ] || git clone -q --reference "$HOME/.cache/amphora-perf/planner-c13-remote" git@github.com:hyper-light/amphora.git "$R" || exit 2
cd "$R" || exit 2
if [ "$SHA" != - ]; then
  git fetch -q origin 2>&1 | tail -2
  git checkout -q -f --detach "$SHA" || exit 3
  git clean -q -fd crates bench
  cp "$O/c18_sql.rs" crates/engine/tests/c18_sql.rs 2>/dev/null
fi
echo "== $(hostname) $(date -u +%T) tree $(git rev-parse --short=8 HEAD) load $(load)"
D=$CARGO_TARGET_DIR/debug/deps
bin() { ls -t "$D"/$1-* | grep -v '\.d$' | head -1; }
for step in "$@"; do
  s=$(date +%s)
  case $step in
    fmt) cargo fmt --all --check > "$O/fmt.log" 2>&1; echo "== fmt rc=$?" ;;
    clippy)
      timeout 900 nice -n 10 cargo clippy -p amphora-engine --all-targets --locked -- -D warnings > "$O/clippy.log" 2>&1; rc=$?
      echo "== clippy rc=$rc $(( $(date +%s) - s ))s"; grep -E "^(error|warning)|Checking amphora-engine|Finished" -A 6 "$O/clippy.log" | head -60 ;;
    benchclippy)
      timeout 900 nice -n 10 cargo clippy --manifest-path bench/Cargo.toml --all-targets -- -D warnings > "$O/benchclippy.log" 2>&1; echo "== benchclippy rc=$? $(( $(date +%s) - s ))s" ;;
    prod)
      timeout 1500 nice -n 10 bash scripts/check-production.sh > "$O/prod.log" 2>&1; rc=$?
      echo "== check-production rc=$rc $(( $(date +%s) - s ))s"; tail -8 "$O/prod.log" | cut -c1-300 ;;
    build:*)
      tests=""; IFS=, read -ra names <<< "${step#build:}"; for n in "${names[@]}"; do tests="$tests --test $n"; done
      timeout 900 nice -n 10 cargo test -p amphora-engine --locked --features simulator $tests --no-run 2>&1 | grep -E "^(error|warning)|Executable|Finished" -A 8 | head -80
      rc=${PIPESTATUS[0]}; echo "== built rc=$rc in $(( $(date +%s) - s ))s"; [ "$rc" = 0 ] || { echo "== BUILD FAILED, nothing run"; exit 4; } ;;
    sql:*)
      C18_SQL=$O/${step#sql:} timeout 300 "$(bin c18_sql)" --nocapture 2>"$O/sql.err" | cut -c1-900
      grep -E "panicked|required" "$O/sql.err" | sort | uniq -c | head ;;
    test:*)  # test:NAME[:FILTER]
      IFS=: read -r _ name filter <<< "$step"
      timeout 900 nice -n 10 "$(bin "$name")" $filter > "$O/test-$name.log" 2>&1; rc=$?
      echo "== test $name $filter rc=$rc $(( $(date +%s) - s ))s"; grep -E "test result|panicked|answers differ|FAILED|failed$" "$O/test-$name.log" | head -12 ;;
    sweeps)  # the two sweeps' tests, reading their fixtures from files
      rm -f "$D"/c13_sweep-* "$D"/c18_sweep-*
      sed -e 's#include_str!("fixtures/on_ctes.tsv")#Box::leak(std::fs::read_to_string(std::env::var("C18_FIXTURE").unwrap()).unwrap().into_boxed_str())#' crates/engine/tests/outer_join_subqueries.rs > crates/engine/tests/c13_sweep.rs
      sed -e 's#include_str!("fixtures/cte_scopes.tsv")#(Box::leak(std::fs::read_to_string(std::env::var("C18_FIXTURE").unwrap()).unwrap().into_boxed_str()) as \&str)#' crates/engine/tests/cte_scopes.rs > crates/engine/tests/c18_sweep.rs
      timeout 900 nice -n 10 cargo test -p amphora-engine --locked --features simulator --test c13_sweep --test c18_sweep --no-run 2>&1 | grep -E "^(error|warning)|Executable|Finished" -A 8 | head -40
      rc=${PIPESTATUS[0]}; echo "== sweeps built rc=$rc in $(( $(date +%s) - s ))s"; [ "$rc" = 0 ] || exit 4 ;;
    wides13:*|wides18:*)  # wides13:COUNT:SEED SEED ... each in a process of its own
      IFS=: read -r which count seeds <<< "$step"; n=${which#wides}
      gen=bench/sqlite_on_ctes.py; filter=on_clauses; [ "$n" = 18 ] && { gen=bench/sqlite_cte_scopes.py; filter=ctes_of; }
      for seed in $seeds; do (
        python3 $gen "$count" "$seed" > "$O/wide$n-$seed.tsv" 2> "$O/wide$n-$seed.err"
        C18_FIXTURE=$O/wide$n-$seed.tsv timeout ${WIDE_TIMEOUT:-420} nice -n 10 "$(bin c${n}_sweep)" $filter > "$O/wide$n-$seed.log" 2>&1; rc=$?
        echo "== wide$n $count $seed rc=$rc statements $(grep -cP '^[qw]\t' $O/wide$n-$seed.tsv) $(head -1 $O/wide$n-$seed.tsv | cut -c1-40) $(grep -E 'answers differ|test result' $O/wide$n-$seed.log | cut -c1-120 | tr '\n' ' ')" ) & done; wait
      echo "== wides$n took $(( $(date +%s) - s ))s" ;;
  esac
done
echo "== done $(date -u +%T) load $(load)"
