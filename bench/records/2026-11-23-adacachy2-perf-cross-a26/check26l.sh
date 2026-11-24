#!/usr/bin/env bash
# Lane A26 on adacachy2, held to $JOBS cores, in the lane's worktree of cross-remote. usage: check26l.sh OUT STEP...
#   build:SHA            SHA's lib test binary, as a26-tests-SHA
#   clippy:SHA  production:SHA  fmt:SHA
#   t:SHA:LABEL:ROUNDS:FILTER[,FILTER...]   the filtered lib tests, ROUNDS times
#   suite:A:B:ROUNDS:FIRST    database::replicated:: replication:: on A and B, the order alternated, rounds FIRST..
set -uo pipefail
OUT=${1:?}; shift; T=${JOBS:-8}
R=$HOME/.cache/amphora-perf/cross-remote
W=${W:-$HOME/.cache/amphora-perf/cross-a22-wt}
git -C "$R" fetch -q origin main perf/cross-a26 || exit 3
git -C "$R" fetch -q origin '+refs/heads/diag/cross-a26*:refs/remotes/origin/diag/cross-a26*' || true
[ -d "$W" ] || git -C "$R" worktree add -q --detach "$W" origin/main || exit 3
cd "$W" || exit 2
export CARGO_HOME=$HOME/.cache/cargo-home-perf-cross PATH=$HOME/.cache/cargo-home-perf-cross/bin:$PATH CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/cross CARGO_BUILD_JOBS=$T CARGO_INCREMENTAL=0
B=$HOME/.cache/amphora-perf-bin/cross; mkdir -p "$B" "$OUT"
stamp() { echo "[$(date -u +%H:%M:%SZ)] $* load=$(cut -d' ' -f1-3 /proc/loadavg) cpu=$(awk 'NR==1{print $2}' /proc/pressure/cpu) io=$(awk 'NR==1{print $2}' /proc/pressure/io)"; }
at() { git checkout -q -f --detach "$1" || exit 3; git log --oneline -1; }
one() {
  local sha=$1 label=$2 s out rc; shift 2; s=$(date +%s)
  timeout 1500 nice -n 10 "$B/a26-tests-$sha" --test-threads="$T" "$@" > "$OUT/$label-$sha.log" 2>&1; rc=$?
  stamp "$label $sha rc=$rc $(( $(date +%s) - s ))s: $(grep 'test result' "$OUT/$label-$sha.log" | tail -1)"
  grep -E '^test .* FAILED$' "$OUT/$label-$sha.log" | head -14
  [ "$rc" = 0 ] || grep -E "panicked at" -A 3 "$OUT/$label-$sha.log" | cut -c1-600 | head -${LINES_MAX:-40}
}
for step in "$@"; do
  IFS=: read -r what a b c d <<<"$step"
  case $what in
    build)
      at "$a"; s=$(date +%s)
      out=$(nice -n 10 cargo test -p amphora --lib --locked --no-run 2>&1)
      bin=$(echo "$out" | sed -n 's/.*Executable unittests src\/lib.rs (\(.*\))/\1/p')
      echo "$out" | grep -E "Compiling amphora(-engine)? |^error" | head -4
      stamp "built $a in $(( $(date +%s) - s ))s"
      [ -x "$bin" ] || { echo "$out" | grep -E "^(error|warning)" -A 14 | head -90; exit 4; }
      cp "$bin" "$B/a26-tests-$a" ;;
    clippy)
      at "$a"; s=$(date +%s)
      nice -n 10 cargo clippy -p amphora --all-targets --locked -- -D warnings > "$OUT/clippy-$a.log" 2>&1; rc=$?
      stamp "clippy $a rc=$rc $(( $(date +%s) - s ))s"; grep -E "Checking amphora|^(error|warning)" -A 12 "$OUT/clippy-$a.log" | grep -v 'vendor/hyper-raft' | head -70 ;;
    production)
      at "$a"; s=$(date +%s)
      nice -n 10 bash scripts/check-production.sh > "$OUT/production-$a.log" 2>&1; rc=$?
      stamp "check-production $a rc=$rc $(( $(date +%s) - s ))s"; tail -6 "$OUT/production-$a.log" ;;
    t)
      for r in $(seq 1 "$c"); do one "$a" "$b-r$r" $(echo "$d" | tr , ' '); done ;;
    suite)
      for r in $(seq "$d" $(( d + c - 1 ))); do
        if [ $((r % 2)) -eq 1 ]; then order="$a $b"; else order="$b $a"; fi
        for S in $order; do LINES_MAX=12 one "$S" "suite-r$r" database::replicated:: replication::; done
      done ;;
  esac
done
stamp "end"
