#!/usr/bin/env bash
# Lane W6 (oltp): W5's A/B (ab-w5.sh) with three arms, T the old timer (98bd0fa9), M main before D1
# (f363d688) and D1 (716ede4d), 5 rounds alternated, binaries as W5 built them. Around it, what else
# ran on the machine (top processes by CPU, the disk's transfers); after it, one row a run
# (ab_w6_runs.py), then ab_w5_quiet.py's table over the runs within W5's flush bound (5.40 ms),
# the bound named before this run. ab-w5.sh itself prints the table over every run.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
O=$HOME/.cache/oltp-ab/w6/out; mkdir -p "$O"
others() { echo "$(date -u +%H:%M:%SZ) load $(sysctl -n vm.loadavg)"; ps -A -o %cpu=,etime=,comm= -r | head -6; iostat -d -w 1 -c 3 | tail -2; }
others
ARMS="T M D1" T=98bd0fa96b4cab06cb855fdd2a7b0e0113ead12f M=f363d688 D1=716ede4d OUT=$O bash "$HERE/ab-w5.sh" 2>&1 | tee "$O.log"
others
python3 -I "$HERE/ab_w6_runs.py" "$O.log" "$O"
python3 -I "$HERE/ab_w5_quiet.py" "T M D1" "$O"
