#!/usr/bin/env bash
set -u; O=$HOME/.cache/amphora-perf/planner-c18-out; G=$HOME/.cache/amphora-perf/planner-c13-remote
export JOBS=4 RUST_TEST_THREADS=4
git -C "$G" fetch -q origin 2>&1 | tail -1
echo "##### p6 on 3cf88284"; C18_ONE=default bash "$O/linux.sh" - sql:p6.sql > "$O/p6.out" 2>&1; python3 "$O/compare.py" "$O/p6.out" "$O/p6.sqlite" | cut -c1-600
RED=$(git -C "$G" rev-parse origin/diag/planner-c18-red); echo "##### RED $RED"
bash "$O/linux.sh" $RED build:c18_sql,oracle_findings,cte_scopes 2>&1 | grep -E "^==|^error" -A 3 | cut -c1-220 | head
bash "$O/linux.sh" - test:oracle_findings test:cte_scopes 2>&1 | grep -E "^==|^test result|differ" | cut -c1-220
cp $O/test-oracle_findings.log $O/red-oracle.log; cp $O/test-cte_scopes.log $O/red-scopes.log
grep -E "^test .*FAILED" $O/red-oracle.log; grep -E "^  left|^ right|every setting|default: " $O/red-oracle.log | head -12 | cut -c1-300
grep -E "answers differ" $O/red-scopes.log | cut -c1-100
echo "red: panics $(grep -c '^    Amphora PANIC' $O/red-scopes.log) errors $(grep -c '^    Amphora error' $O/red-scopes.log) rows $(grep -cE '^    Amphora [0-9]+:' $O/red-scopes.log)"
python3 $O/kinds.py $O/red-scopes.log 0 | head -40
for p in p5 p6; do C18_ONE=default bash "$O/linux.sh" - sql:$p.sql > "$O/$p.red" 2>&1; echo "red $p:"; python3 "$O/compare.py" "$O/$p.red" "$O/$p.sqlite" | cut -c1-500 | tail -14; done
