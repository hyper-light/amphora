#!/usr/bin/env bash
# Lane Q5: each way's answers after K0's mixed phase held to SQLite's on the same rows
# (q5_answers.py), at 1, 32 and 128 clients, one run each.
# usage: BIN=<amphora-bench> TOOLS=<dir with q5_answers.py> WAYS="locked filtered published optimistic" q5_answers.sh
set -uo pipefail
L=~/.cache/analytics-q5 MIXED=${MIXED_SECONDS:-6}
read -r -a WAYS <<< "${WAYS:-locked filtered published optimistic}"
export XDG_CACHE_HOME=$L/xdg
mkdir -p "$L/k0a"; : > "$L/answers.jsonl"
for n in 1 32 128; do
  for way in "${WAYS[@]}"; do
    dir=$L/k0a/$way-$n; rm -rf "$dir"
    echo "=== $way-$n $(date -u +%H:%M:%SZ) load $(cut -d' ' -f1-3 /proc/loadavg)"
    env AMPHORA_LOG_READ_INDEX="$way" AMPHORA_ANSWERS="$dir.answers" nice -n 10 "$BIN" --target amphora --dir "$dir" \
      --sessions 1000 --preload 50 --writers "$n" --readers "$n" --seconds "$MIXED" --warmup 1 --searches 0 > "$dir.out" 2>>"$L/err-answers.log" \
      || { echo "exit $?"; tail -3 "$L/err-answers.log"; }
    python3 -I -c 'import json,sys; r=json.loads(open(sys.argv[1]).read().strip().splitlines()[-1]); print("failures", r.get("failures"), "unanswered", r.get("unanswered"), "reads", (r.get("read_ms") or {}).get("n"))' "$dir.out"
    python3 -I "$TOOLS/q5_answers.py" "$way-$n" "$dir.answers" | tee -a "$L/answers.jsonl"
    rm -rf "$dir" "$dir.answers"
  done
done
python3 -I -c 'import json,sys; a=[json.loads(l) for l in open(sys.argv[1])]; print("answers held to SQLite:", len(a), "runs,", sum(x["sessions"] for x in a), "pages,", sum(x["differ"] for x in a), "differ")' "$L/answers.jsonl"
