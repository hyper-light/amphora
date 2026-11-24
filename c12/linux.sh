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
    t:*)  # t:TEST [filter]: an integration test's run
      IFS=: read -r _ name filter <<< "$step"
      timeout 1200 nice -n 10 "$(bin $name)" --test-threads=$T $filter > "$O/t-$name.log" 2>&1; rc=$?
      echo "== $name $filter rc=$rc $(( $(date +%s) - s ))s load $(load)"
      grep -E "^test result|FAILED|panicked|answers differ" "$O/t-$name.log" | cut -c1-300 | head -${LINES_MAX:-20} ;;
    *) echo "unknown step $step" ;;
  esac
done
echo "== done $(date -u +%T) load $(load)"
