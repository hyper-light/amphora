#!/usr/bin/env bash
# Durable single-row commits from 32 writers (goal "Many clients write through SQL at once, safely":
# 20,000 durable commits a second with 32 writers, against SQLite and PostgreSQL). One driver for
# every system, `amphora-bench --powercut bench` (the power-cut harness's commit loop, the same
# writers, retries and clock), on one machine, one disk and one file system, every system native
# (no containers): Amphora on the pager's storage, Amphora on the versioned storage, SQLite (WAL,
# synchronous = FULL) and PostgreSQL (its own cluster, fresh, its data on the same file system,
# fsync and synchronous_commit on, reached over a Unix socket). Prints a JSON record per system,
# each with the client and the platform (host, OS, kernel, file system, device).
#
# usage: bench/commits.sh <amphora-bench binary> <dir on the disk under test> <commits> [writers]
#   PG_BIN  the directory of PostgreSQL's initdb and pg_ctl (default: `pg_config --bindir`)
#   ONLY    the systems to run, of "pager versioned sqlite postgres" (default: all four)
set -euo pipefail
bin=$(realpath -- "$1")
dir=$2
commits=$3
writers=${4:-32}
mkdir -p "$dir"
dir=$(cd -- "$dir" && pwd)
systems=${ONLY:-pager versioned sqlite postgres}

case "$(uname -s)" in
  Linux)
    fs=$(findmnt -no FSTYPE -T "$dir")
    dev=$(findmnt -no SOURCE -T "$dir")
    ;;
  Darwin)
    mp=$(df -P "$dir" | awk 'NR==2{print $6}')
    dev=$(df -P "$dir" | awk 'NR==2{print $1}')
    fs=$(mount | awk -v d="$dev" '$1==d{gsub(/[(,]/,"",$4); print $4; exit}')
    ;;
  *)
    fs=unknown
    dev=unknown
    ;;
esac
platform=$(printf '{"host":"%s","kernel":"%s","fs":"%s","device":"%s","dir":"%s"}' \
  "$(uname -n)" "$(uname -srm)" "$fs" "$dev" "$dir")

record() {
  # The driver's record with the platform beside it.
  python3 -I -c 'import json,sys; r=json.loads(sys.stdin.read()); r["platform"]=json.loads(sys.argv[1]); print(json.dumps(r))' "$platform"
}

pg=
sock=
cleanup() {
  if [ -n "$pg" ]; then "$pg_bin/pg_ctl" -D "$pg" -m fast -w stop > /dev/null 2>&1 || true; fi
  if [ -n "$sock" ]; then rm -rf "$sock"; fi
}
trap cleanup EXIT

for s in $systems; do
  run="$dir/$s"
  rm -rf "$run"
  mkdir -p "$run"
  case "$s" in
    pager) env -u AMPHORA_STORAGE "$bin" --powercut bench --engine amphora --dir "$run" --writers "$writers" --commits "$commits" | record ;;
    versioned) env AMPHORA_STORAGE=versioned "$bin" --powercut bench --engine amphora --dir "$run" --writers "$writers" --commits "$commits" | record ;;
    sqlite) "$bin" --powercut bench --engine sqlite --dir "$run" --writers "$writers" --commits "$commits" | record ;;
    postgres)
      pg_bin=${PG_BIN:-$(pg_config --bindir)}
      pg="$run/data"
      # The socket's own short directory: a Unix socket's path is limited to about 100 bytes.
      sock=$(mktemp -d "${TMPDIR:-/tmp}/amphora-pg.XXXXXX")
      "$pg_bin/initdb" -D "$pg" -U postgres --auth=trust > "$run/initdb.log"
      "$pg_bin/pg_ctl" -D "$pg" -l "$run/postgres.log" -w \
        -o "-c listen_addresses='' -c unix_socket_directories='$sock' -c fsync=on -c synchronous_commit=on -c full_page_writes=on" start > /dev/null
      "$bin" --powercut bench --engine postgres --dsn "host=$sock user=postgres dbname=postgres" --writers "$writers" --commits "$commits" | record
      "$pg_bin/pg_ctl" -D "$pg" -m fast -w stop > /dev/null
      pg=
      ;;
    *) echo "unknown system $s (pager, versioned, sqlite, postgres)" >&2; exit 2 ;;
  esac
done
