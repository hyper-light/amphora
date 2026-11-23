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

## Not done this hour

- The unit test for section 1; the cause and fix of section 2; the 22-query sweep against SQLite.
