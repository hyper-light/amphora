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
each step it takes the next input queued, or waits on its bell until something wakes it or its next deadline
comes ("Decision (lane constants): the owner's wakes"). The handle's
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
round with nothing to relay yields `POLL` (a test's own pacing; the owner never polls) to the disk, and a counted budget of rounds, about a
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
  configuration, so it is never committed as a change that changes nothing. A change past the
  group's member bound raises the bound first ("The member bound stated once per group", below).
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
  the leader it knows. A request whose deadline passed before the owner took it answers `Timeout`
  and stages nothing, so a stale request never starts a learner's round. It is answered at once,
  beside the write under way, charged as a change is
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

A forward the leader's write slot holds (below, "the write slot") can outlive its holder's lead. Holding it is
processing it: its receipt is answered Ok as it is held, as admission's is, so no host's `receive` waits on a slot.
A member that no longer leads when the slot releases it doesn't step it, since its core would forward it again under
its sender's name. It discards it, counted in the event `replication.write_slot_discarded`, as the core at a
non-leader drops a proposal; the origin's caller is answered on its term change, as above
(amphora-owner-t1-g-slot-fc28e2d-20261112).

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
| `amphora_replica_flush_backlog_entries` | gauge | entries the member applied that its database's log hasn't flushed: what a whole-site power cut can take in the replicated class; in the persisted class, what a restart applies again from the raft log ("A replica's database log") |
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
| Quorum patience (seconds), the leader's ("The leader's quorum patience") | `max by (instance, db) (amphora_replica_quorum_patience_seconds)` | `max:amphora_replica_quorum_patience_seconds{$host} by {host,db}` |
| Answer gaps refused for the patience, their voter suspected | `sum by (db) (rate(amphora_replica_quorum_patience_refused_total[5m]))` | `sum:amphora_replica_quorum_patience_refused_total{$host} by {db}.as_count()` |

Its tests: the counts a host reads, through a held flush and the flushing thread's, and in the persisted class its
own thread's flushes and no held commit (`the_replicated_class_counts_its_backlog_flushes_and_commits`); the buckets
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
member refusing an entry that states another. Decided in "The class stated once per group" below.

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
  identity naming no durability (`an_attachment_whose_durability_differs_from_its_owners_is_refused`, T-T1.8.C1).
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
- **Window**: two of the voter's own generations. A voter's tail is the longest gap it ended in
  the current generation or the one before; a generation lasts as long as the voter went between
  its last two gaps past the election timeout, its first counted from the tick the leader began to
  measure it, so a gap is believed for one to two of its own recurrences: through each while its
  stalls recur, and it lapses once they stop. The window was a fixed one first, measured once on
  one host: a picked number by Ada's word (2026-11-21; "Decision (lane constants): generations
  learned where they run").
- **Quantile**: the window's maximum. A leader patient for less than the longest gap it measured is
  deposed by the gaps above its quantile; the window bounds how long one gap is believed.
- **Quorum**: the k-th smallest of the other voters' tails, k being half the voters rounded down
  (focal's `quorum_tail`): a check needs k of them, and the ones that answer first suffice. Of two
  followers, the smaller tail. In a joint configuration the check needs a majority of each set, so
  the patience is the larger of the two sets' tails, each over its own voters, and a voter of
  either set is measured. The sets are the ones the core's check counts: the newest configuration
  its log states, applied or not (`raft.configuration()`, its tracker's; "Membership"), never the
  applied one, which lags while a change is in the log, so the patience would measure voters the
  check doesn't count. A sender is checked against the same configuration. Agreed by
  amphora-seam-owner at the line's merge (t1/replicated-line). Tested driven, a group held joint:
  `a_leader_in_a_joint_configuration_keeps_its_lead_through_the_late_halfs_tail`.
- **Patience**: that tail less the election timeout, never below zero. A leader whose check is due
  after as many ticks as the gap is never deposed by a silence that long wherever it falls, since
  its check counts every answer since the last (`docs/raft.md` §3.6).
- **Gone, not slow**: a gap over which the host's detector suspected the voter (`Verdict::Suspected`,
  told to the owner) is refused, typed (`Suspected`, naming the gap), logged (`amphora.replica`,
  event `quorum_patience_refused`) and not learned, the write slot's rule. No number bounds a gap:
  the ceiling first taken here, the largest quorum tail of step 1's record, was measured once on
  one host, a picked number by Ada's word (2026-11-21). The patience is never clamped.
- **A cut-off leader's cost**: it steps down at the first check that finds no answer in its
  window, within the election timeout and the patience in force of the first check after the cut:
  the longest gap its window believes, and at most twice that after the cut, against 40 ticks
  (400 ms) at fixed ticks, and only while its window holds gaps that long. Its
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
    refused to learn for its quorum patience, because the host's detector suspected the voter
    meanwhile." (until 2026-11-21: "because they passed its ceiling")
  No labels from Amphora (it may add only small closed sets); kiln adds its own. They are in the
  replica meter (`meter::REPLICA_METRICS`, read through `ReplicaHandle::counters`, Amphora goal
  867fa589): the owner sets the gauge where it sets the patience, the patience in ticks of `TICK`
  while it leads and zero while it follows, and counts a gap refused for its suspected voter where
  `heard` refuses it; the event above stays beside the count. A gap refused because the leader
  measures as many voters as it can (`Unmeasured::Full`) is the event alone, outside the
  counter's help line. Tested by `the_quorum_patience_and_its_refusals_are_counted_for_the_host`.

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
  in force after the first check following the cut
  (`a_leader_cut_off_from_its_followers_steps_down_within_its_patience`);
- a read answered or an entry committed by a leader without a quorum;
- step 1's measurement re-run with the patience in force, on loaded adacachy or a Mac Studio under
  the same load, in which a member's quorum-check step-downs in a run are not fewer than the fewest
  at fixed ticks (63): then the step-downs are not the followers' tails this patience covers. Its
  runs are recorded beside step 1's in `docs/research/owner-stalls.md`.

## The application floor, replicated (proposed 2026-11-01; a to d decided and agreed 2026-11-06)

Built on t1/floor (kiln plan 01a1b1d7b7fe7497afcf0d665327364a, steps 1 to 5); its metrics, events and T-T1.8.A6 are
step 6, still to come. Kiln goal d8362e1ea15980ce84da0b168a93bc8a: Codex asked Amphora to own this seam. amphora-seam-owner,
which holds Codex's role on Ada's word, agreed the contract on 2026-11-06 subject to three changes, written in below:
attestations bound to a member's join, the membership guard's place and its typed refusal, and the tests and metrics.
Its standing constraint holds throughout: agreement proven by the members and the log, never a caller's integer or a
Ready answer, one SDK seam, no second controller. Source notes: docs/research/application-floor.md.

**Today.** Kiln's U05 patch gives the standalone owner an application floor.
- `for_application(name, version)` binds a handle's claim.
- `writer_floor` raises the floor in the original local writer, monotonically, and returns a `WriterBoundary` once the
  raise is durable.
- `application::check` refuses, inside every application transaction, a claim older than the floor.

Replicated mode refuses any database with a floor installed (`require_replication`). No member has proven what it runs,
so a floor there would only be one member's word.

**What the replicated floor promises:**
1. No application writer older than the floor commits a write, on any member, at any index after the floor's.
2. The floor is raised only when every member of the configuration in force at the raise's index (voters and learners)
   has attested that its application runs the floor's version.
3. The floor never falls.
4. It all happens in the existing replicated owner: no second controller, no new entry type, no wire change.

**Joins.** hyper-raft's configuration (`ConfState`: voters, learners, voters_outgoing, learners_next, auto_leave)
carries no index at which a member joined. Amphora's machine records one: `apply_change` writes
`__amphora_members(member, joined)` in the same WAL transaction as the configuration marker it already writes. A member
that the change names in any of the configuration's four sets, and that has no row, gets `joined` = the change's index.
A member with a row that the change leaves in none of them loses it in that same transaction. Promotion and demotion
keep it. Marker J writes a row at 0 for every member then present; before J no member has a row, and every member
joined at 0. The rows, not the member's own configuration before the change, decide who joins: a member to be added
opens with the configuration it is to have, so its own "before" already names it, and comparing against that would
leave it without the row every other member writes (found by T-T1.8.A5's added-after-J case). The rows and the
configuration after the change are the same on every member at the same index, so every member holds the same rows; a
snapshot carries them as rows of its image. A change whose entry another lead replaces never applies, so it writes
nothing.

**Attestation.**
- When a member's application binds its handle (`for_application(name, highest)`), its owner proposes a resolved
  transaction writing that member's row in a reserved table, `__amphora_attestations(member, joined, application,
  highest)`, keyed by member and the join it holds in its own applied state. It does this after applying up to the
  commit index it learned at start, so it binds against the floor and its join as committed.
- A proof counts a member's attestation only when its `joined` is the member's current join. A row from an earlier
  join never vouches, so no row is ever deleted to keep a proof honest, and none blocks a member's return: a member
  removed and added again joins at a new index and attests again.
- Public SQL can't name an `__amphora_*` table (`sql::validate_reserved_sql`), so only this path writes the rows.
- Under Raft's crash-fault model, a member runs Amphora's own code, which writes only its own row.
- `step` gains one check: a forwarded proposal whose entries' contexts name a member other than the authenticated sender
  is refused, typed, and applies nowhere. That keeps a mis-addressed record from being taken for another member's,
  attestation and answer alike. One hop is enough because nothing re-forwards a proposal: a member that receives a
  forward it can't lead refuses it, and the receiver re-forwarding it was rejected (t1/election-flake, "A forward that
  meets an election"). So a forward's authenticated sender is always its origin.

**Raising the floor.** `raise_floor(application, version, until) -> WriterBoundary` on the replicated `Database`.
- The owner prepares one session. At its base, it reads the configuration in force, every member's attestation and
  the floor.
- A raise to the floor in force or below answers the floor in force at once, writing nothing: the floor never
  falls, and a caller asking for an older contract (kiln's `writer_boundary(required)`) is served by the newer one in
  force. A raise for another application than the floor's is refused.
- Above the floor, it writes the new floor only if every member, voter or learner, has an attestation for the
  application at or above the version, under its current join.
- Otherwise it refuses, typed, naming the members short of the version, a learner that has not attested among them.
- The stale-base rule makes the proof hold at apply. The entry applies only if nothing committed after the base the
  session read. Configuration changes are included: `apply_change` advances the point too.
- The answer comes once the entry has applied on the proposing member, like any replicated write. The `WriterBoundary`
  it returns rests on that apply.

This closes the race CockroachDB's automatic finalization leaves as future work, a node with an old binary joining just
before the bump. Here the proof and the bump are one transaction, which can't apply past a configuration change it
didn't see.

**Writers.**
- In replicated mode, every application session's preparation runs `application::check` with its handle's claim,
  against the floor at its base. This replaces `require_replication`'s blanket refusal.
- A claim below the floor is refused before proposal.
- A session prepared before a raise that commits first is refused at apply, by the stale-base rule.
- The claim needs no field in the entry, and every member decides alike.

**Membership.** A configuration change is a hyper-raft entry. The machine can't refuse one (`apply_change` fails only
fatally), and a configuration is in force from its append, not its commit (raft.pdf p. 11). So the owner guards
changes before proposing them.
- It adds a voter, or promotes a learner, only when that member's attestation under its current join is at or above the
  floor and the owner's tail is quiescent: its applied index is its last log index. A tail that isn't is answered at
  once, `Unavailable`, naming both indexes; the guard never waits.
- With a quiescent tail, every raise is wholly before the change, and the guard reads its floor, or wholly after it,
  and its proof reads the new configuration.
- A new member joins as a learner (raft.pdf section 6), catches up, attests, and is then promoted.
- A learner without an attestation at a version blocks raises to that version until it attests or is removed.
- Removing a member needs nothing more: its join row goes with the change's apply, and its attestation, keyed by that
  join, never vouches again. There is no window between a change and a cleanup for a crash to fall into.
- Safety doesn't rest on this guard. A member whose application is older than the floor has every write refused by
  `check`. The guard keeps such a member from becoming a voter.

**Start.**
- A member's application handle binds only after its owner has applied up to the commit index it learned at start.
- If the committed floor for its application is above its highest, binding fails closed, with a typed refusal naming
  both. CockroachDB's node exits the same way on a persisted version its binary can't support.
- Snapshots carry the floor and the attestations as rows of the image, beside the configuration (raft.pdf p. 12). A
  member installed from one holds the same proof.

**The seam.** The replicated `Database` gains:
- `raise_floor(application, version, until) -> WriterBoundary`;
- `floor_status(until)`: the floor and each member's role and attested highest, read without a claim so an
  application below the floor can see it and stand down. A replica answers what its owner published at its last
  applied index, at once; a standalone database reads its floor on a reader by `until` and names no members, being no
  group. The same call in both modes (CLAUDE.md section 8).

Attestation is the owner's own act when a handle binds, so it has no call of its own. The standalone `writer_floor`
stays as it is, the same proof over one member. New rows ride in resolved pages, and the origin check reads context
bytes that already exist, so no wire format changes.

**The owner's own sessions last their role's raft timing (2026-11-11).** A floor session and the seed's internal
attempt each had a picked 60 s lease. Each now takes its role's timing, read from the core when it is made
(`Actor::own_session_lease`; amphora-seam-owner-2, amphora-owner-t1-constants-floor-session-20261111). On a leader
that is two quorum checks, 2 × (election_tick + quorum_patience) ticks. The check runs on its own counter and clears
the answers' marks only when it runs, so a stall that begins just after an answer passes the next check and steps
the leader down at the one after (hyper-raft's raft.rs, `quorum_due`, `tick_heartbeat`, `MsgCheckQuorum`). On a
follower it is its election bound and its stall patience, (2 × election_tick + patience) ticks: it campaigns within
a randomized timeout below twice election_tick, plus its patience (`tick_election`). Past either, the session's role
has moved on, and it can't apply there. A live-quorum leader's session can still reach its lease, behind a slow flush
or a ReadIndex backlog. It then ends unapplied: safe, since the stale-base rule refuses a late apply and the keeper
makes it again. That is the falsifier's case, so each such end is counted and logged, as Kiln Metrics agreed them
(amphora-constants/names/v1): the counter `amphora_replica_own_sessions_expired_total` (no labels, one per replica
owner), and the event `replica.own_session_expired` at info on target `amphora.replica`, with its term and the
running count as fields. `amphora.replica` carries no spans, so kiln's observe can take it at INFO.

| panel | PromQL | kiln |
|---|---|---|
| Own sessions expired (patience too short), bars, beside quorum patience | `sum by (db) (increase(amphora_replica_own_sessions_expired_total[15m]))` | `sum:amphora_replica_own_sessions_expired_total{$host} by {db}.as_count()` |

Its monitor warns on any increase in 15 minutes: each one is a falsifier. The re-request period of a failed seed transfer is the election
timeout read from the core, where it was a bare 20 ticks. `an_owners_session_lasts_its_roles_raft_timing`: a
follower's and a leader's sessions take their role's lease (red at 490ea83: "left: 60s, right: 400ms"), and a
stalled leader's ends within its lease. Falsified by a floor or seed session that would have applied ending
unapplied again and again under a loaded but live quorum: many `replica.own_session_expired` events in one term mean the
patience is too short, and the fix belongs there.

**Decided (2026-11-06), a to d.** Codex's agent ended before it answered, so Amphora's membership-floor agent
decided them from the sources below and the code, and amphora-seam-owner agreed each (its record:
codex-notices/OWNER.md, section 7).
- a) **Crash-fault trust, no signatures.** The authenticated origin, bound by `step`'s sender check and the new
  context check, is enough. Raft promises safety only under non-Byzantine conditions, and Amphora's replication
  already trusts each leader with every entry's resolved pages and every snapshot's image: a member able to lie
  could write any row, a floor and attestations included, so a signature on an attestation would guard nothing that
  member could not already write. A proof that verifies from a snapshot alone needs a Byzantine model for the whole
  log, not for one table. The origin check is a single hop because nothing re-forwards (Attestation, above).
  Falsified by: a deployment that must hold the floor against a member running modified code, which needs Byzantine
  replication first; or a forward applied whose context names a member other than its sender.
- b) **Learners count, and so does every member of a joint configuration.** The members a raise's proof covers are
  all of the configuration applied at its base: `voters`, `learners`, `voters_outgoing` and `learners_next`, the
  set `step` takes records from. A learner applies every entry and forwards writes, and is a promotion away from
  voting; etcd and CockroachDB call a cluster upgraded only once every member runs the version. A raise is a session,
  and its proof reads the configuration at its base: a joint one is refused `Unavailable` ("the group is leaving a
  joint configuration") before it proposes. A change appended but not applied at its base is not visible there; a
  raise beside it applies only if no change applied between its base and its index, since the stale-base rule
  refuses it otherwise, so it is never applied while a member it did not prove belongs to the configuration
  (T-T1.8.A2). `writer_floor` answers that refusal `Unavailable`, the retry class, "the database advanced since
  preparation: the group changed while the raise was proven; raise again"; a raise whose entry another lead replaces
  stays `UnknownOutcome`. A learner that has not attested blocks a
  raise, and the refusal names it. Falsified by: a floor applied while a learner of the configuration at its index had
  no attestation at its version under its current join.
- c) **One `u32`, the highest version the binary runs.** It is the claim U05's standalone floor already carries
  (`Application::version: u32`). The floor is a lower bound on writers and only rises, so the lowest version a binary
  runs is never asked: a rollback below the floor is refused by design, as CockroachDB refuses downgrades once its
  version is bumped. An application that can't run on a floor as old as the one committed reads `floor_status` at
  bind and refuses itself; that is the application's compatibility, not the floor's promise. A lowest version is added
  when an application declares one, as a start check beside the attestation that doesn't change it; today it would be
  speculative (CLAUDE.md section 13). Falsified by: a promise that needs a ceiling on writers (no writer newer than N),
  which a range would carry and this does not.
- d) **The proof replaces `require_replication` in replicated mode, and fail-closed stays.** In replicated mode,
  `application::check` runs in every preparation against the floor at its base, in place of the blanket refusal.
  Still refused, `Fenced`: a handle with no claim on a database with a floor, a claim for another application, and a
  claim below the floor; a raise without proof is refused typed, naming the members short. The bind of a member whose
  highest is below the committed floor fails closed at once, `Invalid` ("application floor": "this binary runs up to
  3; the group's floor is 4"), with no wait and nothing proposed: a binary can't raise its highest by waiting, so the
  host must replace the binary. The owner publishes the floor in force at each applied index it refreshes, and the
  bind reads that; a bind racing a raise not yet applied there passes, and its writes are then `Fenced` at
  preparation, so nothing below the floor applies either way. A floor this group's members didn't prove never enters replicated
  mode: one held where joins aren't tracked (a standalone owner's `writer_floor`) is refused `Fenced` at open, at the
  seed and at an image's install, "a floor not proven by this group's members; raise it again once joins are tracked".
  A raise to the floor in force or below answers that floor at once, proving and writing nothing (promise 3; a caller
  asking for an older contract is served by the newer one, as kiln's `writer_boundary` asks). A read is checked against
  the floor as a write is, standalone and replicated alike. The remote owner's takeover stays refused by `writer_floor`
  ("original non-restoring owner required"), which this proof does not cover (U05: "Replication and remote-owner takeover need
  their own application-floor proof"); `require_replication`, the blanket refusal, is gone.
  Falsified by: a replicated write applied with a claim below the floor in force at its index.

**How it composes with membership changes (t1/membership-fixes, 065fdb3 and 49bf342).** One owner, one slot for a
change (`Changing`), one guard:
- `ReplicaHandle::change` admits a change at its ReadIndex barrier in `member_change`, which already refuses a change
  while another is pending or the configuration is joint. The floor's guard goes there, beside `admits_joiner`, in the
  same admission: `Promote` needs the learner's attestation of the floor's application under its current join at or
  above the floor, read on a quiescent tail (the applied index is the last), so what it reads is the state at the
  change's index. A tail still to apply is refused at once, `Unavailable` ("the log runs to L and A is applied"); a
  learner with no such attestation, `Unavailable` (it may yet attest), naming the learner and the floor; one attested
  below it, `Invalid` ("application floor": "learner 4 runs kiln.host-store up to 6; the group's floor is 7"), since
  waiting can't raise a binary's highest. A group with no floor promotes as before. `AddLearner` needs no attestation, only the snapshot ceiling `admits_joiner` already checks: a
  learner joins, catches up, binds and attests. `Demote` and `Remove` need none.
- `ReplicaHandle::transfer` hands the lead only to a voter, and every voter passed the guard or a raise's proof, so a
  transfer needs no check of its own. The leader is demoted or removed only after it has handed its lead on.
- `raise_floor` is a replicated write in the write slot (`Pending`), never the change slot. Beside a change, it holds
  by its base and the stale-base rule (b, above); a raise forwarded to a leader that is handing its lead over is
  dropped by the core as any proposal is (`Dropped::Transferring`), and its origin answers its caller.
- A change appended and then replaced by another lead's entry gives back the configuration before it (Ongaro's
  dissertation, section 4.1), and with it the members a raise must cover; a replaced change never applies, so it
  writes no join row; the stale-base rule refuses a raise whose base saw either.

**Tests, before code.** Deterministic simulation on driven time with a nemesis (elections at each step, a leader's
crash, lost records), each citing its ID in docs/requirements.toml:
- AC-T1.8.A1, T-T1.8.A1: the CockroachDB race. An old-binary learner is added just before a raise; the raise is
  refused naming it, or the learner's promotion is refused; never both through.
- T-T1.8.A2: a leader crashes between a change and a raise; whoever leads next, the floor's proof covers the
  configuration in force at the raise's index, or the raise is refused.
- T-T1.8.A3: a member installed from a snapshot holds the floor, the join rows and the attestations, and its proof and
  its refusals are the leader's.
- T-T1.8.A4: hostile input: a forwarded proposal whose context names a third member is refused, typed, and applies on
  no member.
- T-T1.8.A5: a member removed and added again attests under its new join; its old row never vouches and blocks nothing.
- T-T1.8.A6: every falsifier below, as an assertion over every run.
- T-T1.8.A7: the upgrade, as written below; and a leader restarted before J asks once, and each member attests once.

**Metrics** (agreed with kiln's Observe owner, the Kiln Metrics agent, under amphora-membership-floor/names/v1,
2026-11-06; CLAUDE.md section 10). Amphora adds one label, `reason` on the refusals counter; the host
adds `db` and its machine's.
- `amphora_floor_version` (gauge): the committed floor, 0 for none.
- `amphora_floor_raises_total` (counter): raises applied.
- `amphora_floor_refusals_total{reason}` (counter): `reason` is one of `unproven`, `stale_base`, `below_floor`,
  `no_claim`, `other_application`, `busy`. The set is closed: a seventh value is agreed first.
- `amphora_floor_unattested_members` (gauge): members of the configuration in force with no attestation at the floor
  under their current join. Every member reports it, from its own applied state, which is the same on every member,
  so dashboards take `max` by `db`.
- `amphora_floor_keeper_failures_total` (counter) and `amphora_floor_keeper_failing` (gauge, 1 while the keeper's last
  pass failed, 0 once one succeeds), agreed under amphora-membership-floor/names/v1-keeper: the keeper looks again only
  when the applied index moves, so on an idle database the counter stands still while the gauge shows it stuck.
- Events go on target `amphora.floor` with an `event` field named in kiln's dotted style (`floor.raised`,
  `floor.refused` with its `reason`, `floor.keeper_failed`); spans stay off that target, as kiln makes `kiln.<span>`
  metrics of a listed target's spans.
- Dashboard rows, a group "Amphora application floor" on kiln's built-in Stores dashboard and the same in PromQL in
  kiln-overview.json: floor version by database, raises, refusals by reason, members unattested, keeper failures
  (bars) and keeper failing (max by `db`), and floor events. Monitors: unattested above 0 for 15 minutes; the keeper
  failing at 1 for 15 minutes. kiln's side (`TARGETS`, exports, the group) is kiln's change, merged on
  Ada's word.

**The upgrade that introduces the join rows** (decided by amphora-seam-owner, 2026-11-06). While members run binaries
from before and after the floor's, a new binary that wrote join rows an old one doesn't would make their pages differ,
and replicas apply resolved pages and must stay byte-identical: the next resolved page applied over a differing one
would corrupt. So:
1. A marker J (`joins tracked from index J`) is written by a resolved session, so it reaches every member as pages.
   Until J is in a member's applied state, `apply_change` writes no join rows.
2. Before J, an attestation counts only if it committed after the last configuration change. The machine's configuration
   marker rewrites its point on every entry, and hyper-raft keeps the newest change's index privately and resets it to
   the applied point on restore, so neither says which index that was. The machine keeps `configuration_at` in memory
   instead: the change's own index when it applies one, the applied point when it opens or installs an image. That is
   never below the true index, so it can only delay J, never let a stale attestation count. Each attestation row carries
   `seen`, the applied index at its session's base, and counts before J only if `seen` is at or above both the prover's
   `configuration_at` and the last request's `at`, and only for the application it names. (This replaces an earlier
   wording that took the index from the configuration marker's point; that point moves with every entry.) It is sound:
   `configuration_at` is never below the true last change's index; a member removed and re-added gets a new change
   index, so its old attestation's `seen` is below it and never counts; an overestimate only excludes valid
   attestations, costing liveness, never safety; the proof is decided once, by the leader, in a resolved session whose
   pages the members apply without judging, so the leader's local estimate diverges nothing; a change after the
   session's base is refused by the stale-base rule, and a pending one by the pending-or-joint refusal. While J waits on
   any member without an attestation that counts (one never made, or one made before the leader's estimate, as after a
   restart), the leader writes `__amphora_attest_requested(at)` by a resolved session, and each member's owner, seeing
   it in its applied state, attests again. Both are bounded: a leader writes a request only while one at or past its
   `configuration_at` is not already written, so at most once per value it holds; a member's keeper attests only when
   its own row is missing or stale (a new join, a change it applied, a request it applied), proposes at most once per
   pass (a new applied index while it has nothing else in hand), through the same barrier and proposal as a call with
   its own deadline, and a refusal (a stale base, an election) is tried again on its next pass, never in a loop. Members
   attesting at once make each other's sessions stale. A floor session (a keeper's attestation, the leader's request)
   therefore prepares at its member's log tail: past its ReadIndex barrier, it waits until its member has applied its
   own last log index as it stood when the barrier passed, then prepares. The member's log already holds what the leader
   appended to it, other members' forwarded attestations included, so contenders share a base; a session prepared at
   what was only committed sits behind entries already in its own log, is stale when it arrives, and two followers out
   of phase keep missing each other (39 proposals for one applied attestation in the restart test before this rule). The
   target is fixed when the barrier passes, so writes appended after it never move it and the session never waits out
   its deadline holding its member's write slot; the wait is bounded by that deadline, a tail a new leader truncates is
   followed by the applied index, and it is local, with no protocol change. So each round of contention applies at least
   one, a member proposes at most `members` attestations per `configuration_at`, the retries end once every member is
   current, and a settled group proposes nothing. The leader's request is the same counted retry: written once, its
   session may be refused by the stale-base rule when members' attestation sessions prepared at the same base commit
   first, and is proposed again on the leader's next pass, so its proposals are at most one more than those refusals,
   each of which needs a session committed in between (at most one attestation per member per request), and none once
   the request is applied. Any change during the upgrade sends every member back to attest; that lasts only until J.
3. J's session is written only when every member of the configuration in force has an attestation committed after the
   last change. At J, the members then present get rows at 0, so their attestations stay valid and nobody attests
   again at J; after it, only changes write rows.
4. A raise is refused, typed, until J is committed: "joins aren't tracked yet; every member must attest first".
5. T-T1.8.A7: a group of mixed binaries through the upgrade, a member removed and re-added before J. Expected: no raise
   through until J, the re-added member's old attestation never counting, and every member's image digest identical
   at every applied index.

What this does not cover: a member running a binary from before the floor that joins after J applies later changes
without writing join rows, and its pages diverge. The floor can't refuse it from inside that old binary; the host
installs every member that joins with the current one. What bounds the harm: such a member can't attest, so the
membership guard never promotes it and it never counts in a quorum; and `floor_status` shows it unattested, which
`amphora_floor_unattested_members` above 0 for 15 minutes alerts on. Every table above is created by a resolved session,
never locally, so before J an old binary applies exactly what a new one does. Members must hold content-identical pages,
the checksum trailer aside: a page's last eight bytes are the engine's checksum of the rest, written when the page is
written out (`crates/engine/storage/checksum.rs`), so whether a read sees it depends on that timing, while the content
is what members must agree on. A test's image digest is of each page's content without that trailer.

Evidence:
- Raft's catch-up as non-voting members, a configuration in force from its append, and snapshots carrying the
  configuration (Ongaro and Ousterhout 2014, sections 6 and 7).
- CockroachDB's cluster version: a monotonic promise that no old binary runs, and a node that exits on a version it can't
  support (RFC 20170815_version_migration). Its automatic finalization checks every live node first and leaves the join
  race open (RFC 20180411, #24670).
- etcd: "The cluster is only considered upgraded once all of its members are upgraded" (etcd.io, upgrade_3_1).
- The code at 29be700: the stale-base rule in `Machine::apply`, `apply_change`'s point and configuration, `step`'s
  authenticated sender, `context()`, `validate_reserved_sql`. Kiln's U05 patch at kiln f77518103a.
- For a to d: Raft's safety holds "under all non-Byzantine conditions" (raft.pdf, section 2); U05's claim is one `u32`
  (`Application::version`) and its `require_replication` names remote-owner takeover as needing its own proof;
  CockroachDB has "no support for downgrades once the version setting has been bumped" (RFC 20170815); a server
  "must be prepared to fall back to the previous configuration in its log" when a change's entry is removed, and a
  leader to be removed transfers its lead first (Ongaro's dissertation, sections 4.1 and 3.10). The membership code
  at t1/membership-fixes 49bf342: `member_change`, `ReplicaHandle::transfer`, `Machine::admits_joiner`, and the
  test `a_removal_another_lead_wrote_over_gives_the_member_back`. The single hop: t1/election-flake 27d8ff8,
  docs/design/replication.md, "A forward that meets an election" (re-forwarding rejected).

Falsified by:
- a write prepared with a claim below the floor that applies on any member at an index after the floor's;
- a floor entry applied while a member of the configuration in force at its index had no attestation at its version;
- a floor that falls, or two members holding different floors at the same applied index;
- a voter added, or a learner promoted, without an attestation at or above the floor at its change's index;
- an application handle bound on a member whose highest is below the committed floor;
- a forwarded proposal applied with a context naming a member other than its authenticated sender;
- a proof that counts an attestation made under a member's earlier join;
- two members holding different join rows at the same applied index;
- a membership guard that waits, or a non-quiescent tail answered other than `Unavailable` naming both indexes;
- a join row written before J is applied, a raise applied before J, or two members' image digests differing at one
  applied index through the upgrade;
- before J, an attestation counted that committed before the last configuration change.

## A local read on a replica (2026-11-06)

CP-T1.10 (follower reads), kiln's every-node plan (kiln plan 01a1a283e14c, step 4; its vendored patch 0003, "A local
read on a replica"), Amphora goal 13f90c05. kiln serves every node from its own replica, and a node must read with its
group's leader gone or out of reach. Today every read on a replica goes through the leader: `Database::query` on a
replicated database runs as a `Database::call`, which crosses the leader's ReadIndex before it evaluates.

### What a replica's own state can promise

A replica applies only entries the group committed, in log order, and a preparation that is not committed rolls back
without a commit ("Resolved transactions" above). Its applied state is therefore always a prefix of the group's
committed history: a state the group was in, never one it was not. On one replica that prefix only grows, since its
applied index never goes back (it is durable with the state it marks). Nothing bounds how far behind the group's
commit that prefix is: a replica cut off from its leader keeps answering from where it stopped.

Against `Consistency`:
- **Linearizable** is not something a replica can answer alone. A linearizable read must see every write acknowledged
  before it began, which only the group's current leader knows, and only once a quorum confirms it still leads: the
  ReadIndex (Ongaro's dissertation §6.4). A follower may serve one by asking the leader for its read index and waiting
  until it has applied that far (same section), which still needs the leader. A lease read answers without that round
  but trusts bounded clock drift (§6.4.1); hyper-raft has none ("No clock is trusted", `read.rs`). Linearizable reads
  stay where they are: through the leader.
- **Snapshot(rev)** names a state of the group's history, and any replica that has applied that revision can answer it
  exactly, refusing it typed (`Ahead`) until it has. That is how Spanner serves a read at a timestamp at any replica
  whose safe time has passed it (Corbett et al., OSDI 2012, §4.1.3) and CockroachDB its follower reads below a closed
  timestamp (Taft et al., SIGMOD 2020). Records' Snapshot reads on a replica's own state are this decision's next
  step, not this change.
- **A local read**, what kiln asks for, names no revision: it reads whatever the replica has applied when it begins.
  It is etcd's serializable read ("serializable member-local reads", etcd's `RangeRequest.serializable`; its reads are
  linearizable unless asked so). It is neither of the two above, so it is a method of its own, never a Consistency.

### The decision

`Database::query_local` runs one read-only SQL session on this node's own committed state:
- on a replica, on a reader of the owner's applied state beside the owner, asking the leader nothing: it answers with
  the leader gone or out of reach;
- it sees a prefix of the group's committed history: never a preparation, a proposal, or anything the group has not
  committed; on one replica, never an older state than a local read that ended before it began;
- it promises no freshness: a caller that needs every acknowledged write uses `Database::query`, which crosses the
  leader;
- a learner promises exactly what a follower does. It applies the same committed entries and counts in no quorum, so
  it can confirm nothing, but its applied prefix is the group's (Ongaro's dissertation §4.2.1, members that catch up
  without voting; etcd's learners serve serializable reads only);
- refused, typed, while the replica has not taken its group's first image (its state is no group's yet), once its
  owner has ended, and while it is fenced;
- standalone, it is `Database::query`; an attached client's runs on its owner's state (the wire, below);
- it runs as `Database::query`'s session does (`run_query`): one read transaction, a restore under way refused, and the
  handle's application claim held against the floor its state holds. It asks nothing of the member's own attestation:
  a member below the floor runs a binary from before it, never this code, so such a check would protect nothing.
Its readers are bounded: at most four at once, each its own connection with its own pages; a read waits for one up to
its deadline and is refused Timeout past it.

### Amendments before code (2026-11-07, amphora-seam-owner's four additions)

The seam owner agreed the decision above with four additions to settle before code (key
amphora-owner-t1-replica-local-read-b40f41d-20261106).

**1. During an image's install: never half an image.** A replica that installs its group's image writes the image's
pages and its applied marker as one transaction of the database's log (`Db::apply_pages`: `wal_insert_begin`, a frame
per page, the marker, `wal_insert_end`). The frames carry no commit until the last, so a reader's snapshot, the last
commit in the log when its read began (sqlite.org/wal.html, "it first remembers the location of the last valid
commit record"; Turso's `begin_read_tx`), holds either none of them or all. A local read is one read transaction
(`Db::query_transaction`, a deferred `BEGIN`), so every statement in it sees the same side: the state before the image,
whole, if it began before the image committed, or the image, whole, if after. A reader open across the install keeps
its old state to its end, and the schema published with the image (`publish_schema_after_external_restore`) is the
next transaction's.

This holds whether or not anything copies the log back into the file while the reader is open, and doesn't rest on
nothing doing so. The open reader holds a read mark at its snapshot's last frame. A checkpoint copies no frame past the
oldest read mark held (`determine_max_safe_checkpoint_frame`, crates/engine/storage/wal.rs: a mark it can't take
exclusively caps what it copies), so the file never holds a page newer than the reader's snapshot where the reader
would read it from the file. A restart of the log takes every read mark exclusively first, and returns `Busy` while any
is held (`begin_restart`, the same file), so it never pulls a reader's frames away. These are SQLite's rules for
checkpoints under readers (sqlite.org/wal.html, "Checkpoint Starvation": a checkpoint stops at the oldest reader, and
the log starts over only once no reader uses it). Today nothing checkpoints a replica's database at all
(`wal_autocheckpoint = 0`, and no checkpointer runs there). That is a gap of its own, not this decision's (below).
Falsified by: a local read whose statements see two states, part from before an image and part from it; a read begun
after the install that sees any of the state before it; a checkpoint during an open read that changes what it sees,
or a log restart that goes through while it is open. The test:
`a_local_read_open_across_an_image_install_sees_one_side_whole` (T-T1.10.L4). A learner's read transaction is held open
while the leader's image installs under it, and in a second leg a passive checkpoint and a log restart run on the
learner's database during it.

**2. The four readers, and the fifth.** Four is policy, stated as the standalone store's: `store::Config::readers`
defaults to 4, and its pool has this rule (store.md, decision 3). It is not a measured number for this path. Each
reader keeps `db::READER_PAGES` (2,000 pages, 8 MiB, Turso's own default; store.md, decision 5), so a replica's local
reads hold at most 32 MiB of pages. A fifth read takes the first reader given back. It holds no reader, page or queue
slot while it waits, only its caller's thread, and it waits no later than its own deadline: past it, it is refused
`Refusal::Timeout`, typed, never answered late (CLAUDE.md section 2: the wait's bound is the caller's deadline, its
overflow a typed refusal). A read that can't wait asks with a deadline that is already near. Once the owner ends,
waiting reads are woken and refused `Fenced("replica owner ended")`. The waiting thread is never the owner's actor: a
local read runs on its caller's thread (`Client::read_local`, the pool's condition variable), and an attached
client's `LocalSession` on its connection's own worker (`amphora.client`, one per connection, at most
`CONNECTION_MAX`, each bounded by the session's deadline). So a fifth read stalls no other request to the owner.
Falsified by: five local reads at once on one
replica; a waiting read answered past its deadline, or answered anything but Timeout or Fenced while all four are out.
The test: `a_fifth_local_read_waits_for_a_reader_and_is_refused_timeout_at_its_deadline` (T-T1.10.L5): four reads
held open on four threads, a fifth on the caller's thread while the owner's actor steps meanwhile.

**3. The wire.** An attached client's local read is a new request, `Request::LocalSession`, appended (its variant last),
whose session runs the same commands as `Request::Session`. The owner runs it with `Database::query_local` and the
session read-only. Appending a variant changes the protocol, so `wire::VERSION` goes from 5 to 6, with its line under
local-owner-transport.md, "Versions". The client asks `LocalSession` directly, without probing the owner's
capabilities and without falling back to `Database::call`. The greeting refuses a peer of another version before any
request, so a version-6 client only ever meets an owner that takes it, and the fallback could never run (CLAUDE.md
section 13). Kiln's 0003 probed and fell back, written before the version gate counted appended requests. For the same
reason no owner advertises a capability for it: no client would read it. The value 2 (bit 1;
`CAP_BOUNDED_COMPACTION` is 1, bit 0), which kiln's 0003 used for `CAP_LOCAL_READ`, stays unassigned, so a later
capability doesn't take a meaning a kiln build once gave it. Falsified by: a version-5 peer admitted by a version-6
owner (`a_peer_of_another_version_is_refused_at_the_greeting`, which greets with `VERSION - 1`); a `LocalSession` that
writes or leaves a write behind (`an_attached_clients_local_read_writes_nothing`, T-T1.10.L7: its insert refused typed,
the count the same before and after, the client never promoted).

**4. Monotonic across a restart, and where it isn't.** Within one opening of a replica, local reads never go back: each
begins on the last commit in its database's log, and the owner's commits only add to it. Across a restart, what the
replica reopens with is what its database's log holds, and that depends on how the replica stopped:
- after a clean close, the owner flushes everything applied (`StateMachine::persist`), so the restart opens where the
  last read saw. Monotonic.
- after the process dies with the machine running, a commit's frames were written to the file before any reader could
  see them, and the OS keeps written data that no sync covers yet, so the restart opens no earlier than the last read.
  Monotonic.
- after a power cut (or the OS's crash), the database keeps what its last finished flush made durable (`durable()`), and
  a read can have seen more than that: every entry applied since the last flush that finished, in either class, as
  both leave their database's flushes to the owner's flushing thread ("Fast acknowledgement", point 4; "A replica's
  database log", 2026-11-09, which moved the persisted class's there too: before it, the persisted machine flushed each
  entry before it applied the next, so at most one). Until the restarted replica applies them again, from its own log
  (the persisted class's raft log holds each durably before it applies) or its group's, a local read there can see an
  older state than one before the cut. It is never older than the state the last finished flush made durable, and it is
  always a prefix of the group's committed history.
This is a stated limit, not a guarantee: "never older than a local read before it on the same replica" holds within one
opening and across a clean close or a process crash, not across a power cut. A caller that must never see a state go
back across a power cut reads through `Database::query`, the leader's ReadIndex. etcd's serializable reads promise no
more: a member answers one from its own state, which may be stale (etcd's API guarantees, its serializable reads).
Falsified by: a local read after a clean close or a process crash that sees an older state than one before it; after a
power cut, one older than the replica's last finished flush. The test:
`a_local_read_after_a_power_cut_is_no_older_than_its_last_flush` (T-T1.10.L6). Replicated class: a write read
locally, its flush held, the member's power cut. The restarted member's read sees the state before the write, never
less, and the write once it has caught up. The persisted class, the same:
`a_persisted_local_read_after_a_power_cut_is_no_older_than_its_last_flush`.

**Requirements.** CP-T1.10 names follower reads but no criterion of what one returns. Amphora's own are registered
beside it (docs/requirements.toml): AC-T1.10.L1 (what a local read returns, and when it is refused), AC-T1.10.L2 (its
readers' bound and the fifth), AC-T1.10.L3 (the wire). Their tests: T-T1.10.L1
`a_local_read_answers_with_the_leader_gone`, T-T1.10.L2 `a_local_read_never_sees_what_the_group_has_not_committed`,
T-T1.10.L3 `a_local_read_waits_for_the_groups_first_image`, and T-T1.10.L4 to L7 above. Its latency (AC-T1.10.3,
follower read p99 at most 2 ms) is measured by bench amphora_read, not by this change.

**Metrics** (CLAUDE.md section 10), agreed with Kiln Metrics under `amphora-local-read/names/v1`, in the replica
meter (`meter::REPLICA_METRICS`, read through `ReplicaHandle::counters`):
- `amphora_replica_local_reads_total`, a counter: local reads this replica's own readers ran;
- `amphora_replica_local_reads_refused_total`, a counter, by `reason`, a closed set (`meter::LocalRefusal`):
  `timeout` (no reader before its deadline, or the deadline already past), `fenced` (the owner ended or is
  fenced), `bootstrapping` (no first image yet). A fourth reason is agreed with Kiln Metrics first;
- `amphora_replica_local_readers_in_use`, a gauge: readers reading now, 0 to 4. The bound is a constant
  (`READERS`), so it is not exported.
No other labels from Amphora; kiln adds `db` and its machine labels. Dashboard rows, in the group **Amphora
replicated class**:

| panel | PromQL | kiln |
|---|---|---|
| Local reads/s | `sum by (db) (rate(amphora_replica_local_reads_total[5m]))` | `sum:amphora_replica_local_reads_total{$host} by {db}` |
| Local reads refused, by reason, stacked | `sum by (db, reason) (rate(amphora_replica_local_reads_refused_total[5m]))` | `sum:amphora_replica_local_reads_refused_total{$host} by {db,reason}.as_count()` |
| Local readers in use, against 4 | `max by (instance, db) (amphora_replica_local_readers_in_use)` | `max:amphora_replica_local_readers_in_use{$host} by {host,db}` |

Tested by `a_fifth_local_read_waits_for_a_reader_and_is_refused_timeout_at_its_deadline` (the gauge, the reads run,
timeout and fenced) and `a_local_read_waits_for_the_groups_first_image` (bootstrapping), and the names by
`the_replica_metrics_are_the_ones_the_design_names`.

**Found beside it: a replica's database log is never copied back.** Nothing checkpoints a replica's database. Its
writer runs with `wal_autocheckpoint = 0` (`Db::open`), and only a standalone store runs a checkpointer
(`Store`'s `Readers::start`, `Engine::checkpoint` for `Local`). The machine's code only flushes (`Machine::advance`,
`persist`, the replicated class's flushing thread). So the log grows with every entry applied, which breaks CLAUDE.md
section 2. It is its own goal, not this change. Amendment 1 holds once it is closed. Closed by "A replica's database
log: copied, bounded, and flushed off the owner" below.

**Withdrawn:** the floor binding (no local read on a member that hasn't attested). A member below the floor runs a
binary from before the floor, which doesn't have this code.

Evidence and sources: the resolved-transaction design above (only committed entries apply); Ongaro's dissertation
§4.2.1, §6.4 and §6.4.1 (https://raft.github.io/raft.pdf); hyper-raft's `read.rs`; etcd's API reference
(`RangeRequest.serializable`) and its learner design; Corbett et al., "Spanner: Google's Globally-Distributed
Database", OSDI 2012, §4.1.3; Taft et al., "CockroachDB: The Resilient Geo-Distributed SQL Database", SIGMOD 2020.

Falsified by:
- a local read that sees a preparation, a proposal, or any write the group has not committed;
- a local read on a replica that sees an older state than one that ended on that replica before it began;
- a local read answered before its replica took its group's first image, or once its owner ended or was fenced;
- a `Database::query` on a replica answered without the leader's ReadIndex;
- more than four readers at once on one replica, or a read that waits past its deadline.

## The class stated once per group (proposed 2026-11-09, Amphora goal 11209e8d)

"Across nodes" above left this open. Each member's owner states its own class, and a leader counts held answers
by the fates it was given. If two nodes were given different fates, whichever leads counts by its own map, and a
held quorum can then sit within one real fate.

**The decision.** The class is stated once, in the group's log, and every member holds itself to it:
- **The entry.** A class entry is a log entry of its own kind: the prefix `amphora.class.v1\0`, then the postcard
  encoding of the class as `api::replication::Durability`, its fates in canonical order (`database::canonical`,
  sorted by member). It holds at most what the largest class encodes to (`ENTRY_BYTES`, 2026-11-11, in place of a
  picked 64 KiB): postcard's varints (postcard.jamesmunns.com, "Wire Format": a tag or u32 at most 5 bytes, a length
  or u64 at most 10) for the tag, the fates' length, two u64s for each of the most members (the group's member bound, below)
  and the span: 1,317 bytes with the 17-byte prefix; a class past the members is refused
  (`a_class_entry_holds_the_most_members_and_no_more`). A leader proposes it in its term whenever its machine records no class: at bootstrap that is
  the group's first leader; for a group from before this change, the first leader that runs it. Two leaders may
  each propose one. A second entry stating the same class applies as nothing.
- **Recorded with the state.** A member applies the entry in one transaction with its applied marker. That
  transaction writes the class into its database (`__amphora_class`, one row), so an image carries it and
  `install` takes it.
- **Refused, typed, at apply.** A member whose own stated class differs from the entry's fences. Its owner ends
  `Refusal::Fenced("the group states another class: ...")`, naming both classes, and its status shows the reason.
  A committed entry can't be skipped by one member: the log is the group's, so the member that disagrees is the
  one that stops. This is also why the rule holds whichever member leads first: whoever's entry commits first is
  the class, and every member stating another is refused.
- **Refused, typed, at open.** A member whose database already records a class refuses to open stating another:
  `Refusal::Invalid { what: "replicated class", why: "the group's log states another class" }`. The same fates
  in another order are the same class.
- **No held quorum until the class is the member's own.** A member counts held answers only once it has applied
  a class entry equal to its own, or opened on a database that records it. Until then it counts as the persisted
  class does: its owner tells the core that every other member is suspected (`Replica::suspect_held`, vendor
  change 0005, already used by the suspicion feed). Once the class is applied, it trusts every member the host's
  detector doesn't suspect. The detector's verdicts and the class gate are kept apart: a member is held-counted
  only when neither suspects it. The class entry itself commits classically, on durable answers.

**Amendment (2026-11-09, amphora-seam-owner's answer at fb18105): how the class changes, and a second entry.** The
seam owner agreed the direction, and blocked on one point: the class needs a way to change. The procedure above
("A member added or removed") has members reopen stating the joiner's fate. A class fixed in the log would refuse
that. The change path is a superseding entry, held to one rule: compatibility.
- **Compatible.** Two classes are compatible when they are the same kind (both persisted, or both replicated with
  the same `spanning`) and give the same fate to every member both name. A member named by only one is free. That
  is a joiner's fate added, or a departed member's dropped.
- **The record is the last compatible entry.** A class entry compatible with the recorded class supersedes it: its
  map becomes the record. An entry incompatible with the recorded class is inert. Every member applies it as an
  entry that changes nothing: it moves the applied point and records nothing. It is deterministic on every member,
  because it depends only on the log. So a leader stating a conflicting map can never overwrite the group's class.
- **Each member holds itself to the record.** After every class entry it applies, and at open, a member's own
  stated class must be compatible with the record. If it isn't, the member fences at apply, or is refused
  `Invalid` at open. A leader whose conflicting entry went inert is refused by that same rule, when it applies its
  own entry.
- **Who proposes: the record only grows between changes of members** (the seam owner's refinement, key
  amphora-owner-t1-class-and-readers-4cfb126-20261109). A leader proposes the record joined with its own stated fates,
  restricted to the members its configuration names, voters and learners (`class::joined`), and only when that differs
  from the record (`Machine::class_proposal`). So:
  - a fate is added only for a member that has none recorded;
  - a fate leaves the record only once its member has left the configuration;
  - a leader whose stated class names fewer members than the record proposes nothing for them.
  Without this, a leader stating a subset would drop a sitting voter's fate, and two leaders stating different
  compatible subsets would flip the record each term: churn, and a held quorum lost for no reason. The joiner's
  procedure is unchanged: a member reopens stating the joiner's fate, and once it leads, its entry records it. A
  leader with no class recorded proposes its own, restricted the same way.
- **What can't change in place.** `spanning`, the kind (persisted or replicated), and a sitting member's fate. A
  member that moves to another fate is removed, then the group's class is recorded without it (a reopening leader
  proposes that), and then it is added again under its new fate. Changing the kind or `spanning` takes a new group
  (an export and an import). It is a stated limit: a group's quorum system changes only through members added and
  removed, which is what Flexible Paxos's intersection argument covers.
- **Why it is safe.** A held count counts a member by its fate. Every member that runs agrees with the record on
  every member the record names, and so with every other running member on every member they both name. So no
  two leaders count one member under two fates. A member that only the leader names (a joiner not yet recorded) is
  counted by the operator's statement, as today. hyper-raft counts no held answer from a voter without a stated fate
  (`held_half`).
- **Q1 accepted** (an older binary fences, typed, on a class entry): on T-T1.8.A7's terms, as a stated limit, and
  told to Duplicate Systems for kiln's rollout.
- **Tests added:**
  - `a_joiners_fate_is_recorded_by_a_compatible_class_entry` (T-T1.8.C5): three voters record a class, and a fourth
    member is added as a learner. A follower reopens stating its fate too, and is handed the lead. Every voter then
    records the wider class, and none gates held answers. The lead then passes three times between that member and
    one stating the narrower class, with a write in each lead: no leader proposes, and the record changed twice in
    all, never back.
  - `a_class_entry_survives_an_image_and_a_restart` (T-T1.8.C4): an incompatible entry is inert; an image's record
    refuses a member stating another class; a reopening under another class is refused `Invalid`.
  - `classes_are_compatible_only_on_every_member_both_name`: the compatibility and the join.

**What it costs.** A class entry moves the applied point, as every entry does. So a write prepared before it, at any
member, is refused at apply as "the database advanced since preparation": the same typed refusal a concurrent writer
or a configuration change gets, which its caller prepares again. This happens at most once per recording: under the
group's first leader, and under the leader that records a joiner's fate. A leader holds its own writes back while
its class entry is in flight (`class_in_flight`), so only a write prepared at a follower in that window meets it.

**The gate, exactly** (the code review at 552c0a1). A member counts held answers only once every fate it states for
a member of its configuration is in the record, the same kind and span: stated ∩ configuration ⊆ record
(`class::recorded_all`). Compatibility alone isn't enough. Two members stating different fates for a joiner not yet
recorded are each compatible with the record, and each would count by its own. Until the record holds its fates, a
member holds every other member suspected. Test: `a_fate_not_yet_recorded_opens_no_held_count`.

**Formats.** The log gains one entry kind under a prefix of its own, which is how the seed (`amphora.seed.v1`)
and the baseline delta were added: append-only, no existing entry read differently. The database gains one
table. `api::replication::Durability` and `Fates` become a frozen format: their postcard encoding is the class entry's
payload and the table's row, so their variants and fields are only ever appended, never reordered or retyped (their
doc comments say so). The owner transport's wire doesn't change: the entry travels in raft's log, which the host carries
(VERSION stays 6). An older binary meets the entry as a command it can't decode, and fences, typed, naming it.
So a group is upgraded member by member before a member running this code leads. That is the same order the
floor's join rows asked for ("The application floor, replicated", T-T1.8.A7). Kiln's own rollout states it.

**Sources.** Raft keeps the group's configuration as log entries, so that every member takes it from the log and
not from its own flags (Ongaro, "Consensus: Bridging Theory and Practice", 2014, §4.1; https://raft.github.io/raft.pdf
Figure 3 and §5.3, Log Matching; §5.4, Leader Completeness). A class entry is that rule for the class. Once
committed, it is in every later leader's log (Leader Completeness), and a member applies it before any entry after
it. Schneider ("Implementing Fault-Tolerant Services Using the State Machine Approach", ACM Computing Surveys
22(4), 1990, §3): replicas agree only if every input that decides their behaviour is ordered by the protocol.
Flexible Paxos (Howard, Malkhi, Spiegelman, OPODIS 2016, §3): a quorum system is safe only if every participant
uses the same intersecting quorums. Fates that differ across members are two quorum systems.

**Evidence (tests, failing first).**
- `two_members_stating_different_classes_the_second_is_refused_typed` (T-T1.8.C2): three voters, two stating one
  map of fates and the third another. Once the leader's class entry commits, the third's owner is fenced, naming
  the class. Reopened stating its own map, it is refused `Invalid`; reopened stating the group's, it opens and
  catches up. Also: a member stating the persisted class in a replicated group is refused the same way.
- `no_held_quorum_is_counted_before_the_class_entry_applies` (T-T1.8.C3): durable answers are held from the start.
  No write is answered on held answers alone until the class entry has committed. Once it applies, with durable
  answers held again, a write is answered on held ones.
- `a_class_entry_survives_an_image_and_a_restart` (T-T1.8.C4): a learner installed from an image records the class
  and refuses another at open.

**Falsified by:** a held commit counted by a member before it applied the group's class, or by one that stated a
different class; a member that applies a class entry differing from its own and keeps running; a database that
records one class opening under another; an older entry read differently.

## A replica's local readers bounded by derivation (proposed 2026-11-09, Amphora goal 314d704a)

Amendment 2 above made four readers policy, borrowed from the standalone store's default. kiln's rule is no picked
numbers (Duplicate Systems, kiln agent 01a1b1935e17). The two things readers cost are what bound them, so the
bound is derived from those.

**What a reader costs.**
- **No file descriptor.** A reader is a connection on the machine's engine `Database`. `Database::connect` makes a
  pager on the database's file and shared log, which it already holds (`init_pager`, crates/engine/database.rs:
  `self.db_file.clone()`, `self.shared_wal`). So the process's descriptor limit (`RLIMIT_NOFILE`, POSIX
  `getrlimit`) doesn't bound readers.
- **Its pages.** `READER_PAGES`, 8 MiB (store.md, decision 5, measured), kept while it is idle.
- **A caller.** A reader is in use only while a read runs on its caller's thread.

**The decision.**
1. **Count: at most `CONNECTION_MAX` readers out at once on one replica.** That is the transport's bound on attached
   clients (32, `transport::wire`). Each client's connection is one worker running one session at a time, so no
   more readers than that can be in use for clients. In-process callers past it wait, as the fifth waits today: on
   their own thread, until their deadline, then `Timeout`. Nothing else here is a stated number.
2. **Pages: every reader is charged to the process's page budget.** Its `READER_PAGES` are charged to the process's
   shared page pool (`db::process_pages()`, a quarter of the memory the process may use; store.md, decision 26).
   The charge is taken when a reader opens and returned when it closes. While charged, the pool keeps that many
   fewer pages: `SharedPages::charge`, an engine change recorded in ORIGIN.md. Insertions into a shard keep it under
   `per_shard - ceil(charged / SHARDS)`, evicting by CLOCK. So the readers' caches and the pool's shared pages
   together never pass the pool's budget, and the count across all of a process's replicas is derived from its
   memory: on a 16 GiB machine, 4 GiB, 1,048,576 pages, 524 readers; where the system doesn't say, 32; at the floor
   (16 MiB), 2.
3. **Idle readers.** They live in one process-wide account, so one replica's idle readers never starve another's
   reads. A read takes, in order:
   - an idle reader of its own replica;
   - a new one, if the charge is granted;
   - a new one after closing another replica's idle reader, oldest given back first, which returns its charge;
   - the first reader given back or charge returned, before its deadline.
   Past the deadline it is refused `Timeout`, typed, and its replica is counted under the existing `timeout` reason
   (no new metric name). A waiter waits on the account's condition variable, with a generation count against lost
   wakeups, and holds nothing while it waits. Once a replica's owner ends, its idle readers close, its waiting reads
   are refused `Fenced`, and none opens again.
- AC-T1.10.L2 becomes: at most `CONNECTION_MAX` local readers out on one replica, and their pages within the
  process's page budget. A read past either waits on its caller's thread until its deadline, then is refused
  Timeout, or Fenced once its owner ends.

**Amendment (2026-11-09, amphora-seam-owner's answer at fb18105).**
- **`CONNECTION_MAX` is policy.** It is the transport's stated bound on attached clients, and the reader count
  borrows it. No measurement made 32. What is derived is that more readers than callers who can use them buy
  nothing.
- **The no-descriptor claim is tested.** `a_local_reader_opens_no_file` counts this process's open descriptors
  (`/dev/fd`, on Unix) before and after eight readers open on one database, and expects the same count.
- **The pool's floor under charge.** A charge is refused once it would leave the pool fewer than one page a shard
  (`SHARDS`, 64). So the pool always keeps a page in every shard, and a charge past it waits like any other.
- **The engine's charge follows CLAUDE.md section 1.** Checked arithmetic, no panics. It is recorded in the engine's
  ORIGIN.md, and the ratchet's counts don't rise.
- **The process-wide account is bounded.** Its idle readers are at most what the charge holds (budget ÷
  `READER_PAGES`). Each replica's `out` is at most `CONNECTION_MAX`. Replicas are registered by id and removed when
  their owner ends. A reader that closes, for an account reclaiming it or for an owner that ended, is dropped after
  the account's lock is let go.
- **The charge is shown.** A gauge of the bytes charged to the
  process's pool by every replica's readers, its name proposed to Kiln Metrics and wired once agreed; the dashboard row reads "against
  32".

**Sources.** PostgreSQL bounds backends by `max_connections` and their shared cache by `shared_buffers`
(documentation, "Connections and Authentication", "Resource Consumption"). These are the two dimensions here: the
callers bound the count and memory bounds the pages. Store.md decisions 5, 20 and 26 measured the reader's 8 MiB
and the pool's budget. CLOCK eviction (Corbató, 1968), as decision 20. The engine's `connect` (above) shows readers
take no descriptor.

**Evidence (tests, failing first).**
- `local_readers_are_bounded_by_the_transport_not_four` (T-T1.10.L8): `CONNECTION_MAX` reads held open on one replica
  all run. One more waits and is refused `Timeout` at its deadline while the actor steps, then answers once one is
  given back. It is red at four today.
- `local_readers_past_the_process_page_budget_wait_and_reclaim_idle_ones` (T-T1.10.L9): an account of two readers'
  pages and two replicas. Replica A's read leaves an idle reader. B's two reads run, the second closing A's idle
  reader. A's next read waits and is refused `Timeout`. The charge never passes the budget, and once all are given
  back it is the idle readers' alone.
- Engine, `a_charge_shrinks_what_the_pool_keeps`: a pool of `n` pages charged `c` keeps at most `n - c` after
  inserts, and keeps `n` again once the charge is returned.
- T-T1.10.L5 (the fifth read) is rewritten as T-T1.10.L8.

**Falsified by:** more than `CONNECTION_MAX` readers out on one replica; readers' charged pages plus the pool's kept
pages above the pool's budget; a read refused while an idle reader of another replica held the charge it needed;
a read answered past its deadline.

**Amendment (lane constants, 2026-11-21): the count follows the descriptor budget.** `CONNECTION_MAX` is gone
(local-owner-transport.md, "Decision (lane constants): one descriptor budget"). Connections are bounded by the
process's descriptor budget, each holding its socket, so the most clients that can be attached at once is the
budget's bound, and a replica's readers out at once are at most that, as it stands at each read. Nothing else in this
decision changes: readers hold no descriptor and charge none, their pages are charged to the page budget, and a read
past either waits to its deadline. Where `CONNECTION_MAX` is named above, read the budget's bound. AC-T1.10.L2 and
T-T1.10.L8 say so; the test sets a budget of its own (6) and holds six readers out, the seventh waiting.

## A replica's database log: copied, bounded, and flushed off the owner (2026-11-09)

Amphora goals 7582f00a and aa81fbbf, one design because both are the replica's database-log I/O. Requirements:
AC-T1.8.D1 and AC-T1.8.D2, tests T-T1.8.D1 and T-T1.8.D2 (docs/requirements.toml).

### What was wrong

- **The log grew without bound** (CLAUDE.md section 2). A replica's database is opened by `Db::open` with
  `wal_autocheckpoint = 0`. Only a standalone store runs a checkpointer (store.md, decision 4). Nothing on the
  replicated path ran `Db::checkpoint` or `Db::restart_log`, so a replica's `-wal` file took every entry it applied:
  1,526 frames for 300 entries on a learner in the red run of T-T1.8.D1. A leader's own log was started again now
  and then only by accident: each preparation is a write transaction, and the engine starts a wholly copied log
  again when a write transaction begins (`try_restart_log_before_write`). No pass ever copied it wholly. A follower
  or learner prepares nothing.
- **The persisted class flushed on its owner's thread.** `Machine::apply` flushed the database's log for each entry
  (`advance(.., flush = !replicated)`), so a flush that stood still stood the owner still. Followers heard nothing,
  campaigned, and terms moved 169 to 171 times in 1,800 s under load (docs/research/owner-stalls.md, "An open
  liveness cost"; step gaps to 3.2 s). Only the replicated class had a flushing thread ("Fast acknowledgement").

### The decision

1. **Every class flushes on the owner's flushing thread** (AC-T1.8.D2). `Flushing::start` spawns
   `amphora.replica.flush` in both classes, and `Machine::apply` flushes no ordinary entry in either. The owner hands
   the thread the point it applied, one flush out at a time, and takes the point back once the flush made it durable
   (`flushed_through`); `durable()` stays at what a finished flush covers. Still flushed inline, all rare:
   `apply_change` (its join rows ride it), `install` (durable before it returns: durable.md, I8), the seed, `open`,
   and `persist`, which hyper-durable's compaction asks before it moves the raft log's start (I8).
   It is safe for the persisted class because `acts_at_start` stays true for each of its entries. hyper-durable
   applies such an entry only once this member's own raft log holds it durably (durable.md §4.1, I5), and an answer
   leaves only after that apply. A power cut that loses the database log's unflushed tail reopens at `durable()`, the
   last flushed marker, which is in the same transaction as its pages. `Replica::open` then applies the rest again
   from the raft log (`keep_after(durable.index)`, §4.3). A compaction never passes `durable()` (I8). So what changes
   is when the database's file catches up, not what is acknowledged. A local read after a power cut is never older
   than the last finished flush, as "A local read on a replica" already states for every class.
2. **The flushing thread copies the log into the file** (AC-T1.8.D1). After a flush it runs a checkpoint pass on a
   connection of its own (`Db::checkpoint`, passive), bounded by the frame the flush made durable (store.md, decision
   15). A passive checkpoint never copies past the oldest reader's mark, so a local read's view stays whole
   (sqlite.org/wal.html, "Checkpointing"). While the log is within its bound, a pass follows a flush at most every
   25 ms, the store's measured period (store.md, decision 4). Past the bound, as the writer counts its frames, a pass
   follows every flush. A pass that copied nothing new waits for the next flush, which on this thread is automatic
   (store.md, decision 25).
3. **The owner starts the log again**, on the machine's own connection between steps, as the store's writer does
   (`Db::restart_log`: Restart, or Truncate past `keep_log`, store.md decision 31). It is the owner's because a
   restart takes the engine's write lock: on another connection it would make the owner's next `wal_insert_begin`
   busy, and a failed apply fences. The owner never waits for a reader: with a flush or a local read out, it tries
   again at its next step. When a restart is due, when reads are held for it, and how far the log may grow are
   "A replica's database log bounded by what it measures" (2026-11-11), below, which replaces this point's picked
   bound (`LOG_FRAMES`, 131,072 frames, and a 64th of it) and points 4 and 5. A leader's log is also started again
   by the engine itself: a preparation is a write transaction, and the engine restarts the log at a write's begin
   when every frame is copied and no reader reads from the log (`try_restart_log_before_write`; SQLite's
   `walRestartLog` does the same, wal.c). That is safe beside the owner's restart: it discards no frame that isn't
   in the file, it needs no reader in the log, as the owner's does, and the preparation's rollback writes nothing
   into the restarted log. The flusher's durable prefix is taken against the log's generation, which a restart
   moves (store.md, decision 15), so neither restart makes the other's state wrong.
4. *(Replaced, 2026-11-11: the bound is stated in "A replica's database log bounded by what it measures", D.)*
5. *(Replaced, 2026-11-11: the disk a log takes is that section's C.)*
6. **Busy is a skip; only I/O fences.** A checkpoint pass that finds the log busy copies nothing and reports nothing:
   its `Checkpointed` is the default, and the owner keeps what the last real pass said. A restart that finds the log
   busy (`Db::restart_log`'s `Ok(false)`) is tried again at the next step. A pass or a restart that fails for any
   other reason fences the owner, as a failed flush does: the file's state is unknown after it (store.md, decision 9's
   argument), so the failure is typed and reported, never silent.
7. **The local reads' pool.** The restart reaches the readers through two calls on `Local`: `hold_reads` (no read
   begins on this replica until the restart) and `restart_unread` (the restart, run only while no read of this replica
   is out). Whatever the pool becomes (amphora-class-and-readers' goal 314d704a, one account across replicas), it
   keeps those two per replica.

### Evidence

As of 2026-11-09. The D1 tests below were rewritten on 2026-11-11's rule (next section), which replaced their
lowered bounds with a lowered room.

- T-T1.8.D1 `a_replicas_database_log_stays_within_its_bound`: a voter and its learner, the learner's bound lowered to
  64 frames; 300 writes of a 3,000-byte row. Red on main: 1,526 frames, no pass. With the change, at most twice the
  bound after every write, restarts and passes counted, every row read back locally and after a restart.
- T-T1.8.D1 `a_local_read_holds_off_the_logs_restart_and_one_after_waits_for_it`: a local read open across 39 writes
  on a learner with a 16-frame bound. No restart under it, and its view stays whole. A read that came while the
  restart was due was refused `Timeout` (red on main: it answered at once). A restart once it ended.
- T-T1.8.D2 `a_held_database_flush_stalls_no_persisted_owner_step`: three persisted voters, the leader's flushes held,
  rounds for four election timeouts at least. Red on main: the leader's owner reached the held flush on its own
  thread. With the change it never did. The write was answered with its flush held, and the term and the leader
  didn't move.
- T-T1.8.D1 `a_checkpoint_pass_copies_no_frame_past_the_last_finished_flush`: a learner's flushes held, 1 flushed,
  2 to 5 applied unflushed. A pass copies through the frame the flush covered, and no further (store.md, decision 15).
- T-T1.8.D1 `a_restart_due_waits_for_a_reads_deadline_and_no_longer`: a local read with no end, under a deadline on a
  clock the test moves, out while the learner's log passes twice its bound. No restart while it runs. It is refused
  at its deadline, and the restart follows.
- T-T1.8.D2 `a_member_power_cut_loses_no_acknowledged_persisted_write`: the power-cut nemesis on three persisted
  voters, 12 seeds: 92 writes acknowledged, 36 cuts, 16 of them of a leader holding writes unflushed, none lost. And
  `a_persisted_local_read_after_a_power_cut_is_no_older_than_its_last_flush` (T-T1.10.L6 in the persisted class).
- The terms moved under load, before and after, are measured with step 1's measurement
  (`production_owner_stalls_measured_on_this_host`) and recorded in docs/research/owner-stalls.md. That is a
  measurement of the owner's liveness under a load the run makes, recorded as such: not a performance claim and not
  a CLAUDE.md section 7 benchmark.

### Metrics

In the replica meter (`meter::REPLICA_METRICS`), as Kiln Metrics agreed them under `amphora-replica-log/names/v1`
(it dropped a separate passes counter, as the histogram's count gives it, and named the restarts after the database's
log, a replica having two logs):
- `amphora_replica_checkpoint_seconds`, a histogram on the flush's buckets: each checkpoint pass the flushing thread
  completed that copied, and its time; its `_count` is those passes. A busy pass (a reader on read-mark 0 beside it)
  copies nothing and is not counted;
- `amphora_replica_database_log_restarts_total`, a counter: times the owner started the database's log again;
- `amphora_replica_database_log_bytes`, a gauge: the database log's length as the owner last counted it
  (frames × 4,120).
`amphora_replica_flush_backlog_entries` and `amphora_replica_flush_seconds` now count the persisted class too.

| panel | PromQL | kiln |
|---|---|---|
| Database log against its limit (bytes; the next section) | `max by (instance, db) (amphora_replica_database_log_bytes)` and `max by (instance, db) (amphora_replica_database_log_limit_bytes)` | `max:amphora_replica_database_log_bytes{$host} by {host,db}` and `max:amphora_replica_database_log_limit_bytes{$host} by {host,db}` |
| Checkpoint seconds p50 / p99 | `histogram_quantile(0.5 / 0.99, sum by (le, db) (rate(amphora_replica_checkpoint_seconds_bucket[5m])))` | `p50:` and `p99:amphora_replica_checkpoint_seconds{$host} by {db}` |
| Checkpoints/s | `sum by (db) (rate(amphora_replica_checkpoint_seconds_count[5m]))` | `count:amphora_replica_checkpoint_seconds{$host} by {db}` |
| Database log restarts/s, bars | `sum by (db) (rate(amphora_replica_database_log_restarts_total[5m]))` | `sum:amphora_replica_database_log_restarts_total{$host} by {db}.as_count()` |

### Falsified by

- (the bound's falsifiers are the next section's);
- a checkpoint pass that changed what an open local read sees;
- a log restart while a local read was out, or a local read that began while a restart was due;
- a persisted owner that flushes an ordinary entry's database log on its own thread, or an acknowledged write lost
  after a power cut in the persisted class (the power-cut nemesis);
- terms that move as often under the same load after the change as before.

## A replica's database log bounded by what it measures (2026-11-11)

Amphora goal 7582f00a's bound, redone on Ada's word (2026-11-11): "We don't tolerate arbitrary constants, learned
and stat derived values only." Requirements: AC-T1.8.D1, tests T-T1.8.D1 and T-T1.8.D3 (docs/requirements.toml).
Agreed by amphora-seam-owner-2 before code (amphora-owner-t1-log-bound-design-20261111 and
amphora-owner-t1-log-bound-rev1-20261111; codex-notices/t1-log-bound.md).

### What was wrong

The 2026-11-09 decision (above, point 3) bounded a replica's log at twice `replication::LOG_FRAMES` (131,072
frames: about 1.08 GB a replica a group) and restarted it once a pass left at most a 64th of that. Neither was
derived. `LOG_FRAMES` is the standalone store's default, the better of two candidates (16,384 and 131,072) at one
load on one host (store.md, decision 4: fedora 2026-10-20, 32 writers), and the 64th is a fraction picked to keep
the restart's own copy small. The one number served two purposes that want two rules: how often the owner may stop
to restart (a latency question) and how much disk the log may take (a resource question).

### The decision

A pure core, `database::log_bound` (CLAUDE.md section 9), which the owner drives with each step and tick, and the
flushing thread with what it reads after each pass. Nothing in it is picked.

**A. A restart spends a share of the commit tail, counted.** A restart holds the owner, and every entry the owner
applies until it is idle again waits for it. The owner counts, in entries as this member applies them (every member
applies every commit once, on a leader, a follower and a learner alike):
- *applied*: every entry;
- *held*: every entry applied from a restart's start until the backlog it made is gone, the first step after it
  that begins with no more input queued than the restart found. Queued input is the owner's own count of admitted
  bytes not yet taken, its channel's and those waiting behind the prepared transaction (each envelope's
  reservation). It is read once, as each step begins, with the step's own input still reserved, and a restart takes
  the reading of the step it runs in, so both are on one basis. Read inside the restart instead, after the step's
  input was dropped, a steady backlog read one envelope more at every later step and the hold never ended (red
  first: `a_restart_hold_ends_on_an_owner_busy_at_a_steady_queue`, amphora-owner-t1-log-bound-004f682-20261111).
  An idle step, with none queued, is one such step. That covers what arrived during the restart, what
  queued behind its backlog, and the commits in flight when it began. It overcounts, which is the conservative side.
  An owner under steady load, never idle, still ends the hold once its queue is back where it was (tested, and
  red-first: ended only at an idle step, such an owner held every entry after its first restart for good, never
  came due by share again, and its log grew to the room: amphora-owner-t1-log-bound-4a274ec-20261111);
- *over*, the base tail: an entry applied, not held, by a step that took longer than AC-T1.3.4's bound on a
  commit's p99.9 (15 ms). This is a proxy: it is the delay the owner added itself, past the p99.9. A commit's
  quorum and network time aren't the owner's to see, and don't change with restarts;
- *next*: the most one of the last 1 / (1 - q) restarts (a thousand) held, the restart about to come guessed by the
  worst seen: the worst of n holds estimates their 1 - 1/n quantile, the quantile the rule holds.

A restart is due when (held + over + next) / applied < 1 - q, all over the current generation: from the restart
that began it, and ended by the restart made once it alone covers its own spend so ("Decision (lane constants): a
generation of restarts ends once it covers its spend"). Here q = 0.999 is the highest
quantile the registry holds a commit's latency to (AC-T1.3.4's p99.9, and AC-T1.4.3's). The code keeps q as the
fraction 999/1,000 and compares cross-multiplied, with checked arithmetic. The comparison is strict, net of the
measured base tail, and counts the coming restart, so a restart that holds no more than the worst seen leaves the
restarts' share of the tail below 1 - q. The first restart is free: nothing is held yet. A restart at low load that
holds nothing costs nothing. One that holds some is spaced by the count, and at least once a window once the window
forgets. The growth rate cancels. As reasoning only: with R a restart's time, restarts at least R / (1 - q) apart
hold at most 1 - q of the time, and arrivals that see time averages (PASTA, Wolff 1982, Operations Research 30(2))
would make that the share of commits. R = 12 ms gives 12 s, which falls between store.md decision 4's two runs:
131,072 frames restarted every 5 to 7 s with p99.9 untouched, and 16,384 every 0.74 s with p99.9 up 3 to 5 ms. The
counting needs no arrival assumption, and bursty arrivals are what it counts.

What the rule holds and what it can't: it keeps the owner's own delays past the p99.9 within its tail by
construction. With h of the tail held, the p99.9 becomes the base's quantile at 1 - (0.001 - h), and what the owner
can't see (quorum, network) can move that by more than AC-T1.4.3's 1 ms. That 1 ms is held by the batch's
benchmark, restarts on against off, not by this rule.

**B. A restart waits for a pass that copied every durable frame.** It is ready when it is due, no flush and no read
is out, and the last pass caught up: it copied through the frame the flush before it made durable (its cap, store.md
decision 15; the flushing thread reports `backfilled >= durable`, or nothing left). What the restart then copies on
the owner is only what the owner applied since that flush was asked, at most one flush's growth, the least any rule
can leave. This replaces "a 64th left". Reads are held (`hold_due`) when a restart is due and the last pass either
caught up (the restart waits only for the reads out) or found the log busy: a read that began on a log wholly copied
holds read-mark 0, and a passive pass is busy beside it (wal.c, `walCheckpoint` takes `WAL_READ_LOCK(0)`). The
flushing thread now reports a busy pass instead of dropping it, and a busy pass is the fact that "twice the bound
however much is left" guessed at. While a restart is due, every flush is followed by a pass, and one is asked even
with nothing to flush.

**C. The log's room, from the filesystem, at each pass.** After each pass the flushing thread (never the owner)
reads:
- *A*: the bytes the filesystem holding the database has available to the process: statvfs's `f_bavail` × `f_frsize`
  on Unix (POSIX `statvfs`), `GetDiskFreeSpaceExW`'s caller-available bytes on Windows, through fs4 0.13.1 (already in
  Cargo.lock through tantivy);
- *F*: the frames the log's file already holds room for, its length reused (store.md decision 13);
- *N*: the replica logs on that filesystem, a live count on the process-wide local-reads `Account`, keyed by the
  filesystem's number. That is `st_dev` on Unix, and the volume serial on Windows (`GetFileInformationByHandle`,
  through winapi-util). Two Windows volumes with the same serial count as one and undercount N, the unsafe side:
  rare (serials are set at format), and stated. On any other platform every log counts as on one filesystem, an
  overcount, so each takes less. A log is counted from its owner's flushing thread's start until the thread ends (`LogShare`). Every replica
  owner, of either class, runs one; a test's `Flushing::Held` doesn't count.

The room, in frames: H = (F × 4,120 + (A - SNAPSHOT_BYTES) / N) / (4,120 + 4,096), with no share once A is below one
image. Each frame takes its own bytes and the page its copy may add to the database's file: a log must leave room for
its own copy, or it can't be drained. One image is kept back, so a snapshot install has room. The free bytes are
shared equally among the logs that may grow into them. Counted in bytes, a frame the log grows by moves bytes from A
to its file, so the room holds still for one log, and for more grows by less than half a frame per frame, the
others' growth seen at the next pass. Free bytes that can't be read give no share (H = what the file holds), and
the refusal says why. A file whose length can't be read fails the pass, and the owner fences, as any failed pass
does.

Past H, on every member, a restart is due whatever the share says, and it counts as held: disk before latency.
Without that, an owner whose own base tail has spent the share would never restart, its log would stay past H, and
its submits would be refused for good. Reads are held. New submits are refused `Refusal::Unavailable("the
database's log holds {frames} frames, past the {H} it may hold: ...")`, typed and retryable. No new variant: none
until kiln shows a caller that must branch on it, and `Refusal` is a wire enum. A follower can't refuse what its
group committed, and the leader can't see a follower's disk without a wire change, so a follower past H applies on.
If its disk then fills, the flush fails and the owner fences with the I/O error, typed, its image whole on reopen
(tested). A leader past H is the same: it refuses submits made at it, but the proposals its followers forward
(`MsgPropose`) are stepped and applied, since dropping them would leave a request with no answer on the wire
(CLAUDE.md section 3). Its log grows on, and on a full disk it fences the same way. The mechanism is the follower
test's, a failed flush on the flushing thread fencing the owner whatever its role, so no test of its own. Both are
limits of availability, not of safety. What a restart keeps of the file (`Db::keep_log`) is set
at each restart to H. A file past what the disk allows is cut (store.md decision 31); one within it is kept. Decision 31 cuts a file past `keep_log` to nothing, not to a length, and cutting costs more than reusing the
file. On a 1 GiB log, a restart that cut it held the writer 66.0 ms (best of 2) against 42.4 ms (best of 6) for one
that kept it. The commit of 4 MiB and its flush after it took 79.7 against 36.0 ms. That is
`crates/amphora/tests/files.rs` `what_cutting_the_log_costs_its_writer`, release without LTO, adacachy, 2026-10-29,
load 66-100, recorded in store.md decision 31 at 3261b48. It is a check, as decision 31 says, not a CLAUDE.md
section 7 run: the direction is what this decision rests on, not the milliseconds. It was min(H, the log's length) until 2026-11-12, which cut the file at most
restarts and paid that cost each time.

Not seen by this rule except through A shrinking at the next pass: the raft log (hyper-durable), standalone stores'
logs, the database file's own growth, and other processes. H is the replica logs' share of the process disk budget
once that budget exists (the goal the previous owner asked for, amphora-owner-disk-budget-goal-20261109). Its hook
is `log_bound::log_room(available, logs, kept)`: the budget passes its room for replica logs as `available`. One
registry, not a second budget beside it. One image is kept back per filesystem, not per group: several groups
installing at once (a node rejoining behind on all its groups) can still meet a full disk mid-image, and fence
typed. `SNAPSHOT_BYTES` (256 MiB, "until streaming snapshots") is itself picked, noted to amphora-constants.

**D. The bound, stated.** A replica's database log holds at most min(H, what its group commits between restarts
due by their share + what it commits while the local reads out at the due point finish). Each of those reads is
bounded by its caller's deadline (`run_query`'s progress handler), and no new one begins. Past H no new submit is
taken on that member. Restarts spend at most 1 - q of the entries, net of the owner's base tail.

How often each value is computed: A, F and N after every checkpoint pass, at most one a flush (one statvfs a pass);
the counts at each owner step and tick; the restart's held at each step until idle.

### Evidence

On adas-mac-studio.local-2 (macOS aarch64), 2026-11-11; the core's tests in `database::log_bound`, the owner's in
`database::replicated`:
- T-T1.8.D3 `the_first_restart_is_free`, `a_restart_spends_what_it_held_and_spaces_the_next` (held 5: due at 10,001
  applied, not 10,000), `long_steps_spend_the_tail_before_any_restart`,
  `a_generation_ends_once_it_covers_its_spend_and_is_forgotten`, `the_guess_is_the_worst_of_the_last_thousand_holds`.
- T-T1.8.D3 `a_restart_due_again_by_its_share_on_an_owner_never_idle`: a restart that found 3 bytes queued holds 7
  entries until a step begins with 3, and through a million steps never idle a restart comes due again. Red on
  4a274ec, whose hold ended only at an idle step: every one of 1,000,005 entries held.
- T-T1.8.D3, the share: `restarts_made_when_due_keep_their_share_below_the_tail`. 240,000 steps of 1 to
  7 entries, a long one in 20,011, each restart made when due and holding 1 to 9. After every restart that held no
  more than the worst before it, held + over stays strictly below a thousandth of the entries applied over the
  generation it ended and the one it began.
- T-T1.8.D3 `a_log_takes_its_file_and_an_equal_share_of_what_is_free_past_one_image`: the room at its edges, and a
  frame grown from free bytes into the file leaves one log's room where it was.
- T-T1.8.D3 `the_first_restart_is_free_and_the_next_waits_on_its_share`: a learner restarts after its first write
  (on b151af0 none, its bound being 131,072 frames). Told of a restart that held one entry, it makes none while 20
  more apply, and makes one once its generation covered its spend.
- T-T1.8.D3 `past_its_room_a_replica_refuses_submits_and_restarts_whatever_its_share`: a voter whose share is spent
  (a million held) and whose filesystem has nothing free past one image. With a local read held open, a write is
  refused `Unavailable` naming the frames past the room, and no restart is made. Once the read ends a restart
  follows, and the next write is taken.
- T-T1.8.D3 `a_follower_past_its_room_applies_on_and_fences_when_its_disk_fills`: a learner past its room refusing
  submits applies 6 to 10. Its flushes then fail as a full disk's do (`StorageFull`), it fences ("failed a flush"),
  and reopened on its files it holds 1 to 10.
- T-T1.8.D3 `a_dropped_replica_leaves_its_filesystems_room_to_the_others`: two logs counted on a tempdir's
  filesystem, then one once an owner dropped and its flushing thread ended.
- T-T1.8.D1, rewritten on the room:
  - `a_replicas_database_log_stays_within_its_room`: 300 writes, both logs at room for 64 frames each; each within
    its room plus what two writes grow it;
  - `a_local_read_holds_off_the_logs_restart_and_one_after_waits_for_it` and
    `a_restart_due_waits_for_a_reads_deadline_and_no_longer`: past a room of 16, no restart under a read, one after
    it;
  - `a_read_on_a_wholly_copied_log_holds_passes_off_and_a_busy_pass_holds_reads`: a restart due by its share, a busy
    pass, reads held, the log within its room;
  - `the_owner_holds_reads_past_the_room_or_once_a_due_restart_can_go`: the rule at each state.

### Metrics

One gauge more in the replica meter (`meter::REPLICA_METRICS`), agreed by Kiln Metrics under
`amphora-log-bound/names/v1` (2026-11-12): `amphora_replica_database_log_limit_bytes`, the room H as the owner last
learned it, in bytes (frames × 4,120). It is the name amphora-replica-log/names/v1 reserved for an exported bound,
`_limit` as Kiln Metrics names the set's other bound. The dashboard's "Database log against its limit" row (unit B) plots it
beside
`amphora_replica_database_log_bytes` (the table above). Restart spacing reads from
`amphora_replica_database_log_restarts_total`'s rate.

### Falsified by

- restarts whose held and the base tail, counted over the window, reach a thousandth of the entries applied after a
  restart that held no more than the worst before it (the share test);
- a commit p99.9 that rises with restarts on against off, at a load where restarts are due by their share (the
  batch's benchmark: AC-T1.4.3's 1 ms is its to hold);
- a submit taken on a member whose log was past its room; a restart not made past the room once no read was out
  and a pass had caught up;
- a disk filled by replica logs while each was within its room. statvfs's `f_bavail` is an estimate on btrfs
  (compression, and space its allocator still holds) and on APFS (purgeable space counted free, and containers
  sharing one pool), so a filesystem may hold less than it said;
- a restart that copied more than what was applied after the last caught-up pass's flush;
- a snapshot install that met a full disk mid-image while its filesystem was within every log's room (several
  installs at once on one filesystem are the stated exception);
- a reopen too slow at a log near its room: a reopen reads the whole log to rebuild its index (wal.c,
  `walIndexRecover`), and readers' wal-index lookups grow with it. The room bounds disk, not that time, so it is
  measured in the batch's heavy runs.

## A follower's write under steady leader writes: the write slot (decided 2026-11-12, Amphora goal 9c63f7d4)

Ada's word (2026-11-12, through the Amphora agent): G now, and D later as a goal of its own on G's slot. Design
agreed by amphora-seam-owner-2 under `amphora-owner-t1-g-slot-design-20261111`, which kept the previous owner's four
points (`amphora-owner-t1-write-starve-044c44f-20261109`) and ruled on three more. Sources:
docs/research/follower-write-starvation.md (f1 to f21). The A/B, made on t1/write-starve at 044c44f with the
prototypes that stay there: bench/records/2026-11-07-write-starve-model.md (a model, simulated time),
2026-11-09-write-starve-driven.md (the prototypes on the driven harness), 2026-11-09-write-starve-kiln-leg.md (kiln's
real-process promotion test, debug). The Amphora agent's release leg (kiln 7ad48c5, cec37f60, five interleaved
rounds): today's rule promoted no runner of 5, `write_again` 64 to 256; the prototype of G promoted 5 of 5 with no
unknown outcome, but each runner's `write_again` still reached 4 to 8.

**The finding.** A prepared write applies only if the entry before it is its base (`replication::on_base`, f1). A
follower's write reaches the leader's log a ReadIndex round, its preparation and half a round trip after the leader's
own would, so under steady leader writes it is refused round after round, and kiln re-runs it until its deadline
(f3).

**The rule.** The kind of slot Kung and Robinson prescribe for a transaction that keeps failing validation (f6), as
the fallback of an optimistic rule, MOCC-style (f7). Once one of a member's writes is refused, that member is queued,
and its next barrier (its ReadIndex) waits at the leader for its slot. While a member holds the slot, the leader
appends nothing of its own and holds the other members' forwards, until the holder's next forward arrives or the
slot's length passes. Today's exact base stays the only admission rule: no entry format, wire or engine change. The
pure core is `database/slot.rs`, driven by the owner (`Actor::keep_slots`, `start_slot`, `step_forward`,
`step_ready`).
- *When a member is queued.* At the leader's append of its refused entry, not at that entry's commit or apply. The
  verdict is a function of the entry and the entry before it (`replication::verdict`), so the append decides the
  refusal its commit will decide. A leader never displaces an entry of its own term (Raft's Leader Append-Only
  property, Ongaro and Ousterhout, USENIX ATC 2014, figure 3), and the queue dies with the term, so no displaced entry
  queues a member (the ruling's Q2 a). The leader's apply can lag its commit (hyper-durable's R-6 isn't built), so the
  prototype, which held its slot from its apply, could let the leader's next write in ahead of the retry.
- *When a slot starts.* When the queued member's next barrier arrives and its turn comes, not at the refusal. This
  departs from the agreed design, which started the slot at the refusal's commit; the code notice asks for the
  ruling. A slot started at the refusal holds the leader for a member that may never come back: a caller that
  doesn't retry, or a floor keeper that retries only once the applied index moves, which a held leader stops. Such a
  slot can only expire, and the late retry it then measures teaches a longer slot, feeding back toward the ceiling.
  Started at the barrier, a member that never comes back never holds the leader, and the measured gap is the retry
  alone: from its barrier's start to its forward. The holder's second barrier before any forward means its slot's
  work was no write (a read, or a write that prepared nothing), and the slot ends `released`. A queued member whose
  next command is a read, and then nothing, holds the leader once, until its learned slot expires: one slot, counted
  `expired`, and it teaches nothing, since no forward follows (amphora-owner-t1-g-slot-878285a-20261111). Agreed
  there, as better than the ruling it departs from.
- *What a slot holds* (Q1). Everything the leader would append between the holder's base and its retry: its own
  writes and floor sessions (`prepare` defers them, `slot_deferred`), its class entry (`propose_class`), and other
  members' forwarded proposals, held after the sender and origin checks. A change of members or a hand-off ends the
  slot `released`, since membership doesn't wait on one member's retry; no slot starts while either, or a bootstrap,
  is under way. The forwards stepped at a slot's end were mostly prepared on an older base, and are refused: correct,
  since each such member then joins the queue (Q1 d).
- *The holder's barrier* (Q2 b). A ReadIndex answers the leader's commit, not its last index, and a follower whose
  progress is probing or paused may not have been sent the entries between. So the leader steps the holder's
  ReadIndex only once its commit reaches the last index it had as the slot began. It appends nothing after that, so
  the commit gets there. Every write a follower makes then prepares at its log tail: the floor session's rule, now a
  follower's every write's (`Actor::tail`).
- *The leader's turn* (amphora-owner-t1-g-slot-878285a-20261111). A turn whose write is refused is a turn in name
  only: without the rule below, every write the driven runs made in the leader's turn was refused (29 of 29 in W2's
  first seed), which inverts the starvation onto the leader. So in its turn after a slot, the leader prepares only
  once it has applied its own log through the last index it held as its barrier passed. That index is the holder's
  entry and the forwards stepped at the slot's end. Other members' forwards stay held through the turn, until the
  leader's own write is appended, or until the next start finds it has none to make. A second forward from a member
  held in the turn ends the turn. So the turn's write follows those entries and nothing else, and none was refused
  in any W2 run. Outside a turn the leader's writes keep today's rule. A general tail rule at the leader is a
  separate question: in the driven runs it refused the leader at most once in a row, against up to 8 without it.
  But two floor tests, `a_raise_beside_a_change_holds_through_a_lost_lead` and
  `a_raise_whose_base_is_joint_is_refused`, rest on a leader write prepared beside an entry in flight, and the
  change is beyond G's. It is raised as a finding, not decided here.
- *More than one starving member* (point 2). A queue, each member at most once; slots go to the queued members in
  the order their barriers arrive, each barrier waiting at the leader until its turn. Between two slots the leader
  gets its own turn: one preparation, if it has a write to prepare, before the next slot starts. So the leader waits
  at most one slot between its writes, and a member's barrier waits at most (members − 1) slots. A second forward from a member
  whose first is held ends the slot `released`, and both are stepped in order (Q1 a), since stepping only the first
  would append an entry ahead of the holder's retry. It is a new write: each owner forwards one at a time, so the
  first one's caller has been answered by then.
- *Elections and members* (point 1). Slots are leader-local and belong to one term. When the term changes or the lead
  is lost, the slot ends `released`, its holds and the waiting barriers step into the core as they would have on
  arrival (today's "A forward that meets an election"), and the queue is forgotten. A change of members or a hand-off
  does the same within the term. A member that leaves the configuration, or that the host's detector suspects,
  leaves the queue, its waiting barrier steps, and its slot ends `released`.

**The slot's length is learned, not picked** (Q3; Ada's rule, 2026-11-11). It is learned the way the quorum patience
learns quorum gaps (`patience`), and adds no number. For each member, the leader measures the gap from the start of
that member's slot to its next forward, and keeps the longest gap of this generation and of the one before
(each generation as long as the member's last two learned gaps were apart: "Decision (lane constants): generations
learned where they run"). A member's slot is the larger of those two. A retry that arrives after its slot expired
is still measured, so an expiry teaches the next slot rather than censoring it. Before a member's first measured gap,
its slot is the core's election timeout in the owner's ticks (`election_tick × TICK`), the derived first value. No
number bounds a gap: a gap over which the host's detector suspected its member is refused, typed
(`slot::Unmeasured::Suspected`), logged (`write_slot_unmeasured`) and not learned: that member was gone, not slow. The
detector's verdict is learned where it runs; the slot no longer borrows `patience::QUORUM_CEILING`, a tail measured
once on one host (Ada's word, 2026-11-21: such a value is picked). TICK and the twenty-tick election timeout are the pinned core's
documented tick mode, not measured. They are noted for amphora-constants, and the slot inherits them only as its
first value.

**Why the prototype's runners still retried 4 to 8 times.** (a) It had a single slot, which the last refused member
overwrote, and nothing held the other members' forwards, so kiln's two runners refused each other in turn. (b) Its
length was a fixed 200 ms. (c) It held the slot from the leader's apply. The queue and held forwards close (a), the
learned length closes (b), and the queue at append with the held barrier closes (c). The `outcome` label tells (a) and
(b) apart in a run: a slot lost to time ends `expired`.

**Bounds.** The queue, the waiting barriers, the learned gaps and the held forwards are each at most one per member
of the configuration (at most the group's member bound): the owner keeps them to its members each step (`Slots::keep`). A second
barrier from a member whose first still waits steps the first, and a second forward from a held member ends the slot
or the turn. The held messages number at most (members − 1)
forwards plus one ReadIndex per member. Each keeps its admission reservation, so its
bytes, at most `MESSAGE_BYTES` each, stay charged to the inbound queue (`QUEUED_BYTES`) until stepped. The `ready`
queue holds what one end released and is drained in the same step.

**Metrics** (CLAUDE.md section 10), in the replica meter (`meter::REPLICA_METRICS`), names sent to Kiln Metrics under
`amphora-g-slot/names/v2`:
- `amphora_replica_write_slot_holds_total`, a counter, by `outcome`, a closed set (`meter::SlotEnd`): `forwarded` (the
  holder's next forward arrived and was stepped first), `expired` (its learned length passed first), `released` (the
  lead or term ended, the member left or was suspected, a change or a hand-off began, or a held member forwarded
  again);
- `amphora_replica_write_slot_held_seconds_total`, a counter: the seconds the leader's own writes were held for
  slots;
- `amphora_replica_write_slot_length_seconds`, a gauge, leader only: the learned length of the slot the leader last
  began, 0 before its first; it sits beside `amphora_replica_quorum_patience_seconds`, learned the same way.
No labels from Amphora besides `outcome`; kiln adds `db` and its machine labels. Each end is an event on
`amphora.replication`, `event = "replication.write_slot"` (debug), with the fields `member`, `outcome` and
`held_seconds`, attributes and never tags; kiln takes `amphora.replication` at WARN, so it stays a plain log line.
What a slot held and a lost lead discarded is the event `replication.write_slot_discarded` (warn), with the counts
`forwards` and `barriers` (amphora-owner-t1-g-slot-fc28e2d-20261112). A held record the core refuses as it is finally
stepped is `replication.write_slot_step_refused` (warn), with `from` and `refusal`. Both are warn, with no counter,
because the Ok a held record's receipt gets says only that its owner processed the wire record. It never tells the
origin's caller that its write was taken: that caller is answered by the write's apply, or UnknownOutcome at its
term change or deadline, so neither event hides an acknowledged write. Agreed with Kiln Metrics under
`amphora-g-slot/events/v1`.
Dashboard rows, in the group **Amphora replicated class**:

| panel | PromQL | kiln |
|---|---|---|
| Write-slot holds, by outcome, stacked bars | `sum by (db, outcome) (rate(amphora_replica_write_slot_holds_total[5m]))` | `sum:amphora_replica_write_slot_holds_total{$host} by {db,outcome}.as_count()` |
| Time held (share), % | `100 * sum by (db) (rate(amphora_replica_write_slot_held_seconds_total[5m]))` | `100 * sum:amphora_replica_write_slot_held_seconds_total{$host} by {db}` |
| Learned slot length, s | `max by (instance, db) (amphora_replica_write_slot_length_seconds)` | `max:amphora_replica_write_slot_length_seconds{$host} by {host,db}` |

**Requirements and tests.** AC-T1.8.W1 (docs/requirements.toml):
- T-T1.8.W1, `a_refused_follower_write_lands_on_its_next_try_under_the_slot`, with its control
  `todays_rule_refuses_a_follower_twice_in_a_row_where_the_slot_lands_its_retry`, an interleaving the test builds with
  held forwards. The randomized run can't be its own control: its interleaving follows the log's own I/O threads, so
  whether today's rule starves its follower is the host's timing, not the seed's. With each write taking a
  millisecond of real time, the same seeds gave different counts in 9 runs of 9, and the Linux gate at load 25 to 50
  found no starvation;
- T-T1.8.W2, `two_followers_and_a_leader_writing_steadily_all_commit_within_their_deadlines`, which also asserts that
  no write the leader made in its turn after a slot was refused, and that it took some;
- T-T1.8.W3, `a_follower_retry_racing_the_slots_start_lands_on_its_next_try`, red with the tail rule alone;
- T-T1.8.W4, the core's own tests in `database::slot` (the queue and turn, the held forwards, a second forward, expiry
  and a late retry learned, the ceiling, generations, the term and members, the held barrier).
W1 and W2 run Elle's list-append checks (f21) on every history: replicas equal, each list its serial replay, and no
G0, G1c or G2 cycle.

Falsified by:
- a non-serializable history under the slot, on the driven harness or in kiln;
- a follower's write refused more than (members − 1) times in a row under steady writes, or kiln's `write_again`
  above that, once the learned length has a generation of measurements;
- an `expired` share of a member's slots above 1/(n + 1) beyond its binomial 99% interval, where n is the number of
  that member's retries measured in the generations its length rests on. The longest of n exchangeable gaps is
  exceeded by the next with probability 1/(n + 1) (the rank of a new draw among n + 1 is uniform; David and
  Nagaraja, *Order Statistics*, 3rd ed., Wiley, 2003), so a higher share means the retries aren't exchangeable
  across a generation and the learned length doesn't fit them;
- the leader's held seconds, as a share of its time, costing it more of its write rate than the model's 0.32 for G
  (bench/records/2026-11-07-write-starve-model.md) at the release benchmarks' rates and round trips;
- a leader that holds its writes past a member's learned length for a write that never comes, or at all for a
  member that never comes back after its refusal.

## Decision (lane follower-reads): Follower reads at a closed time (2026-11-21, G13 step 4, kiln goal 80d4cf5e)

CP-T1.10 asks for follower reads "after ReadIndex, or at bounded staleness" (AC-T1.10.3: p99 at most 2 ms). A local
read on a replica ("A local read on a replica", above) already answers without the leader, but says nothing of *when*
the state it saw was the group's. This decision gives every replica's state a time, so a follower read can say the time
it read at and refuse to answer older than a time its caller asks.

### How CockroachDB does it, and what carries over

CockroachDB closes timestamps (Taft et al., SIGMOD 2020, §3.3 and §4.3): a range's leaseholder promises no write will
commit at or below a closed timestamp, carried in its Raft commands and, for idle ranges, a side transport; a follower
that has applied up to that command serves MVCC reads at or below it. Its closed timestamp lags now by a target
duration so that transactions whose timestamps were taken earlier aren't pushed. Spanner's safe time is the same idea
for Paxos groups (Corbett et al., OSDI 2012, §4.1.3). Two of its parts don't carry over, for reasons in Amphora's own
design:
- **No leases.** hyper-raft trusts no clock (`read.rs`), so a promise made outside the log (a heartbeat, a side
  transport) can't bind the next leader: a deposed leader in a minority could keep closing times a new leader writes
  under. Here a time is known only from committed entries.
- **No lag needed at the leader.** CockroachDB's target lag exists because a transaction's timestamp is taken before its
  commit. Amphora's leader prepares one transaction at a time and proposes it at once ("Resolved transactions", the
  base fence), so the order of the log is the order of time: an entry can close its own time.
- **No MVCC history in a replica.** A replica holds one state, the one after its last applied entry, so a read is
  answered at that state's time, never at an arbitrary earlier one: CockroachDB's bounded-staleness reads
  (`with_min_timestamp`), which read at the follower's closed timestamp if it is new enough, not its exact-time reads.

### The decision

- **Stamps.** Every entry an owner proposes (a resolved transaction, a class entry, a seed's chunk) is stamped with its
  proposer's wall clock: the prefix `amphora.time.v1\0`, a `u64` of microseconds since the Unix epoch, then the entry
  (`replication/closed.rs`). An owner's wall clock is the system's when it started, moved on by its own clock (`wall`
  plus the owner's time since it started), so its stamps never go back within one opening. The room for a stamp comes
  out of the entry's room (`Actor::entry_room`).
- **Times.** Applying entry *i*, every member computes the group's time *t(i)* = max(stamp(i), *t(i-1)* + 1 µs). An
  unstamped entry that changes state (from a binary before stamps) takes *t(i-1)* + 1 once there is a time; one that
  changes nothing (an election's empty entry, a change of members) keeps it. The time is written in the entry's own
  transaction (`__amphora_closed`, made by the first stamped entry, so at the same index on every member and the page
  layout stays the same everywhere), and travels with images. It depends on the log alone, so every member computes the
  same times, and they rise strictly along the committed log whoever leads: a leader whose clock is behind its
  predecessor's has its stamps lifted (counted by the machine), never a time that goes back. Raft's leader completeness
  (Ongaro's dissertation §3.6.3) puts every committed entry, with its time, in every later leader's log.
- **Closed time.** A replica's closed time is the time of its last applied entry. The state it holds is exactly the
  group's at that time: every entry at or before it is applied (they come earlier in the log), and none after (times
  rise). A follower read (`Database::query_closed(at_least, until, f)`) runs on a local reader, reads the time as its
  transaction's first statement (which takes the snapshot, so the time is the state's own), and answers with the
  value and that time. If the time is earlier than `at_least`, it waits on its caller's thread for the owner's next
  published time (the owner publishes after each step and tells waiting reads), up to its deadline, refused Timeout past
  it and Fenced once the owner ends. Each wait is for a time later than the one last found, so the retries are bounded by
  the deadline.
- **The lag, measured.** Each time a replica's closed time moves, its owner samples how far it stands behind its own
  wall clock: replication delay plus the proposer's clock offset. `Database::follower_read_micros` is now less the
  nearest-rank p99 of the last 100 samples: a read asked at it finds its time without waiting at least 99 times in 100
  while the delays hold. The quantile is AC-T1.10.3's; 100 is the fewest samples whose p99 is not their maximum,
  1 / (1 - 0.99). Nothing is measured once and frozen: every replica learns its own, continuously. Refused before the
  first sample.
- **Standalone** (`Writer::Local`), a follower read is `Database::query` at the wall clock when it began: its writes
  carry no time. **An attached client's** is refused, typed, until the wire carries it (a new `Request` variant and
  `wire::VERSION` 7).

### Evidence

- `closed::tests`: the stamp round-trips; times rise strictly whatever the stamps (behind, equal, unstamped); the lag is
  the p99 of the last hundred and forgets older ones.
- T-T1.10.C1, `no_read_under_leader_changes_sees_a_version_past_its_time` (driven shell): three voters whose wall clocks
  stand an hour apart, the lead passed 1, 2, 3, 1, each writing three values, every member reading before and after it
  catches up. Every read sees 1..=k; a read at a later time never sees less; two at one time see the same; lifted stamps
  are counted, so the case where a new leader's clock is behind was exercised. With a time equal to its stamp alone,
  member 2's writes would read at times an hour before member 1's, with more rows: the check fails.
- T-T1.10.C2, `a_follower_read_waits_for_its_time_and_is_refused_past_its_deadline`.
- AC-T1.10.3's p99 is measured by `amphora-bench --cluster --shapes read` (the coordinator's run).

### What it costs, and the limits stated

- 24 bytes an entry, and one more row written in each applied transaction.
- **Idle groups.** A group with no writes has a closed time that stands still while the clock moves, so a read asked
  past it waits to its deadline. The fix keeps the log the only source of times: a replica whose read asks past its
  closed time proposes an empty stamped entry (forwarded to the leader like any follower's proposal), which closes the
  time at the proposer's clock in one commit. Not built in this change (left below).
- **Rollout.** A binary from before this change fences on the first stamped entry (its decode refuses the prefix), as on
  the class entry: every member of a group runs this binary before one of them leads (Ada's word, as for the class).

### Falsified by

- a follower read that sees a write whose time is past the time it answered with, or misses one at or before it;
- two members' times for one index that differ, or a time that goes back along the committed log;
- a follower read answered at a time earlier than its `at_least`, or past its deadline;
- `follower_read_micros` reads that wait more than 1 in 100 under steady replication delay (the lag is too short).

## Decision (lane follower-reads): Search over partitions (2026-11-21, G13 step 4)

CP-T1.10's "search fanned out over directory partitions" and AC-T1.10.4 (search p50 at most 30 ms over 1,000,000 events
on three nodes). Kiln's search is K0's: `SELECT … FROM events WHERE text LIKE '%word%' ORDER BY at DESC LIMIT n`. Its
rows are spread over partitions (groups), so one search is a search of each.

**Decision.** `amphora::search::fan_out` asks every partition at once, the first on the caller's thread and each other on
a scoped thread of its own (bounded by the partitions the caller holds), so a search takes about its slowest
partition's time rather than their sum; `amphora::search::newest` merges their answers, each newest first and cut to the
limit, with a k-way merge holding one row a partition and taking at most the limit (equal keys in partition order, so a
merge is the same every time). On a replicated cluster each partition is asked on this node's replica, at its closed
time (`Database::query_closed`), so a search reads no leader. Each partition answers at its own time: the merged answer
is each partition's state at a time it reports, not one snapshot across them (that is cross-group work, CP-T1.10's
transactions).

**Evidence.** `search::tests`: the merge equals the sorted union cut to the limit over 500 random shapes; every partition
is asked while every other is (each ask waits for all to begin, which asked in turn never happens). AC-T1.10.4 is measured
by `amphora-bench --cluster --shapes search` (three members of every group in one process, relayed by the bench as a
host relays them; the coordinator's run).

**Falsified by:** a search slower than its slowest partition by more than a thread's start; a merged answer that differs
from the newest rows of all partitions together.
## Decision (lane constants): one drive a step (2026-11-21)

Ada's rule: no picked numbers. `DRIVE_QUANTUM` (32) bounded how many times one step of the owner drove its replica
while the drive said there was more to do (`Driven::more`). No measurement made 32.

**What it bounds.** One step of the owner takes one input, ticks if a tick is due, drives, then keeps what waits
(deadlines, the floor, the class, the slots). A drive takes at most one `Ready` and applies at most one page
(hyper-durable's own quantum: its `owner.rs`, docs/durable.md section 7). Drives and inputs compete for the owner's
thread: a long run of drives holds inputs back, a short one holds the replica's work back.

**The decision.** One drive a step, and a step whose drive left more comes again at once, without waiting for an
input (`Actor::driving`, `run_loop`). The input queue and the replica's drives then take turns one unit each: deficit
round robin with every quantum at least the largest unit (one `Ready`), which Shreedhar and Varghese prove fair to
within one unit ("Efficient Fair Queuing Using Deficit Round Robin", SIGCOMM 1995, Theorem 4.5). It is also how
hyper-durable's own owner turns its replicas. No count is left.

**Cost.** A step's keeping (expiry, settling, the status it publishes) now runs once a drive where it ran once per
up to 32. `more` is true only while a write slot is free and a `Ready` is due, or while applies wait past a drive's
page, so the runs are short (the core's depth is 1). The batch's replicated benches measure it.

**Test.** `an_owner_drives_once_a_step_and_steps_again_while_more_waits` (T-DQ.1.1): over an election and 20 writes,
some step leaves more to drive for the next. A step that drives until nothing is left leaves none.

**Falsified by:** an input waiting more than one drive while the replica has more to drive; the replicated benches'
commit latency or throughput worse than at 4e74cee beyond their noise.

## Decision (lane constants): the owner's wakes (2026-11-21)

Ada's rule: no picked numbers. `POLL` (1 ms) was how long the owner thread waited on its input channel before it
stepped anyway: a poll, so the log's finished writes, its flushes and its deadlines were seen at the next poll, and
an idle owner woke a thousand times a second for nothing. No measurement made 1 ms.

**The decision.** The owner waits on a bell (`Bell`: a flag and a condition variable) and steps when it rings or when
its next deadline comes, whichever is first (`run_loop`, `Actor::deadline`). What rings it:
- an input queued (`ReplicaHandle::enqueue`, after the send);
- a handle dropped (`Ring`, after the handle's sender is gone), so the last one's drop finds the channel closed;
- the replication log's write or compaction done: the bell's own `Waker`, which hyper-durable hands to the store
  (`Store::submit`'s waker, woken when the write lands: its `hyperlog.rs`, `memory.rs`);
- a flush of the database's log done (the flushing thread, after its answer).
A ring before the wait is kept, so none is lost between the owner's look at its channel and its wait.

Its next deadline is its next tick, or sooner a deadline it holds: a queued or pending submit's, a change of members',
a hand-off's, the write slot's end. Everything else it checks by time counts in ticks (elections, heartbeats, the
quorum check, a bootstrap's hand-off), so is due at a tick. A step that drove and left more steps again at once, as
before ("one drive a step"). Nothing is left to pick: the waits are the events' own times and the deadlines'.

**What still waits for a tick.** A local reader given back and the host draining the outgoing queue don't ring the
bell: the owner sees them at its next step, at the latest its next tick, as it did at its next poll. The tick itself
(`TICK`) is the next constant this lane replaces.

**Tests.** `the_owners_bell_keeps_a_ring_and_its_waker_rings_it` (T-WK.1.1),
`an_owners_next_deadline_is_its_tick_or_a_sooner_held_deadline` (T-WK.1.2),
`an_owner_thread_answers_on_its_wakes_and_ends_with_its_last_handle` (T-WK.1.3).

**Falsified by:** a write, flush or input that waits for the owner's next tick to be seen (the replicated benches'
commit latency worse than at 954ee1d beyond their noise); an idle owner's thread waking more often than its ticks.

## Decision (lane constants): generations learned where they run (2026-11-21)

Ada's word (2026-11-21): a value measured once on one host and frozen into the code is picked. Two such values were
left in the quorum patience (`patience.rs`), and the write slot and the database log's restarts borrowed one of them:
- `QUORUM_CEILING` (165 ticks), the largest quorum tail of step 1's record on adacachy: a gap past it was refused as
  a voter gone, not slow;
- `GENERATION` (3,000 ticks), how long a learned gap was believed (one to two generations), chosen longer than step
  1's loaded runs went between gaps past the election timeout; the write slot believed its learned gaps for the same
  generations, and the log's restarts counted over a window of two of them.

**The decision.** Each window's generation is learned from its own events where it runs (`patience::Recurrence`): it
lasts as long as the last two of its events were apart, the first counted from the tick the window began to watch,
so what it learned is believed for one to two of its own recurrences: through each while the events recur, and it
lapses once they stop. Whose events:
- the quorum patience: each voter's own gaps past the election timeout, the gaps its patience covers, so each voter
  ages on its own (`Answers`, a generation a voter);
- the write slot: each member's own learned retry gaps (`slot::Slots::expire`), on the owner's clock.

The log's restarts kept the fixed window (`log_bound::GENERATION`) here; the next decision replaces it. A window
learned from their own spacing was tried and fails its share test
(`restarts_made_when_due_keep_their_share_below_the_tail`): it feeds back on itself, since a window as short as the
restarts' spacing forgets a hold, so the next restart is due at once, and the spacing shortens again.
Gone, not slow, is the host's detector's verdict, as the write slot already ruled: a gap over which the detector
suspected its voter is refused (`patience::Unmeasured::Suspected`) and counted in
`amphora_replica_quorum_patience_refused_total`, its help line now saying so.

**What it costs.** A lone stall after a long calm is believed for as long as the calm was; its size, not its
belief's length, bounds what a cut-off leader waits (the patience is the gap less the election timeout). A host
whose detector never suspects a voter that was cut off, under pre-vote (no term moved), teaches the gap the cut
lasted; the detector's verdict is the owner's only word on that.

**Tests.** `a_recurrence_is_the_time_between_its_last_two_events`,
`a_gap_is_believed_for_one_to_two_of_its_voters_own_recurrences`, `a_suspected_voters_gap_is_refused_and_not_learned`
(T-GN.1.1); `a_learned_gap_is_believed_for_one_to_two_of_its_members_generations` (T-GN.1.2);
the driven patience tests,
`the_quorum_patience_and_its_refusals_are_counted_for_the_host` now refusing a suspected voter's gap.

**Falsified by:** step 1's measurement re-run with the learned windows in force, with more quorum-check step-downs
than with the fixed window; a leader that keeps a patience after its voters' stalls stopped for two of their own
recurrences.

## Decision (lane constants): a generation of restarts ends once it covers its spend (2026-11-21)

`log_bound::GENERATION` (3,000 ticks, the quorum patience's old window, measured once on adacachy) set how long the
database log's restarts were counted: a picked number by Ada's word. A window learned from the restarts' own spacing
fed back on itself (the decision before). **The decision** (GN.2): a generation runs from one restart to the restart
made once it alone covers its own spend at the quantile: its held, its base tail and the next restart's guess,
strictly below 1 - q of the entries it applied. That restart begins the next generation, and the one it ended is
forgotten: it paid for itself, so forgetting it never makes a restart due, and a calm stretch banks no restarts for
later. A restart made before (past the log's room, disk before latency) counts into the same generation, so its
spend is paid before the next is due. The guess is the worst of the last 1 / (1 - q) holds (a thousand at the p99.9,
a bounded monotonic queue): it falls back once a large hold is that many restarts old. No time enters: the owner's
tick no longer touches the count.

**What it costs.** Restarts are spaced by each generation's own spend, not by a window's slack: on the share test's
load 64 restarts against the fixed window's 182, each still within the tail.

**Tests.** `a_generation_ends_once_it_covers_its_spend_and_is_forgotten`,
`the_guess_is_the_worst_of_the_last_thousand_holds` (T-GN.2.1); the share test,
`restarts_made_when_due_keep_their_share_below_the_tail`, over the generation a restart ended and the one it began;
the owner's `the_first_restart_is_free_and_the_next_waits_on_its_share`.

**Falsified by:** the restarts bench (AC-T1.4.3, restarts on against off) with p99.9 moved more than with the fixed
window; a restart's hold larger than every one of the last thousand pushing the share past the tail in a run.

## Decision (lane constants): the owner's tick pace (2026-11-21)

Ada's rule: no picked numbers. `TICK` (10 ms) was the replicated owner's period between ticks, so its core's twenty
ticks made a 200 ms election timeout and its two a 20 ms heartbeat on every host and every network, whatever its
paths measured. No measurement made 10 ms.

**The decision (TP.1).** The owner learns its period where it runs (`database/pace.rs`) and derives it by the
vendored hyper-raft's own law (`docs/timing.md` §2.1–§2.3 there):
- each voter's round trips, from the replication messages' own sends and answers: heartbeats, pre-votes and votes,
  none of which touches the voter's disk; a request is timed only once every earlier one to that peer is answered
  (Karn and Partridge 1987), and the smoothed estimator is RFC 9002 §5.3's (`hyper_timing::ExchangeRtt`);
- the flush a vote waits for: the mean of the replica's vote writes (hyper-durable's `Replica::flushes`), or before
  one the mean of its log writes (`Output::flushed`), as hyper-durable's own `Replica::measure` takes them;
- the leader's link: the lateness of each heartbeat at a follower, or each answer to one at the leader, past the
  interval its heartbeats leave at, folded for the term (a new term is a new link);
- the timer's granularity: the lateness of the owner's own timed waits (`hyper_timing::Lateness`).
From them hyper-timing's ballot and split-vote span (`Ballot::measure`, `Ballot::span`), the margin that minimizes
the group's unavailability on the link's arrivals at the heartbeat interval (`arrival_detector_at`, the MTBF the
Jeffreys one of the owner's own exposure, `Exposure`), the election timing (`ElectionTiming::derive`) and the period
at which the core's fixed tick counts cover it (`TickPace::derive`), no finer than the timer's granularity and with no
ceiling picked. It is derived once an election timeout, the ticks the core draws each timeout over. The write slot's
bounds, the owner's session lease, the quorum patience's gauge and the bootstrap hand-off's retry read the pace in
force.

**The start state.** Before a quorum's paths, the timer and the link are measured, the pace is hyper-timing's
`TickPace::floor`, "the pace with nothing measured: the configured period" (hyper-timing's lib.rs): the 10 ms the
owner opens with, so a group can elect its first leader, whose votes are the first round trips measured. Once
measured, it bounds nothing: the period goes below it on a near network and above it on a far one.

**What is left.** The core's tick counts (twenty for an election, two for a heartbeat) are hyper-raft's `Config::new`,
raft-rs 0.7's defaults: a core in tick mode fixes them when it opens (`TickPace`'s premise), so the measured period
carries the time. Deriving the counts themselves means moving the owner to the core's elections by suspicion
(`Elections::Suspicion`), which take no ticks. The owner's exposure counts only since it opened: its restarts and the
group's failures are not stored.

**Tests.** `election_tick_an_answer_times_its_own_request`,
`election_tick_a_request_sent_behind_unanswered_ones_waits_for_their_answers`,
`election_tick_a_peer_the_core_takes_for_lost_starts_its_count_again` (T-TP.1.1);
`tick_the_pace_is_the_start_until_a_quorum_the_timer_and_the_link_are_measured`,
`tick_the_pace_follows_the_measured_election`,
`tick_a_new_term_measures_its_link_afresh_and_a_member_that_left_is_forgotten` (T-TP.1.2); the driven election,
patience and lease tests, which read each owner's pace instead of assuming 10 ms (T-TP.1.3).

**Falsified by:** a multi-node group on real clocks whose leader steps down or whose followers campaign more often
with the learned pace than with 10 ms; a failover p99 past the requirement's 1000 ms where 10 ms met it.

## Decision (lane constants): the compaction's expansion learned where it runs (2026-11-21)

Ada's rule: no picked numbers. The replicated owner compacted its raft log once it held 128 MiB of applied entries,
on every host and for every database, whatever an image or a replay cost there. No measurement made 128 MiB, and the
machine reported no image size (`image_bytes` was none), so hyper-durable's own rule could not run.

**The decision (CT.1).** The log is due by Ongaro's rule (thesis §5.1.2, "When to snapshot": once the log exceeds
the previous snapshot times an expansion factor), as hyper-durable's `Replica::compaction_due` weighs it, with its
wait for a member still taking what was applied. The factor is the owner's (`database/compacting.rs`):
- the machine reports its image's bytes: the pages it opened with (`StateMachine::image_bytes`), and after the first
  compaction hyper-durable weighs the image it made;
- the owner times each image it makes (`Replica::compact`, the image's bytes from the machine) and each drive that
  applied entries (the applied entries' bytes as the core counts them, `EntryRef::encoded_bytes`), on its own clock;
- each cost is a ratio of decaying sums, nanoseconds over bytes, the sums kept at seven eighths at every sample: RFC
  6298 §2's gain (Jacobson 1988), weighted by each sample's bytes, so it follows the host and its load;
- the factor is the image's cost a byte over the replay's, raised to the whole factor the rule takes. The log is the
  rent and the image the price: compacting once replaying the log would cost as much as making a fresh image is ski
  rental's break-even (Karlin, Manasse, McGeoch and Owicki, SODA 1990), within twice the best in hindsight. Raised,
  never rounded down, so no compaction comes before the break-even.
A compaction still runs at once while the log stalls admission for room: it is what frees it.

**The start state.** Before both costs are measured, and for a sample below the clock's resolution, the factor is
one: with nothing measured a byte of log costs a byte of image, whose break-even is the log at its image's size.

**What is left.** The image's bytes at open are its pages' (page count times page size); postcard's few bytes of
framing a page are not counted. A configuration change's entry is not counted in the replay's bytes (it is applied
by `apply_change`, rarely).

**Tests.** `compaction_the_factor_is_one_until_both_costs_are_measured` (T-CT.1.1),
`compaction_the_factor_is_the_image_cost_over_the_replay_cost` (T-CT.1.2), `compaction_samples_weigh_by_their_bytes`
(T-CT.1.3), all in `database::compacting`; the driven compaction tests of `database::replicated` (T-CT.1.4).

## Decision (lane constants): an envelope is what its encoding adds (2026-11-21)

Ada's rule: no picked numbers. A replicated command and an image were each bounded by their pages' budget plus
`ENVELOPE_BYTES`, a mebibyte for what their encoding adds, which nothing measured or derived.

**The decision (EN.1).** The envelope is what postcard's encoding adds to the pages, at the most, from its wire format
(`replication.rs`): a declared baseline's prefix and its declaration (a varint node and a 32-byte identity), a
prepared transaction's base index and term (varints, ten bytes at the most), the page list's length (a varint) and
each page's id (a `u32` varint, five bytes at the most) and length (the page size's varint), for as many pages as the
budget holds at the one page size `validate_pages` admits. So a command's envelope is a few bytes a page (57 KiB over
its 32 MiB), an image's the same over its budget, a test's lowered budget included. A read set still rides in what
a command's entry leaves of its bound, or the write goes without one, as before: a write near its whole budget now
goes without one sooner.

**What is left.** `SNAPSHOT_BYTES`, the image's own budget (256 MiB), is the seam owner's ruling (c): a memory bound
from the process memory budget's room for one image (done: "Decision (lane constants): an image is bounded by the
memory budget's room for one", below; SB.1). The transport's own framing past
an entry (`replicated.rs`'s `MESSAGE_BYTES` and `WIRE_BYTES`, two mebibytes each) is the owner's queue piece's.

**Tests.** `envelope_a_command_at_its_widest_fits_its_bound_exactly` (T-EN.1.1),
`envelope_an_image_at_its_widest_fits_its_bound_exactly` (T-EN.1.2), `envelope_varints_take_seven_bits_a_byte`
(T-EN.1.3), in `replication::envelope_tests`.

## Decision (lane constants): an image is bounded by the memory budget's room for one (2026-11-21)

`SNAPSHOT_BYTES`, a picked 256 MiB, bounded every image a replica made, fitted, installed or took as a baseline, and
the reshard directory's images. The seam owner's ruling (c): an image is held whole in memory until snapshots
stream, so the memory it may take bounds it.

**The decision (SB.1).** `replication::image_room` is the process memory budget's room for an image (store.md,
"Decision (lane constants): one process memory budget"; MB.1), read again at every image made, fitted, installed,
captured or received as a baseline, and kept by the directory. Where the budget can't be read the image is refused
typed (`Unavailable`), or the directory's machine fails, never a fallback. A replica's database log leaves that room
free on its filesystem (`log_bound::log_room` takes it as an argument). A machine's carrier may bound it lower
(`Machine::set_snapshot_bytes`): the replicated owner's command queue holds back room for one wire record's image
(`WIRE_IMAGE_BYTES`), so that owner's image is the smaller of the two.

**What was left.** `WIRE_IMAGE_BYTES` (256 MiB), the owner's queue's split, went with `QUEUED_BYTES` ("Decision
(lane constants): the queue bounds by drain rate"; QB.1).

**Tests.** `image_room_tests::sb1_an_image_is_bounded_by_the_memory_budgets_room_for_one` (T-SB.1.1);
`log_bound::tests::a_log_takes_its_file_and_an_equal_share_of_what_is_free_past_one_image`, the image's room passed in
(T-SB.1.2).

## Decision (lane constants): the queue bounds by drain rate (2026-11-22)

The replicated owner's queues were bounded by picked numbers: the command queue at 512 MiB (`QUEUED_BYTES`, with a
second copy for the local writer's queue in `database.rs`), the outgoing queue at 512 MiB and 4096 messages
(`OUTGOING_BYTES`, `OUTGOING_MESSAGES`), and the image one wire record carries at 256 MiB (`WIRE_IMAGE_BYTES`).

**The decision (QB.1).** A queue holds what its reader takes in an election timeout: what waits longer is older than
the time in which the group elects another leader, so holding more serves no one. Each queue's drain rate is measured
in bytes and messages a second, the outgoing queue's in `ReplicaHandle::drain` (its reader the host) and the command
queue's at the owner's intake, each a decaying ratio of sums with RFC 6298's gain of 1/8, as the compaction's costs are
(CT.1; `database/drain.rs`). An interval counts when the queue held work at its start or its reader took some in it.
The election timeout is learned too: `election_tick` periods of the owner's measured pace (TP.1). The bounds are read
again at each tick of the pace (`Actor::rebound`), where the queues' bytes are charged to the budget (`Budget::take`).
- The outgoing queue holds the lesser of its host's rate times that timeout and the memory budget's room for queues
  (MB.1), at least a record to each peer (one broadcast: a leader's heartbeats or appends), and the room before any
  drain is measured. A bound below one broadcast would shed a record of every broadcast.
- The command queue's bytes are the budget's room for queues. Application calls hold that room less one wire record's,
  so that calls waiting for a leader cannot block the wire record that finds one. A wire record's image is the
  budget's room for one (SB.1), held inside the command queue's bound.
- Application calls are also counted (`Control::call_bound`): no more than the inputs the owner takes in an election
  timeout at its learned message rate (`Drain::bound(..).messages`), at least one, and never more than the queue's
  slots less one, so that a wire record always finds a slot. Before the intake is measured, the slots less one.
- A call is charged its worst case (`REQUEST_MAX`, 32 MiB), while the intake's byte rate is learned mostly from small
  wire records. A byte bound learned from that rate fell to one call and refused a change of members queued beside it
  (`every_falsifier_of_the_floor_holds_under_a_nemesis`). So calls are bounded in count by the rate and in bytes by the
  room, never in bytes by the rate. A call refused for its count is `Full` at the count bound, one refused for its
  bytes `Full` at the calls' byte bound.

The local writer's queue has no election: each request it admits is charged to the budget's room for queues, refused
`Full` at that room.

**A bound that starts small grows before it fences.** A full outgoing queue fenced the owner. Now a queue past the
budget's room fences at once, since memory can't be lent; one past what its host drains in an election timeout fences
only once it has stayed past it for a whole election timeout, so that a burst at start, before any drain is measured,
waits for the host's drain to be measured and the bound to grow.

**What is left.** `MEMBERS` is the group's, stated in its log (lane constants-21, below); `COMMANDS` is derived
(lane constants-20). The
framing a wire record adds (`FRAMING_BYTES`) is derived (below). The image cap set on
the owner's machine is the wire record's room at open; the room read later bounds images through `image_room`.

**Amendment (QB.2, lane constants-17, 2026-11-22, not yet landed): past the outgoing bound, shed; don't fence.**
What the owner does once its outgoing queue is past its learned bound is one switch (`drain::Egress`) with three
candidates: `Fence` (the rule above, the control), `Shed` (drop the oldest records until the new one fits, count each,
and report its member unreachable, so raft sends again what the member lacks; etcd's rafthttp drops a message whose
peer's buffer is full and reports the peer unreachable) and `Pause` (no input and no raft step while the queue is at
its bound; the host's drain rings the owner). The default is `Shed`. In two driven tests whose relay is held while the
leader's clock moves, both fence under `Fence`, and under `Pause` the leader never steps down, because a paused leader
doesn't tick (`a_change_at_its_barrier_is_refused_once_the_lead_is_lost`). Both pass under `Shed`. A shed record is a
lost packet, which raft already tolerates; a fence ends the member, and a pause keeps a stalled leader in office.
A shed is made good by events (lane constants-19): a report of unreachable gives up the member's window, but a probe
already out waits for an answer, so raft would send again only on its next heartbeat tick, and never while a driven
test keeps time still (the likely cause of `a_change_past_its_deadline_answers_what_its_stage_knows` never
settling; not yet confirmed by a run). Now a shed marks a
resend owed (`Outgoing::owed`); the host's next drain rings the owner, and once the queue has room for a record to
each peer the leader beats (`Actor::resend`). Each member's heartbeat answer resumes its probe and brings the entries
it lacks. A follower's shed record is a lost answer, which its leader's heartbeat timeout makes good, as on a lossy
link. The
A/B scenario (`qb2_many_slow_one_stalled_for_good_by_egress`: every host slows at once, then the leader's host stalls
for good) is driven by events (lane constants-19: a round waits on a writing owner's bell, not a poll, and counts an owner
with more to drive as busy), and prints each candidate's numbers; its run of record is the coordinator's. Application calls are held to their own bound apart from the
wire records queued beside them (`Control::called`).

**Derivations for the numbers still picked.**
- `FRAMING_BYTES` is derived (lane constants-19), a format value: what a wire record adds past the command its one
  entry carries. That is the record's fixed bytes with its classic commit and checksum (`MESSAGE_RECORD_FIXED_BYTES`)
  and an incarnation and end (`MESSAGE_TOLD_BYTES`), counted for any record; the entry's fixed bytes
  (`ENTRY_FIXED_BYTES`, hyper-raft `wire.rs`); its context (the request identity `Actor::context` writes: four u64s);
  and what a prepared transaction's encoding adds to its pages (`replication::COMMAND_ENVELOPE`, EN.1). It was a
  picked 2 MiB. An image's record is framed apart (`image_framing(members)`): the record's bytes, the snapshot's
  presence, data length, index and term, and a `ConfState` of four sets of at most the group's member bound of eight-byte ids, beside
  the pages' encoding of its budget (`replication::image_envelope`). An image's cap is the most whose record fits a
  wire record's room (`image_within`).
- `unstable_entries` and `entries_per_message` are no longer overridden (lane constants-19). `hyper_raft::Limits::derive`
  derives them, the first from the memory stated over `size_of::<Entry>()` and the second from a message's payload
  over `ENTRY_FIXED_BYTES`. The memory stated is the budget's room for queues (MB.1), read at open, in place of the
  picked `64 << 20`. `pending_reads` is no longer overridden either (lane constants-20): `Limits::derive` takes it
  from the memory stated over `size_of::<PendingRead>()`, and every read asked comes through the command queue, whose
  inputs are bounded apart. `proposals` stays one, since one command at a time crosses ReadIndex.
- A leader's uncommitted bytes and a ready's committed bytes (`max_uncommitted_size`, `max_committed_size_per_ready`)
  are the budget's room for queues, at least one message (lane constants-20; were a picked 64 MiB each): both are held
  in memory beside the queues.
- A seed chunk's framing (`bootstrap::FRAMING`) is derived from its postcard encoding (lane constants-20; was a picked
  1024): the prefix, the declaration's node varint and 32-byte identity, and the offset, total and image length
  varints.
- `COMMANDS` is derived (lane constants-20). The command queue is an unbounded channel whose inputs `Control` counts
  (`Control::commands`), each held at enqueue to `Bounds::commands`: what the owner takes in the learned election
  timeout at its learned message rate (`Drain::bound(..).messages`), at least a call and a wire record, and no more
  than the budget's room for queues holds of an input's least in memory, its own envelope (`size_of::<Envelope>()`):
  a wire record may carry no bytes, so bytes alone would not bound the count. Before anything is measured, the room's
  envelopes. Calls are counted one fewer, so a wire record always finds a place. An input's place is given back when
  the owner takes it from the channel (`Reservation::taken`), as a bounded channel's slot was; waiting calls are held
  to the same bound. The pump's receipts in the tests grow to the bound (`Pump::receipts`). T-QB.1.6 tests the counts.
- Each queued input is charged its envelope (`ENVELOPE_BYTES`, `size_of::<Envelope>()`) against the queue's bytes,
  beside what it carries (lane constants-21): memory is counted once, in the bytes, and bounded once, by them, where
  before a count of envelopes and a byte bound that left them out together allowed up to twice the room. A change of
  members, a hand-off and a catch-up carry nothing past their envelope (were charged `size_of::<Input>()`), an
  attestation its name. The room held back for a wire record holds its envelope too. T-QB.1.10 tests the charge.
- `MEMBERS` (was 64) is gone (lane constants-21): see "The member bound stated once per group", next.

**Still picked in the replication code (lane constants-20's survey), each with what it should come from.**
- `max_inflight_msgs` (4) and `max_inflight_bytes` (64 MiB), the window to each member: now the control of an A/B
  (lane constants-21, "The window to each member", below).
- hyper-log's settings at open (`segment_bytes` 64 MiB, `max_segments` 16, `group_entries` 65 536, `group_bytes`
  512 MiB, `group_cache` 64 MiB, `queue_submissions` 16): the log's disk room (`log_bound::log_room`) and an image's
  share, the memory budget's room for caches, and the device's measured queue depth.
- `quiet` is now the learned election timeout (lane constants-21; was focal F17's picked 50 ms): the core's election
  ticks of the start pace at open, and of the learned pace at each repace (`quiet_period`, `Replica::set_quiet`), so a
  member a group takes for lost after an election timeout has stated what it applied by then. Tests that let an owner
  state its durable commit before it stops advance its clock a tick past it (`past_quiet`). T-QB.1.12 tests it.
- `TICK` (10 ms): to remove (below).
- `CHECKPOINT_EVERY` (25 ms, measured once on fedora): learned from this host's checkpoint costs, as `compacting`
  learns compaction's.
- The log device's alignment (4096): the device's logical block size, read at open.
- `serve::OUTGOING_MESSAGES` (4096, CP-T1.13's peer senders): the outgoing queue's learned bound in messages.
- `compaction::ENVELOPE` (81): stated as a format value; it could be worked out from the encoding as
  `COMMAND_ENVELOPE` is.
Format and requirement values, not picked: `PAGE_BYTES`, `FRAME_BYTES`, `U64_MOST`, `U32_MOST`, `LAG_SAMPLES` (the
p99 quantile's), `COMMIT_TAIL` (AC-T1.3.4's bound).

**To remove: event-driven (Ada, 2026-11-22).** `TICK`, `POLL` and `DRIVE_QUANTUM`, and any driver that wakes on a timer,
are to be removed, with that work driven from events, not given better intervals. The election and heartbeat
timeouts stay learned, since they are truly about time. Lane repl-measure-8 owns the commit path's timer waits
(`run_loop`, `deadline`, `Bell`).

**Tests.** `drain::tests::drain_nothing_measured_bounds_one_record` (T-QB.1.1),
`drain_bound_is_rate_times_election_timeout_within_room` (T-QB.1.2), `drain_bound_follows_the_reader_and_ignores_no_time`
(T-QB.1.3), `replicated::tests::queue_bound_a_burst_at_start_grows_before_it_fences` (T-QB.1.4),
`replicated::tests::t_qb_1_6_command_queue_inputs_counted_by_rate_within_the_room` (T-QB.1.6) and
`patience::tests::t_qb_1_7_a_leader_measures_the_members_its_core_was_opened_for` (T-QB.1.7).

## The member bound stated once per group (lane constants-21, 2026-11-22, not yet landed)

hyper-raft stops a member handed a configuration of more members than it was opened for (`Progress::apply`, "a
configuration of more members than the member was opened for"), so the bound is the group's, not a member's own. Lane
constants-20 weighed three ways to derive it and found each fails: the memory room differs between hosts, the members a
member is configured with differ between a founder and a joiner (a one-voter group could never add member 2), and a
format bound (about 1.6 million) would take megabytes of every image record. The rule now (`replication::members`):
- **Stated in the log, as the class is.** A bound entry is its own kind of log entry: the prefix
  `amphora.members.v1\0` and the bound as a little-endian u64 (27 bytes). The machine records the highest bound
  applied (`__amphora_member_bound`, part of the image), and a lower or equal one changes nothing, the same at every
  member because it depends on the log alone.
- **Opened with the log's bound.** A member opens its core with the bound its log recorded, and before any, with
  twice the members of the configuration it opens with, two at least (`members::first`): every founder opens with the
  one first configuration, so all hold the same bound, with room to grow. A member opened with more than its log
  recorded (a joiner told today's members) keeps the larger: a bound only rises.
- **Raised ahead of the change that needs it.** A change whose configuration would pass the leader's bound first
  proposes a bound entry of twice the bound, or the new member count if that is more (`members::grown`; doubling's
  amortized constant cost per member added, Cormen et al., "Introduction to Algorithms", 3rd ed., section 17.4), then
  the change behind it (`Actor::raise_members`). While a write is under way, the change waits (`Stage::Raising`) until
  it ends, as the class entry does, because the bound entry moves the point the write was prepared at.
- **Taken as the entry is taken.** The core raises its own bound as it takes an entry that states one, before the
  configuration changes behind it take effect: hyper-raft puts a configuration in force when its entry is appended,
  not applied, so raising on apply would come too late at a follower that takes both entries in one append. This is a
  vendored change to hyper-raft (`vendor/patches/hyper-raft/0006-…`): `Config::stated_members` is the owner's reader
  (`members::stated`), read as entries are taken at open as after, so a member reopened with a bound entry and the
  change behind it taken and neither applied opens in bound; and `Raft::grow_members` raises the tracker's, the fast votes' and the pending reads' bounds
  with it. The fast votes' bytes stay what the core opened with: a vote past them is dropped, and the leader commits
  by the classic track. An image installed brings the bound recorded before it, which the core then takes
  (`Actor::keep_member_bound`), and the voters the leader's quorum patience measures follow the core's bound
  (`patience::Answers::grow`), so a voter added past the old bound is measured.
- **What is still open.** A joiner that installs an image whose configuration names more members than its own opening
  bound fails in hyper-raft before the machine sees the image's bound. A joiner opened with the group's members today
  is in bound. Carrying the bound in the snapshot itself would close this; it needs a hyper-raft wire change.
  A member whose core took an uncommitted bound entry that a new leader replaced keeps the higher bound; that only
  costs room. `IMAGE_FRAMING_BYTES` is now `image_framing(members)`, sized by the bound, read on the flushing thread
  as it rises (`Machine::members_shared`).
- **Tests.** `members::tests::t_qb_1_8_a_bound_entry_states_the_bound_and_growth_always_fits` (the entry, the first bound,
  and 2, 4, 8, 16 as a group grows to 9) and `replicated::tests::t_qb_1_9_a_one_voter_group_grows_to_five_then_nine_members`
  (driven: a voter grows its group to 5 members, then 9, beside a running learner that took the raising entries with
  the changes behind them, then reopens with the 16 its log recorded).

## The window to each member (lane constants-21, 2026-11-22, not yet landed)

What a leader keeps in flight to each member, ahead of its answers, was a picked 4 messages and 64 MiB. It is now one
switch (`drain::Window`) with two candidates, the picked one kept as the control:
- `Picked`: 4 messages and 64 MiB to every member, as before.
- `Carried` (the default): hyper-raft's own rule (mantle note 32 R16): no count of its own (`usize::MAX`), and twice
  what the path carries in a round trip (`hyper_timing::inflight_window`, `Replica::set_carriage`). What the path
  carries is the bytes the host drained to that member (`Outgoing::drained_to`, a `Drain` per member) over its round
  trip as `pace` measures it from heartbeats' and votes' answers (`Pace::round_trip`), at least a record's own bytes
  (`RECORD_BYTES`, a format value). A window that holds nothing still sends one entry, however large (hyper-raft's
  `Inflights::full`), so a path drained of nothing still carries an append; one whole message (32 MiB) as the floor
  would be about the picked 64 MiB again. Before a path is measured, the window is the memory room for queues (at
  least one message), the only bound known before a path has drained, as the queues' bounds start (QB.1): a test whose
  time stands still, and so never measures a path, sends as before. A path carries only what its window let out, so the window doubles each rebound while the path
  keeps up, and holds where it doesn't, as TCP's slow start grows its window by what is acknowledged (RFC 5681 §3.1).
  `Actor::carry` sets it at each rebound, from events the owner already counts, no timer of its own.

Each candidate's failure mode, forced by one driven scenario (`window_slow_flooded_fast_starved`): five voters, members
2 and 3 cut off while the leader commits with 4 and 5, then both reconnected, member 3 slow (its answers held for most
rounds) and member 2 fast. `Picked` floods the slow member: up to 64 MiB in flight to a path that drains a little a
round. `Carried` may starve the fast member: once measured, its window is what it carried, and doubles only as the path proves it
carries more. The scenario prints the most bytes and messages in flight to the slow member and the rounds each member
took to catch up; its run of record, and the choice of the default, are the coordinator's
(`WINDOW_WRITES=512 WINDOW_SLOW=16 cargo test -p amphora --locked --lib -- --nocapture window_slow_flooded_fast_starved`).
Open: a member's round trip is measured only from heartbeats and votes, so a learner's comes from the leader's
heartbeats to it; the per-member drain is weighed at the owner's rebound, which still follows `TICK` (to remove,
above).

## A follower's write that read nothing written since its base: read sets (built 2026-11-21, Amphora goal ceea4e0a)

D of goal 9c63f7d4, on top of the write slot (above). In the release A/B, D halved G's write gaps
(bench/records/2026-11-09-write-starve-*; docs/research/follower-write-starvation.md). The slot makes a refused write's
next try land; read sets keep most writes from being refused at all, when they touched pages nothing since wrote.

**The read set.** The pager records the pages a prepared session reads (`Pager::record_access`, from its `BEGIN
IMMEDIATE` to before its capture, which reads every dirty page again). The write states what it did to page 1: nothing,
only the database's size and the checksum trailer (an allocation), or more. The record's bound is the entry's own
bound in bytes, since each page number takes at least one byte in the entry: a session that read more could not carry
its read set, and goes without one.

**On the wire: append-only.** The read set rides after the write's `Prepared` encoding, as a postcard trailer
(`replication::readsets`). A binary from before it decodes the `Prepared` and ignores what follows (postcard's
`from_bytes` reads a value's prefix), so a mixed group decides every entry the same way. A write whose trailer would
pass the entry's bound goes without one: the exact base decides it.

**The leader decides, once.** As the leader steps a member's forwarded write (`step_forward`), the entry before the
append is its own last entry. If the write's base is older, the leader walks its log from the entry after the base to
that last entry. If none wrote a page the write read or wrote, it moves the write's base to its last entry, rewriting
the two numbers in place. Every member then applies the entry by the exact base, as before, so no member keeps state of
its own for the decision and none can decide it differently: a restart, an image or a binary from before changes
nothing. That is Kung and Robinson's backward validation (research f4, f5), over pages. An entry that no page list
describes (a change of members, a class entry, a seed, one the leader can't read) conflicts. Every apply writes the
replica's marker table and the closed time's table ("Follower reads at a closed time"), so a write that read or wrote
either page conflicts with every entry. The leader reads a write inside its proposer's closed-time stamp, and moving its
base keeps the stamp as it was. The group's first stamped entry makes the closed time's table at apply, a schema change
no page list shows, so a follower carries a read set only on a base whose state already has that table. Entries refused
at their own apply still count as written: conservative, never unsound.

**Page 1 merges its size.** Every session reads page 1. An entry that changed it only in the size conflicts only with
a write that writes page 1 too; one that changed more of it (the schema, its cookie, the free list) conflicts with
every write, whether or not the write's record caught its read of page 1. Two allocations from one base would allocate the same page, which conflicts on that
page anyway. An entry with no read set that wrote page 1 counts as more than a size change.

**The window is derived.** It is the leader's log between the write's base and the append, bounded by the log's own
bound: what it holds since its last compaction. A base the log no longer holds, or whose term differs from the log's, is left to the exact base,
which refuses it. There is no window constant and no cap on age. The walk reads the log in slices of one command's
bytes and ends at the first conflict. A write forwarded with others in one message is left to the exact base.

**Why it is serializable.** A write admitted this way read, at its base, exactly what it would have read at the
leader's last entry: no page it read changed in between, and page 1 changed at most in the size, which it didn't
write. Its pages overwrite none that changed. So it is equivalent to the same session run at the entry before it.

**Observability.** A debug event `replication.read_set` (`outcome` = rebased, conflict or unknown_base, with the base
and the leader's last index). The names go to Kiln Metrics before any metric is added.

**Requirements and tests.** AC-T1.8.R1 to R3 (docs/requirements.toml):
- T-T1.8.R1 to R3, the core's tests in `replication::readsets`;
- T-T1.8.R4, `r4_a_follower_write_on_pages_nothing_since_wrote_lands_on_its_first_try`, an interleaving the test
  builds with the slot off: the follower's write on another table lands on its first try, one on the leader's table is
  refused, and with read sets off both are refused;
- T-T1.8.R5, `r5_read_sets_keep_every_list_append_history_serializable`, Elle's list-append runs with the slot off and
  on. The mutant that moves every stale base unchecked must be caught there.
The W tests run with read sets on.

Falsified by: a non-serializable history with read sets on; a release A/B against G where read sets don't cut G's
write gaps; a `conflict` share near one on workloads whose writes touch different pages (the read set too wide: the
recorder catching reads the session didn't make).

## Decision (lane g12s2): the raft log as the WAL (2026-11-21, G12 step 2)

Requirements AC-T1.8.L1 and L2, tests T-T1.8.L1 and L2 (docs/requirements.toml); kiln plan "G12 Replication
(CP-T1.8)", step 2.

### What was left

Applies already flushed nothing on the owner's thread ("A replica's database log", decision 1), and a write was
already acknowledged only once hyper-durable held it on a quorum's raft logs (durable.md §4.1, I5). One flush was
still the owner's: `Machine::persist`, which hyper-durable's `Replica::compact` asks before it moves the raft log's
start, flushed the database's log inline. So each compaction stood the owner still for a flush, the stall decision 1
removed from applies, and the rule "the engine's log is durable before Raft compacts past it" rested on that inline
flush rather than on what had been made durable.

### The decision

`Machine::persist` does nothing. `Replica::compact` moves the start to `durable() - keep` at most (hyper-durable,
replica.rs, I8), and `Machine::durable` is the point the owner's flushing thread last made durable
(`flushed_through`). So a compaction waits for the engine's flush: it never passes it, and one asked before the flush
ends moves the start only as far as the last finished flush; the next compaction, at a later step, takes the rest.
The raft log always holds every entry past what the database's log made durable, so a power cut that loses the
database log's unflushed tail reopens at `durable()` and applies the rest again from the raft log (§4.3), as before.
The raft log is the write-ahead log; the database's log is a cache of it that the flushing thread makes durable in
its own time. Still flushed inline, all rare and as before: a membership change (its join rows), an install, the
seed, a persisted class entry, and `open`.

The owner compacts when the raft log is due by Ongaro's rule at the expansion it learned ("Decision (lane constants):
the compaction's expansion learned where it runs") or is stalled for room. A stall waits on the
flushing thread, which is always asked to flush what was applied (`Actor::flush`), so it frees room once that flush
ends, not on a flush of the owner's own.

### Evidence

- T-T1.8.L1 `compaction_waits_for_the_engines_flush` (database::replicated): a voter's flushes held, 1 flushed and
  2 to 5 applied. A compaction keeping nothing moves the raft log's start to the flushed point and no further, and
  flushes nothing (2 to 5 stay unflushed); once flushed, the next compaction reaches the last entry applied.
- T-T1.8.L2 `the_records_suite_answers_alike_at_one_voter_and_three` (tests/records_conformance.rs): one harness,
  `Group::new(voters)`, the group's size its only parameter, runs the Records suite and seeded histories against the
  simulator at one voter and at three, each at its leader; every case passes and the two transcripts are identical.
- The tests that compact a leader to make a joiner install its image now wait until its flushing thread has
  flushed what it applied (`Driven::flushed`), the fact that compaction now waits on.

### Falsified by

- a compaction that moves the raft log's start past `Machine::durable`;
- an acknowledged write lost after a power cut that lost the database log's unflushed tail (the power-cut nemesis);
- a stalled owner whose raft log never frees room while its flushing thread keeps flushing;
- a store test whose answers differ between one voter and three.

## Decision (lane ryw-1): read-your-writes by session tokens (2026-11-22, kiln goal 8119f25d)

Ada's order (2026-11-22): read-your-writes on any replica, built behind a switch with today's read as the control,
decided from live runs. Requirements AC-T1.10.R1 to R3, tests T-T1.10.R1 to R3 (docs/requirements.toml, lane ryw-1).

### What was already there

- **Commit at log durability.** A write is acknowledged once hyper-durable holds it on a quorum's raft logs, and the
  leader answers at its own apply, which flushes nothing on the owner's thread ("the raft log as the WAL", above). So a
  read on the leader after its write's answer already sees it, for free. A follower answers a write made through it at
  the commit it hears ("Fast acknowledgement"). Recovery replays the raft log from the database's durable point.
- **Closed times.** Every entry carries the group's time, which rises strictly along the committed log and is the same on
  every member ("Follower reads at a closed time"). A replica publishes the time it is complete up to, and a read at a
  time, or with bounded staleness, is served on the replica with no trip to the leader.
- **Apply on events.** The owner applies in its step, which wakes on the arrival that carried the commit (its bell),
  never on a tick. Lane repl-measure-8 is removing the timers that remain on the commit path (`run_loop`, `deadline`).

### The candidates, behind `ReadMode`

- **(a) `ReadMode::Latest`, the control and today's read.** `Database::query_session(Latest, ..)` is `Database::query`:
  every read crosses the leader's ReadIndex and waits for applied to reach the latest commit, token or not.
- **(b) `ReadMode::Session`, session tokens.** Built in this change:
  - *Tokens.* `Database::session_token` after a write's answer gives `Token { micros, index }`. `micros` is the group's
    time, `index` the commit index. The owner publishes both before it answers, at the leader's apply and at a follower's
    commit answer (`Actor::publish_point`). Within one group the two rise together. `micros` is in the same domain as the
    cross-group coordinator's timestamps (microseconds since the Unix epoch), so a single-group commit and a cross-group
    one compare in one order, time first (Ada's decision: a cross-group commit answers with its timestamp,
    `Token::at`; lane xgroup-sql-6 stamps it). A session keeps the latest of its tokens (`Token::after`).
  - *Reads.* `query_session(Session, Some(token), ..)` is served by the replica it asks once that replica's published
    applied index and closed time are at or past the token. The check is repeated inside the read's own transaction
    (`run_closed`), so the state read is never older than the token. The read waits on that replica alone, only while
    it is behind, woken by the owner's publication (a condition variable the owner signals as its point moves), never
    by a timer.
  - *Redirect.* A replica still behind after its own measured apply lag (the p99 of how far its closed time stood
    behind its clock over its last 100 moves, `closed::Lag`) redirects the read to the leader's read
    (`Database::query`). One that has measured no lag redirects at once. The bound is learned on each replica, and no
    number is picked.
  - *No token.* `query_session(Session, None, ..)` is `Database::query_local`: it never waits for the log.
- **(c) An overlay, not built.** A read on a replica that is behind would merge the committed log entries it has not
  applied over its applied state. Its spec:
  - the replica's log holds committed entries past `applied`, each a resolved transaction's page image or row changes
    (`replication::verdict` already decodes an entry's outcome without applying it);
  - a read with a token whose index is in that range opens a reader on the applied state and lays the entries'
    row changes up to the token over it: a per-table map from key to row or tombstone, consulted before the reader's
    b-tree;
  - it never waits for apply, only for the entry to be in the replica's log and committed;
  - failure mode to force: a long unapplied tail (a follower whose flushes are held) makes every read pay the merge,
    and the overlay's memory is bounded by the log's room. A scan over a table with many overlaid rows costs the merge
    of the two orders.
  It is worth building only if (b)'s measured waits at a lagging follower are a large share of its read latency.

### Instruments

`Database::session_counters` on each replica: reads served at once, reads that waited and their total wait, redirects,
and the apply lag that bounds the wait. `amphora-bench --cluster --shapes ryw` gives, at each count of sessions: read
latency straight after a write (p50, p99, p99.9), write p99, write-read pairs a second, reads that missed their own
write (must be 0), each follower's counters, and the process's CPU seconds and peak memory.

### Evidence

- T-T1.10.R1 `t_t1_10_r1_a_session_reads_its_write_on_a_lagging_follower`: records to one follower are lost while 2 is
  written. Its token read waits (counted, non-vacuity asserted: the follower was behind), and once the follower
  rejoins it answers 1 and 2, never 1 alone. A read with no token answers at once.
- T-T1.10.R2 `t_t1_10_r2_a_behind_replica_with_no_lag_learned_redirects`: a follower that has applied nothing stamped
  redirects at once, counted. Once caught up, every member serves the same token with the write.
- T-T1.10.R3 `t_t1_10_r3_a_session_reads_its_writes_across_leader_changes`: clocks an hour apart and the lead passed
  1, 2, 3, 1. Each new write's token orders after the last, every token read served sees all of the session's writes,
  and once caught up every member's answer equals the leader's state (the control's answer).

### Limits stated

- A token taken after an answer names the highest index this owner has answered, so with concurrent writers it can name
  a later write than the caller's. A read then waits for more than it needs, but it never misses its own write. The
  exact token per answer needs the answer to carry it (a change of `Answer`).
- A token's `index` belongs to its group. A session that writes to several groups keeps one token per database, plus
  `Token::at` for a cross-group commit. Mixing two groups' indexes in one token stays correct, through redirects, but
  waits.
- A follower's commit answer carries its closed time as of that answer, which is a lower bound. That replica's own
  reads check `index`, but a cross-group comparison of such a token by time alone is weaker until it applies.
- The power-cut case (the nemesis with held flushes) waits on lane repl-measure-8's fix of
  `a_member_power_cut_loses_no_acknowledged_replicated_write`. It is not built on here.

### Falsified by

- a token read that answers without the session's own write, on any replica, under lag, leader change or power cut;
- an acknowledged write lost;
- an answer under `Session` that differs from `Latest`'s for a session carrying its tokens;
- a read with no token that waits for the log;
- redirects above 1 in 100 under steady lag (the bound learned is too short).
