#!/usr/bin/env bash
# The check of record for query-speed step 5's join costs (QT.5): build join-methods in release, take the
# host's heavy lock (exit if another holds it), wait for the one-minute load to fall under 2 (CLAUDE.md
# §7; MAX_LOAD for a tiny check on a busy host, which is no record), then run every size, best of 3, in
# turns. Writes <out>/runs.jsonl and <out>/host.txt (the host, the commit and the load); frees the lock on
# any exit. Rounds and sizes, when given, go to join-methods (bench/baselines/run.sh join-kinds).
#   bench/join_methods/run.sh <out dir> [rounds [sizes...]]
set -euo pipefail
out=${1:?out dir}
shift
if [[ $# -eq 0 ]]; then set -- 3; fi
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
lock=$HOME/.cache/amphora-heavy.lock
export PATH=$HOME/.cargo/bin:/usr/sbin:$PATH
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$HOME/.cache/amphora-join-methods-target}
mkdir -p "$out"
if ! mkdir "$lock" 2>/dev/null; then
  echo "heavy lock held: $(cat "$lock/owner" 2>/dev/null)"; exit 75
fi
trap 'rm -rf "${lock:?}"' EXIT
echo "join-methods $(git -C "$here" rev-parse --short HEAD) $(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$lock/owner"
(cd "$here" && nice -n 10 cargo build --release -q)
load1() { sysctl -n vm.loadavg 2>/dev/null | awk '{print $2}' || awk '{print $1}' /proc/loadavg; }
until awk -v l="$(load1)" -v max="${MAX_LOAD:-2}" 'BEGIN { exit !(l < max) }'; do sleep 60; done
{
  echo "host $(hostname) $(sysctl -n hw.model 2>/dev/null) $(sysctl -n machdep.cpu.brand_string 2>/dev/null) cores $(sysctl -n hw.ncpu 2>/dev/null) mem $(sysctl -n hw.memsize 2>/dev/null)"
  echo "commit $(git -C "$here" rev-parse HEAD) diff-sha256 $(git -C "$here" diff HEAD | shasum -a 256 | cut -d' ' -f1)"
  echo "start $(date -u +%Y-%m-%dT%H:%M:%SZ) load $(sysctl -n vm.loadavg 2>/dev/null || cut -d" " -f1-3 /proc/loadavg)"
} > "$out/host.txt"
git -C "$here" diff HEAD > "$out/uncommitted.diff"
db=$HOME/.cache/amphora-join-methods-db; mkdir -p "$db"
"$CARGO_TARGET_DIR/release/join-methods" "$db" "$@" > "$out/runs.jsonl"
rm -rf "${db:?}"
echo "end $(date -u +%Y-%m-%dT%H:%M:%SZ) load $(sysctl -n vm.loadavg 2>/dev/null || cut -d" " -f1-3 /proc/loadavg)" >> "$out/host.txt"
