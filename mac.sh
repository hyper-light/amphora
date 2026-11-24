#!/usr/bin/env bash
# Lane C26's portable driver, run in the synced clone (not for main). usage: mac.sh STEP...
set -uo pipefail
export PATH=/opt/homebrew/bin:$HOME/.cargo/bin:$PATH
[ "$(df -g ~ | tail -1 | awk '{print $4}')" -ge 40 ] || { echo "disk low"; exit 2; }
export CARGO_HOME=$HOME/.cache/cargo-home-perf-dml-c26 CARGO_NET_OFFLINE=${OFFLINE:-true} CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/dml-c26 CARGO_INCREMENTAL=1 CARGO_BUILD_JOBS=${JOBS:-8}
O=$HOME/.cache/amphora-perf/dml-c26-out; mkdir -p "$O" "$CARGO_HOME" "$CARGO_TARGET_DIR"
for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -s "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
echo "== $(hostname) $(date -u +%T) $(pwd) load $(sysctl -n vm.loadavg)"
D=$CARGO_TARGET_DIR/debug/deps
bin() { ls -t "$D"/$1-* | grep -v '\.d$' | grep -v dSYM | head -1; }
for step in "$@"; do
  s=$(date +%s)
  case $step in
    build:*)
      tests=""; IFS=, read -ra names <<< "${step#build:}"; for n in "${names[@]}"; do tests="$tests --test $n"; done
      nice -n 10 cargo test -p amphora-engine --locked $tests --no-run > "$O/build.log" 2>&1; rc=$?
      grep -E "^(error|warning)|Executable|Finished" -A 8 "$O/build.log" | head -${BUILD_LINES:-120}
      echo "== build rc=$rc in $(( $(date +%s) - s ))s"; [ $rc -ne 0 ] && exit $rc ;;
    probe:*)
      C26_FIXTURE=${step#probe:} nice -n 10 "$(bin c26_probe)" --nocapture > "$O/probe.log" 2>&1; rc=$?
      echo "== probe $step rc=$rc $(( $(date +%s) - s ))s"; grep -av "^thread\|^note\|^stack" "$O/probe.log" | cut -c1-${CUT:-900} | tail -${PROBE_LINES:-200} ;;
    test:*)
      IFS=: read -r _ name filter <<< "$step"
      nice -n 10 "$(bin $name)" ${filter:-} --test-threads=${JOBS:-8} > "$O/test-$name.log" 2>&1; rc=$?
      echo "== test $name ${filter:-} rc=$rc $(( $(date +%s) - s ))s"; grep -aE "test result|panicked|answers differ|FAILED|failed$" "$O/test-$name.log" | head -${LINES_MAX:-20} ;;
  esac
done
echo "== done $(date -u +%T)"
