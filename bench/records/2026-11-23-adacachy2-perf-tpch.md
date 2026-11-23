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
