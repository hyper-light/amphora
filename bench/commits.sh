#!/usr/bin/env bash
# Durable single-row commits from many writers (goal "Many clients write through SQL at once, safely":
# 20,000 durable commits a second with 32 writers, against SQLite and PostgreSQL). One driver for
# every system, `amphora-bench --powercut bench` (the power-cut harness's commit loop, the same
# writers, retries and clock), on one machine, one disk and one file system, every system native
# (no containers): Amphora on the pager's storage, Amphora on the versioned storage, SQLite (WAL,
# synchronous = FULL) and PostgreSQL (its own cluster, fresh for each writer count, its data on the
# same file system, fsync and synchronous_commit on, reached over a Unix socket).
#
# Writers climb the concurrency ladder (1, 8, 32, 128, 512, 2048, 4096; the brief's "Concurrency
# ladder") unless counts are given; a tiny run passes `1 8 32 128`. Each system starts from an empty
# database at each count. Prints a JSON record per system and count: the driver's own (engine and
# settings, client, commits a second, p50/p99/p99.9), the platform (host, OS, kernel, file system,
# device), and the client's resources (`client`: its CPU time, the cores it kept busy over the whole
# process, its peak memory, its connections, the machine's cores), so the top of the ladder shows
# whether the driver, not the engine, was the limit. For Amphora and SQLite the engine runs inside
# the driver's process, so their `client` numbers are the engine's too; PostgreSQL's server is apart.
#
# usage: bench/commits.sh <amphora-bench binary> <dir on the disk under test> <commits> [writers...]
#   PG_BIN  the directory of PostgreSQL's initdb and pg_ctl (default: `pg_config --bindir`)
#   ONLY    the systems to run, of "pager versioned sqlite postgres" (default: all four)
set -euo pipefail
bin=$(realpath -- "$1")
dir=$2
commits=$3
shift 3
ladder=${*:-1 8 32 128 512 2048 4096}
mkdir -p "$dir"
dir=$(cd -- "$dir" && pwd)
systems=${ONLY:-pager versioned sqlite postgres}

case "$(uname -s)" in
  Linux)
    fs=$(findmnt -no FSTYPE -T "$dir")
    dev=$(findmnt -no SOURCE -T "$dir")
    ;;
  Darwin)
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

# Each SQLite connection holds three files (the database, its WAL and its shared memory) and each
# PostgreSQL one a socket: the soft limit on open files goes up to the hard one, and a count the
# hard limit can't hold is refused before it runs, not part way.
ulimit -n "$(ulimit -Hn)" 2> /dev/null || true
files=$(ulimit -n)

# Runs the driver, then prints its record with the platform and the client's resources beside it
# (getrusage of the finished child: CPU and peak memory; ru_maxrss is KiB on Linux, bytes on macOS).
measure() {
  python3 -I - "$platform" "$@" << 'PY'
import json, os, resource, subprocess, sys, time
platform, cmd = json.loads(sys.argv[1]), sys.argv[2:]
began = time.monotonic()
done = subprocess.run(cmd, stdout=subprocess.PIPE)
wall = time.monotonic() - began
if done.returncode != 0:
    sys.exit(f"{' '.join(cmd)}: exit {done.returncode}")
r = json.loads(done.stdout.decode())
use = resource.getrusage(resource.RUSAGE_CHILDREN)
rss = use.ru_maxrss * (1 if sys.platform == "darwin" else 1024)
cpu = use.ru_utime + use.ru_stime
r["platform"] = platform
r["client"] = {
    "driver": r.get("client"),
    "cpu_user_s": use.ru_utime,
    "cpu_sys_s": use.ru_stime,
    "process_s": wall,
    "cores_busy": cpu / wall if wall > 0 else None,
    "cores": os.cpu_count(),
    "max_rss_bytes": rss,
    "connections": r.get("writers"),
}
print(json.dumps(r), flush=True)
PY
}

pg=
sock=
cleanup() {
  if [ -n "$pg" ]; then "$pg_bin/pg_ctl" -D "$pg" -m fast -w stop > /dev/null 2>&1 || true; fi
  if [ -n "$sock" ]; then rm -rf "$sock"; fi
}
trap cleanup EXIT

for writers in $ladder; do
  open=$(find /dev/fd/ -mindepth 1 -maxdepth 1 2> /dev/null | wc -l)
  if [ "$files" != unlimited ] && [ $((3 * writers + open)) -gt "$files" ]; then
    echo "$writers writers: SQLite's connections need $((3 * writers + open)) open files; the limit is $files" >&2
    exit 2
  fi
  for s in $systems; do
    run="$dir/$writers/$s"
    rm -rf "$run"
    mkdir -p "$run"
    case "$s" in
      pager) measure env -u AMPHORA_STORAGE "$bin" --powercut bench --engine amphora --dir "$run" --writers "$writers" --commits "$commits" ;;
      versioned) measure env AMPHORA_STORAGE=versioned "$bin" --powercut bench --engine amphora --dir "$run" --writers "$writers" --commits "$commits" ;;
      sqlite) measure "$bin" --powercut bench --engine sqlite --dir "$run" --writers "$writers" --commits "$commits" ;;
      postgres)
        pg_bin=${PG_BIN:-$(pg_config --bindir)}
        pg="$run/data"
        # The socket's own short directory: a Unix socket's path is limited to about 100 bytes.
        sock=$(mktemp -d "${TMPDIR:-/tmp}/amphora-pg.XXXXXX")
        "$pg_bin/initdb" -D "$pg" -U postgres --auth=trust > "$run/initdb.log"
        # Connections: every writer's, and the driver's setup connection, which may still be
        # closing on the server as the writers connect.
        "$pg_bin/pg_ctl" -D "$pg" -l "$run/postgres.log" -w \
          -o "-c listen_addresses='' -c unix_socket_directories='$sock' -c max_connections=$((writers + 1)) -c fsync=on -c synchronous_commit=on -c full_page_writes=on" start > /dev/null
        measure "$bin" --powercut bench --engine postgres --dsn "host=$sock user=postgres dbname=postgres" --writers "$writers" --commits "$commits"
        "$pg_bin/pg_ctl" -D "$pg" -m fast -w stop > /dev/null
        pg=
        rm -rf "$sock"
        sock=
        ;;
      *) echo "unknown system $s (pager, versioned, sqlite, postgres)" >&2; exit 2 ;;
    esac
  done
done
