#!/usr/bin/env bash
# ClickHouse (the official image, 26.8, its current long-term release) for the bench's `clickhouse`
# and `clickhouse-durable` targets (src/clickhouse.rs, over HTTP on 127.0.0.1:58123). The server
# runs as it ships; each target's durability is its table's and its inserts' settings:
# - clickhouse: its defaults. Since 26.2 those gather small inserts server-side (async_insert = 1,
#   wait_for_async_insert = 1; system.settings_changes: "Enable async inserts by default"), each
#   acknowledged once its batch's part is written to the filesystem, before it is flushed
#   (fsync_after_insert = 0, fsync_part_directory = 0): an acknowledged write can be lost to a power
#   cut;
# - clickhouse-durable: the same batching, asked for explicitly on each insert, and the table flushes
#   every inserted part and its directory (fsync_after_insert = 1, fsync_part_directory = 1), so an
#   insert is acknowledged once its part is on disk.
# Memory as it ships (its caches sized from the memory it sees: here the container's 8 GiB cap).
#
# usage: servers/clickhouse.sh start|stop|restart|reset|settings
#   reset drops the data directory and starts on a new one.
set -euo pipefail
NAME=amphora-bench-clickhouse
IMAGE=${IMAGE:-clickhouse/clickhouse-server:26.8}
PORT=58123
DATA=${AMPHORA_BENCH_DATA:-$HOME/.cache/amphora-bench-data-nocow}/clickhouse
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

run() {
  mkdir -p "$DATA/data" "$DATA/log"
  # In place of the image's docker_related_config.xml (which listens on every address): its own
  # config.xml listens on localhost only; HTTP and the native protocol move to ports nothing else on
  # the host uses, and the protocols the bench doesn't speak are off.
  cat >"$DATA/ports.xml" <<XML
<clickhouse>
    <http_port>$PORT</http_port>
    <tcp_port>59000</tcp_port>
    <mysql_port remove="1"/>
    <postgresql_port remove="1"/>
    <interserver_http_port remove="1"/>
</clickhouse>
XML
  docker run -d --name "$NAME" --network host --user "$(id -u):$(id -g)" $(limits) --ulimit nofile=262144:262144 \
    -e CLICKHOUSE_SKIP_USER_SETUP=1 -v "$DATA/data:/var/lib/clickhouse" -v "$DATA/log:/var/log/clickhouse-server" \
    -v "$DATA/ports.xml:/etc/clickhouse-server/config.d/docker_related_config.xml:ro" "$IMAGE"
}

ready() {
  curl -fsS "http://127.0.0.1:$PORT/ping"
}

settings() {
  curl -fsS "http://127.0.0.1:$PORT/" --data-binary "SELECT version() AS version,
    (SELECT mapFromArrays(groupArray(name), groupArray(value)) FROM system.merge_tree_settings WHERE name IN ('fsync_after_insert', 'fsync_part_directory', 'parts_to_delay_insert', 'parts_to_throw_insert', 'old_parts_lifetime')) AS merge_tree_defaults,
    (SELECT mapFromArrays(groupArray(name), groupArray(value)) FROM system.settings WHERE name IN ('async_insert', 'wait_for_async_insert', 'async_insert_use_adaptive_busy_timeout', 'async_insert_busy_timeout_min_ms', 'async_insert_busy_timeout_max_ms', 'async_insert_max_data_size')) AS query_defaults,
    (SELECT any(engine_full) FROM system.tables WHERE database = 'default' AND name = 'events') AS events_table
    FORMAT JSONEachRow"
}

main "$@"
