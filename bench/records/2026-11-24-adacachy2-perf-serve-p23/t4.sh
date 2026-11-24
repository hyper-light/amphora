#!/bin/bash
# Lane P23 (serve) t4, adacachy2: V13's cell on the traced arm diag-H2 (main 3a58a13f) with every second application write
# run inline on its owner (P23_ALT=1) and the others on the session worker, so both run in one cell at one load; then
# trace2.py's per-stage times by mode and the leader's slot-to-slot cycle by the holder's mode. R rounds, NICE the cells' nice.
set -u
R=${R:-3} NICE=${NICE:-10} PORT=${PORT:-31780} TAG=${TAG:-t4}
A=$HOME/.cache/amphora-perf/serve-p7-remote; BIN=$HOME/.cache/amphora-perf-bin/serve-p23
L=$HOME/.cache/amphora-perf/serve-p23-$TAG; mkdir -p "$L/cells"
G=bench/records/2026-11-24-adacachy2-perf-serve-p23
ld() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu-some $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/cpu) io-some $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/io)"; }
echo "start $(date -u +%T) nice $NICE $(ld)"
[ -x "$BIN/diag-H2" ] || { echo "NO BIN"; exit 9; }
for f in cell23t.py trace2.py; do git -C "$A" show ${S:-origin/perf/serve-p23}:$G/$f > "$L/$f" || exit 9; done
for r in $(seq 1 $R); do
  for cls in persisted replicated; do
    echo "alt r$r $cls $(date -u +%T) $(ld) $(P23_ALT=1 nice -n $NICE python3 -I "$L/cell23t.py" "$BIN/diag-H2" $PORT "$L/cells/H2-$cls-32-r$r" 32 spread "$cls" 2>&1 | tail -3 | tr '\n' ' ')"
  done
done
echo "end cells $(date -u +%T) $(ld)"
python3 -I "$L/trace2.py" "$L"/cells/H2-*-32-r* 2>&1
echo "end $(date -u +%T) $(ld)"
