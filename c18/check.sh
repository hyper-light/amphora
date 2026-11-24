#!/usr/bin/env bash
# fmt, check-production, clippy, the lib suites, in a worktree of its own. usage: check.sh [lib]
set -u; O=$HOME/.cache/amphora-perf/planner-c18-out; G=$HOME/.cache/amphora-perf/planner-c18-remote; C=$HOME/.cache/amphora-perf/planner-c18-check
git -C "$G" fetch -q origin 2>&1 | tail -1; SHA=$(git -C "$G" rev-parse origin/perf/planner-c18); cd "$C" && git checkout -q -f --detach $SHA
export CARGO_HOME=$HOME/.cache/cargo-home-perf-planner-c13 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/planner-c13 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-6}
echo "== $(date -u +%T) tree $(git rev-parse --short=8 HEAD) load $(cut -d' ' -f1-3 /proc/loadavg)"
cargo fmt --all --check > $O/fmt.log 2>&1; echo "== fmt rc=$?"
timeout 900 nice -n 10 bash scripts/check-production.sh > $O/prod3.log 2>&1; echo "== check-production rc=$? $(date -u +%T)"; tail -3 $O/prod3.log | cut -c1-300
timeout 900 nice -n 10 cargo clippy -p amphora-engine --all-targets --locked -- -D warnings > $O/clippy3.log 2>&1; echo "== clippy rc=$? $(date -u +%T)"; grep -E "amphora-engine v|^error|^warning|Finished" -A 5 $O/clippy3.log | head -30
[ "${1:-}" = lib ] || exit 0
timeout 600 nice -n 10 cargo test -p amphora-engine --locked --features simulator --lib --no-run > $O/libbuild.log 2>&1; echo "== lib built rc=$? $(date -u +%T)"
B=$CARGO_TARGET_DIR/debug/$(grep -oE "deps/amphora_engine-[0-9a-f]+" $O/libbuild.log | head -1); echo $B
for f in cte subquery view; do timeout 300 nice -n 10 $B $f --test-threads=6 > $O/lib-$f.log 2>&1; echo "== lib $f rc=$?"; grep -E "test result|FAILED|failed$|panicked" $O/lib-$f.log | head -8; done
timeout 300 nice -n 10 $B translate:: --test-threads=6 --skip optimizer:: > $O/lib-tr.log 2>&1; echo "== lib translate:: (no optimizer) rc=$?"; grep -E "test result|FAILED|failed$|panicked" $O/lib-tr.log | head -8
timeout 420 nice -n 10 $B optimizer:: --test-threads=6 --skip t_pl_8_13 --skip t_pl_8_14_ab > $O/lib-opt.log 2>&1; echo "== lib optimizer:: rc=$? $(date -u +%T)"; grep -E "test result|FAILED|failed$|panicked" $O/lib-opt.log | head -8
