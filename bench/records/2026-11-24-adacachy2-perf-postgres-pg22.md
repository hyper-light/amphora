# Lane PG22 (postgres), 2026-11-24: EvalPlanQual's recheck, deadlocks at deadlock_timeout, and lock_timeout

Hour: 2026-11-24T09:47:10Z (the lane's first command) to 10:47:10Z (wind-down from 10:42Z). Branch `perf/postgres-pg22`
from main 07d95147. Unit 15 of docs/design/postgres.md section 11. Not a speed change: nothing timed, no A/B, every build a debug
build.

## Losses first

- 19's specs don't run unmodified: none of the five named is within the statements Amphora has. What was run is
  permutations of eval-plan-qual (7 of its 56), deadlock-hard (its one) and timeouts (1 of 8), put by hand into those
  statements (the table below), and the rest of the list is the lane's own. Both servers run the same adapted steps.
  Not run, and what each needs: lock-update-delete (`FOR KEY SHARE`, advisory locks, DELETE); update-locked-tuple (REFERENCES, an
  UPDATE of two columns); deadlock-simple (LOCK TABLE: a lock upgrade, found before any wait); deadlock-soft, deadlock-soft-2 and deadlock-parallel
  (LOCK TABLE, parallel query); eval-plan-qual's 49 others (RETURNING beside a wait, DELETE, WITH, ctid, FOR UPDATE, MERGE,
  PREPARE, sub-selects, inheritance and partitions).
- The specs' bodies are fetched from REL_19_0 (below), not vendored: the adapted steps are in the repository.
- A unique key another open transaction inserted (an INSERT's, or an UPDATE's new index entry) still undoes the statement and
  runs it again, as PG21 left it. No step of the list has one.
- Sessions that wait for one row go on in the order they come back to it: 19 queues them by the tuple's lock. No step has two
  sessions that wait for one row.
- 19's refusals of a wait carry a CONTEXT ("waiting for ShareLock on transaction N", "while updating tuple (0,52) in relation "q""), which
  names a ctid Amphora's rows don't have: Amphora sends none, and the comparison leaves CONTEXT out. The detail's process and
  transaction numbers are compared by their order of first use.
- `deadlock_timeout` and `lock_timeout` are the only settings SET takes, their values parsed by hand: no hexadecimal or octal
  number, no exponent beyond Rust's float syntax, no `SET SESSION CHARACTERISTICS`.
- A statement with a sub-select in its WHERE can't be rechecked; the engine refuses it on a connection that waits. No statement
  the front end lowers has one.
- A client that goes away mid-wait is noticed when its wait ends, as 19's is without client_connection_check_interval; CancelRequest
  of a wait is still a later unit.
- DELETE and SELECT ... FOR UPDATE, which lock rows the same way, aren't ported; a REPEATABLE READ refusal for a deleted row still reads
  "concurrent update".
- Not measured: the cost of taking each row of an UPDATE before it is read (one more descent a row).
- Not looked at: what a unique key inserted beside a statement's snapshot does under `Snapshot` validation (19 judges it on the
  newest rows).
- No macOS run counted: the one asked for waited for room from 10:10Z and was stopped.

## What is built

19's rule, read in REL_19_0's C (docs/design/postgres.md section 8 cites the lines), is that a READ COMMITTED UPDATE that waited
for a row whose holder committed a change locks the row's last version and judges the statement's quals on it alone. Amphora does
that at `Insn::LockRow`: the row is taken before the statement reads it, so nothing changes it in between; a later
version is judged by the whole WHERE, read from the table's cursor; the assignments and the index entries that go are that
version's. A wait yields the statement, which the session keeps, its earlier changes kept, and goes on from there. A wait has 19's two
deadlines: the session whose `deadlock_timeout` passes looks for a ring once and is refused `40P01` when its own wait closes one, and a wait
longer than `lock_timeout` is refused `55P03`. A refusal in a block ends the engine's transaction at once, as 19's
does, so its waiters go on before the ROLLBACK.

## Against a PostgreSQL 19.0 server

PostgreSQL 19.0 (Debian 19.0-1.pgdg13+1), the postgres:19 image on adacachy2, a container of the lane's own, removed as each
run ended, its answers by `scripts/postgres-isolation-answers.sh`; `recheck-answers.txt` is md5 8eff6e0911a07e5abe5c431883dc4e54
(answers.txt unchanged, b068e44d4a711a644e23e4de6b9165ed). 30 interleavings, 425 steps, two to eight sessions: 35 waits, 34 ended by
their holder's end, one that waits for a second holder, 3 refused by a deadlock check, 5 by lock_timeout. 21 ErrorResponse,
1 NoticeResponse, 71 DataRow, 76 RowDescription, 368 CommandComplete, 389 ReadyForQuery, all the same (severity, SQLSTATE,
message, position, detail and hint of a refusal). The landing's own list was 29 interleavings and 415 steps (md5
5ecaeb386bc6072fdbb6a8a1660cdeb3); one interleaving came after it, from the red run M3 below.

| Interleavings | Source |
|---|---|
| wx1 wx2, wy1 wy2, committed and rolled back (4) | eval-plan-qual: `accounts` with an int balance and a char(8) key given by ALTER TABLE, no RETURNING, no generated column; what it answers is the spec's own expected output (850, 1100, 1050, 1600) |
| writep3a writep3b, writep4a writep4b | eval-plan-qual, one table for the spec's three |
| simplepartupdate conditionalpartupdate | eval-plan-qual, one table for the spec's partitions |
| deadlock-hard | eight sessions, each updating its own row and then the next's, the spec's `LOCK TABLE` by rows; the last's `deadlock_timeout` 10 ms and the others' 100 s, so the last's check finds the ring |
| timeouts: wrtbl lto update | an UPDATE for the spec's DELETE |
| The key a row is found by changed, and changed and still matching | the recheck reads the table's row, not the index's |
| Rows changed before the wait, a second wait, rows that match only from the commit on, a row passed by that stays locked and is no change | the parts of EvalPlanQual the restart differed in |
| Refused partway (in and out of a block), the error's own transaction ended at once, the newest version's value out of range, a multi-row REPEATABLE READ update | what a refusal undoes |
| Two sessions each waiting for the other's row; the first waiter's check finding the ring the second closed | the victim is the session whose check finds it |
| `lock_timeout` and the two settings: units, SHOW, SET LOCAL, RESET, undone by a rollback, refused values | guc.c |

Specs read, at REL_19_0 (raw.githubusercontent.com/postgres/postgres/REL_19_0/src/test/isolation/specs), sha256 by the first 16 digits:
eval-plan-qual 6aab2e073f0bedd2, lock-update-delete 366de3cc6606ff0c, update-locked-tuple 4c31a28a01f816d2, deadlock-simple be7eafc4fedc27ed,
deadlock-hard 2db1d098446b32e6, timeouts 1fb658b12ec1225b.

T-PG.15.2: the ring found from each transaction in it. T-PG.15.3: over sockets, the two deadlines end a wait with no
other session's help. PG21's T-PG.14.2 is gone: it asserted that a ring is refused at the wait that closes it, which is
not 19's.

## On main as landed (b5584c1e)

psql and pgbench of the postgres:19 image against Amphora and a 19.0 server (`scripts/postgres-clients.sh`, r1816): every comparison of
PG20's matrix the same, and section 11's check of unit 15, TPC-B at 32 clients that contend for two branches, 20 tellers and 200 accounts
(`-M simple -c 32 -j 1 -t 50`; not a timed run): Amphora processed 1600/1600, 0 failed, and the accounts', tellers' and branches'
balances and the history's deltas each moved by 114930; 19.0 processed 1600/1600, 0 failed, each by -103148 (its own random numbers).

## Red runs

The two red runs of T-PG.15.1 are r1702 and r1713 below (grammar the steps then left out). Each part taken out of the landed
code (b5584c1e), one at a time, and `cargo test -p amphora --test postgres_isolation` run on it (r1816, r1832; the engine's `t_pg_15_*` where
that stayed green). Not run to an end in the hour: the statement's snapshot kept apart from the commits it follows (`Txn::reading`),
no ring found, unchanged columns read where the index found the row, a refused statement's changes kept, a rollback that keeps
what SET set; T-PG.15.1 and T-PG.15.2 name what each is meant to catch, unproven here:

| Change | What failed |
|---|---|
| M1: the row isn't judged again (no term of the WHERE in `RowRecheck::emit`) | T-PG.15.1, and T-PG.14.1 at `update c set v = v + 1 where v = 10`: 19 says `UPDATE 0` and the value 5, the change `UPDATE 1` and 6 |
| M2: a row locked and passed by is committed as changed (`put_back_unwritten` does nothing) | T-PG.15.1 at its status comparison: the lock-only interleavings |
| M3: the index cursors aren't read at the newest commit (`read_newest` left out) | GREEN on b5584c1e: no interleaving had a waiter change a key its holder had changed. The one added after the landing ("the key's index entry is the newest version's", 489a0727, answers captured from 19.0, passing on the landed code) makes T-PG.15.1 red on it (r1832) |
| M4: a refusal in a block keeps the engine's transaction open | T-PG.15.1 at `s2<`: the waiter isn't woken by the refused session's error |

## Runs

All on adacachy2 (load 46 to 149), `sync: false`, the GitHub clone (`~/.cache/amphora-perf/postgres-remote`), CARGO_HOME of the lane's,
CARGO_TARGET_DIR ~/.cache/amphora-perf-target/postgres, CARGO_INCREMENTAL=0, nice -n 10. Runs refused for room aren't listed.

| Run | Tree | Command | Result |
|---|---|---|---|
| r1616, 09:47Z, cpus=4, load 34 | 07d95147 | `cargo test -p amphora --test postgres_protocol --test postgres_listener --test postgres_isolation --no-run` | exit 0: the build folder warm |
| r1685, 10:01Z, cpus=2 | ce1d2247 | `cargo check -p amphora-engine -p amphora` | a name used twice (`versioned_cursor`) |
| r1689, 10:02Z, cpus=2 | 002cc04c | the same | no error |
| a foreground run, 10:04Z, cpus=2 | 63b69356 | the capture (exit 0; answers.txt the same bytes) | 1165 lines, md5 31960fa72bed749056237889d98cc543 (the first list) |
| r1702, 10:05Z, cpus=4 | 63b69356 and that capture | `cargo test -p amphora --test postgres_isolation` | RED: T-PG.15.1 at its first statement (a `text` column has no ported grammar action); 2 passed, T-PG.14.1's 149 steps among them on the new path |
| r1713, 10:06Z, cpus=3 | d445b237 | the capture; isolation and listener | RED: an UPDATE without WHERE has no ported action |
| r1720, 10:07Z, cpus=3 | 22d11218 | the same | 3 and 9 passed |
| r1747, 10:11Z, cpus=2 | 219e1767 | the capture (md5 5ecaeb38...); isolation, listener, protocol | 3, 9 and 13 passed |
| r1763, 10:14Z, cpus=3 | f557cc58 | `cargo clippy -p amphora-pg-parser -p amphora-pg-analysis -p amphora-pg-catalog -p amphora-engine -p amphora --all-targets --locked -- -D warnings`; `bash scripts/check-production.sh` | clippy exit 0 (every crate `Checking`); check-production exit 1: `cast_possible_wrap` 675 over the ratchet's 674, one cast in explain.rs, taken out |
| r1778, 10:17Z, cpus=4, load 149 | 176b1c6a | `cargo test -p amphora-engine --lib -- storage::versioned::version storage::versioned::switch storage::versioned::sql storage::versioned::cursor dialect::`; `cargo test -p amphora --lib postgres::` | 74 passed (3,191 filtered out); 4 passed |
| r1783, 10:19Z, cpus=4, load 65 | fbdb04d9, main c0ec09f4 merged | the clippy command; isolation, listener, protocol; the engine command; check-production | clippy exit 0; 3, 9, 13 passed; 74 passed; check-production exit 0, 10616 as the ratchet holds |
| r1800, 10:25Z, cpus=6, load 30 | 76a844ed, main 4b5c78b4 merged (lane C16's engine change) | fmt; `cargo clippy -p amphora -p amphora-engine --all-targets --locked -- -D warnings`; isolation, listener, protocol | fmt 0, clippy exit 0 (both `Checking`); 3, 9 and 13 passed |
| r1809, 10:27Z, cpus=6, load 34 | 73271948, main 58cd91cd merged (lane C12's engine change) | the same | fmt 0, clippy exit 0 (both `Checking`); 3, 9 and 13 passed. The tree that landed |
| r1816, 10:29Z, cpus=4, load 50 | b5584c1e, main as landed | `cargo build -p amphora --bin amphora`; `scripts/postgres-clients.sh`; the red runs above; `bash scripts/check-production.sh` | clients exit 0, every comparison the same, and the 32 clients above; stopped by the lane during the red run after M4 to run the interleaving M3 showed missing |
| a foreground run, 10:36Z, cpus=1 | 37752bd6 | the capture with the added interleaving | exit 0; md5 8eff6e09... |
| r1832, 10:37Z, cpus=6, load 21 | 489a0727 (b5584c1e and the added interleaving) | isolation; the red run M3; `bash scripts/check-production.sh` | 3 passed; M3 red; check-production exit 0, 10600 as the ratchet holds |
| this machine | the branch | `cargo fmt --all --check`, `python3 scripts/test_requirements.py` | pass |
