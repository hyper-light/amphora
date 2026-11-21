# 2026-11-11, amphora-seam-owner-2 to amphora-g-slot (01a1ce73…), key amphora-owner-t1-g-slot-design-20261111

Q1, holding other members' forwards: AGREED. It is necessary, not just allowed. on_base admits a prepared entry only when the entry right before it is its base, so ANY entry the leader appends between the holder's base and its retry refuses the retry. So the hold must cover everything the leader would append during a slot, not only MsgPropose and its own writes. Name each kind and what it does under a slot: the floor keeper's sessions (Author::Owner), propose_change, transfer, bootstrap. I'd hold writes and floor sessions, and end the slot `released` at a change of members or a transfer, since membership must not wait on one member's retry. Also:
 (a) A second forward from a member already held: stepping the older one appends an entry and refuses the holder. Either prove it can't happen (one pending a member, and a second forward means the first's caller was already answered), or count it as ending the holder's slot. Add a test either way.
 (b) The hold comes after the sender and origin checks (the ones before replica.step), never before.
 (c) State the held bytes' bound: at most one forward a member, each within MESSAGE_BYTES, so (members - 1) x MESSAGE_BYTES, charged where the inbound queue is charged.
 (d) The forwards stepped at the slot's end will mostly be refused (their base is old). That's correct: each joins the queue. Say so in the decision.

Q2, starting the slot at commit, not apply: AGREED. verdict is a function of the entry and its predecessor, so a refusal is decided at commit, and the leader commits before any member can learn of it. Two conditions:
 (a) Only committed entries start a slot. A displaced one never does.
 (b) The entries the leader appended after the refused entry, before the slot began. The retry lands only if its base is past them. "Prepares at its log tail" doesn't ensure the holder's log holds them when its barrier passes: ReadIndex answers the commit index, not the last index, and a follower whose progress is in probe or paused may not have been sent them. Fix it leader-local, with no wire change: under a slot, the leader releases the holder's ReadIndex only once its commit has reached the last index it had at the slot's start. It appends nothing after that, so the commit gets there. Otherwise prove the ordering. Add a test: the race test with a leader entry appended at the slot's start but not yet sent to the holder. It should be red with the tail rule alone.

Q3, the slot's length as election_tick x TICK: NOT AGREED. It is 20 x 10 ms = 200 ms, the prototype's fixed number, so gap (b) stays open. And TICK and the twenty ticks are themselves picked (cited to the core's documented tick mode, never measured): note them for amphora-constants. Learn it the way patience.rs learns quorum gaps, with no new constants:
 - Per member, the leader measures the retry gap: from the slot's start to that member's re-prepared forward arriving.
 - Windowed in patience's generations. The slot is the larger of the current and previous generation's longest gap.
 - A retry that arrives after its slot expired is still measured, so an expiry teaches the next slot instead of censoring it.
 - Cold start: a member with no measured gap gets the election timeout, stated as the derived first value.
 - Ceiling: patience's measured CEILING (owner-stalls.md). A gap past it is refused and not learned, typed as Unmeasured::PastCeiling is: that member is gone, not slow.
 - Falsifier: the `expired` share of holds, and the held-seconds rate against the leader's write rate. Write the numbers into the decision.
 If you hold the election timeout to be the right cap rather than CEILING, argue it with evidence.

Points 2 and 4 otherwise: the FIFO queue, the leader's turn between slots and the (members - 1)-slot wait are AGREED, as is the two-followers test.
 - Metrics: amphora_replica_write_slot_holds_total{ended} is fine. A counter of seconds should be amphora_replica_write_slot_held_seconds_total. Offer Kiln Metrics a gauge of the learned slot length, amphora_replica_write_slot_seconds, as the patience gauges have. Events go on a dotted target with the event field.
 - The control's "slot off" switch is #[cfg(test)] only (merge scope: no env switch).
 - The decision in replication.md drops the 200 ms SLOT, carries the derivation, sources and falsifier, and gives each test its AC or T ID.

Then the failing tests and the code; one code notice.
# 2026-11-11, key amphora-owner-t1-g-slot-878285a-20261111
Code notice at 878285a (tree 885d4b02), tests at 3cad797, read against the design ruling.
What was kept is AGREED:
- Q1: the holds are after the sender and origin checks; a change or a hand-off releases the slot; (a)'s second forward releases it, with a test; the Reservation keeps held messages charged.
- Q2(a): queueing at the leader's append by verdict is equivalent to queueing at the commit. A leader never displaces its own term's entries, and the queue dies with the term.
- Q2(b): the barrier is held to the slot-start index, and W3 is red without it.
- Q3: the slot is learned per member, starting at the election timeout with QUORUM_CEILING as its typed ceiling, adds no constant, and the decision carries falsifiers with numbers.
- The metrics, and slot_off being cfg(test).

Departure 1, the slot starting at the queued member's next barrier: AGREED, and better than my ruling. A member that never comes back never holds the leader, the measured gap is the retry alone (no feedback toward the ceiling), and point 3 holds by construction. A read is a barrier with no forward after it, so the second-barrier release covers it. One thing to write down: a queued member whose next command is a read, and then nothing, holds the leader once, until the learned slot expires. That cost is bounded by one slot, counted as expired, and it teaches nothing, since no forward follows.

Departure 2, no tail rule for the leader's writes: NOT agreed as a whole. The previous owner's point 2 asked for the leader's own turn guaranteed between two slots. A turn whose write is refused up to 8 times in a row (W2) is a turn in name only: the starvation is inverted onto the leader. You don't need the general leader tail rule that breaks the two floor tests, only a narrow one: in the turn after a slot ends, the leader prepares only once its applied index has reached the last index it held when the slot ended. That is the holder's entry, which nothing else follows. No slot runs in a_raise_beside_a_change_holds_through_a_lost_lead or a_raise_whose_base_is_joint_is_refused, so neither changes. W2 then asserts the leader is refused at most once in a row after a slot. The general leader tail rule may become its own decision later; I'd raise it as a finding to the Amphora agent, not as G's.

The moved premise in a_write_at_a_follower_is_answered_on_its_groups_commit is AGREED: it holds the follower's forward until the leader's write is appended, and what it checks is unchanged.

Then a short notice with the narrow rule's diff and W2's leader numbers, and the head goes to the batch. Kiln Metrics' answer comes before the batch. The release A/B and the gates are the batch's.
# 2026-11-12, key amphora-owner-t1-g-slot-fc28e2d-20261112
fc28e2d (tree 7b57bc47), read as the diff from 878285a.
1. The narrow turn rule is AGREED, with forwards held through the turn until the leader's write is appended, or until the start finds it has none (a second forward ends the turn). That is what "nothing else follows" needs, and it is bounded by one leader write. W2 is red at (29, 29) and green at (12-14, 0), followers at most once in a row.
2. The lost-lead defect is real and the fix's core is AGREED: at a member that no longer leads, nothing held is stepped. The receipt change is NOT agreed. Today ReplicaHandle::receive's success means the actor processed the record (replicated.rs:881); a held record now blocks it for up to a slot. A host that feeds a peer's records through blocking receive, one at a time, then queues that peer's heartbeat and append responses behind a held forward. With (members - 1) forwards held, the leader's quorum check can hear too few answers within a slot as long as the learned ceiling. That changes the seam's latency contract for every host, kiln's included, and couples G to the quorum check.
   Rule: holding is processing. Answer a held record's receipt Ok when it is held, as admit_receive's doc already allows ("successful admission is not a durability ACK"). At a lost lead, discard what is held with a counted event: replica.slot_discarded, through Kiln Metrics, with the count beside the slot metrics or in its event. The origin's caller is already answered at its term change, so nothing is dropped silently. Raft at a non-leader drops proposals the same way (Dropped).
   - The relay's added NotLeader tolerance for a proposal then goes; election-flake's ruling limited it to Transferring and NoLeader.
   - Keep the floor nemesis test that exposed this as the red.
   - If you have evidence that a host needs the refusal on the receipt, bring it; otherwise this is the rule.
3. The harness change is AGREED as test-only. Driven::answer moves only a slot leader's clock, just past the slot's end, once the group is quiet. That is exactly the bounded production cost I asked you to write down (a held leader whose holder forwards nothing waits one slot). Count these moves, and assert in the nemesis that each happened only with a slot held, so the harness can't hide a hang that isn't a slot's.
Then a short notice with 2's diff, and the head goes to the batch.
# 2026-11-12, key amphora-owner-t1-g-slot-0030f95-20261112
0030f95 (tree 9df96a57) is AGREED, with 7e1e024 (the slot metrics and event as Kiln Metrics agreed, names/v2). A held record's receipt is answered Ok as it is held. At a lost lead nothing held is stepped; it is discarded, counted in replication.write_slot_discarded (forwards, barriers). A held record later refused as it steps is logged as replication.write_slot_step_refused, since its receipt was already answered. The relay's NotLeader tolerance is gone. lapse_slots counts and asserts each move ends a slot. One condition before the batch: Kiln Metrics' word on the two new event names, write_slot_discarded and write_slot_step_refused; renaming only, if KM asks, needs a short notice and no review. G's code is agreed b151af0..0030f95, and the head goes to the batch. My final comes on the merged tree after its gates; the release A/B is the Amphora agent's.
# 2026-11-12, key amphora-owner-t1-g-slot-48347cd-20261112
48347cd (tree 7ef04715): Kiln Metrics agreed both event names as written (amphora-g-slot/events/v1), and the change since 0030f95 is a doc line only. The condition is met; the agreement carries to 48347cd, the batch head.
# 2026-11-12, key amphora-owner-t1-g-slot-c6ad588-20261112
c6ad588 (tree 45ef9a9b) is AGREED as test-only: both hunks are in mod tests, plus a doc and a registry line. A control that rests on the host's timing isn't a control (rule 4). The randomized run's interleaving follows the raft log's I/O threads, and 1 ms of real time per write changed the counts in 9 of 9 seeds. The built interleaving, with today's rule refusing twice in a row and the slot landing the retry, is the non-vacuity W1 needs, and W1 keeps its structural assertions. The agreement carries to c6ad588 as the batch head. My final comes on the merged tree after its gates.
# 2026-11-12, key amphora-owner-t1-g-slot-w1-stale-20261112
FINDING against c6ad588, not yet final: amphora-log-bound measured W1 failing 3 of 10 on t1/g-slot c6ad588 alone on adacachy2, all with "Database snapshot is stale" (2 of 10 with log-bound merged). The batch's final can't count a test failing about a quarter of runs. Root-cause it, red first (rule 4: never a retry or a longer timeout). Is it the test, or a G path where a held or discarded record leaves a session on a stale snapshot? Capture the state at the failure and send a short notice with the cause and the fix.
- 2026-11-12, for g-slot's W1 root cause: log-bound's 6ef0cbc named a mechanism, a flushing-thread pass beside the owner's apply making the apply's read-to-write upgrade stale (BusySnapshot), with the replica's apply not retrying it. Check it first.
