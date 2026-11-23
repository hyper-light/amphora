#!/bin/sh
# AUDIT 7.5 reopen A/B: crates/engine/examples/versioned_reopen.rs built at BEFORE and AFTER, each
# in its own clone (outside kiln's reach folder) and target, then REPS reps alternating before/after, the load beside each.
set -u
. "$(dirname "$0")/env.sh"
BEFORE=${BEFORE:-a07b635e}; AFTER=${AFTER:-ac58af91}; REPS=${REPS:-5}; ROWS=${ROWS:-20000}; REOPENS=${REOPENS:-20}
W="$HOME/.cache/versioned-ab"
mkdir -p "$W"
EX=crates/engine/examples/versioned_reopen.rs
for side in before after; do
  eval sha=\$$(echo "$side" | tr a-z A-Z)
  rm -rf "$W/reopen-$side"
  git clone -q --no-checkout . "$W/reopen-$side" && git -C "$W/reopen-$side" checkout -q --detach "$sha" || exit 1
  cp "$EX" "$W/reopen-$side/$EX"
  (cd "$W/reopen-$side" && CARGO_TARGET_DIR="$HOME/.cache/amphora-perf-target/versioned-ab-$side" nice -n 10 cargo build --release --locked -p amphora-engine --example versioned_reopen 2>&1 | tail -n 1) || exit 1
  cp "$HOME/.cache/amphora-perf-target/versioned-ab-$side/release/examples/versioned_reopen" "$W/reopen-bin-$side"
  echo "$side = $(git rev-parse "$sha")"
done
echo "host $(hostname -s) cpus $(nproc 2>/dev/null) load $(load)"
for rep in $(seq 1 "$REPS"); do
  for side in before after; do
    rm -rf "$W/reopen-db-$side"
    printf '%s rep=%s load=[%s] ' "$side" "$rep" "$(load)"
    "$W/reopen-bin-$side" "$W/reopen-db-$side" "$ROWS" "$REOPENS"
  done
done
rm -rf "$W/reopen-db-before" "$W/reopen-db-after"
