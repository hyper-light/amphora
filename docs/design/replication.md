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

Decided for production owners: this change gives them no patience. A follower's slow write already
deposes a sound leader today: a liveness cost, never a safety one. Whether an owner should give
hyper-raft its patience and quorum patience from the stalls it measures (`Raft::set_patience`,
`set_quorum_patience`) is decided on measurements of owners' stalls under load, in kiln goal
6b112f14729c899e826452260ca618c1. The single-owner tests move to the test shell in kiln goal
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

## Fast acknowledgement: the replicated class (approved 2026-11-01, being built)

Kiln goal 1b2c66ae884889609ce5248729385a0a, which Ada set on 2026-11-01 ("we should explore a way to make it work with
our replication and RAFT right?"). Measurements and sources are in docs/research/fast-acknowledgement.md.

**Decision (Ada, 2026-11-01).** Asked whether to build it, opt-in per database with the default unchanged, she chose
"Approve and build it now". It is built failing test first, gated, on a branch, and merged only on her word.

**Why.** Amphora acknowledges a write only after a local flush that began after its commit (store.md: "one is
acknowledged only once a flush that began after its commit ends (AC-T1.4.4)"), so every small write pays one:

| host | flush | load | p50 | p99 | max |
|---|---|---|---|---|---|
| a Mac (APFS) | F_FULLFSYNC | 2 | 3.928 ms | 6.014 ms | 10.917 ms |
| fedora-2 (btrfs) | fdatasync | 15-17 | 8.836 ms | 18.266 ms | 209.365 ms |
| adacachy (btrfs) | fdatasync | 76-78 | 21.324 ms | 1,340.648 ms | 5,625.591 ms |

Kiln's lookups that wrote a hit count measured p99 10.7-13.2 ms on a Mac, and about 92 ms a lookup on loaded adacachy.
In replicated mode an answer waits twice:
- for a quorum's flushes, Raft's rule ("Updated on stable storage before responding to RPCs", raft.pdf Figure 2);
- then for the proposer's own apply flush (`Machine::advance` calls the engine's flusher before apply returns).

CP-T1.8 already names a faster class: a replicated-class commit, p99 at most 2 ms for 3 nodes on a LAN (AC-T1.8.3),
beside the persisted quorum commit's 5 ms (AC-T1.8.2).

**What hyper-raft already does.** The revision Amphora vendors (4d72e19b) builds R-8, commits on entries held in memory
across fates (raft.md §3.7):
- A follower answers at once for entries its durable promises cover.
- The leader counts itself by its disk alone.
- An entry commits on the first of the classic quorum and a held quorum: a majority of unsuspected voters "whose fates
  and the leader's span the number the owner asks (`Config::fates`)".
- With three voters, that is the leader's write and the nearest follower's memory, or both followers' memory.
  "Whichever comes first commits, so the rule is never slower than the classic commit."
- "A leader counts no held answer where no held quorum can be, as among members of one fate".
- A member its owner suspects isn't counted until trusted again (`suspect_held`, `trust_held`).

R-8 follows SAUCR (OSDI 2018), which buffers in memory "with many nodes up" and is "within 0%-9% of a purely
memory-based system", and adds fates. SAUCR counts members, but real failures come in correlated bursts: "most large
bursts of failures are associated with rack- or multi-rack level events" (Ford et al., OSDI 2010). MongoDB ships the
same trade as an opt-in: with `j: false`, a majority write's "acknowledgment requires writing operation in memory", and
such writes "could possibly roll back in the event of a transient loss (e.g. crash and restart) of a majority of nodes".

**The replicated class.**
1. **A class per database.** It's persisted (today's, the default) or replicated with a number of fates asked (at
   least 2). A database whose configuration can't span the fates asked refuses the replicated class, typed, naming its
   fates. A single node or a laptop never has it.
2. **Fates declared.** Each member's replica configuration names its fate: what it fails with, its host and its power
   feed. The owner sets `Config::fates` to the number asked.
3. **Suspicion fed.** The owner gives hyper-raft its node-level detector's verdicts (`suspect_held`, `trust_held`,
   from hyper-liveness), never the group's own heartbeats.
4. **Apply answers before its flush.**
   - For an entry committed under the replicated class, the machine applies it and the answer leaves without waiting
     for the engine's WAL flush. A group flusher makes it durable after.
   - `durable()` reports the last point flushed, which hyper-durable's contract allows ("The last entry a restart opens
     with applied"; compaction never passes it, I8). `persist()` flushes everything applied.
   - Such entries don't act at start (`acts_at_start` false, where today every entry does), so the shell doesn't wait
     for the member's own durable commit before applying them.
   - One invariant holds it together: the WAL never makes durable an entry the member's own log hasn't. A restart
     never finds the machine ahead of its log; it replays from `durable()` what its group committed.
5. **The answer.** It leaves once the entry has committed, held or classic, and the proposer has applied it in memory.
6. **Where it pays.** A leader may apply an entry of its own term once it commits, before its own write is durable
   (R-6, `Config::apply_unpersisted`, durable.md §4.2). A follower applies only what its own log holds durably,
   because "a follower's apply serves no answer" there. So a write proposed at the leader answers on the held commit,
   while one proposed at a follower still waits for that follower's own log write. Unless the answer moves to the
   leader, the class speeds writes made at the leader. hyper-durable reopens a member whose machine got ahead of its
   log by starting the log at the machine's point (durable.md §4.3, case 3).

**What it guarantees** (three voters, fates asked 2):
- **One member crashes or loses power:** nothing acknowledged is lost. Every held commit spans two fates, so a member of
  another fate, or a disk, still holds it.
- **The leader crashes:** nothing acknowledged is lost. The other holder is in another fate, and Leader Completeness
  holds under R-8 (hyper-check: "Leader Completeness and agreement exhausted at two terms (13.4 million classes)").
- **Every member of one fate fails together** (a shared host, one power feed): nothing acknowledged is lost, since each
  held quorum spans two fates. Where every voter shares one fate, no held commit is made, and writes take the classic
  commit.
- **Every fate fails at once** (the whole site loses power): writes acknowledged past each member's last durable promise
  can be lost. That's at most a heartbeat's span in a quiet spell, and as far as the log ran while the promise before
  served under load (raft.md §3.7). This is the class's stated risk, as MongoDB states its own. It is why the class is
  opt-in and the default stays persisted.
- **A single node or a laptop:** the class isn't offered, and every write stays persisted.

**What it costs here** (docs/research/fast-acknowledgement.md). A held commit costs a round trip, and every member
measured reaches the others over Wi-Fi: ping averages 16.7-242 ms, user-level round trips p50 27.6-60.8 ms and p99
374-542 ms, with 4-83 of 2,000 datagrams lost.
- On loaded btrfs (adacachy, flush p99 1.34 s) the held commit answers far sooner.
- On fedora-2 at load 15-17 (flush p50 8.8 ms) the two are close.
- On a calm Mac (flush p99 6.0 ms) the classic commit is faster than this Wi-Fi. R-8 takes whichever comes first, so the
  class costs nothing there and gains nothing.
- On a wired LAN, R-8's own simulation (a 198 us one-way trip and a 4,330 us flush) puts a held commit's p50 at 396 us,
  against 4,726-17,320 us classic. That's the room AC-T1.8.3's 2 ms needs, which no flush measured here meets (the
  Mac's p50 is 3.9 ms).
- The proposer's apply flush stays in the answer's path until item 4 is built.

**What is built** (t1/fast-ack, 2026-11-01):
- `Durability`, `Fates`, `Database::open_replicated_with` and `attach_replicated_with`, all additive.
- The typed refusal (`validate_durability`).
- The core's fates and `apply_unpersisted` (`settings`).
- The machine applying class entries without a flush, `durable()` at the flushed point, and `acts_at_start` false
  for them.
- The owner's flushing thread, one flush out at a time.
- A remote client reopening with the durability it stated.

Its tests:
- the answer before the leader's flush, red with the apply flushing every entry;
- the refusals;
- the core's settings;
- the power-cut nemesis: one member of three fates cut and restarted at the end of each round of writes, its log
  and database losing what they hadn't flushed, every acknowledged write read back (12 seeds in the suite; 10,000
  by hand, `AMPHORA_POWER_SEEDS`, each failing seed named with its account);
- the cut itself: nothing the dying member writes after it survives, as on a powered-off machine (its log's device
  refuses every write and flush from the cut on, though the dying log drains what it took before its threads end).

Not built, and none of it weakens the one-fate guarantee:
- **The suspicion feed** (`suspect_held` from hyper-liveness, not vendored). Without it no member is ever
  suspected. A held quorum spans two fates either way, and the leader counts only its disk. The feed narrows what
  a gray, failing member's held answers can commit, a refinement beyond one fate's loss.
- **Metrics by class.** hyper-raft's core doesn't say which quorum committed an entry, and `ReplicaStatus` crosses
  the transport, so a field there is a wire change. Each takes its own change, with names agreed with the Kiln
  Metrics agent (CLAUDE.md section 10).
- **The answer moving to the leader** for writes made at a follower (item 6).
- **Fates checked across members.** The owner's transport identity doesn't name the durability, so members that
  disagree aren't refused at the transport.
- **A whole-site power cut bounded by the promise window,** as a test of its own.

Falsified by:
- an acknowledged write lost after one fate's failure, or after the leader's;
- a held commit counted among members of one fate, or from a suspected member;
- the replicated class granted to a database that can't span its fates, or to a single node;
- a whole-site power cut losing a write older than the promise window;
- a restart finding the machine's WAL ahead of the member's own log;
- held commits answering later than classic ones at p50 on the same members, against "never slower".
