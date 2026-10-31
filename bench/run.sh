#!/usr/bin/env bash
# Runs the bench once per target and prints a record line for each (JSON): the bench's report
# with when, where and on what it ran (CLAUDE.md §7). Records go to bench/records/ as JSON lines.
#
# A server's target (postgres, orioledb, cedardb, mysql, clickhouse, clickhouse-durable) runs against
# its container (servers/<engine>.sh), restarted first so that the high-water mark its cgroup keeps
# (cgroup v2's memory.peak, which only grows while the cgroup lives) is this run's alone; with
# AMPHORA_BENCH_RESTART=0 it is only started if it isn't running, and the peak is since it started.
# The record adds that peak (server_mem_peak_mb, and where it came from), the container's image, and
# the version and settings the server reports. memory.peak counts the page cache the server's own
# file I/O fills, which no process holds (Amphora's rss_max_mb leaves it out), so the record also
# has server_mem_held_peak_mb: the most anon + shmem its cgroup's memory.stat showed, read every
# 0.2 s through the run: the memory its processes hold, private and shared (PostgreSQL's
# shared_buffers are shmem), the figure to set beside a process's resident set.
#
# usage: bench/run.sh <target>[,<target>...] [bench arguments...]
#   e.g. bench/run.sh amphora,turso,sqlite --sessions 1000 --preload 100 --writers 32 --seconds 30
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
bin="${CARGO_TARGET_DIR:-target}/release/amphora-bench"
data="${AMPHORA_BENCH_DATA:-$HOME/.cache/amphora-bench-data}"
targets="$1"
shift
host=$(hostname -s)
# The host's limits on every target (servers/lib.sh), recorded with each run.
host_env="${AMPHORA_BENCH_HOST_ENV:-$HOME/.config/amphora-bench/host.env}"
if [[ -f "$host_env" ]]; then
  # shellcheck disable=SC1090
  . "$host_env"
fi
cpu=$(lscpu 2>/dev/null | sed -n 's/^Model name: *//p' | head -1)
cores=$(nproc)
mkdir -p "$data"
fs=$(findmnt -no FSTYPE -T "$data" 2>/dev/null || echo unknown)
dev=$(findmnt -no SOURCE -T "$data" 2>/dev/null | sed 's/\[.*//; s#/dev/##; s/p[0-9]*$//')
disk=$(cat "/sys/block/$dev/device/model" 2>/dev/null | xargs || echo unknown)
kernel=$(uname -r)
commit=$(git rev-parse HEAD 2>/dev/null || echo unknown)
dirty=$(git status --porcelain 2>/dev/null | wc -l)
# The engine whose container serves a target, if one does.
engine() {
  case "$1" in
    postgres | orioledb | cedardb | mysql) echo "$1" ;;
    clickhouse | clickhouse-durable) echo clickhouse ;;
  esac
}
# The most of anon + shmem a cgroup's memory.stat shows, read every 0.2 s until the cgroup goes or
# this is killed, written to $2 as it grows.
held_peak() {
  local most=0 now
  while now=$(awk '$1 == "anon" || $1 == "shmem" { n += $2 } END { print n + 0 }' "$1/memory.stat" 2>/dev/null); do
    if ((now > most)); then
      most=$now
      echo "$most" >"$2"
    fi
    sleep 0.2
  done
}
# A run cut short takes its sampler with it.
trap 'kill "${sampler:-}" 2>/dev/null || true' EXIT
for t in ${targets//,/ }; do
  e=$(engine "$t")
  if [[ -n "$e" ]]; then
    c="amphora-bench-$e"
    if [[ "${AMPHORA_BENCH_RESTART:-1}" == 0 ]]; then bash "servers/$e.sh" start; else bash "servers/$e.sh" restart; fi
    cgroup=/sys/fs/cgroup$(sed -n 's/^0:://p' "/proc/$(docker inspect -f '{{.State.Pid}}' "$c")/cgroup")
    held=$(mktemp -p "$data")
    held_peak "$cgroup" "$held" &
    sampler=$!
  fi
  load=$(cut -d' ' -f1 /proc/loadavg)
  out=$("$bin" --target "$t" --dir "$data/$t" "$@" | tail -1)
  load_after=$(cut -d' ' -f1 /proc/loadavg)
  server=()
  if [[ -n "$e" ]]; then
    kill "$sampler" 2>/dev/null || true
    wait "$sampler" 2>/dev/null || true
    # Reaped, its PID may be anyone's: the trap mustn't kill it again.
    sampler=
    # A server that died in the run still gets its record, without these.
    peak=$(cat "$cgroup/memory.peak" 2>/dev/null || true)
    # Without cgroup v2's memory.peak, docker stats' usage at the end: not a peak, and said so.
    stats=$([[ -n "$peak" ]] || docker stats --no-stream --format '{{.MemUsage}}' "$c" 2>/dev/null || true)
    server=(SERVER_CONTAINER="$c" SERVER_IMAGE="$(docker inspect -f '{{.Config.Image}}' "$c" 2>/dev/null || true)" SERVER_IMAGE_ID="$(docker inspect -f '{{.Image}}' "$c" 2>/dev/null || true)"
      SERVER_MEM_PEAK="$peak" SERVER_MEM_STATS="$stats" SERVER_MEM_HELD="$(cat "$held")" SERVER_SETTINGS="$(bash "servers/$e.sh" settings 2>/dev/null || true)")
    rm -f -- "${held:?}"
  fi
  env "${server[@]}" python3 - "$out" <<PY
import json, os, re, sys
report = json.loads(sys.argv[1])
record = {
    "date": "$(date -u +%Y-%m-%dT%H:%M:%SZ)", "host": "$host", "cpu": "$cpu", "cores": $cores,
    "disk": "$disk", "fs": "$fs", "kernel": "$kernel", "commit": "$commit", "dirty_files": $dirty,
    "load_before": $load, "load_after": $load_after, "args": "$*", "report": report,
    "limits": {"memory": "${AMPHORA_BENCH_MEMORY:-8g}", "write_bps": "${AMPHORA_BENCH_WRITE_BPS:-}", "write_devices": "${AMPHORA_BENCH_WRITE_DEVICES:-}"},
}
env = os.environ
if env.get("SERVER_CONTAINER"):
    if env.get("SERVER_MEM_PEAK"):
        peak, source = round(int(env["SERVER_MEM_PEAK"]) / 2**20, 1), "cgroup v2 memory.peak of the container"
    else:
        m = re.match(r"([0-9.]+)\s*([KMG]i?)?B", env.get("SERVER_MEM_STATS", ""))
        scale = {"K": 2**-10, "Ki": 2**-10, "M": 1, "Mi": 1, "G": 2**10, "Gi": 2**10}
        peak = round(float(m.group(1)) * scale.get(m.group(2) or "", 2**-20), 1) if m else None
        source = "docker stats: usage at the end of the run, not a peak"
    try:
        settings = json.loads(env.get("SERVER_SETTINGS") or "null")
    except ValueError:
        settings = env.get("SERVER_SETTINGS")
    held = env.get("SERVER_MEM_HELD")
    record.update({
        "server_container": env["SERVER_CONTAINER"], "server_image": env.get("SERVER_IMAGE"), "server_image_id": env.get("SERVER_IMAGE_ID"),
        "server_version": settings.get("version") if isinstance(settings, dict) else None, "server_settings": settings,
        "server_mem_peak_mb": peak, "server_mem_source": source,
        "server_mem_held_peak_mb": round(int(held) / 2**20, 1) if held else None,
    })
print(json.dumps(record))
PY
  # A server left running loads the machine under every target after it (an idle CedarDB,
  # ClickHouse or MySQL took half a core to two-thirds of one each): stopped once its record is
  # made, unless AMPHORA_BENCH_KEEP=1.
  if [[ -n "$e" && "${AMPHORA_BENCH_KEEP:-0}" != 1 ]]; then
    bash "servers/$e.sh" stop
  fi
done
