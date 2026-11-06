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

## Membership (2026-11-03)

kiln makes every node it installs a member of its one replication group (kiln plan
01a1a283e14c75eb8a28a9347287e4c5, step 1): a node that joins is added as a learner, and one that is
deleted is removed. Amphora fixed its members when a replica opened (`ReplicaConfig`) and gave no
way to change them. `ReplicaHandle::change` now takes one `MemberChange`: add a learner, promote a
learner to voter, demote a voter to learner, or remove a member. `ReplicaHandle::status` reads the
configuration the replica has applied (`voters`, `learners`, and `voters_outgoing`, the voters a
joint configuration leaves, empty outside one). `ReplicaHandle::transfer` hands the lead to a voter.

- Only the leader takes a change. Another member refuses it as `NotLeader`, naming the leader it
  knows, so the host takes it there. A write is different: a follower's or a learner's call passes
  the leader's ReadIndex and its proposal is forwarded, as before, and applies only once committed.
- One change at a time, in a slot of its own beside the write under way (`Changing`); neither waits
  for the other. A second change, or a hand-off beside one, is refused as `Unavailable`, at
  admission. So is one asked while the core's configuration is joint ("the group is leaving a joint
  configuration") or the core holds a change it has not applied (`has_pending_conf`), and one asked
  while the group takes its seed. A hand-off admitted while joint would lose its election for
  nothing: its voter lacks the way out the others hold.
- A change first passes a ReadIndex barrier, as a call does. A leader confirms reads only once it
  has committed in its term, and releases one only once it has applied through it. So the change is
  judged against a configuration that is current, and an election's conservative pending mark (the
  core's `pending_conf_index`, set to its last index when it becomes leader) is applied.
- Two phases (Ongaro and Ousterhout 2014, section 6): the change is proposed into a joint
  configuration (`ConfChangeTransition::Implicit`), and the leader proposes the way out by itself
  once that entry commits (hyper-raft's `leave_joint_when_committed`, raft.rs, appends the way out
  when the joint entry commits, not when it applies). The owner answers once its entry is applied in
  the term it proposed it in and the configuration applied is no longer joint, with one exception
  below: a deadline once the joint configuration is applied. A change the configuration already
  shows proposes nothing and answers at once. A change the core would refuse when applied (one that
  leaves no voter) is refused before it is proposed, by applying hyper-raft's own `Plan` to the
  configuration, so it is never committed as a change that changes nothing. The same check refuses
  one past the members a group may have (`TooLarge`, `MEMBERS` and the core's `Limits::members`).
  The leader neither demotes nor removes itself (`Invalid`): it hands its lead to another voter with
  `transfer` (hyper-durable's `Replica::transfer`, Raft's leadership transfer, Ongaro's
  dissertation, section 3.10), and that voter, once leading, changes it. `transfer` is taken only by
  the leader, never beside a change, a joint configuration or the group's seed, and answers once
  this member follows the voter it named. The core gives up a hand-off its voter did not take within
  an election timeout; that is refused `Unavailable`, a lead that went to another `NotLeader` naming
  it, and a deadline passed first `UnknownOutcome`. A hand-off to a member that does not vote, which
  the core would drop saying nothing, is refused `Invalid`.
- A joiner is limited by the snapshot ceiling. It catches up from the leader's log, or by the
  leader's image once the log no longer reaches back, and an image holds at most `SNAPSHOT_BYTES`
  (256 MiB) until streaming snapshots exist. So a learner is added only while the database is within
  that ceiling: past it the change is refused `TooLarge` before it is proposed, never added to fall
  behind, and never left to make the leader build an image it can't. The database can still grow
  past the ceiling after a joiner is added; that database fences at its next image, as any does past
  the ceiling (the snapshot limit above).
- The core takes a change by marking its entry as its pending change. The owner reads that mark
  before and after proposing, so an entry the core kept in place stating nothing is answered
  `Unavailable`, never awaited.
- Once proposed, a change whose lead moves or whose deadline passes before it is seen applied
  answers `UnknownOutcome`: the caller reads the configuration before trying again. Once its joint
  configuration is applied it is committed, and a deadline then answers Ok with the status applied,
  which may still be joint (`voters_outgoing` non-empty): the third outcome. The leader, this one or
  the next, leaves the joint configuration by itself, and no change or hand-off is admitted until it
  has.
- A wire record's sender is checked against the members the core counts by: the newest configuration
  its log states, applied or not (hyper-raft's `Raft::configuration`, raft.rs; "each server always
  uses the latest configuration found in its log", Ongaro's dissertation, section 4.1). In a joint
  configuration that is both halves, so the outgoing voters are heard throughout it. It was checked
  against the configuration applied, which lags: a leader sends to a member it adds from the moment
  it appends the change, and refused that member's answers until it applied it. A removed member's
  records are refused from the moment the leader appends the removal, which is also when it stops
  sending to it. A change that is appended but never committed is not final: a new leader's entry
  can replace it, and the core then takes back the configuration it stated (hyper-raft's
  `refresh_configuration`, from `conf_before`), so that member's records count again.
- The caveat of that check. The dissertation (section 4.1) has a server accept AppendEntries "from a
  leader that is not part of the server's latest configuration". This check doesn't: a leader that
  is only an outgoing voter (elected during a joint configuration that removes it) is refused by
  each follower from the moment that follower appends the way out, so it can't commit it, and the
  group waits for an election among the members that remain. That costs liveness, not safety: no
  entry is accepted that Raft would refuse. A leader never removes itself here (it hands on its lead
  first, as the dissertation's section 4.2.3 recommends), so the case is a lead lost mid-change.
- A member to be added opens as the learner it is to be: the group's voters and itself as a learner.
  It campaigns for nothing, and catches up from the leader's log or a snapshot once added. Replaying
  the log from its start under that configuration converges on the leader's: adding a learner that
  is one already changes nothing (hyper-raft's `Configuration::change`).

Decided by amphora-seam-owner, which holds Codex's role on Ada's word (2026-11-06):
- No `Busy` or `Unknown` variants on `Refusal`. It is a wire enum kiln matches on, changed
  append-only, and `Unavailable` or `Invalid` with their text say what. Variants are added only when
  kiln shows a caller that must branch on them.
- A follower's write with no leader known keeps waiting to its deadline (`Timeout`). An election in
  progress usually ends inside it, and an immediate `NotLeader { leader: None }` would only make
  callers spin.

Evidence: Raft's configuration changes in two phases, with new servers joining as non-voting
members first ([Ongaro and Ousterhout 2014](https://raft.github.io/raft.pdf), section 6); etcd's
learners neither vote nor count toward a quorum until promoted
([etcd's learner design](https://etcd.io/docs/v3.5/learning/design-learner/)); hyper-raft's
`Raft::propose` keeps a change it can't take in place stating nothing, and its configuration is the
newest its log states (`vendor/hyper-raft/crates/hyper-raft/src/raft.rs`); hyper-durable applies a
change the core refuses as one that leaves the configuration as it was (`Replica::apply_change`).
A leader hands on its lead by bringing a voter's log up to its own and telling it to campaign at
once, and gives up after an election timeout
([Ongaro's dissertation, section 3.10](https://github.com/ongardie/dissertation));
hyper-raft's `handle_transfer_leader` ignores a target that does not vote and clears
`lead_transferee` when it gives up. A server's configuration is the latest in its log, committed or
not, and a log truncated past a configuration entry falls back to the one before (the dissertation,
section 4.1); hyper-raft keeps that one in `conf_before` and restores it in `refresh_configuration`.
The image's ceiling is `SNAPSHOT_BYTES` (`crates/amphora/src/replication.rs`), checked by the
bytes `Db::image_bytes` counts, the same count `Db::image_pages` refuses past.
The tests: `a_learner_added_has_every_write_and_once_removed_receives_none`,
`a_learner_is_promoted_and_demoted_through_the_joint_configuration`,
`member_changes_are_refused_off_the_leader_at_once_or_when_impossible`,
`a_changed_membership_holds_through_a_restart`,
`a_change_past_its_deadline_answers_what_its_stage_knows`,
`a_change_at_its_barrier_is_refused_once_the_lead_is_lost`,
`a_change_another_lead_wrote_over_is_never_answered_done`,
`a_change_holds_through_an_election_at_each_stage`,
`the_leader_is_removed_once_it_has_handed_on_its_lead`,
`a_joiner_to_a_database_past_the_snapshot_ceiling_is_refused_before_it_is_proposed`,
`a_removal_another_lead_wrote_over_gives_the_member_back`,
`a_change_or_hand_off_is_refused_while_the_group_leaves_a_joint_configuration`,
`a_hand_off_the_core_gives_up_is_refused_and_the_lead_stays`,
`a_hand_off_whose_lead_goes_to_a_third_member_is_refused_naming_it`,
`a_hand_off_past_its_deadline_answers_unknown_outcome` and
`a_hand_off_is_refused_beside_a_change_and_to_a_learner`. Requirements: AC-T1.8.M1 to M3 and
T-T1.8.M1 to M5 (docs/requirements.toml).

Falsified by: a change answered Ok whose configuration the replica has not applied; a change that
commits as one changing nothing; two changes in flight on one leader; a learner missing a write
made before or after it was added; any record sent to a member after its removal was answered; a
removed member applying a write of its own; a member's answers refused while the leader counts it;
a removal written over whose member's records are still refused; a change or a hand-off admitted
while the core is joint or has a change to apply; a leader that can't be removed
once it has handed on its lead; a transfer answered Ok while this member follows another; a
learner added while the database is past the snapshot ceiling.

## A forward that meets an election (2026-11-06)

Serves AC-T1.8.1: a record survives the loss of the leader, and a write the loss drops is answered,
never applied unseen. docs/requirements.toml has no criterion of leader loss alone; AC-T1.8.4 is
failover latency, which this decision does not measure.

A follower forwards its caller's proposal to the leader it knows (hyper-raft's `step_follower`).
While the lead moves, the forward can reach that member after it has stopped leading. While it hands
its lead over, it refuses the forward `Dropped::Transferring`. Once the transferee's vote request has
moved it to the new term, and before the new leader's first append reaches it, it is a follower
that knows no leader, and it refuses the forward `Dropped::NoLeader` (hyper-raft's `step_follower`
and `step_candidate`). Either refusal answers the wire record's receipt, typed:
`Unavailable("refused: the proposal is dropped: …")`. The proposal never entered a log. The origin
answers its own caller on the term or leader change (`Phase::Proposed`: UnknownOutcome), as the
retry boundary above requires. The receiving member has no caller to answer.

Decided: the owner's answers stand, and the test relay tolerates both causes. A nemesis election
may produce either, and hyper-raft classes both as ordinary refusals retried at the next leader
(`Dropped`'s documentation), never the caller's bug. The relay had tolerated only Transferring, so
`seed_and_delta_hold_through_an_election_at_each_step` failed whenever the forward arrived after
the step-down: 2 runs in 400 alone and 11 in 150 under the full suite at 521e4bc (kiln goal
fa50935c43758a6ea075b7fe77bb3fa5). `a_forward_that_arrives_mid_election_is_refused_no_leader_and_answers_unknown`
places the arrival in that gap on driven time. The relay holds the forward, the lead moves, and the
forward is released once the old leader stands in the new term with no leader. Before this change
it failed 50 runs of 50 on fedora, with the reported refusal.

Rejected: answering a dropped forward Ok at the receiver, as hyper-raft does for a forwarded read
(`drop_read`). The refusal is the only record that the forward was dropped, and nothing is dropped
silently (CLAUDE.md section 3). Also rejected: the receiver re-forwarding the proposal, or the origin
proposing it again. An evaluated session is never replayed, and its captured base is stale once the
term changes.

Evidence: etcd's raft refuses the same three cases with `ErrProposalDropped`, "so that the proposer
can be notified and fail fast" (etcd-io/raft c51f010: raft.go lines 86-88; stepLeader line 1305, a
transfer in progress; stepCandidate line 1677 and stepFollower line 1714, "no leader at term %d;
dropping proposal"). Its `Node.Propose` states that a request "can be lost without notice, therefore
it is user's job to ensure proposal retries" (node.go line 222). In Raft, a server that doesn't lead
refuses a client's request, and the client retries at another server (Ongaro's dissertation,
section 6.2). A request whose answer is lost may be applied only once (section 6.3), so Amphora
answers UnknownOutcome instead of replaying it. Source notes: docs/research/forwarded-proposals.md.

Falsified by:
- a receipt refused NoLeader by a member that knows a leader in its term;
- a dropped forward whose origin's caller is answered Ok, or whose row appears on any replica;
- a dropped forward whose origin's caller is answered anything but UnknownOutcome, Timeout past its
  deadline, or a refusal given before proposing;
- a relay tolerance that admits a Dropped cause other than Transferring and NoLeader.

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

**What hyper-raft already does.** The revision Amphora vendored when this was decided (4d72e19b; d5d5f8f since)
builds R-8, commits on entries held in memory across fates (raft.md §3.7):
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
   from hyper-liveness), never the group's own heartbeats ("The suspicion feed", below).
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
- **A member added or removed** (Membership, above): fates are stated when a member opens, so one added by `change`
  has no fate at the members that opened before it. hyper-raft counts no held answer from a voter whose fate isn't
  stated (`held_half`, progress.rs) and makes no held commit unless, in each half of a joint configuration, a majority
  of the voters have stated fates spanning what is asked (`held_possible`, raft.rs). So that member counts toward the
  classic quorum alone until the members reopen stating its fate: a cost in speed, never in the one-fate guarantee. A
  member opened with the class states a fate for every member its own configuration names (`validate_durability`).
  Changes can leave no held quorum possible at all (the fated voters removed, or too few fates left among the
  voters): then every write commits classically, slower and still correct, and it shows as
  `amphora_replica_commits_total{quorum="classic"}` once the class's commit counter is in. A change that carries the
  joiner's fate (an append-only field on `MemberChange`'s `AddLearner` or `Promote`) is a later goal of its own, not
  this merge. Agreed by amphora-seam-owner, 2026-11-06.

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
- The suspicion feed on t1/class-quorum ("The suspicion feed", below): `ReplicaHandle::verdict` and the owner
  giving each verdict to the held count.
- On t1/class-quorum, the owner's identity naming its durability, and a follower's write answered on its group's
  commit ("Fates and durability checked, and a follower's answer", below).

Its tests:
- the answer before the leader's flush, red with the apply flushing every entry;
- the refusals;
- the core's settings;
- the power-cut nemesis: one member of three fates cut and restarted at the end of each round of writes, its log
  and database losing what they hadn't flushed, every acknowledged write read back (12 seeds in the suite; 10,000
  by hand, `AMPHORA_POWER_SEEDS`, each failing seed named with its account);
- the cut itself: nothing the dying member writes after it survives, as on a powered-off machine (its log's device
  refuses every write and flush from the cut on, though the dying log drains what it took before its threads end).
- a member the detector suspects left out of held quorums until trusted ("The suspicion feed", below).

**Metrics** (t1/class-metrics, kiln goal 8f0f29ac; CLAUDE.md section 10). Each member's owner counts the class as it
goes, and its host reads the counts through `ReplicaHandle::counters`, a plain snapshot
(`meter::ReplicaCounters`), never through `ReplicaStatus`, which crosses the transport. The host exports them by
`meter::REPLICA_METRICS`, as kiln exports its conversation stores' counters:

| metric | type | what it counts |
|---|---|---|
| `amphora_replica_flush_backlog_entries` | gauge | entries the member applied that its database's log hasn't flushed: what a whole-site power cut can take; 0 in the persisted class, which flushes each entry it applies |
| `amphora_replica_flush_seconds` | histogram | each flush the flushing thread completed, timed on that thread (a failed flush fences the owner, so none is counted); buckets of at most 62.5 µs · 2^i, 62.5 µs to 16.384 s (19 bounds), then the rest |
| `amphora_replica_commits_total{quorum}` | counter | entries the member committed as leader, by the quorum that committed each first: `held` or `classic`; leader only, so a follower's stays at 0 |

- The names are the Kiln Metrics agent's agreement (kiln agent 01a14bf7, message key
  `amphora-class-metrics/names/v1`, 2026-11-06): "Agreed on all three names, with one change to (2)'s buckets".
- The flush's buckets span what was measured, 3.9 ms (p50, a calm Mac) to 5.6 s (max, loaded adacachy), and
  reach below a millisecond as that agreement asked: "kiln's production host is Linux on NVMe, where a quiet
  fsync takes well under a millisecond", and kiln's own sub-millisecond histograms start at 50 µs
  (`STALL_BUCKETS`, kiln's crates/kiln/src/metrics.rs). A histogram, not a summary: "you cannot use a summary if you need to aggregate the
  observations from a number of instances" (Prometheus, "Histograms and summaries"), and a group's members are
  aggregated.
- The flushing thread times its flushes on the real clock, not the owner's ("The owner's time"): it measures a
  disk's real I/O, and no decision of the owner reads it, so it is outside that rule.
- Commits by quorum need hyper-raft's core to say which committed (`Raft::commit_stats`, upstream at hyper-raft
  `d5d5f8f`, the revision Amphora vendors). Its commit takes the first of the classic quorum's index and the held
  quorum's, so what a commit moves through the classic index the classic quorum holds, and past it only the
  held quorum did. The fast track, which Amphora doesn't run, counts its own (`FastStats::committed`).
- Labels: Amphora adds only `quorum`, two values. The host adds its own: kiln adds `db`, the record family's
  name (a fixed, small set: one kiln holds several replicated databases, and a sum by machine would hide which
  one is behind), and its machine labels.

Dashboard row **Amphora replicated class**, one panel each, in PromQL (any host's form) and in kiln's own
queries. kiln carries it as a group of a built-in "Stores" dashboard (crates/kiln/src/observe/dashboards.rs) and
in its Grafana overview (deploy/grafana/dashboards/kiln-overview.json):

| panel | PromQL | kiln |
|---|---|---|
| Flush backlog (entries) | `max by (instance, db) (amphora_replica_flush_backlog_entries)` | `max:amphora_replica_flush_backlog_entries{$host} by {host,db}` |
| Flush seconds p50 / p99 | `histogram_quantile(0.5 / 0.99, sum by (le, db) (rate(amphora_replica_flush_seconds_bucket[5m])))` | `p50:` and `p99:amphora_replica_flush_seconds{$host} by {db}` |
| Commits by quorum, stacked: the held share is the bars' proportion | `sum by (quorum) (rate(amphora_replica_commits_total[5m]))` | `sum:amphora_replica_commits_total{$host} by {quorum}.as_count()` |

Its tests: the counts a host reads, through a held flush and the flushing thread's, and none in the persisted
class (`the_replicated_class_counts_its_backlog_flushes_and_commits`); the buckets
(`flushes_count_in_the_first_bucket_whose_bound_holds_them`); the names here equal to the code's
(`the_replica_metrics_are_the_ones_the_design_names`); the core's count
(`a_leader_counts_each_commit_by_the_quorum_that_made_it_first`, in hyper-raft).

Falsified by: a backlog above 0 once a flush began after the last apply has ended; a held commit counted in a
group of one fate or of the persisted class; an entry counted under both quorums, or under neither.

Not built, and none of it weakens the one-fate guarantee:
- **A whole-site power cut bounded by the promise window,** as a test of its own.

Falsified by:
- an acknowledged write lost after one fate's failure, or after the leader's;
- a held commit counted among members of one fate, or from a suspected member;
- the replicated class granted to a database that can't span its fates, or to a single node;
- a whole-site power cut losing a write older than the promise window;
- a restart finding the machine's WAL ahead of the member's own log;
- held commits answering later than classic ones at p50 on the same members, against "never slower".

### The suspicion feed (2026-11-06)

Kiln goal dcdd872a0ed78f37902b06dd95dd3bf1. Sources and quotes are in docs/research/suspicion-feed.md.

**Decision.** The detector is hyper-liveness, run by the host, and its verdicts reach the held count alone.
- **Which detector.** hyper-liveness, vendored already at 4d72e19b (the "not vendored" above was out of date). It is
  the detector R-8 names ("the owner's node-level detector decides (hyper-liveness), never the group's own
  heartbeats", raft.md §3.7). It keeps one stream per pair of nodes, shared by every group they share, judged by
  NFD-E (Chen, Toueg and Aguilera 2002). Each heartbeat proves a recent flush, so a node whose disk stalls is
  suspected. A stalled member still answers held from memory, so that proof is the one the held count needs; a
  group's own heartbeats would keep counting it (gray failure's "differential observability", Huang et al. 2017).
  No equivalent is written here: one would duplicate a vendored, tested detector.
- **Who runs it.** The host. Amphora carries no records between nodes ("Hosts carry authenticated wire records
  only"), and hyper-liveness's heartbeats ride the host's datagram plane, one detector per node for all its groups.
  The host maps each `Change` to `api::replication::Verdict` and tells every replicated database it shares with that
  member, through `ReplicaHandle::verdict`:
  - `Suspected` or `Stopped` become `Verdict::Suspected`;
  - `Trusted` or `Restarted` become `Verdict::Trusted`;
  - the disk and mode changes concern elections, which Amphora doesn't take from a detector, and aren't told.
- **What it reaches.** The held count alone. Amphora elects on ticks, and there hyper-durable's `Replica::suspect`,
  `trust` and `restarted` are refused once they've taken the held part ("On ticks the replica elects by the owner's
  ticks and takes no detector's word", hyper-durable tests/shell.rs). Relying on a refusal's side effect is
  refused here. Vendor change 0005 adds `Replica::suspect_held` and `trust_held`, the core's `RawNode` calls under
  the shell's unwind boundary. It is additive, and named in vendor/hyper-raft/UPSTREAM.md to offer upstream. Moving
  Amphora's elections to suspicion, as Focal's bus does, is another change, and not this one.
- **Its refusals.** A verdict of this member, or of a node the configuration doesn't name, is refused, typed
  (`Refusal::Invalid`): the core would keep nothing of it, and a dropped verdict would be silent. A full queue is
  `Refusal::Full`, a passed deadline `Refusal::Timeout`, an ended owner `Refusal::Fenced`. The verdict takes one
  slot of the owner's bounded queue (`COMMANDS`), charged its size. The core's set is bounded by the members limit
  (`Error::Capacity`).
- **A reopened owner** trusts every member until its host tells it again. That is safe: every held quorum spans the
  fates asked whether or not anyone is suspected, and the leader counts only its disk. The host restates what its
  detector believes to a database it opens, as Focal's owner does for a reopened replica.

**Its test** (`a_member_the_detector_suspects_isnt_counted_in_a_held_quorum_until_trusted`):
- three voters of three fates, two to span;
- the followers' durable answers held back by the relay, their held answers let through;
- with both followers suspected, a write at the leader isn't answered, though both followers said they hold it;
- trusting one, the write is answered by a held quorum (the leader's disk and that member's memory);
- with both suspected again, the next write is answered only once the durable answers are let through: the classic
  quorum.
It was red with the verdict taken and not given to the core.

**A verdict told while following stays in force once leading**
(`a_verdict_told_while_following_stays_in_force_once_leading`, asked by amphora-seam-owner at 15097f9). A member is
suspected at a follower, and the lead is then handed to that follower. The old leader is cut off, and the suspected
member's durable answers are held back while its held ones pass. A write at the new leader isn't answered: its own
disk and a suspected member's memory are no held quorum. Once the durable answers pass, the classic quorum answers it.
As the control, the member is trusted and its durable answers are held back again, and the next write is answered on
the held quorum. The core keeps its suspected set across `reset` and `become_leader` (hyper-raft, core step R-8).

Falsified by:
- a held commit counted from a member the host's detector suspects, including one suspected before this member led;
- a verdict accepted and dropped: one of a non-member answered `Ok`, or one taken and not reaching the core;
- a verdict changing an election.

### Fates and durability checked, and a follower's answer (2026-11-06)

Kiln goal db9c7620ad5e8df0a91eb2379ae77cb9: the two remaining items above, "the answer moving to the leader" (item 6)
and "fates checked across members".

**1. The owner's identity names its durability.** Every process that opens a member's database meets one owner:
either it is the owner (in this process), or it reaches the owner over the local transport
(docs/design/local-owner-transport.md: "Authenticated identity includes the complete replication reopening
configuration; an explicitly configured attachment must match it"). The durability is part of that reopening
configuration, and until now the identity left it out:
- An in-process open compared it (`acknowledged == durability`), but in the fates' given order. The same fates in
  another order were refused.
- An attachment over the transport compared only the log path and the configuration. A process stating the
  replicated class was attached to a persisted owner, or to one with other fates.
- A process that stated no replica restarted the member persisted when it took the owner's place
  (`remote::attach`: "a client that stated none reopens persisted"), silently changing a replicated member's class.

The change:
- `Identity` gains `durability: Option<Durability>`, appended: `Some` for a replicated owner, `None` for a standalone
  one.
- Both checks compare the class canonically (fates by member). A difference is refused, typed: `Refusal::Invalid`,
  "replica ownership", naming the durability.
- A process that stated no replica restarts with the durability the owner named.

**The wire change bumps the protocol version to 4** (local-owner-transport.md, "Versions"). postcard isn't
self-describing: a struct is its fields in order, with no lengths or names on the wire, so a decoder can't tell a
missing trailing field from a short frame, and `#[serde(default)]` fills nothing on decode. An identity with the field
is a different record, so owner and client change version together across a drain, and a peer of version 3 is refused
at the greeting, before any identity is read. The field carries no `serde(default)`: it would never apply.

**Across nodes**, each member's owner states its own class, and its leader counts held answers by the leader's own
map of fates. What one node states isn't checked against another's here: the raft wire is hyper-raft's, carried by
the host, and the configuration both come from is the host's. A member that states the persisted class answers
nothing held, so that disagreement only loses speed. Fates that disagree across nodes would let a leader count a held
quorum within one real fate. Closing that needs the class stated once for the group: in its log at bootstrap, each
member refusing an entry that states another. That is not built.

**2. A write made at a follower is answered on the leader's commit.** Today a follower answers when it applies the
write, and hyper-durable applies at a follower only what its own log holds durably (item 6). Under the replicated
class, the write is acknowledged by the group's commit, held or classic, not by the proposer's disk. The follower
learns that commit from its leader. So, in the replicated class:
- a follower answers once the commit it has heard covers its entry, and its log holds that entry. It doesn't wait
  for its own log write or its apply.
- The answer's result is the apply's, decided from the log. The machine applies a prepared write only if the
  database's point before it is the write's base (`Machine::apply`, "the database advanced since preparation"), and
  every entry moves that point. So the write succeeds exactly when the entry before it in the log is its base:
  `(index - 1, term(index - 1)) == (base_index, base_term)`.
- The owner keeps the write pending until it applies, as now, so no later preparation can use the base before the
  write is in it. When it applies, its result must agree with the answer given; if it doesn't, the owner fences
  (an invariant broken, never a silent difference).
- Neither a new term nor the caller's deadline ends that pending write: it is committed, so it applies in whatever
  term follows, and its apply is still checked against its answer. A follower whose applied point passes the write
  with no apply answered (an image installed over it) drops it then: nothing is left to check, and nothing waits
  on it. A proposed write that is not yet answered is ended by a new term as before (`UnknownOutcome`).
- Why the verdict is final (the sources). A follower commits only through what its leader says is committed and its
  own log matches (hyper-raft `RaftLog::commit_to`, through `maybe_append`; `maybe_commit` at a leader counts only
  an entry of its own term), and two logs that hold an entry of one index and term are identical up to it (Log
  Matching, [raft.pdf](https://raft.github.io/raft.pdf) Figure 3 and section 5.3). A committed entry is in the log of
  every later leader (Leader Completeness, section 5.4), so neither the write nor the entry before it is ever
  replaced, and `term(index - 1)` read at the commit is the one its apply meets. Every committed entry moves the
  machine's point: a write through `Machine::apply`, a change through `apply_change`, both by `advance`, and an
  image through `install` (crates/amphora/src/replication.rs). So the point before the write's apply is
  `(index - 1, term(index - 1))`, and `replication::verdict` decides from it what `Machine::apply`'s base fence
  (`on_base`) will.
- A later call at the follower crosses a ReadIndex barrier, which waits for the follower's apply. A caller therefore
  reads its own write wherever it reads next.
- The leader is unchanged: it already answers on the commit (R-6, `apply_unpersisted`). The persisted class is
  unchanged: its answer waits for the proposer's own flush.

**Tests:**
- An owner stating the replicated class refuses an attachment stating persisted, or other fates. One stating the
  same fates in another order is taken. One stating nothing takes the owner's class. The test was red with the
  identity naming no durability.
- In the replicated class, a write made at a follower is answered while that follower's own write of it is held.
  The test holds the flushes of the follower's log device once its write is in the leader's log. Held any sooner,
  the follower sends nothing: hyper-durable keeps a member's messages back while a write of its own is out. A
  write at the leader, prepared on the same base, makes one of the two refused; the follower's apply must agree
  with its answer. The test was red with the follower answering at its apply: unanswered until the hold's hang
  guard (120 s) failed the follower's log.
- The identity read across versions: version 3's reader would take a version-4 identity without its durability,
  silently, which is why the version moved; today's refuses version 3's shorter frame
  (`the_identitys_durability_is_a_new_version`).
- The answered write kept across a term change until it applies (`a_follower_keeps_its_answered_write_across_a_term_change_until_it_applies`),
  red when a new term dropped it.
- A follower cut after answering and before its own log flushed loses nothing acknowledged
  (`a_follower_cut_after_answering_on_the_commit_loses_nothing_acknowledged`, AC-T1.8.5).
- An apply that differs from the answer fences (`an_apply_that_differs_from_the_answer_on_the_commit_fences`). The
  displacement arm f8d1738 had for an answered write is removed: `Output::displaced` carries only what the fast track
  proposed, which Amphora doesn't run, and an answered write is committed besides.

Falsified by:
- an attachment taken whose durability differs from its owner's;
- a member reopened in another class than its owner's;
- an older or newer peer's identity misread rather than refused;
- a follower's answer that differs from its write's apply;
- a follower answering before the group committed its write;
- a later read at that follower missing the write.

## The leader's quorum patience (2026-11-06)

Kiln goal 6b112f14729c899e826452260ca618c1, step 2, decided on step 1's record of the owners'
stalls (`docs/research/owner-stalls.md`, a measurement's record, where the falsifier's runs go
too). Every number quoted here from a run is in that record;
`database::replicated::tests::the_decisions_numbers_are_step_ones_or_its_own` holds it.

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
read is answered by a lease, and a leader commits nothing without a quorum. hyper-raft has no lease
read: its reads wait for a round of a quorum's answers ("No clock is trusted", `read.rs`), and a
leader answers one alone only where it is the one voter (`Raft::serve_read`; its tests
`reads_asked_together_leave_in_one_round_and_one_answer_confirms_them` and
`a_leader_the_newest_configuration_leaves_out_answers_no_read_alone`). The owner takes every call
through that read (`Actor::receive`, `Replica::read`), and
`a_leader_cut_off_from_its_followers_steps_down_within_its_patience` asks a read of a patient
leader cut off from its followers and finds it unanswered until the leader steps down. A later check deposes
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
  followers, the smaller tail. In a joint configuration the check needs a majority of each set, so
  the patience is the larger of the two sets' tails, each over its own voters, and a voter of
  either set is measured.
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
- **The flush stays where it is.** The default Persisted owner still flushes its log inline, on
  the owner thread. The patience covers a voter's answer gaps whatever holds them,
  its owner's blocked steps among them (step gaps reached 9477.636 ms on adacachy and 7206.711 ms
  on jupiter). Moving the flush off the owner thread is t1/fast-ack's Flushing driver, the one flush
  driver: it would shorten the gaps this window measures and change nothing in this rule.
- **Where it lives**: under "The owner's time", it reads no time at all, only the owner's ticks,
  one per `TICK` of `at` with no catch-up, so the Driven shell reproduces a stall by holding answers
  while it moves the leader's clock, and production runs the same step. It changes two of the
  owner's functions: `Actor::run_step` sets the patience before each tick, and `Actor::step` counts
  a voter's message in the term the owner leads. The measure is a pure core
  (`database::patience`).
- **Metrics**, in base units, agreed with Kiln Metrics under the key
  `amphora-class-metrics/names/v1-disk-stalls` (through amphora-class-metrics), on its condition
  that their help lines define them:
  - `amphora_replica_quorum_patience_seconds`, a gauge: "The ticks this replica, while it leads,
    waits beyond its election timeout before its quorum check, in seconds; zero while it follows
    or has measured no answer gap.";
  - `amphora_replica_quorum_patience_refused_total`, a counter: "Answer gaps a leading replica
    refused to learn for its quorum patience, because they passed its ceiling."
  No labels from Amphora (it may add only small closed sets); kiln adds its own. They are wired
  into the replica meter (`meter.rs`, t1/class-metrics) once both branches are on main; until
  then the refusal is the event above.

`replicated.rs` is Codex's. Codex's agent has ended, and its role is held by amphora-seam-owner
(kiln agent 01a1b152d389767493f79adff5245f64, on Ada's word), whose agreement stands for Codex's.
This decision was sent to it under the keys amphora-disk-stalls-quorum-patience-decision-20261106,
amphora-disk-stalls-decision-split-20261106 and amphora-disk-stalls-decision-dcf884c-20261106. It
agreed (2026-11-06) with six changes, made here: a joint configuration's two sets, the read path
cited, no clamp at the ceiling, the metrics in base units agreed with Kiln Metrics, the falsifier
run before the goal closes, and the inline flush noted as fast-ack's. Its reasons: the check is
liveness only and the followers' timers are unchanged; the options are weighed on the record; k
other tails make a majority with the leader; the measure reads no clock; the window and the
ceiling bound it; a gap past the ceiling is refused, never clamped.

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
- step 1's measurement re-run with the patience in force, on loaded adacachy or a Mac Studio under
  the same load, in which a member's quorum-check step-downs in a run are not fewer than the fewest
  at fixed ticks (63): then the step-downs are not the followers' tails this patience covers. Its
  runs are recorded beside step 1's in `docs/research/owner-stalls.md`.
