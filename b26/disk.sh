# Lane B26's small disk drill, each arm (bash). env: ARMS (SHAs), JOBS
set -uo pipefail
export CARGO_HOME=$HOME/.cache/cargo-home-perf-pagepath CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/pagepath CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-6}
B=$HOME/.cache/amphora-perf/pagepath-b26; D=$B/small-disk; mkdir -p "$D"
cd $HOME/.cache/amphora-perf/pagepath-remote || exit 3
git fetch -q origin || exit 3
for sha in ${ARMS:?}; do
  git checkout -q -f . && git checkout -q --detach "$sha" || exit 3
  python3 $B/disk.py
  nice -n 10 cargo test -p amphora --locked --test backup --no-run > $B/disk-$sha.build 2>&1 || { echo "$sha: build failed"; grep -E '^error' -A8 $B/disk-$sha.build | head -20; continue; }
  exe=$(grep -oE '\(.*/backup-[0-9a-f]+\)' $B/disk-$sha.build | tr -d '()' | tail -1)
  echo "== arm $(git rev-parse --short HEAD) $exe load $(cut -d' ' -f1-3 /proc/loadavg) $(date -u +%H:%M:%SZ)"
  unshare --user --map-root-user --mount bash -c "mount -t tmpfs -o size=2m tmpfs '$D' && B26_DISK='$D' '$exe' --exact b26_full_disk --nocapture" > $B/disk-$sha.log 2>&1; echo "exit $?"
  grep -E "^B26|panicked|test result" -A1 $B/disk-$sha.log | cut -c1-260 | head -40
  "$exe" --exact b26_left --nocapture > $B/left-$sha.log 2>&1; echo "exit $?"
  grep -E "^B26|panicked|test result" -A1 $B/left-$sha.log | cut -c1-900 | head -20
  git checkout -q -f .
done
echo "done $(date -u +%H:%M:%SZ)"
