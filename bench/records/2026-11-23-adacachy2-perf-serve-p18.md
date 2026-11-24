# Lane P18 (serve): not landed; main's V13 test fails only on the branch, at followers (2026-11-24 UTC, adacachy2)

Goal: every replicated write either commits or gets a typed refusal, and no long write costs the group its leader
(CP-T1.13). This hour's deliverable: perf/serve-p7 on main, done when current main is merged in keeping both sides,
clippy passes, one round of `database::replicated:: replication::` and the serve tests on the merged tree shows no
failure outside main's known flaky set (`every_falsifier_of_the_floor_holds_under_a_nemesis`, `t_t1_10_r1`,
`t_t1_9_r18`), both cp_t1_13 tests pass, rustfmt is clean on the files touched, and the push to main follows rule 6.

All runs through kiln `run`, `on` adacachy2 (00aad8da...), needs `linux/amd64,cpus=4,mem=16g`, CARGO_INCREMENTAL=0,
CARGO_BUILD_JOBS=4, debug test builds in P7's target dir, remote clone ~/.cache/amphora-perf/serve-p7-remote at fixed
SHAs. Scripts on adacachy: ~/.cache/amphora-perf/serve-p18-scripts/ (r1.sh to r4.sh). Logs on adacachy2:
~/.cache/amphora-perf/serve-p18-r1 to -r4. Kept binaries on adacachy2: ~/.cache/amphora-perf-bin/serve-p18/.

## Commits on perf/serve-p7

| SHA | What |
|---|---|
| c7e4a109 | Merge origin/main 57fd89cd. No conflicts. Says the floor test is outside the parity bar, and why. |
| fc5b6e25 | Merge origin/main d88899f6. V14's CP-T1.8 W1 change conflicted in `Slots::read` and `Actor::settle`; both sides kept. |
| cdaac483 | The branch's two slot tests (lanes P14, P16) pass V14's new `stated` argument as `None`; fc5b6e25 did not compile. |

## r1990: the merge c7e4a109 (perf/serve-p7 1ec614e9 + main 57fd89cd)

| check | result |
|---|---|
| build (lib, serve) | 83 s, load 70 |
| lib `database::replicated:: replication::` --test-threads=4 | 173 passed, 2 failed, 61 s, load 78 |
| failed, inside the known set | `every_falsifier_of_the_floor_holds_under_a_nemesis` (the same stall P17 saw: leader 2 at last 37, committed 36, `changing true`) |
| failed, outside the set | `v13_thirty_two_spread_writers_on_moving_time_are_each_answered_with_no_stale_base` (main's, lane V13, CP-T1.8) |
| serve, whole binary, --test-threads=2 | 11 of 11 ok, 13 s; both cp_t1_13 tests ok |
| clippy -p amphora -p amphora-api --all-targets | exit 0, 28 s |
| V13's test alone, 3 runs each | branch 3 of 3 FAILED (stale-base refusals: 1 persisted, 2 replicated); main 57fd89cd 3 of 3 ok |

The V13 test answers every row, then asserts no write was refused stale (`Refusal::Serialization`, the base fence in
replication.rs `on_base`). Main refuses none. The branch refuses 1 to 2 a run.

## r9: what the stale refusals are

The probe 086e2cb0 (c7e4a109 with two `#[cfg(test)]` logs, branch `perf/serve-p18-probe`) logged each forward the
leader stepped while its own session was out on the worker, and each stale answer with the member that gave it.

| what | count |
|---|---|
| forwards stepped while the leader's own session was out | 0 |
| stale answers | 18, at member 2 (9) and member 3 (9); none at the leader, none owner-authored |

So the stale refusals are followers' forwarded writes. I had guessed they were the leader's own write, refused because
a forward stepped while it was out on the worker, and committed a fix for that (01ab1ec7: hold forwards while the
leader's own write is out). The probe shows the premise false, and with the fix the test still failed 3 of 3 (11 and 6
stale refusals in run 3). The fix is not on perf/serve-p7; it stays on `perf/serve-p18-fix` for the record only.

## r30: the branch at cdaac483 (with main d88899f6)

| check | result |
|---|---|
| build (lib, serve) | 46 s, load 69 |
| lib `database::replicated:: database::slot:: replication::` --test-threads=4 | 190 passed, 3 failed, 73 s, load 87 |
| failed, inside the known set | `every_falsifier_of_the_floor_holds_under_a_nemesis` |
| failed, outside the set | `v13_thirty_two_spread_writers_on_moving_time_are_each_answered_with_no_stale_base`: "Persisted: row 25 at member 2: Timeout" (no longer a stale refusal: a follower's write runs out its deadline) |
| failed, outside the set | `seed_and_delta_hold_through_an_election_at_each_step`: "the lead did not move to 2 in 60000 driven rounds" (passed at c7e4a109 in r1990) |
| serve, whole binary | 11 of 11 ok, 13 s; both cp_t1_13 tests ok |
| clippy -p amphora -p amphora-api --all-targets | exit 0, 39 s |
| each failure outside the set alone, 3 runs | branch cdaac483 3 of 3 FAILED for both; main d88899f6 3 of 3 ok for both |

With V14's change merged (a live holder's slot ends by its forward or close, and by time only at its stated deadline),
the branch no longer refuses followers' writes stale in V13's test; one times out instead. And a lead hand-off in
`seed_and_delta_hold_through_an_election_at_each_step` never completes. Both pass on main. Neither is traced.

## rustfmt

slot.rs and replicated.rs (with its test modules) are clean at fc5b6e25 and cdaac483. Main's own slot.rs and
replicated.rs at d88899f6 are not: 3 places each, all in V14's lines; fc5b6e25 formats them, whitespace only. In r1990,
the branch's replication.rs (line 1624) and tests/serve.rs (lines 804, 816) were not clean either; both are earlier
lanes' lines that this lane did not touch, and are left as they are.

## Not done (losses first)

- perf/serve-p7 is NOT on main. Main's V13 test fails on the branch in every run (3 of 3 alone on c7e4a109) and passes
  on main (3 of 3). Its stale-base refusals are followers' forwarded writes (r9). Not traced past that.
- With main d88899f6 merged (cdaac483), two tests fail only on the branch, 3 of 3 alone each, and pass on main 3 of 3:
  V13's test (now a follower's write answered Timeout) and `seed_and_delta_hold_through_an_election_at_each_step` (the
  lead never moves to 2). Neither is traced.
- An hour's fix (01ab1ec7) rested on a guess and was wrong; the probe that refuted it should have come first.

## Next, in order

1. On the probe's binary (or a new probe), log for each follower write refused stale: its base (index, term), the
   entry before it at apply, and who appended that entry and when relative to the follower's slot (begun, ended by
   time or forward, length). Then compare with main's same run. A guess to test first, unverified: the follower's
   session runs on its worker for several rounds after its slot begins, and on moving time the slot ends by time
   before its forward arrives; main prepares inline in one step. V14's change on main (a live holder's slot ends by its
   forward or close, by time only at its stated deadline) may change that; r30 runs the branch with it.
2. Trace `seed_and_delta_hold_through_an_election_at_each_step` on cdaac483 the same way: it passed at c7e4a109 and
   fails with V14's change merged, so the meeting of V14's slot deadline and the branch's session worker is the first
   place to look.
3. Fix at the first place the branch decides differently from main, then the round and the push per rule 6.

## Commands

    # r1990 (r1.sh): c7e4a109 built (lib, serve) and kept; lib database::replicated:: replication:: --test-threads=4;
    #   serve --test-threads=2; clippy -p amphora -p amphora-api --all-targets --locked -- -D warnings; rustfmt --check;
    #   the failure outside the set alone 3x on the head and on main 57fd89cd (built then)
    # r9 (r2.sh): probe 086e2cb0 lib, V13's test once --nocapture; fix 01ab1ec7 lib, V13's test alone 3x (stopped after)
    # r30 (r4.sh): cdaac483 as r1.sh, the suite filter with database::slot:: added, main d88899f6 for reruns
