# Lane C2 (parallel), 2026-11-24 05:47Z to 06:47Z: T-QT.6.18 never finished because of its own statement

## Losses

- **The release runs of the same statements at 1 to 8 workers are not measured.** So whether a user's release build finishes statement 20 in bounded time is not answered. The release build was killed twice, once by the benchmark window (r543) and once by r566's own 1,200 s limit, and the third (r634) got 1:22 of CPU in about 14 minutes on MacBook Pro 2 at load 55 to 77. The harness and its inputs are ready (below).
- Not merged. Ada's benchmark window on adacachy2 ran from 05:55Z past the lane's hour, and check-production counts only on Linux. The lead chose to hold the merge (c). Branch `perf/parallel-c2` holds the fix.
- The brief expected a lost event. There is none: no thread waits on another while the test runs on.
- Not finished on the branch: `cargo clippy -p amphora-engine --all-targets` (r578 hit its 1,800 s limit while it still built dependencies), check-production, `--test join_methods`, `--test adaptive_joins`, and repeated full `morsel` runs. One full `morsel` run finished (below).
- The two-thread picture at 30 s and 75 s on adacachy2 (r537) has a gdb dump of both threads in that machine's `~/.cache/amphora-perf/parallel-c2-logs/main-219603df/gdb.log`; only the `/proc` lines below were read before the window.

## Cause

`random_body_chain` (`crates/engine/vdbe/morsel_scan_tests.rs`) joins every term with `AND` and writes one of them as `{t}.b IS NULL OR {t}.a > {n}` with no parentheses. `AND` binds tighter than `OR`, so the keys bind only the left side. The test's 21st statement (number 20) becomes

```sql
SELECT v.c, t.a, CASE WHEN w.a > w.a THEN w.c ELSE w.c END FROM t, u, v, w
WHERE t.a = u.a AND t.c = u.c AND u.a = v.a AND v.a = w.a AND w.b IS NULL OR w.a > 5
ORDER BY v.c, t.a DESC
```

Every row of `w` with `a > 5` joins every row of `t`, `u` and `v`. The design says the test's statements are inner equi-joins (`docs/requirements.toml` T-QT.6.18) and that a cross product stays serial (`docs/design/executor.md`, the chain of hash tables, lane qs6-10). Statements 3, 9, 20, 29 and 32 carry the ungrouped OR; 20, 29 and 32 join four tables. The test was committed uncompiled and unrun (38b83c19's message says so), so it had never passed.

## Evidence

| Run | What it shows |
|---|---|
| r537 (adacachy2, main 219603df, debug test binary under gdb, load 28 to 31) | At 30 s and at 75 s the process has two threads. libtest's main thread is in `futex_wait`, voluntary switches 309 then 311. The test's thread is in state R with 28.72 s of CPU at 30 s and 70.97 s at 75 s; over 2 s it gains 2.00 s and 1.95 s. No worker thread exists. |
| r540 (adacachy2, SQLite 3.54.0 on the test's fixture) | Statement 20 counts **16,464,756** rows in 11.88 s (7.42 s user). Statements 2, 5, 8, 11, 14 and 17, the other four-table ones before it, count 0 to 64 rows in 0.3 to 0.5 ms. |
| r566 (MacBook Pro 2, main 219603df plus two logging tests, `84efd36b`, debug, load 55 to 90) | The test's own loop with each run logged: 120 runs (statements 0 to 19, serial and 1, 2, 3, 4 and 8 cores) end in 57.3 s. Statement 20's plan is SCAN w, SCAN v, SCAN u, SCAN t, USE SORTER FOR ORDER BY: 26 x 44 x 103 x 519 = 61,154,808 inner rows. Its first run, the serial one (`ParallelMode::Off`, no workers), starts at 57.387 s and has not ended when the 180 s limit stops it. The unmodified test, run beside it, is stopped at the same limit. |
| `sample` of that unmodified test (r566, about 90 s in) | Main thread: `run_tests_console`, `mpmc::list::Channel<CompletedTest>::recv`, `Thread::park`. Test thread: `t_qt_6_18…`, `sql_rows` (morsel_scan_tests.rs:29), `Statement::run_collect_rows`, `Statement::step`, `step_inner`, `op_column`, `op_column_fetch`. No other thread. |

The first wrong place is the generator's term, not a lost event: the engine runs the statement it was given.

## Real use: what is known and what is not

- Known: the engine's serial run of statement 20 (no workers, `ParallelMode::Off`) is the first run that does not end; the debug build was still in it after 122.6 s. SQLite 3.54.0 takes 11.88 s for the same statement as a count.
- Not known: the release build's time at 1, 2, 4 and 8 workers, for statements 20, 29 and 32, and whether its answers equal SQLite's (SQLite's count for 20 is above; 29 and 32 were not counted).
- How to measure it: branch `perf/parallel-c2-diag` (`84efd36b`) has `crates/engine/examples/c2_user.rs`, which loads the fixture through the public API and runs each statement with `ByCost` and `HashJoinsOnly`, serial and on 1, 2, 4 and 8 cores, under a per-statement limit, printing rows, an order-independent hash, prepare and run time. `C2_DIR=<dir> <test binary> --exact vdbe::morsel_scan::tests::c2_dump_fixture` writes `fixture.sql` and `queries.txt` (sha256 8b90ab3a… and 7c3c6f1a… on MacBook Pro 2). Then `c2_user fixture.sql queries.txt <db dir> 60 20`, and 29 and 32, each under an outer limit. Deps of the release build are built on adacachy2 (r543) and MacBook Pro 2.

## The fix and its checks

`62acdd6d` is the T1 reviewer's `350352d5` from `origin/t1/main-review`, cherry-picked unchanged onto main `0101aa38`: the term becomes `({t}.b IS NULL OR {t}.a > {n})`, and a new test, `t_qt_6_18_a_generated_disjunction_keeps_the_join_keys`, runs a generated disjunction on two rows that must not join. The test's assertions, its 36 statements' random stream and its five machine sizes are unchanged. The reviewer's record (`d10ea05c`, on the same branch) reports Linux r136 at 1.93 s on `2571f841`.

| Check on `62acdd6d`, MacBook Pro 2 | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 (r578) |
| `t_qt_6_18` (both tests), debug, `--test-threads=1` | 2 passed in 35.38 s at load about 50 (r578) |
| `morsel` (the whole filter), debug, `--test-threads=4` | 49 passed, 1 failed, 4 ignored in 309 s, no hang (r578). The failure is `t_qt_6_35` ("the hook built no link"), main's own, lane C1's. |
| `t_qt_6_13 t_qt_6_17 t_qt_6_18 t_qt_6_19 t_qt_6_24`, debug, `--test-threads=1`, 2 and 4, limit 240 s each (r648) | With 1 and 2 threads, stopped by the 240 s limit while only `t_qt_6_24` (one thread) and `t_qt_6_24` beside it (two) still ran; 4 threads stopped by me at the hour with `t_qt_6_18` both tests and `t_qt_6_17` passed. `t_qt_6_18` (both tests) passed in all three; `t_qt_6_13` and `t_qt_6_19` pass at load 55 to 77 in over 60 s. |

## For the next lane

On adacachy2 after the window, on `perf/parallel-c2` merged with the then main, each under a timeout:

1. `bash scripts/check-production.sh`, `cargo fmt --all --check`, and `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` (the log must show `Checking amphora-engine`).
2. `cargo test --locked -p amphora-engine --lib -- --test-threads=1 t_qt_6_18`, then `morsel` at `--test-threads` 1, 4 and 8, three runs each; `t_qt_6_35` fails until C1 lands.
3. `cargo test --locked -p amphora-engine --lib -- translate::parallel`, `cargo test --locked -p amphora-engine --test join_methods`, and `cargo test --locked -p amphora-engine --features simulator --test adaptive_joins`.
4. The release runs above, at 1, 2, 4 and 8 workers.

