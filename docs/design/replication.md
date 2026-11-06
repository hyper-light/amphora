# Resolved transactions through hyper-durable (U15, CP-T1.8)

Kiln's unification decision on 2026-10-29 calls for one logical Amphora database, with namespaces
rather than application-managed databases. A namespace is not a replication group. Atomic SQL and
Records state must not be split across independent groups.

A client SQL session can make adaptive decisions, invoke triggers and choose random/time defaults.
Replaying its SQL on followers would repeat those decisions and can produce different rows. The
leader therefore executes the session inside an unpublished engine transaction, captures its dirty
page images (including spilled pages), and rolls back. A Raft entry contains only the resolved
pages and the index/term of the state the session observed. An entry with a stale base is refused;
its index still advances durably, without applying its pages. The owner serializes preparations
until their proposals resolve. User answers held beside those proposals must only leave after the
matching applied result succeeds.

`replication::Machine` implements the existing hyper-durable StateMachine. It inserts imported
pages without commit markers, writes the applied index/term/membership in the same engine
transaction, commits and invokes the engine's real WAL flusher. Only then does apply return an
answer to hyper-durable. All entries act at restart, so hyper-durable's durable-commit fence covers
them. The Raft log is hyper-log through hyper-durable GroupStore; no Raft algorithm is implemented
in Amphora. A speculative preparation never publishes a local commit, so a leader that loses its
term cannot leave that preparation visible after restart.

The replicated owner admits an application request with an unknown leader into its existing
bounded waiting queue, retaining its original absolute deadline and admission charge. It dispatches
only after discovering a leader. If the term or leader changes before application evaluation,
the same untouched operation returns to the front of that queue and must pass a new context and
ReadIndex. This deliberately replaces immediate unknown-leader refusal with bounded waiting;
expiry returns Timeout, or UnknownOutcome if the client's existing receive deadline wins.

The retry boundary ends before `prepare_db` begins and before a SQL session's `FnOnce` is taken.
An evaluated session is never replayed, including one that fails, captures no pages or cannot
propose. Proposed changes and bootstrap seeds retain their UnknownOutcome semantics on leadership
loss, displacement or deadline. A stale read completion cannot turn a proposal into a new operation.
Queue/core-read admission can still refuse work at its existing bounds; no deadline is extended.
Application admission keeps one maximum wire record's bytes free within the existing shared byte
budget, so waiting calls alone cannot prevent replication traffic from entering the same actor.
Already admitted wire traffic can still fill that bounded budget and cause an honest refusal.

The implementation currently retains the engine WAL for atomic application of committed Raft
pages. It does not yet eliminate that apply journal in favor of the Raft log alone. Transactions
are limited to 32 MiB of resolved pages, snapshots to 256 MiB. Larger snapshots are explicitly
refused until a streaming snapshot protocol exists. Snapshot installation atomically imports the
image and applied marker, flushing before hyper-durable may move the log's start.

The host must still wire this machine to the hyper-durable Replica, GroupStore and a bounded
transport owner, driving quorum ReadIndex before serving reads. `Database` currently remains the
local-durable API; its writer must be replaced by that owner before replicated operation is
advertised. No application writer may use the same database alongside a Replica-owned Machine.
Membership changes and recovery use hyper-durable; the host must not synthesize committed indices
or infer quorum durability from a local flush. Missing owner/transport integration is not an
optional degraded mode and is not U15 completion.

Evidence: the existing hyper-durable contract states the durable commit fence, configuration and
snapshot ordering in its [StateMachine and Replica implementations](https://github.com/hyper-light/hyper-raft/tree/4d72e19b9533851e19602eee57053a5a7337b2cd/crates/hyper-durable/src).
Raft's state-machine safety and log compaction rules are described in
[Ongaro's dissertation, chapters 3 and 5](https://raft.github.io/raft.pdf).
The engine's current transaction dirty-page set, spill completion and raw WAL insertion form the
physical capture/apply boundary; `db::Db` preserves that boundary instead of replaying SQL.

Falsified by: any unproposed captured change surviving restart; two replicas applying one entry
with different random/default/trigger output; an acknowledged applied index reopening without its
rows or membership; a stale preparation changing state; or snapshot installation making a new
index durable before every image page. The focused persistence tests exercise these boundaries;
quorum, leader-loss and host integration remain separate required gates.

## The owner's time (2026-10-31)

The replicated owner reads time from one clock, its shell's (`Clock`). It reads it at the start of
each step of its loop (`Actor::run_step(input)`), and again after each preparation, because a
preparation runs the caller's own work, which takes time of its own. Every time the owner reads is
its latest reading `at`:
- the tick (`run_step`, one tick per `TICK` of `at`, never a catch-up);
- the clock hyper-durable is driven with (`now()`: `at` less the owner's start, its clock's first
  reading);
- the leadership-transfer period and its stamp (`bootstrap`);
- the bootstrap's internal sixty-second lease (`bootstrap`);
- every deadline check: the waiting queue and the pending operation in `expire()`, `requeue()`,
  a wire record's and a call's deadline in `receive()`, and the pending operation's deadline in
  `prepare()`, at its read completion and after its preparation;
- the SQL deadline that interrupts a call's work (`Db::deadline_on`: the engine's progress handler,
  every 1,024 VM instructions), which reads the clock itself.

The base 327a8f9 read the real clock after a preparation. This decision's first version (2e7728d)
judged that deadline at the step's instant instead, which in production can't see the work's time:
a call whose work outlasted its deadline was proposed, and one its SQL deadline interrupted answered
the interrupt's error, not Timeout. `a_call_whose_work_outlasts_its_deadline_is_refused_and_never_proposed`
failed that way, and passes once the owner reads its clock after a preparation (kiln goal
557f980dfca48b428bbbb4687324653f).

A compaction step's own deadline checks read the real clock, as the local database's do
(`compaction::ready`, `compaction::deadline_result` and the step's SQL deadline). On the owner
thread the two clocks are one. In a test, a call's deadline is an hour ahead of the real clock, so
those checks never fire there: they are hang guards.

The production shell is the owner thread. Its clock is the real one (`Instant::now`), and before
each step it waits on the bounded channel (`recv_timeout(POLL)`) for the next input. The handle's
caller-side reads stay on the real clock, because they are the caller's own deadlines:
- `submit`, and `receive` with its `enqueue_receive`, before enqueueing and while waiting for the
  answer;
- `ReceiveReceipt::try_result`;
- `start` and its `open`, which make the owner, on the caller's own deadline. `start` also gives the
  owner thread its clock.

A test shell holds all of a group's owners on the test's own thread. Each owner reads its own
virtual clock, as a machine reads its own: the real clock at the shell's start plus an hour, plus an
offset that only the test moves, between steps or from a call's work while its owner runs it, as
slow work moves the real clock. It relays wire records between steps.
- It elects by having one owner campaign (`Replica::campaign`) while every clock stands still.
  Ticking every owner instead raced their election timers against the candidates' persisting,
  because vote requests too wait for their writes, and split the votes for a dozen terms.
- It holds every clock still through a scenario. No tick, heartbeat or quorum check happens then,
  so no election does either, whatever the disk's latency.
- A nemesis forces an election at a chosen point: the leader hands the lead to another owner
  (`Replica::transfer`, which no lease refuses), or the test advances chosen owners' clocks past
  their election timeout or the leader's quorum check.
- Before a restart, it advances every clock past hyper-durable's quiet period, so each owner states
  its durable commit.

A test caller enqueues its call and polls the answer between steps; it never blocks on a channel.
Its deadline is a virtual instant, so it expires only when the test advances a clock past it. The
hour's lead keeps any real-clock check of a test deadline (a receipt's poll) true for an hour. A
round with nothing to relay yields `POLL` to the disk, and a counted budget of rounds, about a
minute of such yields, calls a scenario hung. Both are hang guards, and no test's verdict depends on
them. The three-owner scenarios move to the test shell. Their thread-shell helpers go with them:
the relay's blocking call, `seeded_nodes` and `known_leader`, with their five-second budgets of the
real clock. `expired_sql_rolls_back_before_actor_continues` moves too: its call's work moves its
owner's clock to the deadline before a query of ten million rows, which the SQL deadline interrupts
at its first check. The single-owner tests move too (kiln goal 08cac9bdacc7869b87d6b7f02afd0b71). A
lone member of three answers the leaderless tests, and a single voter, elected by its own campaign,
answers the rest. A call waits for its deadline until its owner's clock reaches it. Admission is
tested from inside a call's work, while the owner runs it. The thread shell's helpers go with them:
`deadline()`, `open()`, `leader()` and `call()`. The tests read the real clock only for the hour's
lead, a hang guard, and for deadlines already past when read, which answer the same at any load.

Production keeps one code path (CLAUDE.md section 8). The step is the same function, and the
production shell adds only its clock, `Instant::now`, and the channel wait.

The owner elects on its ticks, in hyper-raft's Ticks mode: 10 ms ticks, a campaign after 20 to 40
ticks without a leader, and a quorum check every 20. Raft's broadcast time includes each member's
persisting, and a member answers only after its write (hyper-durable sends a Ready's persisted
messages after its write). On a loaded machine, then, a follower's slow write silences its answers
past the leader's quorum window, and the leader steps down. A follower that misses heartbeats for an
election timeout campaigns. In both cases a proposal in flight answers UnknownOutcome, as the
paragraph on retry boundaries above requires.

follower_origin_captures_once_and_reads_after_local_durable_apply and
autonomous_quorum_seed_preserves_unchanged_pages_then_delta_and_restart took those answers for
failures, and failed at the base 327a8f9 under load: 3 of 36 full-suite rounds at load 26-96. Their
five-second waits also ran out, at the election, at a call, and at an owner's start.

On driven time the same tests assert what they asserted before, with no election during a
scenario. A nemesis variant forces an election at each step instead and asserts what must hold
anyway:
- never two Oks for the two captures, and never two of their rows;
- every answer is Ok, the stale capture's refusal, or UnknownOutcome;
- the seed's original image is intact on every replica.
This is the leader-loss gate listed above as still required.

This change gave production owners no patience: a follower's slow write deposed a sound leader, a
liveness cost, never a safety one. Whether an owner gives hyper-raft its patience and quorum
patience from the stalls it measures was decided on measurements of owners' stalls under load
("The leader's quorum patience", below). The single-owner tests move to the test shell in kiln goal
08cac9bdacc7869b87d6b7f02afd0b71.

Evidence: kiln plan 01a18f5daea470a78f8009c26fb9ab9b and its research
01a18f5daeab73f9be931a367c7e7759 (the failures and their messages at 327a8f9, f961f4b and
7780186; the traced step-downs); hyper-durable's time comes only from its owner ("One tick of the
owner's period", "`now` is the owner's monotonic clock", replica.rs); etcd's raft keeps time as an
abstract tick its user drives so that its behavior is deterministic (etcd-io/raft README);
FoundationDB's simulator abstracts network, disk, time and randomness so that every failure
reproduces (Zhou et al., SIGMOD 2021, section 4); Raft's availability depends on timing, and its
safety must not (Ongaro and Ousterhout 2014, section 5.6).

Falsified by:
- a scenario whose outcome differs between the test shell and real threads for the same order of
  inputs;
- a term change during a scenario with no nemesis;
- a test whose verdict still depends on the real clock;
- an owner's read of time that isn't its clock's, other than a compaction step's own checks;
- a call whose work outlasts its deadline that is proposed, or that answers anything but Timeout.

## The leader's quorum patience (2026-11-06)

Kiln goal 6b112f14729c899e826452260ca618c1, step 2, decided on step 1's record of the owners'
stalls (`docs/research/owner-stalls.md`; per run and member, and per host and condition at best,
median and max, in `bench/records/2026-11-06-owner-stalls.jsonl`, made from it by
`bench/owner_stalls.py`). Every number quoted here from a run is in that record.

### What the record shows

A leader checks, every election timeout of 20 ticks of 10 ms, whether a quorum answered it since
its last check, and a follower answers only after its write. On adacachy under load (the runs whose
load medians were 49.9, 52.3 and 40.3), each member was stepped down by its own quorum check 63 to
113 times in a run of 1,800 s, the group moved through 473 to 655 terms in a run, and 241, 223 and
188 writes were refused beside 6,464, 3,824 and 8,990 acknowledged. On jupiter the same check
stepped members down 7 to 34 times a run. Raft's timing requirement, broadcast time well under the
election timeout (Ongaro's thesis §3.4), fails on these hosts on the followers' side:
- a log write's p99 from submission to durable was 474.688 to 2571.589 ms in the loaded runs,
  against a window of 200 ms, and its maximum 22200.560 ms;
- one follower's answers reached 201.524 ms at p99 (loaded run 2, member 1's answers from 2), and
  each follower's answers came more than 200 ms apart 84 to 170 times in a loaded run (13 to 174
  on jupiter);
- the followers' owners stalled, not only their disks: a step gap reached 9477.636 ms (loaded run
  2, member 3).

The record's answer gaps are on the clock and carry the leader's own stalls: in loaded run 1,
member 1's tick gap reached 4075.419 ms and its followers' answer gaps 4138.317 and 4139.250 ms. A
leader's own stall counts one tick (no catch-up, "The owner's time" above) and never deposes it, so
those gaps overstate what its check judges.

### The options

- **Fixed ticks**, Raft's and etcd's stance for the leader's check (thesis §6.2). The record is its
  cost: a sound leader deposed whenever both followers' answers outlast a window, each step-down
  answering the proposals in flight UnknownOutcome. A longer election timeout instead would slow
  the replacement of a dead leader too, which AC-T1.8.4 bounds (failover p99 at most 1000 ms) and
  which the followers' timers decide; the check is the leader's alone. Not kept.
- **Heartbeat answers that leave ahead of a follower's writes**, etcd's asynchronous storage writes
  (`Config.AsyncStorageWrites`, etcd-io/raft). It changes hyper-durable's rule that a member answers
  only after its write: a change to hyper-raft upstream, not Amphora's. It covers only the write,
  and the record shows the followers' owners stalling themselves, which silences every answer in any
  order. It also takes from the leader's check the one thing a follower's disk tells it: a follower
  whose disk hangs keeps the check satisfied without bound, gray failure's pattern of a component
  that looks healthy to its detector while it fails its work (Huang et al., HotOS 2017), in a
  setting where disks fail slow in production for seconds and longer (Gunawi et al., FAST 2018).
  Not taken.
- **A quorum patience the leader derives from its voters' answers** (hyper-raft's
  `Raft::set_quorum_patience`, its `docs/raft.md` §3.6, as focal derives it). Taken.

### The rule

The check is a liveness device only (thesis §6.2): reads are ReadIndex reads a quorum confirms, no
read is answered by a lease, and a leader commits nothing without a quorum. A later check deposes
no sound leader; it costs the time a leader cut off from its followers takes to learn it. The
followers' side is unchanged: each still campaigns after its own election timeout without its
leader, so a dead leader is replaced as fast as before.
- **Measured**: each other voter's answer gaps as the leader sees them, in the leader's own ticks:
  the ticks between two messages of that voter in a term the owner leads, the first counted from
  the tick it began to lead. In ticks, so the leader's own stalls are not counted.
- **Window**: two generations of 3,000 ticks (30 s). A voter's tail is the longest gap it ended in
  the current generation or the one before, so a gap is believed for 30 to 60 s, longer than the
  record's loaded runs went between gaps over 200 ms on average (at fewest, 84 in 1,800 s). Where
  stalls are rare the patience lapses between them.
- **Quantile**: the window's maximum. A leader patient for less than the longest gap it measured is
  deposed by the gaps above its quantile; the window bounds how long one gap is believed.
- **Quorum**: the k-th smallest of the other voters' tails, k being half the voters rounded down
  (focal's `quorum_tail`): a check needs k of them, and the ones that answer first suffice. Of two
  followers, the smaller tail.
- **Patience**: that tail less the election timeout, never below zero. A leader whose check is due
  after as many ticks as the gap is never deposed by a silence that long wherever it falls, since
  its check counts every answer since the last (`docs/raft.md` §3.6).
- **Ceiling**: a gap of 165 ticks, so a patience of at most 145 beyond the election timeout. It is
  the largest quorum tail the record shows: in loaded run 2, member 2's answers from 1 and 3 had a
  p99.9 of 1642.788 and 2286.190 ms, the smaller 1642.788 ms, in whole ticks rounded up; every
  other run's smaller p99.9 is below it, jupiter's at most 331.339 ms. These are clock gaps, which
  overstate the tick gaps. A gap past the ceiling is refused, typed (`PastCeiling`, naming the gap
  and the ceiling), logged (`amphora.replica`, event `quorum_patience_refused`) and not learned: a
  voter silent that long is gone, not slow. The patience is never clamped.
- **A cut-off leader's cost**: it steps down at the first check that finds no answer in its
  window, within the election timeout and the patience in force of the first check after the cut:
  at the ceiling 165 ticks (1.65 s), and at most twice that, 330 ticks (3.3 s), after the cut,
  against 40 ticks (400 ms) at fixed ticks, and only while its window holds gaps that long. Its
  proposals in flight answer UnknownOutcome then, as they do now.
- **A member's own patience** (`Raft::set_patience`) stays zero: its own stalls already count one
  tick, so they never run its election timer, and a patience for its leader's stalls would delay
  replacing a dead leader.
- **The flush stays where it is.** The patience covers a voter's answer gaps whatever holds them,
  its owner's blocked steps among them (step gaps reached 9477.636 ms on adacachy and 7206.711 ms
  on jupiter). Moving the flush off the owner thread is t1/fast-ack's Flushing driver, the one flush
  driver: it would shorten the gaps this window measures and change nothing in this rule.
- **Where it lives**: under "The owner's time", it reads no time at all, only the owner's ticks,
  one per `TICK` of `at` with no catch-up, so the Driven shell reproduces a stall by holding answers
  while it moves the leader's clock, and production runs the same step. It changes two of the
  owner's functions: `Actor::run_step` sets the patience before each tick, and `Actor::step` counts
  a voter's message in the term the owner leads. The measure is a pure core
  (`database::patience`).
- **Metrics**: none in this change. The names proposed, for Kiln Metrics' agreement, are
  `amphora_replica_quorum_patience_seconds` (a gauge: the patience in force) and
  `amphora_replica_answer_gaps_refused_total` (gaps past the ceiling).

`replicated.rs` is Codex's. Codex's agent has ended, and its role is held by amphora-seam-owner
(kiln agent 01a1b152d389767493f79adff5245f64, on Ada's word), whose agreement stands for Codex's.
This decision was sent to it under the keys amphora-disk-stalls-quorum-patience-decision-20261106
and amphora-disk-stalls-decision-split-20261106, and its answer is recorded here when given. No
step 3 code lands before it.

Evidence: step 1's record (`docs/research/owner-stalls.md`); Raft's timing requirement and its
check of the quorum (Ongaro's dissertation §3.4 and §6.2, https://raft.github.io/raft.pdf; Ongaro
and Ousterhout, USENIX ATC 2014, §5.6); hyper-raft's quorum patience, its rule and its tests
(`docs/raft.md` §3.6 at the vendored revision, `crates/hyper-raft/src/tests.rs`); focal's
derivation (`crates/focal-node/src/pace.rs`, `quorum_tail` and `quorum_ticks`); etcd's
asynchronous storage writes (etcd-io/raft, `Config.AsyncStorageWrites`); gray failure (Huang et
al., "Gray Failure: The Achilles' Heel of Cloud-Scale Systems", HotOS 2017); fail-slow disks in
production (Gunawi et al., "Fail-Slow at Scale", FAST 2018).

Falsified by:
- a leader whose followers' answers are held as long as a gap it measured that steps down
  (`a_leader_whose_followers_answer_as_late_as_it_measured_keeps_its_lead`);
- a leader cut off from every follower that still leads past the election timeout and the patience
  in force after the first check following the cut, or past the ceiling
  (`a_leader_cut_off_from_its_followers_steps_down_within_its_patience`);
- a read answered or an entry committed by a leader without a quorum;
- step 1's measurement re-run on loaded adacachy with the patience in force, in which a member's
  quorum-check step-downs in a run are not fewer than the fewest at fixed ticks (63): then the
  step-downs are not the followers' tails this patience covers. Not yet run.
