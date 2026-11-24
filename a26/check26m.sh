# Lane A26 on macOS, in kiln's synced copy of the lane clone, held to $JOBS cores. usage: check26m.sh STEP...
#   build:SHA   t:SHA:LABEL:ROUNDS:FILTER[,FILTER]   suite:A:B:ROUNDS:FIRST
set -u
T=${JOBS:-8}
[ "$(df -g ~ | tail -1 | awk '{print $4}')" -ge 40 ] || { echo "disk low: $(df -g ~ | tail -1)"; exit 2; }
echo "$(hostname) $(sysctl -n hw.ncpu) cores"
case "$(hostname)" in *studio-3*) echo "this runner is not ours to use"; exit 7;; esac
CH=$HOME/.cache/cargo-home-perf-cross
mkdir -p "$CH"
for d in registry git bin; do [ -e "$CH/$d" ] || ln -s "$HOME/.cargo/$d" "$CH/$d"; done
export CARGO_HOME=$CH PATH=/opt/homebrew/bin:$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/cross CARGO_BUILD_JOBS=$T CARGO_INCREMENTAL=0
B=$HOME/.cache/amphora-perf-bin/cross; OUT=$HOME/.cache/amphora-perf/cross-a26-out; mkdir -p "$B" "$OUT"
stamp() { echo "[$(date -u +%H:%M:%SZ)] $* load $(sysctl -n vm.loadavg | tr -d '{}')"; }
one() {
  sha=$1 label=$2; shift 2; s=$(date +%s)
  nice -n 10 "$B/a26-tests-$sha" --test-threads="$T" "$@" > "$OUT/$label-$sha.log" 2>&1; rc=$?
  stamp "$label $sha rc=$rc $(( $(date +%s) - s ))s: $(grep 'test result' "$OUT/$label-$sha.log" | tail -1)"
  grep -E '^test .* FAILED$' "$OUT/$label-$sha.log" | head -14
  [ "$rc" = 0 ] || grep -E "panicked at" -A 3 "$OUT/$label-$sha.log" | cut -c1-500 | head -20
}
for step in "$@"; do
  IFS=: read -r what a b c d <<<"$step"
  case $what in
    build)
      git checkout -q -f --detach "$a" || exit 3; git log --oneline -1
      t0=$(date +%s); out=$(nice -n 10 cargo test -p amphora --lib --locked --no-run 2>&1)
      bin=$(echo "$out" | sed -n "s/.*Executable unittests src\/lib.rs (\(.*\))/\1/p")
      stamp "built $a in $(( $(date +%s) - t0 ))s"
      [ -x "$bin" ] || { echo "$out" | grep -E "^(error|warning)" -A 14 | head -60; exit 4; }
      cp "$bin" "$B/a26-tests-$a" ;;
    t) for r in $(seq 1 "$c"); do one "$a" "$b-r$r" $(echo "$d" | tr , ' '); done ;;
    suite)
      for r in $(seq "$d" $(( d + c - 1 ))); do
        if [ $((r % 2)) -eq 1 ]; then order="$a $b"; else order="$b $a"; fi
        for S in $order; do one "$S" "suite-r$r" database::replicated:: replication::; done
      done ;;
  esac
done
stamp end
