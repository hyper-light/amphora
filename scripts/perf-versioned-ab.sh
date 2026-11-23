#!/bin/sh
# perf/versioned (AUDIT 2.4) A/B: the point-read example built at origin/main and at HEAD, run
# alternately on this machine at 1 to 64 clients, both storages.
set -eu
CH="$HOME/.cache/cargo-home-perf-versioned"
mkdir -p "$CH"
for d in registry git bin; do [ -e "$CH/$d" ] || ln -s "$HOME/.cargo/$d" "$CH/$d"; done
export CARGO_HOME="$CH" CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=8
PATH="$HOME/.cargo/bin:$PATH"
OUT="$HOME/.cache/amphora-perf-versioned-ab"
mkdir -p "$OUT"
HERE=$(pwd)
BASE="$HOME/.cache/amphora-perf-versioned-base-rel"
BASE_SHA=${BASE_SHA:-$(git merge-base HEAD origin/main)}
if [ ! -d "$BASE" ]; then git worktree add --detach "$BASE" "$BASE_SHA"; else (cd "$BASE" && git checkout -q --detach "$BASE_SHA"); fi
mkdir -p "$BASE/crates/engine/examples"
cp crates/engine/examples/versioned_point_reads.rs "$BASE/crates/engine/examples/"
CARGO_TARGET_DIR="$HOME/.cache/amphora-perf-target/versioned-rel" nice -n 10 cargo build --release --locked -p amphora-engine --example versioned_point_reads > "$OUT/after.log" 2>&1 &
A=$!
(cd "$BASE" && CARGO_TARGET_DIR="$HOME/.cache/amphora-perf-target/versioned-base" nice -n 10 cargo build --release --locked -p amphora-engine --example versioned_point_reads > "$OUT/before.log" 2>&1) &
B=$!
wait $A; wait $B
tail -2 "$OUT/after.log" "$OUT/before.log"
cp "$HOME/.cache/amphora-perf-target/versioned-rel/release/examples/versioned_point_reads" "$OUT/after"
cp "$HOME/.cache/amphora-perf-target/versioned-base/release/examples/versioned_point_reads" "$OUT/before"
echo "base $BASE_SHA head $(git rev-parse HEAD) dirty $(git status --porcelain | wc -l) host $(hostname -s) $(sysctl -n machdep.cpu.brand_string 2>/dev/null) load $(sysctl -n vm.loadavg 2>/dev/null) $(cat /proc/loadavg 2>/dev/null) cpus $(nproc 2>/dev/null)"
for rep in 1 2 3; do
  echo "rep=$rep load $(cat /proc/loadavg 2>/dev/null)$(sysctl -n vm.loadavg 2>/dev/null)"
  for c in 1 8 32 64; do
    n=$((20000 / c + 200))
    for storage in versioned default; do
      for v in before after; do
        printf '%s rep=%s ' "$v" "$rep"; "$OUT/$v" "$storage" "$OUT/data" 10000 "$c" "$n"
      done
    done
  done
done
