#!/usr/bin/env bash
# Lane A24 on adacachy2, held to $JOBS cores, in the lane worktree of cross-remote, at FIX: clippy on amphora,
# scripts/check-production.sh, the a21_ tests and the session_reads module. usage: a24gates.sh BRANCH:FIX OUT
set -uo pipefail
FIX=${1#*:}; BRANCH=${1%%:*}; OUT=${2:?}; T=${JOBS:-8}
R=$HOME/.cache/amphora-perf/cross-remote
W=$HOME/.cache/amphora-perf/cross-a24-wt
git -C "$R" fetch -q origin "$BRANCH" || exit 3
[ -d "$W" ] || git -C "$R" worktree add -q --detach "$W" "$FIX" || exit 3
cd "$W" || exit 2
git checkout -q -f --detach "$FIX" || exit 3
[ "$(git rev-parse HEAD)" = "$FIX" ] || { echo "HEAD is not $FIX"; exit 1; }
git log --oneline -1
export CARGO_HOME=$HOME/.cache/cargo-home-perf-cross PATH=$HOME/.cache/cargo-home-perf-cross/bin:$PATH CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/cross CARGO_BUILD_JOBS=$T CARGO_INCREMENTAL=0
B=$HOME/.cache/amphora-perf-bin/cross; mkdir -p "$B" "$OUT"
stamp() { echo "[$(date -u +%H:%M:%SZ)] $* load=$(cut -d' ' -f1-3 /proc/loadavg) cpu=$(awk 'NR==1{print $2}' /proc/pressure/cpu) io=$(awk 'NR==1{print $2}' /proc/pressure/io)"; }
nice -n 10 cargo clippy -p amphora --all-targets --locked --message-format=short -- -D warnings > "$OUT/clippy.log" 2>&1; CL=$?
grep -E "^crates/|^error|Finished|Checking amphora(-engine)? " "$OUT/clippy.log" | grep -v "vendor/hyper-raft" | head -20
stamp "clippy rc=$CL"
nice -n 10 bash scripts/check-production.sh > "$OUT/check-production.log" 2>&1; CP=$?
tail -4 "$OUT/check-production.log"; stamp "check-production rc=$CP"
t0=$(date +%s); out=$(nice -n 10 cargo test -p amphora --lib --locked --no-run 2>&1)
bin=$(echo "$out" | sed -n 's/.*Executable unittests src\/lib.rs (\(.*\))/\1/p')
stamp "built in $(( $(date +%s) - t0 ))s"; [ -x "$bin" ] || { echo "$out" | grep -E "^(error|warning)" -A 14 | head -60; exit 4; }
cp "$bin" "$B/a24-tests-$FIX"
for f in a21_ session_reads::; do
  for i in 1 2 3; do
    o=$(nice -n 10 "$B/a24-tests-$FIX" --test-threads="$T" "$f" 2>&1); rc=$?
    echo "$o" > "$OUT/$f$i.log"; stamp "$f run $i rc=$rc $(echo "$o" | grep '^test result' | tail -1 | cut -c1-70)"
    echo "$o" | grep -E '^test .* FAILED$|panicked at' | head
  done
done
stamp "end clippy=$CL check-production=$CP"
