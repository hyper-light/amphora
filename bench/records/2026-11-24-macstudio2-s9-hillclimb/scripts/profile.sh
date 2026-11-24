#!/bin/bash
# Lane S9: a samply profile of one statement on one arm's binary, on the database a short load left (--reuse), so the
# profile holds the statement's run. usage: profile.sh BIN LABEL OP CLIENTS [NAME=VALUE...]
S9=~/.cache/amphora-s9; O=$S9/out/prof; mkdir -p $O $S9/profdata/$2; export XDG_CACHE_HOME=$S9/xdg
ulimit -n 61440 2>/dev/null
B=$S9/bin/amphora-bench-$1; L=$2 OP=$3 C=$4; shift 4
env "$@" $B --target amphora --dir $S9/profdata/$L --suite --only "$OP\$" --items 200000 --clients 1 --seconds 0.1 --warmup 0.05 >/dev/null 2>$O/$L-load.err
env "$@" $S9/tools/bin/samply record --save-only --unstable-presymbolicate -r 2000 -o $O/$L-$OP-$C.json.gz -- $B --target amphora --dir $S9/profdata/$L --suite --reuse --only "$OP\$" --items 200000 --clients $C --seconds 4 --warmup 0.5 > $O/$L-$OP-$C.out 2> $O/$L-$OP-$C.err
echo "$L $OP $C rc=$? $(ls -la $O/$L-$OP-$C.json.gz 2>/dev/null | awk '{print $5}') bytes; ops/s $(python3 -c 'import json,sys; r=json.loads(open(sys.argv[1]).read().strip().splitlines()[-1])["results"][0]; print(r["ops_per_s"], r["ms"])' $O/$L-$OP-$C.out 2>&1)"
