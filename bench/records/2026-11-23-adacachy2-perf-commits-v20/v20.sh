#!/usr/bin/env bash
# Lane V20: V19's v19.sh on prebuilt test binaries (linux.sh's `build` step keeps each arm's as
# ~/.cache/amphora-perf-bin/commits/v20-tests-<label>): the two restart tests alone, the arm order
# alternated each round, then the replicated suites, SUITES rounds a side, the arms alternated. The
# main arm runs T1 and T2 by their old names and the branch by its new ones.
# usage: v20.sh OUT THREADS ROUNDS SUITES MAIN_LABEL BRANCH_LABEL
set -uo pipefail
OUT=${1:?}; T=${2:?}; ROUNDS=${3:?}; SUITES=${4:?}; MAIN=${5:?}; BRANCH=${6:?}
load() { if [ -r /proc/loadavg ]; then echo "$(cut -d' ' -f1-3 /proc/loadavg) psi cpu $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io $(head -1 /proc/pressure/io | cut -d' ' -f2)"; else sysctl -n vm.loadavg; fi; }
ms() { python3 -c 'import time;print(int(time.time()*1000))'; }
B=$HOME/.cache/amphora-perf-bin/commits; mkdir -p "$OUT"
P=database::replicated::tests
main_t1=$P::a_local_read_holds_off_the_logs_restart_and_one_after_waits_for_it
main_t2=$P::a_restart_due_waits_for_a_reads_deadline_and_no_longer
branch_t1=$P::a_local_read_holds_off_no_restart_of_the_log_and_one_after_waits_for_nothing
branch_t2=$P::a_restart_due_waits_for_no_read_and_the_next_for_its_deadline_at_most
branch_t3=$P::a_slow_local_read_has_no_write_refused_past_the_logs_room
one() { # label side test round
  local name="${2}_$3" s rc; s=$(ms)
  perl -e 'alarm 100; exec @ARGV' "$B/v20-tests-$1" --exact "${!name}" --test-threads=1 > "$OUT/alone-$1-$3-r$4.log" 2>&1; rc=$?
  echo "alone r$4 $1 $3 rc=$rc $(( $(ms) - s ))ms load=$(load)"
}
for ((r = 1; r <= ROUNDS; r++)); do
  sides=("main" "branch"); if ((r % 2 == 0)); then sides=("branch" "main"); fi
  for side in "${sides[@]}"; do
    if [ "$side" = main ]; then one "$MAIN" main t1 "$r"; one "$MAIN" main t2 "$r"
    else one "$BRANCH" branch t1 "$r"; one "$BRANCH" branch t2 "$r"; one "$BRANCH" branch t3 "$r"; fi
  done
done
for l in "$MAIN" "$BRANCH"; do
  for t in t1 t2 t3; do
    n=$(ls "$OUT"/alone-"$l"-"$t"-r*.log 2>/dev/null | wc -l | tr -d ' '); [ "$n" = 0 ] && continue
    f=$(grep -L 'test result: ok. 1 passed' "$OUT"/alone-"$l"-"$t"-r*.log | wc -l | tr -d ' ')
    echo "== ALONE $l $t failed $f of $n"
  done
done
for ((r = 1; r <= SUITES; r++)); do
  order=("$MAIN" "$BRANCH"); if ((r % 2 == 0)); then order=("$BRANCH" "$MAIN"); fi
  for l in "${order[@]}"; do
    s=$(ms); "$B/v20-tests-$l" --test-threads="$T" database::replicated:: replication:: > "$OUT/suite-$l-r$r.log" 2>&1
    echo "== suite r$r $l $(( ($(ms) - s) / 1000 ))s load=$(load): $(grep 'test result' "$OUT/suite-$l-r$r.log" | tail -1)"
    grep -E '^test .* FAILED$' "$OUT/suite-$l-r$r.log" | sed 's/^test //; s/ \.\.\. FAILED//' | sort | tr '\n' ' '; echo
  done
done
echo "== done $(date -u +%T) load=$(load)"
