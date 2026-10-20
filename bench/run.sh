#!/usr/bin/env bash
# Runs the bench once per target and prints a record line for each (JSON): the bench's report
# with when, where and on what it ran (CLAUDE.md §7). Records go to bench/records/ as JSON lines.
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
cpu=$(lscpu 2>/dev/null | sed -n 's/^Model name: *//p' | head -1)
cores=$(nproc)
mkdir -p "$data"
fs=$(findmnt -no FSTYPE -T "$data" 2>/dev/null || echo unknown)
dev=$(findmnt -no SOURCE -T "$data" 2>/dev/null | sed 's/\[.*//; s#/dev/##; s/p[0-9]*$//')
disk=$(cat "/sys/block/$dev/device/model" 2>/dev/null | xargs || echo unknown)
kernel=$(uname -r)
commit=$(git rev-parse HEAD 2>/dev/null || echo unknown)
dirty=$(git status --porcelain 2>/dev/null | wc -l)
for t in ${targets//,/ }; do
  load=$(cut -d' ' -f1 /proc/loadavg)
  out=$("$bin" --target "$t" --dir "$data/$t" "$@" | tail -1)
  load_after=$(cut -d' ' -f1 /proc/loadavg)
  python3 - "$out" <<PY
import json, sys
report = json.loads(sys.argv[1])
record = {
    "date": "$(date -u +%Y-%m-%dT%H:%M:%SZ)", "host": "$host", "cpu": "$cpu", "cores": $cores,
    "disk": "$disk", "fs": "$fs", "kernel": "$kernel", "commit": "$commit", "dirty_files": $dirty,
    "load_before": $load, "load_after": $load_after, "args": "$*", "report": report,
}
print(json.dumps(record))
PY
done
