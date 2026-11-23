#!/usr/bin/env bash
# Amphora as a server, `amphora serve`, for the bench's `amphora-serve` target (src/serve.rs), on
# 127.0.0.1:57401: one member, its group's only voter, reached through Amphora's own client as
# every other server is through its own (docs/design/client-protocol.md, decision 2).
#
# Built from this checkout in rust:1-bookworm with the workspace's release profile, into volumes of
# its own (as backup_restore.sh builds Amphora's leg), its binary baked into an image on
# debian:bookworm-slim, the build's glibc: amphora-bench-amphora-image.
#
# Durable as it ships: class = persisted, a commit acknowledged once its log is flushed on the
# member's disk, as PostgreSQL's synchronous_commit = on. Its memory is what its budget learns from
# the container's limit (lib.sh), as every server's is held to it.
#
# Its config and the cluster's secret live in $CONF, mounted at the same path inside, so the bench
# reads the one config the member serves: its address, its secret and its deadline. The deadline is
# the give-up the bench gives SQLite (src/vanilla.rs, busy_timeout), so each target gives up alike.
#
# usage: servers/amphora.sh start|stop|restart|reset|settings
#   reset drops the data directory and starts on a new one.
set -euo pipefail
NAME=amphora-bench-amphora
IMAGE=${IMAGE:-amphora-bench-amphora-image}
PORT=57401
DEADLINE_MS=60000
DATA=${AMPHORA_BENCH_DATA:-$HOME/.cache/amphora-bench-data-nocow}/amphora
CONF=${AMPHORA_SERVE_CONF:-$HOME/.config/amphora-bench/amphora-serve}
ROOT=$(cd "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

# The member's config, and the cluster's secret once: 32 random bytes, HMAC-SHA256's key length
# (RFC 2104 §3), the shortest `amphora serve` takes.
config() {
  mkdir -p "$CONF"
  if [[ ! -s "$CONF/secret" ]]; then
    (umask 077 && head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' >"$CONF/secret")
  fi
  cat >"$CONF/member.conf" <<EOF
node = 1
members = 1@127.0.0.1:$PORT
data = $DATA
class = persisted
secret_file = $CONF/secret
deadline_ms = $DEADLINE_MS
EOF
}

# This checkout's `amphora`, built in rust:1-bookworm, baked into $IMAGE.
image() {
  docker run --rm -v "$ROOT:/src:ro" -v "$NAME-cargo:/usr/local/cargo/registry" -v "$NAME-target:/target" \
    -e CARGO_TARGET_DIR=/target rust:1-bookworm sh -c 'cd /src && cargo build -q -p amphora --release --locked --bin amphora' >&2
  local ctx
  ctx=$(mktemp -d "$HOME/.cache/amphora-bench-image.XXXXXX")
  docker run --rm --user "$(id -u):$(id -g)" -v "$NAME-target:/target:ro" -v "$ctx:/out" rust:1-bookworm cp /target/release/amphora /out/amphora
  printf 'FROM debian:bookworm-slim\nCOPY amphora /usr/local/bin/amphora\n' >"$ctx/Dockerfile"
  docker build -q -t "$IMAGE" "$ctx" >/dev/null
  rm -rf -- "${ctx:?}"
}

run() {
  config
  image
  docker run -d --name "$NAME" --network host --user "$(id -u):$(id -g)" $(limits) \
    -v "$DATA:$DATA" -v "$CONF:$CONF:ro" "$IMAGE" amphora serve "$CONF/member.conf"
}

ready() {
  docker exec "$NAME" amphora health "$CONF/member.conf"
}

settings() {
  printf '{"version": "amphora %s", "class": "persisted", "deadline_ms": %s}\n' "$(git -C "$ROOT" rev-parse HEAD)" "$DEADLINE_MS"
}

main "$@"
