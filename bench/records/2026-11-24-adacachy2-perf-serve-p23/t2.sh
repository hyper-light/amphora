#!/bin/bash
# Lane P23 (serve) t2, adacachy2: V13's cell on the traced arms (diag-H, diag-M of t1.sh), arms alternated, each member's
# stage timestamps kept; then trace.py's per-stage times of each follower's write by arm and class. NICE is the cells' nice
# (10 as lane P22's), R the rounds.
set -u
R=${R:-3} NICE=${NICE:-10} PORT=${PORT:-31760} TAG=${TAG:-t2}
A=$HOME/.cache/amphora-perf/serve-p7-remote; BIN=$HOME/.cache/amphora-perf-bin/serve-p23
L=$HOME/.cache/amphora-perf/serve-p23-$TAG; mkdir -p "$L/cells"
G=bench/records/2026-11-24-adacachy2-perf-serve-p23
ld() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu-some $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/cpu) io-some $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/io)"; }
echo "start $(date -u +%T) nice $NICE $(ld)"
for a in H M; do [ -x "$BIN/diag-$a" ] || { echo "NO BIN $a"; exit 9; }; done
for f in cell23t.py trace.py; do git -C "$A" show ${S:-origin/perf/serve-p23}:$G/$f > "$L/$f" || exit 9; done
for r in $(seq 1 $R); do
  if [ $((r % 2)) -eq 1 ]; then arms="H M"; else arms="M H"; fi
  for a in $arms; do for cls in persisted replicated; do
    echo "trace r$r $a $cls $(date -u +%T) $(ld) $(nice -n $NICE python3 -I "$L/cell23t.py" "$BIN/diag-$a" $PORT "$L/cells/$a-$cls-32-r$r" 32 spread "$cls" 2>&1 | tail -3 | tr '\n' ' ')"
  done; done
done
echo "end cells $(date -u +%T) $(ld)"
python3 -I "$L/trace.py" "$L"/cells/*-32-r* 2>&1
echo "end $(date -u +%T) $(ld)"
