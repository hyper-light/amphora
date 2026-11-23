#!/bin/bash
# J3: a_change_another_lead_wrote_over_is_never_answered_done alone, 6 times, per label:sha.
set -u
export CARGO_HOME=$HOME/.cache/cargo-home-perf-j2 PATH=$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=false CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=16
export CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/main-baseline
SRC=$HOME/.cache/amphora-perf/j2-remote; LOGS=$HOME/.cache/amphora-perf/j2-logs
cd "$SRC" || exit 2
git fetch -q origin '+refs/heads/*:refs/remotes/origin/*' || exit 2
T=a_change_another_lead_wrote_over_is_never_answered_done
for arg in "$@"; do
  label=${arg%%:*}; sha=${arg#*:}
  git checkout -q --detach "$sha" || { echo "no sha $sha"; continue; }
  echo "#### I-test $label $(git rev-parse --short HEAD)"
  timeout 2400 nice -n 10 cargo test -p amphora --lib --locked --no-run >"$LOGS/itest-$label-${sha:0:8}-build.log" 2>&1 || { echo "build failed"; tail -5 "$LOGS/itest-$label-${sha:0:8}-build.log"; continue; }
  for i in 1 2 3 4 5 6; do
    log=$LOGS/itest-$label-${sha:0:8}-$i.log
    timeout 600 nice -n 10 cargo test -p amphora --lib --locked -- $T --test-threads=1 >"$log" 2>&1
    echo "run $i exit=$? $(grep -E 'test result' "$log" | tail -1) load=$(cut -d' ' -f1-3 /proc/loadavg)"
  done
done
echo IDONE
