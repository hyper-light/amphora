# Lane P19 (serve): not landed; both failures traced to where the branch first differs from main, neither fixed (2026-11-24 UTC, adacachy2)

Goal: every replicated write either commits or gets a typed refusal, and no long write costs the group its leader
(CP-T1.13). This hour's deliverable: perf/serve-p7 on main, done when the two tests that fail only on the branch
(`v13_thirty_two_spread_writers_on_moving_time_are_each_answered_with_no_stale_base`,
`seed_and_delta_hold_through_an_election_at_each_step`) are traced against main, fixed where the branch first decides
differently, pass 3 of 3 with one clean round and both cp_t1_13 tests, and the branch is merged to main.

## Not done (losses first)

- perf/serve-p7 is NOT on main. No fix was written. Items 3 to 5 of the deliverable are not done.
- V13 is traced to a cause but not to one decision to change. The branch's session worker costs each write driven
  rounds that main's inline session does not, and the test's 5 s budget runs out. Not settled: whether that is the
  harness's moving time charging the worker's real latency, or a loss a real-time run shows too. The real-time
  scenario (lane V13's 32 spread writers on real time, its r1814) was not run.
- seed_and_delta is traced to the lost order: the leader queues its `MsgTimeoutNow` to member 2, and QB.2's shed
  drops it from the outgoing queue as its oldest record. Not traced: why the leader's queue is past its bound at that
  moment on the branch and not on main.
- kiln restarted at 02:18Z and lost the first run (r72, macOS). adacachy2 had 1 to 14 idle cores; most runs asked 1
  core (CARGO_BUILD_JOBS=1) after a 4-core ask was refused.

## Trace 1: V13 on moving time

Probe: the branch head 2f7c88a1 plus `#[cfg(test)]` logs of every write-slot decision at the leader (wait, start,
forward, close, end and how), each stale verdict with its base and the author of the entry before it, each place a
write is answered Timeout, each session handed to the worker and back, and each close. The same logs on main d88899f6.

| | branch (probe fd8be249, r15) | main (probe 816f78e1, r33) |
|---|---|---|
| result | FAILED: "Persisted: row 28 at member 2: UnknownOutcome" | ok |
| slots begun / ended | 19 / 19, then one barrier (member 3) waits and never gets one | 126 / 126 |
| writes refused stale | 0 | 0 |
| Timeout answers | member 2: one write queued behind its own write in `proposed`, at t=5010; member 3: its barrier, at t=5010 | none |
| a follower's slot, start to forward | 110 to 420 ms (e.g. start 4330, session handed 4380, back 4410, forward 4440) | 60 to 70 ms (e.g. start 2300, forward 2360) |
| between two follower slots | the leader's own turn on the worker: handed 4470, back 4490, next slot 4490 | none seen |
| slot start to the next slot start | about 160 ms | about 120 ms |
| one session on the worker | 20 to 260 ms of moving time (member 3: handed 4580, back 4840) | inline, no moving time |

Times are each owner's moving clock in ms from the first slot event. In r15 and r33 no write was refused stale (the
probe's stale line printed 0 times on both arms), as P18's r30 saw once V14's deadline was merged (cdaac483). So the
brief's "each follower write refused stale" had no case to log in this hour's runs.

Where it differs from main: main prepares a write inside the owner's step, so a session costs no driven round. The
branch hands it to the worker and decides it in a later step (`Actor::decide`), and the leader begins no slot while
its own session is out (`start_slot`'s `Phase::Running` guard). The harness moves every clock a tick per round, and
waits for work off its thread only after a round that relayed nothing (`Driven::wait_if_moving`). So a session out on
the worker costs as many ticks as rounds pass while it runs in real time; with 32 writers a round with nothing to
relay is rare (inferred from the harness's code, not counted). Measured: 20 to 260 ms of moving time a session, on
adacachy2 at load 37 to 64.

## Trace 2: seed_and_delta's hand-off to member 2

Probe: the same logs, plus each raft message an owner sends and receives through the hand-off, the leader's progress
for the target before and after the call, the core's hand-off decision, and each member reported unreachable.

| step | branch (probe 8bb5582c, r62; r45, r54) | main (probe 34070c78, r66) |
|---|---|---|
| member 3's close for its call | arrives at t=20000, after its slot ended by time (`Expired`, held 10000 ms) | arrives at t=0, ends its slot `Released` |
| before `transfer(2)` | leader's progress for 2: matched 6 of last 6, Replicate | matched 5 of last 5, Replicate |
| the core's decision | sends `MsgTimeoutNow` (matched == last) | the same |
| the leader's outgoing queue | queues `MsgTimeoutNow` to 2, heartbeat to 2, heartbeat to 3; then sheds a record for 2 (the run's only shed) and reports 2 unreachable (progress Replicate to Probe) | queues the same three; no shed in the whole run |
| member 2 | never receives `MsgTimeoutNow` (r45, r62); its term stays 3 for 60000 rounds | receives it in round 1, campaigns, leads term 4 by round 8 |

The shed (QB.2, the owner's send loop) drops the oldest queued record when a new one would pass the queue's bound,
"raft sends again what a member still lacks, as over a lossy link". The log names the shed record's member, not its
kind; that it was the order is inferred from member 2 never receiving one. The core never sends a `MsgTimeoutNow`
again, and with every clock standing still the transfer never runs out its election timeout, so the lead never
moves.

The earlier divergence in the same run: each follower's close for its call reaches the leader only after the slot
ended by time (at t=5000, 10000 and 20000), where main's close releases the slot at t=0. Not traced: how the late
closes, or the leader's clock moved 10 s at each, leave the outgoing queue past its bound at the hand-off.

## Next, in order

1. Log the outgoing queue's bound, count and bytes at each push from the first late close on, on both arms, and the
   clock each was learned at. Find why the branch's queue is past its bound at the hand-off.
2. Trace why each close arrives after its slot ended by time on the branch: log each owner's clock moves and what
   moves them (`Driven::advance` and its callers) from the call's answer to the close's arrival, on both arms. One
   candidate, unverified: on the branch `decide` answers the call and queues the close after the step's drive, and
   `end_slot` sets no `driving` where `propose_stamped` does.
3. For V13, run lane V13's real-time bench (r1814) on the branch and on main. If the branch answers every write with
   no stale base on real time, the failure is the harness's moving time. Then the harness would treat a session out
   on the worker as main's inline one ("Decision (lane P6)": the worker moves where a session runs, not when). That
   changes a test, so it is Ada's call under rule 0.
4. Side finding, not acted on: QB.2's shed premise does not hold for `MsgTimeoutNow`, a one-shot order. Under a shed
   on main too, a hand-off stalls until its election timeout.

## Runs

All through kiln `run`, `on` adacachy2 (00aad8da...), CARGO_INCREMENTAL=0, debug test builds in P7's target dir
(~/.cache/amphora-perf-target/serve-p7), the remote clone ~/.cache/amphora-perf/serve-p7-remote checked out at each
probe's SHA, each test alone with `--nocapture`. Logs on adacachy2: ~/.cache/amphora-perf/serve-p19-r2 to -r7.

| run | needs | tree | result |
|---|---|---|---|
| r15 | cpus=4,mem=12g, jobs 4 | fd8be249 (branch + logs) | built 39 s; both fail as above |
| r33 | cpus=1,mem=6g, jobs 1 | 816f78e1 (main d88899f6 + logs) | built 41 s; both pass |
| r45 | cpus=1,mem=6g | 1cae950a (+ messages in and out) | seed fails; no `MsgTimeoutNow` leaves the leader |
| r54 | cpus=1,mem=6g | 575ff91d (+ leader's progress) | matched == last before; Probe after |
| r62 | cpus=1,mem=6g | 8bb5582c (+ core decision, unreachable) | `MsgTimeoutNow` queued, then shed |
| r66 | cpus=1,mem=6g | 34070c78 (main + the same) | the same three records, no shed; passes |

Probe branches, never to merge: `perf/serve-p19-probe` (8bb5582c, on the branch head) and
`perf/serve-p19-probe-main` (34070c78, on main d88899f6). Its last commit puts `eprintln!` in vendored hyper-raft.
