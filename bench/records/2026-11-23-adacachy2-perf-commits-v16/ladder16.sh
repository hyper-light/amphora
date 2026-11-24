#!/usr/bin/env bash
# Lane V16: V13's ladder (ladder13.sh, cell13.py) on Linux or macOS: Amphora alone at 32 and 128
# spread writers, both classes, ROUNDS rounds, the cell order reversed each even round; BIN is a
# release amphora-bench. Run from the repository root (CELLS names cell13.py's folder otherwise).
# usage: ladder16.sh BIN OUT [ROUNDS]
set -uo pipefail
BIN=${1:?}; OUT=${2:?}; ROUNDS=${3:-3}; S=${CELLS:-bench/records/2026-11-23-adacachy2-perf-commits-v13}
ulimit -n "$(ulimit -Hn)" 2>/dev/null || true
load() { if [ -r /proc/loadavg ]; then cut -d' ' -f1-3 /proc/loadavg; else sysctl -n vm.loadavg; fi; }
cells=("persisted 32" "replicated 32" "persisted 128" "replicated 128")
for r in $(seq 1 "$ROUNDS"); do
  order=("${cells[@]}"); [ $((r % 2)) -eq 0 ] && order=("${cells[3]}" "${cells[2]}" "${cells[1]}" "${cells[0]}")
  for cell in "${order[@]}"; do
    read -r cls w <<<"$cell"
    echo "r$r $(date -u +%T) load=$(load) $(nice -n 10 python3 -I "$S/cell13.py" "$BIN" 29490 "$OUT/$cls-$w-r$r" "$w" spread "$cls")"
  done
done
