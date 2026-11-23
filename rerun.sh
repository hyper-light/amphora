#!/bin/bash
# J3 second check: stack-only failures rerun alone on main and stack prefixes; I's db suite.
set -u
export CARGO_HOME=$HOME/.cache/cargo-home-perf-j2 PATH=$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=false CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=16
export CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/main-baseline
SRC=$HOME/.cache/amphora-perf/j2-remote; LOGS=$HOME/.cache/amphora-perf/j2-logs
cd "$SRC" || exit 2
git fetch -q origin '+refs/heads/*:refs/remotes/origin/*' || exit 2
echo "== I's earlier branch failure:"; grep -n -A12 'a_change_another_lead_wrote_over_is_never_answered_done' ~/.cache/concurrency-ab/logs/u1-i4-branch.log | grep -v '^.*test .* ok$' | head -30
r() { local log=$1; shift; timeout 900 nice -n 10 cargo "$@" >"$log" 2>&1; echo "  exit=$? $(grep -E 'test result' "$log" | tail -1) load=$(cut -d' ' -f1-3 /proc/loadavg)"; grep -E '^test .* FAILED$' "$log" | sort -u; }
for arg in main:5bbf63bd E:084aba06 K:615652c5 G:edd274b3; do
  label=${arg%%:*}; sha=${arg#*:}; git checkout -q --detach "$sha" || continue
  echo "#### $label $sha t_qt_6_30 x3"
  for i in 1 2 3; do r $LOGS/j3re-$label-$sha-q630-$i.log test -p amphora-engine --lib --locked -- t_qt_6_30 --test-threads=16; done
  if [ $label = main ] || [ $label = G ]; then
    echo "#### $label records_conformance x3"
    for i in 1 2 3; do r $LOGS/j3re-$label-$sha-rc-$i.log test -p amphora --locked --test records_conformance -- --test-threads=16; done
  fi
done
for arg in conc:cd372c69 main:5bbf63bd; do
  label=${arg%%:*}; sha=${arg#*:}; git checkout -q --detach "$sha" || continue
  echo "#### $label database:: x2"
  for i in 1 2; do r $LOGS/j3re-$label-$sha-db-$i.log test -p amphora --lib --locked database:: -- --test-threads=16; done
done
echo RDONE
