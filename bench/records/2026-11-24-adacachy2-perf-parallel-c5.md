# Lane C5 (parallel), 2026-11-24 06:57Z to 07:36Z: T-QT.6.18's fix is on main, and the statement behind it ends on a release build

## Losses

- **No morsel run of this lane is green.** `t_qt_6_35` fails in all nine ("the hook built no link"). It fails with the
  same words on main's own binary (219603df, run here at 07:23Z) and in lane C1's record. It belongs to the
  planner's lane and was not touched. Nothing else fails. Lane C4 has since changed that test's setup on
  main (`171e902c`, its assertions as they were), and its record has the whole suite on `1eaff6c8`, which has
  `60993be4` in it: 51 passed, 0 failed, 4 ignored, 95.9 s (its r950). This lane did not run that tree.
- **`translate::parallel` checked nothing.** The filter matches 0 of the lib's 3,231 tests, so its "ok" is
  empty. `crates/engine/translate/parallel.rs` (2,169 lines) has no test module. Lanes R4, R10, R12, durable and
  C2 list the filter beside others, whose tests gave the counts. No test was added.
- **Amphora takes longer than SQLite on statements 20 and 32, and the timing is not a benchmark.** On 20 it
  took 1.04 to 1.74 times SQLite's time in the same repetition, on 32 1.21 to 1.87 times, on 29 0.68 to 0.76
  times. adacachy2 was at load 40 to 87 from other lanes' work. SQLite alone, with the same work, took 8.9 s
  to 19.1 s for statement 20 in this hour. A run had 2 cores, not the 16 the rules ask for. Nothing was
  profiled, so no cause is given.
- **The worker settings measure one thing.** The harness sets 1, 2, 4 and 8 cores, but the engine ran the
  statement on one thread wherever it was sampled: 10 samples over statement 29's run, 2 to 8 cores of both
  methods among them, and 13 over 31 s of one of statement 20's, at 1, 2 and part of 4 cores. Each showed 1
  thread, state R, and CPU time growing with elapsed time. `docs/design/executor.md` says why: no equality
  links a table to the rest, so "a cross product stays serial". These runs show that the statement ends at
  those settings. They say nothing about how workers scale.
- The binary timed is the merged tree `f65a11cf` (main `75efff54` and the fix), the tests and lints are
  `6e6d2c2d` (main `95669341` and the fix), and main's landing is `60993be4` (main `04aee7c9`, records only
  over `6e6d2c2d`). No file under `crates/engine*` differs among the three. `pg-catalog`, which the engine
  depends on, differs between the first and the other two.
- macOS was not run. The two macOS asks (r744, r796) waited for room and never started, and were stopped.
- The first 12 minutes got no run. adacachy2 had 21 idle cores at 06:58Z and 0 at 07:01Z, and asks for 8, 6, 4,
  3, 2 and 1 cores were refused. The release build got 2 cores at 07:08Z and the test build 1 at 07:13Z. Asks got
  no answer from the kiln server for about a minute at 07:06Z and at 07:12Z.

## What landed

`62acdd6d` is the T1 reviewer's `350352d5` (`origin/t1/main-review`), cherry-picked by lane C2 with `-x` and not
changed since. Both have patch id `f5b651f4…`. It is test code only: the generated OR term gets its
parentheses, so it keeps the statement's join keys, and the new
`t_qt_6_18_a_generated_disjunction_keeps_the_join_keys` pins that. It went to main as `60993be4` at 07:23Z. Main
has moved since (`b53a29b1`, lane B23), and the fix is in it.

## Checks on the merged tree, adacachy2, debug

| Check | Result |
|---|---|
| `cargo fmt --all --check` | exit 0, on adacachy2 and again on adacachy (no build) |
| `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` | exit 0 in 73 s; the log shows `Compiling amphora-engine v0.1.0` (the crate has a build script, so cargo names it once, as Compiling) and the fingerprint of the engine's lib-test unit is dated 07:19:19Z, when the run ended |
| `bash scripts/check-production.sh` | exit 0; engine checked; 10617 left across 17 lints, as the ratchet holds |
| `t_qt_6_18` (2 tests), `--test-threads` 1, 4, 8, three runs each | 2 passed in all nine: 4.33, 2.42, 2.46 s; 2.80, 2.45, 2.27 s; 2.27, 2.31, 2.35 s (load 79 to 91) |
| `morsel` (55 tests), `--test-threads` 1, three runs | 50 passed, 1 failed (`t_qt_6_35`), 4 ignored each: 158.0, 149.9, 129.9 s, no hang |
| the same at 4 | the same: 59.7, 50.0, 41.7 s |
| the same at 8 | the same: 35.3, 37.1, 31.7 s |
| `--test join_methods` | 4 passed, 1 ignored, 4.25 s |
| `--features simulator --test adaptive_joins` | 23 passed, 22.4 s |
| `translate::parallel` | 0 tests (above) |

Before the fix, the debug build's serial run of statement 20 was still going after 122.6 s (lane C2, r566).
No run here was stopped by a limit.

## The statement on a release build

The engine is the release profile (thin LTO, one codegen unit, overflow checks on), on an AMD Ryzen 9 9950X3D2
(32 threads), Linux 7.3.3, rustc 1.99.1. `c2_user` (lane C2's, from `84efd36b`, unchanged) loads the test's fixture
into a database file through the public API and runs the 21st statement (20) and the two others with four
tables and an ungrouped OR (29 and 32), with a limit of 60 s each. Fixture and queries are C2's (sha256 8b90ab3a… and
7c3c6f1a…). SQLite is the system's 3.54.0, through `c5_sqlite.c`, which does the same work per row: each value
made a string, hashed with SipHash-1-3, summed. On statements 0, 2, 5 and 8 the two give the same hash, so
the hashes can be compared. No run reached its limit.

All 50 Amphora runs (three of each setting for 20, one for 29 and 32) give SQLite's rows and SQLite's hash:

| Statement | Rows | Hash, SQLite's and Amphora's at serial, 1, 2, 4 and 8 cores under ByCost and HashJoinsOnly |
|---|---|---|
| 20 | 16,464,756 | `e5f620c1f88fbd6c` |
| 29 | 2,968,680 | `8255fc5c3c02eab2` |
| 32 | 15,437,188 | `81ec4404a7cd2349` |

Statement 20, seconds, three repetitions (load at start 68, 42 and 47):

| | SQLite | serial | 1 | 2 | 4 | 8 |
|---|---|---|---|---|---|---|
| 07:17Z ByCost | 11.5 | 18.7 | 19.5 | 20.0 | 15.9 | 15.4 |
| 07:17Z HashJoinsOnly | | 18.5 | 17.7 | 16.3 | 16.7 | 18.9 |
| 07:24Z ByCost | 8.9 | 11.1 | 11.3 | 11.4 | 12.8 | 13.4 |
| 07:24Z HashJoinsOnly | | 13.7 | 13.4 | 13.9 | 13.4 | 13.6 |
| 07:27Z ByCost | 10.8 | 16.8 | 12.8 | 11.9 | 12.0 | 11.3 |
| 07:27Z HashJoinsOnly | | 11.3 | 11.7 | 11.6 | 12.2 | 16.2 |

Statement 29 (07:20Z, load 86): SQLite 2.6 s. Amphora ByCost 1.9, 2.0, 1.9, 1.8, 1.8, HashJoinsOnly 1.8, 1.9, 1.9, 1.9,
1.8. Statement 32 (07:21Z, load 72 to 55): SQLite 10.2 s. Amphora ByCost 12.4, 12.3, 16.8, 19.0, 19.1, HashJoinsOnly 16.3, 16.6,
13.3, 14.0, 12.7.

At 07:30Z, load 52 to 70, SQLite alone: statement 20 counted in 11.3 s, stepped in 14.7 s, hashed in 19.1 s (CPU 9.2,
9.8 and 11.5 s); statement 32 counted in 10.5 s, hashed in 15.5 s. So a wall time here moves by up to 2.1 times with what
else runs.

## How to repeat it

- Branch `perf/parallel-c5-diag` (`7b4b3781`, not for main) has `crates/engine/examples/c2_user.rs` and
  `crates/engine/examples/c5_sqlite.c`. `gcc -O2 -o c5_sqlite c5_sqlite.c -lsqlite3`.
- `cargo build --release --locked -p amphora-engine --example c2_user` (5 m 33 s with 2 jobs at load 78), then
  `c2_user fixture.sql queries.txt <db dir> 60 20`, and 29 and 32; `c5_sqlite fixture.sql queries.txt <db file> 20 rows`.
- Fixture and queries: `~/.cache/amphora-perf/parallel-c2-fixture` on adacachy2, or `C2_DIR=<dir> <test binary> --exact
  vdbe::morsel_scan::tests::c2_dump_fixture` on `84efd36b`.
- The lib tests are `6e6d2c2d-lib` (sha256 b1826a31c77e9c92) and `f65a11cf-c2_user` (b58a47cf1cf6e6bd), in
  `~/.cache/amphora-perf-bin/parallel-c5` on adacachy2; logs in `~/.cache/amphora-perf/parallel-c5-logs`.
- The kiln CLI ended each run with "decode: Serde Deserialization Error" and exit 1 after the remote script had
  finished. Exit codes here are the script's own.
