# Lane B25's check run on Linux (bash). env: SHA, JOBS, TT, TAG
set -uo pipefail
export CARGO_HOME=$HOME/.cache/cargo-home-perf-pagepath CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/pagepath CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-4}
L=$HOME/.cache/amphora-perf/pagepath-b25/${TAG:-r}; mkdir -p "$L"
load() { cut -d' ' -f1-3 /proc/loadavg; }
cd $HOME/.cache/amphora-perf/pagepath-remote || exit 3
git fetch -q origin || exit 3
git checkout -q --detach "${SHA:?}" || exit 3
echo "host $(hostname) tree $(git rev-parse HEAD) jobs $CARGO_BUILD_JOBS tt ${TT:-4} load $(load) $(date -u +%H:%M:%SZ)"
rm -f "$L"/*.rc
run() {
  local name=$1; shift; local t0; t0=$(date +%s)
  nice -n 10 "$@" > "$L/$name.log" 2>&1; local r=$?
  echo "$r" > "$L/$name.rc"
  { echo "== $name exit $r in $(( $(date +%s)-t0 )) s, load $(load) $(date -u +%H:%M:%SZ)"
    grep -E '^(error|warning)|^test .*FAILED|^failures:|panicked|^test result|Checking amphora|Compiling amphora-engine|as the ratchet|left ' "$L/$name.log" | cut -c1-300 | head -n 40; } > "$L/$name.sum"
  cat "$L/$name.sum"
}
T=${TT:-4}
run fmt cargo fmt --all --check
run build-amphora cargo test -p amphora --locked --lib --test backup --test store --test crash --no-run
( run backuplib cargo test -p amphora --locked --lib -- backup:: --test-threads=2
  run store cargo test -p amphora --locked --test store -- --test-threads=2 ) &
( run backup cargo test -p amphora --locked --test backup -- --test-threads="$T" ) &
( run crash cargo test -p amphora --locked --test crash -- a_power_cut_at_any_change --test-threads=1 ) &
wait
run build-engine cargo test -p amphora-engine --lib --locked --no-run
run d45 cargo test -p amphora-engine --lib --locked -- d45_ --test-threads=2
touch crates/engine/lib.rs
run clippy cargo clippy -p amphora -p amphora-engine --all-targets --locked -- -D warnings
touch crates/engine/lib.rs
run prod bash scripts/check-production.sh
rc=0
for f in "$L"/*.rc; do r=$(cat "$f"); echo "$(basename "$f" .rc) $r"; [ "$r" = 0 ] || rc=$r; done
echo "done $(date -u +%H:%M:%SZ) load $(load)"
exit $rc
