# Lane B26's focused run on Linux (bash). env: SHA, JOBS, TAG, T (tests/backup.rs filters), L (lib filters)
set -uo pipefail
export CARGO_HOME=$HOME/.cache/cargo-home-perf-pagepath CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/pagepath CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-4}
B=$HOME/.cache/amphora-perf/pagepath-b26; O=$B/${TAG:-r}; mkdir -p "$O"
cd $HOME/.cache/amphora-perf/pagepath-remote || exit 3
git fetch -q origin || exit 3
git checkout -q -f . && git checkout -q --detach "${SHA:?}" || exit 3
load() { cut -d' ' -f1-3 /proc/loadavg; }
echo "host $(hostname) tree $(git rev-parse HEAD) jobs $CARGO_BUILD_JOBS load $(load) $(date -u +%H:%M:%SZ)"
nice -n 10 cargo test -p amphora --locked --lib --test backup --no-run > "$O/build.log" 2>&1; r=$?
echo "build exit $r $(date -u +%H:%M:%SZ)"
if [ $r != 0 ]; then grep -E '^(error|warning)' -A14 "$O/build.log" | head -150; exit $r; fi
grep -E '^warning' -A8 "$O/build.log" | head -40
if [ -n "${T+x}" ]; then nice -n 10 cargo test -p amphora --locked --test backup -- --test-threads=${TT:-4} $T > "$O/t.log" 2>&1; echo "tests/backup.rs exit $? $(date -u +%H:%M:%SZ)"; fi
if [ -n "${L+x}" ]; then nice -n 10 cargo test -p amphora --locked --lib -- --test-threads=${TT:-4} $L > "$O/l.log" 2>&1; echo "lib exit $? $(date -u +%H:%M:%SZ)"; fi
for f in t l; do [ -f "$O/$f.log" ] && grep -E "^test .*(FAILED|failed)|panicked|^test result|left:|right:|refused|lost" -A3 "$O/$f.log" | cut -c1-900 | head -${LINES_MAX:-80}; done
echo "load $(load) $(date -u +%H:%M:%SZ)"
