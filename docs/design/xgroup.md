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
key-value space over the groups; SQL sessions across groups are not built (AC-T1.10.1 is open).
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
latest }` (wire version 8; both variants appended after every existing one). `transport::Nodes`
(CP-T1.9, lane split-merge-10's) carries it as it carries a pull: one authenticated connection and
one request, to the endpoint of the group's database, whose owner applies the step as a local one
is applied, one `call_internal` and one atomic commit. The coordinator's newest commit stamp goes
with the step and the group's newest comes back with the reply, so the hybrid logical clock each
step carries (T-T1.12.X2) crosses nodes as it crosses threads. A reply lost after the request went
out is `UnknownOutcome`, which the protocol already treats as a step that may have landed. The node
makes its group's tables before it serves (`sql::prepare`). Values introduced: none; a step's size
is held by the frame's bound, as every request's is. Between machines, a group's owner
also listens at a network address under mutual TLS (`Database::listen`, `Nodes::network`;
reshard.md, "The transport between machines"); the harness's `--listen`, `--cert`, `--key` and
`--roots` use it.

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
