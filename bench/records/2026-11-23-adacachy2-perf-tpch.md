# Lane T3 (tpch), 2026-11-23 12:27Z to 13:22Z: q05's wrong answer found and fixed; the slow plan not yet

Machine: adacachy2 (runner 00aad8da, `needs linux/amd64,cpus=8,mem=24g`), load average 18 to 80 on
32 cores from other lanes' work during the hour. Data: TPC-H SF0.1 from `bench/planner/tpch/dbgen.sh
0.1` (dbgen 32f1c1b9; lineitem 600,572 rows, orders 150,000), at `~/.cache/planner-bench/tpch-sf0.1`
there. Before this hour: lane T and T2's record, `2026-11-23-macbook-pro-2-perf-tpch.md`.

## The probe

`~/.cache/amphora-perf/tpch-t3/t3_q05.rs` on adacachy and adacachy2 (an engine example, release,
not committed): it loads the SF0.1 tables as the planner bench loads Amphora (its schema.sql with
REAL and TEXT, indexes.sql, ANALYZE), takes NestedLoopsOnly's q05 answer on a second connection,
then runs q05 again and again on one connection (the learned weights moving as they do in the
bench), printing EXPLAIN QUERY PLAN before each run and checking each answer against NestedLoopsOnly's.
Command on adacachy2: `RUNS=12 TO=20 JOBS=8 bash ~/.cache/amphora-perf/tpch-t3/probe.sh`.

## 1. The wrong answer (fixed: d94a1077)

At 364266aa (origin/main), the first run in a new process planned this and answered wrong, the
same wrong answer r414 gave on the MacBook Pro 2:

```
MATERIALIZE hash build input for orders      (customer scanned, orders hash-joined to it)
MATERIALIZE hash build input for nation      (supplier scanned, nation hash-joined to it)
HASH JOIN lineitem                           (probes orders' input)
HASH JOIN region                             (probes nation's input)
```

| commit | run 0 | answer |
|---|---|---|
| 364266aa | done, 4,906 ms | CHINA 163947441.6399, INDIA 145574909.0453, INDONESIA 138165060.1056, JAPAN 130872320.0269, VIETNAM 121097532.3995: **wrong** |
| 1bfe3b8f (the fix) | done, 4,939 ms, the same shape | CHINA 7822103.0, INDIA 6376121.5085, JAPAN 6000077.2184, INDONESIA 5580475.4027, VIETNAM 4497840.5466: **right** (NestedLoopsOnly's, and SQLite's in T2's record) |

One run each: a correctness check, not a timing.

The cause: `prune_join_order_for_materialized_inputs` (`translate/emitter/select.rs`) marked a WHERE
term consumed when the union of every key+payload input's prefix held its tables. Each input's
subplan runs only the terms its own prefix holds. `c_nationkey = s_nationkey` reads customer (in
orders' input) and supplier (in nation's input), so no subplan ran it, the main plan skipped it as
consumed, and every 1994 order's line with an Asian supplier counted, whatever its customer's
nation: the totals came out 23.1x high (30,276,617.6 against 699,657,263.1; one customer nation in
25 matches the supplier's). The fix consumes a term only when one
input's prefix holds it; the main plan runs the rest. Here it runs at region's loop:
`loop_index_for_table_row` maps supplier, a member of region's build group, to that loop, after
lineitem's probe has matched orders' input (customer's columns are in its payload).

Not done: a unit test that takes this shape. The PL.8 hooks (`bushy_test_hook::PIN` and `SPLIT`)
make one hash join and one build group; this shape needs a left-deep materialized input and a
build group in one plan. The successor's first item.

## 2. The slow plan (not fixed)

Every run after the first, before and after the fix, planned a bushy run and timed out at 20 s:

```
MATERIALIZE hash build input for lineitem
  HASH JOIN customer (build orders)
  SCAN lineitem
  SCAN orders
SCAN region / SEARCH nation USING INDEX nation_regionkey / HASH JOIN supplier (build lineitem)
```

FORMAT=JSON (r510, the plan after run 0) gives lineitem's step as `"type":"scan"` with
`input_rows 23,291`, `rows_per_input 4.0`, `access_cost 25,729`: about one scan of lineitem at the
learned weights, while the program scans lineitem once per (orders, customer) row (T2 counted it).
The operation is `Operation::Scan` (a `BTreeTable` access with no constraint), so the planner
priced a plain scan under loops as one scan. PL.6's whole-read refusal should keep a whole read out
from under loops that give more than one row when a keyed read exists (lineitem's autoindex by
`l_orderkey`), and `loops_give_at_most_one_row` says a hash join's probe can give more than one row.
Not yet found: where the run's lineitem step became a scan carrying a hash join's or one scan's cost
(the bushy search takes the run from the memo's plans of {orders, customer, lineitem}; it refuses a
run holding a materialized hash join). Next: print each run's `JoinN` data (table, access method
kind, cost) as the bushy search takes it.

Second plan change seen: run 0 plans on a cold cache (`page_miss` near 1), the runs after on a warm
one, and the plans differ (section 4).

## 3. The planner bench records every run (1bfe3b8f)

Each timed run now records the EXPLAIN QUERY PLAN FORMAT=JSON read on its connection just before
it (`runs[i].plan`), and under `--answers` writes its rows (`amphora.tsv`, then `amphora-run<i>.tsv`)
so `answers.py` holds every run's answer to the others'. Not built this hour (the bench compiles
bundled DuckDB); the successor's sweep builds it.

## 4. Plan stability (report, no change)

A prepare plans on the database's learned weights (`learned_costs.rs`): the cost of a seek, a hash
build and probe, a sort, in rows' visits, and `page_miss`, the share of the connection's recent page
fetches that read the file. Every timed run moves them, and `page_miss` moves from about 1 on a cold
cache to about 0.05 after one run (T2's states A and B), a 20x change in what a page costs against a
row. So one SQL text over one set of statistics gets a different plan from run to run. What a stable
plan for a given set of statistics would take:

- Plan from a snapshot of the weights with a version, not the live estimate: prepare reads the
  current version, and a new version is cut only when an estimate has moved past its own measured
  confidence interval (the samples are already counted per operation, `RunCounts`).
- Keep the cache state out of the plan's identity, or make it an input the plan is chosen for: a
  plan chosen at `page_miss` 1 and run warm is a different bet from one chosen warm.
- Record the weights' version with each plan (EXPLAIN and the bench's `runs[i].plan`), so a plan
  change names the weights that caused it.
- Test it: the same statement prepared twice over the same statistics and the same weights version
  gives the same plan.

## Checks and why the fix is not on main yet

- clippy (`cargo clippy -p amphora-engine --all-targets --locked -- -D warnings`) at 1bfe3b8f: clean.
- `cargo test -p amphora-engine --lib -- pl_8 hash_join materializ bushy t_qt_5` at 1bfe3b8f: 65
  passed, 1 failed, `t_pl_8_13_a_left_join_inside_a_bushy_run_keeps_its_null_rows`.
- That test, A/B, the same two test binaries alternated three times on adacachy2 at load 17 to 62:
  main 364266aa failed twice at mod.rs:4590 (`made_bushy > 0`, no bushy plan) and passed once; the
  fix failed three times at mod.rs:4591 (`in_runs > 0`, bushy but no LEFT JOIN in the run). Its
  asserts are plan shapes the learned weights choose after one timed run, so load moves them; but
  the fix failing at the other assert every time may mean it changes what the first run does in one
  of its queries. Not settled, so the fix stays on perf/tpch: the next step is that test with
  `--nocapture` on both binaries (`~/.cache/amphora-perf-bin/tpch/engine-lib-test-{1bfe3b8f,364266aa}`
  on adacachy2), reading which query fails and whether its first run's answer differs.
- The post-merge check at 0566dd00 (r604) did not start in the hour.

## Not done this hour

- The unit test for section 1; the cause and fix of section 2; the 22-query sweep against SQLite.

# Lane T4 (tpch), 2026-11-23 13:18Z to 14:13Z: the wrong-answer fix on main; the slow plan's cause found and fixed

Machine: adacachy2 (runner 00aad8da), `needs linux/amd64,cpus=8,mem=24g`, load average 15 to 1,189
on 32 cores from other lanes during the hour (kiln refused most named runs for want of 8 idle cores).

## 1. t_pl_8_13 settled; d94a1077 merged (main 74e832a2)

r668: T3's two saved test binaries, `t_pl_8_13 --nocapture`, alternated 3 times each:

| binary | runs | fails at | query |
|---|---|---|---|
| 364266aa (main) | 3/3 fail | mod.rs:4590 `made_bushy > 0` | the 7th, `... AND (s.id IS NULL OR s.cat + f2.k = 11)`, planned left-deep |
| 1bfe3b8f (fix) | 3/3 fail | mod.rs:4591 `in_runs > 0` | the 6th, `d2 LEFT JOIN d1 s ... AND s.id IS NULL`, bushy with the LEFT JOIN outside the run |

- The fix cannot change these queries' programs. Every plan printed in both arms holds at most one
  materialized build input, and with one input the fix's test (one input's prefix holds the term)
  is the old one (the union of the prefixes holds it). Every answer reached matched NestedLoopsOnly's.
- The per-binary split is calibration. Each executable reads back its own stored calibration
  (`$XDG_CACHE_HOME/amphora/calibration`, keyed by CPU and executable path), measured once at
  whatever load its first run met. r754, each binary on a fresh empty calibration cache, 3 rounds:
  round 1 fix at 4591 (4th query), main at 4590 (6th); rounds 2 and 3 both at 4590 (6th).
- The plan the test prints is not always the plan that ran: in r668's fix run 1, the 5th query's
  counters say its LEFT JOIN ran inside the run, and the EXPLAIN prepared right after shows the
  mirror plan. Each run moves the learned weights before the next prepare.
- Verdict: the test fails on main at whatever query the weights flip (6 of 6 runs here; J2's main
  baseline too). Section 3's versioned costs, with the test pinning a version, are its fix.

The unit test T3 asked for, `t_pl_8_2_a_term_across_two_build_inputs_runs_in_the_main_plan`
(24bfee35): q05's shape at small scale through the PIN hook, which now takes the hash joins'
positions (`PIN_HASH_JOINS`; none keeps the last table alone). r693 at 9539c215: it passes; with
d94a1077 reversed it fails, `[n3,90,27090; n6,60,17970; n9,60,18150]` against the nested loops'
`[n3,10,3300]`. Touched suites (`pl_8 hash_join materializ bushy t_qt_5`): 66 passed, 1 failed
(t_pl_8_13, as on main). Clippy (`-p amphora-engine --all-targets -D warnings`) clean.

## 2. q05's slow plan: the cause, and the fix (580b04ed)

The planner did not misprice it. It priced lineitem's step as a seek by `o_orderkey` on
`l_orderkey`'s autoindex for 23,291 input rows. PL.6 had nothing to refuse, since the planner
never chose a whole read. The emitter made the scan. A materialized build input's subplan
(`build_materialized_build_input_plan`, `translate/emitter/select.rs`) checked each
read's seek keys against its own loops (`join_prefix_mask`). In the run {orders, customer,
lineitem}, customer probes orders' hash table, so orders has no loop of its own. lineitem's seek
was judged to read a table outside the prefix and fell to `Operation::default_scan_for`. The
subplan's WHERE terms used the wider `included_tables` and ran `l_orderkey = o_orderkey` as a
filter, so the answer stayed right while the work became 23,291 scans of 600,572 rows. The
EXPLAIN JSON's `"type":"scan"` with `access_cost 25,729` was the rewritten op beside the seek's
price. The rewrite dates from the Turso import. It began to bite when PL.8's bushy runs could
hold a seek keyed by a hash-build-only table.

The fix: a probe's match positions its build table's cursor (`main_loop/hash.rs`), so a read
after the probe in the subplan's loops may key on the build table's columns. The seek stands and
the program does the seeks it was priced at. Test
`t_pl_8_2_a_seek_on_a_probed_build_table_stands_in_its_run`: the run `o c l` built for `s`'s hash
join, `c` probing `o`, `l` seeking by `o`'s key. It asserts the nested loops' answer and at most
each table and the run's rows read whole once (the statement's `FullscanStep` counter).

Checks. r782 at 580b04ed: touched suites 66 passed, 2 failed (t_pl_8_13, main's; and the new
test, whose first table was scanned, so `c`'s hash join was materialized and the run refused:
the test never formed the run). `--test nested_loops` 7/7 (it checks materialized chains);
clippy clean. r800 at f986aa0b (`r` holds 1,000 rows so `r.id = 1` seeks one row): the three
`t_pl_8_2_a_` tests pass; clippy clean; with the select.rs change reversed the new test fails
with 399,800 full-scan steps against a bound of 5,275, on q05's slow shape (`MATERIALIZE ...
HASH JOIN c / SCAN l / SCAN o`). Merged: main 3b7e8121.

Not measured yet: q05 at SF0.1 before and after. kiln refused the A/B four times, 14:00Z to
14:05Z, because adacachy2 had 2 to 12 of its 32 cores idle and the A/B asks for 16. Its script,
`~/.cache/amphora-perf/tpch-t4/q05_ab.sh` (on adacachy2 and adacachy), builds T3's probe at
74e832a2 (before) and 3b7e8121 (after) and alternates them for 3 rounds of 4 runs, with a 20 s
timeout. Each process gets a fresh calibration cache, and each run's answer is checked against
NestedLoopsOnly's. Load and CPU and I/O pressure are logged beside each run, and the logs go to
`~/.cache/amphora-perf/tpch-t4/logs/q05-ab-<sha>-<round>.log`. Run it with `on` 00aad8da and
needs `linux/amd64,cpus=16,mem=24g`.

## 3. Versioned learned costs: not started this hour

Where it goes, from reading `translate/optimizer/learned_costs.rs`. `LearnedCosts` holds the live
`Rates` under a mutex. Every prepare reads them through `weights_at(miss)`, and every timed run
moves them (`observe`, `observe_sampled`). The coordinator's design maps onto it like this:
- Cut versions from the rates the samplers already keep. Each `Sampling` holds a Welford `Spread`,
  which gives a standard error. A new snapshot (`Arc<LearnedWeights>` plus a version number) is
  cut only when a rate's mean has moved past its own error.
- `weights_at` reads the current snapshot, and the version goes into EXPLAIN and the bench's
  `runs[i].plan`.
- `page_miss` is per connection and live: about 1 on a cold cache and 0.05 warm (T3, section 4).
  It needs its own versioning, or it becomes an input the plan is chosen for. Otherwise one
  statement over one set of statistics still gets two plans.
- Tests pin a version. That ends t_pl_8_13's flakiness (section 1). Calibration is stored per
  executable, so two builds of the same code start from different weights; a pinned version
  removes that too.
- A/B against live costs: plan variance and p50/p99/p99.9 over repeated TPC-H SF0.1 and JOB runs.

## 4. The bench change (1bfe3b8f) and the 22-query sweep: not started this hour

# Lane T5 (tpch), 2026-11-23 14:12Z to 15:07Z: q05 measured against SQLite and DuckDB; not fixed to the bar

Machine: adacachy2 (runner 00aad8da). The builds asked `needs linux/amd64,cpus=8,mem=24g` and the
timed run `cpus=12,mem=24g`. During the timed run the load average was 51 to 101 on 32 cores and
CPU pressure (avg10) 3% to 60%. Every run's load is in its record, and a 10 s sampler wrote the
load and CPU, I/O and memory pressure to `~/.cache/amphora-perf/tpch-t5/out/monitor.log` there.
Kiln refused four of my named runs for want of 8 or 12 idle cores (0 to 7 idle).

## 1. The q05 A/B (r898)

Arms: A = d4d0e096 (main just before 74e832a2, so neither fix), B = b122f970 (main with both
fixes). Client: the planner bench (`amphora-bench --planner`) built at each arm, so Amphora,
SQLite (rusqlite 0.40.2, bundled) and DuckDB (crate 1.20001.0, bundled, `threads = 1`) run through
one client on one machine. Data: dbgen SF0.1, loaded once by B into `tpch-t5/db-sf0.1-B` and
reflink-copied for A. Each round is one process per arm: the bench's counted warm-up, then 3
timed runs, each run's plan read just before it and its answer kept. 5 rounds alternated (A,B /
B,A / ...). Timeouts: 10 s a timed run, 20 s the counted run (30 s at SF1). Script:
`~/.cache/amphora-perf/tpch-t5/ab.sh` (adacachy and adacachy2).

Calibration (RULES 5e). The store keys a calibration by the executable's path, length and mtime
(`learned_costs.rs`, `stored::Key::here`), so one shared `XDG_CACHE_HOME` still gives each binary
its own: each measures at its first open. The script measures once with B, then writes B's
measured body under A's key too. Both files' bodies hash to sha256 f83d91b5ed2e5f30. Rule 5e as
written does not give two builds one calibration.

### SF0.1, Amphora's timed runs, ms (status: done unless marked)

| round | load at start | A = d4d0e096 | B = b122f970 |
|---|---|---|---|
| 1 | 72 / 74 | timed out (10 s) | timed out (10 s) |
| 2 | 90 / 101 | 1,240 right; 2nd timed out | 1,727, 307, 784 |
| 3 | 100 / 84 | 1,486 right; 2nd timed out | 9,274, 513, 470 |
| 4 | 79 / 79 | 5,857 **wrong**; 2nd timed out | 1,413, 325, 1,086 |
| 5 | 62 / 52 | 5,239 **wrong**; 2nd timed out | 760, 234, 263 |

- B answered right on every run that finished (12 of 12). `answers.py` held every run's answer to
  SQLite's and DuckDB's. A's rounds 4 and 5 gave T3's wrong answer (CHINA 163947441.6399 against
  7822103.0).
- B's plan changes from run to run. Its 13 runs: median 760 ms, range 234 ms to over 10 s. Round 1
  (the first process on the freshly loaded database) counted on a left-deep plan of seeks (1,344
  ms), then planned T3's shape, `MATERIALIZE hash build input for orders / HASH JOIN orders / SCAN
  customer / MATERIALIZE hash build input for nation / HASH JOIN nation / SCAN supplier / HASH JOIN
  lineitem`, and passed 10 s. 74e832a2 made that shape answer right. It is still slow.
- SQLite: 111, 111, 116, 119, 112 ms (median 112). DuckDB: 46, 44, 44, 46, 44 ms (median 44).
  Both ran right after B's round 5, at load 52 and CPU pressure 3%, the quietest point of the
  run. B's own round 5 ran under the same conditions: 760, 234, 263 ms.
- Ratios at SF0.1 (B's median over its runs against each engine's median): 6.8x slower than
  SQLite and 17x slower than DuckDB. At B's best run (234 ms): 2.1x and 5.3x slower. The bar is
  10x faster than both.

Verdict at SF0.1: right answers on main now, but q05 is not fixed. The run-to-run plan changes (the
learned weights move between prepares) give a 40x spread, and even the best run is 2x behind
SQLite. The limiter is the plan choice, not a slow operator. The same binary goes from 234 ms to
over 10 s on the same data, with nothing changed but its learned weights. Section 2 is the fix the
coordinator chose for that.

### SF1 (loaded by B in 178 s), ms

| round | A counted | A timed | B counted | B timed |
|---|---|---|---|---|
| 1 | 19,034 | timed out (10 s) | 25,267 | timed out (10 s) |
| 2 | 2,991 | timed out | 2,661 | timed out |
| 3 | 2,273 | timed out | 4,227 | timed out |
| 4 | 1,479 | timed out | 4,828 | timed out |
| 5 | 3,216 | timed out | 2,763 | timed out |

- SQLite: 1,701, 1,735, 1,731, 1,700, 1,714 ms (median 1,714). DuckDB: 253, 195, 259, 268, 221 ms
  (median 253). They ran right after round 5, at load 29 to 33. They agree on the answer. No
  Amphora timed run finished, so no Amphora answer at SF1 was compared.
- The counted run is the bench's warm-up, timed with row counters on. B's took 2.7 to 4.8 s (25 s
  in the first process on the fresh database), with seek-based plans (r2: `MATERIALIZE ... supplier
  / HASH JOIN nation / ... / HASH JOIN lineitem / SEARCH orders / SEARCH customer`; r5: seeks
  from supplier through lineitem, orders and customer).
- Every B timed run, after the counted run moved the weights, planned one shape, 5 of 5:
  `MATERIALIZE hash build input for orders / HASH JOIN orders / SCAN customer USING COVERING INDEX
  customer_nationkey / MATERIALIZE hash build input for nation / HASH JOIN nation / SCAN supplier
  USING COVERING INDEX supplier_nationkey / HASH JOIN lineitem / HASH JOIN region`. It is T3's
  two-input shape. Since 74e832a2 it answers right (`c_nationkey = s_nationkey` runs in the main
  plan, at region's loop), and at SF1 it passes 10 s.
- Verdict at SF1: not back to October's 636 to 661 ms. Every timed run passed 10 s, over 15x
  October's, over 5.8x SQLite's 1,714 ms and over 40x DuckDB's 253 ms. 3b7e8121 fixed the
  23,291 scans of lineitem in one bushy run. This is another plan.
- Guess, not measured: the planner prices this shape as if `c_nationkey = s_nationkey` cut the
  rows where the old code consumed it. Since 74e832a2 the term runs only after lineitem's probe of
  orders' input, so every 1994 order's line with an Asian supplier flows through it (without the
  term, T3's totals came out 23.1x high: that many more rows reach region's loop than the answer keeps). The next step is the counted run of this plan (`--count-
  timeout-s` past its run), set against its EXPLAIN estimates at lineitem's and region's steps.

## 2. Versioned learned costs: what the code does today (no code this hour)

From reading `translate/optimizer/learned_costs.rs` and `translate/mod.rs` at b122f970. These are
the premises a versioned snapshot rests on:

- Under the default rule (`WeightRule::BatchMeans`), a standing rate already moves only when a
  closed batch sits past the spread between batch means (`Batches::refutes`, Student's t at
  `MOVE_Z`). So the rates are versioned in effect. Nothing numbers the versions or records them
  with a plan.
- With one exception, and it is the one the bench and t_pl_8_13 hit. `Batches::judge` returns
  true when no batch has closed yet (`_ => true`), so each kind's first closed batch replaces
  calibration's rate unjudged. In a fresh database, that happens during the first statements.
  The counted warm-up moves the weights, and the timed runs plan on others (q05's round 1 above:
  a seek plan counted, then a materialized plan timed). For "a new version only past the measured
  error" to hold from the start, the first batch must be judged too. Calibration times several
  samples and keeps the median (`measure_calibration`), so their spread can seed `Batches`. That
  is the first change.
- `page_miss` is read live per connection at every prepare (`translate/mod.rs`,
  `weights_at(page_miss)`). It is about 1 on a cold cache and 0.05 warm (T3). It needs its own
  version, cut when the ratio moves past its binomial error over the pager's window, or it must
  become an input the plan is chosen for.
- The only pin, `Connection::pin_cost_weights`, holds 3 of the 8 weights (seek, hash build, hash
  probe), behind the `simulator` feature. A test pin of a whole version means a full
  `LearnedWeights` pin, with the three callers migrated (statement.rs, tests/adaptive_joins.rs,
  optimizer/mod.rs).
- t_pl_8_13's database is in memory (`facts()`, `MemoryIO`), so `page_miss` is not what moves
  there. The rates are: `facts()`'s inserts and ANALYZE and each query's first run close batches
  before the second prepare. Untested premise: that the asserts (bushy plan, LEFT JOIN inside the
  run) hold at the version pinned from the test's start. T4's r754 saw the 6th query fail on fresh
  calibration caches in 2 of 3 rounds, so the pinned version may fail it too. The first
  experiment: pin the whole `LearnedWeights` read after `facts()` for every prepare in the test,
  then run it 6 times. If it fails the same way every time, the assert and the cost model disagree,
  and that is a decision, not flakiness.

## 3. The bench change (1bfe3b8f) built, and the 22-query sweep started (r898)

- `amphora-bench` at main b122f970 did not build: f8f65110 made `amphora` depend on hyper-rt,
  and the bench's own `[patch.crates-io]` lacked it, so it took crates.io's hyper-rt 0.1.0 (r867,
  E0432/E0433). The guard `crates/amphora/tests/bench_workspace.rs` fails on main for the same
  reason. I fixed it on perf/tpch (ce2b0a09); main got the identical line meanwhile (7f2650a9).
  Both arms built with that line (r893: B in 44 s, A in 85 s, both in the lane's one target dir,
  CARGO_INCREMENTAL=0).
- The plan recorded before every timed run works: each run's `plan` is in
  `tpch-t5/out/q05-*.jsonl` at B. A's runs carry none: 1bfe3b8f reached main with 74e832a2, after d4d0e096.
- The sweep: all 22 queries at SF1 on amphora, sqlite and duckdb, 3 timed runs each after the
  warm-up, 300 s timeouts, every run's answer written and held to the others' by `answers.py`.
  It started at 14:54:27Z in r898, at load 33 and CPU pressure 9%, and was still running at the end of the hour.
  Its record is `~/.cache/amphora-perf/tpch-t5/out/sweep-sf1.jsonl` on adacachy2, its answers
  `tpch-t5/ans/sweep`, and r898's output ends with the `answers.py` verdict. The table, losses
  first, is the successor's first item.

# Lane T6 (tpch), 2026-11-23 15:14Z to 16:14Z: q05's SF1 plan priced as it runs; the 22-query table

Machine: adacachy2 (runner 00aad8da). Builds and tests asked `needs linux/amd64,cpus=8,mem=24g`,
the timed A/B `cpus=12,mem=24g`, every run named onto adacachy2 (`on`).

## 1. q05's cost trace at SF1, and the fix (579d22e4)

Every B timed run at SF1 in T5's r898 planned one bushy pair: the run {supplier, nation}, built
for region's hash join, under the outer loops {customer, orders, lineitem} (customer and orders
materialized for lineitem's hash join). The planner's estimates are that plan's EXPLAIN FORMAT=JSON
(`tpch-t5/out/q05-sf1-b122f970-r1.jsonl`). The actual rows are counted by SQL on the bench's SF1
SQLite database (`tpch-t5/db-sf1-B/sqlite/bench.db`, r971), one count per step of the plan as the
emitter runs it:

| step | estimated rows | actual rows |
|---|---|---|
| orders' build input (customer x orders in 1994) | 191,894 | 227,597 |
| lineitem's probe of it (the outer rows) | 767,733 | 910,519 |
| the run {supplier, nation} | 10,000 | 10,000 |
| region (`r_name = 'ASIA'`) | 1 | 1 |
| region's match: the rows its hash join visits | 6,141 | 1,823,769,557 (910,519 x 2,003 suppliers in ASIA) |
| after `l_suppkey = s_suppkey` | | 182,183 |
| after `c_nationkey = s_nationkey` (the join's rows) | 6,141 | 7,243 |

T5's guess holds, and the gap is wider than the nation match. The bushy pair's cost
(`optimizer/join.rs`) priced the probe's visits at the whole join's rows, as if
`l_suppkey = s_suppkey` and `c_nationkey = s_nationkey` cut the match. The emitter runs both
after it, at region's loop: no input's prefix holds either term
(`prune_join_order_for_materialized_inputs`), and region's hash join is keyed by
`n_regionkey = r_regionkey` alone. So the match visits each outer row times the run's rows in
ASIA, 297,000x the estimate. At 10 to 20 ns a visit (a guess, not measured) that is 18 to 36 s,
which would fit every SF1 timed run passing 10 s.

The fix prices the match where the emitter places the terms (579d22e4, b876567c). Where a WHERE
term reads both the run and the outer loops, the match visits rows(outer + probe) x
rows(run + probe) / rows(probe), never fewer than the join's rows, so those terms count only
after the match. A pair with no such term keeps the join's rows as its visits, as before. For
this plan that is 767,733 x 2,000 / 1 = 1.54e9 estimated visits, against 1.82e9 counted.

Test: `t6_q05_terms_from_a_run_to_the_outer_loops_are_priced_after_its_match` (optimizer/mod.rs).
It runs q05's shape at small scale by cost (6 tables, 60,000 lines, 500 suppliers in ASIA). It
asserts the nested loops' answer, and fewer VM steps than the late match alone visits (the outer
rows times the run's rows in ASIA). That is a count of work, not a time.

Checks: r1042 at b876567c on adacachy2 (load 62 to 71):
- clippy (`cargo clippy -p amphora-engine --all-targets --locked -- -D warnings`): clean.
- `cargo test -p amphora-engine --lib -- t6_q05 pl_8 hash_join materializ bushy t_qt_5`: 68 passed,
  1 failed. The failure is `t_pl_8_13_a_left_join_inside_a_bushy_run_keeps_its_null_rows`, "not
  planned bushy" at its 7th query, which fails the same way on main (T4's r668 and r754).
- `--test nested_loops`: 7 of 7. At 579d22e4 (r994), `a_materialized_chain_leaves_its_prefix_out_of_the_main_plan`
  failed once ("MATERIALIZE" missing from a 3-table plan). The change cannot reach it: a bushy pair
  needs a run of 2 tables or more plus a probe and an outer table, so 3 tables make no pair. It
  passed at b876567c and on main with the test (a9ca008d).
- The new test is not a regression guard. On main with the test alone (a9ca008d, r1042) it
  passes too: at this scale the old cost does not choose the late match. It holds the work bound
  for whatever plan is chosen, nothing more.

## 2. q05 before and after, against SQLite and DuckDB

r1006, `~/.cache/amphora-perf/tpch-t6/ab.sh` on adacachy2 (needs `cpus=12,mem=24g`). Arms:
A = main 952f5766, B = 579d22e4 (the fix; b876567c changes only plans with no late term, so q05's
pair is priced the same). Both are release builds of `amphora-bench --planner` in the lane's one
target dir. Data: T5's loaded SF0.1 and SF1 directories, reflink-copied per arm. Calibration per
RULES 5e: each arm opened once, then B's body was written under A's key (both hash to sha256
e611e52b204e77f7). 5 rounds alternated, each a process: the counted warm-up, then 3 timed runs
(10 s timeout; the bench ends a process at its first timeout). SQLite and DuckDB ran 5 times
each, right after. Every finished run's answer went to `answers.py`: 0 differ at SF0.1 and SF1.
The load average was 41 to 51 at SF0.1 and 52 to 106 at SF1, with CPU pressure (avg10) 45% to 65%
at SF1. That is a loaded machine, and every engine's SF1 times here are above T5's.

SF1, ms:

| arm | processes that finished | timed runs | median | range | plan of the runs |
|---|---|---|---|---|---|
| A (main) | 0 of 5 | 5 timed out | over 10,000 | | 5 of 5 the late-match bushy pair |
| B (fix) | 4 of 5 | 12 done, 1 timed out | 3,439 (13 runs, the timeout counted as over 10 s) | 1,129 to 5,438, and one over 10 s | 8 seek chains (6 `SCAN orders / SEARCH customer / SEARCH lineitem / SEARCH supplier ... (s_nationkey=?) / SCAN region / HASH JOIN nation`, 2 with customer by hash join), 4 hash-then-seek (`HASH JOIN lineitem / SEARCH supplier ... (s_nationkey=?)`), 1 late-match pair |
| SQLite | | 5 | 3,708 | 2,596 to 4,811 | |
| DuckDB | | 5 | 189 | 180 to 235 | |

- B's plans apply the nation match early in 12 of 13 runs: supplier is read by
  `supplier_nationkey` keyed on the customer's nation. That is October's keyed-seek chain, with
  the region and nation hash join last.
- Not October's 636 to 661 ms. B's best run was 1,129 ms, its median 3.4 s, about SQLite's
  3.7 s measured beside it under the same load, and 18x DuckDB's.
- One B process (round 3) still chose the late-match pair and timed out. Its plan JSON prices
  region's step at 620,439 (A's same plan: 199,821), so the fix took effect, and the plan cost
  1,209,924 in all. In round 2 the seek chain the planner took cost 595,717. Round 3's counted
  run took 6,930 ms, the slowest of B's. After it, the learned weights put every other plan above
  1.21M. A visit is priced at cpu_cost_per_row, about 0.00027 here (420,618 / 1.54e9). At that
  rate 1.82e9 visits rank only 2x above a chain that runs 10x faster or more. The visit runs two
  filters on the payload's columns, and the model charges them nothing (`ready_where_work` counts
  only terms with extra steps). The next lane's first item: price a late term's steps per visited
  row, then measure again.

SF0.1, ms:

| arm | timed runs | median | range |
|---|---|---|---|
| A (main) | 15 done | 452 | 215 to 1,461 |
| B (fix) | 15 done | 390 | 220 to 3,513 |
| SQLite | 5 | 169 | 121 to 180 |
| DuckDB | 5 | 296 | 240 to 378 |

- No measurable difference at SF0.1: the ranges overlap almost entirely, and both arms change
  plan from run to run (2 to 3 distinct plans per process in both). Both are behind SQLite
  (2.3x to 2.7x at the median).

## 3. Versioned learned costs: not started this hour

What the successor needs, from reading the code at 579d22e4:
- The pin (`Connection::pin_cost_weights`, simulator feature) takes 3 of the 8 weights, and
  `translate/mod.rs` fills the other 5 from the live `weights_at(page_miss)` at every prepare.
  It has four callers: `statement.rs` (order flips), `optimizer/mod.rs`
  (`t_pl_8_2_the_bushy_plan_is_cheapest_at_every_corner_of_the_weights`), and
  `tests/adaptive_joins.rs` twice (lines 66, 455). That last one is an integration test, so a pin
  of all 8 needs a public weight type behind the simulator feature: `LearnedWeights` is
  `pub(crate)`.
- T5's experiment (the whole set pinned at t_pl_8_13's start, 6 runs) needs that pin first.

## 4. The 22 queries at SF1 (T5's r898 sweep), losses first

All 22 at SF1, one process: each engine's counted warm-up, then 3 timed runs per query.
Medians in ms. "against the faster" is Amphora against the faster of SQLite and DuckDB. The load
average was 38 to 99 on 32 cores during Amphora's runs (per run in `sweep-sf1.jsonl`). It is one
process per engine with no alternation, so read a ratio under 2x as noise. answers.py: 22 queries
compared, 0 differ. Amphora's answers match SQLite's and DuckDB's on every query.

| query | Amphora ms (median, range) | SQLite ms | DuckDB ms | Amphora against SQLite | against DuckDB | against the faster | answers |
|---|---|---|---|---|---|---|---|
| q19 | 577 (575 to 3,426) | 33.6 | 106 | 17.2x slower | 5.5x slower | 17.2x slower | same |
| q03 | 2,330 (1,494 to 3,078) | 681 | 149 | 3.4x slower | 15.6x slower | 15.6x slower | same |
| q06 | 652 (521 to 884) | 314 | 57.3 | 2.1x slower | 11.4x slower | 11.4x slower | same |
| q01 | 3,578 (2,959 to 3,770) | 5,431 | 332 | 1.5x faster | 10.8x slower | 10.8x slower | same |
| q14 | 683 (628 to 2,258) | 7,453 | 75.7 | 10.9x faster | 9.0x slower | 9.0x slower | same |
| q09 | 2,116 (1,244 to 2,311) | 1,402 | 268 | 1.5x slower | 7.9x slower | 7.9x slower | same |
| q05 | 1,589 (1,191 to 1,976) | 2,579 | 227 | 1.6x faster | 7.0x slower | 7.0x slower | same |
| q22 | 288 (284 to 328) | 43.9 | 125 | 6.6x slower | 2.3x slower | 6.6x slower | same |
| q07 | 2,035 (1,146 to 3,156) | 9,771 | 363 | 4.8x faster | 5.6x slower | 5.6x slower | same |
| q21 | 1,004 (869 to 1,107) | 428 | 186 | 2.3x slower | 5.4x slower | 5.4x slower | same |
| q12 | 669 (608 to 705) | 769 | 130 | 1.1x faster | 5.2x slower | 5.2x slower | same |
| q10 | 807 (647 to 831) | 1,215 | 159 | 1.5x faster | 5.1x slower | 5.1x slower | same |
| q20 | 277 (277 to 715) | 71.7 | 176 | 3.9x slower | 1.6x slower | 3.9x slower | same |
| q18 | 3,121 (2,410 to 3,223) | 839 | 58,626 | 3.7x slower | 18.8x faster | 3.7x slower | same |
| q04 | 255 (250 to 325) | 154 | 74.3 | 1.7x slower | 3.4x slower | 3.4x slower | same |
| q11 | 94.1 (73.8 to 110) | 35.6 | 104 | 2.6x slower | 1.1x faster | 2.6x slower | same |
| q15 | 1,893 (1,839 to 2,294) | 7,974 | 788 | 4.2x faster | 2.4x slower | 2.4x slower | same |
| q13 | 2,014 (1,913 to 2,058) | 1,219 | 900 | 1.7x slower | 2.2x slower | 2.2x slower | same |
| q17 | 149 (142 to 153) | 85.7 | 100 | 1.7x slower | 1.5x slower | 1.7x slower | same |
| q16 | 163 (154 to 248) | 95.8 | 482 | 1.7x slower | 3.0x faster | 1.7x slower | same |
| q08 | 166 (164 to 206) | 12,066 | 250 | 72.7x faster | 1.5x faster | 1.5x faster | same |
| q02 | 25.7 (23.6 to 34.4) | 136 | 47.4 | 5.3x faster | 1.8x faster | 1.8x faster | same |

- Against the bar (10x faster than both): 0 of 22. Faster than both: 2 of 22 (q08, q02).
- q05 here ran 1,589 ms (1,191 to 1,976) with another plan, while every SF1 timed run in T5's
  q05-only A/B passed 10 s. In one process after 4 other queries, the learned weights stood
  elsewhere and q05 planned differently. That is section 3's instability in the sweep itself.
