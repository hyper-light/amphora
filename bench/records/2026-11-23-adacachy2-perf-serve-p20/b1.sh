#!/bin/bash
# Lane P20 (serve) b1, adacachy2: lane V13's real-time cell (32 spread writers, both classes, its flags) on the branch
# (HOLD: the branch with main 9fbfdbaf and the shed fix) and main 9fbfdbaf, release amphora-bench built in turn in the
# lane's target dir (no Cargo.lock is tracked for bench/, so --offline, not --locked), ROUNDS rounds, arm order
# alternated; then the branch with main 7de5344d and no shed fix (be3b72bc), seed_and_delta once. Built in one shared
# clone under ~/.cache/serve-p20-ab, beside l1's checkouts in the remote clone.
set -u
ROUNDS=${1:-2}
HOLD=143b82cd7b85f49ade232dc3178284a776f1350d; M=9fbfdbaf257d452ef086c92f6528c2611f4abef2; BR=be3b72bc0047b706b93121ce15ed3baf5129fabf
A=$HOME/.cache/amphora-perf/serve-p7-remote
export PATH=$HOME/.cargo/bin:$PATH CARGO_HOME=$HOME/.cache/cargo-home-perf-serve-p7
export CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/serve-p7 CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
L=$HOME/.cache/amphora-perf/serve-p20-b1; BIN=$HOME/.cache/amphora-perf-bin/serve-p20; mkdir -p "$L" "$BIN"
ld() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu-some $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/cpu) io-some $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/io)"; }
echo "start $(date -u +%T) $(ld)"
S=$L/v13; mkdir -p "$S"; git -C "$A" show $M:bench/records/2026-11-23-adacachy2-perf-commits-v13/cell13.py > "$S/cell13.py" || exit 9
B=$HOME/.cache/serve-p20-ab/b1; [ -d "$B/.git" ] || git clone -q --shared "$A" "$B" || exit 9
for sha in $HOLD $M; do
  mkdir -p "$BIN/$sha"; git -C "$B" checkout -q -f --detach $sha || exit 9
  s=$(date +%s); (cd "$B/bench" && nice -n 10 cargo build --release --offline > "$L/build-bench-$sha.log" 2>&1) && cp "$CARGO_TARGET_DIR/release/amphora-bench" "$BIN/$sha/bench" \
    || { echo "BUILD_FAILED bench ${sha:0:8}"; grep -E '^error' -A10 "$L/build-bench-$sha.log" | head -30; exit 8; }
  echo "built bench ${sha:0:8} in $(( $(date +%s) - s ))s $(date -u +%T)"
done
for r in $(seq 1 "$ROUNDS"); do
  if [ $((r % 2)) -eq 1 ]; then arms="$HOLD $M"; else arms="$M $HOLD"; fi
  for sha in $arms; do for cls in persisted replicated; do
    echo "cell r$r ${sha:0:8} $cls $(date -u +%T) $(ld) $(nice -n 10 python3 -I "$S/cell13.py" "$BIN/$sha/bench" 29620 "$L/cells/${sha:0:8}-$cls-32-r$r" 32 spread "$cls")"
  done; done
done
cd "$B" && git checkout -q -f --detach $BR && s=$(date +%s) && lib=$(nice -n 10 cargo test -p amphora --locked --lib --no-run 2>&1 | tee "$L/build-lib-$BR.log" | sed -n 's/.*Executable .*(\(.*\))$/\1/p' | tail -1)
echo "built lib ${BR:0:8} in $(( $(date +%s) - s ))s $(date -u +%T)"
cd "$B/crates/amphora" && CARGO_MANIFEST_DIR=$B/crates/amphora "$lib" --exact database::replicated::tests::seed_and_delta_hold_through_an_election_at_each_step 2>&1 | grep -E '^test result|panicked at|did not move' | head -4
echo "end $(date -u +%T) $(ld)"
