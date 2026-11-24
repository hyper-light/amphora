# Lane P15 (serve): t_t1_10_r3 traced to a log restart the branch defers past a lent session; not fixed, not merged (2026-11-23, adacachy2)

Goal: every replicated write of any size commits or is refused typed, and no long write costs the group its leader
(CP-T1.13). This hour's deliverable: perf/serve-p7 merged to main, done when clippy passes at 82eed2e0;
`t_t1_10_r3_a_session_reads_its_writes_across_leader_changes` is traced with the probe to the first place the branch
decides differently from main, fixed there, and passes 6 of 6; main is merged in and 3 alternated rounds of run2.sh show
no test main passes failing on the branch, both cp_t1_13 tests passing; rustfmt is clean on replicated.rs and
session_worker.rs.

All runs through kiln `run`, `on` adacachy2 (00aad8da...), needs `linux/amd64,cpus=4,mem=16g`, CARGO_INCREMENTAL=0,
CARGO_BUILD_JOBS=4, debug test builds in P7's target dir (~/.cache/amphora-perf-target/serve-p7), remote clone
~/.cache/amphora-perf/serve-p7-remote at fixed SHAs, restored after each probe build. Scripts on adacachy:
~/.cache/amphora-perf/serve-p15-scripts/ (r1.sh to r4.sh, probe15.py, probe16.py, probemain.py). Logs on adacachy2:
~/.cache/amphora-perf/serve-p15-r1/ to serve-p15-r4/. The branch's lib test binary at 82eed2e0 is kept under
~/.cache/amphora-perf-bin/serve-p15/82eed2e0.../lib.

## Clippy at 82eed2e0 (r1703)

`cargo clippy -p amphora --all-targets --locked -- -D warnings`: exit 0. The warnings in its log are vendored hyper-rt's
`fetch_update` deprecations.

## The failure

P14's run 2 (r1662) rounds 1 and 2, and r1703 round 1: `session_reads.rs:232:49`, `called Result::unwrap() on an Err
value: Timeout`. That is the "at once" token read of a member whose published applied index covers the session's token.
`HANG_READ` is 60 s, so a failing suite runs about 90 s instead of about 50 s.

| run | arm | t_t1_10_r3 | suite time | load (1 min) |
|---|---|---|---|---|
| r1703, alone | 82eed2e0 | 6 of 6 pass (0 to 4 s) | | 68-71 |
| r1703, suite 4 threads | 82eed2e0 | 1 of 2 fail | 97 s fail, 64 s pass | 74-75 |
| r1720, suite, probe15 | 82eed2e0 + probe | 0 of 4 fail | 55 to 93 s | 33-54 |
| r1734, suite, probe16 | 82eed2e0 + probe | round 1 fail (stopped there) | 90 s | 44 |
| r1744, suite, probemain | main 98bd0fa9 + probe | 2 of 2 pass (forced to fail at its end) | 53 s | 36-45 |

Other failures the probe builds showed (branch): r1720 round 1
`a_read_on_a_wholly_copied_log_holds_passes_off_and_a_busy_pass_holds_reads` ("a pass copied beside the read",
`Checkpointed { log: 122, left: 117 }`); round 4 `a_raise_beside_a_change_holds_through_a_lost_lead` and
`session_worker::cp_t1_13_a_write_whose_lead_moved_while_its_session_ran_is_refused_not_leader`; r1734 round 1
`v3_two_replicated_writes_in_a_row_answer_with_every_log_flush_held`. The cp_t1_13 one's trace is the same wait as
below: its local read sat 60 s in the reader pool with its replica's reads held for a restart.

## The probe (not for main)

probe15.py prints each wait in the token read path: `wait_token`'s wait and timeout, `read_local`'s refusal, a read that
found an earlier time, and `Local::with` waiting for a reader (with `held`, the replica's reads held for a restart). The
test prints each "at once" read's owner, token and status. probe16.py adds one line per owner step where a restart's
hold is due (`hold_due`), with `lent` (a session out on the worker), the last pass's `caught_up` and `busy`, `past`,
`due`, a flush `out`, and the applied index. probemain.py is probe16's flush line on main (no `lent` there), with the
test made to fail at its end so its captured lines print.

The branch, r1734, the failing round (leader node 3, term 3, the third write, applied 6 to 7):

    26  flush id 3 t3 hold_due lent true  caught_up true  busy false past false due true out false applied 6
    27  flush id 3 t3 hold_due lent true  caught_up true  busy false past false due true out true  applied 6 (x2)
    30  flush id 3 t3 hold_due lent true  caught_up false busy true  past false due true out false applied 6
        ... x88 steps, lent true, busy true, through 117
    118 flush id 3 t3 hold_due lent false caught_up false busy true  past false due true out false applied 6
        ... busy true through 129, where applied 7: the write's answer goes out with node 3's reads held
    132 test at-once read owner 2 leader 2 value 3 session Token { ..., index: 7 } status ... node_id: 3, applied: 7
    133 with waits replica 497 held true own_idle None out 0 ... left 59.999995011s
        ... read_local refused Timeout token Token { ..., index: 7 } after 60.003695835s

The second write (leader node 2, term 2) shows the same steps (13: `lent true caught_up true busy false`; 15 to 21:
`busy true`, the answer at 21). At 22 the restart's condition held (caught up, no flush out, not lent), one step after
the answer and before the test's read, which was served.

Main, r1744, both rounds: every `hold_due` step on every member, every write, is `caught_up true busy false out false`.
The hold and the restart happen in the same step, and no read waited (`with waits` 0).

## The first place the branch decides differently from main

`Actor::flush`, in the step that hands a caller's session to the worker. In one step `release` takes the barrier's read
state and calls `prepare`, which lends the write lock (`hand_off`); `flush` runs after it. Main runs the session inline
in `prepare`, so that step's `flush` finds a restart due and the last pass caught up, and restarts the log. The branch's
`flush` finds `lent` and skips the restart (`due && pass.caught_up && !*out && !lent`). Every pass during the session
then found the log busy (88 steps, `busy true`). `Pass::busy` is a reader on read-mark 0 holding passes off (wal.c's
`WAL_READ_LOCK(0)`); the session's connection, begun on a wholly copied log, is the likely reader (inferred, not
probed). Once the session is back, `hold_due` holds the replica's reads for a restart that waits on
a pass that has caught up, and the write can be proposed, committed, applied and answered first. In the driven harness
the test's read then blocks the only thread that steps the owners, so the restart never runs and the read times out.
In a real deployment the owner steps on its own, so the read would wait for a pass and a restart, not hang (inferred,
not run).

Not explained yet: passes stayed busy for 11 steps after the session came back (118 to 128), so something still held
read-mark 0 after the session's transaction ended.

## Not done (losses first)

- perf/serve-p7 is NOT merged to main.
- No fix. Two shapes the trace points at, neither built nor tested: (a) in the step that lends the write lock, run a
  restart that is due and ready before `Sessions::hand`, as main's step does after its inline session; (b) run `flush`
  before `release` in `run_step`. Either needs a test that fails first (Amphora CLAUDE.md section 4) and the 6 rounds.
  Neither covers a restart that comes due while a session is out; check that case too.
- Main is not merged in. Main moved 70 commits past 98bd0fa9, including lane V11 (ad4c0922, f05b2c05: a read's barrier
  and the close of a write that forwarded nothing carry a `READ_BARRIER` mark, and `Slots::pass` steps them without
  asking for a slot). A trial merge conflicts in slot.rs (1 hunk: P14's `read` against V11's `pass` and `released`),
  replicated.rs (4 hunks: `prepare` and `end_slot`, `run_session` against V11's inline session with `Operation::Query`,
  and `settle` against V11's `!writes` refusal) and replication.md (1). P14's `ask` auto-merges. Read, not run: under
  V11, `pass` clears `Gap::from` only for a slot still held, so after an expiry P14's `ask` would let the member's next
  write barrier step without a slot once. The slot core's tests should settle that after the merge.
- No alternated rounds; rustfmt not run (it belongs after the merge, since V11 changed replicated.rs).

## Commands

    # r1703: clippy and the failure rate at 82eed2e0
    cargo clippy -p amphora --all-targets --locked -- -D warnings
    lib t_t1_10_r3_a_session_reads_its_writes_across_leader_changes --test-threads=1          # 6x
    lib database::replicated:: replication:: --test-threads=4                                  # 2x
    # r1720, r1734: python3 -I probe15.py <tree> (and probe16.py <tree> probe15.py) on 82eed2e0, the same suite
    # r1744: python3 -I probemain.py <tree> probe15.py on 98bd0fa9, the same suite 2x
