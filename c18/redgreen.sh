#!/usr/bin/env bash
# red (main's engine with lane C18's tests), then green (the branch)
set -u; O=$HOME/.cache/amphora-perf/planner-c18-out; G=$HOME/.cache/amphora-perf/planner-c13-remote
export JOBS=4 RUST_TEST_THREADS=4
git -C "$G" fetch -q origin 2>&1 | tail -1
SHA=$(git -C "$G" rev-parse origin/perf/planner-c18)
echo "##### GREEN $SHA"
bash "$O/linux.sh" $SHA build:c18_sql,oracle_findings,cte_scopes,outer_join_subqueries,cte_columns sweeps 2>&1 | grep -E "^==|^error" -A 3 | cut -c1-220 | head -30
bash "$O/linux.sh" - test:oracle_findings 2>&1 | grep -E "^==|^test"
for p in p4 p5; do C18_ONE=default bash "$O/linux.sh" - sql:$p.sql > "$O/$p.out" 2>&1; python3 "$O/compare.py" "$O/$p.out" "$O/$p.sqlite" | cut -c1-700; done
bash "$O/linux.sh" - test:cte_scopes 2>&1 | grep -E "^==|^test|differ" | cut -c1-250
python3 $O/kinds.py $O/test-cte_scopes.log 2 | cut -c1-900
