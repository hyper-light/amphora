#!/usr/bin/env bash
# Lane Q8: K0's mixed phase profiled at 32 and 128 clients for each SHA in ARMS (lane Q6's prof():
# AMPHORA_PROFILE at 997 Hz, the bench's threads' scheduler counters, the load and CPU and I/O
# pressure before and after each run), each run's mixed-phase flushes beside it, then q7_limit.py and
# q8_frames.py (every sample at the frame cache's lock by caller, every taker's samples by what it
# does). Each arm built from its SHA in the lane's worktree and target dir, one after the other.
# usage: ARMS="<sha> ..." TOOLSHA=<sha holding q8_frames.py> JOBS=<n> TAG=<prefix> q8_prof.sh
set -uo pipefail
ARMS=${ARMS:?} TOOLSHA=${TOOLSHA:?} JOBS=${JOBS:-8} TAG=${TAG:-p}
L=~/.cache/analytics-q8 SRC=~/.cache/analytics-q6/src T=~/.cache/amphora-perf-target/analytics R=~/.cache/amphora-perf/analytics-remote
BIN=~/.cache/amphora-perf-bin/analytics H=~/.cache/cargo-home-perf-analytics TOOLS=$L/tools
RQ=bench/records/2026-11-24-adacachy2-perf-reads
export XDG_CACHE_HOME=$L/xdg
mkdir -p "$L/k0" "$L/prof" "$BIN" "$XDG_CACHE_HOME" "$TOOLS" || exit 1
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
echo "=== start $(date -u +%H:%M:%SZ) $(state) cores $(nproc) jobs $JOBS"
ps -eo pcpu,nlwp,comm --sort=-pcpu | head -8
git -C "$R" fetch -q origin main perf/analytics || exit 1
for f in q6-q6_profile.sh q6-q6_sched.py q7-q7_limit.py q8-q8_frames.py; do
  git -C "$R" show "$TOOLSHA:$RQ-${f%%-*}/${f#*-}" > "$TOOLS/${f#*-}" || exit 1
done
[ -d "$H" ] || { mkdir -p "$H" && for d in registry git bin; do ln -s ~/.cargo/$d "$H/$d"; done; }
for sha in $ARMS; do
  [ -x "$BIN/amphora-bench-$sha" ] && continue
  git -C "$SRC" checkout -q --detach "$sha" || exit 1
  echo "=== build $sha $(date -u +%H:%M:%SZ) $(state)"
  (cd "$SRC/bench" && env CARGO_HOME="$H" CARGO_TARGET_DIR="$T" CARGO_BUILD_JOBS="$JOBS" CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true \
    nice -n 10 cargo build --release > "$L/build-${sha:0:8}.log" 2>&1); rc=$?
  grep -E '^error|Compiling amphora-engine |Finished' "$L/build-${sha:0:8}.log" | tail -3
  [ "$rc" = 0 ] && [ "$(git -C "$SRC" rev-parse HEAD)" = "$sha" ] || { echo "build of $sha failed ($rc)"; tail -20 "$L/build-${sha:0:8}.log"; exit 1; }
  cp "$T/release/amphora-bench" "$BIN/amphora-bench-$sha" || exit 1
done
source <(sed -n '/^prof() {/,/^}/p' "$TOOLS/q6_profile.sh")
for n in 32 128; do for sha in $ARMS; do
  prof "$TAG-${sha:0:8}-$n" "$BIN/amphora-bench-$sha" "$n" | grep -v '^{'
  python3 -I -c '
import json, sys
x = json.loads(open(sys.argv[1]).read().strip().splitlines()[-1])
m, p = x["stats_after_mixed"], x["stats_after_preload"]
f, fm = m["flushes"] - p["flushes"], m["flush_ms"] - p["flush_ms"]
print("reads/s %.0f events/s %.0f events_written %d cores %.1f failures %s unanswered %s mixed flushes %d ms a flush %.0f" % (x["read_ms"]["n"] / x["seconds"], x["write_events_per_s"], x["events_written"], (x.get("mixed_cpu_s") or 0) / x["seconds"], x.get("failures"), x.get("unanswered"), f, fm / f if f else float("nan")))' "$L/k0/$TAG-${sha:0:8}-$n.out"
done; done
python3 -I "$TOOLS/q7_limit.py" --top 15 "$L"/prof/"$TAG"-*.folded
python3 -I "$TOOLS/q8_frames.py" "$L"/prof/"$TAG"-*.folded
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
