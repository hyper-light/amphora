# Lane F3 (hygiene): bench clippy clean on main, and Turso waiting on its own busy handler (2026-11-24, adacachy2)

Ada's goal is a green main every lane can merge onto, and a bench that measures the work and not its own
waiting. This lane had one deliverable: `cargo clippy --manifest-path bench/Cargo.toml --all-targets -- -D
warnings` exits 0 on main, with Ada's two decisions on F2's questions applied.

Verdict, losses first.
- Turso's write tail got worse and its CPU doubled, in one run each. Write p99.9 went from 729 to
  5,243 ms and the longest write from 1,002 to 9,843 ms. Mixed-phase CPU went from 27.6 to 52.5 s. The
  median, p90 and p99 writes got faster, and writes went from 2,458 to 2,559 (section 4). Run-to-run noise is
  unknown (one run each, load 46-49 on 32 cores), and no profile or trace says why. The source fits the
  numbers (a waiting writer steps again without pause, and its handler retries the lock 1 to 100 ms apart where
  the bench retried every 200 us), but that is read from the code, not measured.
- The Turso runs used 2 writers, 2 pool connections and 1 reader, held to the 4 cores the balancer gave
  (8 were refused). K0's default is 32 writers and 64 connections, where up to 31 threads could step in a
  loop at once. That is read from the source, not run.
- suite_db.rs's Turso target got the same change and was not run. None of the bench's 49 tests runs either
  Turso target.
- Ada's decision says Turso's own handler does the waiting. At 3e09a82 it doesn't block: it hands the caller
  `Sleep { duration }`. The bench now does what Turso's SDK does with it (step again at once), so no
  `#[expect]` was added, but that is a loop with no wait in it. Turso's JS binding sleeps the duration instead.
  Which of the two the bench copies is Ada's call (section 2).
- Bench clippy exits 0 on Linux at this lane's head and on its merge with main. macOS was not checked.

## 1. Clippy

F2's scripts (`~/.cache/amphora-perf/hygiene-f2-scripts/f2_clippy.sh`, the same on adacachy2), Linux, lane
target dir `~/.cache/amphora-perf-target/hygiene`:

| commit | what | findings | exit |
|---|---|---|---|
| aad61ad1 | F2's head (03:59Z, 2 jobs) | 23 | 101 |
| b02b2759 | this lane's fixes (04:10Z, 1 job; the bench crate re-checked, 8 s) | 0 | 0 |
| f3700bad | b02b2759 merged with main 9ed5b097 (r251, 04:14Z, 2 jobs; `Checking amphora-engine`, `amphora`, `amphora-bench`, 45 s) | 0 | 0 |
| b5632498 | merged with main 5bd0f46e (r292, 04:27Z, 4 jobs; `amphora` and `amphora-bench` re-checked); tests 49 of 49, per-test identical to 7de5344d | 0 | 0 |
| 81b3cb3d | merged with main 0c13cf68 (r295, 04:30Z, 4 jobs; `amphora-engine`, `amphora`, `amphora-bench` re-checked); tests 49 of 49, identical. Pushed as main at 04:33Z | 0 | 0 |

The 23, fixed in code (no `#[expect]` or `#[allow]` added):
- collapsible_if (7): oracle/mod.rs (2), oracle/reduce.rs (2), oracle/synth.rs, planner/prepared.rs,
  suite_db.rs's VACUUM. Let chains, the body one level out; no `else` on any of them.
  `hygiene-f3-scripts/collapse.py` did the four of one shape and refuses any other.
- type_complexity (4): reduce.rs's clause cuts are `[fn(&mut Sel) -> bool; 4]` (non-capturing closures,
  no boxes); pg.rs `EventRow`, query.rs `Run`, repl.rs `Program` name the other three.
- explicit_auto_deref (2), double_ended_iterator_last (`next_back`), manual_is_multiple_of,
  map_entry (colcopy.rs, the entry API), redundant_closure, redundant_guards (suite_db.rs's Amphora
  retry errors as one or-pattern), unnecessary_sort_by (`sort_by_key` with `Reverse`: the same stable
  descending order).
- too_many_arguments (cc.rs `class_txn`, 8): writer `w` and its RNG (seeded from `w`, cc.rs:450) go as
  one `Writer`, destructured in the signature, so the body is unchanged.
- wrong_self_convention: scale.rs `to_leader(&mut self)` is `ask_leader`.
- items_after_test_module: `mod inproc`'s test module moved to its end, unchanged.
- disallowed_methods `std::env::temp_dir` (scale.rs, the T-SH.10 test): `tempfile::tempdir()`, the
  test's directory a fresh path inside it, as before.

The `#[expect]`s left in the bench are Ada's three: PD's states (scale.rs, twice) and the load average
(planner/mod.rs).

## 2. Vanilla Turso's busy retries (decision 1)

What Turso does, read at the pinned revision (3e09a82, `~/.cargo/git/checkouts/turso-455557cd4a2364c7/3e09a82`):
- A connection's busy handler is `BusyHandler::None` until set (core/connection.rs:518): `Busy` at once.
  So before this lane every contended write came back `Busy`, and the bench's 200 us loops did all the
  waiting; the `Sleep` arms never ran.
- `set_busy_timeout` (core/connection.rs:4831), the same as `PRAGMA busy_timeout` (core/translate/pragma.rs:361),
  installs `BusyHandler::Timeout`: SQLite's default busy callback, its backoff schedule up to the total
  (core/busy.rs:61-90 and 142-167).
- Every `Busy` a statement meets goes through that handler (core/vdbe/mod.rs:2681 turns an opcode's
  `LimboError::Busy` into `StepResult::Busy`; core/statement.rs:753-775 invokes the handler on it).
- The handler doesn't block. `step()` yields `StepResult::Sleep { duration }` (the backoff left), and keeps
  yielding it until that instant passes (core/statement.rs:669-681, core/busy.rs:42-48).
- Turso's own SDK answers `Sleep` by running the IO loop and stepping again: sdk-kit's `step_inner`
  (sdk-kit/src/rsapi.rs:1520-1526), sync mode; the Rust crate's async path re-polls at once
  (bindings/rust/src/lib.rs:436-441). Python and Go take sdk-kit's step. On Linux, `io.step()` returns at once
  (core/io/unix.rs:81). Only the JS binding sleeps the duration (bindings/javascript/src/lib.rs:214-219).

What changed (b02b2759):
- Turso gets the busy timeout each file's SQLite target already had: vanilla.rs 60 s, suite_db.rs 120 s
  (`WAIT`, already Amphora's deadline there). One constant per file sets both engines.
- The four 200 us loops are gone (vanilla.rs `write` and `history`, suite_db.rs `transaction` and `run`).
- The `Sleep` arms run the IO loop and step again, as sdk-kit does. Their two `thread::sleep` calls
  and `#[expect]`s are gone.
- A `Busy` that outlasts the timeout is an error, as SQLite's is. vanilla.rs's `search` used to return
  its partial count when it met `Busy`; it now waits like the rest.

What it costs: a writer waiting for the lock re-steps without pause until its backoff instant, as
Turso's SDK users' writers do. Under the default K0 (32 writers, 64 pool connections), up to 31 threads
can spin while one writes. The 200 us loop slept between tries.

## 3. Tests

`f2_tests.sh 7de5344d b02b2759` (r280, 04:21-04:22Z, adacachy2, 4 jobs, `--release --bin amphora-bench`):
49 tests at each commit, 49 ok, the per-test results identical. F2's aad61ad1 is identical to main too
(r198, 04:19-04:20Z). The suite takes 0.34-0.38 s.

## 4. Turso, before and after (one run each)

The bench's K0 workload on vanilla Turso, built at aad61ad1 (F2's head: the 200 us loops, no busy handler)
and b02b2759 (this lane's: the busy timeout, Sleep stepped through), each binary built in the lane's one
target dir and kept under `~/.cache/amphora-perf-bin/hygiene/<sha>` on adacachy2. One run each, back to back
(r280, `hygiene-f3-scripts/f3_final.sh`):

    <bin> --target turso --dir ~/.cache/amphora-perf/hygiene-turso-ab/<sha> --writers 2 --connections 2 --readers 1

K0's defaults otherwise: 1,000 sessions, 200 events each preloaded, 30 s mixed, batch 1, 20 searches.

| | before (aad61ad1) | after (b02b2759) |
|---|---|---|
| writes in 30 s | 2,458 | 2,559 |
| write events/s | 81.8 | 85.2 |
| write ms p50 / p90 / p99 | 13.6 / 47.1 / 149.5 | 9.5 / 17.4 / 52.2 |
| write ms p99.9 / max | 729 / 1,002 | 5,243 / 9,843 |
| seconds whose longest write passed 1 s | 1 of 32 | 7 of 32 |
| mixed-phase CPU | 27.6 s | 52.5 s |
| reads: n, p50 / p99 / p99.9 ms | 304,796, 0.078 / 0.46 / 3.49 | 274,849, 0.079 / 0.69 / 4.54 |
| searches p50 / p90 ms | 36.4 / 581.6 | 42.0 / 352.3 |
| preload events/s (2 writers side by side) | 4,758 | 4,377 |
| failures, unanswered | 0, 0 | 0, 0 |
| 1-min load, start / end | 45.8 / 48.9 | 48.9 / 46.9 |
| CPU pressure avg10, start / end | 23.9% / 24.6% | 24.6% / 28.1% |
| I/O pressure avg10, start / end | 18.8% / 4.8% | 4.8% / 3.9% |

Ada asked for one run each and no other A/B, so the noise is unknown and nothing here is a finding about
why. The full JSON of each run is in `~/.cache/amphora-perf/hygiene-f3-turso-<sha>.out` on adacachy2.
