#!/bin/sh
# AUDIT 7.5 mutant check: the crash test must fail once the flush between a checkpoint's pages and
# its header is deleted. Builds the tree at $SHA and its mutant in their own clones; then times
# the flush the versioned disk uses (File::sync_data) against fsync(2) and fcntl(F_FULLFSYNC).
set -u
. "$(dirname "$0")/env.sh"
SHA=${SHA:-$(git rev-parse HEAD)}
W="$HOME/.cache/versioned-ab"
mkdir -p "$W"
for side in base mutant; do
  rm -rf "$W/$side"
  git clone -q --no-checkout . "$W/$side" && git -C "$W/$side" checkout -q --detach "$SHA" || exit 1
done
F=crates/engine/storage/versioned/durable.rs
perl -0pi -e 's/\n        disk\.sync\(File::Data\)\?;\n(        Database::write_header\(disk, stamp\)\?;)/\n$1/' "$W/mutant/$F"
echo "mutant diff:"; (cd "$W/mutant" && git diff --stat && git diff | grep '^[-+] ')
T="$HOME/.cache/amphora-perf-target/versioned"
echo "host $(hostname -s) $(uname -m) load $(load) sha $SHA"
(cd "$W/base" && CARGO_TARGET_DIR="$T" nice -n 10 cargo test -p amphora-engine --lib --locked -- storage::versioned::durable --test-threads=${JOBS:-8} 2>&1 | grep -E '^test .*FAILED|^test result|panicked' )
(cd "$W/mutant" && CARGO_TARGET_DIR="$T" nice -n 10 cargo test -p amphora-engine --lib --locked -- audit_7_5_a_crash_at_each_step_of_a_checkpoint_recovers_every_commit --test-threads=${JOBS:-8} > "$W/mutant.log" 2>&1; echo "MUTANT_EXIT=$?")
grep -E -A3 'panicked|^test result' "$W/mutant.log" | head -20
cat > "$W/flush.rs" <<'RS'
use std::io::Write;
use std::os::fd::AsRawFd;
use std::time::Instant;
unsafe extern "C" { fn fsync(fd: i32) -> i32; fn fcntl(fd: i32, cmd: i32, ...) -> i32; }
const F_FULLFSYNC: i32 = 51; // <sys/fcntl.h>
fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let mut f = std::fs::File::create(format!("{dir}/flush.bin")).unwrap();
    let page = [7u8; 4096];
    for (name, how) in [("sync_data", 0), ("fsync", 1), ("F_FULLFSYNC", 2), ("sync_data", 0), ("fsync", 1), ("F_FULLFSYNC", 2)] {
        let mut us = Vec::new();
        for _ in 0..40 {
            f.write_all(&page).unwrap();
            let t = Instant::now();
            match how { 0 => f.sync_data().unwrap(), 1 => assert_eq!(unsafe { fsync(f.as_raw_fd()) }, 0), _ => assert_eq!(unsafe { fcntl(f.as_raw_fd(), F_FULLFSYNC) }, 0) }
            us.push(t.elapsed().as_secs_f64() * 1e6);
        }
        us.sort_by(f64::total_cmp);
        println!("{name:12} n=40 min={:.0}us median={:.0}us max={:.0}us", us[0], us[20], us[39]);
    }
}
RS
rustc -O --edition 2024 -o "$W/flush" "$W/flush.rs" && "$W/flush" "$W"
echo "rustc $(rustc --version) load $(load)"
