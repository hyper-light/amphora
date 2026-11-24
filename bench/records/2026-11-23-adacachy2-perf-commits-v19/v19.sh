#!/usr/bin/env bash
# Lane V19: the two restart-wait tests alone, in pairs, on each arm, the arm order alternated each
# round; then, if SUITES > 0, the replicated suites, SUITES rounds a side, the arms alternated.
# Builds each arm in the lane's one target dir in turn, keeps its test binary as
# ~/.cache/amphora-perf-bin/commits/v19-tests-<label>. Runs from a clone of the repository.
# usage: v19.sh OUT THREADS ROUNDS SUITES LABEL=SHA [LABEL=SHA ...]
set -uo pipefail
OUT=${1:?}; T=${2:?}; ROUNDS=${3:?}; SUITES=${4:?}; shift 4
load() { if [ -r /proc/loadavg ]; then echo "$(cut -d' ' -f1-3 /proc/loadavg) psi cpu $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io $(head -1 /proc/pressure/io | cut -d' ' -f2)"; else sysctl -n vm.loadavg; fi; }
ms() { python3 -c 'import time;print(int(time.time()*1000))'; }
B=$HOME/.cache/amphora-perf-bin/commits; mkdir -p "$B" "$OUT"
labels=()
for arm in "$@"; do
  label=${arm%%=*}; sha=${arm#*=}; labels+=("$label")
  git checkout -q --detach "$sha" || exit 3
  s=$(date +%s); out=$(nice -n 10 cargo test -p amphora --lib --locked --no-run 2>&1)
  bin=$(echo "$out" | grep -o '/[^ )]*/debug/deps/amphora-[0-9a-f]*' | tail -1)
  echo "== built $label $sha in $(( $(date +%s) - s ))s load=$(load)"
  [ -x "$bin" ] || { echo "$out" | grep -E "^(error|warning)" -A 14 | head -90; exit 4; }
  cp "$bin" "$B/v19-tests-$label"
done
P=database::replicated::tests
T1=$P::a_local_read_holds_off_the_logs_restart_and_one_after_waits_for_it
T2=$P::a_restart_due_waits_for_a_reads_deadline_and_no_longer
one() { # label test short round
  local s rc; s=$(ms)
  perl -e 'alarm 100; exec @ARGV' "$B/v19-tests-$1" --exact "$2" --test-threads=1 > "$OUT/alone-$1-$3-r$4.log" 2>&1; rc=$?
  echo "alone r$4 $1 $3 rc=$rc $(( $(ms) - s ))ms load=$(load)"
}
for ((r = 1; r <= ROUNDS; r++)); do
  order=("${labels[@]}"); if ((r % 2 == 0)); then order=(); for l in "${labels[@]}"; do order=("$l" ${order[@]+"${order[@]}"}); done; fi
  for l in "${order[@]}"; do one "$l" "$T1" t1 "$r"; one "$l" "$T2" t2 "$r"; done
done
for l in "${labels[@]}"; do
  for t in t1 t2; do
    f=$(grep -L 'test result: ok' "$OUT"/alone-"$l"-"$t"-r*.log 2>/dev/null | wc -l | tr -d ' ')
    n=$(ls "$OUT"/alone-"$l"-"$t"-r*.log 2>/dev/null | wc -l | tr -d ' ')
    echo "== ALONE $l $t failed $f of $n"
  done
done
for ((r = 1; r <= SUITES; r++)); do
  order=("${labels[@]}"); if ((r % 2 == 0)); then order=(); for l in "${labels[@]}"; do order=("$l" ${order[@]+"${order[@]}"}); done; fi
  for l in "${order[@]}"; do
    s=$(ms); "$B/v19-tests-$l" --test-threads="$T" database::replicated:: replication:: > "$OUT/suite-$l-r$r.log" 2>&1
    echo "== suite r$r $l $(( ($(ms) - s) / 1000 ))s load=$(load): $(grep 'test result' "$OUT/suite-$l-r$r.log" | tail -1)"
    grep -E '^test .* FAILED$' "$OUT/suite-$l-r$r.log" | sed 's/^test //; s/ \.\.\. FAILED//' | sort | tr '\n' ' '; echo
  done
done
echo "== done $(date -u +%T) load=$(load)"
