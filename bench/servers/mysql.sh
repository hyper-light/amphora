#!/usr/bin/env bash
# MySQL 8.4, the official image, for the bench's `mysql` target (src/vitess.rs speaking to it
# directly, database `convo`), on 127.0.0.1:33306.
#
# Durable as it ships, given explicitly so an image's change can't loosen them:
# innodb_flush_log_at_trx_commit = 1 (the redo log flushed at every commit) and sync_binlog = 1 with
# the binary log on (its default), so a commit is acknowledged once it is on disk in both, as
# Amphora's is. Memory as it ships (innodb_buffer_pool_size 128MB), so its memory compares with
# Amphora's on equal terms. max_connections = 1100: --connections up to 1024 and the readers. The X
# protocol is off: on the host's network its port (33060) could collide, and the bench doesn't use
# it.
#
# usage: servers/mysql.sh start|stop|restart|reset|settings
#   reset drops the data directory and starts on a new one.
set -euo pipefail
NAME=amphora-bench-mysql
IMAGE=${IMAGE:-mysql:8.4}
PORT=33306
DATA=${AMPHORA_BENCH_DATA:-$HOME/.cache/amphora-bench-data-nocow}/mysql
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

run() {
  # secure_file_priv's directory belongs to the image's own user; the calling user gets one of its own.
  docker run -d --name "$NAME" --network host --user "$(id -u):$(id -g)" --memory 8g --memory-swap 8g \
    --tmpfs "/var/lib/mysql-files:uid=$(id -u),gid=$(id -g),mode=0750" \
    -e MYSQL_ALLOW_EMPTY_PASSWORD=yes -e MYSQL_DATABASE=convo -v "$DATA:/var/lib/mysql" \
    "$IMAGE" --port="$PORT" --bind-address=127.0.0.1 --mysqlx=OFF --max-connections=1100 \
    --innodb-flush-log-at-trx-commit=1 --sync-binlog=1
}

ready() {
  docker exec "$NAME" mysql -h 127.0.0.1 -P "$PORT" -u root -e "SELECT 1" convo
}

settings() {
  docker exec "$NAME" mysql -h 127.0.0.1 -P "$PORT" -u root -NB -e "SELECT JSON_OBJECT('version', @@version, 'innodb_flush_log_at_trx_commit', @@innodb_flush_log_at_trx_commit, 'sync_binlog', @@sync_binlog, 'log_bin', @@log_bin, 'innodb_doublewrite', @@innodb_doublewrite, 'innodb_flush_method', @@innodb_flush_method, 'innodb_buffer_pool_size', @@innodb_buffer_pool_size, 'innodb_redo_log_capacity', @@innodb_redo_log_capacity, 'max_connections', @@max_connections, 'transaction_isolation', @@transaction_isolation, 'collation_server', @@collation_server)"
}

main "$@"
