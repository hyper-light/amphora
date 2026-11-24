# Lane P9 (serve): heartbeats and votes on a path of their own per peer (2026-11-23, adacachy2)

Goal: every replicated write of any size commits or is refused typed, and no long write costs the group its leader
(CP-T1.13), at agentic Meta scale. Coordinator's decision (16:44Z): heartbeats, votes and their answers never wait
behind a large entry: a path of their own per peer; large entries in parts sized by what a path carries in a heartbeat
interval; the proposal's copies and the apply off the owner thread. Design: docs/design/replication.md, "Decision
(lane P9)". Built here: part 1. Not merged to main: the merge gate (the replicated suite back to main's pass set, both
cp_t1_13 tests) is not met; the failures are the worker's (P7/P8), not part 1's (below).

All runs through kiln `run`, `on` adacachy2 (00aad8da...), needs `linux/amd64,cpus=4,mem=16g`, CARGO_INCREMENTAL=0,
CARGO_BUILD_JOBS=4, debug test build in P7's warm target dir, scripts ~/.cache/amphora-perf/serve-p9-scripts/
(run.sh, ab2.sh, ab3.sh). Probe: P5's `p5_probe_large_replicated_writes` with P8's `P8PROBE` owner logging
(perf/serve-p8-probe 2abf4f13), `P5_SIZES=20,...` (each a fresh three-member group, one 20 MiB write each).

## Part 1 (963f9af1): a control path per peer

`Outbound::route` (`api::replication::Route { Log, Control }`), set by the owner as it releases a record: `MsgAppend`
and `MsgSnapshot` take the log's path, every other record the control path. `serve` keeps one sender thread, one
queue and one connection per peer and route; the control connection is a new kind `H`, framed as `R`.

### A/B on the 20 MiB commit rate (measured)

r1249 (load average 23-26, CPU pressure some avg10 2-13%), 5 writes an arm, B then A:

| arm | Ok | not Ok |
|---|---|---|
| d1473a15 = probe + V4's 0009 (perf/commits-v3) + part 1 | 5 | 0 |
| 0f16b8f0 = probe + V4's 0009 | 5 | 0 |

Five writes do not separate them (P8 measured 0f16b8f0 at 4 of 5 and 6 of 8).

r1257, 10 writes an arm a round, arms alternated, two rounds (load average and CPU pressure some avg10 at each arm's end):

| arm | round 1 Ok | round 2 Ok | total |
|---|---|---|---|
| d1473a15 (V4 + part 1) | 7 of 10 (load 16-22, cpu 12.9%) | 10 of 10 (load 20-22, cpu 0.9%) | 17 of 20 |
| 0f16b8f0 (V4) | 0 of 10 (load 28-23, cpu 25.9%) | 9 of 10 (load 17-22, cpu 0.7%) | 9 of 20 |
| 445f37a4 (no V4 + part 1) | 9 of 10 (load 21-23, cpu 0.6%) | void (0 of 10, see below) | 9 of 10 |
| 2abf4f13 (no V4) | 0 of 10 (load 23-22, cpu 2.5%) | void (0 of 10, see below) | 0 of 10 |

Void: round 2 of the no-V4 pair, and r1257's raise-alone runs below. This lane started r1267 in the same remote
clone (~/.cache/amphora-perf/serve-p7-remote) while r1257 still built from it, and each run's `git checkout` changed
the other's sources mid-build (r1257's first raise-alone run printed no result at all). Only arms that finished
before r1267 started count: r1257's round 1 and its round 2 V4 pair.

Every write not Ok answered UnknownOutcome ("the commit outcome is unknown") after 1.26-2.26 s. Part 1 did no worse
than its pair in any clean round: 17 of 20 against 9 of 20 with V4's change, 9 of 10 against 0 of 10 without it. But
the noise is as large as the effect: the same arm went from 0 to 9 of 10 between rounds (0f16b8f0), and an arm's
ten writes run in about a minute of their own, at their own load, so the arms of a round do not share their conditions (inferred cause: the leader's proposal step and the followers' record steps
stretch with CPU contention, against an election timeout learned in quiet). Read as: part 1 helps, by an amount these
runs do not pin down. A fair A/B alternates the arms write by write on a quiet machine (cpus=16), or forces the
contention (a fixed CPU hog beside both arms) so both meet the same failure mode. The `P8PROBE` count of leaders lost within a term in these scripts matched nothing (the pattern did not fit
the log's format): read the logs, not that count.

### On main's hyper-raft 0009 (V5's `answer_beats_now`, on in Amphora), the coordinator's ask

r1267: perf/serve-p7 merged with origin/main (33121cad, then the design doc d50695f9), the probe added
(f2562197), and the same with part 1 reverted (4cc98921), alternated, two rounds of 10:

| arm | round 1 | round 2 |
|---|---|---|
| f2562197 = merge + probe + part 1 | void (no write ran: built from r1257's checkout) | 2 Ok, 5 UnknownOutcome, then the probe's next group "never healthy" (tests/serve.rs:88, exit 101); load 24-25, cpu 1.9% |
| 4cc98921 = the same, part 1 reverted | 1 of 10 Ok (suspect: r1257 still checking out; load 32, cpu 25.9%) | still running at hand-over |

On main's 0009 the remaining loss is not the path (measured, one write of round 2, `P8PROBE`): the leader's proposal
step took 310.5 ms with no input; 25 ms after it ended both followers lost their leader within term 1
((1, 3) -> (1, 0)) and member 2 campaigned into term 2. The followers tick every 10 ms with `election_tick` 20, so a
310 ms leader step with no heartbeat outlasts their election timeout: part 3's cost (the proposal's copies on the
owner thread; P8 measured 264.6 ms), which part 1 cannot touch. Inferred: the V4 arms above lost fewer writes because
their proposal steps (P8: 258-264 ms) fell under more of the followers' randomized timeouts; one run, not checked.
Its round 2's second arm and the merged head's check were still running at hand-over (r1267). Logs on adacachy2: ~/.cache/amphora-perf/serve-p9-ab3-<sha>-<round>.log and
serve-p9-m-{clippy,serve,servelib,repl}.log; the successor reads them into this record.

### Checks

- 963f9af1 (r1249): clippy `-p amphora --all-targets -D warnings` exit 0; `--test serve cp_t1_13` 4 of 4 (both
  cp_t1_13 tests pass at this commit and load); `--lib serve::` 11 of 11; `--lib database::replicated` 133 passed,
  6 failed: P8's five (`a_raise_another_lead_wrote_over_is_answered_unknown_outcome`,
  `a_raise_whose_base_is_joint_is_refused`, `a_refused_follower_write_lands_on_its_next_try_under_the_slot`,
  `a_slot_holder_whose_write_fails_ends_its_slot_without_waiting_its_length`,
  `a_write_at_a_follower_is_answered_on_its_groups_commit`) and `a_raise_beside_a_change_holds_through_a_lost_lead`
  (failed under P7, passed under P8). Part 1 does not reach the driven suite: its pump relays `Outbound`s itself
  and ignores the route. The raise test alone x5 on 963f9af1 and on b16c779f (r1257) is void (shared clone,
  above); rerun it alone before blaming or clearing anything.
- The merged head d50695f9: clippy, cp_t1_13, serve:: and the replicated suite queued in r1267 (above).
- `python3 scripts/vendor.py check hyper-raft` on the merge: 1377 files at d5d5f8f6, 10 recorded changes,
  0 differences. This branch's apply hold is now `0010-cp-t1-13-the-owner-s-apply-hold-beside-the-commit-fence.patch`
  (main's `0009` is V5's, `0011` W5's), its UPSTREAM.md row between theirs.

## Not done in this hour

- Parts 2 (entries in parts sized by a path's carriage in a heartbeat interval) and 3 (the proposal's copies and the
  apply on the session worker): designed in replication.md, not built.
- Small-write commit latency at 1 and 32 writers, before and after part 1: not run.
- P8's open items: the write slot's turn covering the holder's whole session (two slot tests and
  `a_write_at_a_follower`'s "both commit"), the worker's reader taken on the worker thread: not started.

## The two raise tests: a read-only trace (a delegate of this lane, nothing run)

- `a_raise_another_lead_wrote_over_is_answered_unknown_outcome` (read in code): the panic is
  `answered.err().unwrap()`, so the raise was answered `Ok`, not UnknownOutcome. Inferred mechanism: on the branch the
  write is proposed in `decide()`, after the step's drive and `release`, so when the test sees `Phase::Proposed` and
  drains the leader's outgoing, the append is not there yet; the next drive sends it, the followers take it before
  either can win, and the raise commits. Main proposes inside `prepare()`. Check that settles it: right after `reach`,
  whether the leader's `control.outgoing` holds the raise's append, on main and on the branch. If so the code follows
  P6's design and the test's assumption broke: wait for the append to be queued in the test's `reach`, or step once
  without relaying before the drain.
- `a_raise_whose_base_is_joint_is_refused` (read in code): every step up to letting the appends through passes; then
  the group never leaves its joint configuration in 60000 rounds. Not found. Candidates: (a) the leader's applies stay
  held after the session (`hold_apply(false)` resumes the core only with no fence set, replica.rs:638, else waits for
  `release_fence`), so the way-out entry commits and never applies; (b) the pause changes how the lost way-out entry
  is sent again. Check: at the failure, print each voter's `sessions.out`, `apply_held()`, fence, `apply_paused()`,
  committed, applied and `pending_conf_index`.
- Seen beside them: `raise_waiting` fires only when `pending` is none, so a promotion parked behind a caller's session
  waits for that write's commit, not only for its session's return.
