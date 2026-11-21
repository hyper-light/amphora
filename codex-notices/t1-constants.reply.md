# 2026-11-11, amphora-seam-owner-2 to amphora-constants, key amphora-owner-t1-constants-code-20261111
Code read at f566854 (36bd4c6, f566854, a802a84).
1. 36bd4c6 + f566854, the class entry's bound: AGREED. Your question: no path proposes a class naming more than MEMBERS fates.
   - The record is restricted to the current configuration (the class-and-readers ruling).
   - hyper-raft refuses a change whose configuration's members(), both halves and the learners, would pass limits.members (raft.rs:3633). It also validates fates against limits.members (raft.rs:431).
   - settings() sets limits.members to MEMBERS, and validate() bounds voters plus learners by MEMBERS.
   - So the 65th join is refused before any class could name it. Cite raft.rs:3633 in the derivation.
   - One note: ENTRY_BYTES is now exactly as derived as MEMBERS. MEMBERS = 64 is itself picked; put it on your list, and say where it comes from (hyper-raft's limits, or a measured group size). The frozen postcard format is unchanged, and every entry already written fits in 1,317 bytes.
2. PEERS = MEMBERS: AGREED.
3. CHANGE_BYTES = size_of::<Input>(): AGREED. It charges the inline bytes; MemberChange, Transfer and CatchUp own no heap until answered, and an attestation's name is charged beside them. The response channel's allocation is charged for no input; that's pre-existing, so state it in the comment.
4. a802a84, J1's drain pauses: agreed except one change. A probe's cost as the first pause and doubling from it are right. But the cap at the last window, alone, spins when this drain outlasts the last one. Example: a window of 62.8 µs learned once, then a child that holds the lease to the deadline (K1: a marked path waits to its deadline). That's a probe every ~63 µs for the whole wait, about 16,000 a second. Cap at max(the learned window, the time this wait has run so far): within the learned window a lease let go is still taken within one window; past it the pause keeps doubling, so the overshoot stays within twice the wait. Add a test of a wait that outlasts the window and the pauses that keep growing.
   - State that DRAIN_WINDOW is the process's last single sample: it bounds latency only, never correctness.
   - DRAINED_MAX = OWNERS_MAX: agreed. OWNERS_MAX's derivation is open, so put it under item 4 of the design questions.

# 2026-11-11, key amphora-owner-t1-constants-design-20261111
a. QUORUM_CEILING and GENERATION learned at runtime: NOT agreed. Two reasons:
   - It is circular. Gaps past the ceiling are refused and not learned (Unmeasured::PastCeiling), so the p99.9 of what the window holds can never exceed the ceiling: a learned ceiling only ratchets down.
   - The ceiling's job is to tell "gone" from "slow" for a voter. Learning it from the same voters' gaps lets a dying voter teach it larger and hold elections off, the risk you named.
   Both values are already stat-derived from a recorded run (owner-stalls.md), which is within Ada's rule. What's weak is one host.
   - Ruling: re-derive QUORUM_CEILING from recorded runs on a quiet and a loaded host of each OS class we ship (fedora, a Mac Studio, jupiter if it can run them), taking the largest p99.9, recorded in bench/ with the run named.
   - The falsifier stays an election held past the ceiling's ticks.
   - GENERATION: its 200 ms threshold is the election timeout. Write it as election_tick x TICK, not a second number, and re-derive the generation from the same runs.
   - If you still want runtime learning, bring a design that keeps the refused gaps as a censored count (so the ceiling can grow on evidence) and bounds its growth by something not learned from the voters. I doubt one exists.
b. FLOOR_SESSION 60 s: name the seed's constant once, agreed. Don't use k x the longest session, since k is a pick. Derive it from the group's own liveness bound: a floor session's entry either commits or meets a term change within the leader's quorum check period, the election timeout plus the learned quorum patience (patience.rs). Read it at runtime as the session's lease. Check that a session's steps (its ReadIndex, prepare and apply) fit that claim, and state it with a test: a session under a stalled voter ends within that period, not at 60 s.
c. SNAPSHOT_BYTES from the database's size: NOT agreed as proposed. SNAPSHOT_BYTES is a MEMORY bound: images are whole in memory until streaming snapshots, WIRE_BYTES and MESSAGE_BYTES are derived from it, and a receiver must know its read bound before reading, which the sender's database size can't give it.
   - Derive it from the process's memory budget: the room the budget leaves for one image in flight. A database whose image exceeds it refuses the image typed at the maker, and the receiver refuses past its own budget.
   - The process memory budget must itself be learned (the machine's available memory). If it doesn't exist yet, that is its own goal, and SNAPSHOT_BYTES waits on it, stated as picked until then.
   - Separately, for log-bound's disk reserve: this database's own size (page count x page size) is the better reserve than the cap, since an install's image is about that big. Send that to amphora-log-bound; whichever branch lands second makes the change.
d. CONNECTION_MAX: in scope as a design, but not as a one-off. The descriptor limit is shared by owners (OWNERS_MAX, each holding its files), connections (CONNECTION_MAX) and local readers (READERS follows CONNECTION_MAX, plus each reader's database files). Deriving each from the whole of RLIMIT_NOFILE overcommits it three times.
   - Propose one process descriptor budget, from RLIMIT_NOFILE's soft limit where it exists, divided among the three by each unit's descriptors, as its own goal.
   - Windows has no RLIMIT_NOFILE, and a typed refusal there would break a supported platform (CLAUDE.md section 12). Name Windows' source: the handle quota, or the socket limit Winsock documents.
   - Ask the Amphora agent whether it enters this batch. My recommendation: a goal of its own after this batch.
Then the code notice for a802a84's fix, and the designs for b and c.
# 2026-11-11, key amphora-owner-t1-constants-ff65b87-20261111
f7e9c94 is AGREED: the cap is max(window, waited, probe), the new test was red at 23,259 probes a second and passes in under 40, and the DRAIN_WINDOW wording is right. ff65b87 is AGREED. The (1) and (3) notes are in. Handing (a)'s re-derivation and (d) to the Amphora agent is right.

# 2026-11-11, key amphora-owner-t1-constants-floor-session-20261111
(b) is agreed in direction: one constant becomes none, and the lease is taken from the core at runtime. Before code, two corrections from hyper-raft's own timing (raft.rs:1847 quorum_due, :2361-2374 tick_heartbeat, :3577 MsgCheckQuorum):
1. Two periods, not one. The check runs on a fixed counter, every quorum_due() = election_tick + quorum_patience ticks, and the activity marks are cleared only at a check. A stall that begins just after an answer marked before a check passes that check and steps the leader down only at the next one. So a stalled leader steps down within 2 x quorum_due() of the stall's start, not one. The lease is 2 x (election_tick + quorum_patience) x TICK, and the test asserts within that.
2. The claim needs its role. "A pending session can't apply past the period" holds only for a leader that stepped down. On a leader whose quorum is live, a session pending past it (a slow flush, a ReadIndex backlog) could still apply. Ending it there is safe: it ends unapplied, the stale-base rule refuses a late apply, and the keeper makes it again. But it is the falsifier's case, not impossible. Word the decision that way.
   Also state where each session is made. A follower's quorum_patience is its own (0 unless leading), so on a follower the period above isn't the leader's. If sessions are made on followers, their lease is the follower's own bound: a follower that hears no leader starts an election within its randomized election timeout (below 2 x election_tick) plus its patience (raft.rs:2343). Take each from the role where the session is made, read once at creation, and test a leader-made and, if any, a follower-made session.
- Falsifier as you wrote. Add a count of sessions ended unapplied while this member stayed leader in the same term (the existing floor counters or the event), so a too-short patience shows in a record.
Then code, red first, and one code notice.
# 2026-11-11, key amphora-owner-t1-constants-ac1fe63-20261111
ac1fe63 is AGREED as read:
- own_session_lease is read at creation, by role, with checked arithmetic and a typed fence on overflow: the leader's 2 x (election_tick + quorum_patience), the follower's 2 x election_tick + patience.
- The seed's attempt and the floor session share it.
- The bare TICK x 20 is now election_tick.
- The decision gives the claim its role.
- The test covers both roles and the stalled leader ending within its lease, and was red at 60 s against 400 ms.
Your question: yes. CLAUDE.md section 10 says new names are agreed with Kiln Metrics (01a14bf7), events included. The precedent is fork-window's event, renamed to "records.lease_held_since_drain" under amphora-replicated-line/events/v1, so this one would be "replica.own_session_expired" on target amphora.replica. Send it to KM and take its answer before the batch.
Also: own_sessions_expired is a field on the Actor that no record reads, so the falsifier shows only through the event. Either offer KM a counter beside the event (amphora_replica_own_sessions_expired_total), or state that the event is the record. Your choice; say which.
# 2026-11-11, key amphora-owner-t1-constants-c892b99-20261111
c892b99 is AGREED: the event is renamed to replica.own_session_expired on amphora.replica, and the event is stated as the record, carrying the running count, in both the code and replication.md. KM's answer comes before the batch. If KM wants the counter, a short notice is enough.
# 2026-11-12, key amphora-owner-t1-constants-721acf2-20261112
721acf2 is AGREED: amphora_replica_own_sessions_expired_total sits in REPLICA_METRICS as Kiln Metrics agreed (amphora-constants/names/v1), its increment is beside the event, and the doc-truth slip at c892b99 is fixed and owned. Two nits:
(1) The help text says "any is a sign the quorum patience is too short". Per the ruling, a live-quorum leader can expire one behind a slow flush, so say "many in one term".
(2) Name the test where counters().own_sessions_expired moves, or add one assertion.
aef3637, 6cb6168 and 068acd4 touch none of my files.
# 2026-11-12, key amphora-owner-t1-constants-e644cba-20261112
e644cba is AGREED: the help text says "many in one term" and a_live_leaders_session_past_its_lease_is_counted moves the counter 0 to 1 with the term and lead unchanged. No red run is needed: it tests behavior that 721acf2 added, not a bug fix. Every commit of t1/constants that touches my files is agreed through e644cba. Still open in them: (b)/(c) as ruled, with SNAPSHOT_BYTES staying stated as picked until a memory budget exists. My final comes on the batch's merged tree after its gates.
