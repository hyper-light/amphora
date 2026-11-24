#!/usr/bin/env bash
# Lane Q7, step 1: the merged tree's checks on macOS (RULES 6): cargo fmt, clippy on amphora-engine
# with the engine's root touched so it is re-checked (the log must show "Checking amphora-engine";
# Q6's run exited 0 without that line), Q6's tests (storage::wal::test, packed_log, log_read_index,
# spilled_log) and scripts/check-production.sh. Every step runs, check-production before the log tests (RULES 5b gates the merge on clippy and the
# wal tests, which come first); the exit code is the first failure's.
# usage (kiln run, synced from the lane's clone): [STEPS='fmt clippy wal log production'] bash bench/records/2026-11-24-adacachy2-perf-reads-q7/q7_check.sh
set -uo pipefail
[ "$(df -g ~ | tail -1 | awk '{print $4}')" -ge 40 ] || { echo "disk low"; exit 2; }
L=~/.cache/analytics-q7 H=~/.cache/cargo-home-perf-analytics
mkdir -p "$L/py" || exit 1
# scripts/engine-ratchet.py imports tomllib (Python 3.11): r159's check-production failed on this
# Mac's older python3 before the engine's count ran. The first python3 that has it goes first in PATH.
export PATH=$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH
for p in python3.14 python3.13 python3.12 python3.11 python3; do
  q=$(command -v "$p") && "$q" -c 'import tomllib' 2>/dev/null && { ln -sf "$q" "$L/py/python3"; break; }
done
export PATH=$L/py:$PATH
echo "=== python3 $(python3 -c 'import sys; print(sys.version.split()[0])') tomllib $(python3 -c 'import tomllib' 2>&1 && echo yes)"
[ -d "$H" ] || { mkdir -p "$H" && for d in registry git bin; do ln -s ~/.cargo/$d "$H/$d"; done; }
export CARGO_HOME=$H CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/analytics CARGO_BUILD_JOBS=8
echo "=== tree $(git rev-parse HEAD) dirty $(git status --porcelain -- crates Cargo.toml Cargo.lock | wc -l | tr -d ' ') $(date -u +%H:%M:%SZ) $(uname -sm) load $(sysctl -n vm.loadavg)"
first=0
STEPS=${STEPS:-fmt clippy wal log production}
want() { case " $STEPS " in *" $1 "*) return 0 ;; esac; return 1; }
step() { # name filter command...
  local name=$1 filter=$2 log=$L/$1.log rc; shift 2
  echo "=== $name $(date -u +%H:%M:%SZ)"
  nice -n 10 "$@" > "$log" 2>&1; rc=$?
  grep -E "$filter" "$log" | tail -40
  echo "=== $name exit $rc $(date -u +%H:%M:%SZ)"
  [ "$rc" = 0 ] || [ "$first" != 0 ] || first=$rc
}
want fmt && step fmt '.' cargo fmt --all --check
want clippy && touch crates/engine/lib.rs && step clippy-engine '^(warning|error)|Checking amphora-engine|Finished' cargo clippy -p amphora-engine --all-targets --locked -- -D warnings
want clippy && { n=$(grep -c 'Checking amphora-engine' "$L/clippy-engine.log"); echo "=== clippy-engine Checking amphora-engine lines: $n"
[ "$n" -gt 0 ] || [ "$first" != 0 ] || first=3; }
want wal && step wal-tests 'test result:|FAILED|panicked|q6_' cargo test -p amphora-engine --locked --lib -- storage::wal::test:: --test-threads=8
want production && step check-production '.' scripts/check-production.sh
want log && step log-tests 'Running|test result:|FAILED|panicked' cargo test -p amphora-engine --locked --test packed_log --test log_read_index --test spilled_log -- --test-threads=8
echo "=== end $(date -u +%H:%M:%SZ) exit $first"
exit "$first"
