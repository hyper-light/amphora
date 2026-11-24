#!/usr/bin/env bash
# Lane V20's Linux driver. usage: linux.sh SHA LABEL STEP...   (the GitHub clone on adacachy2)
set -uo pipefail
SHA=$1; LABEL=$2; shift 2
export CARGO_HOME=$HOME/.cache/cargo-home-perf-commits CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/commits CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-8}
mkdir -p "$CARGO_HOME"; for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -s "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
B=$HOME/.cache/amphora-perf-bin/commits; O=$HOME/.cache/amphora-perf/commits-v20-out; mkdir -p "$B" "$O"
load() { echo "$(cut -d' ' -f1-3 /proc/loadavg) psi cpu $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
cd "$HOME/.cache/amphora-perf/commits-remote" || exit 2
git fetch -q origin 2>&1 | tail -2
git checkout -q --detach "$SHA" || exit 3
echo "== $(hostname) $(date -u +%T) tree $(git rev-parse --short=8 HEAD) $(df -h --output=avail ~ | tail -1) load=$(load)"
P=database::replicated::tests
for step in "$@"; do
  s=$(date +%s)
  case $step in
    build)
      out=$(nice -n 10 cargo test -p amphora --lib --locked --no-run 2>&1); rc=$?
      bin=$(echo "$out" | grep -o '/[^ )]*/debug/deps/amphora-[0-9a-f]*' | tail -1)
      echo "== built $LABEL rc=$rc in $(( $(date +%s) - s ))s load=$(load)"
      [ -x "$bin" ] || { echo "$out" | grep -E "^(error|warning)" -A 16 | head -140; exit 4; }
      cp "$bin" "$B/v20-tests-$LABEL" ;;
    clippy)
      nice -n 10 cargo clippy -p amphora --all-targets --locked -- -D warnings > "$O/clippy-$LABEL.log" 2>&1; rc=$?
      echo "== clippy $LABEL rc=$rc in $(( $(date +%s) - s ))s load=$(load)"; grep -E "Checking amphora|^(error|warning)" -A 12 "$O/clippy-$LABEL.log" | head -80 ;;
    fmt)
      cargo fmt --all --check > "$O/fmt-$LABEL.log" 2>&1; echo "== fmt rc=$?"; head -40 "$O/fmt-$LABEL.log" ;;
    production)
      nice -n 10 bash scripts/check-production.sh > "$O/production-$LABEL.log" 2>&1; rc=$?
      echo "== check-production $LABEL rc=$rc in $(( $(date +%s) - s ))s load=$(load)"; tail -12 "$O/production-$LABEL.log" ;;
    suite)
      timeout 1500 "$B/v20-tests-$LABEL" --test-threads="${THREADS:-8}" database::replicated:: replication:: database::log_bound:: > "$O/suite-$LABEL.log" 2>&1; rc=$?
      echo "== suite $LABEL rc=$rc in $(( $(date +%s) - s ))s load=$(load): $(grep 'test result' "$O/suite-$LABEL.log" | tail -1)"
      grep -E '^test .* FAILED$' "$O/suite-$LABEL.log"; grep -E "panicked at" -A 4 "$O/suite-$LABEL.log" | head -80 ;;
    t:*)
      for r in $(seq 1 "${ROUNDS:-1}"); do
        s=$(date +%s); t=${step#t:}
        timeout 200 "$B/v20-tests-$LABEL" --exact "$P::$t" --test-threads=1 --nocapture > "$O/$LABEL-$t-r$r.log" 2>&1; rc=$?
        echo "== r$r $LABEL $t rc=$rc $(( $(date +%s) - s ))s load=$(load)"
        [ $rc = 0 ] || grep -E "panicked at|left:|right:|assert" -A 2 "$O/$LABEL-$t-r$r.log" | head -24
      done ;;
  esac
done
echo "== done $(date -u +%T) load=$(load)"
