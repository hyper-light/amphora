#!/usr/bin/env bash
# Lane W3 (oltp): the flusher with no gathering wait (B) against main before it (A), alternated
# (A,B then B,A). Each round: insert_one at 1, 32 and 128 clients (W2's untraced suite command),
# update_plain at 32 and 128, and SQLite's insert_one at 1 client. Each run is a fresh process on a
# fresh database. A load-only run per arm gives the load's own rows, commits and flushes, so a run's
# rows a flush cover its warm-up and timed window alone. Then traced runs (stages.c) of each arm at 1
# and 32 clients: per-insert stages, and log writes a flush.
# Run from a copy outside the clone: it checks out each arm to build it.
set -uo pipefail
A=${1:?A sha}; B=${2:?B sha}; ROUNDS=${3:-5}; HERE=$(cd "$(dirname "$0")" && pwd)
L=$HOME/.cache/amphora-perf/oltp-run; R=$HOME/.cache/amphora-perf/oltp-remote; BIN=$HOME/.cache/amphora-perf-bin/oltp
O=$L/ab-w3; mkdir -p "$O" "$BIN"; rm -f "${O:?}"/*
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
export CARGO_HOME=$HOME/.cache/cargo-home-perf-oltp CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/oltp CARGO_BUILD_JOBS=${JOBS:-4} CARGO_INCREMENTAL=0 \
  CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
cd "$R" || exit 3
git fetch -q origin main perf/oltp || exit 3
for sha in "$A" "$B"; do
  [ -x "$BIN/amphora-bench-$sha" ] && continue
  git checkout -q --detach "$sha" || { echo "checkout $sha failed"; exit 3; }
  echo "$(date -u +%H:%M:%SZ) build $sha $(state)"
  nice -n 10 cargo build --release --offline --manifest-path bench/Cargo.toml --bin amphora-bench >"$O/build-$sha.log" 2>&1 || { tail -20 "$O/build-$sha.log"; exit 4; }
  cp "$CARGO_TARGET_DIR/release/amphora-bench" "$BIN/amphora-bench-$sha"
done
echo "$(date -u +%H:%M:%SZ) built $(state) nproc $(nproc)"
cc -O2 -shared -fPIC -o "$O/stages.so" "$HERE/stages.c" -ldl || exit 4
D=$L/data-ab; mkdir -p "$D" && chattr +C "$D" 2>/dev/null
export XDG_CACHE_HOME=$L/xdg-ab
run() {  # arm-sha target op clients label [traced]
  rm -rf "${D:?}/x"; local s; s=$(state)
  local args=(--target "$2" --dir "$D/x" --suite --only "$3" --items 200000 --clients "$4" --seconds 1 --warmup 0.5)
  if [ "${6:-0}" = 1 ]; then
    STAGES_OUT="$O/$5.rec" LD_PRELOAD="$O/stages.so" nice -n 10 "$BIN/amphora-bench-$1" "${args[@]}" 2>>"$O/runs.err" | tail -1 >"$O/$5.json"
  else
    nice -n 10 "$BIN/amphora-bench-$1" "${args[@]}" 2>>"$O/runs.err" | tail -1 >"$O/$5.json"
  fi
  python3 -I "$HERE/ab_w3.py" line "$O/$5.json" "$5" "$s"
}
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
    for c in 1 32 128; do run "$sha" amphora 'insert_one$' "$c" "r$i-$arm-insert_one-$c"; done
    for c in 32 128; do run "$sha" amphora 'update_plain$' "$c" "r$i-$arm-update_plain-$c"; done
  done
  run "$A" sqlite 'insert_one$' 1 "r$i-sqlite-insert_one-1"
done
python3 -I "$HERE/ab_w3.py" summary "$O"
for arm in A B; do
  sha=$A; [ "$arm" = B ] && sha=$B
  for c in 1 32; do run "$sha" amphora 'insert_one$' "$c" "traced-$arm-insert_one-$c" 1; done
done
for arm in A B; do for c in 1 32; do echo "== traced $arm insert_one $c clients"; python3 -I "$HERE/stages.py" "$O/traced-$arm-insert_one-$c.rec" amphora; done; done
echo "$(date -u +%H:%M:%SZ) done $(state)"
