# t1/constants: for amphora-seam-owner-2 (from amphora-constants, 2026-11-11)

Ada, 2026-11-11: "We don't tolerate arbitrary constants, learned and stat derived values only." Branch t1/constants
(~/Projects/amphora-t1-constants) on 490ea83, head f566854. Not pushed, not merged. The full list:
~/.cache/claude-scratch/amphora/handoff/amphora-constants-list.md.

## 1. Code review asked (one per branch): what touches your files
- 36bd4c6 + f566854 (replication/class.rs, database/patience.rs, database/replicated.rs, replication.md):
  - class.rs ENTRY_BYTES 64 KiB → what the largest class encodes to, from postcard's varint widths: prefix 17 +
    tag 5 + length 10 + MEMBERS × 20 + span 5 = 1,317 bytes. `replicated::MEMBERS` is made pub(crate).
    Red: `a_class_entry_holds_the_most_members_and_no_more` ("a class past the members", taken under 64 KiB).
    The question for you: can any path propose a class naming more than MEMBERS fates (a joiner's fate added
    at 64 members)? I found none; the members bound refuses the 65th join first.
  - patience.rs PEERS 64 → `super::replicated::MEMBERS`.
  - replicated.rs CHANGE_BYTES 1,024 ("policy") → `size_of::<Input>()`, the queued input's own bytes; an
    attestation is charged its name beside it. No behavior test: only the budget's arithmetic changes.
- a802a84 (database.rs, database/remote.rs, local-owner-transport.md, J1, which the previous owner decided):
  FIRST_PAUSE 50 µs / LONGEST_PAUSE 1 ms → learned. The first pause is the probe's measured cost; each next
  pause doubles (never below the probe's cost), capped at the window this process last waited out before
  taking a drained lease (DRAIN_WINDOW); before any window, doubling to the deadline. Pure core `next_pause`,
  unit test `database::drain_pause`. DRAINED_MAX → OWNERS_MAX, named once where 4096 was written three times.
  Green: database:: 128; guards+remote 19/19 ×3; amphora lib 173; clippy clean (local-2).

## 2. Design before code: the rest of yours (your word asked on each)
- patience.rs QUORUM_CEILING 165 ticks (adacachy's p99.9, 1,642.788 ms) and GENERATION 3,000 ticks were
  measured once on one host. Proposal: learn both from the window's own record. The ceiling becomes the p99.9
  (the quantile replication.md's record took) of the gaps the window held over its last generation. The
  generation becomes the time the window took to fill with PEERS × its answers, so no 200 ms gap threshold is
  needed. Open: a learned ceiling can grow and hold an election off. Recommendation: learn it, record it on a
  loaded and a quiet host first, and let the falsifier be an election held past the learned ceiling's ticks.
- replicated.rs FLOOR_SESSION 60 s ("policy, the same as the seed's attempt"): proposal: name the seed's
  constant so there is one, then learn it from the floor sessions' measured durations (a session past k times
  the longest seen ends). k is still a pick; do you have a better source?
- replication.rs SNAPSHOT_BYTES 256 MiB: proposal: the database's size in force (page count × page size) when
  the image is taken, refused against the filesystem's free space. amphora-log-bound's disk reserve takes it.
- CHECKPOINT_EVERY, FRAME_BYTES's 4096, restart_ready's /64, LOG_FRAMES: amphora-log-bound's, not touched.
- transport/wire.rs CONNECTION_MAX 32 (2026-10-29, so before the window; READERS follows it): proposal: from the
  process's descriptor limit in force (getrlimit(RLIMIT_NOFILE) where it exists, a typed refusal naming the
  platform where not), each connection holding its socket. Is it in scope for this batch?

Answer under `amphora-owner-t1-constants-<topic>-20261111`, appended to codex-notices/t1-constants.reply.md.

## 3. After amphora-owner-t1-constants-code-20261111 and -design-20261111 (2026-11-11)
Code notice (head ff65b87):
- f7e9c94, (4)'s fix: the pause cap is now max(the learned window, this wait so far). A new test,
  `a_wait_that_outlasts_the_learned_window_keeps_doubling`, was red with the old cap ("23259 probes in a second")
  and takes under 40 probes in a second with the fix. DRAIN_WINDOW is stated as one last sample that bounds latency
  only. The (1) and (3) notes are in: ENTRY_BYTES cites raft.rs:3633 and :431; CHANGE_BYTES says the response
  channel is charged for no input. Green: drain_pause + class 6, guards+remote 19/19 ×3, clippy clean.
- ff65b87, (a): GENERATION's 200 ms is written as the election timeout (election_tick × TICK).
- (a)'s re-derivation of QUORUM_CEILING and GENERATION from recorded runs (a quiet and a loaded host of each OS
  class, the largest p99.9, recorded in bench/) is a bench run. It goes to the Amphora agent, who owns the batch's
  benchmarks.
- MEMBERS = 64 is added to my list as picked; (d)'s descriptor budget goes to the Amphora agent as its own goal.

Design (b), FLOOR_SESSION, before code:
- One constant becomes none. The seed's internal attempt (replicated.rs:2712, a bare 60 s) and FLOOR_SESSION (60 s)
  both take the leader's quorum check period, read at runtime from the core where each session is made:
  `(raft.config().election_tick + raft.quorum_patience()) × TICK`.
- Why that period: a leader that hears no quorum within it steps down, so a session still pending past it can't
  be applied in this term. It ends unapplied and is made again by the keeper's next pass (the seed's committed
  chunks survive, as now).
- Test, red first: a floor session under a stalled voter (the test shell's clock moved past the period, a quorum
  held back) ends unapplied within election_tick + patience ticks and is made again. At 490ea83 it waits the 60 s.
- Falsified by: a floor or seed session that would have applied ending unapplied over and over under a loaded but
  live quorum. The patience is the learned quorum tail, so that would mean the patience is too short, and the
  fix belongs there.
Your word on (b) before I code it. (c) is passed to amphora-log-bound as you said.

## 4. Code notice for (b), ac1fe63 (2026-11-11)
- FLOOR_SESSION and the seed's bare 60 s become `Actor::own_session_lease`, read when the session is made. A leader
  gets 2 × (election_tick + quorum_patience) ticks; a follower (floor sessions are made on attesting followers)
  gets 2 × election_tick + patience. The seed's attempt is leader-only. The decision text in replication.md gives
  the claim its role: past the lease the role has moved on; on a live-quorum leader it's the falsifier's case.
  Such an end is counted (`own_sessions_expired`, same term, still leader) and logged at amphora.replica, event
  own_session_expired. The event name is new: does it need the Kiln Metrics agent's word?
- The seed transfer's re-request period: `TICK × 20` becomes the core's election_tick.
- Red: `an_owners_session_lasts_its_roles_raft_timing` ("the follower's session lease, left: 60s, right:
  400ms"). It tests a follower-made and a leader-made session, and that a stalled leader's session ends within its
  lease. Green: amphora lib 175, tests/replication.rs 3, clippy clean (local-2).
