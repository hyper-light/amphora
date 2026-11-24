#!/bin/bash
# Lane S9: B with its graduated defaults put back to A's one switch at a time (and all of them, Bd), one statement list.
# usage: ablate.sh OPS CLIENTS REPEATS   (OPS as --only takes them)
S9=~/.cache/amphora-s9; export XDG_CACHE_HOME=$S9/xdg; ulimit -n 61440 2>/dev/null; O=$S9/out/ablate.jsonl
OPS=$1 CL=$2 N=${3:-2}
run() { local label=$1 bin=b; shift; [ "$1" = BIN=a ] && { bin=a; shift; }; for i in $(seq 1 $N); do
  env "$@" $S9/bin/amphora-bench-$bin --target amphora --dir $S9/profdata/ablate --suite --only "$OPS" --items 200000 --clients $CL --seconds 1 --warmup 0.5 2>/dev/null | tail -1 | python3 -c 'import json,sys; r=json.loads(sys.stdin.read()); print(json.dumps({"label": sys.argv[1], "repeat": int(sys.argv[2]), "results": [[x["op"], x["clients"], x["ops_per_s"], (x["ms"] or {}).get("p50"), x["errors"]] for x in r["results"]]}))' "$label" $i >> $O; done; }
mkdir -p $S9/profdata/ablate
run default X=1
run A_default BIN=a X=1
run A_shared+slots BIN=a AMPHORA_SHARED_PAGES=process AMPHORA_READ_CHECKOUT=slots
run shared=0 AMPHORA_SHARED_PAGES=0
run page_path=today AMPHORA_PAGE_PATH=today
run checkout=pool AMPHORA_READ_CHECKOUT=pool
run reader_pages=2000 AMPHORA_READER_PAGES=2000
run writer_pages=1024 AMPHORA_WRITER_PAGES=1024
run all_old AMPHORA_SHARED_PAGES=0 AMPHORA_PAGE_PATH=today AMPHORA_READ_CHECKOUT=pool AMPHORA_WRITER_PAGES=1024
cat $O | python3 -c '
import json,sys,statistics as s
d={}
for l in sys.stdin:
    r=json.loads(l)
    for op,c,o,p,e in r["results"]: d.setdefault((op,c),{}).setdefault(r["label"],[]).append(o)
for k,v in d.items(): print(k, {a:[round(x) for x in b] for a,b in v.items()})'
