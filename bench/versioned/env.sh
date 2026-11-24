# Sourced by the perf/versioned lane scripts: a private cargo home and lock, offline, held to $JOBS jobs (default 8: RULES 6).
CH="$HOME/.cache/cargo-home-perf-versioned"
mkdir -p "$CH"
for d in registry git bin; do [ -e "$CH/$d" ] || ln -s "$HOME/.cargo/$d" "$CH/$d"; done
export CARGO_HOME="$CH" CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=${JOBS:-8}
PATH="$HOME/.cargo/bin:$PATH"
load() { cat /proc/loadavg 2>/dev/null || sysctl -n vm.loadavg 2>/dev/null; }
