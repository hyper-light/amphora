# xgroup: WaitOnLock reads that never answer (lane K9, 2026-11-23, adacachy2)

Goal: Amphora's work on main with every test green. `xgroup::sql::sessions::t_t1_10_w7` and `t_t1_10_w11`
fail on main in their `LockedReads::WaitOnLock` mode: the read waits on its row's lock and is never answered
once the gate lets the resolution through; it gives up at its 120 s deadline (`Err(Timeout)`).

All runs: adacachy2 (`on` 00aad8da, needs `linux/amd64,cpus=4,mem=16g`), CARGO_INCREMENTAL=0, CARGO_BUILD_JOBS=4,
target dir `~/.cache/amphora-perf-target/xgroup`, each SHA's lib test binary copied to
`~/.cache/amphora-perf-bin/xgroup/<sha>`, each test run alone (`<bin> xgroup::sql::sessions::t_t1_10_<t> --test-threads=1`).
Scripts: `~/.cache/amphora-perf/xgroup-scripts/{arm,bisect,instr}.sh` (copies on adacachy2 under the same path),
logs `~/.cache/amphora-perf/xgroup-logs/` on adacachy2.

## A10's guess (968b9c1a) is refuted

| arm | w7 | w11 |
|---|---|---|
| main 93b6458c (r1295) | 0 of 2 | 0 of 2 |
| main with 968b9c1a reverted, f424e48b (branch `k9/revert-968b`, r1295) | 0 of 2 | 1 of 2 |
| 4fd15364, "Land t1/land-all" (r1317) | 1 of 1 | 0 of 1 |

Reverting 968b9c1a on main leaves w7 red in both runs, so 968b9c1a does not break w7. 1495f4fb, eca9dc6d (the
first commit holding both 1495f4fb and 968b9c1a) and a8b3db97 do not compile (their lanes landed them
uncompiled), so the bisect starts at 4fd15364, where w7 passes and w11 already fails.

## Bisect of w7 (r1323, `git bisect --first-parent 93b6458c 4fd15364`, green = 3 runs green)

- good fbf7dbdb (perf-A6's call charge A/B commit on perf/cross-call)
- bad 8deb1ee6 "Merge origin/main into perf/cross-call (lane perf-A6)", parents fbf7dbdb and 5659f086
- bad 3adf4eea, 4535a304, 9a7b14bb (perf-A7, after the merge); 4a099be6 skipped (did not build)

So w7 turns red where main at 5659f086 (103 commits past fbf7dbdb) merges into perf-A6's call-charge branch.
Of those 103 commits only 8d30a33a (perf-A6: a scan's page and a page of changes end at RESULT_MAX) touches
database.rs, xgroup/, store.rs or session.rs. Not yet tested: 5659f086 alone (red there means main's side; green
means the merge's interaction with fbf7dbdb's call charge).

## Mechanism: what the code says, not yet measured

The waiting read (database.rs `wait_joined`) blocks on its row's bell until `ring` bumps that row's count;
for standalone groups the only ring is `Database::ring_locks(&dropped)` in xgroup/sql.rs `step_on`/`serve`, with
`dropped` collected by `SqlTablet::drop_locks_of` (the only delete of `__amphora_xg_lock`). Keys match on both
sides (`record_key`). Between 4fd15364 and main the submit path was rebuilt around admission by charge
(`submit_local`, `room_until`, `call_nowait`, `AnswerMax`). A failing read ends `Err(Timeout)`, which can come
from the bell's wait or from a retry's `submit` waiting for room. Branch `k9/instr` (66f8df31) prints each
step on a local member, each ring (keys and lists) and each try/wake/timeout of the read; it never ran: built
in a second worktree beside a running build in the same target dir, cargo reused the other checkout's
hyper_raft rlib (path crates hash by their path inside the workspace), so it failed to compile. Build it in
`~/.cache/amphora-perf/xgroup-remote` itself, one build at a time.
