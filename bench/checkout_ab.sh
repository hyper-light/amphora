#!/usr/bin/env bash
# perf/writes unit 2 (AUDIT 2, B3's ceiling): the reader checkout, the pool against the slots, in one
# release amphora-bench built from the working tree, switched by AMPHORA_READ_CHECKOUT. Arm order
# alternates each round; the load average is recorded beside every run. Run from the repository root.
set -uo pipefail
export CARGO_HOME=$HOME/.cache/cargo-home-perf-writes CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/writes-bench CARGO_BUILD_JOBS=${JOBS:-16} PATH=$HOME/.cargo/bin:$PATH
mkdir -p "$CARGO_HOME"; for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -s "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
bins=$HOME/.cache/amphora-perf-bin/writes; data=$HOME/.cache/amphora-perf-data/writes-checkout
mkdir -p "$bins" "$data"
[ -e bench/Cargo.lock ] || cp "$HOME/.cache/amphora-perf-src/pagepath-base/bench/Cargo.lock" bench/ 2>/dev/null
locked=; [ -e bench/Cargo.lock ] && locked=--locked
nice -n 10 cargo build --release $locked --manifest-path bench/Cargo.toml 2>&1 | grep -E '^(error|warning: unused)|-->|Finished' | head -30
[ "${PIPESTATUS[0]}" -eq 0 ] || { echo BUILD-FAILED; exit 1; }
bin=$bins/bench-checkout-$(git rev-parse --short HEAD)
cp "$CARGO_TARGET_DIR/release/amphora-bench" "$bin"
out=$bins/checkout-ab-$(date -u +%H%M).jsonl
echo "host=$(hostname) nproc=$(nproc) commit=$(git rev-parse --short HEAD) dirty=$(git status --short | wc -l) out=$out"
args=(--only 'point_pk$,point_secondary$' --clients "${CLIENTS:-1,32,128,1024}" --seconds "${SECS:-5}" --warmup 1 --items 200000)
for rep in $(seq 1 "${ROUNDS:-5}"); do
  if [ $((rep % 2)) = 1 ]; then order="pool slots"; else order="slots pool"; fi
  for arm in $order; do
    load=$(cut -d' ' -f1-3 /proc/loadavg)
    echo "== rep $rep $arm load=$load $(date -u +%T)"
    line=$(AMPHORA_READ_CHECKOUT=$arm nice -n 10 "$bin" --target amphora --dir "$data/amphora" --suite --reuse "${args[@]}" 2>/dev/null | tail -1)
    python3 -c 'import json,sys; r=json.loads(sys.argv[1]); r["arm"]=sys.argv[2]; r["rep"]=int(sys.argv[3]); r["load"]=sys.argv[4]; print(json.dumps(r))' "$line" "$arm" "$rep" "$load" >> "$out" || echo "RUN-FAILED $arm $rep"
  done
done
python3 - "$out" <<'PY'
import json,sys,statistics as st,collections
d=collections.defaultdict(list)
for l in open(sys.argv[1]):
    r=json.loads(l)
    for o in r["results"]:
        sys_share=(o["cpu_sys_s"] or 0)/max((o["cpu_sys_s"] or 0)+(o["cpu_user_s"] or 0),1e-9)
        d[(o["op"],o["clients"],r["arm"])].append((o["ops_per_s"],o["ms"]["p50"],o["ms"]["p99"],o["ms"]["p999"],o["errors"],sys_share))
print("op clients arm | ops/s median [min-max] | p50 p99 p99.9 ms (medians) | errors | sys share median")
for k in sorted(d,key=lambda k:(k[0],k[1],k[2])):
    v=d[k]; ops=[x[0] for x in v]
    m=lambda i: round(st.median([x[i] for x in v]),4)
    print(k[0],k[1],k[2],"|",round(st.median(ops)),f"[{round(min(ops))}-{round(max(ops))}]","|",m(1),m(2),m(3),"|",sum(x[4] for x in v),"|",m(5),"| n",len(v))
PY
echo "records: $out"
