#!/bin/bash
# Lane S5: rqlite's three voters, one rqlited on each of three Mac Studios on the LAN, as amphora-bench's
# --scripts system script (bench/servers/repl/lib.sh's verbs). The orchestrator (s5.js) starts each node as a
# kiln run on its own Mac before the measure and stops it after, so `up` only waits for every node to answer
# /readyz (rqlited's own readiness: it knows a leader) and `down` is the orchestrator's. Bring-up is outside
# the timed region.
#
# Durability is rqlite's defaults (bench/servers/repl/rqlite.sh's account at v10.7.4): a write is acknowledged
# once a quorum (2 of 3) of Raft logs hold it, each BoltDB commit synced; on macOS Go's File.Sync is
# fcntl(F_FULLFSYNC). Writes go to /db/execute?transaction (no queue=1); a follower forwards to the leader.
set -eu
IPS=($RQ_IPS)
PORT=$RQ_PORT
leader() {
  local n id
  for n in 0 1 2; do
    id=$(curl -sf --max-time 2 "http://${IPS[$n]}:$PORT/status" 2>/dev/null | tr -d ' \n' | grep -o '"leader":{[^}]*}' | grep -o '"node_id":"[0-9]*"' | grep -o '[0-9]\+' || true)
    if [ -n "$id" ]; then echo "$id"; return 0; fi
  done
}
case "${1:-}" in
  up)
    for n in 0 1 2; do
      curl -sf --retry 120 --retry-delay 1 --retry-connrefused --retry-all-errors --max-time 2 "http://${IPS[$n]}:$PORT/readyz" >/dev/null
    done
    [ -n "$(leader)" ] || { echo "rqlite named no leader once every node was ready" >&2; exit 1; }
    ;;
  down) ;;
  leader) leader ;;
  endpoint) echo "http://${IPS[$2]}:$PORT" ;;
  kill|start) echo "lane S5 drives commit only" >&2; exit 1 ;;
  about)
    v=$(curl -sf --max-time 2 "http://${IPS[0]}:$PORT/status" | python3 -I -c 'import json,sys; b=json.load(sys.stdin).get("build",{}); print(b.get("version","?"), b.get("compiler",""), b.get("branch",""))' 2>/dev/null || echo unknown)
    python3 -I -c 'import json,sys; print(json.dumps({"system": "rqlite", "version": sys.argv[1], "platform": "native, three Mac Studios (macOS arm64, 14 cores, 36 GiB) on the LAN, one node each; built from the v10.7.4 source tag (no darwin release asset)", "durability": "acknowledged once a quorum (2 of 3) of Raft logs hold it, each BoltDB commit synced (Go File.Sync = F_FULLFSYNC on macOS); rqlited defaults", "column": "quorum", "nodes": sys.argv[2]}))' "rqlited $v" "$RQ_IPS"
    ;;
  *) echo "usage: $0 up|down|leader|endpoint N|about" >&2; exit 2 ;;
esac
