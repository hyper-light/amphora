# Lane C2 (parallel), 2026-11-24 05:47Z to 06:47Z: T-QT.6.18 never finished because of its own statement

## Losses

- Nothing is merged. Ada's benchmark window on adacachy2 ran from 05:55Z past the lane's hour, and check-production counts only on Linux. Branch `perf/parallel-c2` holds the fix.
- The brief expected a lost event. There is none: no thread waits on another while the test runs on.
- Not run: check-production, `--test join_methods`, `--test adaptive_joins`, and the repeated `morsel` runs at several `--test-threads` (the list for the next lane is below).
- The two-thread picture at 30 s and 75 s on adacachy2 (r537) is in that machine's `~/.cache/amphora-perf/parallel-c2-logs/main-219603df/gdb.log`; only its `/proc` lines were read before the window.

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

