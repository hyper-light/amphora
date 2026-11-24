#!/usr/bin/env bash
# Lane A26 on adacachy2, held to $JOBS cores, in cross-remote: SHA's release amphora-bench (the diag driver counts every
# failure by kind), then V13's ladder cells and bursts, ROUNDS rounds. usage: bench26.sh SHA BRANCH OUT ROUNDS CELL...
# A CELL is class:writers:route.
set -uo pipefail
SHA=${1:?}; BRANCH=${2:?}; OUT=${3:?}; ROUNDS=${4:?}; shift 4; T=${JOBS:-8}; S=$(cd "$(dirname "$0")" && pwd)
R=$HOME/.cache/amphora-perf/cross-remote
git -C "$R" fetch -q origin main "$BRANCH" || exit 3
cd "$R" || exit 2
git checkout -q -f --detach "$SHA" || exit 3
SHA=$(git rev-parse HEAD)
git log --oneline -1
ss -ltnp 2>/dev/null | grep -E ":2949[0-9]" | head -6
export CARGO_HOME=$HOME/.cache/cargo-home-perf-cross PATH=$HOME/.cache/cargo-home-perf-cross/bin:$PATH CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/cross CARGO_BUILD_JOBS=$T CARGO_INCREMENTAL=0
B=$HOME/.cache/amphora-perf-bin/cross; mkdir -p "$B" "$OUT"
stamp() { echo "[$(date -u +%H:%M:%SZ)] $* load=$(cut -d' ' -f1-3 /proc/loadavg) cpu=$(awk 'NR==1{print $2}' /proc/pressure/cpu) io=$(awk 'NR==1{print $2}' /proc/pressure/io)"; }
if [ ! -x "$B/a26-$SHA" ]; then
s=$(date +%s)
(cd bench && nice -n 10 cargo build --release --offline) > "$OUT/build.log" 2>&1
BR=$?
grep -E "Compiling amphora|Adding|Updating|^error|Finished" "$OUT/build.log" | head -12
stamp "bench built rc=$BR in $(( $(date +%s) - s ))s"
[ "$BR" = 0 ] || { tail -30 "$OUT/build.log"; exit 4; }
cp "$CARGO_TARGET_DIR/release/amphora-bench" "$B/a26-$SHA" || exit 5
fi
ls -la --time-style=+%H:%M "$B/a26-$SHA"
ulimit -n "$(ulimit -Hn)" 2>/dev/null || true
for r in $(seq 1 "$ROUNDS"); do
  for cell in "$@"; do
    IFS=: read -r cls w route <<<"$cell"
    echo "r$r $(date -u +%T) load=$(cut -d' ' -f1-3 /proc/loadavg) cpu=$(awk 'NR==1{print $2}' /proc/pressure/cpu) io=$(awk 'NR==1{print $2}' /proc/pressure/io) $(nice -n 10 python3 -I "$S/cell26.py" "$B/a26-$SHA" "${PORT:-29490}" "$OUT/$cls-$w-$route-r$r" "$w" "$route" "$cls")"
  done
done
stamp "end"
