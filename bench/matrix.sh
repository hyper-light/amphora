#!/usr/bin/env bash
# Runs every target at every scale, each run's record appended to bench/records/<date>-<host>.jsonl
# (CLAUDE.md §7: hardware, commit, date and load beside every figure). A target that isn't up (its
# server isn't running) records its failure and the matrix goes on.
#
# usage: bench/matrix.sh [targets] [writers...]
#   e.g. bench/matrix.sh amphora,postgres,mysql 32 1000 10000 100000 1000000
# Environment: AMPHORA_GROUPS for Amphora's groups (12 when unset), AMPHORA_BENCH_DATA for where
# the in-process engines keep their files, MATRIX_SECONDS to scale every run's length, MATRIX_OUT
# for where the records go (a runner that syncs this tree wipes what it finds untracked there).
set -uo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
targets="${1:-amphora,turso,sqlite,postgres,orioledb,cedardb,mysql,clickhouse,clickhouse-durable}"
shift || true
writers=("$@")
[ ${#writers[@]} -eq 0 ] && writers=(32 1000 10000 100000 1000000)
out="${MATRIX_OUT:-records/$(date -u +%Y-%m-%d)-$(hostname -s).jsonl}"
mkdir -p "$(dirname -- "$out")"
export AMPHORA_GROUPS="${AMPHORA_GROUPS:-12}"
scale="${MATRIX_SECONDS:-1}"
err="${XDG_CACHE_HOME:-$HOME/.cache}/amphora-matrix-$$.err"
for w in "${writers[@]}"; do
  # Long enough for every writer to finish a few writes: at a million writers one round of writes
  # takes seconds, so the run and its warmup grow with the writers.
  case $w in
    32) seconds=20 warmup=0 ;;
    1000) seconds=20 warmup=5 ;;
    10000) seconds=30 warmup=10 ;;
    100000) seconds=40 warmup=15 ;;
    *) seconds=70 warmup=30 ;;
  esac
  seconds=$((seconds * scale)) warmup=$((warmup * scale))
  connections=$(( w < 256 ? w : 256 ))
  for t in ${targets//,/ }; do
    echo "== $t writers=$w seconds=$seconds warmup=$warmup connections=$connections load=$(cut -d' ' -f1 /proc/loadavg)" >&2
    # A run gets its window and a quarter of an hour more (a slow engine drains a million writers'
    # writes slowly), then it is stopped and recorded as failed: a server that dies mid-run (MySQL's
    # exited at a million writers, 2026-10-20) leaves its client waiting on it otherwise.
    if line=$(timeout --kill-after=30 $((seconds + warmup + 900)) ./run.sh "$t" --sessions 4000 --preload 25 --writers "$w" --readers 4 --seconds "$seconds" --warmup "$warmup" \
      --submitters 16 --connections "$connections" --searches 3 2>"$err"); then
      echo "$line" >> "$out"
    else
      python3 - "$t" "$w" "$(tail -c 2000 "$err")" >> "$out" <<'PY'
import json, sys
print(json.dumps({"failed": True, "target": sys.argv[1], "writers": int(sys.argv[2]), "error": sys.argv[3]}))
PY
    fi
    rm -f "${err:?}"
  done
done
echo "records: $out" >&2
