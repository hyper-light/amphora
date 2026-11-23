#!/usr/bin/env bash
# perf/writes A/B (AUDIT 2.5): records_load built from the working tree (after) and from HEAD's
# database.rs (before), run alternately. Run from the repository root: bash bench/records_ab.sh
set -uo pipefail
export CARGO_HOME=$HOME/.cache/cargo-home-perf-writes CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/writes CARGO_BUILD_JOBS=8 PATH=$HOME/.cargo/bin:$PATH
B=$HOME/.cache/amphora-perf-bench/writes; mkdir -p "$B"
F=crates/amphora/src/database.rs
nice -n 10 cargo build --release --locked -p amphora --example records_load 2>&1 | tail -2 || exit 1
cp "$CARGO_TARGET_DIR/release/examples/records_load" "$B/after"
cp $F "$B/database.rs.after"
git show HEAD:$F > $F
nice -n 10 cargo build --release --locked -p amphora --example records_load 2>&1 | tail -2 || { cp "$B/database.rs.after" $F; exit 1; }
cp "$CARGO_TARGET_DIR/release/examples/records_load" "$B/before"
cp "$B/database.rs.after" $F
echo "host=$(hostname) nproc=$(nproc) load=$(cut -d' ' -f1-3 /proc/loadavg) commit=$(git rev-parse --short HEAD)"
for round in 1 2; do for v in before after; do
  d="$B/db-$v-$round"; rm -rf "${d:?}"; mkdir -p "$d"
  echo "== $v round $round load=$(cut -d' ' -f1 /proc/loadavg)"
  "$B/$v" "$d" "${SECS:-3}" "${CLIENTS:-1,8,64,256,1024}" "${OPS:-get}" 2>&1
  rm -rf "${d:?}"
done; done
