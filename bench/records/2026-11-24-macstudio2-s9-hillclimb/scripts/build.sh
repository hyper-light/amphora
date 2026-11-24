#!/bin/bash
# usage: build.sh ARM JOBS   (ARM is a, b or c: a git worktree under ~/.cache/amphora-s9)
set -uo pipefail
S9=~/.cache/amphora-s9
arm=$1 jobs=$2
export CARGO_HOME=$S9/cargo-home CARGO_TARGET_DIR=$S9/target-$arm CARGO_BUILD_JOBS=$jobs
export CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
free=$(df -g ~ | tail -1 | awk '{print $4}'); [ "$free" -ge 30 ] || { echo "disk low: $free GB"; exit 2; }
[ -d $CARGO_TARGET_DIR ] || cp -cRp ~/.cache/amphora-perf-target/bench-s5 $CARGO_TARGET_DIR
cd $S9/$arm || exit 1
sha=$(git rev-parse HEAD)
echo "arm $arm sha $sha jobs $jobs $(rustc -V) / $(cargo -V) start $(date -u +%FT%TZ)"
env | grep '^CARGO' | sort
nice -n 10 cargo build --release --offline --manifest-path bench/Cargo.toml --bin amphora-bench || nice -n 10 cargo build --release --manifest-path bench/Cargo.toml --bin amphora-bench || exit 1
cp $CARGO_TARGET_DIR/release/amphora-bench $S9/bin/amphora-bench-$arm
cp bench/Cargo.lock $S9/log/Cargo.lock-$arm
echo "arm $arm end $(date -u +%FT%TZ) sha256 $(shasum -a 256 $S9/bin/amphora-bench-$arm | cut -d' ' -f1)"
