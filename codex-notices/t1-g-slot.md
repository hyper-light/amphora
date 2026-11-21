# t1/g-slot: G's design, before code (key g-slot-design-20261111)

From amphora-g-slot. Branch t1/g-slot from b151af0, nothing changed yet. Goal 9c63f7d4's fix. Ada chose G now and
D later (2026-11-12, via the Amphora agent). The previous owner agreed G on four points
(amphora-owner-t1-write-starve-044c44f-20261109). Merge scope is G alone: no Admission enum, no env switch, no engine
read recorder, no `Prepared.reads`. Everything sits in `Actor` (replicated.rs), plus one pure core
`database/slot.rs` (sans-IO, unit-tested). Nothing changes in the entry format, the wire or the engine.

## Why the prototype's runners still retried 4-8 times (release A/B, 5 of 5 promoted)
The prototype had three gaps. The design below closes each one.
- a. One slot, overwritten. `slot: Option<(member, until)>` held the last member refused. When runner B was refused,
  B took the slot from runner A. Nothing held B's forwards during A's slot, so B's commit refused A's retry, and the
  next refusal moved the slot again. The two runners refused each other in turn. In kiln's test both runners write
  steadily, so this is the likeliest cause of most of the 4-8.
- b. A fixed 200 ms. Under a loaded host, a retry (kiln re-runs its closure, then a ReadIndex round, the tail apply,
  the preparation and the forward) can outlast a fixed slot. The leader then writes again and the retry is stale.
- c. The hold started at the leader's apply of the refused entry. A follower answered on the commit (the replicated
  class) can send its retry's ReadIndex before the leader applies that entry. The leader's next own preparation can
  then land after the retry's barrier.
The record can't separate a from b. The new metric's `ended` label will: a retry lost to another member's commit
ends its slot `refused`, and one lost to time ends it `expired`.

## The design
State (pure core `Slots`, leader-local): `term`, `queue: VecDeque<member>` (FIFO, each member at most once),
`held: Option<(member, until, since)>`, `turn: bool` (the leader's own turn is owed), and `forwards: VecDeque<(member,
Message)>` (non-holders' forwarded write proposals held during a slot).

1. Elections and member changes. All of the state belongs to one term at one leader. When the term changes or this
   member stops leading, the core is emptied: the slot ends `released` and any held forwards are stepped into the core
   as they would have been had they arrived then (today's "forward that meets an election" path; nothing dropped). A
   member that leaves the configuration (`raft.configuration().contains` false) or that the host's detector suspects
   (`Verdict::Suspected`) leaves the queue, and its slot ends `released` at once. The slot's length is the core's
   election timeout, read at runtime as `election_tick × TICK` (the core's `Elections::Ticks` mode). It is never a
   picked number. Reasoning: the leader already accepts up to one election timeout of unavailability (kiln's assertion
   uses the same bound), and a member that can't retry within one is past the core's own liveness horizon. The
   `expired` count falsifies it if retries routinely outlast it.
2. More than one starving member. A member refused at the leader joins the queue's tail unless it's already queued.
   The queue's bound is the configuration's member count minus the leader, by construction (a set of members), so it
   can't overflow. The head holds the slot. During a slot the leader prepares nothing of its own, and it holds
   forwarded write proposals (`MsgPropose`) from other members instead of stepping them. Their bound is one per member
   (each owner forwards one write at a time). If a second one arrives from a member already holding one, the older is
   stepped first, so the queue never grows past the member count. A slot ends `landed` (the holder's write applied),
   `refused` (its write refused again: it rejoins the tail), `expired`, or `released`. At each end the held forwards
   are stepped in arrival order, and the leader gets its own turn: one preparation of its pending write, if it has
   one, before the next head's slot starts. So the leader waits at most one slot between its writes. A member waits at
   most (members − 1) slots, each with one leader turn between. That gap (a) is closed.
   Test: `two_followers_and_a_leader_writing_steadily_all_commit_within_their_deadlines`. The leader and two followers
   each make steady list-append writes under seeded delays. Expect: none misses its deadline, each follower write is
   refused at most (members − 1) times in a row, and the leader commits between every two slots (non-vacuity: the slot
   was held more than once for each follower).
3. The barrier after the slot is held. (i) The leader starts the slot at its commit of the refused entry, decided from
   the log by `replication::verdict`, as `answer_committed` decides a follower's answer. It doesn't wait for its own
   apply. Because the leader commits before any member can learn of the commit, the slot is held before any retry's
   ReadIndex arrives. That closes (c). (ii) Every write prepares at its member's log tail (the floor session's rule,
   extended from `Author::Owner` to every write). It prepares only once it has applied its own log through the last
   index it held as its barrier passed. That covers a leader write already appended before the slot. Test:
   `a_follower_retry_racing_the_slots_start_lands_on_its_next_try`. Hold the leader's apply (Driven's held flush), so
   the retry's ReadIndex reaches the leader between its commit of the refused entry and its apply of it, with the
   leader's next own write waiting. Expect the retry to land. A mutant that starts the slot at apply is caught.
   The test the Amphora agent asked for, "a refused write lands on its next try under the slot", is
   `a_refused_follower_write_lands_on_its_next_try_under_the_slot`. The leader writes steadily, one follower writes,
   with seeded delays, 3 seeds. Expect each follower write to be refused at most once, and its retry to commit. The
   control is today's rule with the slot off (the core's `hold` disabled by a test-only switch): the follower is
   refused past its deadline.
   The isolation check (the list-append run from t1/write-starve, ported) runs under G: replicas equal, a serial
   replay per list, no G0/G1c/G2 cycle.
4. Metrics, names going to Kiln Metrics (01a14bf7) for agreement at the same time as this notice:
   `amphora_replica_write_slot_holds_total{ended="landed|refused|expired|released"}` (counter: slots this leader held,
   by how each ended) and `amphora_replica_write_slot_held_seconds` (counter: the seconds its own writes were held, in
   total, so the leader's cost is the rate). The labels are a fixed set of four, with no member label. An event
   `amphora.replication` `event = "write_slot"` (debug) carries the member, the end and the time held.

## Out of scope
D's page validation, `Prepared.reads`, `Window`. Kiln step 10's own 200 ms gap assertion: the prototype's gaps of
443-969 ms across a promotion are expected to shrink under (a) and (b), but this goal doesn't own that assertion.

## What I ask
Agree, or rule otherwise, on: holding other members' forwarded proposals during a slot (point 2; this goes beyond "the
leader holds its own preparations", and it is what makes a refused write's next try land when two followers write);
starting the slot at the leader's commit rather than its apply (point 3.i); and the slot length as the core's election
timeout read at runtime (point 1).

# Code notice (key g-slot-code-20261111): t1/g-slot at 878285a, tree 885d4b02ea7c0238d7d9f10b487f5bd58e51e8b0

Commits: 3cad797, the failing tests (W1 and W2 red on today's rule: the follower was refused 12 times in a row in W1,
37 and 36 in W2); 878285a, the slot. Built for merge, not merged, not pushed.

## What I kept from your ruling
- Q1: during a slot the leader holds its own writes and floor sessions (`prepare` defers them), its class entry
  (`propose_class`), and other members' forwards, after the sender and origin checks (in `step`, after the checks
  and `answers.heard`). A change or a hand-off releases the slot, and no slot starts during either or a bootstrap.
  (a) A second forward from a member already held ends the slot `released`, and both are stepped in order; tested in
  `a_second_forward_from_a_held_member_releases_the_slot`. (c) Held messages keep their admission `Reservation`, so
  they stay charged to `QUEUED_BYTES`: at most (members − 1) forwards plus one barrier per member. (d) is in the
  decision.
- Q2 (a): a member is queued at the leader's append of its refused entry (`step_forward`, by
  `replication::verdict` against the entry before it). That is earlier than the commit, and equivalent to it: a leader
  never displaces its own term's entries (Leader Append-Only), and the queue dies with the term. (b): the holder's
  barrier steps only once the commit reaches the leader's last index at the slot's start. W3,
  `a_follower_retry_racing_the_slots_start_lands_on_its_next_try`, sets this up with the leader's W appended, one
  follower's answers held and the holder's appends lost. It is red without the hold ("the database advanced since
  preparation"), green with it.
- Q3: the length is learned per member from slot start to forward, the longer of the current and previous generation
  (`patience::GENERATION`). An expired slot keeps measuring, so a late retry teaches the next. The first value is
  `election_tick × TICK`, the ceiling is `patience::QUORUM_CEILING` (typed `slot::Unmeasured::PastCeiling`, not
  learned), and the slot has no constant of its own. TICK and the 20 ticks are noted for amphora-constants in the
  decision. Falsifiers with numbers are in the decision: the expired share against 1/(n + 1) (order statistics), and
  the held share against the model's 0.32.
- Metrics, as you asked: `amphora_replica_write_slot_holds_total{ended=forwarded|expired|released}`,
  `amphora_replica_write_slot_held_seconds_total` and the gauge `amphora_replica_write_slot_seconds`. Sent to Kiln
  Metrics (g-slot-metric-names-v2-20261111); no answer yet. The doc-truth test checks the names and the `ended` values.
- `slot_off` is `#[cfg(test)]` only.

## Two departures, for your ruling
1. **The slot starts at the queued member's next barrier, not at the refusal.** My first build started it at the
   append of the refused entry. Five existing driven tests then hung (time stands still in them). The cause was a real
   fault, not the harness: a slot started at the refusal holds the leader for a member that may never come back. That
   covers a caller that doesn't retry, and the floor keeper, which retries only once the applied index moves, which
   a held leader stops. Such a slot can only expire, and the late retry it then measures teaches a longer slot,
   feeding back toward the ceiling. Now the refused member is queued, and its next ReadIndex waits at the leader
   until its turn: the queue in barrier order, the leader's turn between, the barrier stepped once the commit
   reaches the slot's start. A member that never comes back never holds the leader. The gap measured is the retry
   alone. The holder's second barrier before any forward (a read) ends the slot `released`. Point 3 ("the barrier
   taken after the slot is held") holds by construction.
2. **The tail rule covers floor sessions and every write a follower makes, not the leader's own writes.** With it at
   the leader too, the leader's writes in W2 were refused at most once in a row (against up to 8 without it), and the
   tail rule alone kept W1's follower to one refusal in a row. But `a_raise_beside_a_change_holds_through_a_lost_lead`
   and `a_raise_whose_base_is_joint_is_refused` rest on a leader write prepared beside an entry in flight. A leader
   tail rule changes that, which is beyond what G was agreed to change. Today's rule at the leader refuses it no more
   than before (W2's leader: 29-33 refusals per 15 writes, against 45 per 15 at 3cad797). Your call whether it
   becomes its own decision.
   One existing test's premise moved for point 3: `a_write_at_a_follower_is_answered_on_its_groups_commit` now holds
   the follower's forward (the Pump's captures) until the leader's write is appended. The follower now prepares on its
   tail, so without the hold both writes land. What it checks is unchanged.

## Tests (adas-mac-studio.local-2, debug, 2026-11-11, load 4 to 7)
fmt --check, clippy -p amphora --all-targets -D warnings, check-production.sh, cargo test -p amphora --lib (184 passed,
2 ignored), and the integration tests replication, application, kiln_seam and transport all passed.
- W1, the leader and one follower over 2 and 32 lists, 3 seeds each: the follower was refused at most once in a row
  (2 to 5 refusals per 15 writes; 0 in one run). The control (slot off) still starves.
- W2, two followers and the leader: every write committed, none unknown, each follower refused at most once in a row.
- Every history was serializable (Elle list-append: replicas equal, a serial replay per list, no cycle).
- W4: ten core tests in database::slot.
Not run: the seven gates, Linux and Windows clippy, the workspace test, the release A/B (the batch's).

# Notice (key g-slot-receipts-20261112): t1/g-slot at 0030f95, tree 9df96a577ddceb94f4973fa9e0ef0feaa50789e9
Ruling 2 of amphora-owner-t1-g-slot-fc28e2d-20261112, applied:
- A held forward or barrier answers its receipt Ok as it is held. The receipt plumbing (`step_checked`, held
  responses) is gone, and `step` is back to its 878285a shape.
- At a lost lead, `step_ready` steps nothing that was held. It discards it, counted in the warn event
  `replication.write_slot_discarded` with `forwards` and `barriers` (the name is going to Kiln Metrics).
- The relay's NotLeader tolerance is removed. "A forward that meets an election" now describes the discard, and its
  falsifier is back to Transferring and NoLeader.
Ruling 3: `Driven::lapse_slots` counts its moves (`Driven::lapses`) and asserts that each ends the slot it was made
for.
Green on local-2: fmt; clippy -p amphora --all-targets; check-production.sh; amphora lib (185 passed); the integration
tests; the floor nemesis 12 of 12; W2's followers at most once in a row and its turns (13-14, 0). Kiln Metrics agreed
the slot names (amphora-g-slot/names/v2), applied at 7e1e024.
