#!/usr/bin/env bash
# Lane V12 (commits): the replicated-commit ladder on one host. Amphora's two classes against rqlite
# and dqlite at 1, 32 and 128 writers, spread over the three members and leader-only, 3 rounds with
# the cell order reversed each round. Each cell's start and end go to OUT/cells.log with the load and
# the CPU and I/O pressure; each record to OUT/r<round>/<system>-commit.jsonl. S6's flags (at cfa0107c).
# usage: ladder.sh OUT   (from the repository's root; competitors' binaries from bench/servers/repl/setup.sh)
set -uo pipefail
OUT=${1:?usage: ladder.sh OUT}
ROOT=$(git rev-parse --show-toplevel)
mkdir -p "$OUT"
FLAGS=(--batch 1 --text-bytes 600 --deadline-s 10 --relay-records 128 --spanning 2 --warmup-s 1 --seconds 5)
SYSTEMS=(amphora-procs-persisted amphora-procs-replicated rqlite dqlite)
ulimit -n "$(ulimit -Hn)" 2>/dev/null || true
cargo build --release --manifest-path "$ROOT/bench/Cargo.toml" || exit 1
TARGET=$(cargo metadata --format-version 1 --no-deps --manifest-path "$ROOT/bench/Cargo.toml" | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
BIN="$TARGET/release/amphora-bench"
[[ -x "$BIN" ]] || { echo "no $BIN" >&2; exit 1; }
P() { echo "$(date -u +%FT%T.%3NZ) $1 load=$(cut -d' ' -f1-3 /proc/loadavg | tr ' ' ,) cpu_some=$(awk 'NR==1{print $2}' /proc/pressure/cpu) io_some=$(awk 'NR==1{print $2}' /proc/pressure/io) io_full=$(awk 'NR==2{print $2}' /proc/pressure/io)" >>"$OUT/cells.log"; }
cells=()
for w in 1 32 128; do for r in spread leader; do for s in "${SYSTEMS[@]}"; do cells+=("$s $w $r"); done; done; done
for round in 1 2 3; do
  order=("${cells[@]}")
  if ((round % 2 == 0)); then order=(); for ((i = ${#cells[@]} - 1; i >= 0; i--)); do order+=("${cells[$i]}"); done; fi
  mkdir -p "$OUT/r$round"
  for cell in "${order[@]}"; do
    read -r s w r <<<"$cell"
    P "start r$round $s $w $r"
    case "$s" in
      amphora-procs-*)
        MEASURES=commit MEMBER_DIR=.cache/amphora-perf/commits-v12-members "$ROOT/bench/repl_members.sh" "$OUT/r$round" "${s#amphora-procs-}" \
          127.0.0.1:29290 127.0.0.1:29291 127.0.0.1:29292 "${FLAGS[@]}" --writers "$w" --route "$r" >>"$OUT/driver.log" 2>&1
        rc=$? ;;
      *)
        "$BIN" --repl commit --system "$s" --scripts "$ROOT/bench/servers/repl" --record "$OUT/r$round/$s-commit.jsonl" \
          "${FLAGS[@]}" --writers "$w" --route "$r" >>"$OUT/driver.log" 2>&1
        rc=$? ;;
    esac
    P "end r$round $s $w $r rc=$rc"
  done
done
python3 -I "$ROOT/bench/records/2026-11-23-adacachy2-perf-commits-v12/ladder_table.py" "$OUT"
