#!/usr/bin/env bash
# Lane S7 (bench): after the rounds. usage: finish.sh analyze | commit "MESSAGE"
# analyze: the tables over the complete rounds and S4's records; commit: the data files to main (records only),
# from a worktree at origin/main, with the answers (untimed read rows) stripped from the raw round files.
set -uo pipefail
SHA=0101aa386850d1e8e641c6254238567adf9d8aa7
L=~/.cache/amphora-perf/bench-s7-run; O=$L/out
S=bench/records/2026-11-24-adacachy2-bench-s7-suite
SRC=~/.cache/amphora-perf/bench-s
cd "$SRC" || exit 1
if [ "${1:-}" = analyze ]; then
  git fetch -q origin perf/bench-s7b || exit 1
  REV=$(git rev-parse FETCH_HEAD)
  for f in s7_tables.py phases_summary.py verdict.py finish.sh; do git show $REV:$S/$f >"$L/scripts/$f" || exit 1; done
  echo "scripts at branch commit $REV"
  python3 -I "$L/scripts/phases_summary.py" "$O" >"$O/PHASES.md" 2>&1
  python3 -I "$L/scripts/s7_tables.py" "$O" "$SRC/bench/records/2026-11-23-adacachy2-suite-inproc/records.jsonl" >"$O/TABLES.md" 2>"$O/TABLES.err"
  echo "tables rc=$? lines=$(wc -l <"$O/TABLES.md")"; cat "$O/TABLES.err"
  exit 0
fi
[ "${1:-}" = commit ] || { echo "usage: finish.sh analyze|commit MESSAGE"; exit 2; }
msg=$2
W=$L/commit-wt
[ -d "$W" ] || git worktree add -q --detach "$W" origin/main || exit 1
for i in 1 2 3 4 5; do
  git -C "$W" fetch -q origin && git -C "$W" checkout -q -f --detach origin/main || continue
  mkdir -p "$W/$S"
  for f in rounds.txt phases.log PHASES.md TABLES.md smoke-answers.txt smoke.err sha.txt; do [ -f "$O/$f" ] && cp "$O/$f" "$W/$S/"; done
  # Process command lines as ps printed them, with anything that looks like a secret cut out.
  sed -E 's/(token|secret|password|passwd|api[_-]?key|bearer)[^ ]*/[redacted]/Ig' "$O/procs.txt" >"$W/$S/procs.txt"
  cp "$O"/round*-answers.txt "$O"/round*.err "$W/$S/" 2>/dev/null
  # The raw rounds and the smoke pass, without the answers (compared untimed, value for value, by suite_answers.py).
  python3 -I - "$O" "$W/$S" <<'PY'
import json, os, sys, glob
src, dst = sys.argv[1], sys.argv[2]
for p in sorted(glob.glob(os.path.join(src, "round[0-9]*.jsonl"))) + [os.path.join(src, "smoke.jsonl")]:
    if not os.path.exists(p):
        continue
    with open(os.path.join(dst, os.path.basename(p)), "w") as out:
        for line in open(p):
            try:
                r = json.loads(line)
            except ValueError:
                continue
            rep = r.get("report")
            if isinstance(rep, dict):
                n = len(rep.pop("answers", None) or [])
                rep["answers_stripped"] = n
            out.write(json.dumps(r) + "\n")
PY
  cp "$L/bench-cargo-lock.diff" "$W/$S/" 2>/dev/null
  tail -5 "$L/build.log" >"$W/$S/build.log.tail"
  git -C "$SRC" fetch -q origin perf/bench-s7b
  for f in record-head.md record-tail.md load-template.md judgment.md record_assemble.py; do git -C "$SRC" show FETCH_HEAD:$S/$f >"$L/scripts/$f" || { echo "no $f at FETCH_HEAD"; exit 1; }; done
  python3 -I "$L/scripts/record_assemble.py" "$O" "$L/scripts" "$(date -u +%FT%TZ)" "$W/$S.md" || exit 1
  (cd "$L/scripts" && cp rounds.sh smoke_summary.py s7_tables.py phases_summary.py verdict.py record_assemble.py finish.sh "$W/$S/")
  git -C "$W" add -- "$S" "$S.md" && git -C "$W" diff --cached --quiet && { echo "nothing to commit"; exit 0; }
  git -C "$W" commit -q -m "$msg

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" || exit 1
  git -C "$W" push -q origin HEAD:main && { git -C "$W" log --oneline -1; git -C "$W" show --stat HEAD | tail -8; exit 0; }
  echo "push attempt $i failed"
done
exit 1
