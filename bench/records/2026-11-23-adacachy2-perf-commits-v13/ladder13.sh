#!/usr/bin/env bash
# Lane V13: Amphora alone at 32 and 128 spread writers, both classes, ROUNDS rounds, the cell order
# reversed each even round; BIN is a release amphora-bench. usage: ladder13.sh BIN OUT [ROUNDS]
set -uo pipefail
BIN=${1:?}; OUT=${2:?}; ROUNDS=${3:-3}; S=$(dirname "$0")
ulimit -n "$(ulimit -Hn)" 2>/dev/null || true
cells=("persisted 32" "replicated 32" "persisted 128" "replicated 128")
for r in $(seq 1 "$ROUNDS"); do
  order=("${cells[@]}"); [ $((r % 2)) -eq 0 ] && order=("${cells[3]}" "${cells[2]}" "${cells[1]}" "${cells[0]}")
  for cell in "${order[@]}"; do
    read -r cls w <<<"$cell"
    echo "r$r $(date -u +%T) load=$(cut -d' ' -f1-3 /proc/loadavg) cpu=$(awk 'NR==1{print $2}' /proc/pressure/cpu) io=$(awk 'NR==1{print $2}' /proc/pressure/io) $(nice -n 10 python3 -I "$S/cell13.py" "$BIN" 29490 "$OUT/$cls-$w-r$r" "$w" spread "$cls")"
  done
done
