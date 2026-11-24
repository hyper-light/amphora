#!/usr/bin/env bash
# Lane Q6: K0's mixed phase (N writers commit while N readers read the newest pages) profiled with
# AMPHORA_PROFILE (997 Hz) at 32 and 128 clients, each run's threads' scheduler counters kept
# (q6_sched.py), the host's load and CPU and I/O pressure recorded before and after every run.
#   1. Q5's binary (b6e2e72f, the `optimistic` way main kept) at 32 and 128: the fallback;
#   2. main (SHA) built in the lane's target dir, then profiled the same way;
#   3. every spin sample attributed by call chain (q6_spins.py).
# usage: SHA=<main sha> DEADLINE=<epoch s the run must end by> q6_profile.sh
set -uo pipefail
L=~/.cache/analytics-q6 T=~/.cache/amphora-perf-target/analytics R=~/.cache/amphora-perf/analytics-remote
OLD=~/.cache/amphora-perf-bin/analytics/amphora-bench-b6e2e72fd5a5ae401d17cd27b2ff903781e60577
TOOLS=$L/tools
export XDG_CACHE_HOME=$L/xdg
mkdir -p "$L/prof" "$L/k0" "$XDG_CACHE_HOME" || exit 1
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
left() { echo $(( DEADLINE - $(date +%s) )); }
echo "=== start $(date -u +%H:%M:%SZ) $(state) cores $(nproc)"
ps -eo pcpu,nlwp,comm --sort=-pcpu | head -9
prof() { # tag bin clients
  local tag=$1 bin=$2 n=$3 dir=$L/k0/$1 pid rc
  rm -rf "${dir:?}"
  echo "=== $tag $(date -u +%H:%M:%SZ) $(state)"
  env AMPHORA_LOG_READ_INDEX=optimistic AMPHORA_PROFILE="$L/prof/$tag" nice -n 10 "$bin" --target amphora --dir "$dir" \
    --sessions 1000 --preload 50 --writers "$n" --readers "$n" --seconds 6 --warmup 1 --searches 3 > "$dir.out" 2>>"$L/err.log" &
  pid=$!
  python3 -I "$TOOLS/q6_sched.py" "$pid" > "$L/prof/$tag.sched"
  wait "$pid"; rc=$?
  echo "exit $rc $(date -u +%H:%M:%SZ) after: $(state)"
  tail -1 "$dir.out" | cut -c1-1200
  cat "$L/prof/$tag.sched"
  rm -rf "${dir:?}"
}
for n in 32 128; do prof "q5bin-$n" "$OLD" "$n"; done
python3 -I "$TOOLS/q6_spins.py" --top 25 "$L"/prof/q5bin-*.folded
echo "=== build main $SHA $(date -u +%H:%M:%SZ) $(state), $(left) s left"
if [ ! -d "$L/src" ]; then git -C "$R" fetch -q origin main && git -C "$R" worktree add -q --detach "$L/src" "$SHA" || exit 1; fi
git -C "$L/src" checkout -q --detach "$SHA" && git -C "$L/src" log --oneline -1
H=~/.cache/cargo-home-perf-analytics
[ -d "$H" ] || { mkdir -p "$H" && for d in registry git bin; do ln -s ~/.cargo/$d "$H/$d"; done; }
budget=$(( $(left) - 75 ))
[ "$budget" -gt 60 ] || { echo "no time left to build main"; exit 0; }
(cd "$L/src/bench" && env CARGO_HOME="$H" CARGO_TARGET_DIR="$T" CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true \
  nice -n 10 timeout "$budget" cargo build --release --locked 2>&1 | grep -E '^(error|warning: unused)|Compiling amphora|Finished' | tail -8)
NEW=$T/release/amphora-bench
if [ "$(git -C "$L/src" rev-parse HEAD)" = "$SHA" ] && [ "$NEW" -nt "$L/src/bench/Cargo.toml" ] && [ "$(left)" -gt 40 ]; then
  mkdir -p ~/.cache/amphora-perf-bin/analytics && cp "$NEW" ~/.cache/amphora-perf-bin/analytics/amphora-bench-"$SHA"
  for n in 32 128; do [ "$(left)" -gt 20 ] && prof "main-$n" ~/.cache/amphora-perf-bin/analytics/amphora-bench-"$SHA" "$n"; done
  python3 -I "$TOOLS/q6_spins.py" --top 25 "$L"/prof/main-*.folded
else
  echo "main not profiled: build unfinished or no time ($(left) s left)"
fi
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
