#!/usr/bin/env bash
# Lane W4 (oltp): lane W3's A/B (../2026-11-23-adacachy2-perf-oltp/ab-w3.sh) on Mac Studio 2: main
# before 81fc0dca (A) against 81fc0dca (B), the flusher with no gathering wait. 5 rounds alternated
# (A,B then B,A); each round insert_one and update_plain at 1, 32 and 128 clients, and SQLite's
# insert_one at 1 client; each run a fresh process on a fresh database, A's calibration under both
# arms' keys (RULES 5e). macOS has no pressure stall information: beside each run, the load and the
# CPU all processes take (ps %cpu summed, 100 a core). The Linux-only stages shim is not run here.
# MODE=build: both arms built in one local clone, one target dir (only what B changes rebuilds).
# MODE=run: the timed runs. Run from the repository's root (kiln's synced copy).
set -uo pipefail
A=${A:?A sha}; B=${B:?B sha}; ROUNDS=${ROUNDS:-5}; MODE=${MODE:-run}
HERE=$(cd "$(dirname "$0")" && pwd); PY=$HERE/../2026-11-23-adacachy2-perf-oltp/ab_w3.py
W=$HOME/.cache/oltp-ab; BIN=$HOME/.cache/amphora-perf-bin/oltp; O=$W/out
state() { echo "load $(sysctl -n vm.loadavg | awk '{print $2, $3, $4}') cpu_pct $(ps -A -o %cpu= | awk '{s+=$1} END {printf "%.0f", s}') ncpu $(sysctl -n hw.ncpu)"; }
[ "$(df -g ~ | tail -1 | awk '{print $4}')" -ge 40 ] || { echo "disk low"; exit 2; }
mkdir -p "$W" "$BIN"
if [ "$MODE" = build ]; then
  CH=$HOME/.cache/cargo-home-perf-oltp; mkdir -p "$CH"
  for d in registry git bin; do [ -e "$CH/$d" ] || ln -s "$HOME/.cargo/$d" "$CH/$d"; done
  export CARGO_HOME=$CH CARGO_TARGET_DIR=$W/target CARGO_BUILD_JOBS=${JOBS:-8} CARGO_INCREMENTAL=0 \
    CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
  export PATH=$HOME/.cargo/bin:$PATH
  if [ -d "$W/repo/.git" ]; then git -C "$W/repo" fetch -q "$PWD" || exit 3; else git clone -q "$PWD" "$W/repo" || exit 3; fi
  for sha in "$A" "$B"; do
    [ -x "$BIN/amphora-bench-$sha" ] && { echo "have $sha"; continue; }
    git -C "$W/repo" checkout -q --detach "$sha" || { echo "checkout $sha failed"; exit 3; }
    echo "$(date -u +%H:%M:%SZ) build $sha $(state)"
    ( cd "$W/repo" && nice -n 10 cargo build --release --offline --manifest-path bench/Cargo.toml --bin amphora-bench ) >"$W/build-$sha.log" 2>&1 \
      || ( cd "$W/repo" && nice -n 10 cargo build --release --manifest-path bench/Cargo.toml --bin amphora-bench ) >>"$W/build-$sha.log" 2>&1 \
      || { tail -30 "$W/build-$sha.log"; exit 4; }
    cp "$CARGO_TARGET_DIR/release/amphora-bench" "$BIN/amphora-bench-$sha" || exit 4
  done
  echo "$(date -u +%H:%M:%SZ) built $(state)"; ls -la "$BIN"; exit 0
fi
mkdir -p "$O"; rm -f "${O:?}"/*
D=$W/data; mkdir -p "$D"; export XDG_CACHE_HOME=$W/xdg
run() {  # arm-sha target op clients label
  rm -rf "${D:?}/x"; local s; s=$(state)
  nice -n 10 "$BIN/amphora-bench-$1" --target "$2" --dir "$D/x" --suite --only "$3" --items 200000 --clients "$4" --seconds 1 --warmup 0.5 2>>"$O/runs.err" | tail -1 >"$O/$5.json"
  python3 -I "$PY" line "$O/$5.json" "$5" "$s"
}
echo "$(date -u +%H:%M:%SZ) start $(state)"
run "$A" amphora 'none$' 1 load-A; run "$B" amphora 'none$' 1 load-B
python3 -I - "$XDG_CACHE_HOME/amphora/calibration" "$BIN/amphora-bench-$A" <<'PY'
import glob, os, sys
d, src = sys.argv[1], sys.argv[2]
texts = {f: open(f).read() for f in glob.glob(os.path.join(d, "*")) if not os.path.basename(f).startswith(".")}
mine = [f for f, t in texts.items() if len(t.split("\n")) > 3 and t.split("\n")[2].startswith("exe " + src + " ")]
print("calibration files", len(texts), "A's", len(mine))
if len(mine) == 1:
    body = texts[mine[0]].split("\n", 3)[3]
    for f, t in texts.items():
        open(f, "w").write("\n".join(t.split("\n")[:3]) + "\n" + body)
    print("calibration: A's body written under every arm's key")
PY
for i in $(seq 1 "$ROUNDS"); do
  if [ $((i % 2)) -eq 1 ]; then order="A B"; else order="B A"; fi
  for arm in $order; do
    sha=$A; [ "$arm" = B ] && sha=$B
    for op in insert_one update_plain; do for c in 1 32 128; do run "$sha" amphora "$op\$" "$c" "r$i-$arm-$op-$c"; done; done
  done
  run "$A" sqlite 'insert_one$' 1 "r$i-sqlite-insert_one-1"
done
python3 -I "$PY" summary "$O"
echo "$(date -u +%H:%M:%SZ) done $(state)"
