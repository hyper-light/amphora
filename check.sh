#!/bin/bash
# J3 integrator check: for each "label:sha" arg, check out the sha in the GitHub clone on this machine,
# then run the baseline suites; logs under ~/.cache/amphora-perf/j2-logs/<label>-<sha>-<suite>.log.
# Optional env CLIPPY_ON=<label> runs clippy on amphora + amphora-engine first for that label; EXTRA="<cargo test args>;..." adds suites.
set -u
export CARGO_HOME=$HOME/.cache/cargo-home-perf-j2
mkdir -p "$CARGO_HOME"
for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -s "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
export PATH=$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=false CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=16
export CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/main-baseline
SRC=$HOME/.cache/amphora-perf/j2-remote
LOGS=$HOME/.cache/amphora-perf/j2-logs
mkdir -p "$LOGS"
[ -d "$SRC/.git" ] || git clone -q https://github.com/hyper-light/amphora.git "$SRC" || git clone -q git@github.com:hyper-light/amphora.git "$SRC"
cd "$SRC" || exit 2
git fetch -q origin '+refs/heads/*:refs/remotes/origin/*' || exit 2
uptime; df -h "$HOME" | tail -1
suite() { # name, timeout_s, cargo args...
  local name=$1 to=$2; shift 2
  local log=$LOGS/$label-${sha:0:8}-$name.log
  echo "== $name $(date -u +%H:%MZ) -> $log"
  timeout "$to" nice -n 10 cargo "$@" >"$log" 2>&1
  local rc=$?
  echo "EXIT $name=$rc $(date -u +%H:%MZ) load=$(cut -d' ' -f1-3 /proc/loadavg) cpu=$(head -1 /proc/pressure/cpu 2>/dev/null) io=$(head -1 /proc/pressure/io 2>/dev/null)"
  grep -E 'test result|^error(\[|:)' "$log" | tail -6
  grep -E '^test .* FAILED$|^    [a-z_:0-9]+$' "$log" | sort -u | head -60
  grep -E 'has been running for over' "$log" | head -10
}
for arg in "$@"; do
  label=${arg%%:*}; sha=${arg#*:}
  git checkout -q --detach "$sha" || { echo "no sha $sha"; continue; }
  sha=$(git rev-parse HEAD)
  echo "#### $label $sha"
  if [ "$label" = "${CLIPPY_ON:-}" ]; then
    suite clippy 3600 clippy -p amphora -p amphora-engine --all-targets --locked -- -D warnings
  fi
  suite db 3600 test -p amphora --lib --locked database:: -- --test-threads=16
  suite repl 1800 test -p amphora --lib --locked -- replication:: store:: --test-threads=16
  suite storetests 1800 test -p amphora --locked --test store --test records_conformance -- --test-threads=16
  suite englib 1800 test -p amphora-engine --lib --locked -- bushy_search_tests t_qt_6_ audit_3_1 --skip t_qt_6_18 --skip t_pl_8_14 --test-threads=16
  suite unnesting 1800 test -p amphora-engine --features simulator --locked --test unnesting -- --test-threads=16
  suite column_stats 1800 test -p amphora-engine --locked --test column_stats -- --test-threads=16
  IFS=';' read -ra extras <<<"${EXTRA:-}"
  i=0; for e in "${extras[@]}"; do i=$((i+1)); [ -n "$e" ] && suite "extra$i" 2400 $e; done
done
echo DONE
