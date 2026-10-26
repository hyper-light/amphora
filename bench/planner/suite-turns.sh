#!/usr/bin/env bash
# The bench suite (`amphora-bench --suite`) on two builds in turns, each run begun once the machine
# is quiet, each recorded as bench/run.sh records one (CLAUDE.md §7): when, where, which commit,
# the load before and after, and the suite's report. Runs on Linux and in Git for Windows' shell,
# where Windows keeps no load average and its nearest stands in (bench/planner/README.md).
#
# usage: bench/planner/suite-turns.sh <out.jsonl> <turns> <name>=<checkout>:<binary>... -- <suite arguments>
#   e.g. suite-turns.sh out.jsonl 3 before=~/src/before:~/t-before/release/amphora-bench \
#          after=~/src/after:~/t-after/release/amphora-bench -- --target amphora --items 200000
set -euo pipefail
out=${1:?out}
turns=${2:?turns}
shift 2
builds=()
while [[ $# -gt 0 && "$1" != "--" ]]; do
  builds+=("$1")
  shift
done
shift
MAX_LOAD=${MAX_LOAD:-2}
LOAD_WAIT_S=${LOAD_WAIT_S:-3600}
DATA=${AMPHORA_BENCH_DATA:-$HOME/.cache/amphora-bench-data}

if [[ -r /proc/loadavg && "$(uname -s)" == Linux ]]; then
  load() { cut -d' ' -f1 /proc/loadavg; }
  load_kind="Linux's one-minute load average"
  host=$(hostname -s)
  cpu=$(lscpu | sed -n 's/^Model name: *//p' | head -1)
  kernel="Linux $(uname -r)"
  mkdir -p "$DATA"
  fs=$(findmnt -no FSTYPE -T "$DATA")
  dev=$(findmnt -no SOURCE -T "$DATA" | sed 's/\[.*//; s#/dev/##; s/p[0-9]*$//')
  disk=$(xargs <"/sys/block/$dev/device/model" 2>/dev/null || echo unknown)
else
  # Busy cores plus the processor queue, averaged over five seconds (the harness's own measure).
  load() {
    powershell -NoProfile -Command "\$c = [Environment]::ProcessorCount; \$s = Get-Counter '\\Processor(_Total)\\% Processor Time','\\System\\Processor Queue Length' -SampleInterval 1 -MaxSamples 5; \$b = (\$s | % { \$_.CounterSamples[0].CookedValue } | Measure-Object -Average).Average; \$q = (\$s | % { \$_.CounterSamples[1].CookedValue } | Measure-Object -Average).Average; '{0:N2}' -f (\$b / 100 * \$c + \$q)" | tr -d '\r'
  }
  load_kind="Windows: busy cores plus the processor queue, averaged over 5 s (it keeps no load average)"
  host=$(powershell -NoProfile -Command '$env:COMPUTERNAME' | tr -d '\r')
  cpu=$(powershell -NoProfile -Command '(Get-CimInstance Win32_Processor).Name' | tr -d '\r' | xargs)
  kernel=$(powershell -NoProfile -Command '$v = Get-CimInstance Win32_OperatingSystem; "$($v.Caption) $($v.Version)"' | tr -d '\r')
  drive=$(cygpath -w "$HOME" | cut -c1)
  disk=$(powershell -NoProfile -Command "(Get-Partition -DriveLetter $drive | Get-Disk).FriendlyName" | tr -d '\r' | xargs)
  fs=$(powershell -NoProfile -Command "(Get-Volume -DriveLetter $drive).FileSystemType" | tr -d '\r')
fi
cores=$(nproc)

# Waits for a quiet machine, at most LOAD_WAIT_S: the load it began at.
quiet() {
  local until=$((SECONDS + LOAD_WAIT_S)) now
  while now=$(load); do
    if awk -v l="$now" -v m="$MAX_LOAD" 'BEGIN { exit !(l < m) }' || ((SECONDS >= until)); then
      echo "$now"
      return
    fi
    sleep 5
  done
}

for ((turn = 1; turn <= turns; turn++)); do
  for build in "${builds[@]}"; do
    name=${build%%=*}
    rest=${build#*=}
    checkout=${rest%%:*}
    bin=${rest#*:}
    commit=$(git -C "$checkout" rev-parse HEAD)
    dirty=$(git -C "$checkout" status --porcelain | wc -l | tr -d ' ')
    dir="$DATA/suite-$name"
    load_before=$(quiet)
    date=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    report=$("$bin" --suite --dir "$dir" "$@" | tail -1)
    load_after=$(load)
    printf '{"date": "%s", "build": "%s", "turn": %d, "host": "%s", "cpu": "%s", "cores": %d, "disk": "%s", "fs": "%s", "kernel": "%s", "commit": "%s", "dirty_files": %d, "load_kind": "%s", "load_before": %s, "load_after": %s, "args": "%s", "report": %s}\n' \
      "$date" "$name" "$turn" "$host" "$cpu" "$cores" "$disk" "$fs" "$kernel" "$commit" "$dirty" "$load_kind" "$load_before" "$load_after" "--suite $*" "$report" >>"$out"
    echo "turn $turn $name: load $load_before -> $load_after" >&2
  done
done
