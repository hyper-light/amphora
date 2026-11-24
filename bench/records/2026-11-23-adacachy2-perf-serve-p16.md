# Lane P16 (serve): main merged in clean; the restart fix regresses the suite; not merged (2026-11-23, adacachy2)

Goal: every replicated write of any size commits or is refused typed, and no long write costs the group its leader
(CP-T1.13). This hour's deliverable: perf/serve-p7 merged to main, done when main is merged in first (V11), a test that
fails first shows a restart due and ready as a session is lent, the product runs that restart before lending (and one
due while the session is out before the answer), 3 alternated rounds show no test main passes failing on the branch
(t_t1_10_r3 included) and both cp_t1_13 tests passing, and rustfmt is clean on replicated.rs and session_worker.rs.

All runs through kiln `run`, `on` adacachy2 (00aad8da...), needs `linux/amd64,cpus=4,mem=16g`, CARGO_INCREMENTAL=0,
CARGO_BUILD_JOBS=4, debug test builds in P7's target dir, remote clone ~/.cache/amphora-perf/serve-p7-remote at fixed
SHAs. Scripts on adacachy: ~/.cache/amphora-perf/serve-p16-scripts/ (r1.sh to r7.sh, probe16.py, nodefer.py). Logs on
adacachy2: ~/.cache/amphora-perf/serve-p16-r1/ to r6/. Kept binaries: ~/.cache/amphora-perf-bin/serve-p16/<sha>/
(main a7568034: lib, serve; 47c95d85: lib; 970b4386: lib, serve).

## Commits on perf/serve-p7

| SHA | What |
|---|---|
| f53e5373 | Merge origin/main a7568034 (113 commits, V11 among them). Both sides kept (below). |
| 47c95d85 | The two failing-first tests, no fix. |
| 970b4386 | The fix and rustfmt. REGRESSES the suite (below). |
| 0f592fbb | Merge origin/main c6f915b1 (main moved in store.rs, the engine's storage, pg-parser). Not built. |

The merge (f53e5373): slot.rs keeps P14's late close (`read` clears `Gap::from`) beside V11's `pass` and `released`.
replicated.rs: `prepare` sends V11's marked close for a write only, through P14's `end_slot` (the leader and term that
gave the slot); V11's `Query` arm moved into `run_session`, which the owner and the session worker share; `settle`
refuses a session that wrote for work that writes nothing (V11), and `decide` closes only for a write. replication.md
keeps P14's late-close paragraph and V11's "Which barriers ask".

## The red run (r1797, 47c95d85)

- `a_late_marked_close_after_its_slot_expired_leaves_the_next_write_its_slot` (slot.rs) FAILED: "the next write stepped
  without a slot". P15's read of the merge holds: under V11 a late close goes to `pass`, which kept `Gap::from`.
- `t_t1_13_a_restart_due_and_ready_runs_before_a_session_is_lent` FAILED: "the session ran lent the write lock with a
  restart due and ready not run". The other 13 slot tests passed.

## The green run (r1808, 970b4386 against main a7568034, alternated)

| round | main suite | branch suite | t_t1_10_r3 main / branch | main serve cp_t1_13 | branch serve cp_t1_13 |
|---|---|---|---|---|---|
| 1 (M, B) | 171 ok, 0 failed, 56 s, load 66 | 166 ok, 8 failed, 66 s, load 68 | ok / ok | sql FAILED | 4 of 4 ok |
| 2 (B, M) | 171 ok, 0 failed, 49 s, load 85 | 165 ok, 9 failed, 63 s, load 71 | ok / ok | sql FAILED | 4 of 4 ok |
| 3 (M, B) | 171 ok, 0 failed, 54 s, load 94 | 167 ok, 7 failed, 56 s, load 90 | ok / ok | sql FAILED | 4 of 4 ok |

Main passes and the branch fails (rounds failing): a_refused_follower_write_lands_on_its_next_try_under_the_slot (3),
no_writer_is_refused_twice_in_a_row_when_every_member_writes (3), r5_read_sets_keep_every_list_append_history_serializable
(3), two_followers_and_a_leader_writing_steadily_all_commit_within_their_deadlines (3),
writers_through_five_leader_crashes_each_land_and_every_acknowledged_row_is_held (2),
v7_a_hundred_and_twenty_eight_spread_writers_are_each_answered_with_no_stale_base (2),
a_hundred_and_twenty_eight_writers_each_land_and_every_acknowledged_row_is_held (2),
every_falsifier_of_the_floor_holds_under_a_nemesis (1), seed_and_delta_hold_through_an_election_at_each_step (1),
todays_rule_refuses_a_follower_twice_in_a_row_where_the_slot_lands_its_retry (1). The new restart test failed in all
3 rounds and 3 of 3 alone. The slot core: 14 of 14 (the new slot test passes). Clippy was stopped unfinished.

## The bisect (r1822): the merge is clean, the fix regresses

47c95d85's kept binary (the merge and the tests, no fix), the suite once: 172 passed, 2 failed (the new restart test,
and t_t1_10_r3: the known hang, 91 s). So the regressions are 970b4386's, not the merge's.

Which part, not yet run (r7 refused twice: adacachy2 had 0 of 32 cores idle). The inference to test first: the
deferral in `Actor::decide` (hold the write while this replica's reads are held for a restart and no read is out)
keeps the leader's own write in `Running`, and no member's slot starts while the leader's write is out on the worker
(P12's f5d1208b), so followers' writes wait past their deadlines. P15 saw passes stay busy 11 steps after a session
came back, which this deferral would now wait on. Guess, not measured.

Why the new restart test stays red with the fix, not yet run: in the state it builds, the owner's restart does not
run at the hand-off even though `hand_off` now steps the flushing first. Candidates, unmeasured: `Db::restart_log`'s
RESTART checkpoint answering Busy (a reader mark held by an idle connection), `due` false, or a read out. probe16.py
prints each at every flush and at the hand-off.

## rustfmt

`rustfmt --check --edition 2024 --config-path rustfmt.toml` on replicated.rs (with its test modules) and
session_worker.rs: clean at 970b4386, after formatting P14's ten hunks and the merge's. slot.rs: clean.

## Not done (losses first)

- perf/serve-p7 is NOT merged to main: 970b4386 fails 6 to 8 tests main passes in every round (7, 8, 6), the new
  restart test besides.
- The restart fix is unproven: its test fails with the fix in place.
- The second part (a restart due while the session is out runs before the answer) is in 970b4386, and is the
  likely cause of the regressions (inferred above).
- 0f592fbb (main c6f915b1 merged in) is unbuilt. Clippy on 970b4386 did not finish.

## Next, in order

1. Run r7.cmd (V1: 970b4386 without the slot change; V2: without the deferral; the probe on the restart test).
2. Keep the slot change if V1 shows it isn't the cause. Fix the restart test or the hand-off by what the probe
   shows. Drop or rework the deferral by V2.

## Commands

    # r1 (r1769): main a7568034 lib and serve test binaries kept
    # r2 (r1797): 47c95d85 built; lib database::slot:: --test-threads=4; lib t_t1_13_a_restart_due... --test-threads=1
    # r3 (r1808): 970b4386 built; slot; the restart test 3x; 3 rounds of
    #   lib database::replicated:: replication:: --test-threads=4 and serve cp_t1_13 --test-threads=2, M B / B M / M B
    # r6 (r1822): 47c95d85's kept lib, database::replicated:: replication:: --test-threads=4 once
