#!/usr/bin/env bash
# Lane W5 (oltp): clippy and the store's tests on the tree as synced, then decision 14's first new
# test on main's store.rs (RED=<sha>) for its red run. Run from the repository's root.
set -uo pipefail
[ "$(df -g ~ | tail -1 | awk '{print $4}')" -ge 40 ] || { echo "disk low"; exit 2; }
CH=$HOME/.cache/cargo-home-perf-oltp; mkdir -p "$CH"
for d in registry git bin; do [ -e "$CH/$d" ] || ln -s "$HOME/.cargo/$d" "$CH/$d"; done
export CARGO_HOME=$CH CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/oltp CARGO_BUILD_JOBS=8 CARGO_INCREMENTAL=0 PATH=$HOME/.cargo/bin:$PATH
rc=0
step() { echo "== $(date -u +%H:%M:%SZ) $*"; nice -n 10 "$@"; local r=$?; echo "== rc $r"; [ $r -eq 0 ] || rc=1; }
step cargo clippy -p amphora --all-targets --locked --offline -- -D warnings
step cargo test -p amphora --locked --offline --test store -- --test-threads=8
step cargo test -p amphora --locked --offline --test crash -- --test-threads=8
step cargo test -p amphora --locked --offline --lib store -- --test-threads=8
step cargo test -p amphora --locked --offline --test design_index -- --test-threads=8
if [ -n "${RED:-}" ]; then
  git show "$RED:crates/amphora/src/store.rs" > crates/amphora/src/store.rs || exit 3
  for i in 1 2 3; do
    echo "== red $i on $RED's store.rs"; nice -n 10 cargo test -p amphora --locked --offline --test store a_batch_handed_before_a_flush_begins -- --test-threads=8 2>&1 | grep -E 'test a_batch|panicked|left:|right:|x and y|test result' ; done
  git checkout -- crates/amphora/src/store.rs
fi
echo "== $(date -u +%H:%M:%SZ) done rc $rc"; exit $rc
