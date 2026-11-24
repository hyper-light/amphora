# Lane B26's last check: the follow-up's tests (bash). env: SHA, JOBS, TAG
set -uo pipefail
export CARGO_HOME=$HOME/.cache/cargo-home-perf-pagepath CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/pagepath CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-4}
B=$HOME/.cache/amphora-perf/pagepath-b26; L=$B/${TAG:-last}; mkdir -p "$L"
load() { cut -d' ' -f1-3 /proc/loadavg; }
cd $HOME/.cache/amphora-perf/pagepath-remote || exit 3
git fetch -q origin || exit 3
git checkout -q -f . && git clean -q -fd crates && git checkout -q --detach "${SHA:?}" || exit 3
echo "host $(hostname) tree $(git rev-parse HEAD) jobs $CARGO_BUILD_JOBS load $(load) $(date -u +%H:%M:%SZ)"
cargo fmt --all --check; echo "fmt exit $?"
nice -n 10 cargo test -p amphora --locked --lib --test backup --no-run > $L/build.log 2>&1; r=$?; echo "build exit $r $(date -u +%H:%M:%SZ)"; [ $r = 0 ] || { grep -E '^error' -A10 $L/build.log | head -40; exit $r; }
nice -n 10 cargo test -p amphora --locked --lib -- backup:: --test-threads=3 > $L/l.log 2>&1; echo "backup:: exit $? $(date -u +%H:%M:%SZ)"; grep -E "FAILED|panicked|test result" -A3 $L/l.log | cut -c1-300 | head -20
nice -n 10 cargo test -p amphora --locked --test backup -- --test-threads=4 > $L/t.log 2>&1; echo "tests/backup.rs exit $? $(date -u +%H:%M:%SZ)"; grep -E "FAILED|panicked|test result" -A3 $L/t.log | cut -c1-300 | head -20
nice -n 10 cargo clippy -p amphora --all-targets --locked -- -D warnings > $L/clippy.log 2>&1; echo "clippy exit $? $(date -u +%H:%M:%SZ)"; grep -E "^error|Checking amphora" -A6 $L/clippy.log | head -20
for m in m9-the-ask-wakes-nothing m14-the-base-for-it-keeps-chunks; do
  git checkout -q -f .; python3 $B/mutants.py $m || continue
  nice -n 10 cargo test -p amphora --locked --lib -- --test-threads=3 a_check_that > $L/$m.log 2>&1; echo "$m exit $? $(date -u +%H:%M:%SZ)"; grep -E "^test .*(FAILED|ok$)|panicked" -A2 $L/$m.log | cut -c1-240 | head -12
done
git checkout -q -f .
echo "done load $(load) $(date -u +%H:%M:%SZ)"
