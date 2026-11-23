# Lane P13 (serve): the branch not merged; its six failures traced on the branch to a slot begun for a stray barrier (2026-11-23, adacachy2)

Goal: every replicated write of any size commits or is refused typed, and no long write costs the group its leader
(CP-T1.13). This hour's deliverable: perf/serve-p7 merged to main once, over 3 alternated runs each, its
`database::replicated:: replication::` suite matches main's pass set and both cp_t1_13 serve tests pass.

All runs through kiln `run`, `on` adacachy2 (00aad8da...), needs `linux/amd64,cpus=4,mem=16g`, CARGO_INCREMENTAL=0,
CARGO_BUILD_JOBS=4, debug test builds in P7's target dir (~/.cache/amphora-perf-target/serve-p7), remote clone
~/.cache/amphora-perf/serve-p7-remote at fixed SHAs. Each arm's lib and serve test binaries are kept under
~/.cache/amphora-perf-bin/serve-p13/<sha>. Script on adacachy: ~/.cache/amphora-perf/serve-p13-scripts/run1.sh (and
probe.py, the probe's edits). Logs on adacachy2: ~/.cache/amphora-perf/serve-p13-r1/ and serve-p13-probe/. Runs: r1576
(the arms, the suite and serve 3x, clippy), the probe run at 21:52Z (kiln gave it no id).

## Not done (losses first)

- perf/serve-p7 is NOT merged to main. Its head 94d55d98 is the branch merged with main b85ca877 (lane V9); clippy
  `-p amphora --all-targets` exit 0 there (r1576). Seven tests pass on main in 3 of 3 runs and fail on the branch in
  3 of 3: P12's six (`a_call_whose_work_outlasts_its_deadline_is_refused_and_never_proposed`,
  `a_forward_that_arrives_mid_election_is_refused_no_leader_and_answers_unknown`,
  `a_follower_cut_after_answering_on_the_commit_loses_nothing_acknowledged`,
  `a_leader_whose_followers_answer_as_late_as_it_measured_keeps_its_lead`,
  `autonomous_quorum_seed_preserves_unchanged_pages_then_delta_and_restart`,
  `seed_and_delta_hold_through_an_election_at_each_step`) and `every_falsifier_of_the_floor_holds_under_a_nemesis`.
  V9's merge (five tests with the slot off) changed none of the six.
- No fix. The first divergence is traced on both arms (below), at 21:59Z; no time was left to fix and verify it (a
  19-minute worktree checkout on adacachy and an API outage at about 21:30Z took the hour's middle).
- Main still fails `cp_t1_13_sql_takes_bound_parameters_and_answers_typed_rows_with_their_names` (3 of 3); the branch
  passes it (3 of 3). Main fails `writers_through_five_leader_crashes_each_land_and_every_acknowledged_row_is_held`
  (3 of 3 here); the branch passes it (3 of 3).

## The suite and the serve tests, alternated (r1576)

`database::replicated:: replication::` with 4 threads, then `--test serve cp_t1_13` with 2; arm order main, branch in
rounds 1 and 3, branch, main in round 2. Main b85ca877, branch 94d55d98. P12's six alone, 4 threads: branch 0/6
(4.9 s), main 6/6.

| round | main: suite | branch: suite | main: serve | branch: serve | load (1 min) |
|---|---|---|---|---|---|
| 1 | 168/1 (42.9 s) | 164/7 (51.6 s) | 2/1 (sql) | 4/0 | 18-27 |
| 2 | 168/1 (49.5 s) | 164/7 (50.7 s) | 2/1 (sql) | 4/0 | 25-33 |
| 3 | 168/1 (44.6 s) | 162/9 (100.7 s) | 2/1 (sql) | 4/0 | 30-54 |

Main's one failure each round: `writers_through_five_leader_crashes_each_land_and_every_acknowledged_row_is_held`.
The branch's: the seven above each round, plus `a_raise_beside_a_change_holds_through_a_lost_lead` and
`session_reads::t_t1_10_r3_a_session_reads_its_writes_across_leader_changes` in round 3 (load 50-54). CPU pressure
"some" avg10 3.7-34.9%, I/O 0.1-6.1%.

## The six: one stuck state

All six panic at the driven harness's `answer` (replicated.rs:7148): an operation unanswered after 60000 rounds, the
group quiet (pump records 0, nothing in flight), the leader holding a write slot (200 ms left, or 0 ns), one
follower's write at its barrier (`pending barrier`).

## The trace (probe at f3819ee9, branch diag/serve-p13-probe, not for main)

Probes print each slot decision: a barrier at the leader, a slot begun or ended, a session handed off and decided, a
follower's ending barrier. `a_call_whose_work_outlasts_its_deadline_is_refused_and_never_proposed` alone, after the
leader's three calls:

    P13 L1 readindex from 2 step false slots[holder None queue [] waiting [2] ...]
    P13 L1 slot began member 2 length 200ms last 3 commit 3 own false slots[holder Some((2, 3)) ... ready 1]
    P13 M2 hand_off leader false
    P13 M2 decide ok true entry false pending true
    P13 M2 end_slot pending false leader false sends true
    P13 L1 slot ended member 2 how Expired
    P13 L1 readindex from 2 step false slots[holder None queue [] waiting [2] ...]
    P13 L1 slot began member 2 length 200ms last 3 commit 3 own false slots[holder Some((2, 3)) ... ready 1]
    P13 L1 readindex from 3 step false slots[holder Some((2, 3)) queue [] waiting [3] ...]

Member 2's read takes a slot, comes back from the worker with no entry, and its owner queues the ending barrier
(`end_slot`, in `decide`, after the step's drive). Before that barrier reaches the leader, the slot ends Expired: the
harness's `lapse_slots` found the group quiet and moved the leader's clock past the slot's end (inferred from the
order above and its quiet rule, which counts no message an owner's core holds unsent). The ending barrier then
arrives at a leader where member 2 no longer holds a slot, so `Slots::ask` (lane V8) queues it as a new write's
barrier and a second slot begins for member 2, which has no work and never ends it. Member 3's barrier waits behind
it (`waiting [3]`), and with member 3's work in hand the harness never moves a clock again. In real time that slot
would end by its length, so each such stray barrier would hold the leader's writes and every other follower's for a
slot length (inferred, not measured).

Main b85ca877, the same probes (probe_main.py; main's ending barrier is sent from `prepare()`), the same test, 21:59Z,
passed:

    P13 L1 readindex from 2 step false slots[holder None queue [] waiting [2] ...]
    P13 L1 slot began member 2 length 200ms slots[holder Some((2, 3)) ... ready 1]
    P13 M2 prepare sends its ending barrier
    P13 L1 readindex from 2 step true slots[holder None queue [] waiting [] read None turn true ready 0]
    P13 L1 slot ended member 2 how Released
    P13 L1 readindex from 3 step false slots[holder None queue [] waiting [3] ...]
    P13 L1 slot began member 3 length 200ms ...
    P13 M3 prepare sends its ending barrier

The first divergence, measured on both arms: member 2's ending barrier reaches the leader while member 2 holds its
slot on main (ended Released), and after that slot ended Expired on the branch, where it begins a second slot. Why the
branch's clock moved first is inferred, not shown: its session's result is decided a step after the barrier's answer,
and `lapse_slots` counts the group quiet while the ending barrier is still unsent in member 2's core. Lane V9's part 2
design (a barrier whose work writes nothing is marked, and a marked barrier never asks for a slot) would keep a late
ending barrier from beginning a slot; not tested.

## Next

Decide which side is wrong where the two differ: the product (a late ending barrier begins a slot no work will end:
legitimate in real time, where a long session outlasts its slot) or the harness (it moves a clock while an owner holds
an unsent record). Fix there, then rerun r1576's script on the fixed head.

## Commands

    # r1576: ~/.cache/amphora-perf/serve-p13-scripts/run1.sh, shipped base64 in the run's command
    cargo test -p amphora --locked --lib --no-run; cargo test -p amphora --locked --test serve --no-run   # per arm
    lib <six names> --test-threads=4                                                                # per arm
    lib database::replicated:: replication:: --test-threads=4; serve cp_t1_13 --test-threads=2       # 3 rounds
    cargo clippy -p amphora --all-targets --locked -- -D warnings                                    # at 94d55d98: exit 0
    # probe, 21:52Z: f3819ee9 (python3 -I probe.py <tree> on 94d55d98)
    lib a_call_whose_work_outlasts_its_deadline_is_refused_and_never_proposed --test-threads=1 --nocapture
