#!/usr/bin/env bash
# The corpus on both storages (goal "Many clients write through SQL at once, safely": the whole
# corpus gives the same answers on both storage engines, reads as fast, writes 3 times faster):
# `amphora-corpus run` on the pager's B-trees, then with AMPHORA_STORAGE=versioned, on the same
# machine, then storages.py's verdicts (one line each for answers, reads and writes; a line for
# each record whose answer differs). No budget unless one is passed: a budget's fill files are what
# the machine reached, which two runs needn't share.
#
# usage: bench/corpus/storages.sh <amphora-corpus binary> <pool dir> <out dir> [run flags...]
#   e.g. bench/corpus/storages.sh target/release/amphora-corpus bench/corpus/subset ~/.cache/storages
# Exits 0 when the answers are the same, 1 when they differ, 2 when a run itself failed.
set -uo pipefail
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
bin=$(realpath -- "$1")
pool=$(realpath -- "$2")
out=$3
shift 3
mkdir -p "$out/pager" "$out/versioned"
{
  echo "host $(uname -n), $(uname -srm), binary $bin, pool $pool, flags: $*"
} > "$out/platform.txt"
for storage in pager versioned; do
  if [ "$storage" = versioned ]; then
    env AMPHORA_STORAGE=versioned "$bin" run "$pool" --times "$out/$storage/times.jsonl" "$@" > "$out/$storage/out.txt"
  else
    env -u AMPHORA_STORAGE "$bin" run "$pool" --times "$out/$storage/times.jsonl" "$@" > "$out/$storage/out.txt"
  fi
  rc=$?
  echo "$storage: exit $rc: $(tail -n 1 "$out/$storage/out.txt")"
  # 1 is a disagreement with SQLite, which the comparison reads; 2 and 3: the run didn't happen.
  if [ "$rc" -ge 2 ]; then
    exit 2
  fi
done
cat "$out/platform.txt"
python3 -I "$here/storages.py" "$pool" "$out/pager" "$out/versioned" | tee "$out/verdict.txt"
exit "${PIPESTATUS[0]}"
