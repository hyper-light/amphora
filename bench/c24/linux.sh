#!/usr/bin/env bash
# Lane C24's Linux driver (not for main). usage: linux.sh SHA|- STEP...
set -uo pipefail
SHA=$1; shift
export CARGO_HOME=$HOME/.cache/cargo-home-perf-vdbe-c24 CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-8}
export CARGO_TARGET_DIR=${TGT:-$HOME/.cache/amphora-perf-target/vdbe-c24}
mkdir -p "$CARGO_HOME"; for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -sfn "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
O=$HOME/.cache/amphora-perf/vdbe-c24-out; mkdir -p "$O"
load() { cut -d' ' -f1-3 /proc/loadavg; }
R=${REMOTE:-$HOME/.cache/amphora-perf/vdbe-c24-remote}
[ -d "$R" ] || git clone -q --reference "$HOME/.cache/amphora-perf/expr-c21-remote" --dissociate git@github.com:hyper-light/amphora.git "$R" || exit 2
cd "$R" || exit 2
if [ "$SHA" != - ]; then
  git fetch -q origin "$SHA" 2>&1 | tail -2
  git checkout -q -f --detach "$SHA" || exit 3
  git clean -q -fd crates bench
fi
cp "$R"/bench/c24/*.tsv "$O"/ 2>/dev/null; [ -f "$R/bench/c24/c24_sql.rs" ] && cp "$R/bench/c24/c24_sql.rs" crates/engine/tests/c24_sql.rs; rustfmt --edition 2021 crates/engine/tests/c24_sql.rs 2>/dev/null
echo "== $(hostname) $(date -u +%T) tree $(git rev-parse --short=8 HEAD) load $(load)"
D=$CARGO_TARGET_DIR/debug/deps
bin() { ls -t "$D"/$1-* | grep -v '\.d$' | head -1; }
for step in "$@"; do
  s=$(date +%s)
  case $step in
    seed)  # a private copy of C21's finished build folder: its third-party crates, not a fresh build
      if [ ! -d "$CARGO_TARGET_DIR" ]; then mkdir -p "$CARGO_TARGET_DIR"; cp -a --reflink=always "$HOME/.cache/amphora-perf-target/expr-c21/." "$CARGO_TARGET_DIR/"; echo "== seeded rc=$? $(( $(date +%s) - s ))s"; fi ;;
    fmt) cargo fmt --all --check > "$O/fmt.log" 2>&1; echo "== fmt rc=$?"; head -30 "$O/fmt.log" ;;
    clippy)
      timeout 900 nice -n 10 cargo clippy -p amphora-engine -p amphora-engine-parser --all-targets --locked -- -D warnings > "$O/clippy.log" 2>&1; rc=$?
      echo "== clippy rc=$rc $(( $(date +%s) - s ))s"; grep -E "^(error|warning)|Checking amphora-engine|Finished" -A 6 "$O/clippy.log" | head -60 ;;
    prod)
      timeout 1500 nice -n 10 bash scripts/check-production.sh > "$O/prod.log" 2>&1; rc=$?
      echo "== check-production rc=$rc $(( $(date +%s) - s ))s"; tail -8 "$O/prod.log" | cut -c1-300 ;;
    build:*)
      tests=""; IFS=, read -ra names <<< "${step#build:}"; for n in "${names[@]}"; do tests="$tests --test $n"; done
      timeout 1200 nice -n 10 cargo test -p amphora-engine --locked --features simulator $tests --no-run 2>&1 | grep -E "^(error|warning)|Executable|Compiling amphora|Finished" -A 8 | head -80
      rc=${PIPESTATUS[0]}; echo "== built rc=$rc in $(( $(date +%s) - s ))s"; [ "$rc" = 0 ] || { echo "== BUILD FAILED, nothing run"; exit 4; } ;;
    sql:*)
      C24_SQL=$O/${step#sql:} timeout 300 "$(bin c24_sql)" --nocapture --test-threads=1 2>"$O/sql.err" | cut -c1-${CUT:-1500}
      grep -E "panicked|required" "$O/sql.err" | sort | uniq -c | head ;;
    test:*)  # test:NAME[:FILTER]
      IFS=: read -r _ name filter <<< "$step"
      timeout 900 nice -n 10 "$(bin "$name")" $filter --test-threads=${THREADS:-8} > "$O/test-$name.log" 2>&1; rc=$?
      echo "== test $name $filter rc=$rc $(( $(date +%s) - s ))s"; grep -E "test result|panicked|answers differ|FAILED|failed$" "$O/test-$name.log" | cut -c1-300 | head -${LINES_OUT:-14} ;;
    cat:*) sed -n 1,${CAT_LINES:-80}p "$O/${step#cat:}" | cut -c1-${CUT:-1500} ;;
  esac
done
echo "== done $(date -u +%T) load $(load)"
