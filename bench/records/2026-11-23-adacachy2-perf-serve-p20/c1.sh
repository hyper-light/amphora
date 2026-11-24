#!/bin/bash
# Lane P20 (serve) c1, adacachy2: the landing checks after main moved in crates/engine (fac69083). S2 (the shed fix with
# main) and H3 (the branch with the fix, V13's hold and main), each built in turn in the remote clone and the lane's
# target dir: the tests touched and clippy on each.
set -u
S2=0de72a59a76c7a1612d3c6eb441ed786222dbe59; H3=201534be5d0624834e3e9e2f6041923a8e14589c
A=$HOME/.cache/amphora-perf/serve-p7-remote
export PATH=$HOME/.cargo/bin:$PATH CARGO_HOME=$HOME/.cache/cargo-home-perf-serve-p7
export CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/serve-p7 CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
export CARGO_MANIFEST_DIR=$A/crates/amphora
L=$HOME/.cache/amphora-perf/serve-p20-c1; mkdir -p "$L"
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
check $H3 qb2_the_shed_never_takes_a_hand_offs_order_to_campaign seed_and_delta_hold_through_an_election_at_each_step v13_thirty_two_spread_writers_on_moving_time_are_each_answered_with_no_stale_base cp_t1_13 a_drain_at_the_queues_bound_takes_the_whole_queue
echo "end $(date -u +%T) load $(cut -d' ' -f1-3 /proc/loadavg)"
