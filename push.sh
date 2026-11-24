#!/usr/bin/env bash
# ship lane C25's scripts through the branch diag/dml-c25
D=~/.cache/amphora-perf/dml-c25-diag/repo
cp -r ~/.cache/amphora-perf/dml-c25-scripts/. "$D"/ && git -C "$D" add -A && git -C "$D" commit -q -m "lane C25's scripts (not for main)" && git -C "$D" push -q -f origin diag/dml-c25 && git -C "$D" log --oneline | head -1
