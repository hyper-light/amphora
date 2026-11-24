#!/usr/bin/env bash
# psql and pgbench of the postgres:19 image run the same statements on Amphora's listener and on a 19 server of that
# image (docs/design/postgres.md, section 11's unit 9): pgbench's tables and rows, psql's statements with their
# SQLSTATEs, and pgbench's TPC-B script at `-M simple`. psql's output is compared byte for byte, and the tables
# pgbench leaves row for row. Linux, with docker.
#
# usage: scripts/postgres-clients.sh <amphora binary> <work folder>
set -euo pipefail
AMPHORA=$(realpath "${1:?amphora binary}")
WORK=${2:?work folder}
mkdir -p "$WORK/sql"
rm -rf "${WORK:?}"/postgres.db*
WORK=$(realpath "$WORK")
NAME=amphora-clients-$$
SERVER=
trap 'docker rm -f "$NAME" > /dev/null 2>&1 || true; [ -z "$SERVER" ] || kill "$SERVER" 2> /dev/null || true' EXIT

python3 - > "$WORK/roles" <<'PY'
import base64, hashlib, hmac
salt, rounds = b"0123456789abcdef", 4096
salted = hashlib.pbkdf2_hmac("sha256", b"pencil", salt, rounds)
key = lambda name: hmac.new(salted, name, hashlib.sha256).digest()
text = lambda raw: base64.b64encode(raw).decode()
print("ada SCRAM-SHA-256$%d:%s$%s:%s superuser" % (rounds, text(salt), text(hashlib.sha256(key(b"Client Key")).digest()), text(key(b"Server Key"))))
PY

python3 - > "$WORK/sql/setup.sql" <<'PY'
# pgbench.c's createTables and initGenerateData, at a scale of two branches with 10 tellers and 100 accounts each.
print("create table pgbench_history(tid int,bid int,aid    int,delta int,mtime timestamp,filler char(22));")
print("create table pgbench_tellers(tid int not null,bid int,tbalance int,filler char(84)) with (fillfactor=100);")
print("create table pgbench_accounts(aid    int not null,bid int,abalance int,filler char(84)) with (fillfactor=100);")
print("create table pgbench_branches(bid int not null,bbalance int,filler char(88)) with (fillfactor=100);")
print("begin;")
for bid in (1, 2):
    print("insert into pgbench_branches(bid,bbalance) values(%d, 0);" % bid)
for tid in range(1, 21):
    print("insert into pgbench_tellers(tid,bid,tbalance) values (%d, %d, 0);" % (tid, (tid - 1) // 10 + 1))
for aid in range(1, 201):
    print("insert into pgbench_accounts(aid,bid,abalance,filler) values (%d, %d, 0, '');" % (aid, (aid - 1) // 100 + 1))
print("commit;")
for table, key in (("branches", "bid"), ("tellers", "tid"), ("accounts", "aid")):
    print("alter table pgbench_%s add primary key (%s);" % (table, key))
PY

# One session's statements of T-PG.9.3's list, each followed by its SQLSTATE and its row count as psql saw them.
python3 - "$(dirname -- "${BASH_SOURCE[0]}")/../crates/amphora/tests/tpcb/listener.txt" > "$WORK/sql/statements.sql" <<'PY'
import sys
lines = open(sys.argv[1], encoding="utf-8").read().splitlines()
# DETAIL and HINT aren't made yet (section 4).
print("\\set VERBOSITY terse")
start = next(i for i, line in enumerate(lines) if line.startswith("s2|"))
for line in lines[start:]:
    sql = line.split("|", 1)[1]
    if "mtime FROM" in sql or sql.strip() == ";" or "BEGIN" in sql and line.startswith("s2|"):
        continue
    if line.startswith("s2|") and not sql.startswith("SELECT"):
        continue
    print(sql.replace("100001", "101").rstrip(";") + ";")
    print("\\echo :SQLSTATE :ROW_COUNT")
PY

for clients in 1 2; do
  cat > "$WORK/sql/tpcb-$clients.sql" <<SQL
\\set client :client_id
\\set bid 1 + :client
\\set aid random(1, 100) + 100 * :client
\\set tid random(1, 10) + 10 * :client
\\set delta random(-5000, 5000)
BEGIN;
UPDATE pgbench_accounts SET abalance = abalance + :delta WHERE aid = :aid;
SELECT abalance FROM pgbench_accounts WHERE aid = :aid;
UPDATE pgbench_tellers SET tbalance = tbalance + :delta WHERE tid = :tid;
UPDATE pgbench_branches SET bbalance = bbalance + :delta WHERE bid = :bid;
INSERT INTO pgbench_history (tid, bid, aid, delta, mtime) VALUES (:tid, :bid, :aid, :delta, CURRENT_TIMESTAMP);
END;
SQL
done

docker run -d --rm --name "$NAME" -e POSTGRES_USER=ada -e POSTGRES_PASSWORD=pencil -p 127.0.0.1:0:5432 postgres:19 > /dev/null
THEIRS=$(docker port "$NAME" 5432/tcp | head -1 | sed 's/.*://')
"$AMPHORA" postgres --listen 127.0.0.1:0 --roles "$WORK/roles" --database "$WORK/postgres.db" 2> "$WORK/amphora.err" &
SERVER=$!
client() { docker run --rm --network host -e PGPASSWORD=pencil -v "$WORK/sql:/sql:ro" postgres:19 "$@"; }
for _ in $(seq 1 120); do
  docker exec "$NAME" pg_isready -h 127.0.0.1 -U ada > /dev/null 2>&1 && grep -q 'listening on' "$WORK/amphora.err" && break
  sleep 0.5
done
OURS=$(sed -n 's/.*listening on .*://p' "$WORK/amphora.err")
echo "19: $(client psql -h 127.0.0.1 -p "$THEIRS" -U ada -d ada -AtqXc 'select version()')"
echo "client: $(client psql --version), $(client pgbench --version)"

failed=0
compare() { # <what> <ours> <theirs>
  if cmp -s "$2" "$3"; then
    echo "same: $1, $(wc -l < "$2") lines, $(wc -c < "$2") bytes"
  else
    failed=1
    echo "DIFFERENT: $1"
    diff "$3" "$2" | head -40 || true
  fi
}
both() { # <what> <command...> with PORT in it
  local what=$1
  shift
  client "${@/PORT/$OURS}" > "$WORK/$what.ours" 2> "$WORK/$what.err.ours" || true
  client "${@/PORT/$THEIRS}" > "$WORK/$what.theirs" 2> "$WORK/$what.err.theirs" || true
}
for script in setup statements; do
  both "$script" psql -h 127.0.0.1 -p PORT -U ada -d ada -X -a -f "/sql/$script.sql"
  compare "psql -f $script.sql, stdout" "$WORK/$script.ours" "$WORK/$script.theirs"
  compare "psql -f $script.sql, stderr" "$WORK/$script.err.ours" "$WORK/$script.err.theirs"
done
both one psql -h 127.0.0.1 -p PORT -U ada -d ada -X -c 'BEGIN; UPDATE pgbench_branches SET bbalance = bbalance + 0 WHERE bid = 1; SELECT bid FROM pgbench_branches WHERE bid = 1; END'
compare "psql -c of four statements" "$WORK/one.ours" "$WORK/one.theirs"
for clients in 1 2; do
  both "pgbench-$clients" pgbench -h 127.0.0.1 -p PORT -U ada -n -f "/sql/tpcb-$clients.sql" -M simple -c "$clients" -j 1 -t 100 --random-seed=42 ada
  for side in ours theirs; do
    cat "$WORK/pgbench-$clients.$side" "$WORK/pgbench-$clients.err.$side" | grep -E 'actually processed|failed|aborted|ERROR|error' > "$WORK/pgbench-$clients-counts.$side" || true
    echo "pgbench -c $clients, $side: $(tr '\n' ';' < "$WORK/pgbench-$clients-counts.$side")"
  done
  compare "pgbench -c $clients's transactions" "$WORK/pgbench-$clients-counts.ours" "$WORK/pgbench-$clients-counts.theirs"
  for query in "SELECT aid, bid, abalance, filler FROM pgbench_accounts" "SELECT tid, bid, tbalance, filler FROM pgbench_tellers" "SELECT bid, bbalance, filler FROM pgbench_branches" "SELECT tid, bid, aid, delta, filler FROM pgbench_history"; do
    both rows psql -h 127.0.0.1 -p PORT -U ada -d ada -X -At -c "$query"
    sort "$WORK/rows.ours" > "$WORK/rows.sorted.ours"
    sort "$WORK/rows.theirs" > "$WORK/rows.sorted.theirs"
    compare "after pgbench -c $clients, sorted: $query" "$WORK/rows.sorted.ours" "$WORK/rows.sorted.theirs"
  done
done
exit "$failed"
