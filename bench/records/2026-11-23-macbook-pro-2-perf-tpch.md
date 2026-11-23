# Lane T (tpch), hour 1: TPC-H q05's loss is a regression after 2026-10-29, not a tuning gap

Lane M's record (2026-11-23-mac-studio-d1323898-perf-analytics.md) found q05 at SF1 unfinished in
1800 s on amphora and amphora-adaptive at head 2d939193, against SQLite 1.73 s and DuckDB (one
thread) 0.10 s. This hour sets the regression window and leaves the SF0.1 reproduction queued for
the successor. No fix and no sweep yet.

## The same query ran in 0.64 s at 26657fdf

`bench/records/2026-10-26-jupiter-planner-tpch-sf1-e.jsonl` (jupiter, Windows, 32 cores, commit
26657fdf, 2026-10-29, `--runs 3`), q05 at SF1, one thread each:

| engine | runs (ms) |
|---|---|
| amphora | 660.7, 645.2, 636.1 |
| sqlite | 3436.9, 3343.2, 3350.1 |
| duckdb (1 thread) | 241.5, 220.8, 221.9 |
| duckdb-mt | 103.1, 101.7, 100.2 |

Amphora then beat SQLite by 5.3x. Its plan (EXPLAIN QUERY PLAN FORMAT=JSON in that record), with
the rows each step counted:

| step | access | estimate | rows counted |
|---|---|---|---|
| orders | SCAN (date range as filter) | 7,500 | 227,597 |
| customer | SEARCH rowid=? | 7,500 | 227,597 |
| nation | SEARCH rowid=? | 7,500 | 227,597 |
| region | SEARCH rowid=? | 750 | 46,008 |
| lineitem | SEARCH sqlite_autoindex_lineitem_1 (l_orderkey=?) | 3,750 | 184,082 |
| supplier | SEARCH rowid=? (s_nationkey = c_nationkey checked on the row) | 6 | 7,243 |

The largest intermediate result is 227,597 rows: no cross product, every join on a key. The
cycle `c_nationkey = s_nationkey` is checked on the supplier row the `l_suppkey = s_suppkey`
lookup found.

## The window

- `git log 2d939193..origin/main -- crates/engine/translate crates/engine/vdbe` is empty: current
  main (28fa609c) plans and runs q05 as 2d939193 did, so lane M's loss stands on main.
- `git log --no-merges 26657fdf..2d939193 -- crates/engine/translate/optimizer
  crates/engine/translate/main_loop.rs` holds 165 commits. Those touching the join search and
  estimates include the bushy plans (PL.8, aca73004 and after), the degree-sequence bounds (PL.11,
  ec0bf7ba), a seek and a hash join weighed by cost alone with the `rhs_has_selective_seek` gate
  removed (QT.5.8, 9422adcb), pages priced by cache misses (c3e83e5f), and no table read whole
  beneath loops (PL.6, 2084ac96). None is named as the cause yet: that needs the new plan.

## Reproduction (queued, not finished this hour)

On the MacBook Pro 2 (runner e30189ea, `needs macos/arm64,cpus=4,mem=16g`, load average 34 to 61
on 18 cores during the hour), in `~/.cache/amphora-perf/tpch-remote` at origin/main 28fa609c:

- TPC-H SF0.1 is generated at `~/.cache/planner-bench/tpch-sf0.1` (dbgen 32f1c1b9; lineitem
  600,572 rows, orders 150,000, customer 15,000, supplier 1,000).
- `amphora-bench` (bench workspace, release) was still compiling bundled DuckDB after 45 minutes
  at 4 jobs on the loaded machine; target dir `~/.cache/amphora-perf-target/tpch`.
- The q05 run is queued behind it: `q.sh origin/main q05 4 300 q05-main` (lane T's script, kept at
  `~/.cache/amphora-perf/tpch-scripts/q.sh` on adacachy and the Mac). It runs the planner bench
  with `--targets amphora,sqlite --runs 4 --timeout-s 300 --count-timeout-s 300 --answers`, prints
  Amphora's plan and the rows each step counted beside SQLite's EXPLAIN QUERY PLAN, and compares
  answers. Its output lands in `~/.cache/amphora-perf/tpch-out/q05-main/` on the Mac.

It reproduces at SF0.1 (inferred from the run, not yet read from a record): run r414 at origin/main
78fa9fdb loaded both engines (SQLite's lineitem in 3.0 s, its indexes and statistics in 2.3 s) and
Amphora's counted q05 was still running 4 min 43 s into the process at 05:43Z. The October plan's
work at SF0.1 is about a tenth of SF1's 0.64 s. The record (plan and the rows counted at the
300 s deadline) lands in `~/.cache/amphora-perf/tpch-out/q05-main/rec.jsonl` on the Mac when r414
ends.

## Next

1. Read q05's plan and counts at SF0.1 from that run against the October plan above. The step
   whose rows explode names the cause.
2. If the plan only goes wrong at SF1, rerun at SF1 with `--count-timeout-s` short and read the
   plan alone (`record["plan"]` is written before the counted run).
3. Bisect the 165 commits by plan shape, not time: EXPLAIN QUERY PLAN needs no run.

## Lane T2, hour 1 (2026-11-23 05:46Z to 06:40Z): what q05 does at SF0.1 now

Machine: the MacBook Pro 2 (runner e30189ea, `needs macos/arm64,cpus=4,mem=16g`), load average
34 to 81 on 18 cores from other lanes' work all hour. Binary: origin/main 78fa9fdb (no engine change
since: `git log 78fa9fdb..origin/main -- crates/engine` is empty). Data: the SF0.1 databases r414
loaded, `~/.cache/planner-bench/db-tpch-sf0.1-78fa9fdb`. Probe: a throwaway example
(`bench/examples/q05_plans.rs`, not committed) that opens Amphora's copy, prints EXPLAIN QUERY PLAN
(text and FORMAT=JSON) before each run, runs q05 with row counters on, and prints the answer and the
rows each step counted. No time below is a benchmark: the machine was loaded; the rows are the facts.

### 1. r414 (the planner bench, `q.sh 78fa9fdb q05 4 300 q05-main`)

| run | plan | time | answer |
|---|---|---|---|
| counted (row counters on) | region, nation, supplier by `s_nationkey`, lineitem by `l_suppkey`, orders by rowid, customer by `c_nationkey` | 588 ms | right |
| timed 1 | not recorded (the bench prints the plan once, before the counted run) | 22,147 ms | **wrong**: `CHINA 163947441.6399`, SQLite `CHINA 7822103.0` (21x) |
| timed 2 | not recorded | timed out at 300 s | none |

SQLite: 457 to 722 ms over 4 runs, answer `CHINA 7822103.0, INDIA 6376121.5085, JAPAN 6000077.2184,
INDONESIA 5580475.4027, VIETNAM 4497840.5466`.

### 2. q05's plan is not one plan: it changes from run to run in one process

Each prepare plans on the learned cost weights (`learned_costs.rs`, QT.5), which every run moves.
Six runs in one process (r430), plans read before each:

| run | plan (join order and method) | rows counted per step | time | answer |
|---|---|---|---|---|
| 0 | bushy: (region, nation hash, supplier by `s_nationkey`) built; lineitem hash-probed; orders by rowid; customer by `c_nationkey` | 1, 5, 225, lineitem 134,374, orders 20,499, customer 865 | 1.5 s | right |
| 1 | hash joins: lineitem scan built, orders probe, then customer, supplier, nation, region | 600,572, 92,293, 92,293, 3,690, 3,690, 865 | 5.0 s | right |
| 2 | bushy: lineitem built, orders, supplier, nation, region hash; customer by `c_nationkey` | 600,572, 92,293, 92,293, 92,293, 20,499, 865 | 16.9 s | right |
| 3, 4, 5 | bushy: run (orders built, customer hash-probes it, **lineitem SCAN**) materialized, then supplier, nation, region hash | orders 22,958; customer 206; **lineitem 809 after 25 s**, then timeout | >25 s | none |
| NLJ 0, 1 (`JoinMethods::NestedLoopsOnly`) | lineitem scan, orders rowid, supplier rowid, customer `c_nationkey`, nation, region; then region-first seeks | largest step 134,374 | 0.87 s, 1.06 s | right |

The October plan (26657fdf, SF1): orders scan 227,597, customer by rowid, nation by rowid, region by
rowid 46,008, lineitem by `l_orderkey` 184,082, supplier by rowid 7,243. No plan seen today reads
lineitem by its key (`sqlite_autoindex_lineitem_1`, `l_orderkey=?`).

### 3. The step that explodes

Plan 3's lineitem step: `SCAN lineitem`, inside the bushy run, after orders and customer, with
`l_orderkey = o_orderkey` tested on each row. Its estimate (FORMAT=JSON, r433): input 23,291 rows,
4.0 rows each, output 93,253, **access cost 16,527** (24,208 one run earlier). It runs a whole scan
of lineitem's 600,572 rows for each of the 23,291 (orders, customer) rows: 1.4e10 row visits at
SF0.1, about 0.12 s each outer row on this machine (206 rows reached customer and 809 passed
lineitem in 25 s), so about 45 minutes at SF0.1 and 100x that at SF1: the >1800 s of the SF1 record.

### 4. Why the planner takes it (a temporary trace, r435)

A throwaway `eprintln!` in `join_lhs_and_rhs` (Mac copy only, reverted after the build; binary
`~/.cache/amphora-perf-bin/tpch/q05_plans_trace`, `Q05_TRACE=1`) printed lineitem's choices with
orders and customer before it (input 23,291 rows):

| learned state | seek by `l_orderkey` (rows 5 each) | hash join, build = orders and customer materialized, probe lineitem (rows 4 each) | chosen |
|---|---|---|---|
| A: `cpu_cost_per_row` 2.69e-4, `page_miss` 0.988 | 23,092.7 | 21,484.8 | hash join |
| B: `cpu_cost_per_row` 5.98e-6, `page_miss` 0.048 | 1,754.0 | 1,012.1 | hash join |

PL.6's whole-read refusal does not fire (`refused=false`: the first choice is already the keyed
seek); the hash join then wins by cost alone (QT.5.8, 9422adcb removed the
`rhs_has_selective_seek` gate that kept the seek, the October plan's step).

The hash join is priced as one scan of lineitem: with a materialized build input,
`hash_probe_multiplier` is 1.0 (`join.rs`, the `should_materialize` branch), on the premise that the
prefix (orders, customer) is materialized before lineitem is read. The run does not do that: in 25 s,
206 rows passed customer and 809 passed lineitem (about 4 a customer row, lineitem's matches), while
the build of the next join (`HashBuild lineitem`) held 0 rows. Lineitem's probe scan runs inside the
loops that make its build input, once a (orders, customer) row, not once. Here the build table
(customer) is itself the probe of the hash join built from orders (`build_table_is_prior_probe`, one
of `needs_materialization`'s reasons). So the cost the planner compares (one scan) and the work the
program does (a scan a prefix row) disagree for this shape; 9422adcb made the comparison decide by
that cost, and the learned weights decide on which run it flips.

Not yet shown, for the next hour: the emitter's code for a materialized build input whose build
table is a prior hash join's probe (`translate/main_loop`, `emitter/select.rs`): whether it nests
the probe loop inside the build input's loops. A unit test can show it without data or time:
three tables a, b, c with `a.k = b.k` and `b.j = c.j` equalities as q05's orders, customer and
lineitem, `JoinMethods::HashJoinsOnly` or pinned weights (`pin_cost_weights`, simulator feature) so
the plan is that shape, row counters on; expect c's rows read once (its row count), not its row
count times the prefix's rows; and the answer equal to `NestedLoopsOnly`'s.

### 5. Open: the wrong answer

r414's first timed run answered `CHINA 163947441.6399` against SQLite's `CHINA 7822103.0` in 22 s.
No run of the probe (14 runs over 5 plans) gave a wrong answer, and the bench does not record the
plan of a timed run. 21x is about the ratio a dropped `c_nationkey = s_nationkey` would give (the
customer's nation unchecked against the supplier's: 25 nations), a guess. The next step: print the
plan before each timed run in the planner bench (`bench/src/planner/mod.rs` records it once, before
the counted run), rerun r414's sequence, and keep the plan that answers wrong.

### Not done this hour

- No fix landed: the engine's release build took 10 min 8 s on this machine at load 81, so a fix,
  its test, clippy and a second build did not fit after the diagnosis.
- No bisect: the plan shape is not a function of the commit alone (section 2), so a bisect by
  EXPLAIN shape needs the weights pinned (`pin_cost_weights` is behind the simulator feature, which
  the bench doesn't build).
- No 22-query sweep.
