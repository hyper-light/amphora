#!/bin/sh
# perf/versioned (AUDIT 2.4): clippy and the versioned storage's tests with a private cargo home.
set -eu
CH="$HOME/.cache/cargo-home-perf-versioned"
mkdir -p "$CH"
for d in registry git bin; do [ -e "$CH/$d" ] || ln -s "$HOME/.cargo/$d" "$CH/$d"; done
export CARGO_HOME="$CH" CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$HOME/.cache/amphora-perf-target/versioned" CARGO_BUILD_JOBS=8
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
PATH="$HOME/.cargo/bin:$PATH"
df -h "$HOME/.cache" | tail -1
PKG=$(cargo metadata --no-deps --format-version 1 --offline | python3 -c 'import json,sys;print([p["name"] for p in json.load(sys.stdin)["packages"] if p["manifest_path"].endswith("crates/engine/Cargo.toml")][0])')
echo "engine package: $PKG"
case "${1:-all}" in
  clippy) nice -n 10 cargo clippy -p "$PKG" --all-targets --locked -- -D warnings ;;
  test) nice -n 10 cargo test -p "$PKG" --lib --locked -- storage::versioned ;;
  all) nice -n 10 cargo clippy -p "$PKG" --all-targets --locked -- -D warnings && nice -n 10 cargo test -p "$PKG" --lib --locked -- storage::versioned ;;
esac
