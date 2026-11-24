# Lane C7 (planner), 2026-11-24 07:53:20Z to 08:53:20Z: C6's sweep of outer joins run to the end on main, no wrong answer; the middle and fact shards split so each ends

Machines: adacachy2 (runner 00aad8da, Linux, 32 cores, load 45 to 75 all hour from other lanes: no round here is a quiet machine's).
The sweep's matrix ran in an optimized test build (cargo's dev profile with `CARGO_PROFILE_DEV_OPT_LEVEL=2`, so
debug assertions and overflow checks stay on), 4 processes at `nice -n 10`, one test process per slice of runs. Its driver and the
test that runs a slice by environment are perf/planner-c7-probe (not for main); the test is C6's sweep loop, one line a run.

## 1. Losses first

- **The fact shard's hand-built trees without statistics are not run whole.** 48 of 600 runs (2 settings x 300) finished
  before I stopped them: 13 s a run on average, up to 88 s, so the two settings need hours; the landed tests leave them out (a comment at their macro says so). With ANALYZE they ran whole (both
  settings, 600 runs, 8 to 9 minutes of query time each). C6's sweep ran the fact shard by cost only; these are runs beyond it.
- Not run: the mutation (C6's fix taken out, to show this lane's driver sees the wrong answers it fixed: the build found no room;
  C6's own red runs of the same comparison stand for that), a debug build of the fact shards (their landed trimmed form was
  stopped by me after 14 minutes, not finished), a release build (the optimized test build is cargo's dev profile at opt-level 2, debug
  assertions on).
- The machine was full for much of the hour (0 to 6 of 32 cores idle), so runs were small and several were refused.
- `--test planner` is red as on main (planner.rs:182, a_whole_table_group_by_scans_and_sorts_where_lookups_would_read_pages).

## 2. The sweep, run to the end

Statements: 1,260 (420 joins, each as aggregates, under an ORDER BY and as groups): small 480, small_any 480, middle 240, facts 60.
Each ran with and without ANALYZE under the six plan settings (by cost; weights seek 2, hash build and probe 0.5; seek
2,000, builds and probes 0.5; seek 2,000, build 50, probe 0.5; the FROM-order trees split at 1 and at 2), each with the parallel mode off
and at 1, 2, 8 (padded outer probe) and 8 (kept outer probe) stated workers: 30 runs a statement. Optimized build 7f877d4b (main
67cc7e69 plus the test), adacachy2, 08:02Z to 08:24Z.

| shard | runs | wrong | query time, s | slowest, ms | runs with a bushy plan | runs whose workers built hash tables |
|---|---|---|---|---|---|---|
| small, ANALYZE | 14,400 | 0 | 2.1 | 17 | 2,224 | 962 |
| small | 14,400 | 0 | 1.3 | 17 | 330 | 557 |
| small_any, ANALYZE | 14,400 | 0 | 1.3 | 18 | 1,219 | 732 |
| small_any | 14,400 | 0 | 1.1 | 18 | 300 | 473 |
| middle, ANALYZE | 7,200 | 0 | 114 | 591 | 406 | 336 |
| middle | 7,200 | 0 | 52 | 137 | 30 | 257 |
| facts, ANALYZE, by cost | 300 | 0 | 225 | 2,488 | 240 | 0 |
| facts, by cost | 300 | 0 | 97 | 676 | 0 | 0 |
| facts, ANALYZE, the other five settings | 1,500 | 0 | 1,484 | 3,328 | 900 | 0 |
| facts, the three weights settings | 900 | 0 | 309 | 805 | 0 | 0 |
| facts, hand-built at 1 (partial) | 48 of 300 | 0 | 640 | 87,578 | 9 | 0 |

Total: 75,048 runs, 0 wrong. No query to list: no answer differed. (The facts' 600 runs by cost were C6's sweep whole; the other 2,448
extend it.) The counts of hooked and bushy runs are per statement's run, 0 where the plan has none to make.

MacBook Pro 2 (macOS, opt-level 2, its own build of 7f877d4b, 8 processes, load 38 to 61) ran the same 72,600 runs of C6's sweep by
08:28Z: small and small_any 57,600, middle 14,400, facts by cost 600, 0 wrong. Its by-cost plans are that machine's (bushy in 239 of the 300
runs of the facts with ANALYZE; adacachy2 240).

## 3. How the shards were cut, and why nothing distinct is lost

C6's landed tests were one test per database and statistics setting, the middle and fact ones too long for a debug build to end in 10 to 17 minutes
(r967, r968, and this lane's r1109: the landed middle ANALYZE test, 3,360 runs (its trimmed 14 a statement), passed in 692 s at load 55;
the landed fact ANALYZE test was stopped by me after 14 minutes, not finished). They are now one test per plan setting: 12 for
the middle shard (six settings, with and without ANALYZE) and 10 for the facts. Every statement (the joins' shapes, with their ORDER BY and
grouped forms) and every one of the five worker settings is run by each test, and the database and the statistics are its own, so the union
of a shard's tests is the full product of statements x settings x worker settings: nothing was trimmed. The facts by cost alone were C6's;
the weights settings (with and without ANALYZE) and the hand-built trees (with ANALYZE) are added, and the hand-built ones without statistics are
not, for the time above (a comment at the test's macro says so). The small shards keep C6's 14 runs a statement in the gate: 6,720 a test, 26,880 in
all; their full 30-run form ran in section 2 (57,600 runs).

What each test asserts beyond SQLite's answer (so a setting that stops doing its work shows): a bushy plan in the settings where the plan is the
same on every machine and one was seen (the weights settings with ANALYZE, the hand-built split at 1 on middle and, with ANALYZE, the weights on the facts), and
a hash table built by the workers where one was seen; never by cost, whose plans follow the learned weights. Its flags come from the runs of section 2.

## 4. `#[ignore]` marks, and what runs in the gate

| tests | ignored | why, or time |
|---|---|---|
| `t_pl_8_13_outer_joins_answer_as_sqlite_small`, `_small_analyzed`, `_small_any`, `_small_any_analyzed` | no, in the gate | 4 passed, 26,880 runs, 63.8 s at 3 test threads in a debug build at load 60 (72.6 s on df20a9cb) |
| 12 `..._middle_*` (6 settings, each with and without ANALYZE; 1,200 runs each) | yes | "minutes each in a debug build: 7 to 26 s at CARGO_PROFILE_DEV_OPT_LEVEL=2, --ignored": seen in debug at load 55 to 72, one process at a time of two: with ANALYZE by cost 222 s, seeks cheap 78, hash joins cheap 221, hash builds dear 258, hand-built at 1 259 and at 2 218; without by cost 103, hand-built at 1 and at 2 188 each, hash builds dear 99, hash joins cheap 97, seeks cheap 115 |
| 10 `..._facts_*` (4 settings without ANALYZE, 6 with; 300 runs each) | yes | "1.5 to 9 minutes each at CARGO_PROFILE_DEV_OPT_LEVEL=2, --ignored; longer in a debug build" (97 to 523 s of query time each in the optimized build at 4 processes) |

Run the 22 with `CARGO_PROFILE_DEV_OPT_LEVEL=2 cargo test -p amphora-engine --lib outer_joins_answer_as_sqlite -- --ignored` (set CARGO_PROFILE_DEV_OPT_LEVEL=2 so it does not take hours).
The 22 ran at opt-level 2 on b5760c39 (r1302, adacachy2, two at a time, load about 55): every one of the 12 middle tests passed in 5 to 16 s
(the by-cost, hand-built and pinned settings, with and without ANALYZE); all 10 fact tests passed: the eight not hand-built in 59 to 165 s each, the two hand-built with ANALYZE in 343 and 359 s. 0 wrong in each of the 22.
In a debug build (r1230, two at a time, load 55 to 72) all 12 middle tests passed, 78 to 259 s each.

## 5. Checks

| check | where | result |
|---|---|---|
| `cargo fmt --all --check` | local, b5760c39 | clean |
| clippy `-p amphora-engine --all-targets --locked -D warnings` | adacachy2, df20a9cb (r1215) and the merged b5760c39 (r1282) | exit 0, the log of each shows `Compiling amphora-engine v0.1.0` (r1215) or `Checking amphora-engine v0.1.0` (r1282) |
| the sweep's four small shards | adacachy2, b5760c39 (r1282) | 4 passed, 63.8 s at 3 threads |
| `translate::optimizer`, the sweep left out | adacachy2, df20a9cb (r1227) | 180 passed, 0 failed, 256 s at 3 threads |
| `--test planner` (simulator feature) | adacachy2, df20a9cb (r1226) | 13 passed, 1 failed: `a_whole_table_group_by_scans_and_sorts_where_lookups_would_read_pages` (planner.rs:182), main's own |
| `--test join_methods` | adacachy2, df20a9cb (r1226) | 4 passed, 1 ignored |
| `--test adaptive_joins` | adacachy2, df20a9cb (r1226) | 23 passed |
| check-production | adacachy2, df20a9cb (r1226) | exit 0: "10617 left in the engine's production code across 17 lints, as the ratchet holds" |
| check-production, merged b5760c39 | adacachy2 | exit 0 at 08:31Z: the same 10617, as the ratchet holds |

The change is test code in crates/engine/translate/optimizer/mod.rs alone; the merged tree took main ecaee231, whose crates moved since the first build (Postgres, backup, errors) are not the planner's.

## 6. Not this lane's, seen

- `--test planner` (planner.rs:182) stays red on main.
- The machine's room: 0 to 6 of 32 cores idle for most of the hour. The test rows' debug times are under load 55 to 72.
- Main changed `rewrite_or_terms` (lane's oracle finding CF, commits ec5a9ce7 and fcbdf0c2) after this lane's runs. The full matrix above is main 67cc7e69's
  engine; its landed tests are b5760c39. The middle and fact matrix was not re-run on it. The four gate tests passed on main ef8cb085, which has that change (r1337, a debug build, 70.8 s at 2 threads, `running 4 tests`).
- Lane B24 read pending output of three of my runs (r1226, r1227, r1230) by mistake. I read their logs on disk before counting any of them; the numbers above agree with the logs.
