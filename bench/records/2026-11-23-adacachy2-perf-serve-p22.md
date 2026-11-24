# Lane P22 (serve): V13's real-time bench on the session worker, the landing checks, and the landing (2026-11-24 UTC, adacachy2)

Goal: every replicated write either commits or gets a typed refusal, and no long write costs the group its leader
(CP-T1.13). Deliverable: perf/serve-p7 (the session worker of replication.md, "Decision (lane P6)") on main, or V13's
failure shown real. Decision 1 (the coordinator's, binding): V13's clock hold lands only if V13's real-time bench shows
the branch answering every write with no stale refusal.

The lane ran from 04:58:54Z to 05:58:54Z. Tree under test: H = `d0c5722d` (perf/serve-p20-v13hold with main
`6d8e7dcd` merged), M = main `6d8e7dcd`.

## Losses first

- The branch is slower than main in V13's cell, and nothing here explains it (no profile was taken; rule 0). In r349
  (3 rounds, both classes, arms alternated) the branch's p50 was higher than main's in 6 of 6 round-class pairs, by 1.09x
  to 1.63x, and its commits a second lower in 5 of 6. In lane P20's r174 (2 rounds, older trees, below) it was slower in
  4 of 4: 0.48x to 0.81x main's commits a second, p50 1.23x to 2.02x. Every cell ran on 2 cores at load 40 to 58, so
  these are signals, not a timed A/B: the A/B with a profile of one lane's cell on cores of its own is not done.
- The first two asks for the bench and the checks at cpus=4 (and one at cpus=3) were refused for room (1 to 3 of 32
  cores idle); the bench ran at cpus=2,mem=8g (r349), the checks at cpus=4,mem=16g (r341) once room opened.
- The round's four failures are main's own reds (B19's) as lane P20 called them: `a_replicas_database_log_stays_within_its_room`
  fails on main about half the runs (1 of 2 on each arm here), and three fail on both arms every run (below).
  Not fixed here: not this lane's.
- Not settled: lane P21's two diagnosis tests (a restart due waits for a read's deadline, a local read holds off the log's
  restart) failed in its round of the tree `338e6916` and, for the first, on its diag branch; neither failed in this lane's
  round of H (r341). They are not in this round's failures, and P21's question is open.

## The real scenario (decision 1)

V13's real-time cell: lane V13's `cell13.py` (32 writers spread over 3 members, batch 1, 600 B rows, 1 s warmup, 5 s
counted, `--deadline-s 10`, `--relay-records 128`, persisted or replicated class, release `amphora-bench` of each tree).
`refused` counts the driver's `Serialization` refusals (a stale base, each retried at the same member) over the writes it
counted; `misses` the writes unanswered within their 10 s deadline; `writers_missed` the writers with any. Two runs:

r174 (lane P20's b1.sh; arms `143b82cd`, the branch with main `9fbfdbaf` and the shed fix, and main `9fbfdbaf`; 2 rounds,
arms alternated, 2 cores, load 48 to 55):

| arm | class | round | writes | commits/s | p50 ms | p99 ms | refused | misses |
|---|---|---|---|---|---|---|---|---|
| branch `143b82cd` | persisted | 1 | 62 | 12.4 | 2851 | 3965 | 0 | 0 |
| branch | persisted | 2 | 70 | 14.0 | 2294 | 3736 | 0 | 0 |
| branch | replicated | 1 | 39 | 7.8 | 2785 | 5308 | 0 | 0 |
| branch | replicated | 2 | 76 | 15.2 | 1606 | 3146 | 0 | 0 |
| main `9fbfdbaf` | persisted | 1 | 101 | 20.2 | 1704 | 2195 | 0 | 0 |
| main | persisted | 2 | 86 | 17.2 | 1868 | 3113 | 0 | 0 |
| main | replicated | 1 | 82 | 16.4 | 2048 | 3178 | 0 | 0 |
| main | replicated | 2 | 146 | 29.2 | 795 | 1606 | 0 | 0 |

(one branch cell's driver said at its stop "node 2 closed the connection": the stop's answer raced the member's exit;
r349's main arm did the same on its stop, so it is the stop's, not the branch's. Every driver exited 0.)

r349 (`bench/records/2026-11-23-adacachy2-perf-serve-p22/b1.sh`; arms H `d0c5722d` and M `6d8e7dcd`; 3 rounds, arms
alternated, 2 cores, load 40 to 58, CPU pressure some 23 to 45 percent; the H binary holds the session worker's text (1 hit of
"second session handed to the session worker") and M's none (0)):

| arm | class | writes | commits/s by round | p50 ms by round | p99 ms by round | refused | misses | writers missed |
|---|---|---|---|---|---|---|---|---|
| branch `d0c5722d` | persisted | 461 | 27.4, 39.8, 25.0 | 1212, 934, 1180 | 1704, 1229, 2195 | 0 | 0 | 0 |
| branch `d0c5722d` | replicated | 539 | 35.8, 34.4, 37.6 | 909, 754, 713 | 1442, 1507, 1376 | 0 | 0 | 0 |
| main `6d8e7dcd` | persisted | 485 | 30.8, 36.2, 30.0 | 745, 860, 778 | 1524, 1262, 1655 | 0 | 0 | 0 |
| main `6d8e7dcd` | replicated | 637 | 40.0, 43.8, 43.6 | 590, 532, 549 | 1180, 1081, 1098 | 0 | 0 | 0 |

20 cells in all, 0 refused, 0 misses, 0 writers missed, rc 0: in real time the branch answered every write with no stale
refusal, as main does. Decision 1 is met.

## The landing checks (r341, `c1.sh`; tree H, 4 jobs, load 44 to 53)

| check | result |
|---|---|
| `cargo fmt --all --check` | clean (exit 0) |
| `qb2_the_shed_never_takes_a_hand_offs_order_to_campaign` | ok |
| seed_and_delta_hold_through_an_election_at_each_step, 3 runs | ok, ok, ok |
| v13_thirty_two_spread_writers_on_moving_time_are_each_answered_with_no_stale_base (the hold), 3 runs | ok, ok, ok; its rounds moved the clocks 655 to 678 of 918 to 957 (Persisted) and 578 to 579 of 773 to 817 (Replicated) |
| cp_t1_13 lib tests | 16 of 16 ok |
| clippy -p amphora --all-targets --locked -D warnings | exit 0, 36 s, "Checking amphora", engine and api re-checked |
| serve binary (`tests/serve.rs`) | 11 of 11 ok (12 s), cp_t1_13_sql_takes_bound_parameters_and_answers_typed_rows_with_their_names among them |
| one round, `database::replicated:: database::slot:: replication::` (67 s) | 192 passed, 4 failed, 5 ignored |

The round's 4 failures, each run alone twice on M (`6d8e7dcd`) and twice on H, arms alternated: `a_checkpoint_pass_copies_no_frame_past_the_last_finished_flush`,
`a_local_read_open_across_an_image_install_sees_one_side_whole` and
`past_its_room_a_replica_refuses_submits_and_restarts_whatever_its_share` failed on both arms every time;
`a_replicas_database_log_stays_within_its_room` failed on both arms in 1 of 2 (flaky on main too). None is the branch's
own (lane P20's "main's four, B19's"). The floor test did not fail this round.

## Merging

Main `6d8e7dcd` (lane V17's landing of V16's close fix and V15's test in `crates/amphora`, lane Q6's read lock in `crates/engine`, lane F2's
bench/Cargo.toml) merged into the branch without a conflict (`git merge-tree` first) as `d0c5722d`. Main had not moved
when the record was committed (`git fetch` at 05:18:30Z), so no second merge was needed before the push.

## Runs

| run | needs | what |
|---|---|---|
| r341 | cpus=4,mem=16g, jobs 4 | c1.sh: the landing checks on H, then the round's failures on M and H (05:08:49Z to 05:13:41Z) |
| r349 | cpus=2,mem=8g, jobs 2 | b1.sh: the release benches of H (built in 236 s) and M (96 s), V13's cell alternated 3 rounds |
| lane P20's r174 | cpus=2,mem=8g, jobs 2 | b1.sh of P20 (2 rounds; on `143b82cd` and main `9fbfdbaf`), read here, not re-run |

Both scripts ran on adacachy2 (00aad8da...) through kiln `run`, sync false, in the remote clone
`~/.cache/amphora-perf/serve-p7-remote` (the checks) and lane P20's shared clone `~/.cache/serve-p20-ab/b1` (the bench
builds), CARGO_INCREMENTAL=0, the lane's target dir `~/.cache/amphora-perf-target/serve-p7`, CARGO_HOME
`~/.cache/cargo-home-perf-serve-p7`. Binaries under `~/.cache/amphora-perf-bin/serve-p22/<sha>/`.
