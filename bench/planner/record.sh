#!/usr/bin/env bash
# The planner bench's baseline (bench/planner/README.md): TPC-H at scale factor 1 twice, at 10
# once, and JOB twice, every engine, best of 3, each pass to a record of its own in <out dir>
# (`--resume`, so a pass stopped part way goes on where it stopped). The second passes are the
# repeat the plan's test asks for: the same estimates, and median run times within 5%.
#
# usage: [TARGETS=a,b] bench/planner/record.sh <amphora-bench> <out dir> <data dir> <JOB queries dir> [pass...]
#   <data dir> holds tpch-sf1, tpch-sf10 and imdb; the passes default to all five:
#   tpch-sf1-a tpch-sf1-b tpch-sf10-a job-a job-b
set -euo pipefail
bin=${1:?amphora-bench}
out=${2:?out dir}
data=${3:?data dir}
job=${4:?JOB queries}
shift 4
passes=("$@")
if [[ ${#passes[@]} -eq 0 ]]; then
  passes=(tpch-sf1-a tpch-sf1-b tpch-sf10-a job-a job-b)
fi
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
mkdir -p "$out"
# Native Windows programs take Windows paths; elsewhere a path is itself.
native() { if command -v cygpath >/dev/null; then cygpath -w "$1"; else echo "$1"; fi; }
# TARGETS: the engines, all five unless given (PostgreSQL only when its server is there to start).
targets=${TARGETS:-amphora,sqlite,duckdb,duckdb-mt,postgres}
if [[ ",$targets," == *,postgres,* ]]; then
  bash "$here/postgres.sh" start
fi
for pass in "${passes[@]}"; do
  case "$pass" in
    tpch-sf1-*) args=(--bench tpch --sf 1 --data "$(native "$data/tpch-sf1")" --queries "$(native "$here/tpch/queries")" --dir "$(native "$data/db-tpch-sf1")") ;;
    tpch-sf10-*) args=(--bench tpch --sf 10 --data "$(native "$data/tpch-sf10")" --queries "$(native "$here/tpch/queries")" --dir "$(native "$data/db-tpch-sf10")") ;;
    job-*) args=(--bench job --data "$(native "$data/imdb")" --queries "$(native "$job")" --dir "$(native "$data/db-job")") ;;
    *) echo "no pass $pass" >&2; exit 2 ;;
  esac
  echo "== $pass $(date -u +%Y-%m-%dT%H:%M:%SZ)" >&2
  "$bin" --planner "${args[@]}" --targets "$targets" --runs 3 --timeout-s 600 --count-timeout-s 1800 \
    --max-load 2 --load-wait-s 3600 --out "$(native "$out/$pass.jsonl")" --resume 2>>"$out/$pass.log"
done
echo "== done $(date -u +%Y-%m-%dT%H:%M:%SZ)" >&2
