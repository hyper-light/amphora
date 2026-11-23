#!/bin/bash
# J4: clippy on the two re-merges, G's engine suites and I's replication/store suites against main, from the built binaries.
set -u
export CARGO_HOME=$HOME/.cache/cargo-home-perf-j4 PATH=$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=false CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=8
export CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/integrate-j4
MAIN=${MAIN:?} G=${G:?} I=${I:?}
SRC=$HOME/.cache/amphora-perf/j4-remote LOGS=$HOME/.cache/amphora-perf/j4-logs BIN=$HOME/.cache/amphora-perf-bin/j4
cd "$SRC" || exit 2
git fetch -q origin '+refs/heads/*:refs/remotes/origin/*' || exit 2
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg)"; }
for arg in G:$G I:$I; do
  label=${arg%%:*}; sha=${arg#*:}; git checkout -q --detach "$sha" || exit 3
  log=$LOGS/clippy-$label.log
  timeout 2400 nice -n 10 cargo clippy -p amphora -p amphora-engine --all-targets --locked -- -D warnings >"$log" 2>&1
  rc=$?; echo "clippy $label $sha rc=$rc $(state)"; grep -E '^(error|warning)' "$log" | sort | uniq -c | head -10
done
one() { local log=$1 crate=$2 sha=$3 bin=$4; shift 4
  git checkout -q --detach "$sha"; ( cd crates/$crate && timeout 1800 nice -n 10 "$bin" "$@" ) >"$log" 2>&1
  local r; r=$(grep -E '^test result' "$log" | tail -1); echo "  $(basename $log): $r $(state)"; grep -E '^test .* FAILED$' "$log" | sort -u; }
for arm in main:$MAIN G:$G; do l=${arm%%:*}; s=${arm#*:}
  one $LOGS/englib-$l.log engine $s $BIN/$s/amphora_engine bushy_search_tests t_qt_6_ audit_3_1 --skip t_qt_6_18 --skip t_pl_8_14 --test-threads=8
done
for arm in main:$MAIN I:$I; do l=${arm%%:*}; s=${arm#*:}
  one $LOGS/repl-$l.log amphora $s $BIN/$s/amphora replication:: store:: --test-threads=8
done
echo C2DONE
