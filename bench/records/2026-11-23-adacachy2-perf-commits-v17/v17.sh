#!/usr/bin/env bash
# Lane V17: land lane V16's fix and check it, in the lane's one target dir, in turn:
# 1. MERGE (V16's branch with main merged): the focused tests, clippy, check-production.
# 2. MAIN (main without the fix): its lib tests built; the test V16's fix failed once in three suite rounds run
#    alone on MAIN and MERGE in pairs, the order alternated, for WINDOW seconds, so both run it as many times.
# 3. MERGE's release bench, then V16's ladder (32 and 128 spread writers, both classes, LADDER rounds).
# Runs from the repository it sits in. usage: v17.sh MERGE MAIN WINDOW LADDER OUT THREADS
set -uo pipefail
MERGE=${1:?}; MAIN=${2:?}; WINDOW=${3:?}; LADDER=${4:?}; OUT=${5:?}; T=${6:-8}
load() { echo "$(cut -d' ' -f1-3 /proc/loadavg) psi cpu $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
B=$HOME/.cache/amphora-perf-bin/commits; mkdir -p "$B" "$OUT"
build() {
  git checkout -q --detach "$1" || exit 3
  s=$(date +%s); out=$(nice -n 10 cargo test -p amphora --lib --locked --no-run 2>&1)
  bin=$(echo "$out" | grep -o '/[^ )]*/debug/deps/amphora-[0-9a-f]*' | tail -1)
  echo "== built $1 in $(( $(date +%s) - s ))s load=$(load)"
  [ -x "$bin" ] || { echo "$out" | grep -E "^(error|warning)" -A 14 | head -90; exit 4; }
  cp "$bin" "$B/v17-tests-$1"
}
build "$MERGE"
echo "== focused $MERGE $(date -u +%T) load=$(load)"
"$B/v17-tests-$MERGE" --test-threads="$T" v15_a_write_its_deadline database::slot:: v14_a_slow a_slot_holder_whose_write_fails 2>&1 \
  | grep -E 'FAILED$|test result|panicked at|left:|right:' | head -40
focused=${PIPESTATUS[0]}
echo "== clippy $MERGE $(date -u +%T)"
nice -n 10 cargo clippy -p amphora --all-targets --locked -- -D warnings 2>&1 | grep -E "^(error|warning)" -A 10 | head -60
clippy=${PIPESTATUS[0]}
echo "== check-production $MERGE $(date -u +%T)"
nice -n 10 bash scripts/check-production.sh 2>&1 | tail -4
prod=${PIPESTATUS[0]}
echo "== LANDING $MERGE focused=$focused clippy=$clippy check-production=$prod $(date -u +%T) load=$(load)"

build "$MAIN"
N=database::replicated::tests::a_local_read_holds_off_the_logs_restart_and_one_after_waits_for_it
declare -A pass=(["$MAIN"]=0 ["$MERGE"]=0) fail=(["$MAIN"]=0 ["$MERGE"]=0)
r=0; end=$(( $(date +%s) + WINDOW )); echo "== alone from $(date -u +%T) load=$(load)"
while [ "$(date +%s)" -lt "$end" ]; do
  r=$((r + 1))
  if [ $((r % 2)) -eq 1 ]; then arms="$MERGE $MAIN"; else arms="$MAIN $MERGE"; fi
  for sha in $arms; do
    s=$(date +%s%N); out=$("$B/v17-tests-$sha" --exact "$N" --test-threads=1 2>&1); rc=$?
    ms=$(( ($(date +%s%N) - s) / 1000000 ))
    [ "$r" -le 2 ] && echo "== alone r$r $sha rc=$rc ${ms}ms"
    if [ "$rc" -eq 0 ] && echo "$out" | grep -q '1 passed'; then pass[$sha]=$((pass[$sha] + 1)); else
      fail[$sha]=$((fail[$sha] + 1)); echo "$out" > "$OUT/alone-r$r-$sha.log"
      echo "== alone r$r $sha FAILED rc=$rc ${ms}ms $(date -u +%T) load=$(load)"; echo "$out" | grep -E 'panicked at|left:|right:|test result' | head -6
    fi
  done
done
echo "== ALONE pairs=$r $MAIN pass=${pass[$MAIN]} fail=${fail[$MAIN]} $MERGE pass=${pass[$MERGE]} fail=${fail[$MERGE]} $(date -u +%T) load=$(load)"

git checkout -q --detach "$MERGE" || exit 3
s=$(date +%s); out=$(cd bench && nice -n 10 cargo build --release 2>&1); rc=$?
echo "== release bench $MERGE rc=$rc in $(( $(date +%s) - s ))s load=$(load)"
[ "$rc" -eq 0 ] || { echo "$out" | grep -E '^(error|warning)' -A 10 | head -40; exit 5; }
cp "$CARGO_TARGET_DIR/release/amphora-bench" "$B/v17-bench-$MERGE"
bash bench/records/2026-11-23-adacachy2-perf-commits-v16/ladder16.sh "$B/v17-bench-$MERGE" "$OUT/ladder" "$LADDER"
echo "== LADDER done $(date -u +%T) load=$(load)"
