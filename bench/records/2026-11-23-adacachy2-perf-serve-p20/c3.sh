#!/bin/bash
# Lane P20 (serve) c3 (c1 again after main moved, the shed fix only), adacachy2: the landing checks after main moved in crates/engine (fac69083). S2 (the shed fix with
# main) and H3 (the branch with the fix, V13's hold and main), each built in turn in the remote clone and the lane's
# target dir: the tests touched and clippy on each.
set -u
S2=92654cd298b67b1657f0fec584e93f2e1ff30d02
A=$HOME/.cache/amphora-perf/serve-p7-remote
export PATH=$HOME/.cargo/bin:$PATH CARGO_HOME=$HOME/.cache/cargo-home-perf-serve-p7
export CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/serve-p7 CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
export CARGO_MANIFEST_DIR=$A/crates/amphora
L=$HOME/.cache/amphora-perf/serve-p20-c3; mkdir -p "$L"
echo "start $(date -u +%T) load $(cut -d' ' -f1-3 /proc/loadavg)"
git -C "$A" fetch -q origin || exit 9
check() { # check SHA TEST...: build, the named tests alone, clippy
  local sha=$1; shift; cd "$A" && git checkout -q -f --detach "$sha" || exit 9
  local s=$(date +%s) lib
  lib=$(nice -n 10 cargo test -p amphora --locked --lib --no-run 2>&1 | tee "$L/build-$sha.log" | sed -n 's/.*Executable .*(\(.*\))$/\1/p' | tail -1)
  [ -x "$lib" ] || { echo "BUILD_FAILED ${sha:0:8}"; grep -E '^error' -A12 "$L/build-$sha.log" | head -40; return 1; }
  echo "built ${sha:0:8} in $(( $(date +%s) - s ))s $(date -u +%T)"
  cd "$A/crates/amphora"
  for t in "$@"; do echo "test ${sha:0:8} $t: $("$lib" --test-threads=2 "$t" 2>&1 | grep -E '^test result' | tail -1)"; done
  cd "$A" && s=$(date +%s) && nice -n 10 cargo clippy -p amphora --all-targets --locked -- -D warnings > "$L/clippy-$sha.log" 2>&1
  echo "CLIPPY_EXIT=$? ${sha:0:8} $(( $(date +%s) - s ))s $(date -u +%T) $(grep -c 'Checking amphora' "$L/clippy-$sha.log") amphora checks"; grep -E '^error' -A12 "$L/clippy-$sha.log" | head -30
}
check $S2 qb2_the_shed_never_takes_a_hand_offs_order_to_campaign a_drain_at_the_queues_bound_takes_the_whole_queue
echo "end $(date -u +%T) load $(cut -d' ' -f1-3 /proc/loadavg)"
