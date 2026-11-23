#!/usr/bin/env bash
# The restore benchmark (AC-T1.12.3, kiln goal 15580afd): the same data restored by Amphora from
# its folder store, by PostgreSQL from pgBackRest's posix repository and by SQLite from
# Litestream's file replica. The goal's bar: 100 GB in 30 minutes.
#
#   BYTES=<data bytes> WORK=<folder on the disk under test> bench/backup_restore.sh [amphora] [sqlite] [postgres]
#
# A fair comparison (BRIEF.md, "Fair comparisons"): every leg runs the same way, in one container
# image (postgres:17, Debian bookworm, with pgBackRest, sqlite3 and Litestream added: $PREFIX-image)
# on one Docker host, its data on $WORK (one disk), and each restore is timed inside its container
# by the same `date +%s%N` pair around the system's own restore command, its process start
# included; PostgreSQL's time runs on through its start, which replays the archived log, as a
# restore isn't usable before it. Each leg then counts the rows with its own client, untimed. Each
# record says the client and the platform, and its size counts the database's log beside its file.
#
# BYTES defaults to 4 MiB (a tiny instance); WORK to ~/.cache/amphora-restore-bench (never /tmp:
# a RAM disk on some machines); PREFIX (container, image and volume names) to amphora-restore-bench.
# Rows are 4096 bytes in every leg. Only Docker is needed: Amphora's leg is built in rust:1-bookworm,
# whose glibc is the image's.
set -euo pipefail
BYTES=${BYTES:-$((4 << 20))}
WORK=${WORK:-$HOME/.cache/amphora-restore-bench}
PREFIX=${PREFIX:-amphora-restore-bench}
ROWS=$(( (BYTES + 4095) / 4096 ))
LEGS=${*:-amphora sqlite postgres}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
LITESTREAM=0.3.13
IMAGE=$PREFIX-image
mkdir -p "$WORK"

docker build -q -t "$IMAGE" - >/dev/null <<EOF
FROM postgres:17
RUN apt-get update -qq && apt-get install -y -qq pgbackrest sqlite3 curl ca-certificates >/dev/null \
 && curl -fsSL https://github.com/benbjohnson/litestream/releases/download/v$LITESTREAM/litestream-v$LITESTREAM-linux-\$(dpkg --print-architecture).tar.gz | tar xz -C /usr/local/bin \
 && printf "[global]\nrepo1-path=/w/repo\n[db]\npg1-path=/var/lib/postgresql/data\n" > /etc/pgbackrest.conf
EOF
PLATFORM="docker:$(docker version -f '{{.Server.Os}}/{{.Server.Arch}}'),image=postgres:17+$IMAGE,cpus=$(docker info -f '{{.NCPU}}'),mem=$(docker info -f '{{.MemTotal}}')"

# One container of the image on the leg's folder, named for the leg; what it leaves is removed by it.
leg() {
  local name=$1 w=$2; shift 2
  docker rm -f "$PREFIX-$name" >/dev/null 2>&1 || true
  docker run --rm --name "$PREFIX-$name" -v "$w:/w" "$@"
}
fresh() {
  local w=$WORK/$1
  [ -d "$w" ] && docker run --rm -v "$w:/w" "$IMAGE" sh -c 'rm -rf /w/* /w/.[!.]*' >/dev/null 2>&1 || true
  mkdir -p "$w"; chmod 777 "$w"; echo "$w"
}
# The timer every leg runs inside its container, around its restore command alone.
TIMED='t0=$(date +%s%N); restore; t1=$(date +%s%N); echo "ns=$((t1 - t0))" > /w/restore.ns'

amphora() {
  local w; w=$(fresh amphora)
  # Built in rust:1-bookworm (the image's glibc), into a volume of its own.
  local exe
  exe=$(docker run --rm -v "$ROOT:/src:ro" -v "$PREFIX-cargo:/usr/local/cargo/registry" -v "$PREFIX-target:/target" \
    -e CARGO_TARGET_DIR=/target rust:1-bookworm sh -c 'cd /src && cargo test -q -p amphora --release --locked --test backup --no-run 2>&1' \
    | sed -n 's/.*Executable tests\/backup.rs (\(.*\))/\1/p')
  [ -n "$exe" ] || { echo "amphora: the bench didn't build" >&2; return 1; }
  docker run --rm -v "$PREFIX-target:/target:ro" -v "$w:/w" "$IMAGE" cp "$exe" /w/bench
  local run="/w/bench --exact restore_bench_amphora --ignored --nocapture"
  leg amphora "$w" -e AMPHORA_RESTORE_BENCH_DIR=/w -e AMPHORA_RESTORE_BENCH_BYTES="$BYTES" -u postgres "$IMAGE" sh -c "
    AMPHORA_RESTORE_BENCH_PHASE=backup $run | grep '^backup ' > /w/backup.out
    restore() { AMPHORA_RESTORE_BENCH_PHASE=restore $run >/dev/null; }
    $TIMED
    AMPHORA_RESTORE_BENCH_PHASE=count $run | grep '^count ' > /w/count.out"
  local rows; rows=$(sed -n 's/.*rows=\([0-9]*\).*/\1/p' "$w/count.out")
  [ "$rows" = "$ROWS" ] || { echo "amphora: restored $rows rows of $ROWS" >&2; return 1; }
  echo "restore system=amphora client=amphora-restore platform=$PLATFORM rows=$rows $(sed 's/^backup //' "$w/backup.out") $(cat "$w/restore.ns")"
}

sqlite() {
  local w; w=$(fresh sqlite)
  leg sqlite "$w" -u postgres "$IMAGE" sh -c "
    sqlite3 /w/src.db \"PRAGMA journal_mode=WAL; CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL);
      WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < $ROWS)
      INSERT INTO t SELECT i, printf('%.4096c', 'x') FROM n;\" >/dev/null
    echo \$(( \$(stat -c %s /w/src.db) + \$(stat -c %s /w/src.db-wal 2>/dev/null || echo 0) )) > /w/source.bytes
    # Replicate until the first snapshot is whole, then stop: Litestream's -exec ends with its child.
    litestream replicate -exec \"sh -c 'until ls /w/replica/generations/*/snapshots/* >/dev/null 2>&1; do sleep 1; done'\" /w/src.db file:///w/replica >/dev/null 2>&1
    restore() { litestream restore -o /w/restored.db file:///w/replica >/dev/null; }
    $TIMED
    sqlite3 /w/restored.db 'SELECT count(*) FROM t' > /w/count.out"
  local rows; rows=$(cat "$w/count.out")
  [ "$rows" = "$ROWS" ] || { echo "sqlite: restored $rows rows of $ROWS" >&2; return 1; }
  echo "restore system=sqlite-litestream client=litestream-$LITESTREAM platform=$PLATFORM rows=$rows source_bytes=$(cat "$w/source.bytes") $(cat "$w/restore.ns")"
}

postgres() {
  local w; w=$(fresh postgres)
  mkdir -p "$w/repo"; chmod 777 "$w/repo"
  # The source: a server archiving its log to pgBackRest, loaded, then a full backup.
  leg postgres-source "$w" -d -e POSTGRES_HOST_AUTH_METHOD=trust "$IMAGE" >/dev/null
  local name=$PREFIX-postgres-source
  trap 'docker rm -f '"$name"' >/dev/null 2>&1 || true' RETURN
  docker exec "$name" sh -c 'until pg_isready -q; do sleep 1; done'
  docker exec -u postgres "$name" psql -q -c "ALTER SYSTEM SET archive_mode = on" -c "ALTER SYSTEM SET archive_command = 'pgbackrest --stanza=db archive-push %p'"
  docker restart "$name" >/dev/null; docker exec "$name" sh -c 'until pg_isready -q; do sleep 1; done'
  docker exec -u postgres "$name" pgbackrest --stanza=db stanza-create >/dev/null
  docker exec -u postgres "$name" psql -q -c "CREATE TABLE t (id bigint PRIMARY KEY, v text NOT NULL)" -c "ALTER TABLE t ALTER v SET STORAGE PLAIN" \
    -c "INSERT INTO t SELECT i, repeat('x', 4096) FROM generate_series(0, $ROWS - 1) i"
  docker exec -u postgres "$name" pgbackrest --stanza=db --type=full backup >/dev/null
  # The size: the database's files and its log.
  local bytes; bytes=$(docker exec -u postgres "$name" psql -tAq -c "SELECT pg_database_size(current_database()) + (SELECT coalesce(sum(size), 0) FROM pg_ls_waldir())")
  docker rm -f "$name" >/dev/null
  # A fresh server restores the repository into an empty data directory and starts, replaying the
  # archived log, timed until it accepts connections; then it counts the rows.
  leg postgres "$w" "$IMAGE" sh -c "
    mkdir -p /var/lib/postgresql/data && chown postgres /var/lib/postgresql/data && chmod 700 /var/lib/postgresql/data
    restore() { gosu postgres pgbackrest --stanza=db restore >/dev/null && gosu postgres pg_ctl -D /var/lib/postgresql/data -w -t 3600 start >/dev/null; }
    $TIMED
    gosu postgres psql -tAq -c 'SELECT count(*) FROM t' > /w/count.out"
  local rows; rows=$(cat "$w/count.out")
  [ "$rows" = "$ROWS" ] || { echo "postgres: restored $rows rows of $ROWS" >&2; return 1; }
  echo "restore system=postgres-pgbackrest client=pgbackrest+pg_ctl platform=$PLATFORM rows=$rows source_bytes=$bytes $(cat "$w/restore.ns") (start's log replay included)"
}

for leg in $LEGS; do "$leg"; done
