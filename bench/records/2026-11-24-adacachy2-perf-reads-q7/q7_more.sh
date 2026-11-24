#!/usr/bin/env bash
# Lane Q7: rounds FROM..TO of q7_ab.sh's timed runs on the binaries it built, appended to its runs
# (rounds 1-5 kept as ab-r1-5.jsonl too), then q7_leaf.py on its four profiles and q7_table.py over
# every round. Odd rounds run A,B, even ones B,A, as in q7_ab.sh; rounds stop at DEADLINE.
# usage: A=<sha> B=<sha> TOOLSHA=<sha> FROM=6 TO=10 DEADLINE=<epoch s> q7_more.sh
set -uo pipefail
A=${A:?} B=${B:?} TOOLSHA=${TOOLSHA:?} FROM=${FROM:?} TO=${TO:?} DEADLINE=${DEADLINE:?}
L=~/.cache/analytics-q7 R=~/.cache/amphora-perf/analytics-remote BIN=~/.cache/amphora-perf-bin/analytics TOOLS=~/.cache/analytics-q7/tools
RQ=bench/records/2026-11-24-adacachy2-perf-reads-q7
export XDG_CACHE_HOME=$L/xdg-ab
git -C "$R" fetch -q origin perf/analytics || exit 1
for f in q7_leaf.py q7_table.py q7_ab.sh; do git -C "$R" show "$TOOLSHA:$RQ/$f" > "$TOOLS/$f" || exit 1; done
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
echo "=== start $(date -u +%H:%M:%SZ) $(state)"
[ -f "$L/ab-r1-5.jsonl" ] || cp "$L/ab.jsonl" "$L/ab-r1-5.jsonl" || exit 1
python3 -I "$TOOLS/q7_leaf.py" '^atomic_|^_mm_pause' 12 7 "$L"/prof/ab-*.folded
source <(sed -n '/^timed() {/,/^}/p' "$TOOLS/q7_ab.sh")
for i in $(seq "$FROM" "$TO"); do
  [ $(( DEADLINE - $(date +%s) )) -gt 90 ] || { echo "=== round $i not started: deadline"; break; }
  if [ $((i % 2)) -eq 1 ]; then order="$A $B"; else order="$B $A"; fi
  echo "=== round $i $(date -u +%H:%M:%SZ)"
  for n in 1 32 128; do for arm in $order; do timed "${arm:0:8}-$i-$n" "$arm" "$n"; done; done
done
python3 -I "$TOOLS/q7_table.py" "$L/ab.jsonl" "$L/ab-answers.jsonl" "$A" "$B"
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
