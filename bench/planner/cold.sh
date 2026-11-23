#!/usr/bin/env bash
# PL.13 (lane replan-9): each query's cold run on every engine. For each query named, PostgreSQL is
# restarted (its shared buffers empty), the operating system's page cache dropped where this host
# lets it be, and a process of its own times the query once on each engine as it opened it
# (`amphora-bench --planner --phase cold`): Amphora with no correction learned, nothing run before
# it. The driver's record says whether the cache was dropped (`os_cache_dropped`) and what was
# cached as the run began (`page_cache_kib`).
#
# usage: bench/planner/cold.sh <query,query,...> <the driver's command and arguments, without
#        --only or --phase>
set -euo pipefail
Q=$1
shift
here=$(cd "$(dirname "$0")" && pwd)
# Linux's page cache, by whichever way this host allows: written as root, by sudo without a
# password, or from a privileged container (the host's kernel: the cache is not a container's).
drop_cache() {
  sync
  if [[ -w /proc/sys/vm/drop_caches ]]; then
    echo 3 >/proc/sys/vm/drop_caches
  elif sudo -n true 2>/dev/null; then
    sudo -n sh -c 'echo 3 > /proc/sys/vm/drop_caches'
  else
    docker run --rm --privileged "${DROP_IMAGE:-busybox}" sh -c 'echo 3 > /proc/sys/vm/drop_caches' 2>/dev/null
  fi
}
for q in ${Q//,/ }; do
  bash "$here/postgres.sh" restart
  dropped=()
  if drop_cache; then
    dropped=(--os-cache-dropped)
  else
    echo "cold.sh: the page cache could not be dropped before $q" >&2
  fi
  "$@" --phase cold --only "$q" ${dropped[@]+"${dropped[@]}"}
done
