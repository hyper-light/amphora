#!/bin/bash
# Lane S9 smoke: calibration stored per arm, every arm's plans (--explain), and what each engine's commits call
# (fsync, F_FULLFSYNC, F_BARRIERFSYNC), counted by libs9sync.dylib. Nothing here is a timed round.
S9=~/.cache/amphora-s9; O=$S9/out; mkdir -p $O $S9/smoke; export XDG_CACHE_HOME=${SMOKE_XDG:-$S9/xdg}
ulimit -n 61440 || ulimit -n 10240
ONLY='point_pk$,point_secondary$,point_text$,json_field$,blob_read$,insert_one$,insert_batch_100$,update_plain$,update_indexed$,upsert$,delete_insert$,txn_read_write$,table_count$,table_group_by$,filter_no_index$,aggregates_range$,join_three$'
OLD="AMPHORA_SHARED_PAGES=0 AMPHORA_READ_CHECKOUT=pool AMPHORA_PAGE_PATH=today AMPHORA_WRITER_PAGES=1024"
date -u
for spec in "A a amphora" "B b amphora" "S a sqlite" "T a turso" "Bd b amphora"; do set -- $spec
  e=""; [ $1 = Bd ] && e=$OLD
  t0=$(date +%s)
  env $e $S9/bin/amphora-bench-$2 --target $3 --dir $S9/smoke/$1 --suite --explain --only "$ONLY" --items 200000 --clients 1 --seconds 0.1 --warmup 0.05 > $O/explain-$1.json 2> $O/explain-$1.err
  echo "explain $1 rc=$? $(( $(date +%s) - t0 ))s bytes $(wc -c < $O/explain-$1.json)"
done
for spec in "A a amphora" "B b amphora" "S a sqlite" "T a turso"; do set -- $spec
  for mode in count full; do
    f=0; [ $mode = full ] && f=1
    out=$(DYLD_INSERT_LIBRARIES=$S9/bin/libs9sync.dylib S9_FULLFSYNC=$f $S9/bin/amphora-bench-$2 --target $3 --dir $S9/smoke/sync-$1 --suite --only 'insert_one$' --items 2000 --clients 1 --seconds 0.5 --warmup 0.1 2>$O/sync-$1-$mode.err)
    echo "sync $1 $mode ops/s $(echo "$out" | tail -1 | python3 -c 'import json,sys; r=json.loads(sys.stdin.read())["results"][0]; print(r["ops_per_s"], "p50", r["ms"] and r["ms"]["p50"], "errors", r["errors"])' 2>&1) | $(grep s9sync $O/sync-$1-$mode.err)"
  done
done
ls -la $S9/xdg/amphora/calibration 2>/dev/null; for f in $S9/xdg/amphora/calibration/*; do echo "--- $f"; cat "$f"; done
date -u
