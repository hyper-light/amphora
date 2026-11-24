#!/usr/bin/env bash
set -u; O=$HOME/.cache/amphora-perf/planner-c18-out; G=$HOME/.cache/amphora-perf/planner-c13-remote
export JOBS=${JOBS:-4} RUST_TEST_THREADS=4
git -C "$G" fetch -q origin 2>&1 | tail -1
export CARGO_HOME=$HOME/.cache/cargo-home-perf-planner-c13 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/planner-c13 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=$JOBS
one() { for i in 1 2 3; do timeout 100 nice -n 10 "$1" a_whole_table_group_by --test-threads=1 2>&1 | grep -E "test result|panicked at" | cut -c1-110 | tr '\n' ' '; cut -d' ' -f1 /proc/loadavg; done; }
bash "$O/linux.sh" 200e6d31 build:planner 2>&1 | grep -E "^== (built|adacachy2)"
cp -f $(ls -t $CARGO_TARGET_DIR/debug/deps/planner-* | grep -v '\.d$' | head -1) $O/planner-landed
(cd $HOME/.cache/amphora-perf/planner-c18-remote && timeout 400 nice -n 10 cargo test -p amphora-engine --locked --features simulator --lib --no-run > $O/libbuild.log 2>&1; echo "== lib built rc=$? $(date -u +%T)")
B=$CARGO_TARGET_DIR/debug/$(grep -oE "deps/amphora_engine-[0-9a-f]+" $O/libbuild.log | head -1); cp -f "$B" $O/lib-landed
for f in cte subquery; do timeout 120 nice -n 10 $O/lib-landed $f --test-threads=4 > $O/lib-$f.log 2>&1; echo "== lib $f rc=$?"; grep -E "test result|FAILED|failed$" $O/lib-$f.log | head -6; done
bash "$O/linux.sh" 837071af build:planner 2>&1 | grep -E "^== (built|adacachy2)"
cp -f $(ls -t $CARGO_TARGET_DIR/debug/deps/planner-* | grep -v '\.d$' | head -1) $O/planner-before
echo "== before (837071af)"; one $O/planner-before
echo "== landed (200e6d31)"; one $O/planner-landed
timeout 150 nice -n 10 $O/lib-landed translate:: --test-threads=4 --skip optimizer > $O/lib-tr.log 2>&1; echo "== lib translate:: (no optimizer) rc=$?"; grep -E "test result|FAILED|failed$" $O/lib-tr.log | head -6
