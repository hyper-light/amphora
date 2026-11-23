#!/usr/bin/env bash
# perf/writes A/B (AUDIT 2.5): records_load built from the working tree (after) and from $BASE's
# copies of $FILES (before; default origin/main's database.rs, database/remote.rs and store.rs),
# run alternately, ROUNDS times. Run from the repository root: bash bench/records_ab.sh
set -uo pipefail
export CARGO_HOME=$HOME/.cache/cargo-home-perf-writes CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/writes CARGO_BUILD_JOBS=8 PATH=$HOME/.cargo/bin:$PATH
B=$HOME/.cache/amphora-perf-bench/writes; mkdir -p "$B/saved"
BASE=${BASE:-origin/main}
FILES=${FILES:-crates/amphora/src/database.rs crates/amphora/src/database/remote.rs crates/amphora/src/store.rs}
nice -n 10 cargo build --release --locked -p amphora --example records_load 2>&1 | tail -2 || exit 1
cp "$CARGO_TARGET_DIR/release/examples/records_load" "$B/after"
for f in $FILES; do mkdir -p "$B/saved/$(dirname "$f")"; cp "$f" "$B/saved/$f"; git show "$BASE:$f" > "$f"; done
nice -n 10 cargo build --release --locked -p amphora --example records_load 2>&1 | tail -2; built=$?
cp "$CARGO_TARGET_DIR/release/examples/records_load" "$B/before"
for f in $FILES; do cp "$B/saved/$f" "$f"; done
[ $built -eq 0 ] || exit 1
echo "host=$(hostname) nproc=$(nproc) load=$(cut -d' ' -f1-3 /proc/loadavg) commit=$(git rev-parse --short HEAD) base=$(git rev-parse --short "$BASE")"
for round in $(seq 1 "${ROUNDS:-2}"); do for v in before after; do
  d="$B/db-$v-$round"; rm -rf "${d:?}"; mkdir -p "$d"
  echo "== $v round $round load=$(cut -d' ' -f1 /proc/loadavg)"
  "$B/$v" "$d" "${SECS:-3}" "${CLIENTS:-1,8,64,256,1024}" "${OPS:-get}" 2>&1
  rm -rf "${d:?}"
done; done
