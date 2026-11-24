#!/usr/bin/env bash
# Lane Q6, run 2: the raw spin chains of run 1's profiles, then main (SHA) built in the lane's
# target dir (the bench workspace keeps no lock file, so no --locked; offline, from the registry
# cache) and profiled as q6_profile.sh does, at 32 and 128 clients.
# usage: SHA=<main sha> DEADLINE=<epoch s> q6_main.sh
set -uo pipefail
L=~/.cache/analytics-q6 T=~/.cache/amphora-perf-target/analytics TOOLS=~/.cache/analytics-q6/tools
export XDG_CACHE_HOME=$L/xdg
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
left() { echo $(( DEADLINE - $(date +%s) )); }
python3 -I "$TOOLS/q6_raw.py" 6 13 150 "$L/prof/q5bin-32.folded"
grep -h 'Finished' ~/.cache/analytics-q5/build-*.log | tail -3
echo "=== build main $SHA $(date -u +%H:%M:%SZ) $(state), $(left) s left"
git -C "$L/src" checkout -q --detach "$SHA" && git -C "$L/src" log --oneline -1
H=~/.cache/cargo-home-perf-analytics
budget=$(( $(left) - 75 ))
[ "$budget" -gt 60 ] || { echo "no time left to build main"; exit 0; }
(cd "$L/src/bench" && env CARGO_HOME="$H" CARGO_TARGET_DIR="$T" CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true \
  nice -n 10 timeout "$budget" cargo build --release 2>&1 | grep -E '^error|Compiling amphora|Finished' | tail -8)
NEW=$T/release/amphora-bench
if [ "$NEW" -nt "$L/src/bench/Cargo.toml" ] && [ "$(left)" -gt 40 ]; then
  mkdir -p ~/.cache/amphora-perf-bin/analytics && cp "$NEW" ~/.cache/amphora-perf-bin/analytics/amphora-bench-"$SHA"
  source <(sed -n '/^prof() {/,/^}/p' "$TOOLS/q6_profile.sh")
  for n in 32 128; do [ "$(left)" -gt 20 ] && prof "main-$n" ~/.cache/amphora-perf-bin/analytics/amphora-bench-"$SHA" "$n"; done
  python3 -I "$TOOLS/q6_spins.py" --top 12 "$L"/prof/main-*.folded
  python3 -I "$TOOLS/q6_raw.py" 4 13 150 "$L/prof/main-32.folded"
else
  echo "main not profiled: build unfinished or no time ($(left) s left)"
fi
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
