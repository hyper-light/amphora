#!/usr/bin/env bash
# Pushes the lane's scripts to origin's perf/tpch-t22-scripts (an orphan-like commit on top of the branch's last), so
# adacachy2 gets them with `git fetch`.
set -euo pipefail
S=~/.cache/amphora-perf/tpch-t22
cd ~/.cache/amphora-perf/tpch
export GIT_INDEX_FILE=$(mktemp -u)
for f in "$S"/*.sh "$S"/*.py "$S"/q10.sql; do
  [ -f "$f" ] || continue
  h=$(git hash-object -w "$f")
  git update-index --add --cacheinfo 100755,"$h","t22/$(basename "$f")"
done
t=$(git write-tree)
p=$(git ls-remote origin refs/heads/perf/tpch-t22-scripts | cut -f1)
if [ -n "$p" ]; then git fetch -q origin "$p"; c=$(git commit-tree "$t" -p "$p" -m "lane T22's scripts"); else c=$(git commit-tree "$t" -m "lane T22's scripts"); fi
rm -f "$GIT_INDEX_FILE"; unset GIT_INDEX_FILE
git push -q origin "$c:refs/heads/perf/tpch-t22-scripts" && echo "$c"
