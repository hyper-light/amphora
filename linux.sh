#!/usr/bin/env bash
# Lane C27's Linux driver (not for main). usage: linux.sh SHA|- STEP...
set -uo pipefail
SHA=$1; shift
export CARGO_HOME=$HOME/.cache/cargo-home-perf-oracle-c27 CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-8}
export CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/oracle-c27
O=$HOME/.cache/amphora-perf/oracle-c27-out; R=$HOME/.cache/amphora-perf/${CLONE:-oracle-c27-remote}; S=$HOME/.cache/amphora-perf/oracle-c27-scripts
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
    seedfrom:*)  # a private copy of a finished build folder's debug half: its third-party crates, not a fresh build
      if [ ! -d "$CARGO_TARGET_DIR/debug" ]; then cp -a --reflink=always "$HOME/.cache/amphora-perf-target/${step#seedfrom:}/debug" "$CARGO_TARGET_DIR/"; echo "== seeded rc=$? $(( $(date +%s) - s ))s"; fi ;;
    tbuild:*)
      tests=""; IFS=, read -ra names <<< "${step#tbuild:}"; for n in "${names[@]}"; do tests="$tests --test $n"; done
      timeout 1200 nice -n 10 cargo test -p amphora-engine --locked $tests --no-run > "$O/tbuild.log" 2>&1; rc=$?
      grep -E "^(error|warning)|Executable|Compiling amphora|Finished" -A 8 "$O/tbuild.log" | head -${BUILD_LINES:-80}
      echo "== tbuild rc=$rc in $(( $(date +%s) - s ))s"; [ $rc -ne 0 ] && exit 4 ;;
    test:*)  # test:NAME[:FILTER]
      IFS=: read -r _ name filter <<< "$step"; b=$(ls -t "$CARGO_TARGET_DIR"/debug/deps/$name-* | grep -v '\.d$' | head -1)
      timeout 900 nice -n 10 "$b" $filter --test-threads=${THREADS:-8} > "$O/test-$name.log" 2>&1; rc=$?
      echo "== test $name $filter rc=$rc $(( $(date +%s) - s ))s"; grep -aE "test result|panicked|answers differ|FAILED|failed$" "$O/test-$name.log" | cut -c1-300 | head -${LINES_OUT:-14} ;;
    cat:*) sed -n 1,${CAT_LINES:-80}p "$O/${step#cat:}" | cut -c1-${CUT:-700} ;;
    red:*)  # red:REF:FILE[,FILE..]: these files as REF has them
      IFS=: read -r _ ref files <<< "$step"; git checkout -q -f "$ref" -- ${files//,/ }; echo "== red: $files as $(git rev-parse --short=8 $ref)"; git status --short | head -5 ;;
    green) git checkout -q -f "$TREE" -- . && git status --short | head -5; echo "== green" ;;
    fmt) cargo fmt --all --check > "$O/fmt.log" 2>&1; echo "== fmt rc=$?"; head -40 "$O/fmt.log" ;;
    clippy)
      timeout 1500 nice -n 10 cargo clippy -p amphora-engine --all-targets --locked -- -D warnings > "$O/clippy.log" 2>&1; rc=$?
      echo "== clippy rc=$rc $(( $(date +%s) - s ))s"; grep -E "Checking amphora-engine|^(error|warning)" -A 12 "$O/clippy.log" | head -80 ;;
    prod)
      timeout 1500 nice -n 10 bash scripts/check-production.sh > "$O/prod.log" 2>&1; rc=$?
      echo "== check-production rc=$rc $(( $(date +%s) - s ))s"; tail -8 "$O/prod.log" | cut -c1-400 ;;
    benchclippy)
      timeout 1500 nice -n 10 cargo clippy --manifest-path bench/Cargo.toml --all-targets -- -D warnings > "$O/benchclippy.log" 2>&1; rc=$?
      echo "== bench clippy rc=$rc $(( $(date +%s) - s ))s"; grep -E "Checking amphora|^(error|warning)" -A 10 "$O/benchclippy.log" | tail -30 ;;
    oracletests)
      mkdir -p bench-lite/src; cp "$S/bench-lite/Cargo.toml" "$S/bench-lite/Cargo.lock" bench-lite/; cp "$S/bench-lite/src/main.rs" bench-lite/src/
      ( cd bench-lite && timeout 1200 nice -n 10 cargo test --release > "$O/oracletests.log" 2>&1 ); rc=$?
      echo "== oracle unit tests rc=$rc $(( $(date +%s) - s ))s"; grep -E "test result|^test |FAILED|panicked|^error" -A 3 "$O/oracletests.log" | head -50 ;;
    lib:*)
      timeout 1500 nice -n 10 cargo test -p amphora-engine --locked --lib --no-run > "$O/lib.log" 2>&1; rc=$?
      grep -E "^(error|warning)|Executable|Finished" -A 8 "$O/lib.log" | head -40; echo "== lib build rc=$rc in $(( $(date +%s) - s ))s"
      b=$(grep -oE "\(.*amphora_engine-[0-9a-f]+\)" "$O/lib.log" | tr -d '()' | tail -1); [ -z "$b" ] && b=$(ls -t "$CARGO_TARGET_DIR"/debug/deps/amphora_engine-* | grep -vE '\.(d|rlib|rmeta)$' | head -1)
      timeout 900 nice -n 10 "$b" $(echo "${step#lib:}" | tr , ' ') --test-threads=${THREADS:-8} > "$O/libtest.log" 2>&1; rc=$?
      echo "== libtest ${step#lib:} rc=$rc $(( $(date +%s) - s ))s"; grep -aE "test result|FAILED|failed$|panicked" "$O/libtest.log" | cut -c1-250 | head -20 ;;
    sh:*) bash -c "${step#sh:}" ;;
  esac
done
echo "== done $(date -u +%T) load $(load)"
