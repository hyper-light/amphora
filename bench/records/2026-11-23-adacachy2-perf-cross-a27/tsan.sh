#!/usr/bin/env bash
# Lane A27: the a27_, a26_, a21_ and a11_ tests under ThreadSanitizer. usage: tsan.sh SHA ROUNDS. A private nightly in its own RUSTUP_HOME,
# with the precompiled TSan std (rustup target x86_64-unknown-linux-gnutsan), so nothing shared is touched.
set -uo pipefail
SHA=${1:?}; ROUNDS=${2:-3}
export RUSTUP_HOME=$HOME/.cache/rustup-a27 CARGO_HOME=$HOME/.cache/cargo-home-perf-cross PATH=$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/cross CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0
N=nightly-2026-09-05
rustup toolchain install $N --profile minimal -c rust-src --no-self-update >/dev/null 2>&1
rustup target add --toolchain $N x86_64-unknown-linux-gnutsan >/dev/null 2>&1
R=$HOME/.cache/amphora-perf/cross-remote; W=$HOME/.cache/amphora-perf/cross-a27-wt3; OUT=$HOME/.cache/amphora-perf/cross-a27-out
git -C "$R" fetch -q origin main perf/cross-a27; [ -d "$W" ] || git -C "$R" worktree add -q --detach "$W" "$SHA"; cd "$W" && git checkout -q -f --detach "$SHA"
nice -n 10 cargo +$N test -p amphora --lib --locked --no-run --target x86_64-unknown-linux-gnutsan > "$OUT/tsan-build.log" 2>&1 || exit 1
bin=$(sed -n 's/.*Executable unittests src\/lib.rs (\(.*\))/\1/p' "$OUT/tsan-build.log"); cd crates/amphora
for r in $(seq 1 "$ROUNDS"); do
  rm -f "$OUT"/tsan.report*
  TSAN_OPTIONS="halt_on_error=0 exitcode=66 history_size=7 second_deadlock_stack=1 log_path=$OUT/tsan.report" timeout 900 nice -n 10 "$bin" --test-threads=4 --nocapture a27_ a26_ a21_ a11_ unknown_leader_application_pressure > "$OUT/tsan-$r.log" 2>&1
  echo "round $r rc=$? $(grep 'test result' "$OUT/tsan-$r.log") reports: $(ls "$OUT"/tsan.report* 2>/dev/null | wc -l)"
done
