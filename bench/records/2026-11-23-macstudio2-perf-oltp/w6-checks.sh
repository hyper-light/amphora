#!/usr/bin/env bash
# Lane W6 (oltp): the merge gate on perf/oltp merged with main (W5's w5-checks.sh, reordered so each
# build is reused): the store's, crash and lib store tests, the design index, clippy; then decision
# 14's two new tests on main's store.rs (RED=<sha>, 3 runs) and the design index on main's docs.
# Run from the repository's root.
set -uo pipefail
[ "$(df -g ~ | tail -1 | awk '{print $4}')" -ge 40 ] || { echo "disk low"; exit 2; }
CH=$HOME/.cache/cargo-home-perf-oltp; mkdir -p "$CH"
for d in registry git bin; do [ -e "$CH/$d" ] || ln -s "$HOME/.cargo/$d" "$CH/$d"; done
export CARGO_HOME=$CH CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/oltp CARGO_BUILD_JOBS=8 CARGO_INCREMENTAL=0 PATH=$HOME/.cargo/bin:$PATH
rc=0
step() { echo "STEP $(date -u +%H:%M:%SZ) $*"; nice -n 10 "$@"; local r=$?; echo "STEP rc $r"; [ $r -eq 0 ] || rc=1; }
T="cargo test -p amphora --locked --offline"
NEW="a_batch_handed_before_a_flush_begins a_sender_told_durable_that_sends_nothing_more"
doc() { nice -n 10 $T --test design_index 2>&1 | grep -E 'has no line|no quote cited|^test result|FAILED|error'; }
step $T --test store -- --test-threads=8
step $T --test crash -- --test-threads=8
step $T --lib store -- --test-threads=8
echo "STEP design index, this tree"; doc
step cargo clippy -p amphora --all-targets --locked --offline -- -D warnings
if [ -n "${RED:-}" ]; then
  git show "$RED:crates/amphora/src/store.rs" > crates/amphora/src/store.rs || exit 3
  for i in 1 2 3; do
    echo "STEP red $i, $RED's store.rs"
    nice -n 10 $T --test store -- --test-threads=8 $NEW 2>&1 | grep -E '^test |panicked|left:|right:|^test result|error'
  done
  git show "$RED:docs/design.md" > docs/design.md; git show "$RED:docs/design/store.md" > docs/design/store.md
  echo "STEP design index, $RED's docs"; doc
  git checkout -- crates/amphora/src/store.rs docs/design.md docs/design/store.md
fi
echo "STEP $(date -u +%H:%M:%SZ) done rc $rc"; exit $rc
