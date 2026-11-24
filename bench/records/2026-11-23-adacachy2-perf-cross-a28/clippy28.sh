#!/usr/bin/env bash
# Lane A28: clippy of the touched crates and of the loom harness under --cfg loom, check-production, fmt and the requirements test, on one SHA.
set -uo pipefail
SHA=${1:?}; OUT=$HOME/.cache/amphora-perf/cross-a28-out; W=${W:-$HOME/.cache/amphora-perf/cross-a28-wt}; T=${JOBS:-4}
R=$HOME/.cache/amphora-perf/cross-remote; git -C "$R" fetch -q origin perf/cross-a28 main || exit 3
[ -d "$W" ] || git -C "$R" worktree add -q --detach "$W" "$SHA"; cd "$W" && git checkout -q -f --detach "$SHA" && { [ -z "${TOUCH:-}" ] || git ls-files -z | xargs -0 touch -c; } && git log --oneline -1
export CARGO_HOME=$HOME/.cache/cargo-home-perf-cross PATH=$HOME/.cache/cargo-home-perf-cross/bin:$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/cross CARGO_BUILD_JOBS=$T CARGO_INCREMENTAL=0
stamp() { echo "[$(date -u +%H:%M:%SZ)] $* load=$(cut -d' ' -f1-3 /proc/loadavg)"; }
s=$SECONDS; nice -n 10 cargo clippy -p amphora -p amphora-line-loom --all-targets --locked -- -D warnings > "$OUT/clippy-$SHA.log" 2>&1; stamp "clippy rc=$? $((SECONDS-s))s: $(grep -E 'Checking amphora|Compiling amphora' "$OUT/clippy-$SHA.log" | tr '\n' ' ')"; grep -E '^(error|warning)' -A 12 "$OUT/clippy-$SHA.log" | grep -v 'vendor/hyper-raft' | head -60
s=$SECONDS; RUSTFLAGS="--cfg loom" nice -n 10 cargo clippy -p amphora-line-loom --all-targets --locked -- -D warnings > "$OUT/clippy-loom-$SHA.log" 2>&1; stamp "clippy of the loom harness rc=$? $((SECONDS-s))s"; grep -E '^(error|warning)' -A 12 "$OUT/clippy-loom-$SHA.log" | head -40
s=$SECONDS; nice -n 10 cargo fmt --all --check > "$OUT/fmt-$SHA.log" 2>&1; stamp "fmt rc=$? $((SECONDS-s))s"; head -20 "$OUT/fmt-$SHA.log"
s=$SECONDS; python3 scripts/test_requirements.py > "$OUT/req-$SHA.log" 2>&1; stamp "requirements rc=$?"; tail -2 "$OUT/req-$SHA.log"
s=$SECONDS; nice -n 10 bash scripts/check-production.sh > "$OUT/prod-$SHA.log" 2>&1; stamp "check-production rc=$? $((SECONDS-s))s"; tail -4 "$OUT/prod-$SHA.log"
