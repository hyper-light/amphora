# Lane A24 on macOS, in kiln synced copy of the lane clone: builds FIX (diag on the fix), then r5 alone on each of SHAS
# with the reader thread started DELAY us late, six processes at a time. usage: a24m2.sh FIX REPS "DELAYS" SHAS...
set -u
FIX=$1; REPS=$2; DELAYS=$3; shift 3; J=${JOBS:-8}
[ "$(df -g ~ | tail -1 | awk '{print $4}')" -ge 40 ] || { echo "disk low"; exit 2; }
CH=$HOME/.cache/cargo-home-perf-cross
export CARGO_HOME=$CH PATH=$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/cross CARGO_BUILD_JOBS=$J CARGO_INCREMENTAL=0
B=$HOME/.cache/amphora-perf-bin/cross; OUT=$HOME/.cache/amphora-perf/cross-a24-out/delay; mkdir -p "$B" "$OUT"
stamp() { echo "[$(date -u +%H:%M:%SZ)] $* load $(sysctl -n vm.loadavg | tr -d '{}')"; }
if [ ! -x "$B/a24-tests-$FIX" ]; then
  git checkout -q -f --detach "$FIX" || exit 3
  t0=$(date +%s); out=$(nice -n 10 cargo test -p amphora --lib --locked --no-run 2>&1)
  bin=$(echo "$out" | sed -n 's/.*Executable unittests src\/lib.rs (\(.*\))/\1/p')
  stamp "built $FIX in $(( $(date +%s) - t0 ))s"
  [ -x "$bin" ] || { echo "$out" | grep -E '^(error|warning)' -A 14 | head -60; exit 4; }
  cp "$bin" "$B/a24-tests-$FIX"
fi
T=database::replicated::tests::session_reads::t_t1_10_r5_a_session_reads_its_write_on_a_lagging_follower
export B OUT T
one() {
  local d=$1 s=$2 i=$3
  A24_READER_DELAY_US=$d nice -n 10 "$B/a24-tests-$s" --exact "$T" --test-threads=1 --show-output > "$OUT/$s-$d-$i.log" 2>&1
  echo "rc=$? sha=${s:0:8} delay_us=$d run=$i $(grep -h a24-r5 "$OUT/$s-$d-$i.log" | head -1 | cut -c1-330) $(grep -h 'panicked at' -A 3 "$OUT/$s-$d-$i.log" | grep -E 'assertion|left|right|panicked|waited' | tr '\n' ' ' | cut -c1-260)"
}
export -f one
stamp start
for d in $DELAYS; do for s in "$@"; do for i in $(seq 1 "$REPS"); do echo "$d $s $i"; done; done; done | xargs -P 6 -L 1 bash -c 'one "$0" "$1" "$2"'
stamp end
