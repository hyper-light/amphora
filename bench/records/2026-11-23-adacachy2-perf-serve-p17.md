# Lane P17 (serve): the restart test replaced; the raise test fixed; a branch-only stall left; not merged (2026-11-23, adacachy2)

Goal: every replicated write of any size commits or is refused typed, and no long write costs the group its leader
(CP-T1.13). This hour's deliverable: perf/serve-p7 merged to main, done when the head (5c58dfc2 merged with current
main) builds and clippy passes; P16's restart test is replaced by a test of a read made after a write's answer, served
across a session hand-off, that passes and fails on the hang build (47c95d85); 3 alternated rounds against main show no
test main passes failing on the branch and both cp_t1_13 tests passing; rustfmt is clean on replicated.rs,
session_worker.rs and slot.rs.

All runs through kiln `run`, `on` adacachy2 (00aad8da...), needs `linux/amd64,cpus=4,mem=16g` (refused twice at 00:33,
0 and 1 of 32 cores idle; accepted at 00:53), CARGO_INCREMENTAL=0, CARGO_BUILD_JOBS=4, debug test builds in P7's target
dir, remote clone ~/.cache/amphora-perf/serve-p7-remote at fixed SHAs. A macOS run queued meanwhile (r1913) was stopped
before it started: its head was superseded. Scripts on adacachy: ~/.cache/amphora-perf/serve-p17-scripts/ (r2.sh, r3.sh,
p17test.py). Logs on adacachy2: ~/.cache/amphora-perf/serve-p17-r2/, serve-p17-r3/. Kept binaries on adacachy2:
~/.cache/amphora-perf-bin/serve-p17/ (head: ad3c0ba8 lib, serve; hang-p17: 47c95d85 with the new test, lib; main:
8b151ea8 lib, serve; head2: ea57741b lib, serve).

## Commits on perf/serve-p7

| SHA | What |
|---|---|
| 1f34c0e6 | Merge origin/main 8b151ea8: main had moved only in pg-parser since c6f915b1. Clean. |
| ad3c0ba8 | P16's restart test replaced by `t_t1_13_a_read_after_a_writes_answer_is_served_across_a_session_hand_off`. |
| ea57741b | `a_raise_beside_a_change_holds_through_a_lost_lead` holds the append answers until both its entries are proposed. |

## The new test (r1943)

P16's probe (r1849) showed its premise false: the voter's wholly copied log went from 7 frames to 0 between the setup and
the hand-off with no owner restart, so nothing was due there. What differs between the hang build and the head is
`hand_off`: the head runs `flush` and then `Local::unhold_reads` before lending the write lock; 47c95d85 does neither.
A hold on a replica's reads is released only by a restart that runs (`restart_unread`) or by the owner's end, so reads
held for a restart that is no longer due stay held on 47c95d85. The new test makes that state (the voter's reads held,
none due, after 1 is written and the logs settle), writes 2 through a call, which runs on the session worker, and reads
on the voter once the call is answered.

| build | the new test |
|---|---|
| head ad3c0ba8 | ok (0 s) |
| 47c95d85 with the new test patched in (p17test.py), "hang-p17" | FAILED after 61 s: `left: Err(Timeout)`, `right: Ok([1, 2])` |

The kept binary of 47c95d85 (P16's) predates the test, so the fail-first build is 47c95d85's tree with the test applied
by the same script that wrote it on the branch.

## The rounds against main (r1943: ad3c0ba8 against main 8b151ea8, alternated)

| round | main suite | branch suite | t_t1_10_r3 main / branch | main serve cp_t1_13 | branch serve cp_t1_13 |
|---|---|---|---|---|---|
| 1 (M, B) | 171 ok, 0 failed, 44 s, load 23 | 174 ok, 0 failed, 46 s, load 29 | ok / ok | sql FAILED (2 of 3) | 4 of 4 ok |
| 2 (B, M) | 171 ok, 0 failed, 47 s, load 28 | 173 ok, 1 failed, 44 s, load 25 | ok / ok | sql FAILED (2 of 3) | 4 of 4 ok |
| 3 (M, B) | 171 ok, 0 failed, 47 s, load 30 | 174 ok, 0 failed, 48 s, load 29 | ok / ok | sql FAILED (2 of 3) | 4 of 4 ok |

Main passes and the branch fails: `a_raise_beside_a_change_holds_through_a_lost_lead` (round 2), panicking at its setup
with "the change was never proposed". Clippy at ad3c0ba8: exit 0 (25 s; the warnings are vendored hyper-rt's
`fetch_update` deprecations).

## The raise test: where the branch decides differently from main

The test asks the leader to add learner 5, then submits a raise (an application's call), and rounds until the change
is pending and the raise is proposed at once, to lose the lead between them. Main prepares the raise inline in the step
that takes it, so both stand together in that step. On the branch the raise runs on the session worker (`hand_off`),
the leader applies nothing while it is out (`hold_apply`), and in the step it comes back the drive applies what
committed before `decide` proposes it. When the followers' answers to the change arrive before the session is back,
the change applies first and the two never stand together: 60,000 rounds, then the assert. Read from the code and the
panic, not probed. The product's expectations (the raise applied exactly when it precedes the change, never answered
as raised past it) hold in either order; what fails is the setup's assumption of main's inline preparation. P15 saw the
same failure once (r1720 round 4).

The fix is in the test (ea57741b): `Pump::hold_durable` from before the change is asked until both are in the leader's
log, so neither commits before the raise is proposed; `let_durable` before the lead is handed off. Its expectations
are unchanged.

## The rounds after the fix (r1962: ea57741b against main 8b151ea8, alternated)

The raise test alone at ea57741b: 10 of 10 passed. The new test: ok.

| round | main suite | branch suite | t_t1_10_r3 main / branch | main serve cp_t1_13 | branch serve cp_t1_13 |
|---|---|---|---|---|---|
| 1 (M, B) | 171 ok, 0 failed, 45 s, load 25 | 173 ok, 1 failed, 46 s, load 26 | ok / ok | sql FAILED (2 of 3) | 4 of 4 ok |
| 2 (B, M) | 171 ok, 0 failed, 46 s, load 26 | 174 ok, 0 failed, 46 s, load 24 | ok / ok | sql FAILED (2 of 3) | 4 of 4 ok |
| 3 (M, B) | 171 ok, 0 failed, 45 s, load 27 | 174 ok, 0 failed, 43 s, load 26 | ok / ok | sql FAILED (2 of 3) | 4 of 4 ok |

Main passes and the branch fails: `every_falsifier_of_the_floor_holds_under_a_nemesis` (round 1; P16 saw it once in
r1808 too). Clippy at ea57741b: exit 0. Its panic (replicated.rs:7238, `Driven::settle`'s hang guard): "a driven
operation was unanswered after 60000 rounds", with the pump empty (records 0, lost [], held []) and the leader, id 2,
term 4, at `last 37`, `committed 36`, `pending ("proposed", 4, 2, true)`, `changing true`, `flush_out false`,
`in_flight 0`; ids 1, 3 and 4 at last 36. The leader holds entry 37, proposed during a change of members, that no
follower was ever sent, and nothing moved for 60,000 rounds. Not traced: where the branch first decides differently from
main here is not yet found.

## rustfmt

`rustfmt --check --edition 2024 --config-path rustfmt.toml` on replicated.rs (with its test modules, session_worker.rs
among them), session_worker.rs and slot.rs: clean at ad3c0ba8 and at ea57741b.

## Not done (losses first)

- perf/serve-p7 is NOT merged to main. In r1962 round 1 `every_falsifier_of_the_floor_holds_under_a_nemesis` failed on
  the branch and passed on main: the leader's entry 37, proposed while a change of members was in flight, was never
  sent to a follower (above). Untraced.
- The raise test's fix (ea57741b) rests on a trace read from the code and the panic, not a probe: 10 of 10 alone and 3 of
  3 rounds pass after it, against 1 failure in 3 rounds (r1943) and 1 in P15's 4 before.
- The new test's fail-first is shown on 47c95d85's tree with the test patched in, not on P16's kept binary, which
  predates the test.
- The second part P16 named (a restart that comes due while a session is out runs before its answer) is not built; the
  new test covers a hold placed before the hand-off.

## Next, in order

1. Trace `every_falsifier_of_the_floor_holds_under_a_nemesis` (round 1 log: serve-p17-r3/suite-head2-1.log on adacachy2):
   why the leader's entry 37 is never sent with `changing true`, against main's same step; fix at that point.
2. Then the same 3 alternated rounds (r3.sh against the kept main binaries), clippy, and the merge.

## Commands

    # r1943 (r2.sh): head ad3c0ba8, 47c95d85 + p17test.py, main 8b151ea8 built (lib, serve) and kept;
    #   lib t_t1_13_a_read_after_a_writes_answer_is_served_across_a_session_hand_off --test-threads=1 on head and hang-p17;
    #   3 rounds of lib database::replicated:: replication:: --test-threads=4 and serve cp_t1_13 --test-threads=2,
    #   M B / B M / M B; cargo clippy -p amphora --all-targets --locked -- -D warnings
    # r1962 (r3.sh): head ea57741b built and kept; lib a_raise_beside_a_change_holds_through_a_lost_lead --test-threads=1
    #   10x; the new test once; the same 3 rounds against main's kept binaries; clippy
