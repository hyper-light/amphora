#!/usr/bin/env bash
# The SQLite language against every engine (G2 step 1): rebuilds the inventory from SQLite's
# documentation and checks it (nothing documented missing), writes the probes, runs them on each
# engine and builds the support matrix.
#
# usage: bench/corpus/run.sh <results dir> [engine...]
#   engines: sqlite duckdb amphora turso postgres orioledb cedardb mysql clickhouse (all by default)
# needs:
#   SQLITE_DOC     sqlite.org's documentation, unzipped (sqlite-doc-NNNNNNN)
#   AMPHORA_BENCH  the bench built with --probe (for amphora and turso)
#   CORPUS_PYTHON  a Python with sqlglot, psycopg, pymysql, requests and duckdb (python3 by default)
# The server engines run in the bench's own containers (bench/servers/<engine>.sh): each is started
# for its probes, restarted if a probe kills it, and stopped after.
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
bench=$(dirname "$here")
out=${1:?usage: run.sh <results dir> [engine...]}
shift
py=${CORPUS_PYTHON:-python3}
doc=${SQLITE_DOC:?set SQLITE_DOC to the sqlite.org documentation directory}
engines=("$@")
if [[ ${#engines[@]} -eq 0 ]]; then
  engines=(sqlite duckdb amphora turso postgres orioledb cedardb mysql clickhouse)
fi
declare -A dsn=(
  [postgres]="postgres://postgres@127.0.0.1:55432/postgres"
  [orioledb]="postgres://postgres@127.0.0.1:55433/postgres"
  [cedardb]="postgres://postgres:amphorabenchlocalonly2026@127.0.0.1:55434/postgres"
  [mysql]="mysql://root@127.0.0.1:33306/convo"
  [clickhouse]="http://127.0.0.1:58123"
)
mkdir -p "$out"

"$py" -I "$here/inventory.py" "$doc" "$here/features.json"
"$py" -I "$here/check_inventory.py" "$doc" "$here/features.json" | tee "$out/check_inventory.txt"
"$py" -I "$here/probes.py" "$here/features.json" "$here/probes.json"
"$py" -I "$here/spellings.py" "$here/spellings.json" "$here/probes.json"

server=""
stop_server() {
  if [[ -n "$server" ]]; then
    bash "$bench/servers/$server.sh" stop || true
    server=""
  fi
}
trap stop_server EXIT

for e in "${engines[@]}"; do
  echo "$(date -Is) $e"
  case "$e" in
    sqlite | duckdb)
      "$py" -I "$here/probe.py" "$e" "$here/probes.json" "$out/$e.json" --spellings "$here/spellings.json"
      ;;
    amphora | turso)
      if [[ $e == amphora ]]; then
        label="Amphora $(git -C "$bench" rev-parse --short HEAD)$(git -C "$bench" diff --quiet HEAD -- ../crates || echo ' with uncommitted engine changes')"
      else
        label="Turso $(sed -n '/^name = "turso_core"/{n;s/version = //;s/"//g;p;q}' "$bench/Cargo.lock") at $(grep -o 'turso?rev=[0-9a-f]*' "$bench/Cargo.lock" | head -1 | cut -c11-17)"
      fi
      "${AMPHORA_BENCH:?set AMPHORA_BENCH to the bench binary}" --probe --target "$e" --probes "$here/probes.json" --out "$out/$e.json" --label "$label"
      ;;
    postgres | orioledb | cedardb | mysql | clickhouse)
      server=$e
      bash "$bench/servers/$e.sh" start
      "$py" -I "$here/probe.py" "$e" "$here/probes.json" "$out/$e.json" --dsn "${dsn[$e]}" \
        --restart "bash $bench/servers/$e.sh start" --spellings "$here/spellings.json"
      stop_server
      ;;
    *)
      echo "unknown engine $e" >&2
      exit 2
      ;;
  esac
done

"$py" -I "$here/matrix.py" "$here/features.json" "$here/probes.json" "$out" "$here/matrix.json" "$here/matrix.md"
