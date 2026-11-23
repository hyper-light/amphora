#!/usr/bin/env bash
# perf/writes checks: clippy on amphora, then the given test commands. Run from the repository root.
set -uo pipefail
export CARGO_HOME=$HOME/.cache/cargo-home-perf-writes CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/writes CARGO_BUILD_JOBS=8 PATH=$HOME/.cargo/bin:$PATH
mkdir -p "$CARGO_HOME"; for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -s "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
echo "host=$(hostname) load=$(cut -d' ' -f1-3 /proc/loadavg) commit=$(git rev-parse --short HEAD) dirty=$(git status --short | wc -l)"
fail=0
if [ "${CLIPPY:-1}" = 1 ]; then
  nice -n 10 cargo clippy -p amphora --all-targets --locked -- -D warnings 2>&1 | grep -E '^(error|warning)|Finished|-->' | head -40; [ "${PIPESTATUS[0]}" -eq 0 ] || { echo "CLIPPY FAILED"; fail=1; }
fi
for spec in "$@"; do
  for i in $(seq 1 "${REPEAT:-1}"); do
    echo "== cargo test $spec (run $i)"
    nice -n 10 cargo test -p amphora --locked $spec 2>&1 | grep -E 'test result|FAILED|panicked|failed|error(\[|:)|^test .* \.\.\. (FAILED|ok)$' | grep -v '\.\.\. ok$' | head -30
    [ "${PIPESTATUS[0]}" -eq 0 ] || { echo "TEST FAILED: $spec run $i"; fail=1; }
  done
done
echo "load=$(cut -d' ' -f1-3 /proc/loadavg) exit=$fail"
exit $fail
