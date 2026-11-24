#!/usr/bin/env bash
# Lane C26's Linux driver (not for main). usage: linux.sh SHA|- STEP...
set -uo pipefail
SHA=$1; shift
export CARGO_HOME=$HOME/.cache/cargo-home-perf-dml-c26 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/dml-c26 CARGO_INCREMENTAL=${INCR:-1} CARGO_BUILD_JOBS=${JOBS:-8}
O=$HOME/.cache/amphora-perf/dml-c26-out; R=$HOME/.cache/amphora-perf/${CLONE:-dml-c26-remote}; S=$HOME/.cache/amphora-perf/dml-c26-scripts
mkdir -p "$O" "$CARGO_HOME" "$CARGO_TARGET_DIR"
for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -s "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
for f in config.toml config; do [ -e "$HOME/.cargo/$f" ] && [ ! -e "$CARGO_HOME/$f" ] && ln -s "$HOME/.cargo/$f" "$CARGO_HOME/$f"; done
export PATH=$CARGO_HOME/bin:$PATH
load() { cut -d' ' -f1-3 /proc/loadavg; }
[ -d "$R/.git" ] || git clone -q git@github.com:hyper-light/amphora.git "$R" || exit 2
cd "$R" || exit 2
git fetch -q origin 2>&1 | tail -2
rm -rf "${S:?}"; mkdir -p "$S"; git archive origin/diag/dml-c26 | tar -x -C "$S"
if [ "$SHA" != - ]; then
  git checkout -q -f --detach "$SHA" || exit 3
fi
for f in "$S"/probe/*.rs; do [ -f "$f" ] && { cmp -s "$f" crates/engine/tests/$(basename "$f") || cp "$f" crates/engine/tests/; }; done
echo "== $(hostname) $(date -u +%T) tree $(git rev-parse --short=8 HEAD) load $(load)"
D=$CARGO_TARGET_DIR/debug/deps
bin() { ls -t "$D"/$1-* | grep -v '\.d$' | head -1; }
for step in "$@"; do
  s=$(date +%s)
  case $step in
    build:*)
      tests=""; IFS=, read -ra names <<< "${step#build:}"; for n in "${names[@]}"; do tests="$tests --test $n"; done
      timeout 1200 nice -n 10 cargo test -p amphora-engine --locked $tests --no-run > "$O/build.log" 2>&1; rc=$?
      grep -E "^(error|warning)|Executable|Finished" -A 8 "$O/build.log" | head -${BUILD_LINES:-120}
      echo "== build rc=$rc in $(( $(date +%s) - s ))s"; [ $rc -ne 0 ] && exit $rc ;;
    probe:*)  # probe:FILE
      f=$S/${step#probe:}
      C26_FIXTURE=$f timeout ${WIDE_TIMEOUT:-420} nice -n 10 "$(bin c26_probe)" --nocapture > "$O/probe.log" 2>&1; rc=$?
      echo "== probe $f rc=$rc $(( $(date +%s) - s ))s"; grep -av "^thread\|^note\|^stack" "$O/probe.log" | cut -c1-${CUT:-900} | tail -${PROBE_LINES:-200} ;;
    gen:*)  # gen:COUNT:SEED
      IFS=: read -r _ count seed <<< "$step"
      python3 "$S/sqlite_foreign_keys.py" "$count" "$seed" > "$O/wide-$seed.tsv" || exit 9
      C26_FIXTURE=$O/wide-$seed.tsv timeout ${WIDE_TIMEOUT:-420} nice -n 10 "$(bin c26_probe)" --nocapture > "$O/wide-$seed.log" 2>&1; rc=$?
      echo "== wide $count $seed rc=$rc $(( $(date +%s) - s ))s load $(load)"
      tail -5 "$O/wide-$seed.log" | cut -c1-2000
      head -c ${WIDE_BYTES:-12000} "$O/wide-$seed.log" | cut -c1-${CUT:-700} ;;
    test:*)  # test:NAME[:FILTER]
      IFS=: read -r _ name filter <<< "$step"
      timeout 900 nice -n 10 "$(bin $name)" ${filter:-} --test-threads=${JOBS:-8} > "$O/test-$name.log" 2>&1; rc=$?
      echo "== test $name ${filter:-} rc=$rc $(( $(date +%s) - s ))s"; grep -aE "test result|panicked|answers differ|FAILED|failed$" "$O/test-$name.log" | head -${LINES_MAX:-20} ;;
    lib)
      timeout 1500 nice -n 10 cargo test -p amphora-engine --locked --lib --no-run > "$O/lib.log" 2>&1; rc=$?
      grep -E "^(error|warning)|Executable|Finished" -A 8 "$O/lib.log" | head -60
      echo "== lib rc=$rc in $(( $(date +%s) - s ))s" ;;
    libtest:*)
      timeout 900 nice -n 10 "$(ls -t "$D"/amphora_engine-* | grep -v '\.d$' | grep -v rlib | grep -v rmeta | head -1)" $(echo "${step#libtest:}" | tr , ' ') --test-threads=${JOBS:-8} > "$O/libtest.log" 2>&1; rc=$?
      echo "== libtest ${step#libtest:} rc=$rc $(( $(date +%s) - s ))s"; grep -E "test result|FAILED|failed$|panicked" "$O/libtest.log" | head -20 ;;
    clippy)
      timeout 1500 nice -n 10 cargo clippy -p amphora-engine --all-targets --locked -- -D warnings > "$O/clippy.log" 2>&1; rc=$?
      echo "== clippy rc=$rc $(( $(date +%s) - s ))s"; grep -E "Checking amphora-engine|^(error|warning)" -A 12 "$O/clippy.log" | head -80 ;;
    production)
      timeout 1500 nice -n 10 bash scripts/check-production.sh > "$O/production.log" 2>&1; rc=$?
      echo "== check-production rc=$rc $(( $(date +%s) - s ))s"; tail -8 "$O/production.log" ;;
    fmt)
      cargo fmt --all --check > "$O/fmt.log" 2>&1; echo "== fmt rc=$?"; head -60 "$O/fmt.log" ;;
    sh:*) bash -c "${step#sh:}" ;;
  esac
done
echo "== done $(date -u +%T) load $(load)"
