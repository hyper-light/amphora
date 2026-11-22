#!/usr/bin/env bash
# The restore benchmark (AC-T1.12.3, kiln goal 15580afd): the same data restored by Amphora from
# its folder store, by PostgreSQL from pgBackRest's posix repository and by SQLite from
# Litestream's file replica, on one machine; each leg prints one line
# `restore system=<name> ... ns=<restore nanoseconds>`. The goal's bar: 100 GB in 30 minutes.
#
#   BYTES=<data bytes> WORK=<folder on the disk under test> bench/backup_restore.sh [amphora] [sqlite] [postgres]
#
# BYTES defaults to 4 MiB (a tiny instance); WORK to ~/.cache/amphora-restore-bench (never /tmp:
# a RAM disk on some machines). Rows are 4096 bytes in every leg. The two others run in Docker
# (postgres:17 with Debian's pgbackrest; litestream/litestream:0.3.13), so only Docker is needed.
set -euo pipefail
BYTES=${BYTES:-$((4 << 20))}
WORK=${WORK:-$HOME/.cache/amphora-restore-bench}
ROWS=$(( (BYTES + 4095) / 4096 ))
LEGS=${*:-amphora sqlite postgres}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
now() { date +%s%N; }

amphora() {
  rm -rf "$WORK/amphora"; mkdir -p "$WORK/amphora"
  (cd "$ROOT" && AMPHORA_RESTORE_BENCH_BYTES=$BYTES AMPHORA_RESTORE_BENCH_DIR=$WORK/amphora \
    cargo test -q -p amphora --release --locked --test backup restore_bench_amphora -- --ignored --nocapture) \
    | grep '^restore system='
}

sqlite() {
  local w=$WORK/sqlite; rm -rf "$w"; mkdir -p "$w"
  sqlite3 "$w/src.db" "PRAGMA journal_mode=WAL; CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL);
    WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < $ROWS)
    INSERT INTO t SELECT i, printf('%.4096c', 'x') FROM n;" >/dev/null
  # Replicate until the first snapshot is whole, then stop: Litestream's -exec ends with its child.
  docker run --rm -u "$(id -u):$(id -g)" -v "$w:/w" litestream/litestream:0.3.13 replicate \
    -exec "sh -c 'until ls /w/replica/generations/*/snapshots/* >/dev/null 2>&1; do sleep 1; done'" \
    /w/src.db file:///w/replica >/dev/null 2>&1
  local t0; t0=$(now)
  docker run --rm -u "$(id -u):$(id -g)" -v "$w:/w" litestream/litestream:0.3.13 restore -o /w/restored.db file:///w/replica >/dev/null
  local t1; t1=$(now)
  local got; got=$(sqlite3 "$w/restored.db" 'SELECT count(*) FROM t')
  [ "$got" = "$ROWS" ] || { echo "sqlite: restored $got rows of $ROWS" >&2; return 1; }
  echo "restore system=sqlite-litestream rows=$ROWS source_bytes=$(stat -c %s "$w/src.db") ns=$((t1 - t0)) (docker start included)"
}

postgres() {
  local w=$WORK/postgres name=amphora-restore-bench-pg
  # The repository is written by the containers' root: removed by one.
  [ -d "$w" ] && docker run --rm -v "$w:/w" postgres:17 rm -rf /w/repo; rm -rf "$w"; mkdir -p "$w/repo"; chmod -R 777 "$w"
  local conf='printf "[global]\nrepo1-path=/repo\n[db]\npg1-path=/var/lib/postgresql/data\n" > /etc/pgbackrest.conf'
  local install='apt-get update -qq && apt-get install -y -qq pgbackrest >/dev/null'
  docker rm -f "$name" >/dev/null 2>&1 || true
  docker run -d --name "$name" -e POSTGRES_HOST_AUTH_METHOD=trust -v "$w/repo:/repo" postgres:17 >/dev/null
  trap 'docker rm -f '"$name"' >/dev/null 2>&1 || true' RETURN
  docker exec "$name" sh -c "$install && $conf"
  docker exec "$name" sh -c 'until pg_isready -q; do sleep 1; done'
  docker exec -u postgres "$name" psql -q -c "ALTER SYSTEM SET archive_mode = on" -c "ALTER SYSTEM SET archive_command = 'pgbackrest --stanza=db archive-push %p'"
  docker restart "$name" >/dev/null; docker exec "$name" sh -c 'until pg_isready -q; do sleep 1; done'
  docker exec -u postgres "$name" pgbackrest --stanza=db stanza-create >/dev/null
  docker exec -u postgres "$name" psql -q -c "CREATE TABLE t (id bigint PRIMARY KEY, v text NOT NULL)" -c "ALTER TABLE t ALTER v SET STORAGE PLAIN" \
    -c "INSERT INTO t SELECT i, repeat('x', 4096) FROM generate_series(0, $ROWS - 1) i"
  docker exec -u postgres "$name" pgbackrest --stanza=db --type=full backup >/dev/null
  local bytes; bytes=$(docker exec -u postgres "$name" psql -tAq -c "SELECT pg_database_size(current_database())")
  docker rm -f "$name" >/dev/null
  # A fresh server restores the repository into an empty data directory, timed around the restore
  # alone; then it starts (replaying the archived WAL) and counts the rows.
  local out; out=$(docker run --rm -v "$w/repo:/repo" postgres:17 sh -c "$install && $conf && mkdir -p /var/lib/postgresql/data && chown postgres /var/lib/postgresql/data && chmod 700 /var/lib/postgresql/data &&
    t0=\$(date +%s%N) && gosu postgres pgbackrest --stanza=db restore >/dev/null && t1=\$(date +%s%N) &&
    gosu postgres pg_ctl -D /var/lib/postgresql/data -w -t 3600 start >/dev/null && gosu postgres psql -tAq -c 'SELECT count(*) FROM t' | sed \"s/^/rows=/; s/\$/ ns=\$((t1 - t0))/\"")
  case "$out" in "rows=$ROWS "*) ;; *) echo "postgres: $out (expected rows=$ROWS)" >&2; return 1 ;; esac
  echo "restore system=postgres-pgbackrest source_bytes=$bytes $out (WAL replay at start not included)"
}

for leg in $LEGS; do "$leg"; done
