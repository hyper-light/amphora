#!/usr/bin/env bash
# amphora-bench at one commit against another (kiln goal ab3aedaf: each benchmark recorded against the commit
# before it). Each commit's bench is built in release in its own worktree and target directory
# (bench/ab_build.sh), then each round runs the given amphora-bench arguments at both commits back to back, the
# order turned each round (ABBA), each run its own process on a fresh --dir. Each JSON line a run prints goes to
# $AB_OUT with the commit, its tree, the arm, the round, the host, the load at the run's start, the bytes left in
# --dir after it (all files, and the log files: names holding "wal" or "log"), and, where the host counts them, the
# process's instructions retired (perf stat on Linux, /usr/bin/time -l on macOS). The bench's own CPU-time columns
# (Linux schedstat) come with its rows. A table of the best of the rounds follows (bench/commits_ab.py).
#
# usage: bench/commits_ab.sh <before> <after> <rounds> <amphora-bench arguments, without --dir...>
#   finding 4's Linux leg with CPU time (kiln goal d38fd52c step 3; bench/records/2026-11-12-finding4-hot-loop.md):
#     bench/commits_ab.sh a005407 839cd0e 3 --sql-api --shapes reapi,logsearch,insert,parse --runs 1
#   bulk statements and the log they leave, against the commit before (goal 60805ec4, store.md decisions 37, 39, 44):
#     bench/commits_ab.sh 13e385da 3e91d702 3 --suite --target amphora --only insert_batch_100,delete_range_insert \
#       --items 200000 --clients 1,32 --seconds 5 --warmup 1   (3e91d702: the small commits; 13e385da the base they were measured on)
#   the same against the replica log's bound (goal 7582f00a: 7cfadaf3, the batch before log-bound, against 6c343067, its last merge)
# amphora-bench must build at both commits: on main 0ace0e0d it does not (bench/src/columns.rs:174, Feed::new's arguments).
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
before="$1"; after="$2"; rounds="$3"; shift 3
source bench/ab_build.sh
out="${AB_OUT:-$AB_WORK/commits-ab-$(date -u +%Y%m%dT%H%M%SZ).jsonl}"
data="$AB_WORK/data-$$"
mkdir -p "$data" "$(dirname -- "$out")"
trap 'rm -rf -- "${data:?}"' EXIT
build() { # commit -> prints its amphora-bench
  local dir target
  dir=$(ab_worktree "$1")
  target="$AB_WORK/target-$(ab_tree "$1")"
  (cd "$dir/bench" && CARGO_TARGET_DIR="$target" cargo build --release --bin amphora-bench >&2) || return 1
  echo "$target/release/amphora-bench"
}
bin_before=$(build "$before"); bin_after=$(build "$after")
bytes() { # dir [find tests...] -> bytes of its files that pass the tests
  local dir="$1"
  shift
  find "$dir" -type f "$@" -exec wc -c {} + 2>/dev/null | awk '$2 != "total" { n += $1 } END { print n + 0 }'
}
one() { # arm commit bin round
  local arm="$1" commit="$2" bin="$3" round="$4" counts="$data/counts" lines instr load disk logs
  shift 4
  rm -rf -- "${data:?}/db"
  mkdir -p "$data/db"
  load=$(ab_load)
  if [[ "$(uname -s)" == Darwin ]]; then
    lines=$(/usr/bin/time -l -o "$counts" "$bin" "$@" --dir "$data/db")
    instr=$(awk '/instructions retired/ { print $1 }' "$counts")
  elif command -v perf >/dev/null; then
    lines=$(perf stat -x, -e instructions:u -o "$counts" "$bin" "$@" --dir "$data/db")
    instr=$(awk -F, '/instructions/ { print $1 }' "$counts")
  else
    lines=$("$bin" "$@" --dir "$data/db")
    instr=""
  fi
  disk=$(bytes "$data/db"); logs=$(bytes "$data/db" \( -name '*wal*' -o -name '*log*' \))
  printf '%s\n' "$lines" | python3 -c 'import json, sys
a = sys.argv[1:]
for line in sys.stdin:
    line = line.strip()
    if not line.startswith("{"):
        continue
    r = json.loads(line)
    r.update(arm=a[0], commit=a[1], tree=a[2], round=int(a[3]), host=a[4], load_start=a[5],
             disk_bytes_after=int(a[6]), log_bytes_after=int(a[7]), instructions=int(a[8]) if a[8].isdigit() else None)
    print(json.dumps(r))' "$arm" "$(git rev-parse "$commit")" "$(ab_tree "$commit")" "$round" "$(hostname -s)" "$load" "$disk" "$logs" "${instr:-}" >>"$out"
}
for round in $(seq 1 "$rounds"); do
  if ((round % 2)); then order="before after"; else order="after before"; fi
  for arm in $order; do
    echo "== round $round $arm $(date -u +%T) load $(ab_load)" >&2
    if [[ $arm == before ]]; then one before "$before" "$bin_before" "$round" "$@"; else one after "$after" "$bin_after" "$round" "$@"; fi
  done
done
python3 bench/commits_ab.py "$out"
echo "records: $out" >&2
