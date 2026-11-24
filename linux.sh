#!/bin/bash
# Lane D5: Linux checks of R28/R30 on main. usage: linux.sh <mode> <sha> [jobs]   modes: lint | tests | prod | one <filter...>
set -u
MODE=${1:?} SHA=${2:?} J=${3:-8}
shift 3 2>/dev/null || true
URL=git@github.com:hyper-light/amphora.git
LOAD() { cut -d' ' -f1-3 /proc/loadavg; }
PSI() { echo "cpu $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/cpu) io $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/io)"; }
export CARGO_HOME=$HOME/.cache/cargo-home-perf-durable-d4
mkdir -p "$CARGO_HOME"
for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -s "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
export PATH=$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=false CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=$J
export CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/durable-d2
SRC=$HOME/.cache/amphora-perf/durable-d5-remote-$MODE LOGS=$HOME/.cache/amphora-perf/durable-d5-logs/$SHA-$MODE
mkdir -p "$LOGS"
BASE=$HOME/.cache/amphora-perf/durable-d2-remote
if [ ! -d "$SRC/.git" ]; then git clone -q "$BASE" "$SRC" 2>/dev/null || git clone -q "$URL" "$SRC" || exit 2; git -C "$SRC" remote set-url origin "$URL"; fi
git -C "$SRC" fetch -q origin '+refs/heads/*:refs/remotes/origin/*' || exit 2
git -C "$SRC" checkout -q --detach "$SHA" || exit 3
cd "$SRC" || exit 2
echo "== $(hostname -s) $MODE $(git log --oneline -1 | cut -c1-80) $(date -u +%H:%M:%SZ) load $(LOAD) $(PSI)"
t() { # log cargo-test-args...
  local log=$LOGS/$1; shift
  local s=$(date +%s)
  nice -n 10 cargo test --locked "$@" >"$log" 2>&1
  local rc=$?
  echo "-- $(basename "$log") exit=$rc compiled=$(grep -cE 'Compiling amphora' "$log") $(grep -E '^test result' "$log" | awk '{p+=$4; f+=$6; i+=$8} END {print "passed=" p " failed=" f " ignored=" i}') $(( $(date +%s) - s ))s $(date -u +%H:%M:%SZ) load $(LOAD) $(PSI)"
  grep -E '^test .* FAILED$' "$log" | head -20
  grep -E 'panicked at' -A10 "$log" | cut -c1-300 | head -60
  grep -E '^error' -A12 "$log" | head -80
}
case $MODE in
lint)
  s=$(date +%s); nice -n 10 cargo clippy --workspace --all-targets --locked -- -D warnings >"$LOGS/clippy.log" 2>&1
  echo "-- clippy workspace exit=$? checked=$(grep -cE '^ *(Checking|Compiling) amphora' "$LOGS/clippy.log") $(( $(date +%s) - s ))s $(date -u +%H:%M:%SZ) load $(LOAD)"; grep -E '^(error|warning)' -A8 "$LOGS/clippy.log" | head -80
  s=$(date +%s); nice -n 10 bash scripts/check-production.sh >"$LOGS/check-production.log" 2>&1
  echo "-- check-production exit=$? $(( $(date +%s) - s ))s $(date -u +%H:%M:%SZ)"; grep -E '^ *(Checking|Compiling) amphora' "$LOGS/check-production.log" | sed -E 's/^ *(Checking|Compiling) ([^ ]*).*/\1 \2/' | sort | uniq | tr '\n' ';'; echo; grep -E '^error' -A8 "$LOGS/check-production.log" | head -40; tail -6 "$LOGS/check-production.log"
  ;;
tests)
  t amphora-lib.log -p amphora --lib -- --test-threads=$J xgroup:: database::across:: transport:: replication:: database::replicated:: backup:: db::
  t serve.log -p amphora --test serve -- --test-threads=$J
  t transport.log -p amphora --test transport -- --test-threads=$J
  ;;
one)
  t one.log -p amphora --lib -- --test-threads=$J "$@"
  ;;
esac
echo "== done $(date -u +%H:%M:%SZ)"
