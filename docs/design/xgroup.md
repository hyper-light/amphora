# Transactions across groups (CP-T1.10; G13 step 3; lane xgroup-txn, 2026-11-21)

`crates/amphora/src/xgroup.rs` and `xgroup/`. A transaction reads and writes keys in several fixed
groups and commits in all of them or none (AC-T1.10.5, T-T1.10.1). The step asks for two prototypes,
behind a switch (`Protocol`), the faster kept at zero anomalies.

## The two protocols

- **Two-phase commit over groups** (Spanner, OSDI 2012, section 4.2.1, "Running two-phase commit
  over Paxos mitigates the availability problems"): each participant group locks its keys and holds
  the writes (prepare); then the anchor group records the decision; the client hears after two
  group commits one after the other. An abort needs no record: only the coordinator could have
  decided commit, and a recovery that finds no decision aborts.
- **Parallel Commits** (CockroachDB, SIGMOD 2020, section 3.2: "a staging transaction status which
  makes the true status of the transaction conditional on whether all of its writes have been
  replicated"): the anchor writes a staging record naming the participants in the same commit as its
  own locks, beside the other groups' locks; the transaction is committed once every lock is in
  place, so the client hears after one group commit. The explicit commit and the resolutions go out
  together afterwards. An abort must turn the staging record aborted before anyone hears so.

Both share every group-side rule (`apply`). Concurrency control is two-phase locking, no-wait: a
step that meets another transaction's conflicting lock is refused whole and its transaction aborts
(no deadlock, no waiting; a waiting or wound-wait policy is a later choice, measured). Reads take
shared locks, writes exclusive ones, all held until resolution, so a committed transaction held all
its locks at once (conflict-serializable, and strictly so: the lock point precedes the answer).

## Crashes

Every decision lives in a group; the coordinator keeps nothing. An incarnation is an epoch the
registry group hands out (`Step::Begin`). A new incarnation scans every group (`Step::Scan`), which
also fences the older ones there: a group refuses any later step from an older epoch (`Fenced`), so
the scan's list of what the old incarnation left is complete. It then finishes each transaction:
`Recover` at the anchor answers the decision, aborts one with no record, or returns a staging
record's participants; `Prevent` at each participant answers whether it holds the locks and, if it
doesn't, makes sure it never will (a done row). All present: committed (Parallel Commits' implicit
commit); any absent: aborted. Every non-lock step is idempotent and is sent again when its outcome
is lost; a lost lock aborts its transaction.

A group refuses a late step of a transaction it has finished (its done row) and of any below the
coordinator's floor, the oldest transaction it still runs, which every step carries once recovery
is done; done rows and decided records below the floor are deleted, so they are bounded by what the
coordinator runs at once.

## On a group's database

`xgroup::sql`: each step is one `Database` call (`call_internal`, the owner's own session, able to
name `__amphora_xg_*`): one atomic commit in its group, committed by its quorum on a replicated
group before it answers. The data the prototype's transactions act on is `__amphora_xg_data`, a
key-value space over the groups; a SQL session commits its rows through `Cluster::commit` (below).
`Cluster` runs one coordinator on a driver thread; the route from key to group is the caller's, so
the routing lanes keep it. Values introduced: none tuned. Bounds are the API's own (`TX_MAX`,
`KEY_MAX`, `VALUE_MAX`, `REQUEST_MAX`, `SCAN_MAX` for a scan page); the transactions in flight are
the caller's (its threads calling `transact`). Until the client hears, each step's deadline is the
caller's for that transaction. After, each resolution step gets the steps' learned timeout: the
driver times every step answered at its first send and keeps a smoothed mean and mean deviation as
TCP keeps a round trip's (RFC 6298 §2; Karn's rule for resends), giving mean + 4 × deviation,
doubled for each resolution step lost (RFC 6298 §5.5). It follows the groups where they run and as
their load moves; nothing in it is tuned. A step past its deadline stops the incarnation, as a crash
would, and the next open finishes it.

A begin that finds the coordinator full (its `in_flight` transactions running, recovered ones
among them) waits, parked, and begins in arrival order as transactions finish (lane xgroup-lan-2).
A caller's next transaction often arrives while its last one still resolves, after the caller
heard; refusing it `Full` would make every caller spin. At most `in_flight` begins wait, the
callers the shell was opened for; one more is refused `Full`, and one whose caller's deadline
passed while it waited is answered `Timeout` and never begun. Values introduced: none.

## A SQL session's commit (lane xgroup-sql, AC-T1.10.1)

`Cluster::commit` takes a session's transaction, the rows it read and wrote by key. Rows all in one
group this process holds commit in that group's one database transaction, without a coordinator step:
the fast path, one group commit and no record. A row another transaction holds a lock on (a prepare
or a staging write not yet resolved) aborts it `Conflict`, as a lock step would, so a cross-group
transaction's held rows are never written under it. Rows in several groups, or a group reached over
the transport, commit through the coordinator (`Cluster::transact`), under either protocol. Answers
are the same as the same statements on one group, reads compared as a set (the coordinator answers
them as their groups reply): `xgroup::sql::sessions`, seeded, both protocols. Values introduced: none.

What is left: the grouped database (Database over Groups, lane split-merge-13) refuses a batch with
rows in more than one group (`Refused::Groups`); that refusal is where its SQL session hands the
batch to `Cluster::commit`, once its row keys are the coordinator's keys. Found by lane
xgroup-sql-5 (2026-11-22): no `Database` runs over `Groups` yet (its writer is one `Store<Local>`,
which never refuses `Groups`; only the tests' `Groups<Local>` does), and handing the rows over today
would be wrong in two ways. The coordinator writes `__amphora_xg_data`, not the records tables a
group's `get`, `scan` and change feed read, so a committed row would not be read back; and a
single-group Records batch commits without looking at `__amphora_xg_lock`, so it could write under a
cross-group transaction's lock. A Records batch's conditions (`Absent`, `At`) also have no place in
the coordinator's transaction, which reads and writes but decides nothing on what it read. The
hookup needs the Tablet over the records tables (revisions and feed rows written as a Records commit
writes them) and the records commit checking the locks. A group failing mid-commit
is tested on the coordinator (T-T1.10.X3), not yet through a SQL session over real groups. The fast
path refuses a row another transaction only reads; the coordinator's lock rules decide that case the
same way today (any lock conflicts).

### Reading its own writes across groups (lane xgroup-sql-8, AC-T1.10.1)

A transaction across groups is decided before its groups apply it: each group writes its rows (or
drops its locks, for an abort) when it resolves it, one round of steps after the decision. So when
the caller hears decides what it can rely on, and `Options::answer_at` switches it:
- `AnswerAt::Decided`, the control and today's: the answer comes at the decision (`Event::Answer`). A
  read straight after it can miss the rows in a group that hasn't resolved it yet, and a retry
  straight after an abort can run into the transaction's own locks.
- `AnswerAt::Resolved`: each answer, aborts included, is held until every group it touched has
  resolved it (`Event::Finished`). The caller waits that one more round. Held answers are counted
  (`Counted::answers_held`, `answers_held_most`). If the coordinator stops while it holds one, the
  caller hears `UnknownOutcome`, never "stopped", since its rows may not be readable yet.

`commit_records` answers a token: the newest closed time any touched group published once the commit
answered (`Database::session_token`; Ada's decision: the commit's revision across groups is the
cross-group hybrid-logical-clock timestamp, the same one the read-your-writes tokens use, `Token::at`).
Under `Resolved` every group's resolution is in its closed time by then, so a read that carries the
token reads the rows on any replica. A group on a node adds its closed time too (lane xgroup-sql-9,
wire version 9): its owner answers each step with the group's closed time after it, a session
token's microseconds (`Answer::Stepped`'s `closed`, none for a group that keeps none), and the
cluster keeps the newest heard from each group on a node. Its last step resolved the commit there
before a `Resolved` answer comes, so that time is at or past the commit's rows in that group
(`t_t1_10_w4_*`). A session reading at a follower behind the commit waits, on its owner's
publication, until the follower has applied the token's time, or is redirected to the leader past
the follower's measured apply lag; it never answers without the commit (`t_t1_10_w5_*`, a follower
cut off before the commit and reached again while the read waits).

Every committed answer carries that stamp (lane xgroup-sql-10, for lane split-merge-20's conditional
commits across groups): `Answer::Committed`'s `at`, an `Option<u64>` of the same microseconds, the
last field. The sans-IO coordinator has no clock, so its shell stamps the answer (`Cluster::stamped`),
the cross-group path and the one-group fast path alike, from the groups the transaction touched, read
once it answered; none when no group it touched keeps a closed time. `commit_records` answers
`Token::at` of it. A stamp that can't be read by the caller's deadline leaves the caller no token past
the commit's rows, so the caller hears `UnknownOutcome`, as for a commit whose answer was lost
(counted, `Counted::stamps_unread`; `t_t1_10_w6_*`).

Each mode's failure mode is forced in `xgroup::sql::sessions`: one group's resolution held back (a
group behind the decision), a read of it straight after the answer
(`ac_t1_10_1_a_read_straight_after_the_answer_in_a_group_behind`), and an abort with every lock held,
retried at once by a second caller (`ac_t1_10_1_a_retry_straight_after_an_abort_meets_no_lock_of_its_own`).
At scale, `xgroup_lan ryw` runs both modes under both protocols, callers writing only their own rows
so every conflict met is their own earlier transaction's lock. It reports read-your-writes misses,
conflicts met, the commit's p50, p99 and p99.9 (its tries again included), the answers held at most,
and throughput. Which mode stays is decided from those numbers, not yet measured.

The alternative, reads that wait on a held lock (lane xgroup-sql-10, option 2 of xgroup-sql-7's), is
built as a sibling switch on each group's database, `Database::set_locked_reads`:
- `LockedReads::Committed`, the control and today's: a Records get of a row a cross-group transaction
  holds a lock on answers the row as last committed there.
- `LockedReads::Wait`: the get looks the lock up first (one indexed lookup in `__amphora_xg_lock` on
  every read of a latest row, single-group reads included; a read at an older revision never waits).
  Under a lock it answers nothing and waits on the group's change event, which each step and each
  commit there moves, never a timer, then reads again; the transaction's write once it committed, the
  row before it once it aborted. The change counter is read before each try, so a resolution landing
  between a try and the wait can't be missed. It is refused `Timeout` at the read's deadline. Reads
  that waited are counted (`Database::locked_waits`).

With `AnswerAt::Decided` this moves the wait from the commit to the read that needs it: the caller
hears at the decision, and only a read of a row still under the lock waits. It does nothing for a
retry straight after an abort, which still meets the transaction's own locks in its commit; nor, yet,
for scans, SQL reads or a session's reads inside the writer. Its failure mode at scale is the change
event: every waiting read tries again on every change in its group, its own resolution or not.
`t_t1_10_w7_*` forces the three settings against one held resolution (the control misses the row,
`Resolved` holds the answer, `Wait` holds the read) and prints what each adds; `t_t1_10_w8_*` holds
a read on an aborted transaction's lock and finds the row as it was before; `xgroup_lan ryw` runs
`locked-wait` beside `decided` and `resolved`, with the reads' p50, p99 and p99.9 and the reads that
waited.

## A refusal before the step (lane xgroup-sql-4, AC-T1.10.6)

A group that refuses a step before taking it (`NotLeader`: its leader moved; `Full`: its writer
admitted nothing) ran nothing of it. What the driver does then is a switch, `Options::on_refused`,
until the coordinator's live runs decide:
- `Stop`, today's and the control: the step counts as lost, as one whose answer never came. A lost
  lock step aborts its transaction; any other is sent again at once, and a step past its deadline
  stops the coordinator.
- `Retry`: the same step is held and sent again after the steps' learned timeout (RFC 6298 §2, the
  same estimator the resolution uses; the refusal itself, answered at a first send, is a sample),
  doubled for each refusal of that step (§5.5), while that comes before the step's deadline; past
  it, as `Stop`. The transaction keeps its locks while its step is held.

`Cluster::counted` gives the instruments: refusals by kind, retries, steps and transactions held
now and at most, coordinator stops, and each answered transaction's time (p50, p99, p99.9 from the
LAN harness's log-linear buckets). `Options::fault` is the seam the scenarios use: a group refusing
a transaction's steps as a sick or restarting group would. `xgroup::sql::sessions` runs one group
that keeps refusing and all groups refusing at once, both protocols, under both settings: retried,
the transaction commits with no stop; under the control, it aborts. Values introduced: none.

A group failing mid-commit is also tested through a session over three real groups:
`ac_t1_10_1_a_group_failure_mid_commit_through_a_session_is_all_or_nothing` fails each group at
each of its steps in turn, reopens the coordinator, and checks the rows are all written or none and
read as on one group that ran the transaction or didn't, under both protocols.

### At scale (lane xgroup-sql-5)

`xgroup_lan refusals` (the LAN harness's example) runs the switch under many callers and correlated
refusals: `--clients` callers each run `--txns` transactions (one read, two writes, mostly across
groups) while every group refuses together, as a fleet restarting at once: of each `--every` steps
sent, the first `--burst` are refused (`--refusal not-leader` or `full`) before they are taken. The
groups are `--node`s over the transport or `--groups` local databases under `--dir`. Each run is one
JSON line per protocol and setting: outcomes (committed, aborted by why, unknown, failed), the
coordinator's instruments summed over its incarnations (a coordinator that stops is opened again, as
its process would be, and counted), and the committed transactions' p50, p99 and p99.9. The scenario's
sizes are its inputs, not tuned values. Which setting stays is the coordinator's live runs' to decide.

## A group's failure

A group that fails mid-commit (its quorum lost, or its owner's node gone) refuses the step's call,
a refusal the protocol can't retry (`Unavailable`; a step whose answer was lost after it went out
is `UnknownOutcome` and is retried). The driver stops that incarnation as a crash would: its waiting
callers hear `Unavailable`, and nothing it decided is lost, since every decision lives in a group.
The next open, once the group answers again, scans every group and finishes each transaction left:
committed everywhere or aborted everywhere, never in part. `t_t1_10_x3_*` fails each group (the
registry and anchor, and each participant) at every one of the coordinator's inputs, with the step
in flight landing first or not, under both protocols (T-T1.10.X3).

## Over the transport

A group on another node is reached as `Member::Node` (`xgroup::sql`): each step is one request over
the owner transport, `Request::Step { here, carried, envelope }`, answered `Answer::Stepped { reply,
latest, closed }` (wire version 8; both variants appended after every existing one; version 9 added
`closed`, below in "Reading its own writes across groups"). `transport::Nodes`
(CP-T1.9, lane split-merge-10's) carries it as it carries a pull: one authenticated connection and
one request, to the endpoint of the group's database, whose owner applies the step as a local one
is applied, one `call_internal` and one atomic commit. The coordinator's newest commit stamp goes
with the step and the group's newest comes back with the reply, so the hybrid logical clock each
step carries (T-T1.12.X2) crosses nodes as it crosses threads. A reply lost after the request went
out is `UnknownOutcome`, which the protocol already treats as a step that may have landed. The node
makes its group's tables before it serves (`sql::prepare`). Values introduced: none; a step's size
is held by the frame's bound, as every request's is. The listener is still loopback: reaching
another machine waits on split-merge-11's network listener, which `Nodes` addresses through
unchanged.

The harness that chooses the protocol is `xgroup::lan` with its command line,
`crates/amphora/examples/xgroup_lan.rs`: `node` serves one group at an endpoint; `bench` runs
transactions each spanning `--span` groups through each protocol for `--secs`, `--clients` at once
over `--keys` keys per group, and writes p50, p99, p99.9 commit latency and commits a second to
bench/records (latencies in fixed log-linear buckets, HdrHistogram's way, under 1% high); `elle`
runs `--seeds` seeded histories and checks each Elle-style, as the simulator's runs are checked. The
seed fixes the workload; the interleaving is the network's.

## Decision (lane xgroup-lan): which protocol stays, the faster at zero anomalies

The rule (G13 step 3): of two-phase commit over groups and Parallel Commits, the protocol that stays
is the one with the lower cross-group p99 on the LAN among those with exactly 0 anomalies over
10,000 Elle seeds over the transport; the other is removed after this record is filled in. A
protocol with any anomaly is out whatever its speed. If neither has a p99 at most 10 ms, neither
meets AC-T1.10.2 and the faster is kept only as the step's state, with AC-T1.10.2 open.

The coordinator's measurement (blanks to fill):

| | two-phase | Parallel Commits |
|---|---|---|
| hosts, groups, clients, keys per group, span, seconds | ___ | ___ |
| p50 / p99 / p99.9 commit latency (ms) | ___ / ___ / ___ | ___ / ___ / ___ |
| commits a second | ___ | ___ |
| aborted / unknown | ___ / ___ | ___ / ___ |
| anomalies over 10,000 seeds (unknown outcomes) | ___ (___) | ___ (___) |
| record | bench/records/___ | bench/records/___ |

Decision: ___ stays; ___ is removed (commit ___). Expected from the simulator: Parallel Commits,
one group commit before the client hears against two; falsified if its p99 is not below two-phase
commit's on the LAN, or if it shows an anomaly two-phase commit doesn't.

Until the coordinator's numbers are in (lane xgroup-lan-2, 2026-11-22): both protocols stay behind
`Protocol`, the reversible option. Removing one before the LAN run would decide on the simulator's
round counts alone; keeping both costs one switch, and both pass every crash, group-failure and
seeded test. The evidence so far favours Parallel Commits (one group commit before the client hears
against two, `ac_t1_10_2_parallel_commits_answers_after_one_group_commit_two_phase_after_two`).

## Evidence so far

- In the simulator, Parallel Commits answers after one group commit and two-phase commit after two
  (`ac_t1_10_2_parallel_commits_answers_after_one_group_commit_two_phase_after_two`): Parallel
  Commits is expected to be the faster by one group commit per transaction. The step's decision
  waits for the LAN measurement (AC-T1.10.2, bench `amphora_txn` on multi-node groups), which needs
  the host to carry replication between machines.
- A coordinator crash after each of its inputs, the steps out then landing or lost, and the
  recovering incarnation crashed again at each of its own, ends committed or aborted everywhere,
  matching what the client heard (T-T1.10.1).
- Seeded runs with lost, late and reply-lost steps and coordinator crashes, checked Elle-style
  (Kingsbury and Alvaro, PVLDB 14(3)): read-modify-writes of unique values give each key's version
  chain; a lost update, an aborted or unwritten read, a partial commit, or a cycle in ww, wr, rw and
  real-time edges is an anomaly.

## Metrics (proposed; names to agree with kiln's Observe owner)

`amphora_xgroup_committed_total`, `amphora_xgroup_aborted_total`, `amphora_xgroup_unknown_total`,
`amphora_xgroup_recovered_total{outcome}`, `amphora_xgroup_retried_total`, from `Coordinator::counts`.

## Falsified by

A seed with an anomaly; a crash point that leaves a lock, a staging record, or a partly applied
transaction; or, on the LAN, Parallel Commits' p99 not below two-phase commit's.
