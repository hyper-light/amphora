# Lane V20's macOS driver. usage: mac.sh SHA LABEL STEP...   (run from the synced clone's folder)
set -uo pipefail
[ "$(df -g ~ | tail -1 | awk '{print $4}')" -ge 40 ] || { echo "disk low"; df -g ~; exit 2; }
R=$(pwd); SHA=$1; LABEL=$2; shift 2
export PATH=/opt/homebrew/bin:$HOME/.cargo/bin:$PATH
export CARGO_HOME=$HOME/.cache/cargo-home-perf-commits
mkdir -p "$CARGO_HOME"; for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -s "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
export CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/commits CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-8}
S=$HOME/.cache/amphora-perf/commits-v20-src; B=$HOME/.cache/amphora-perf-bin/commits; O=$HOME/.cache/amphora-perf/commits-v20-out
mkdir -p "$B" "$O" "$HOME/.cache/amphora-perf"
[ -d "$S/.git" ] || git clone -q --no-checkout "$R" "$S" || exit 3
cd "$S" || exit 3
git fetch -q "$R" "$SHA" 2>/dev/null || git fetch -q "$R" "+refs/heads/*:refs/remotes/synced/*" || exit 3
git checkout -q --detach "$SHA" || exit 3
echo "== $(hostname) $(date -u +%T) tree $(git rev-parse --short=8 HEAD) load=$(sysctl -n vm.loadavg)"
P=database::replicated::tests
for step in "$@"; do
  s=$(date +%s)
  case $step in
    build)
      out=$(nice -n 10 cargo test -p amphora --lib --locked --no-run 2>&1); rc=$?
      bin=$(echo "$out" | grep -o '/[^ )]*/debug/deps/amphora-[0-9a-f]*' | tail -1)
      echo "== built $LABEL rc=$rc in $(( $(date +%s) - s ))s load=$(sysctl -n vm.loadavg)"
      [ -x "$bin" ] || { echo "$out" | grep -E "^(error|warning)" -A 16 | head -140; exit 4; }
      cp "$bin" "$B/v20-tests-$LABEL" ;;
    clippy)
      nice -n 10 cargo clippy -p amphora --all-targets --locked -- -D warnings > "$O/clippy-$LABEL.log" 2>&1; rc=$?
      echo "== clippy $LABEL rc=$rc in $(( $(date +%s) - s ))s load=$(sysctl -n vm.loadavg)"; grep -E "Checking amphora|^(error|warning)" -A 12 "$O/clippy-$LABEL.log" | head -80 ;;
    fmt)
      cargo fmt --all --check > "$O/fmt-$LABEL.log" 2>&1; echo "== fmt rc=$?"; head -40 "$O/fmt-$LABEL.log" ;;
    suite)
      perl -e 'alarm 1500; exec @ARGV' "$B/v20-tests-$LABEL" --test-threads="${THREADS:-8}" database::replicated:: replication:: database::log_bound:: > "$O/suite-$LABEL.log" 2>&1; rc=$?
      echo "== suite $LABEL rc=$rc in $(( $(date +%s) - s ))s load=$(sysctl -n vm.loadavg): $(grep 'test result' "$O/suite-$LABEL.log" | tail -1)"
      grep -E '^test .* FAILED$' "$O/suite-$LABEL.log"; grep -E "panicked at" -A 4 "$O/suite-$LABEL.log" | head -80 ;;
    t:*)
      for r in $(seq 1 "${ROUNDS:-1}"); do
        s=$(date +%s); t=${step#t:}
        perl -e 'alarm 200; exec @ARGV' "$B/v20-tests-$LABEL" --exact "$P::$t" --test-threads=1 --nocapture > "$O/$LABEL-$t-r$r.log" 2>&1; rc=$?
        echo "== r$r $LABEL $t rc=$rc $(( $(date +%s) - s ))s load=$(sysctl -n vm.loadavg)"
        [ $rc = 0 ] || grep -E "panicked at|left:|right:|assert" -A 2 "$O/$LABEL-$t-r$r.log" | head -24
      done ;;
  esac
done
echo "== done $(date -u +%T) load=$(sysctl -n vm.loadavg)"
