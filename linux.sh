#!/usr/bin/env bash
# Lane C27's Linux driver (not for main). usage: linux.sh SHA|- STEP...
set -uo pipefail
SHA=$1; shift
export CARGO_HOME=$HOME/.cache/cargo-home-perf-oracle-c27 CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-8}
export CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/oracle-c27
O=$HOME/.cache/amphora-perf/oracle-c27-out; R=$HOME/.cache/amphora-perf/oracle-c27-remote; S=$HOME/.cache/amphora-perf/oracle-c27-scripts
LIB=$HOME/.cache/sqlite-oracle/libsqlite3-oracle-3540000.so
mkdir -p "$O" "$CARGO_HOME" "$CARGO_TARGET_DIR" "$HOME/.cache/amphora-perf-bin/oracle-c27"
for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -s "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
for f in config.toml config; do [ -e "$HOME/.cargo/$f" ] && [ ! -e "$CARGO_HOME/$f" ] && ln -s "$HOME/.cargo/$f" "$CARGO_HOME/$f"; done
export PATH=$CARGO_HOME/bin:$PATH
load() { cut -d' ' -f1-3 /proc/loadavg; }
[ -d "$R/.git" ] || git clone -q --reference "$HOME/.cache/amphora-perf/vdbe-c24-remote" --dissociate git@github.com:hyper-light/amphora.git "$R" || exit 2
cd "$R" || exit 2
git fetch -q origin 2>&1 | tail -2
rm -rf "${S:?}.new"; mkdir -p "$S.new"; git archive origin/diag/oracle-c27 | tar -x -C "$S.new" && rm -rf "${S:?}" && mv "$S.new" "$S"
if [ "$SHA" != - ]; then git checkout -q -f --detach "$SHA" || exit 3; fi
TREE=$(git rev-parse --short=8 HEAD)
BIN=${BIN:-$HOME/.cache/amphora-perf-bin/oracle-c27/$TREE/oracle-lite}
echo "== $(hostname) $(date -u +%T) tree $TREE load $(load)"
for step in "$@"; do
  s=$(date +%s)
  case $step in
    build)
      mkdir -p bench-lite/src; cp "$S/bench-lite/Cargo.toml" "$S/bench-lite/Cargo.lock" bench-lite/; cp "$S/bench-lite/src/main.rs" bench-lite/src/
      ( cd bench-lite && timeout 1200 nice -n 10 cargo build --release > "$O/build.log" 2>&1 ); rc=$?
      grep -E "^(error|warning)|Compiling amphora|Finished" -A 8 "$O/build.log" | tail -${BUILD_LINES:-60}
      echo "== build rc=$rc in $(( $(date +%s) - s ))s"; [ $rc -ne 0 ] && exit $rc
      mkdir -p "$(dirname "$BIN")"; cp "$CARGO_TARGET_DIR/release/oracle-lite" "$BIN"; ls -la "$BIN"; sha256sum "$BIN" | cut -c1-16 ;;
    campaign:*)  # campaign:SECONDS:SEED=ORACLES[+SEED=ORACLES..]
      IFS=: read -r _ secs runs <<< "$step"
      pids=""
      for one in ${runs//+/ }; do
        seed=${one%%=*}; ors=${one#*=}; D=$O/main-$seed; rm -rf "${D:?}"; mkdir -p "$D"
        timeout $((secs+140)) nice -n 10 "$BIN" --oracle --oracles "$ors" --seconds "$secs" --seed "$seed" --reduce-seconds 3 --features "$R/bench/corpus/features.json" --sqlite "$LIB" --label "c27-$TREE-$seed" --out "$D" > "$D/run.log" 2>&1 &
        pids="$pids $!:$seed"
      done
      for p in $pids; do wait "${p%%:*}"; echo "== seed ${p##*:} rc=$? $(tail -1 "$O/main-${p##*:}/run.log" | cut -c1-200)"; done
      echo "== campaign $(( $(date +%s) - s ))s load $(load)" ;;
    sh:*) bash -c "${step#sh:}" ;;
  esac
done
echo "== done $(date -u +%T) load $(load)"
