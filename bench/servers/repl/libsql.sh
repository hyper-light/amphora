#!/usr/bin/env bash
# libSQL's server (sqld), one primary and two replicas, each a process on this host (setup.sh's
# release binary): HTTP (Hrana over HTTP, /v2/pipeline) on 127.0.0.1:54220-54222, the primary's
# gRPC for its replicas on 54230, its admin API on 54223 (PORT+3: between the node ports and the
# gRPC ports, so it meets no other system's default range). Replicas forward writes to the
# primary and follow it; there is no election, so node 0 is always the leader.
#
# Durability, from libsql-server-v0.24.32's source (what setup.sh fetches; re-check on a newer tag):
# - Each connection sets PRAGMA synchronous from its database's durability_mode
#   (libsql-server/src/connection/connection_core.rs:80-83), whose default is Relaxed, which is
#   synchronous=NORMAL (connection/config.rs:115-132). In WAL mode NORMAL doesn't sync at commit:
#   "A transaction committed in WAL mode with synchronous=NORMAL might roll back following a power
#   loss" (sqlite.org/pragma.html#pragma_synchronous). So sqld's default acknowledges an unsynced
#   commit.
# - The strongest it offers is Strong (FULL; Extra is "no different from FULL in WAL mode", the same
#   page), set per database through the admin API: POST /v1/namespaces/<ns>/config with
#   durability_mode (http/admin/mod.rs:145-148, 296-345). That POST sets every field of the
#   namespace's config from its body (block_reads, block_writes, block_reason, allow_attach,
#   txn_timeout_s, jwt_key: mod.rs:330-342), so the harness reads the config first (GET, mod.rs:
#   244-265) and posts it back with only durability_mode changed; max_db_size is left out, which
#   keeps the namespace's own (mod.rs:336-338) rather than round-trip it through ByteSize's text.
#   Here: Strong on the default namespace, set before the first measured
#   connection exists (sqld keeps it in its metastore database, namespace/meta_store.rs:113 and 366,
#   and the primary is restarted after setting it, so every connection is opened under it), and `up`
#   refuses unless the primary answers PRAGMA synchronous = 2.
# - Replicas pull the primary's replication log; a commit only notifies them
#   (replication/primary/replication_logger_wal.rs:60 and 113-116, send_replace) and returns. No
#   flag makes the primary wait for a replica (src/main.rs's options), so no synchronous replica
#   mode exists: one member's disk, not 2 of 3.
set -euo pipefail
SYS=libsql
PORT=${AMPHORA_BENCH_REPL_PORT:-54220}
DURABILITY="acknowledged once the primary's SQLite WAL holds it, fsynced (durability_mode=strong, synchronous=FULL; sqld's default, relaxed, is synchronous=NORMAL and doesn't sync at commit); replicas pull it asynchronously and sqld has no mode that waits for one, so no quorum: one member's disk"
# sqld's own line once its user HTTP listener is bound (src/main.rs:476-480 at v0.24.32; RUST_LOG
# defaults to info, main.rs:748-749), then one blocking request (after_ready).
READY='listening for incoming user HTTP connection on '
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
admin_port() { echo $((PORT + 3)); }

run_node() {
  local n=$1
  need_bin sqld
  if [[ "$n" == 0 ]]; then
    spawn "$n" "$BIN/sqld" --db-path "$(dir "$n")/data" --http-listen-addr "127.0.0.1:$(port "$n")" --grpc-listen-addr "127.0.0.1:$(port2 0)" \
      --admin-listen-addr "127.0.0.1:$(admin_port)"
  else
    spawn "$n" "$BIN/sqld" --db-path "$(dir "$n")/data" --http-listen-addr "127.0.0.1:$(port "$n")" --primary-grpc-url "http://127.0.0.1:$(port2 0)"
  fi
}

# The line comes as the listener binds, before sqld serves on it: one request, which the bound
# listener holds until sqld answers, with bring-up's bound as the give-up.
after_ready() { curl -sf --max-time "$BRINGUP_S" "http://127.0.0.1:$(port "$1")/health" >/dev/null; }

# Strong durability on the primary's default namespace (see the header), then a restart, so no
# connection opened before the change serves a measured write.
before_up() {
  run_node 0
  ready_node 0 || { echo "libsql primary didn't come up to set its durability; its log ends:" >&2; tail -n 40 "$(dir 0)/log" >&2; return 1; }
  local url="http://127.0.0.1:$(admin_port)/v1/namespaces/default/config" config
  # The namespace's own config, durability_mode alone changed (see the header).
  config=$(curl -sf --max-time "$BRINGUP_S" "$url" | python3 -I -c 'import json, sys
c = json.load(sys.stdin); c.pop("max_db_size", None); c["durability_mode"] = "strong"; print(json.dumps(c))') \
    || { echo "libsql: the admin API didn't give the default namespace's config" >&2; return 1; }
  curl -sf --max-time "$BRINGUP_S" -X POST -H 'content-type: application/json' -d "$config" "$url" >/dev/null \
    || { echo "libsql: the admin API refused durability_mode=strong" >&2; return 1; }
  kill_node 0
}

# The primary's own PRAGMA synchronous, through the same HTTP API the harness writes through.
synchronous() {
  curl -sf --max-time "$BRINGUP_S" -H 'content-type: application/json' \
    -d '{"requests": [{"type": "execute", "stmt": {"sql": "PRAGMA synchronous"}}, {"type": "close"}]}' \
    "http://127.0.0.1:$(port 0)/v2/pipeline" \
    | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["results"][0]["response"]["result"]["rows"][0][0]["value"])'
}

after_up() {
  local s
  s=$(synchronous) || s="unread"
  [[ "$s" == 2 ]] || { echo "libsql: the primary runs PRAGMA synchronous=$s, not 2 (FULL): durability_mode=strong didn't take" >&2; return 1; }
}

durability_check() { echo "primary PRAGMA synchronous=$(synchronous || echo unread)"; }

leader() { curl -sf --max-time 2 "http://127.0.0.1:$(port 0)/health" >/dev/null && echo 0; }

version() { "$BIN/sqld" --version; }

help_text() { "$BIN/sqld" --help; }

main "$@"
