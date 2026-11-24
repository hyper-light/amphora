#!/usr/bin/env bash
set -u; O=$HOME/.cache/amphora-perf/planner-c18-out; G=$HOME/.cache/amphora-perf/planner-c13-remote
export JOBS=${JOBS:-6} RUST_TEST_THREADS=${JOBS:-6}
git -C "$G" fetch -q origin 2>&1 | tail -1
tail -5 $O/red-scopes.log | cut -c1-200
SHA=$(git -C "$G" rev-parse origin/perf/planner-c18)
echo "##### GREEN $SHA"
bash "$O/linux.sh" $SHA build:c18_sql,oracle_findings,cte_scopes,outer_join_subqueries,cte_columns,in_subquery,join_methods,adaptive_joins,on_terms,planner,hash_semi_join,unnesting,top_n,hash_group_by,upsert_or_clause sweeps 2>&1 | grep -E "^==|^error|^warning" -A 5 | cut -c1-220 | grep -v Executable | head -30
bash "$O/linux.sh" - test:oracle_findings 2>&1 | grep -E "^==|^test"
for p in p1 p2 p4 p5 p6; do C18_ONE=default bash "$O/linux.sh" - sql:$p.sql > "$O/$p.out" 2>&1; echo "-- $p"; python3 "$O/compare.py" "$O/$p.out" "$O/$p.sqlite" | cut -c1-400 | tail -12; done
(bash "$O/linux.sh" - "wides18:300:s1 s2 s3 s4 s5 s6" > $O/w18.txt 2>&1; bash "$O/linux.sh" - "wides13:300:w1 w2 w3 w4 w7 w8" > $O/w13.txt 2>&1) &
bash "$O/linux.sh" - test:cte_scopes test:outer_join_subqueries test:cte_columns test:planner test:on_terms test:adaptive_joins test:in_subquery test:join_methods test:hash_semi_join test:unnesting test:top_n test:hash_group_by test:upsert_or_clause 2>&1 | grep -E "^==|^test|differ" | cut -c1-200
wait; grep -hE "^== wide" $O/w18.txt $O/w13.txt; for s in s1 s2 s3 s4 s5 s6; do python3 $O/kinds.py $O/wide18-$s.log 2 | cut -c1-900; done; python3 $O/kinds.py $O/test-cte_scopes.log 2 | cut -c1-900
