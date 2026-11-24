#!/bin/bash
# Lane D5: macOS checks. usage: mac.sh <reach-dir> <sha> <jobs> <name> <mode> [args...]  modes: lib <filters..> | test <name> | clippy | red
set -u
REACH=${1:?} SHA=${2:?} J=${3:?} NAME=${4:?} MODE=${5:?}; shift 5
[ $(df -g ~ | tail -1 | awk '{print $4}') -ge 40 ] || { echo "disk low"; exit 2; }
LOAD() { sysctl -n vm.loadavg; }
export CARGO_HOME=$HOME/.cache/cargo-home-perf-durable-d4
mkdir -p "$CARGO_HOME"
for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -s "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
export PATH=/opt/homebrew/bin:$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=$J
export CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/durable-d5-$NAME
[ -d "$CARGO_TARGET_DIR" ] || { for d in durable-d4-re durable-d4-all; do if [ -d "$HOME/.cache/amphora-perf-target/$d" ]; then cp -cR "$HOME/.cache/amphora-perf-target/$d" "$CARGO_TARGET_DIR"; break; fi; done; }
SRC=$HOME/.cache/durable-d5-check-$NAME LOGS=$HOME/.cache/amphora-perf/durable-d5-logs/$SHA-$NAME
mkdir -p "$LOGS"
[ -d "$SRC/.git" ] || git clone -q --no-checkout "$REACH" "$SRC" || exit 2
cd "$SRC" || exit 2
git fetch -q "$REACH" '+refs/remotes/origin/*:refs/remotes/origin/*' '+refs/heads/*:refs/remotes/reach/*' || exit 2
case $NAME in green) SHA=$(git rev-parse refs/remotes/reach/perf/durable-d5) ;; red) SHA=$(git rev-parse refs/remotes/origin/perf/durable-d5-red) ;; esac
git checkout -q -f --detach "$SHA" || exit 3
echo "== $(hostname -s) $NAME $MODE $(git log --oneline -1 | cut -c1-80) $(date -u +%H:%M:%SZ) load $(LOAD)"
t() { # log cargo-test-args...
  local log=$LOGS/$1; shift
  local s=$(date +%s)
  nice -n 10 cargo test --locked "$@" >"$log" 2>&1
  local rc=$?
  echo "-- $(basename "$log") exit=$rc compiled=$(grep -cE 'Compiling amphora' "$log") $(grep -E '^test result' "$log" | awk '{p+=$4; f+=$6; i+=$8} END {print "passed=" p " failed=" f " ignored=" i}') $(( $(date +%s) - s ))s $(date -u +%H:%M:%SZ) load $(LOAD)"
  grep -E '^test .* FAILED$' "$log" | head -30
  grep -E 'panicked at' -A8 "$log" | cut -c1-400 | head -80
  grep -E '^error' -A12 "$log" | head -90
}
case $MODE in
lib) t lib.log -p amphora --lib -- --test-threads=$J "$@" ;;
test) t "test-$1.log" -p amphora --test "$1" -- --test-threads=$J ;;
clippy) s=$(date +%s); nice -n 10 cargo clippy -p amphora --all-targets --locked -- -D warnings >"$LOGS/clippy.log" 2>&1
  echo "-- clippy exit=$? checked=$(grep -cE '^ *(Checking|Compiling) amphora' "$LOGS/clippy.log") $(( $(date +%s) - s ))s $(date -u +%H:%M:%SZ)"; grep -E '^(error|warning)' -A8 "$LOGS/clippy.log" | head -80 ;;
esac
echo "== done $(date -u +%H:%M:%SZ)"
