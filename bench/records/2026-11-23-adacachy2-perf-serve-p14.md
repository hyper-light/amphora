# Lane P14 (serve): a slot ends once, and a close goes only to the leader that gave the slot; not merged (2026-11-23, adacachy2)

Goal: every replicated write of any size commits or is refused typed, and no long write costs the group its leader
(CP-T1.13). This hour's deliverable: a late ending barrier never begins a slot (ending is idempotent), with a test;
main merged in; over 3 alternated runs the branch's `database::replicated:: replication::` suite matches main's pass
set or does better and both cp_t1_13 serve tests pass; perf/serve-p7 merged to main.

All runs through kiln `run`, `on` adacachy2 (00aad8da...), needs `linux/amd64,cpus=4,mem=16g`, CARGO_INCREMENTAL=0,
CARGO_BUILD_JOBS=4, debug test builds in P7's target dir (~/.cache/amphora-perf-target/serve-p7), remote clone
~/.cache/amphora-perf/serve-p7-remote at fixed SHAs. Each arm's lib and serve test binaries are kept under
~/.cache/amphora-perf-bin/serve-p14/<sha>. Scripts on adacachy: ~/.cache/amphora-perf/serve-p14-scripts/ (run1.sh,
run2.sh, probe.py, probe1.sh). Logs on adacachy2: ~/.cache/amphora-perf/serve-p14-r1/, serve-p14-r2/, serve-p14-probe/.

## The two fixes

1. `Slots::ask` and `Slots::read` (036ebe32). A slot begun for a member waits for its close until one comes
   (`Gap::from`). A barrier from a member whose slot ended before its close came (its length passed, or a release) is
   that close: it ends nothing, asks for nothing, and steps. Test (T-T1.8.W4):
   `a_late_second_barrier_after_its_slot_expired_begins_no_slot`, red on the code before it (the late close waited
   for a slot of its own).
2. `Actor::end_slot` (ca10e5c5). A follower sends its close only while the lead is the one its operation's barrier
   went to. A slot dies with its term, and a new leader, which gave none, took the close as an ask. The leader can't
   tell: a follower's ReadIndex carries no term, and its context the sender's current one. Its failing test is
   `seed_and_delta_hold_through_an_election_at_each_step` (red on 036ebe32 in 4 of 4 runs).

Both rules are in replication.md, "A follower's write under steady leader writes: the write slot", "When a slot
starts".

## Run 1 (r1645): main 98bd0fa9 against the branch with fix 1 (036ebe32)

`database::replicated:: replication::` with 4 threads, then `--test serve cp_t1_13` with 2; arm order main, branch
in rounds 1 and 3, branch, main in round 2. P13's seven alone, 4 threads: branch 6/1
(`seed_and_delta_hold_through_an_election_at_each_step`), main 7/0. The slot core's tests on the branch: 12/0.

| round | main: suite | branch: suite | main: serve | branch: serve | load (1 min) |
|---|---|---|---|---|---|
| 1 | 168/1 (48.5 s) | 170/1 (46.7 s) | 2/1 (sql) | 4/0 | 25-29 |
| 2 | 169/0 (45.3 s) | 169/2 (86.5 s) | 2/1 (sql) | 4/0 | 19-23 |
| 3 | 169/0 (43.9 s) | 170/1 (44.7 s) | 2/1 (sql) | 4/0 | 21-23 |

Main's one failure: `every_falsifier_of_the_floor_holds_under_a_nemesis` (round 1). The branch's:
`seed_and_delta_hold_through_an_election_at_each_step` every round, and
`session_reads::t_t1_10_r3_a_session_reads_its_writes_across_leader_changes` in round 2. Before fix 1 (P13's r1576)
the branch failed seven every round. clippy `-p amphora --all-targets` exit 0 at 036ebe32.

## The probe (on 036ebe32, not for main)

P13's probes plus each line's term and the member's `moved` verdict in `decide`. `seed_and_delta_hold_through_an_
election_at_each_step` alone, the end of the trace:

    P14 L1 t5 slot began member 3 length 200ms last 8 commit 8 own false ...
    P14 L1 t6 slot ended member 3 how Released
    P14 M3 t6 lead 2 decide moved true op term 5 op leader 1
    P14 M3 t6 lead 2 end_slot pending false leader false sends true
    P14 L2 t6 slot began member 1 length 200ms last 9 commit 9 ...
    P14 L2 t6 readindex from 3 step false slots[holder Some((1, 9)) queue [] waiting [3] ...]
    P14 L2 t6 slot ended member 1 how Expired
    P14 L2 t6 slot began member 3 length 200ms last 9 commit 9 ...
    P14 L2 t6 readindex from 1 step true slots[holder Some((3, 9)) ...]
    P14 L2 t6 readindex from 1 step false slots[holder Some((3, 9)) queue [] waiting [1] ...]

Member 3's slot, given by leader 1 in term 5, ended with the term. Its session came back in term 6 under leader 2
and sent its close there; leader 2 queued it as an ask and began a slot no work ends, and member 1's next barrier
waited behind it until the harness gave up (60000 rounds). Fix 1 shows in the same trace: member 1's close after
its own slot expired stepped and began nothing (`readindex from 1 step true`).

## Run 2 (r1662): main 98bd0fa9 against the branch with both fixes (ca10e5c5)

P13's seven alone on the branch, 4 threads: 7/0. The slot core's tests: 12/0. Main's binaries are run 1's.

| round | main: suite | branch: suite | main: serve | branch: serve | load (1 min) |
|---|---|---|---|---|---|
| 1 | 169/0 (58.1 s) | 170/1 (90.0 s) | 2/1 (sql) | 4/0 | 30-44 |
| 2 | 169/0 (68.3 s) | 170/1 (89.2 s) | 2/1 (sql) | 4/0 | 41-52 |
| 3 | 167/2 (91.8 s) | 171/0 (47.7 s) | 2/1 (sql) | 4/0 | 58-107 |

The branch's failure in rounds 1 and 2: `session_reads::t_t1_10_r3_a_session_reads_its_writes_across_leader_changes`
(main passed it in all 6 rounds of runs 1 and 2; the branch failed it in 3 of 6). Main's in round 3, at load 104-107:
`every_falsifier_of_the_floor_holds_under_a_nemesis` and `v3_two_replicated_writes_in_a_row_answer_with_every_log_flush_held`,
which the branch passed in all 3. Passing on main in any round and in no round on the branch: none. clippy at ca10e5c5
exit 101 (`bool::then_some`, disallowed by clippy.toml), fixed at 82eed2e0, which no clippy run has checked yet.

## Not done (losses first)

- perf/serve-p7 is NOT merged to main: the branch fails `t_t1_10_r3_a_session_reads_its_writes_across_leader_changes`
  in 3 of 6 rounds, main in 0 of 6. Untraced; its failure text was not read (adacachy2 had no idle core at 22:47Z).
- 82eed2e0 (the clippy fix) is unchecked by clippy and unbuilt.
- Main has moved past 98bd0fa9 (engine only: decimal, pager, optimizer, morsel; nothing in crates/amphora); not merged
  into the branch.

## Commands

    # per arm, in the remote clone at the arm's SHA
    cargo test -p amphora --locked --lib --no-run; cargo test -p amphora --locked --test serve --no-run
    lib database::slot:: --test-threads=4                                                  # the branch
    lib <P13's seven> --test-threads=4
    lib database::replicated:: replication:: --test-threads=4; serve cp_t1_13 --test-threads=2   # 3 rounds, alternated
    cargo clippy -p amphora --all-targets --locked -- -D warnings                          # the branch
    # probe: python3 -I probe.py <tree> on 036ebe32; lib seed_and_delta_hold_through_an_election_at_each_step --nocapture
