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
# the driver's process, so their `client` numbers are the engine's too; PostgreSQL's server is apart,
# its memory read with every connection open (`client.held`). Before each count, what it needs is
# extrapolated from the counts that system already ran and checked against the host (memory, open
# files, threads); one that doesn't fit is refused with exit 2, saying why. PostgreSQL's
# shared_buffers and work_mem are sized to fit beside its backends (bench/commits_fit.py), and the
# settings are in its record (`engine`) and in <dir>/<writers>/postgres/sizing.json.
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

# The soft limit on open files goes up to the hard one (each SQLite connection holds three files,
# each PostgreSQL one a socket). What each count needs (memory, open files, threads) is learned from
# the counts already run on that system and checked against the host before it runs, so a count the
# host can't hold is refused, saying why, not run unfairly (bench/commits_fit.py check).
ulimit -n "$(ulimit -Hn)" 2> /dev/null || true
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
learned="$dir/learned.jsonl"
rm -f "$learned"

# Runs the driver with --hold, reads what it holds at its last commit (its open files, PostgreSQL's
# memory), then prints its record with the platform and the client's resources beside it.
measure() {
  python3 -I "$here/commits_fit.py" measure "$learned" "$platform" "$@"
}

# Refuses a count that doesn't fit (exit 2, why on stderr); its JSON verdict otherwise.
fits() {
  local verdict
  if ! verdict=$(python3 -I "$here/commits_fit.py" check "$learned" "$@"); then
    echo "$1 at $2 writers: refused: $(printf '%s' "$verdict" | python3 -Ic 'import json,sys; print(json.load(sys.stdin)["why"])')" >&2
    printf '%s\n' "$verdict" >&2
    return 2
  fi
  printf '%s' "$verdict"
}

pg=
sock=
cleanup() {
  if [ -n "$pg" ]; then "$pg_bin/pg_ctl" -D "$pg" -m fast -w stop > /dev/null 2>&1 || true; fi
  if [ -n "$sock" ]; then rm -rf "$sock"; fi
}
trap cleanup EXIT

for writers in $ladder; do
  for s in $systems; do
    run="$dir/$writers/$s"
    rm -rf "$run"
    mkdir -p "$run"
    case "$s" in
      pager)
        fits "$s" "$writers" > /dev/null || exit 2
        measure --system "$s" -- env -u AMPHORA_STORAGE "$bin" --powercut bench --engine amphora --dir "$run" --writers "$writers" --commits "$commits" --hold
        ;;
      versioned)
        fits "$s" "$writers" > /dev/null || exit 2
        measure --system "$s" -- env AMPHORA_STORAGE=versioned "$bin" --powercut bench --engine amphora --dir "$run" --writers "$writers" --commits "$commits" --hold
        ;;
      sqlite)
        fits "$s" "$writers" > /dev/null || exit 2
        measure --system "$s" -- "$bin" --powercut bench --engine sqlite --dir "$run" --writers "$writers" --commits "$commits" --hold
        ;;
      postgres)
        pg_bin=${PG_BIN:-$(pg_config --bindir)}
        pg="$run/data"
        # The socket's own short directory: a Unix socket's path is limited to about 100 bytes.
        sock=$(mktemp -d "${TMPDIR:-/tmp}/amphora-pg.XXXXXX")
        "$pg_bin/initdb" -D "$pg" -U postgres --auth=trust > "$run/initdb.log"
        # Connections: every writer's, and the driver's setup connection, which may still be
        # closing on the server as the writers connect.
        conns=$((writers + 1))
        # The shared memory its connections take beside the buffers (its lock table, its process
        # slots), as PostgreSQL computes it with the fewest buffers it allows (16 of 8kB), in MB.
        shared_mb=$("$pg_bin/postgres" -D "$pg" -C shared_memory_size -c max_connections="$conns" -c shared_buffers=128kB)
        # shared_buffers and work_mem sized to what the host has beside the backends and the
        # driver (bench/commits_fit.py pg_settings), recorded in the run's record and in sizing.json.
        verdict=$(fits "$s" "$writers" --pg-shared-bytes $((shared_mb * 1048576))) || exit 2
        printf '%s\n' "$verdict" > "$run/sizing.json"
        sb=$(printf '%s' "$verdict" | python3 -Ic 'import json,sys; print(json.load(sys.stdin)["pg"]["shared_buffers_kb"])')
        wm=$(printf '%s' "$verdict" | python3 -Ic 'import json,sys; print(json.load(sys.stdin)["pg"]["work_mem_kb"])')
        "$pg_bin/pg_ctl" -D "$pg" -l "$run/postgres.log" -w \
          -o "-c listen_addresses='' -c unix_socket_directories='$sock' -c max_connections=$conns -c shared_buffers=${sb}kB -c work_mem=${wm}kB -c fsync=on -c synchronous_commit=on -c full_page_writes=on" start > /dev/null
        measure --system "$s" --server-pid-file "$pg/postmaster.pid" -- "$bin" --powercut bench --engine postgres --dsn "host=$sock user=postgres dbname=postgres" --writers "$writers" --commits "$commits" --hold
        "$pg_bin/pg_ctl" -D "$pg" -m fast -w stop > /dev/null
        pg=
        rm -rf "$sock"
        sock=
        ;;
      *) echo "unknown system $s (pager, versioned, sqlite, postgres)" >&2; exit 2 ;;
    esac
  done
done
