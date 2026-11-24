#!/bin/bash
# Lane S9: waits for arm C's build, then the timed rounds. Orders: a Williams design on A,B,C,S,T,Bd (every neighbour pair
# at most once); Sf and Tf (S and T with F_FULLFSYNC, writes only) follow S and T in rounds 1-3; Bd in rounds 1-3.
S9=~/.cache/amphora-s9
until grep -q 'arm c end' $S9/log/build-c2.log 2>/dev/null; do grep -q '^error' $S9/log/build-c2.log 2>/dev/null && { echo "C build failed"; exit 1; }; sleep 0.5; done
cp $S9/log/build-c2.log $S9/out/; cp $S9/log/build-?.log $S9/out/
sleep 3
date -u; ls -la $S9/bin
cd $S9 && exec python3 $S9/scripts/rounds.py $S9/out 1 5 "A,B,Bd,C,T,Tf,S,Sf/B,C,A,S,Sf,Bd,T,Tf/C,S,Sf,B,T,Tf,A,Bd/S,T,C,B,A/T,S,A,C,B"
