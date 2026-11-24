#!/bin/bash
# Lane P22 (serve) b1, adacachy2 (r349, needs cpus=2,mem=8g, jobs 2): V13's real-time cell (32 spread writers, both
# classes, its flags) on the merged branch H (perf/serve-p20-v13hold with main 6d8e7dcd) and on main M (6d8e7dcd), release
# amphora-bench built in turn in the lane's target dir (no Cargo.lock is tracked for bench/, so --offline), 3 rounds, arm
# order alternated, in lane P20's shared clone under ~/.cache/serve-p20-ab beside the remote clone. Prints each cell with
# the load, then a summary per arm and class: writes, refused (the driver's Serialization, a stale base, per node), deadline
# misses (unanswered within --deadline-s 10) and writers_missed.
set -u
H=d0c5722de0fb1085bd052ce1655856c13517af53; M=6d8e7dcd38be29214187a06bbfabdf9b054acb2a; J=2
A=$HOME/.cache/amphora-perf/serve-p7-remote; B=$HOME/.cache/serve-p20-ab/b1
export PATH=$HOME/.cargo/bin:$PATH CARGO_HOME=$HOME/.cache/cargo-home-perf-serve-p7
export CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/serve-p7 CARGO_BUILD_JOBS=$J CARGO_INCREMENTAL=0
L=$HOME/.cache/amphora-perf/serve-p22-b1; BIN=$HOME/.cache/amphora-perf-bin/serve-p22; mkdir -p "$L/cells" "$BIN"
ld() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu-some $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/cpu) io-some $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/io)"; }
echo "start $(date -u +%T) jobs $J $(ld)"; df -h ~ | tail -1
for s in $H $M; do git -C "$A" cat-file -e "$s^{commit}" || git -C "$A" fetch -q origin || { echo "NO $s"; exit 9; }; done
[ -d "$B/.git" ] || git clone -q --shared "$A" "$B" || exit 9
echo "tempfile in registry: $(ls $HOME/.cargo/registry/cache/*/ 2>/dev/null | grep -c '^tempfile-3')"
S=$L/v13; mkdir -p "$S"; git -C "$A" show $M:bench/records/2026-11-23-adacachy2-perf-commits-v13/cell13.py > "$S/cell13.py" || exit 9
for sha in $H $M; do
  mkdir -p "$BIN/$sha"; git -C "$B" checkout -q -f --detach $sha || exit 9
  s=$(date +%s); ( cd "$B/bench" && nice -n 10 cargo build --release --offline > "$L/build-bench-${sha:0:8}.log" 2>&1 ) && cp "$CARGO_TARGET_DIR/release/amphora-bench" "$BIN/$sha/bench" \
    || { echo "BUILD_FAILED bench ${sha:0:8}"; grep -E '^error' -A12 "$L/build-bench-${sha:0:8}.log" | head -40; tail -5 "$L/build-bench-${sha:0:8}.log"; exit 8; }
  echo "built bench ${sha:0:8} in $(( $(date +%s) - s ))s $(date -u +%T) $(ld); $(grep -E 'Compiling amphora' "$L/build-bench-${sha:0:8}.log" | tr '\n' ' ')"
  echo "  ${sha:0:8} binary: $(stat -c %s "$BIN/$sha/bench") bytes, session-worker text in it: $(grep -a -c 'second session handed to the session worker' "$BIN/$sha/bench") (branch expects >=1, main 0)"
done
for r in 1 2 3; do
  if [ $((r % 2)) -eq 1 ]; then arms="$H $M"; else arms="$M $H"; fi
  for sha in $arms; do for cls in persisted replicated; do
    echo "cell r$r ${sha:0:8} $cls $(date -u +%T) $(ld) $(nice -n 10 python3 -I "$S/cell13.py" "$BIN/$sha/bench" 31640 "$L/cells/${sha:0:8}-$cls-32-r$r" 32 spread "$cls" 2>&1 | tail -3 | tr '\n' ' ')"
  done; done
done
echo "end cells $(date -u +%T) $(ld)"
python3 -I - "$L/cells" <<'PY'
import json, glob, os, sys, collections
tot = collections.defaultdict(lambda: dict(cells=0, writes=0, refused=0, misses=0, writers_missed=0, rate=[], p50=[], p99=[], load=[]))
for f in sorted(glob.glob(os.path.join(sys.argv[1], "*", "rec.jsonl"))):
    name = os.path.basename(os.path.dirname(f)); sha, cls = name.split("-")[0], name.split("-")[1]
    r = json.loads(open(f).read().strip().splitlines()[-1])
    k = (sha, cls); t = tot[k]; t["cells"] += 1
    n = sum(b["latency_ms"]["n"] for b in r["by_node"]); t["writes"] += n
    t["refused"] += sum(b["refused"] for b in r["by_node"]); t["misses"] += r["deadline_misses"]; t["writers_missed"] += r["writers_missed"]
    t["rate"].append(r["writes_per_s"]); t["p50"].append(r["latency_ms"]["p50"]); t["p99"].append(r["latency_ms"]["p99"]); t["load"].append(r.get("load1"))
for k in sorted(tot):
    t = tot[k]; print("SUM", k, {x: (round(v, 1) if isinstance(v, float) else v) for x, v in t.items()})
PY
echo "end $(date -u +%T) $(ld)"
