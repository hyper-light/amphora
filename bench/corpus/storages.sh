#!/usr/bin/env bash
# The corpus on both storages (goal "Many clients write through SQL at once, safely": the whole
# corpus gives the same answers on both storage engines, reads as fast, writes 3 times faster):
# `amphora-corpus run` on the pager's B-trees, then with AMPHORA_STORAGE=versioned, each twice on
# the same machine: in memory (the gate's database: the answers' comparison), and on a file on the
# disk under test (`--disk`: WAL and `synchronous = FULL`, the commit bench's durability, so a
# commit returns once it is on the disk: the times' comparison, T-T1.4.1). Then storages.py's
# verdicts: answers (in memory), answers on disk, reads and writes (on the disk), and a line for
# each record whose answer differs. No budget unless one is passed: a budget's fill files are what
# the machine reached, which two runs needn't share.
#
# usage: bench/corpus/storages.sh <amphora-corpus binary> <pool dir> <out dir> <dir on the disk under test> [run flags...]
#   e.g. bench/corpus/storages.sh target/release/amphora-corpus bench/corpus/subset ~/.cache/storages /mnt/nvme/storages
# The disk's runs keep one file's database at a time there (amphora-corpus removes each as the
# next file opens its own), in a folder of their own removed at the end.
# Exits 0 when the answers are the same, 1 when they differ or a record of the pool was left out
# (storages.py counts the pool's records before the runs), 2 when the pool holds none, a run itself
# failed, or the runs name another client or platform (storages.py refuses to compare them).
set -uo pipefail
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
bin=$(realpath -- "$1")
pool=$(realpath -- "$2")
out=$3
mkdir -p "$4"
disk=$(cd -- "$4" && pwd)/storages.$$
shift 4
case "$(uname -s)" in
  Linux) fs=$(findmnt -no FSTYPE -T "$(dirname "$disk")") dev=$(findmnt -no SOURCE -T "$(dirname "$disk")") ;;
  Darwin)
    dev=$(df -P "$(dirname "$disk")" | awk 'NR==2{print $1}')
    fs=$(mount | awk -v d="$dev" '$1==d{gsub(/[(,]/,"",$4); print $4; exit}')
    ;;
  *) fs=unknown dev=unknown ;;
esac
trap 'rm -rf "$disk"' EXIT
mkdir -p "$out"
# The pool's own records, counted before the runs: the verdict's SAME needs both storages to reach
# every one (T-T1.4.1), and a pool of none is refused here, not after four runs.
counted=$(python3 -I "$here/storages.py" count "$pool") || { echo "$counted"; exit 2; }
{
  echo "host $(uname -n), $(uname -srm), binary $bin, pool $pool, flags: $*"
  echo "disk under test: $disk, file system $fs, device $dev"
  echo "$counted"
} > "$out/platform.txt"
for where in memory disk; do
  for storage in pager versioned; do
    run="$out/$where/$storage"
    mkdir -p "$run"
    at=()
    if [ "$where" = disk ]; then
      rm -rf "$disk"
      mkdir -p "$disk"
      at=(--disk "$disk")
    fi
    if [ "$storage" = versioned ]; then
      env AMPHORA_STORAGE=versioned "$bin" run "$pool" --times "$run/times.jsonl" ${at[@]+"${at[@]}"} "$@" > "$run/out.txt"
    else
      env -u AMPHORA_STORAGE "$bin" run "$pool" --times "$run/times.jsonl" ${at[@]+"${at[@]}"} "$@" > "$run/out.txt"
    fi
    rc=$?
    echo "$where $storage: exit $rc: $(tail -n 1 "$run/out.txt")"
    # 1 is a disagreement with SQLite, which the comparison reads; 2 and 3: the run didn't happen.
    if [ "$rc" -ge 2 ]; then
      exit 2
    fi
  done
done
cat "$out/platform.txt"
python3 -I "$here/storages.py" "$pool" "$out/memory/pager" "$out/memory/versioned" "$out/disk/pager" "$out/disk/versioned" | tee "$out/verdict.txt"
exit "${PIPESTATUS[0]}"
