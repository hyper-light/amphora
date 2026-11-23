#!/bin/bash
# J4: G re-merged on the moved main: clippy, then G's tests and the engine suite against that main.
set -u
export CARGO_HOME=$HOME/.cache/cargo-home-perf-j4 PATH=$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=false CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=8
export CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/integrate-j4
MAIN=${MAIN:?} G=${G:?}
SRC=$HOME/.cache/amphora-perf/j4-remote LOGS=$HOME/.cache/amphora-perf/j4-logs
cd "$SRC" || exit 2
git fetch -q origin '+refs/heads/*:refs/remotes/origin/*' || exit 2
git checkout -q --detach "$G" || exit 3
timeout 2400 nice -n 10 cargo clippy -p amphora -p amphora-engine --all-targets --locked -- -D warnings >"$LOGS/clippy-G2.log" 2>&1
echo "clippy G2 $G rc=$? load $(cut -d' ' -f1-3 /proc/loadavg)"; grep -E '^(error|warning)' "$LOGS/clippy-G2.log" | sort | uniq -c | head
for arm in main2:$MAIN G2:$G; do l=${arm%%:*}; s=${arm#*:}; git checkout -q --detach "$s"
  log=$LOGS/englib-$l.log
  timeout 2400 nice -n 10 cargo test -p amphora-engine --lib --locked -- bushy_search_tests t_qt_6_ audit_3_1 --skip t_qt_6_18 --skip t_pl_8_14 --test-threads=8 >"$log" 2>&1
  echo "englib $l $s: $(grep -E '^test result' $log | tail -1) load $(cut -d' ' -f1-3 /proc/loadavg)"; grep -E '^test .* FAILED$' "$log" | sort -u
done
echo C3DONE
