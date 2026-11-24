# Lane P12 (serve): pages on main, the slot test fixed in the product, the branch not merged (2026-11-23, adacachy2)

Goal: every replicated write of any size commits or is refused typed, and no long write costs the group its leader
(CP-T1.13). This hour's deliverable: perf/serve-p7 merged to main once its replicated suite matches main's pass set
and the cp_t1_13 serve tests pass, over 3 alternated runs of each.

All runs through kiln `run`, `on` adacachy2 (00aad8da...), needs `linux/amd64,cpus=4,mem=16g`, CARGO_INCREMENTAL=0,
CARGO_BUILD_JOBS=4, debug test builds in P7's target dir, remote clone ~/.cache/amphora-perf/serve-p7-remote at fixed
SHAs. Each arm's lib and serve test binaries are kept under ~/.cache/amphora-perf-bin/serve-p12/<sha> and run from
crates/amphora, main then branch, alternated in one run, so both arms see the same load. Scripts on adacachy:
~/.cache/amphora-perf/serve-p12-scripts (run1.sh, run2.sh, clip.sh, probe.py, probe-test.rs). Logs on adacachy2:
~/.cache/amphora-perf/serve-p12-*.log. Runs: r1494 (the bases at the hour's start), r1518 (the bases after lane V8),
r1531 (clippy at the final merge), r1537 (the merge without the slot fix).

## Not done (losses first)

- perf/serve-p7 is NOT merged to main. Its head f654e20c is the branch merged with main feb9f1f2; clippy exit 0
  there (r1531). Six replicated tests pass on main in every run and fail on the branch in every run:
  `a_call_whose_work_outlasts_its_deadline_is_refused_and_never_proposed`,
  `a_forward_that_arrives_mid_election_is_refused_no_leader_and_answers_unknown`,
  `a_follower_cut_after_answering_on_the_commit_loses_nothing_acknowledged`,
  `a_leader_whose_followers_answer_as_late_as_it_measured_keeps_its_lead`,
  `autonomous_quorum_seed_preserves_unchanged_pages_then_delta_and_restart`,
  `seed_and_delta_hold_through_an_election_at_each_step`.
  They come from merging lane V8 (8600350e, landed on main during this hour) into the branch, not from this lane's
  slot fix: at 05224ef8 (the branch merged with V8's main, without the fix) each fails alone, 6 of 6 (r1537). Before
  V8, the branch's merge (10d19e4e) failed none of them in 3 suite runs (r1494). Not traced this hour.
- Main's own replicated suite broke with V8. Before it, main with pages (11725d28) ran 144 passed, 0 failed, in 3 of
  3 runs, about 45 s each, at load 32-35 (r1494). After it (e931b6dd): 138/7, 139/6 and 140/5, about 145 s each, at load
  10-40 (r1518). Failing on main: `a_floor_session_waits_for_its_log_as_it_was_not_for_writes_behind_it`,
  `an_overlapping_write_is_refused_serialization_and_a_malformed_one_invalid`,
  `follower_origin_captures_hold_through_an_election_at_each_step`,
  `follower_origin_captures_once_and_reads_after_local_durable_apply`,
  `writers_through_five_leader_crashes_each_land_and_every_acknowledged_row_is_held`,
  `a_write_at_a_follower_is_answered_on_its_groups_commit` (runs 1 and 2; run 3 failed 5, the first four and the last of these); `every_falsifier_of_the_floor_holds_under_a_nemesis`
  (run 1). V8's owner or the coordinator should know; this lane did not touch it.
- Main fails `cp_t1_13_sql_takes_bound_parameters_and_answers_typed_rows_with_their_names` in every serve run (r1494
  and r1518); the branch passes it in every run. Not traced.
- Part 2, the apply half and the latency runs: not started (the next lane's, per the brief).

## Step 1: a write's page images as byte strings, on main

a52a337a (lane P11) merged onto main three times as main moved: 11725d28 (clippy exit 0, `--lib replication::` 24
passed with `pages_encode_as_before_and_decode_across`, r1494), e931b6dd (the same, r1518), then feb9f1f2, pushed
20:22Z. The last merge took only lane A13's commits, none of them in replication.rs, the one file the change touches;
feb9f1f2's diff against the main before it is replication.rs alone. No A/B (the brief).

## Step 2: the slot test, by P11's real-clock scenario

P11's probe (`p11_slot_scenario_on_a_running_clock`: the slot test's writes with every owner's clock advanced each
round), r1494:

| tree | holder | leader | member 2 | lapses | rounds |
|---|---|---|---|---|---|
| branch, 00122209 | refused (no such table) | Ok | Serialization, "the database advanced since preparation" | 0 | 19 |
| main, fd3deabc | refused (no such table) | Ok | Ok | 0 | 14 |

Only main commits member 2's write on a running clock, so the product is fixed. The first place the branch decides
differently: `Actor::start_slot` counted the leader's own write only in `Phase::Barrier`. On the branch a caller's
write waits on the session worker in `Phase::Running`, so with the leader's turn owed after the holder's slot,
`Slots::start` took the turn and began member 2's slot; the leader's write came back and was appended inside that
slot, ahead of member 2's, whose base it passed. On main the leader prepares and proposes its write in one step, so
no slot can begin between its barrier and its append. f5d1208b: no slot begins while the leader's own write is out on
the worker (replication.md, "the write slot": the leader appends nothing of its own while a slot is held; "Decision
(lane P6)": the worker moves where a session runs, not when).

The slot test, alone, alternated, load 22-23 (r1518): main e931b6dd 5 of 5, branch f5d1208b 5 of 5. At 05224ef8,
without the fix, it fails (r1537). In the suite it passes on the branch in every run below.

Lane V8 removed the follower's `slotted` term on main; the branch's `end_slot`, one rule for `prepare` and for a
session back from the worker, takes V8's condition in the merge (05224ef8).

## Step 3: the two tests that failed once each

Each alone, alternated main then branch, 5 runs each:

| test | r1494 (main 11725d28 / branch 10d19e4e, load 39-46) | r1518 (main e931b6dd / branch f5d1208b, load 22-23) |
|---|---|---|
| `a_raise_beside_a_change_holds_through_a_lost_lead` | main 5/5, branch 4/5 (run 3 failed) | main 5/5, branch 5/5 |
| `cp_t1_13_a_leader_request_made_during_an_election_is_answered_once_it_is_chosen` | 5/5, 5/5 | 5/5, 5/5 |

The election test never failed (20 runs). The lost-lead test failed once in 10 branch runs, on the merge before the
slot fix and before V8, never on main; not given step 2's treatment this hour.

## The suite and the serve tests, alternated

`database::replicated` with 4 threads, then `--test serve cp_t1_13` with 2, main then branch, per round:

| run | main: replicated | branch: replicated | main: serve cp_t1_13 | branch: serve cp_t1_13 |
|---|---|---|---|---|
| r1494 1 (11725d28 / 10d19e4e) | 144/0 | 144/2 | 2/1 (sql) | 4/0 |
| r1494 2 | 144/0 | 144/2 | 2/1 (sql) | 4/0 |
| r1494 3 | 144/0 | 143/3 | 2/1 (sql) | 4/0 |
| r1518 1 (e931b6dd / f5d1208b) | 138/7 | 136/11 | 2/1 (sql) | 4/0 |
| r1518 2 | 139/6 | 136/11 | 2/1 (sql) | 4/0 |
| r1518 3 | 140/5 | 136/11 | 2/1 (sql) | 4/0 |

r1494's branch failures: the slot test (3 of 3), `a_refused_follower_write_lands_on_its_next_try_under_the_slot` and
`no_writer_is_refused_twice_in_a_row_when_every_member_writes` (2 of 3 each). r1518's branch failures: main's six or
seven above (but `follower_origin_captures_once_and_reads_after_local_durable_apply` and
`writers_through_five_leader_crashes_each_land_and_every_acknowledged_row_is_held`, which pass on the branch), plus
the six branch-only tests. Main has 3 cp_t1_13 serve tests, the branch 4 (its
`cp_t1_13_a_replicated_write_past_an_election_timeout_commits_and_keeps_its_lead` among them).

Loads at r1518's suite runs: 14-23 (runs 1 and 2), 29-40 (run 3). Across r1518's three runs, passing on main in at
least one run and on the branch in none: the six branch-only tests and `every_falsifier_of_the_floor_holds_under_a_nemesis`
(main passed it in runs 2 and 3). Main's sql serve failure panics at crates/amphora/tests/serve.rs:759.

P11's real-clock probe on the new bases (r1518, the probe without V8's removed `slotted` lines):

| tree | holder | leader | member 2 | lapses | rounds |
|---|---|---|---|---|---|
| branch with the fix, 1fb87111 (f5d1208b) | refused (no such table) | Ok | Ok | 0 | 54 |
| main, 61edcb17 (e931b6dd) | refused (no such table) | Ok | Ok | 0 | 15 |

Both commit member 2's write on a running clock now; the branch takes 54 driven rounds to main's 15 (one run each,
not explained).

## Next

The six branch-only failures, at f5d1208b or 05224ef8: each one's first divergence from main, the way step 2 found
the slot's. They appeared with V8's every-follower slot (`Slots::ask`) meeting the session worker.
