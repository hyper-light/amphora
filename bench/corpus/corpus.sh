#!/usr/bin/env bash
# G2 step 2: the correctness corpus, built and run on SQLite. Every source in sources.json run (each
# checkout at its revision, or nothing runs), kiln's statements and the written cases made, each
# translated source's files run back on SQLite and settled, and the totals written to a record.
#
# usage: bench/corpus/corpus.sh <out dir> <record dir>
#   CORPUS_SRC  where the sources are checked out (default ~/.cache/research-src)
#   CORPUS_PY   a Python with sqlglot and psycopg (default ~/.cache/corpus-venv/bin/python)
#   KILN_REPO   kiln's checkout (default ~/Projects/kiln)
#
# Statements run from a sandbox directory of their own (<out dir>/sandbox): one that ATTACHes a
# relative path writes there, and what it holds is listed and emptied at the end. PostgreSQL 17 runs
# in a container of the corpus's own (amphora-corpus-postgres on 127.0.0.1:55433, a fresh data
# directory, fsync off), never the bench's.
set -uo pipefail
HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
OUT=$(realpath -m -- "$1")
RECORD=$(realpath -m -- "$2")
SRC=${CORPUS_SRC:-$HOME/.cache/research-src}
PY=${CORPUS_PY:-$HOME/.cache/corpus-venv/bin/python}
KILN=${KILN_REPO:-$HOME/Projects/kiln}
F="$HERE/features.json"
SANDBOX="$OUT/sandbox"
PG=amphora-corpus-postgres
export PYTHONDONTWRITEBYTECODE=1

source_field() { "$PY" -I -c 'import json, sys; print(json.load(open(sys.argv[1]))["sources"][sys.argv[2]][sys.argv[3]])' "$HERE/sources.json" "$1" "$2"; }
stage() { echo "== $(date -Is) $*"; }
run() {
  stage "$@"
  (cd "$SANDBOX" && "$PY" -I "$HERE/build_corpus.py" "$@")
  local rc=$?
  stage "$1 exit $rc"
  return $rc
}

for s in slt duckdb cockroach postgres; do
  dir="$SRC/$(source_field "$s" checkout)"
  want=$(source_field "$s" revision)
  have=$(git -C "$dir" rev-parse HEAD 2>/dev/null)
  if [[ "$have" != "$want" ]]; then
    echo "$s: $dir is at ${have:-nothing}, sources.json reads $want" >&2
    exit 1
  fi
done
kiln_rev=$(source_field kiln revision)
git -C "$KILN" cat-file -e "$kiln_rev^{commit}" || { echo "kiln: $KILN has no $kiln_rev" >&2; exit 1; }

mkdir -p "$OUT" "$SANDBOX"
run slt "$SRC/$(source_field slt checkout)" "$F" "$OUT" --jobs 4 || exit 1
run duckdb "$SRC/$(source_field duckdb checkout)" "$F" "$OUT" --jobs 4 || exit 1
run cockroach "$SRC/$(source_field cockroach checkout)" "$F" "$OUT" --jobs 4 || exit 1

pgdata="$OUT/pgdata"
rm -rf -- "${pgdata:?}"
mkdir -p "$pgdata"
docker rm -f "$PG" >/dev/null 2>&1
docker run -d --name "$PG" --network host --user "$(id -u):$(id -g)" --memory 4g --memory-swap 4g --cpus 4 --shm-size 256m \
  -e POSTGRES_HOST_AUTH_METHOD=trust -v "$pgdata:/var/lib/postgresql/data" postgres:17 \
  postgres -c listen_addresses=127.0.0.1 -c port=55433 -c fsync=off -c synchronous_commit=off -c full_page_writes=off >/dev/null || exit 1
for _ in $(seq 240); do docker exec "$PG" pg_isready -q -h 127.0.0.1 -p 55433 -U postgres && break; sleep 0.5; done
docker exec "$PG" psql -h 127.0.0.1 -p 55433 -U postgres -AtX -c "SELECT version()"
CORPUS_PG_DSN="postgres://postgres@127.0.0.1:55433/postgres" run postgres "$SRC/$(source_field postgres checkout)" "$F" "$OUT" --jobs 4
pg_rc=$?
docker exec "$PG" psql -h 127.0.0.1 -p 55433 -U postgres -AtX -c "SELECT count(*) || ' script databases left' FROM pg_database WHERE datname LIKE 'corpus\_%' AND datname <> 'corpus_base'"
docker rm -f "$PG" >/dev/null
rm -rf -- "${pgdata:?}"
[[ $pg_rc -eq 0 ]] || exit 1

stage kiln_queries
(cd "$SANDBOX" && "$PY" -I "$HERE/kiln_queries.py" "$KILN" "$kiln_rev" "$OUT") || exit 1
stage written_cases
(cd "$SANDBOX" && "$PY" -I "$HERE/written_cases.py" "$F" "$OUT") || exit 1
run kiln "$OUT" "$F" "$OUT" --jobs 1 || exit 1
run written "$OUT" "$F" "$OUT" --jobs 4 || exit 1

# every translated source's files run back on SQLite; a query whose answer moved between the run
# and its replay is settled into a statement, and the replay runs again
for s in duckdb cockroach postgres; do
  run "replay-$s" "$OUT" "$F" "$OUT" --jobs 4 || exit 1
  "$PY" -I "$HERE/settle.py" "$OUT" "$s"
  run "replay-$s" "$OUT" "$F" "$OUT" --jobs 4 || exit 1
done

stage aggregate
"$PY" -I "$HERE/aggregate.py" "$F" "$OUT" "$RECORD" "$HERE/sources.json" "$HERE/triage.json" || exit 1
"$PY" -I -c 'import sqlite3; c = sqlite3.connect(":memory:"); print("SQLite", sqlite3.sqlite_version, "compile options:", ", ".join(r[0] for r in c.execute("PRAGMA compile_options")))' > "$RECORD/sqlite.txt"
echo "sandbox: $(find "$SANDBOX" -mindepth 1 | wc -l) files left by statements: $(find "$SANDBOX" -mindepth 1 -printf '%P ' | head -c 500)"
rm -rf -- "${SANDBOX:?}"
stage done
