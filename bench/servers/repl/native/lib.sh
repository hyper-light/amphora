# Sourced by the native three-node cluster scripts beside it: the same systems and the same
# interface as ../lib.sh's containers (bench/src/repl.rs drives either, `--scripts` naming the
# folder), but every node a process on this host, from the release binaries setup.sh fetched. Ada's
# fair-comparison rule (2026-11-22): every system on the same platform, containers for all or for
# none; Amphora's members (bench/repl_members.sh) are processes here, so its competitors are too.
#
# Each script sets SYS and PORT (node n's client port is PORT+n on 127.0.0.1; a system with a
# second port a node takes PORT+10+n, and a third PORT+20+n), defines `run_node N` (starts node N in
# the background and prints nothing), `ready_node N`, `leader` (node 0 to 2, or nothing) and
# `version` (the binary's own version line), states DURABILITY (what an acknowledged write has
# survived, with the flags that make it so), then calls `main "$@"`.
#
# Binaries come from AMPHORA_REPL_BIN (default ~/.cache/amphora-repl-bin, setup.sh's). Each node's
# data, log and PID file live under AMPHORA_REPL_DATA (default ~/.cache/amphora-repl-data)/<name>-<sys>/N:
# `down` kills only the PIDs recorded there (kill -9) and removes that folder. <name> is
# AMPHORA_BENCH_REPL_NAME (default amphora-bench-repl): runs sharing a host give each their own, with
# their own AMPHORA_BENCH_REPL_PORT.
#
# usage: <system>.sh up|down|kill N|start N|leader|endpoint N|about
set -euo pipefail
NAME=${AMPHORA_BENCH_REPL_NAME:-amphora-bench-repl}
BIN=${AMPHORA_REPL_BIN:-$HOME/.cache/amphora-repl-bin}
DATA=${AMPHORA_REPL_DATA:-$HOME/.cache/amphora-repl-data}/$NAME-$SYS
port() { echo $((PORT + $1)); }
port2() { echo $((PORT + 10 + $1)); }
port3() { echo $((PORT + 20 + $1)); }
dir() { echo "$DATA/$1"; }

need_bin() {
  [[ -x "$BIN/$1" ]] || { echo "no $BIN/$1: run bench/servers/repl/native/setup.sh on this host first" >&2; return 1; }
}

# Starts `$@` as node N's process, its output in its log, its PID recorded for kill and down. It
# leads a process group of its own (setsid), so a kill takes what it started too (LiteFS's shim).
spawn() {
  local n=$1
  shift
  mkdir -p "$(dir "$n")"
  setsid "$@" >>"$(dir "$n")/log" 2>&1 </dev/null &
  echo $! >"$(dir "$n")/pid"
}

kill_node() {
  local f
  f="$(dir "$1")/pid"
  [[ -f "$f" ]] || return 0
  # Wait on the process's own exit (a pidfd turns readable when it ends: Linux 5.3+), so its ports
  # and files are free before a start; it isn't this shell's child, so `wait` can't.
  python3 -I -c 'import os,select,signal,sys
try: fd=os.pidfd_open(int(sys.argv[1]))
except ProcessLookupError: sys.exit(0)
try: os.killpg(int(sys.argv[1]), signal.SIGKILL)
except ProcessLookupError: pass
try: signal.pidfd_send_signal(fd, signal.SIGKILL)
except ProcessLookupError: sys.exit(0)
select.select([fd],[],[])' "$(cat "$f")"
  rm -f "$f"
  if declare -F after_kill >/dev/null; then after_kill "$1"; fi
}

# A node's readiness through its own blocking check where the system has one; else `$@` is tried
# until it answers, bounded by the first start's two minutes (../lib.sh's bound). This is bring-up,
# outside every measure.
wait_for() {
  local i
  for ((i = 0; i < 2400; i++)); do
    if "$@" >/dev/null 2>&1; then return 0; fi
    sleep 0.05
  done
  return 1
}

down() {
  local n
  for n in 0 1 2; do kill_node "$n"; done
  if declare -F after_down >/dev/null; then after_down; fi
  rm -rf "$DATA"
}

up() {
  down
  mkdir -p "$DATA"
  if declare -F before_up >/dev/null; then before_up; fi
  local n
  for n in 0 1 2; do run_node "$n"; done
  for n in 0 1 2; do
    if ! wait_for ready_node "$n"; then
      echo "$SYS node $n didn't come up; its log ends:" >&2
      tail -n 40 "$(dir "$n")/log" >&2 || true
      return 1
    fi
  done
  if ! wait_for has_leader; then
    echo "$SYS named no leader" >&2
    return 1
  fi
}

has_leader() { [[ -n "$(leader)" ]]; }

# What the record says of this cluster: the platform it ran on and what a commit means here.
about() {
  local fs kernel
  mkdir -p "$DATA"
  fs=$(stat -f -c %T "$DATA" 2>/dev/null || echo unknown)
  kernel=$(uname -srm)
  # Every option the binary's own help names for syncing or durability, so the record shows the
  # defaults it ran with, not only what DURABILITY says of them.
  python3 -I -c 'import json,sys; print(json.dumps({"system": sys.argv[1], "version": sys.argv[2], "platform": "native", "kernel": sys.argv[3], "data_fs": sys.argv[4], "durability": sys.argv[5], "sync_options": [l.strip() for l in sys.argv[6].splitlines() if l.strip()]}))' \
    "$SYS" "$(version 2>&1 | head -n 1)" "$kernel" "$fs" "$DURABILITY" "$(help_text 2>&1 | grep -iE 'sync|durab|fsync' || true)"
}

main() {
  case "${1:-}" in
    up) up ;;
    down) down ;;
    kill) kill_node "$2" ;;
    start)
      run_node "$2"
      wait_for ready_node "$2"
      ;;
    leader) leader ;;
    endpoint) echo "http://127.0.0.1:$(port "$2")" ;;
    about) about ;;
    *)
      echo "usage: $0 up|down|kill N|start N|leader|endpoint N|about" >&2
      return 2
      ;;
  esac
}
