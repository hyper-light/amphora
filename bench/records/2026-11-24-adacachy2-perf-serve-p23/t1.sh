#!/bin/bash
# Lane P23 (serve) t1, adacachy2: builds the two traced arms (diag.py on H 3a58a13f's crates and M 6d8e7dcd, release
# amphora-bench, in lane P20's shared clone and the serve lanes' target dir), one after the other.
set -u
H=3a58a13fa4c42f6b8d4c8729c0c5b125c1588cbc; M=6d8e7dcd38be29214187a06bbfabdf9b054acb2a; J=${J:-2}
A=$HOME/.cache/amphora-perf/serve-p7-remote; B=$HOME/.cache/serve-p20-ab/b1
export PATH=$HOME/.cargo/bin:$PATH CARGO_HOME=$HOME/.cache/cargo-home-perf-serve-p7
export CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/serve-p7 CARGO_BUILD_JOBS=$J CARGO_INCREMENTAL=0
L=$HOME/.cache/amphora-perf/serve-p23-t1; BIN=$HOME/.cache/amphora-perf-bin/serve-p23; mkdir -p "$L" "$BIN"
ld() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu-some $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/cpu) io-some $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/io)"; }
echo "start $(date -u +%T) jobs $J $(ld)"; df -h ~ | tail -1
git -C "$A" fetch -q origin || git -C "$A" fetch -q origin || { echo FETCH_FAILED; exit 9; }
for s in $H $M; do git -C "$A" cat-file -e "$s^{commit}" || { echo "NO $s"; exit 9; }; done
git -C "$A" show ${S:-origin/perf/serve-p23}:bench/records/2026-11-24-adacachy2-perf-serve-p23/diag.py > "$L/diag.py" || exit 9
for arm in M H; do
  sha=$M; [ $arm = H ] && sha=$H
  git -C "$B" fetch -q "$A" 2>/dev/null; git -C "$B" checkout -q -f --detach $sha || { git -C "$B" fetch -q origin; git -C "$B" checkout -q -f --detach $sha; } || exit 9
  python3 -I "$L/diag.py" "$B" $arm || exit 9
  s=$(date +%s); ( cd "$B/bench" && nice -n 10 cargo build --release --offline > "$L/build-$arm.log" 2>&1 ) && cp "$CARGO_TARGET_DIR/release/amphora-bench" "$BIN/diag-$arm" \
    || { echo "BUILD_FAILED $arm"; grep -E '^error' -A12 "$L/build-$arm.log" | head -60; tail -5 "$L/build-$arm.log"; exit 8; }
  echo "built diag-$arm in $(( $(date +%s) - s ))s $(date -u +%T) $(ld); $(grep -E 'Compiling amphora' "$L/build-$arm.log" | tr '\n' ' ') text $(grep -a -c 'P23 ' "$BIN/diag-$arm")"
done
git -C "$B" checkout -q -f --detach $M
echo "end $(date -u +%T) $(ld)"
