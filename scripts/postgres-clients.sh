#!/usr/bin/env bash
# psql and pgbench of the postgres:19 image run the same statements on Amphora's listener and on a 19 server of that
# image (docs/design/postgres.md, section 11's units 9 and 10): pgbench's tables and rows, psql's statements with their
# SQLSTATEs, its \bind, \parse and pipelines, and pgbench's TPC-B script at `-M simple`, `extended` and `prepared`.
# psql's output is compared byte for byte, and the tables pgbench leaves row for row; 32 clients that contend for the
# same rows leave balances that agree (unit 15), and pgbench's own built-in scripts (unit 12) leave the same tables.
# PARTS=builtin runs those alone. Linux, with docker.
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

cat > "$WORK/sql/tpcb-32.sql" <<SQL
\\set bid random(1, 2)
\\set aid random(1, 200)
\\set tid random(1, 20)
\\set delta random(-5000, 5000)
$(sed -n '/^BEGIN;/,$p' "$WORK/sql/tpcb-1.sql")
SQL

cat > "$WORK/sql/tpcb-init.sql" <<SQL
\\set aid random(1, 100000)
\\set bid random(1, 1)
\\set tid random(1, 10)
\\set delta random(-5000, 5000)
$(sed -n '/^BEGIN;/,$p' "$WORK/sql/tpcb-1.sql")
SQL

# libpq's extended protocol as psql 19 drives it: PQsendQueryParams (pgbench.c:868's relkind query with it), PQsendPrepare,
# PQsendQueryPrepared, PQsendClosePrepared, and a pipeline with PQsendFlushRequest, PQpipelineSync and an error in it.
cat > "$WORK/sql/extended.sql" <<'SQL'
SELECT relkind FROM pg_catalog.pg_class WHERE oid=$1::pg_catalog.regclass \bind pgbench_accounts \g
SELECT relkind FROM pg_catalog.pg_class WHERE oid=$1::pg_catalog.regclass \bind nosuch \g
\echo :SQLSTATE :ROW_COUNT
SELECT aid, bid, abalance, filler FROM pgbench_accounts WHERE aid = $1 AND bid = $2 \bind 3 1 \g
SELECT $1::int4 + 1, $2::"text", $3::timestamptz \bind 41 'a b' '2026-11-24 01:02:03+04' \g
SELECT $1::int4 + 1 \bind x \g
SELECT 1 / $1 \bind 0 \g
SELECT bbalance FROM pgbench_branches WHERE bid = $1 \parse branch
\bind_named branch 1 \g
\bind_named branch 2 \g
\bind_named branch \g
SELECT 1 \parse branch
\close_prepared branch
\bind_named branch 1 \g
\startpipeline
SELECT $1::int4 + 1 \bind 1 \sendpipeline
SELECT $1::int4 + 2 \bind 2 \sendpipeline
\flushrequest
\getresults
INSERT INTO pgbench_history (tid, bid, aid, delta) VALUES ($1, 99, 99, 99) \bind 99 \sendpipeline
\syncpipeline
SELECT tid FROM pgbench_history WHERE bid = $1 \bind 99 \sendpipeline
INSERT INTO pgbench_history (tid, bid, aid, delta) VALUES ($1, 98, 98, 98) \bind 98 \sendpipeline
SELECT 1 / $1 \bind 0 \sendpipeline
SELECT 3 \bind \sendpipeline
\syncpipeline
SELECT tid FROM pgbench_history WHERE bid = $1 \bind 98 \sendpipeline
\endpipeline
SELECT tid FROM pgbench_history WHERE bid = $1 \bind 98 \g
BEGIN \bind \g
UPDATE pgbench_history SET tid = $1 WHERE bid = $2 \bind 97 99 \g
SELECT 1 / $1 \bind 0 \g
SELECT 1 \bind \g
ROLLBACK \bind \g
SELECT tid FROM pgbench_history WHERE bid = $1 \bind 99 \g
SQL

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
if [ "${PARTS:-all}" != builtin ]; then
for script in setup statements extended; do
  both "$script" psql -h 127.0.0.1 -p PORT -U ada -d ada -X -a -f "/sql/$script.sql"
  compare "psql -f $script.sql, stdout" "$WORK/$script.ours" "$WORK/$script.theirs"
  compare "psql -f $script.sql, stderr" "$WORK/$script.err.ours" "$WORK/$script.err.theirs"
done
both one psql -h 127.0.0.1 -p PORT -U ada -d ada -X -c 'BEGIN; UPDATE pgbench_branches SET bbalance = bbalance + 0 WHERE bid = 1; SELECT bid FROM pgbench_branches WHERE bid = 1; END'
compare "psql -c of four statements" "$WORK/one.ours" "$WORK/one.theirs"
tpcb() { # <script> <mode> <clients>: pgbench on both, the transactions it commits and the tables it leaves
  local what="pgbench -M $2 -c $3"
  both tpcb pgbench -h 127.0.0.1 -p PORT -U ada -n -f "/sql/$1.sql" -M "$2" -c "$3" -j 1 -t 100 --random-seed=42 ada
  for side in ours theirs; do
    cat "$WORK/tpcb.$side" "$WORK/tpcb.err.$side" | grep -E 'query mode|actually processed|failed|aborted|ERROR|error' > "$WORK/tpcb-counts.$side" || true
    echo "$what, $side: $(tr '\n' ';' < "$WORK/tpcb-counts.$side")"
  done
  grep -q "actually processed: $((100 * $3))/$((100 * $3))" "$WORK/tpcb-counts.ours" || failed=1
  compare "$what's transactions" "$WORK/tpcb-counts.ours" "$WORK/tpcb-counts.theirs"
  for query in "SELECT aid, bid, abalance, filler FROM pgbench_accounts" "SELECT tid, bid, tbalance, filler FROM pgbench_tellers" "SELECT bid, bbalance, filler FROM pgbench_branches" "SELECT tid, bid, aid, delta, filler FROM pgbench_history"; do
    both rows psql -h 127.0.0.1 -p PORT -U ada -d ada -X -At -c "$query"
    sort "$WORK/rows.ours" > "$WORK/rows.sorted.ours"
    sort "$WORK/rows.theirs" > "$WORK/rows.sorted.theirs"
    compare "after $what, sorted: $query" "$WORK/rows.sorted.ours" "$WORK/rows.sorted.theirs"
  done
}
for clients in 1 2; do
  tpcb "tpcb-$clients" simple "$clients"
done
for mode in extended prepared; do
  for clients in 1 8; do
    tpcb tpcb-32 "$mode" "$clients"
  done
done

# 32 clients contend for two branches: none of their transactions fails, and each table's balances move by what the
# history's rows add up to (section 11's unit 15).
moved() { # <port>: the sum of each table's balances, and of the history's
  for query in "SELECT abalance FROM pgbench_accounts" "SELECT tbalance FROM pgbench_tellers" "SELECT bbalance FROM pgbench_branches" "SELECT delta FROM pgbench_history"; do
    client psql -h 127.0.0.1 -p "$1" -U ada -d ada -X -At -c "$query" | awk '{ sum += $1 } END { printf "%d ", sum }'
  done
}
for side in ours theirs; do
  port=$OURS
  [ "$side" = ours ] || port=$THEIRS
  read -r -a before <<< "$(moved "$port")"
  client pgbench -h 127.0.0.1 -p "$port" -U ada -n -f /sql/tpcb-32.sql -M simple -c 32 -j 1 -t 50 ada > "$WORK/pgbench-32.$side" 2>&1 || true
  read -r -a after <<< "$(moved "$port")"
  counts=$(grep -E 'actually processed|failed|aborted|ERROR|error' "$WORK/pgbench-32.$side" | sort | uniq -c | tr '\n' ';')
  moves=()
  for i in 0 1 2 3; do moves+=($((after[i] - before[i]))); done
  echo "pgbench -c 32 -t 50, $side: $counts the tables moved by ${moves[*]}"
  grep -q 'actually processed: 1600/1600' "$WORK/pgbench-32.$side" && grep -q 'failed transactions: 0 ' "$WORK/pgbench-32.$side" || failed=1
  [ "${moves[0]}" = "${moves[1]}" ] && [ "${moves[1]}" = "${moves[2]}" ] && [ "${moves[2]}" = "${moves[3]}" ] || failed=1
done
fi
# pgbench -i of unit 11, its default steps and the server-side ones: the tables it leaves, row for row, its notices and
# output, its keys (by what they refuse), and TPC-B at -c 8 on them.
init() { # <steps>
  local what="pgbench -i -I $1"
  for side in ours theirs; do
    port=$OURS
    [ "$side" = ours ] || port=$THEIRS
    client pgbench -h 127.0.0.1 -p "$port" -U ada -i -s 1 -I "$1" ada > "$WORK/init.$side" 2>&1 || { echo "$what, $side: failed"; failed=1; }
    # The times vary; nothing else does.
    sed -E 's/[0-9]+\.[0-9]+ s/T s/g' "$WORK/init.$side" > "$WORK/init-text.$side"
  done
  compare "$what's output, times aside" "$WORK/init-text.ours" "$WORK/init-text.theirs"
  grep -q 'done in T s' "$WORK/init-text.ours" || failed=1
  for query in "SELECT bid, bbalance, filler FROM pgbench_branches" "SELECT tid, bid, tbalance, filler FROM pgbench_tellers" "SELECT aid, bid, abalance, filler FROM pgbench_accounts" "SELECT tid, bid, aid, delta, filler FROM pgbench_history"; do
    both rows psql -h 127.0.0.1 -p PORT -U ada -d ada -X -At -c "$query"
    sort "$WORK/rows.ours" > "$WORK/rows.sorted.ours"
    sort "$WORK/rows.theirs" > "$WORK/rows.sorted.theirs"
    compare "after $what, sorted: $query" "$WORK/rows.sorted.ours" "$WORK/rows.sorted.theirs"
    echo "  $(wc -l < "$WORK/rows.sorted.ours") rows"
  done
  # Each key and each NOT NULL refuses as on 19.
  for insert in "INSERT INTO pgbench_branches (bid, bbalance) VALUES (1, 0)" "INSERT INTO pgbench_tellers (tid, bid, tbalance) VALUES (1, 1, 0)" "INSERT INTO pgbench_accounts (aid, bid, abalance) VALUES (100000, 1, 0)" "INSERT INTO pgbench_branches (bbalance) VALUES (0)" "INSERT INTO pgbench_accounts (bid, abalance) VALUES (1, 0)" "INSERT INTO pgbench_history (tid, bid, aid, delta) VALUES (1, 1, 1, 1)"; do
    both key psql -h 127.0.0.1 -p PORT -U ada -d ada -X -v VERBOSITY=terse -c "$insert" -c '\echo :SQLSTATE'
    cat "$WORK/key.ours" "$WORK/key.err.ours" > "$WORK/key-all.ours"
    cat "$WORK/key.theirs" "$WORK/key.err.theirs" > "$WORK/key-all.theirs"
    compare "after $what: $insert" "$WORK/key-all.ours" "$WORK/key-all.theirs"
  done
  for query in "ALTER TABLE pgbench_branches ADD PRIMARY KEY (bid)" "ALTER TABLE pgbench_tellers ADD PRIMARY KEY (tid)" "ALTER TABLE pgbench_accounts ADD PRIMARY KEY (aid)"; do
    both key psql -h 127.0.0.1 -p PORT -U ada -d ada -X -v VERBOSITY=terse -c "$query" -c '\echo :SQLSTATE'
    cat "$WORK/key.ours" "$WORK/key.err.ours" > "$WORK/key-all.ours"
    cat "$WORK/key.theirs" "$WORK/key.err.theirs" > "$WORK/key-all.theirs"
    compare "after $what: $query" "$WORK/key-all.ours" "$WORK/key-all.theirs"
  done
}
if [ "${PARTS:-all}" != builtin ]; then
init dtgvp
for mode in simple extended prepared; do
  both tpcb pgbench -h 127.0.0.1 -p PORT -U ada -n -f /sql/tpcb-init.sql -M "$mode" -c 8 -j 1 -t 100 --random-seed=42 ada
  for side in ours theirs; do
    cat "$WORK/tpcb.$side" "$WORK/tpcb.err.$side" | grep -E 'query mode|actually processed|failed|aborted|ERROR|error' > "$WORK/tpcb-counts.$side" || true
    echo "pgbench -M $mode -c 8 on pgbench -i's tables, $side: $(tr '\n' ';' < "$WORK/tpcb-counts.$side")"
  done
  compare "pgbench -M $mode -c 8's transactions on pgbench -i's tables" "$WORK/tpcb-counts.ours" "$WORK/tpcb-counts.theirs"
  for query in "SELECT aid, bid, abalance, filler FROM pgbench_accounts" "SELECT tid, bid, tbalance, filler FROM pgbench_tellers" "SELECT bid, bbalance, filler FROM pgbench_branches" "SELECT tid, bid, aid, delta, filler FROM pgbench_history"; do
    both rows psql -h 127.0.0.1 -p PORT -U ada -d ada -X -At -c "$query"
    sort "$WORK/rows.ours" > "$WORK/rows.sorted.ours"
    sort "$WORK/rows.theirs" > "$WORK/rows.sorted.theirs"
    compare "after pgbench -M $mode -c 8 on pgbench -i's tables, sorted: $query" "$WORK/rows.sorted.ours" "$WORK/rows.sorted.theirs"
  done
done
init dtGvp
fi

# pgbench's built-in scripts (unit 12), which start by asking the count of the branches and the partition query, and by
# a vacuum and a truncate: what pgbench prints but its times, the tables it leaves, and the balances, which move by what
# the history's rows add up to.
sums() { # <port>: the sum of each table's balances, and of the history's
  for query in "SELECT abalance FROM pgbench_accounts" "SELECT tbalance FROM pgbench_tellers" "SELECT bbalance FROM pgbench_branches" "SELECT delta FROM pgbench_history"; do
    client psql -h 127.0.0.1 -p "$1" -U ada -d ada -X -At -c "$query" | awk '{ sum += $1 } END { printf "%d ", sum }'
  done
}
builtin() { # <script> <mode> <clients>
  local what="pgbench -b $1 -M $2 -c $3" side port before after
  for side in ours theirs; do
    port=$OURS
    [ "$side" = ours ] || port=$THEIRS
    read -r -a before <<< "$(sums "$port")"
    client pgbench -h 127.0.0.1 -p "$port" -U ada -b "$1" -M "$2" -c "$3" -j 1 -t 100 --random-seed=42 ada > "$WORK/builtin.$side" 2>&1 || true
    grep -vE '^(latency|initial connection time|tps)' "$WORK/builtin.$side" > "$WORK/builtin-text.$side" || true
    read -r -a after <<< "$(sums "$port")"
    echo "$what, $side: $(grep -E 'actually processed|failed' "$WORK/builtin.$side" | tr '\n' ';') the tables moved by $((after[0] - before[0])) $((after[1] - before[1])) $((after[2] - before[2])), the history is ${after[3]}"
    case $1 in
      tpcb-like) [ $((after[0] - before[0])) = "${after[3]}" ] && [ $((after[1] - before[1])) = "${after[3]}" ] && [ $((after[2] - before[2])) = "${after[3]}" ] || { echo "the balances don't agree"; failed=1; } ;;
      simple-update) [ $((after[0] - before[0])) = "${after[3]}" ] && [ "${before[1]} ${before[2]}" = "${after[1]} ${after[2]}" ] || { echo "the balances don't agree"; failed=1; } ;;
    esac
  done
  grep -q "actually processed: $((100 * $3))/$((100 * $3))" "$WORK/builtin.ours" && grep -q 'failed transactions: 0 ' "$WORK/builtin.ours" || failed=1
  compare "$what's output, times aside" "$WORK/builtin-text.ours" "$WORK/builtin-text.theirs"
  for query in "SELECT aid, bid, abalance, filler FROM pgbench_accounts" "SELECT tid, bid, tbalance, filler FROM pgbench_tellers" "SELECT bid, bbalance, filler FROM pgbench_branches" "SELECT tid, bid, aid, delta, filler FROM pgbench_history"; do
    both rows psql -h 127.0.0.1 -p PORT -U ada -d ada -X -At -c "$query"
    sort "$WORK/rows.ours" > "$WORK/rows.sorted.ours"
    sort "$WORK/rows.theirs" > "$WORK/rows.sorted.theirs"
    compare "after $what, sorted: $query" "$WORK/rows.sorted.ours" "$WORK/rows.sorted.theirs"
  done
}
# Without tables, and with them.
both none psql -h 127.0.0.1 -p PORT -U ada -d ada -X -c 'drop table if exists pgbench_accounts, pgbench_branches, pgbench_history, pgbench_tellers'
both none pgbench -h 127.0.0.1 -p PORT -U ada -b tpcb-like -t 1 ada
sed 's/port [0-9]*/port P/' "$WORK/none.err.ours" > "$WORK/none-text.ours"
sed 's/port [0-9]*/port P/' "$WORK/none.err.theirs" > "$WORK/none-text.theirs"
compare "pgbench -b tpcb-like without tables, stderr" "$WORK/none-text.ours" "$WORK/none-text.theirs"
init dtgvp
for script in tpcb-like simple-update select-only; do
  for mode in simple extended prepared; do
    for clients in 1 8; do
      builtin "$script" "$mode" "$clients"
    done
  done
done
exit "$failed"
