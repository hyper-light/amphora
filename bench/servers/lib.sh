# Sourced by the engine scripts beside it. Each sets NAME (always amphora-bench-<engine>: only that
# container is ever stopped or removed), DATA (its data directory on the host) and defines `run` (its
# `docker run -d`), `ready` (true once it takes connections) and `settings` (one JSON line: its
# version and the settings it reports), then calls `main "$@"`.
#
# Every engine runs on the host's network bound to 127.0.0.1, so no docker-proxy sits in each round
# trip (a published port goes through one), and as the calling user, so the files under DATA are
# the caller's to remove. Memory is capped at 8 GiB, swap off: a bound on a shared machine, far above
# what any of them takes on the bench's workload.

running() {
  [[ "$(docker inspect -f '{{.State.Running}}' "$NAME" 2>/dev/null)" == true ]]
}

start() {
  running && return 0
  docker rm "$NAME" >/dev/null 2>&1 || true
  mkdir -p "$DATA"
  run >/dev/null
  # Two minutes at most: MySQL's first start initializes its data directory, the slowest of them.
  local i
  for ((i = 0; i < 240; i++)); do
    if ready >/dev/null 2>&1; then
      return 0
    fi
    running || break
    sleep 0.5
  done
  echo "$NAME didn't come up; its log ends:" >&2
  docker logs --tail 40 "$NAME" >&2 || true
  return 1
}

stop() {
  docker stop -t 120 "$NAME" >/dev/null 2>&1 || true
  docker rm "$NAME" >/dev/null 2>&1 || true
}

reset() {
  stop
  rm -rf -- "${DATA:?}"
  start
}

main() {
  case "${1:-}" in
    start) start ;;
    stop) stop ;;
    restart)
      stop
      start
      ;;
    reset) reset ;;
    settings) settings ;;
    *)
      echo "usage: $0 start|stop|restart|reset|settings" >&2
      return 2
      ;;
  esac
}
