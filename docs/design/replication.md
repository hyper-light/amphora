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
- The leader says how far each learner has caught up (2026-11-06), so that its host promotes only
  a learner that won't stall commits: kiln chooses its voters itself (kiln plan 01a1a283e14c, its
  `docs/p2p.md` P3, kiln's Amphora patch 0003). `ReplicaHandle::catch_up` answers, for each
  learner of the configuration applied, where catching it up stands (`CatchUp`), as hyper-raft's
  rounds judge it (`Raft::catch_up`, the dissertation's section 4.2.1): each round sends what the
  leader held as it began. Asked first, a learner is staged; asked again, it is judged as an
  election timeout passes: `Ready` once a round ended within one, `Aborted` once its lag didn't
  shrink over one (staged afresh when asked again), else `Pending`. A new leader stages afresh, and
  a learner whose removal the core has appended but not applied is left out. Only the leader
  answers, as only it tracks each learner's progress; another member refuses `NotLeader`, naming
  the leader it knows. It is answered at once, beside the write under way, charged as a change is
  at admission, and bounded by the configuration's learners (hyper-raft keeps one staging a
  learner). Promoting is the host's change to make (`MemberChange::Promote`), and nothing promotes
  by itself. A fatal error from the core while judging fences the owner, as a wire record's does;
  kiln's patch answered it and went on. No wire format, log entry or state-machine change.

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
A new server is caught up in rounds, each replicating "all the log entries present in the leader's
log at the start of the round"; "if the last round lasts less than an election timeout, then the
leader adds the new server", it aborts for one "that will never catch up", and "the caller may always
try again" (the dissertation, section 4.2.1, `membership/availability.tex`); hyper-raft's
`catchup.rs` builds that rule, giving a learner up once its lag didn't shrink over an election
rather than after a count of rounds. etcd promotes a learner only once "its log has caught up to
leader's" ([etcd's learner design](https://etcd.io/docs/v3.5/learning/design-learner/)), judged on
the leader alone: "only raft leader has information on whether the to-be-promoted learner node is
ready" (etcd `server/etcdserver/server.go` at d622ec57, line 1454; its measure is a ratio,
`readyPercentThreshold = 0.9`, line 113, where Raft's is a round against an election timeout).
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
`a_hand_off_is_refused_beside_a_change_and_to_a_learner`,
`the_leader_judges_a_learners_catch_up_in_rounds` and
`a_learner_that_never_answers_is_given_up_after_an_election_timeout`. Requirements: AC-T1.8.M1 to M3 and
T-T1.8.M1 to M5 (docs/requirements.toml).

Falsified by: a change answered Ok whose configuration the replica has not applied; a change that
commits as one changing nothing; two changes in flight on one leader; a learner missing a write
made before or after it was added; any record sent to a member after its removal was answered; a
removed member applying a write of its own; a member's answers refused while the leader counts it;
a removal written over whose member's records are still refused; a change or a hand-off admitted
while the core is joint or has a change to apply; a leader that can't be removed
once it has handed on its lead; a transfer answered Ok while this member follows another; a
learner added while the database is past the snapshot ceiling; a learner judged `Ready` whose last
round lasted an election timeout or more, or one never judged `Aborted` whose lag didn't shrink over
one; `catch_up` answered with anything but `NotLeader` off the leader; a learner of the
configuration applied left out of its answer while the core still counts it.
