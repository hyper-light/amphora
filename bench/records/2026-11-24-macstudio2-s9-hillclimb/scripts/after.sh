#!/bin/bash
# Lane S9: after round 5: the switch ablation on B, three profiles, many connections, then the bundle of what isn't a round file.
S9=~/.cache/amphora-s9; O=$S9/out; mkdir -p $O/prof $O/scripts
until grep -E '"round": 5, .*"label": "end"' $O/rounds.log >/dev/null 2>&1; do sleep 1; done
sleep 2; date -u; echo "== ablation"
$S9/scripts/ablate.sh 'point_pk$,table_count$' 1,32 1 > $O/ablate.txt 2>&1; tail -8 $O/ablate.txt
date -u; echo "== profiles"
$S9/scripts/profile.sh b B table_count 1
$S9/scripts/profile.sh b B table_count 32
$S9/scripts/profile.sh b B aggregates_range 1
for f in $O/prof/*.json.gz; do python3 $S9/scripts/samply_top.py $f '' 22 > ${f%.json.gz}.txt 2>&1; done
date -u; echo "== many connections"
python3 $S9/scripts/many.py $O/many.jsonl A,B > $O/many.txt 2>&1; cat $O/many.txt
cp $S9/scripts/* $O/scripts/; cp $S9/log/Cargo.lock-* $S9/log/build-*.log $O/ 2>/dev/null
cd $O && tar czf bundle.tgz --exclude 'round*.jsonl' --exclude 'bundle.tgz' --exclude 'prof/*.json.gz' --exclude '*.syms.json' . && ls -la bundle.tgz
date -u; echo AFTER-DONE
