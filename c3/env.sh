# Lane C3: the environment of every run on a Mac. Never runner 2768d0d5 (Adas-Mac-Studio-4).
case "$(hostname)" in Adas-Mac-Studio-4*) echo "runner 2768d0d5: not allowed"; exit 3;; esac
[ "$(df -g ~ | tail -1 | awk '{print $4}')" -ge 40 ] || { echo "disk low"; exit 2; }
export CARGO_HOME=$HOME/.cache/cargo-home-perf-planner-c3
mkdir -p "$CARGO_HOME"
for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -s "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
export PATH="$CARGO_HOME/bin:/opt/homebrew/bin:$PATH"
export CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/planner-c3 CARGO_BUILD_JOBS=${JOBS:-8} CARGO_INCREMENTAL=0
BINS=$HOME/.cache/amphora-perf-bin/planner-c3; mkdir -p "$BINS"
echo "== $(hostname -s) $(date -u +%H:%M:%SZ) $(uptime | sed 's/.*load/load/') HEAD $(git rev-parse --short HEAD) $(git status --short | wc -l | tr -d ' ') changed files"
