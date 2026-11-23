# Lane P8 (serve): P7's session worker checked, data requeued on a moved lead, the second stall traced (2026-11-23, adacachy2)

Goal: a replicated server that answers writes of any size and length; every write commits or is refused typed,
and one long write never costs the group its leader (CP-T1.13). Lane P7 built the session worker on perf/serve-p7
(513a8912, never built). This lane checked it, added P7's items 2 and 3, and traced the 20 MiB write's second stall.
Nothing merged to main: the merge gate (the replicated suite back to main's pass set, both cp_t1_13 tests) is not met.

All runs through kiln `run`, `on` adacachy2 (00aad8da...), needs `linux/amd64,cpus=4,mem=16g`, CARGO_INCREMENTAL=0,
CARGO_BUILD_JOBS=4, debug test build in P7's warm target dir, script ~/.cache/amphora-perf/serve-p8-scripts/run.sh
(`run.sh <sha> check`: clippy, `--lib session_worker`, `--lib database::replicated`, `--test serve cp_t1_13`).

## Item 1: 513a8912 built and checked (r1170, load average 22-29)

- `cargo clippy -p amphora --all-targets --locked -- -D warnings`: exit 0 (4ff28e52 included).
- `session_worker`: 2 of 2 ok (the lead held through three election timeouts during a session; a mid-session hand-off
  refused NotLeader with the session run once).
- `database::replicated`: 132 passed, 7 failed, 4 ignored (main: 134 of 134). Failed:
  `a_raise_another_lead_wrote_over_is_answered_unknown_outcome` (the raise never answered in ROUNDS),
  `a_raise_whose_base_is_joint_is_refused` (the group never settles once appends flow again),
  `a_refused_follower_write_lands_on_its_next_try_under_the_slot`,
  `a_slot_holder_whose_write_fails_ends_its_slot_without_waiting_its_length` (unanswered after ROUNDS; not in P7's list),
  `seed_and_delta_hold_through_an_election_at_each_step`, `a_write_at_a_follower_is_answered_on_its_groups_commit`,
  `receive_receipts::admission_is_bounded_while_processed_receive_waits_and_errors_remain_observable`.
  P7's `no_writer_is_refused_twice_in_a_row_when_every_member_writes` and `a_raise_beside_a_change_holds_through_a_lost_lead`
  passed in this run.
- 4ff28e52 (the worker rings the owner as it ends, the owner fences): the admission test's session panicked on the
  worker and the test failed in 152 s of suite time instead of hanging 120 s on the driver: the fence works there.
- `--test serve cp_t1_13`: 0 of 2 (16 MiB at serve.rs:822, 20 MiB at serve.rs:759).

## Items 2 and 3 (5a9a8f62), and the seed test (ab4853eb)

- Item 2: the worker hands the operation back with its result (`Back = (Ran, Operation)`). A write whose lead moved
  while its session ran, and which prepared something, is requeued if it is data (anything but `Operation::Call`),
  as a write whose lead moved before its preparation is (`requeue`); a caller's closure, spent, is refused NotLeader.
- Item 3: a promotion whose barrier passes while a caller's session is out waits in `Stage::Raising` for the write
  under way to end (`raise_waiting`), as a raise past the member bound does; `admits_voter`'s Busy refusal for an
  out session is gone (its one caller now waits first).
- `seed_and_delta_hold_through_an_election_at_each_step` submits its delta as a closure, so item 2 does not requeue
  it: it is refused NotLeader, nothing proposed. The test now accepts NotLeader beside Ok and UnknownOutcome and
  asserts every replica still reads the old value (x'02'). It passes alone on 2abf4f13 (r1195).

5a9a8f62 (r1186, load average 16-29): clippy exit 0; session_worker 2 of 2; replicated 132 passed, the same 7 failed;
serve cp_t1_13 1 of 2 (P5's 16 MiB ok, the 20 MiB one failed at serve.rs:759).

ab4853eb (r1201, load average 13-17): clippy exit 0; session_worker 2 of 2; replicated 132 passed, 7 failed:
`seed_and_delta_hold_through_an_election_at_each_step` now passes, and
`two_followers_and_a_leader_writing_steadily_all_commit_within_their_deadlines` failed at its streak check (a follower
write refused more than twice in a row), which passed in r1170 and r1186. It is the write slot's family, as P7's
`no_writer_is_refused_twice_in_a_row_when_every_member_writes` is; which of those fail varies from run to run, and its
writes are closures, which item 2 leaves as they were (inferred: the slot's turn, item 4, not this change). serve
cp_t1_13 0 of 2 (16 MiB at serve.rs:822, 20 MiB at serve.rs:759); the 16 MiB one passed in r1186 and failed here.

## Item 5: the second stall is the leader's own check-quorum step-down

Probe on branch perf/serve-p8-probe (2abf4f13, never for main): P5's `p5_probe_large_replicated_writes` back in
tests/serve.rs, and each owner step logged (`P8PROBE`) when it changed term or leader, outlasted one tick period,
left egress paused, or carried a record over 64 KiB. Command (r1195, load average 14-17):
`P5_SIZES=20 cargo test -p amphora --locked --test serve p5_probe_large -- --nocapture --test-threads=1`;
full log ~/.cache/amphora-perf/serve-p8-probe.log on adacachy2.

Measured, one run (times from the probe's first event):

| at | member | what |
|---|---|---|
| before | all | leader 3, term 1; tick period: leader 0.98 ms, followers 10 ms; election_tick 20 |
| 1.460 s | 3 (leader) | one step, no input, took 264.6 ms; last index 3 -> 4 (the 20 MiB entry proposed) |
| 1.486 s | 3 | next step took 25.9 ms |
| 1.504 s | 1, 2 | each stepped one record of 21,082,349 bytes from 3 (the append), 12.6 ms each |
| 1.522 s | 3 | term 1, leader 3 -> 0, no input, no term change: the leader stepped down on its own (check quorum) |
| 1.816 s | all | term 2, member 3 elected again |
| 2.745 s | all | each owner took 915-924 ms in one step applying the 20 MiB entry (applied 3 -> 5) |

The write answered UnknownOutcome after 1.26 s; the next write and the select answered Ok. No follower campaigned
first: the leader lost its lead in term 1 by its own check of a quorum, 62 ms after its 264.6 ms step (measured). Its
tick period was 0.98-1.05 ms (measured), learned from heartbeat and vote round trips, vote flushes and lateness
(`TickPace::derive`, pace.rs), with no term for the size of a record in flight; its quorum check runs after
`election_tick` plus its quorum patience of its own ticks (raft.rs `become_follower(term, 0)` gives the leader 0 seen).

Why the quorum went silent, read in the code at 513a8912 (a read-only trace this lane ran; line numbers there):
- A follower's messages, heartbeat answers included, leave only after its `Ready`'s write and every earlier write
  are durable (hyper-raft node.rs 310-330, 937-951; hyper-durable replica.rs 2143-2164), so both followers say
  nothing to the leader until the 20 MiB entry is on their disks.
- Members talk over one TCP stream and one sender thread per peer, every record type in one FIFO (serve.rs 540-551,
  1364-1396); the reader reads one whole record and then blocks in `receive` until the owner has stepped it (serve.rs
  649-652, replicated.rs 1701-1709), so a heartbeat behind the 21 MB append is not even read until the append is.
- No chunking: one command is one entry (`max_size_per_msg`, replicated.rs 50, 2816).
- Ticks: at most one per step, the input stepped before it (replicated.rs 3171-3183); no burst after a long step.
- "durable 3" in P7's probe is the durable commit (`durable_commit`), not the end of each member's durable log; P7's
  "nothing past index 3 durable" over-reads it.

V4's hyper-raft 0009 on perf/commits-v3 ("a member whose term and vote are durable answers a heartbeat at once, naming
the durable end of its log and its durable commit", `Raft::answer_beat_now`) changes the first of these. Run with it
(r1209, branch perf/serve-p8-probe-v4 0f16b8f0 = the probe merged with perf/commits-v3, load average 9-15,
`P5_SIZES=20,20,20`, three fresh groups; log ~/.cache/amphora-perf/serve-p8-probe-v4.log on adacachy2):

| write | answer | leader's proposal step | leader's tick period | what happened |
|---|---|---|---|---|
| 1 | Ok after 1.86 s | 259.7 ms | 1.32 ms | lead kept; then every member applied for 901-907 ms in one step |
| 2 | UnknownOutcome after 1.22 s | 258.6 ms | 0.60 ms | 42 ms after that step the leader stepped down in term 1 (leader 3 -> 0); term 2 elected it again |
| 3 | Ok after 1.89 s | 257.9 ms | 1.00 ms | follower 1 lost its leader in term 1 for 238 ms, until the 21,082,349-byte append reached it; lead kept |

`--test serve cp_t1_13` on 0f16b8f0: 3 of 4, the 20 MiB test failed at serve.rs:759, P5's 16 MiB one ok.
So V4's change is needed but not enough (one run of three, no noise measured): the leader still steps down when its
heartbeats' answers come later than its quorum window. Inferred from the timeline: with followers' answers no longer
waiting for their disks, what remains is the 21 MB append ahead of the heartbeats in each peer's one FIFO, read whole
and stepped (12.6-14.8 ms) before the heartbeat behind it is read, against a quorum window of `election_tick` (20) plus
the quorum patience, in ticks of 0.6-1.3 ms.

Three costs proportional to the write's size, all on owner threads (measured above, debug build):
1. the leader's proposal step, 264.6 ms + 25.9 ms (encode, own log append, the sends; not yet split by part);
2. each follower's step of the 21 MB record, 12.6 ms, ahead of the heartbeats queued behind it;
3. each member's apply of the entry, 915-924 ms: no election here only because all three stalled together. P6's
   design moved the session off the owner, not the apply.

Not done here; the fix is the successor's, at these causes, none with a picked size:
- followers' heartbeat answers waiting for their log write: V4's 0009 (above), once it lands on main;
- heartbeats behind a large append in one FIFO per peer: control records (heartbeats, votes and their answers) on a
  path of their own per peer (serve.rs `serve`, the host loop, `to_peer`, `connection`; `Outbound` carrying the
  message's kind from `release`), and the reader on the non-blocking `admit_receive` (replicated.rs 1716);
- one record outlasting the ticks: split a large entry at `propose` by what each path carries in a heartbeat interval,
  which the owner already measures (`Drain::carried`, `carry`), reassembled on apply as a seed's chunks are;
- the owner's own size-proportional steps (the proposal's copies, 264.6 ms; the apply, 915-924 ms): off the owner,
  as the session now is.

## Not done in this hour

- Item 4 (the slot's turn covers the holder's whole session): not started. Two slot tests fail.
- Item 6 (the worker's connection from the replica's reader budget, P6 point 6): not started. `Local::with` waits on
  a condvar, so the take must happen on the worker thread, not the owner's.
- `a_write_at_a_follower_is_answered_on_its_groups_commit`: two overlapping follower sessions now prepare at once;
  the later one's base is stale at apply (Serialization). Before, each prepared on its tail and proposed in one step.
- `receive_receipts::admission_is_bounded_...`: its premise ("the owner is inside this capture") no longer holds,
  since the owner runs while the session does; the test needs another way to hold the owner.
- The two raise tests: not traced.
- The commit-latency A/B at 1 and 32 writers: not run (nothing merged).
