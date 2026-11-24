#!/bin/bash
# Lane S9: pull what the Mac made, check the answers, build the tables and README. usage: finalize.sh (no arguments)
set -uo pipefail
R=~/.cache/amphora-s9/rec; D=~/.cache/amphora-s9/data; U=http://192.168.1.82:18573
cd $D || exit 1
for i in 1 2 3 4 5; do curl -s -f -m 40 -o round$i.jsonl $U/round$i.jsonl || { echo "no round$i.jsonl"; rm -f round$i.jsonl; }; done
curl -s -f -m 40 -o bundle.tgz $U/bundle.tgz && tar xzf bundle.tgz && rm -f bundle.tgz
cp -f rounds.log phases.log $D/ 2>/dev/null
python3 -I $R/s9_answers.py $D > $D/answers.txt 2>&1; echo "answers rc=$?" | tee -a $D/answers.txt
cd $R
python3 -I s9_tables.py $D > TABLES.md 2> TABLES.err; echo "tables rc=$? $(wc -l < TABLES.md) lines"; cat TABLES.err
python3 -I s9_compact.py $D > COMPACT.md; 
python3 -I make_head.py $D > HEAD.md && python3 -I make_extra.py $D > EXTRA.md && cat EXTRA.md README-tail.md > TAIL.md
python3 -I make_readme.py $D HEAD.md README-static.md TAIL.md > README.md; echo "readme rc=$? $(wc -c < README.md) bytes"
