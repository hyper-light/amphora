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
      echo "== built in $(( $(date +%s) - s ))s" ;;
    sql:*)
      C18_SQL=$O/${step#sql:} timeout 300 "$(bin c18_sql)" --nocapture 2>"$O/sql.err" | cut -c1-900
      grep -E "panicked|required" "$O/sql.err" | sort | uniq -c | head ;;
    test:*)  # test:NAME[:FILTER]
      IFS=: read -r _ name filter <<< "$step"
      timeout 900 nice -n 10 "$(bin "$name")" $filter > "$O/test-$name.log" 2>&1; rc=$?
      echo "== test $name $filter rc=$rc $(( $(date +%s) - s ))s"; grep -E "test result|panicked|answers differ|FAILED|failed$" "$O/test-$name.log" | head -12 ;;
  esac
done
echo "== done $(date -u +%T) load $(load)"
