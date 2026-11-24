#!/bin/bash
set -uo pipefail
S9=~/.cache/amphora-s9; arm=c
export CARGO_HOME=$S9/cargo-home CARGO_TARGET_DIR=$S9/target-$arm CARGO_BUILD_JOBS=8
export CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
cd $S9/$arm || exit 1
sha=$(git rev-parse HEAD)
echo "arm $arm sha $sha jobs 8 (resumed with arm A's compiled libduckdb-sys and duckdb, same lock, profile and toolchain) $(rustc -V) start $(date -u +%FT%TZ)"
nice -n 10 cargo build --release --offline --manifest-path bench/Cargo.toml --bin amphora-bench || exit 1
cp $CARGO_TARGET_DIR/release/amphora-bench $S9/bin/amphora-bench-$arm
cp bench/Cargo.lock $S9/log/Cargo.lock-$arm
echo "arm $arm end $(date -u +%FT%TZ) sha256 $(shasum -a 256 $S9/bin/amphora-bench-$arm | cut -d' ' -f1)"
