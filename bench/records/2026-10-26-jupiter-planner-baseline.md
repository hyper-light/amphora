# The planner's baseline: estimates against the rows counted at every join prefix, JOB and TPC-H

Step 3 of the planner plan (kiln plan 01a16eab7f457673b4cf9933adacfc4d, "the estimator measured";
CLAUDE.md §7; docs/design/planner.md, decision 12; docs/research/planner.md, section 8): every JOB
and TPC-H query loaded into Amphora after ANALYZE, run with its row counters, and timed beside
SQLite, DuckDB and PostgreSQL on the same machine (`bench/planner/README.md` says how).

- **Hardware:** jupiter: AMD Ryzen AI Max+ 395 (16 cores, 32 threads), 123.6 GiB, WD PC SN5000S 1 TB
  NVMe (NTFS, 96 to 99% full during the runs), Windows 11 Pro 10.0.26200. Other work used the
  machine at times (other agents' builds and tests, Windows Defender, the search indexer), and every
  run waited for it.
- **Binary:** amphora-bench in release (`bench/Cargo.toml`'s profile, rustc 1.99.0), built on
  jupiter from a clean checkout of 26657fd (`dirty_files` 0 in every record) with the
  `bench/Cargo.lock` that build resolved: duckdb 1.20000.0 (DuckDB v2.0.0-dev88614, bundled),
  rusqlite 0.40.2 (SQLite 3.53.2, bundled), postgres 0.19.14. DuckDB's JOB 15a to 15d ran on
  f4feaff's build with the same `Cargo.lock`: 26657fd with e80d4e7 (PostgreSQL's refusals in its own
  words) and f4feaff (DuckDB is given the alias `at` quoted), `crates/` unchanged.
- **Engines:** each query on one thread, class for class: Amphora and SQLite with a 2 GiB page
  cache; DuckDB with `threads = 1` and, beside it as `duckdb-mt`, on all 32 threads with a database
  of its own, its memory limit as shipped (100 GiB); PostgreSQL 17.11 (the postgres:17 image's
  Debian build) in Docker Desktop's Linux VM, with `max_parallel_workers_per_gather = 0` and the
  image's settings (shared_buffers 128MB, work_mem 4MB, effective_cache_size 4GB).
- **Data:** JOB: the IMDb tables of 2013 (event.cwi.nl/da/job/imdb.tgz) and the 113 queries of
  gregrahn/join-order-benchmark at a396036, with its primary keys and foreign-key indexes. TPC-H:
  electrum/tpch-dbgen at 32f1c1b at scale factors 1 and 10, and the 22 queries of
  `bench/planner/tpch/queries` with the specification's validation parameters.
- **Runs:** `bench/planner/record.sh`: each query three times on each engine and once more on
  Amphora with its counters, best of the three, each run begun once the load was under 2 (Windows
  keeps no load average: the cores busy plus the processor queue, over 3 s), waiting up to an hour.
  Every one of the 8,478 runs began under 2 (the highest at 1.9999997). Deadlines: 600 s a run,
  1,800 s for Amphora's counted run. A pass stopped part way (kiln's runner on jupiter restarted
  seven times, a session running a pass stopped three times and adacachy rebooted once) went on
  where it stopped (`--resume`), a run header in its record for each start, all on 26657fd.

| pass | record | when (UTC) | engines | here |
|---|---|---|---|---|
| TPC-H SF 1, a | `2026-10-26-jupiter-planner-tpch-sf1-a.jsonl` | 2026-10-26 05:53 to 08:52 | all five | loaded the data; the first passes |
| TPC-H SF 1, b | `2026-10-26-jupiter-planner-tpch-sf1-b.jsonl` | 2026-10-26 08:52 to 10:12 | all five | run times |
| TPC-H SF 1, c | `2026-10-26-jupiter-planner-tpch-sf1-c.jsonl` | 2026-10-27 06:07 to 07:33 | all but PostgreSQL | the repeat |
| TPC-H SF 10, a | `2026-10-26-jupiter-planner-tpch-sf10-a.jsonl` | 2026-10-26 10:12 to 17:30 | all five to q06, then all but PostgreSQL | loaded the data; run times |
| TPC-H SF 10, b | `2026-10-26-jupiter-planner-tpch-sf10-b.jsonl` | 2026-10-27 20:17 to 23:34 | all but PostgreSQL | run times (best of 3); the repeat |
| TPC-H SF 10, c | `2026-10-26-jupiter-planner-tpch-sf10-c.jsonl` and its reruns' `2026-10-26-jupiter-planner-tpch-sf10-c-again.jsonl` | 2026-10-27 23:34 to 2026-10-28 03:42; reruns 04:06 to 06:03 | all but PostgreSQL | run times (best of 3); the repeat |
| TPC-H SF 10, PostgreSQL | `2026-10-26-jupiter-planner-tpch-sf10-pg.jsonl` | 2026-10-27 13:18 to 13:31 | PostgreSQL | run times |
| JOB, a | `2026-10-26-jupiter-planner-job-a.jsonl` | 2026-10-26 17:30 to 21:41 | all but PostgreSQL | loaded the data; the first passes |
| JOB, b | `2026-10-26-jupiter-planner-job-b.jsonl` | 2026-10-26 21:41 to 2026-10-27 03:36 | all but PostgreSQL | run times |
| JOB, DuckDB's 15a to 15d | `2026-10-26-jupiter-planner-job-at.jsonl` | 2026-10-26 23:33 to 23:38 | DuckDB, one thread and 32 | run times |
| JOB, c | `2026-10-26-jupiter-planner-job-c.jsonl` | 2026-10-27 07:33 to 13:13 | all but PostgreSQL | the repeat |
| JOB, PostgreSQL | `2026-10-26-jupiter-planner-job-pg.jsonl` | 2026-10-27 13:37 to 14:10 (its load 13:31 to 13:37, `2026-10-26-jupiter-planner-load-job.jsonl`) | PostgreSQL | run times |
| JOB, d | `2026-10-26-jupiter-planner-job-d.jsonl` and its reruns' `2026-10-26-jupiter-planner-job-d-again.jsonl` | 2026-10-27 15:36 to 20:17; reruns 2026-10-28 03:42 to 04:00 | all five (PostgreSQL to 15c) | the repeat |
| JOB, e | `2026-10-26-jupiter-planner-job-e.jsonl` | 2026-10-28 13:31 to 17:09 | all but PostgreSQL, one process, Docker Desktop stopped | the repeat |
| JOB, f | `2026-10-26-jupiter-planner-job-f.jsonl` | 2026-10-29 04:29 to 07:43 | all but PostgreSQL, one process, Docker Desktop stopped | the repeat, against e |
| JOB, f's two stopped starts | `-job-f-stopped-0256.jsonl` (to 9c), `-job-f-stopped-0320.jsonl` (to 2d) | 2026-10-29 01:23 to 02:56, 03:03 to 03:20 | as f | kept, not compared: jupiter's runner restarted under the first, and the second was stopped when another session's lab took eight cores |
| DuckDB on 32 threads, six JOB queries | `2026-10-29-jupiter-planner-duckdb-mt-probe.jsonl` | 2026-10-29 00:34 | DuckDB, 32 threads | a process of their own (the next section) |
| TPC-H SF 1, d | `2026-10-26-jupiter-planner-tpch-sf1-d.jsonl` | 2026-10-29 07:58 to 08:24 | all but PostgreSQL, one process, Docker Desktop stopped | the repeat |
| TPC-H SF 1, e | `2026-10-26-jupiter-planner-tpch-sf1-e.jsonl` | 2026-10-29 12:17 to 14:49 | as d | the repeat, against d |
| JOB d, PostgreSQL's share from 15d | `-job-d.jsonl` (its run from 00:58), its 31a to 33c again in `-job-d-again.jsonl` | 2026-10-29 00:58 to 01:21; again 08:47 to 08:49 | PostgreSQL | the repeat, against `job-pg` |
| TPC-H SF 10, b and c, PostgreSQL's share | `-tpch-sf10-b.jsonl`, `-tpch-sf10-c.jsonl` | 2026-10-29 08:49 to 09:13, 09:13 to 09:28 | PostgreSQL | the repeat |

## Estimates and counts at every join prefix

**JOB: all 113 queries** have Amphora's estimated and counted rows at every join prefix, 977
prefixes, none refused. **TPC-H: all 22 at scale factor 1 and all 22 at 10**, 94 prefixes each (the
22 queries' plans and 19 of their subqueries'). Every pass recorded the same estimates and counts: 0
of JOB's 1,090 counters differ in estimate or in rows counted between any two of passes a, b, c and
d, and 0 of TPC-H's 135, at SF 1 (a, b, c) and at SF 10 (a, b, c), with the reruns alike
(`repeat.py`).

A prefix with n joins is the (n+1)th table of its plan's join order, its rows those that left it
into the next level after every term the plan evaluates there (a correlated subquery's over its
calls). The q-error is max(est/act, act/est), each at least one row (Moerkotte et al., PVLDB 2(1),
2009); the percentiles are nearest rank.

| joins in the prefix | prefixes | median q-error | p90 | p95 | max |
|---:|---:|---:|---:|---:|---:|
| 0 | 113 | 1 | 1.342e+04 | 1.342e+04 | 1.924e+04 |
| 1 | 113 | 10.9 | 365 | 1.380e+04 | 1.866e+05 |
| 2 | 113 | 37.9 | 3.45e+03 | 1.237e+05 | 5.031e+05 |
| 3 | 113 | 1.07e+03 | 5.638e+05 | 7.855e+05 | 9.048e+06 |
| 4 | 110 | 1.14e+03 | 2.684e+05 | 1.302e+06 | 7.797e+06 |
| 5 | 90 | 4.69e+03 | 9.783e+05 | 4.961e+06 | 7.797e+06 |
| 6 | 88 | 3.44e+03 | 2.216e+05 | 8.815e+05 | 2.833e+06 |
| 7 | 72 | 2.97e+03 | 3.199e+05 | 8.372e+05 | 4.317e+06 |
| 8 | 51 | 2.5e+03 | 5.328e+05 | 9.339e+05 | 4.112e+07 |
| 9 | 37 | 1.56e+03 | 9.339e+05 | 1.268e+07 | 4.112e+07 |
| 10 | 30 | 743 | 4.502e+04 | 5.895e+04 | 1.897e+05 |
| 11 | 20 | 477 | 8.65e+03 | 1.146e+04 | 1.501e+04 |
| 12 | 9 | 6.14e+03 | 1.151e+06 | 1.151e+06 | 1.151e+06 |
| 13 | 9 | 1.62e+03 | 7.676e+05 | 7.676e+05 | 7.676e+05 |
| 14 | 3 | 30 | 803 | 803 | 803 |
| 15 | 3 | 4.89e+03 | 9.044e+04 | 9.044e+04 | 9.044e+04 |
| 16 | 3 | 1.62e+03 | 2.240e+04 | 2.240e+04 | 2.240e+04 |
| all | 977 | 273 | 1.651e+05 | 7.384e+05 | 4.112e+07 |

| joins in the prefix | prefixes | SF 1 median | p90 | p95 | max | SF 10 median | p90 | p95 | max |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 | 41 | 9.4 | 2.5e+03 | 2.054e+04 | 4.991e+04 | 8.08 | 1e+04 | 1.999e+05 | 3.108e+05 |
| 1 | 22 | 9.4 | 2.5e+03 | 3.099e+04 | 1.183e+05 | 19.1 | 1e+04 | 3.108e+05 | 1.187e+06 |
| 2 | 12 | 15.3 | 2.5e+03 | 4.991e+04 | 4.991e+04 | 3.68 | 60.9 | 2.5e+03 | 2.5e+03 |
| 3 | 8 | 60.6 | 4.991e+04 | 4.991e+04 | 4.991e+04 | 38.2 | 1e+03 | 1e+03 | 1e+03 |
| 4 | 5 | 60.6 | 4.991e+04 | 4.991e+04 | 4.991e+04 | 60.9 | 121 | 121 | 121 |
| 5 | 4 | 30.8 | 4.991e+04 | 4.991e+04 | 4.991e+04 | 8.08 | 1.52e+03 | 1.52e+03 | 1.52e+03 |
| 6 | 1 | 6.94 | 6.94 | 6.94 | 6.94 | 8.08 | 8.08 | 8.08 | 8.08 |
| 7 | 1 | 6.94 | 6.94 | 6.94 | 6.94 | 8.08 | 8.08 | 8.08 | 8.08 |
| all | 94 | 18.8 | 2.004e+04 | 4.991e+04 | 1.183e+05 | 9.39 | 2.5e+03 | 1.999e+05 | 1.187e+06 |

The largest intermediate result's q-error, one a query (the result Han et al., PVLDB 15(4), 2021,
find matters):

| benchmark | queries | median | p90 | p95 | max |
|---|---:|---:|---:|---:|---:|
| JOB | 113 | 322 | 2.833e+06 | 7.797e+06 | 4.112e+07 |
| TPC-H SF 1 | 22 | 30.3 | 3.099e+04 | 4.991e+04 | 1.183e+05 |
| TPC-H SF 10 | 22 | 7.65 | 1.999e+05 | 3.108e+05 | 1.187e+06 |

On one table JOB's estimates are mostly right (median 1), but 16 of the 113 are off by 13,417 or
more. 15 of those are an equality on `keyword.keyword`, which no index covers, so it keeps the fixed
selectivity of 0.1 whatever ANALYZE found (`cost_params.rs`'s `sel_eq_unindexed`; 2a to 2d, 6a, 6c,
6e, 16b, 16c and 17a to 17f: 1 row counted, 13,417 estimated, a tenth of keyword's 134,170). Each
join multiplies the error on: the median passes 1,000 by the third join, and down the long chains
the estimate falls below one row while the counts grow, so the q-error there is the count itself
(26c's ninth table: 3.4e-33 rows estimated, 41,119,579 counted). TPC-H's joins follow keys and its
error stays nearer (median 18.8 over every prefix at SF 1, 9.39 at SF 10); its largest come from
filters' fixed selectivities (at SF 10, q16's three filters on part leave 1.44 rows estimated
against 296,824 counted, and its join to partsupp 0.0576 against 1,186,602).

## Run times

Best of 3; each engine's time over Amphora's, the geometric mean over the queries both finished
(under 1, the engine was faster), and on how many queries it was the faster. JOB's from its pass b,
with DuckDB's 15a to 15d from `job-at`, and TPC-H SF 1's from its pass b, both timed by processes
that had not loaded the data (the next section says why); SF 10's are the best of its passes a, b
and c, each query's fastest pass (`summary.py --best`). PostgreSQL's JOB times are its own pass's of
2026-10-27, its SF 10 times the best of its own pass and its shares of passes b and c (2026-10-29),
and its SF 1 times pass b's. Every engine counted the same rows for every
query.

| benchmark | SQLite | DuckDB, one thread | DuckDB, 32 threads | PostgreSQL |
|---|---:|---:|---:|---:|
| JOB (113) | 1.101, 63 faster | 1.013, 64 faster | 0.435, 75 faster | 0.354, 83 faster |
| TPC-H SF 1 (22) | 1.417, 12 faster | 0.481, 16 faster | 0.126, 20 faster | 0.535, 19 faster |
| TPC-H SF 10 (22) | 1.198 (21 of 22), 13 faster | 0.289, 16 faster | 0.021, 22 faster | 0.328, 20 faster |

Amphora's planning time (its prepare, best of 3) is a median 1.9 ms (92.1 ms at most, 27c's 12
tables) on JOB and 0.5 to 0.6 ms (1.4 ms at most) on TPC-H.

## The first passes: timed by the process that had loaded the data

`record.sh` as these passes ran (26657fd) loaded a benchmark in the process that then timed its
first pass. Those first passes ran slower than the second, timed by fresh processes on the loaded
data, though their estimates and counts are the same (`repeat.py`, pass a against pass b):

JOB (113 queries; DuckDB's 15a to 15d, refused in both passes, left out):

| engine | queries | median best ms, pass a | pass b | b/a | median of b/a by query |
|---|---:|---:|---:|---:|---:|
| amphora | 113 | 824.5 | 784.2 | 0.9510 | 0.9578 |
| duckdb | 109 | 1018.7 | 876.0 | 0.8599 | 0.9351 |
| duckdb-mt | 109 | 577.4 | 332.8 | 0.5764 | 0.6964 |
| sqlite | 113 | 818.9 | 774.5 | 0.9457 | 0.9872 |

TPC-H SF 1 (22 queries):

| engine | queries | median best ms, pass a | pass b | b/a | median of b/a by query |
|---|---:|---:|---:|---:|---:|
| amphora | 22 | 843.8 | 814.6 | 0.9654 | 1.0417 |
| duckdb | 22 | 218.8 | 226.3 | 1.0344 | 1.0052 |
| duckdb-mt | 22 | 136.0 | 99.5 | 0.7315 | 0.8552 |
| postgres | 22 | 335.9 | 345.9 | 1.0299 | 1.0220 |
| sqlite | 22 | 948.7 | 1086.7 | 1.1455 | 1.0698 |

By the order the queries ran (b over a, the median of each twelve), Amphora, SQLite and DuckDB on
one thread ran JOB's first queries slower in pass a (b over a 0.87, 0.92 and 0.77 for 1a to 4a) and
level from 14a on, past the pass's first hour and a half; DuckDB on all 32 threads was slower
throughout (0.39 to 0.84). TPC-H SF 1's pass b began at higher loads than its pass a (a median 1.75
against 0.93) and ran SQLite 7% slower, while DuckDB on all 32 threads again ran faster in b (0.86).
So JOB's and SF 1's run times above are their second passes', and `record.sh` loads a benchmark in a
process of its own since c652376. SF 10's best of 3 is each query's fastest of passes a, b and c
with c's reruns: b gave 42 of the 87 four-engine records with a time, c 35, c's reruns 5 and a 5;
SQLite's q07 passed the deadline in all three.

## Quiet runs: what ran beside the passes of 2026-10-27 to 29

TPC-H SF 10's passes b and c, JOB's passes d, e and f and the passes after them ran beside a monitor:
every process's CPU over each 10 s (PowerShell's `Get-Process` totals, differenced; those using a
fifth of a core or more) and C:'s free space (`2026-10-26-jupiter-planner-monitor.log`, from
2026-10-27 15:36Z, while a pass ran), and
each pass's log lines with the time they appeared (`2026-10-26-jupiter-planner-passes-ts.log`). A
record's runs happen between the log line before it and its own: `bench/planner/disturbed.py` finds
the records during whose span other processes (all but amphora-bench, and Docker's VM for
PostgreSQL's records) held more than 2 cores in any sample, or which overlapped other work the
monitor can't see, the disk-heavy deletes another agent reported on jupiter (a temp sweep ending at
15:47Z, 280 GB deleted from 00:24 to 01:17Z and 115 GB from 01:23 to 01:25Z). Those records ran
again, by a process of their own with the same binary, once the machine was quiet (the passes'
`-again.jsonl`, every run kept; a query's last record is its rerun). The windows, as given:
`--windows
2026-10-27T15:40:00Z/2026-10-27T15:48:00Z,2026-10-28T00:24:00Z/2026-10-28T01:18:00Z,2026-10-28T01:23:00Z/2026-10-28T01:26:00Z`.
A pass's start line names it too, as a pass whose log alone is followed gets no `==>` header, and a
pass started again from its beginning keeps its stopped starts' lines, which `--since` leaves out
(`bench/planner/test_disturbed.py`).
Each pass from 2026-10-29 on began only once no build and no container but PostgreSQL's ran and the
load had stayed under 1.5 for five minutes (its waits and steps in `2026-10-26-jupiter-planner-chain.log`).

- JOB d: 45 of 506 records disturbed, by Shards' Windows builds and tests (15:56 to 16:57Z and 19:12
  to 20:03Z) and the temp sweep; the 41 of the four engines ran again, all quiet. PostgreSQL's 3
  (1a, 6c, 6d) could not, Docker Desktop having stopped, and DuckDB's 15d is a refusal either way.
- TPC-H SF 10 b: 0 of 88.
- TPC-H SF 10 c: 24 of 88 (q09 to q14 in the deletes, SQLite's q07 and DuckDB's q18 beside other
  work); all 24 ran again, and 6 of the reruns were disturbed in turn.
- JOB e: 18 of 452, all from 13:40 to 14:38Z beside other agents' builds (rustc on up to 16 cores).
- JOB f: 15 of 452 since it began at 04:28:56Z (`--since 2026-10-29T04:28:56Z`), from 05:52 to
  06:33Z beside other agents' builds. The records disturbed in e or f are left out of their
  comparison rather than run again: a run by a process of its own isn't timed alike for DuckDB on
  32 threads (the repeat's section on it).
- TPC-H SF 1 d: 0 of 88. TPC-H SF 1 e: 21 of 88, from 12:30 to 14:13Z beside other agents' builds on
  up to 25 cores, the runs waiting at times for the load to fall (e took 2 h 32 min, d 26 min).
- PostgreSQL's share of JOB d from 15d (`--since 2026-10-29T00:58:10Z`): 3 of 59 (16b, 16c and 17b,
  beside Docker Desktop's own processes and kiln), and none of the 8 run again at 08:47Z.
- PostgreSQL's share of TPC-H SF 10 b: 3 of 22 (q07; q08 beside other agents' builds on 22 cores; q09
  beside their VMs); of c, 5 of 22 (q01, q09, q10 beside Windows Defender and the indexer; q02 and
  q04 beside builds). For PostgreSQL the monitor counts Docker's VM as the bench's own, so every
  other container's CPU each 10 s went in `2026-10-26-jupiter-planner-docker-stats.log`: none ran
  beside these shares but another session's lab at 00:58 to 01:21Z, at 1.7 cores at most.

## The repeat: two passes of one build on one machine

The plan's test: the harness run twice on one build and machine records the same estimates, and
median run times within 5% (`bench/planner/repeat.py`: per engine, the median over the queries of
each query's best time; beside it, the median of the queries' own ratios). Every pair of passes
recorded the same estimates and counts (above). The run times are compared between passes timed
alike, by fresh processes on loaded data, those of 2026-10-27 and 28 with their disturbed records
replaced by reruns (the section before); JOB's pass b ran before the monitor.

**TPC-H, SF 10, pass b against pass c**, as run:

| engine | queries | median best ms, pass b | pass c | c/b | median of c/b by query |
|---|---:|---:|---:|---:|---:|
| amphora | 22 | 23287.8 | 23299.3 | 1.0005 | 1.0027 |
| duckdb | 22 | 1791.4 | 1857.6 | 1.0370 | 1.0082 |
| duckdb-mt | 22 | 291.6 | 292.0 | 1.0013 | 0.9738 |
| sqlite | 21 | 24203.1 | 24081.6 | 0.9950 | 0.9991 |

and with pass c's 24 disturbed records replaced by their reruns:

| engine | queries | median best ms, pass b | pass c | c/b | median of c/b by query |
|---|---:|---:|---:|---:|---:|
| amphora | 22 | 23287.8 | 23424.8 | 1.0059 | 1.0132 |
| duckdb | 22 | 1791.4 | 1857.6 | 1.0370 | 1.0341 |
| duckdb-mt | 22 | 291.6 | 292.0 | 1.0013 | 0.9731 |
| sqlite | 21 | 24203.1 | 23972.7 | 0.9905 | 1.0036 |

Within 5% for every engine, either way: Amphora 1.001 (1.006 with the reruns), DuckDB 1.037, DuckDB
on all 32 threads 1.001 and SQLite 0.995 (0.990). Six reruns were themselves disturbed (other
agents' builds and tests on jupiter from 04:49Z, a kiln test and Docker Desktop's VM later): four of
them have their fastest in pass b's quiet record, SQLite's q07 passed the deadline in every pass,
and SQLite's q14 is pass c's 485.9 s against b's 487.1 s, so the best of 3 above doesn't rest on
them. PostgreSQL has no time in passes b and c (below).

**JOB, pass b against pass d** (pass d's 41 disturbed records of the four engines replaced by their
reruns, all quiet):

| engine | queries | median best ms, pass b | pass d | d/b | median of d/b by query |
|---|---:|---:|---:|---:|---:|
| amphora | 113 | 784.2 | 762.9 | 0.9729 | 0.9973 |
| duckdb | 109 | 876.0 | 840.3 | 0.9593 | 0.9866 |
| duckdb-mt | 109 | 332.8 | 384.4 | 1.1550 | 1.0330 |
| sqlite | 113 | 774.5 | 760.6 | 0.9821 | 0.9801 |

Within 5% for Amphora (0.973), SQLite (0.982) and DuckDB on one thread (0.959); not for DuckDB on
all 32 threads, 1.155 by the median of its best times, though 1.033 by the median of its queries'
own ratios: pass b's 109 best times have a gap at their median (333 to 354 ms) that pass d's lack.
Pass b ran with Docker Desktop stopped; pass d's 1a to 15d ran with its VM up for PostgreSQL's
share, and over those the one-thread engines were 0.99 to 1.04 against b, over 16a to 33c (Docker
stopped in both) 0.92 to 0.95. PostgreSQL's 51 undisturbed queries of the 54 it ran in pass d,
against its own pass of 2026-10-27: 0.967 (by query 0.997).

### DuckDB on all 32 threads, and the age of the process timing it

A pass's queries are timed by one process, and DuckDB on all 32 threads runs slower the older that
process is, the more of the pass it has run; the one-thread engines don't. Passes c and e were each
timed by one process from start to end (c beside other agents' work, e on a quiet machine with
Docker Desktop stopped). Passes b and d went on where a stopped process left them (`--resume`): b in
five processes, begun at its 1a, 9d, 12a, 13b and 30b (2026-10-26 21:41, 22:21 and 23:38, 2026-10-27
01:06 and 03:26), and d in two, begun at 1a and 15d (2026-10-27 15:36 and 18:17). Six of JOB's
queries on DuckDB with 32 threads, best of 3 in ms, in each pass and in a process started for them
alone on 2026-10-29 at 00:34 (`2026-10-29-jupiter-planner-duckdb-mt-probe.jsonl`; the processors
at 103 to 158% of their 3,000 MHz base while it ran, `-probe-perf.txt`):

| query | probe | b | c | d | e |
|---|---:|---:|---:|---:|---:|
| 1a | 61.3 | 57.1 | 54.7 | 58.8 | 57.9 |
| 2b | 68.7 | 63.5 | 68.5 | 67.3 | 65.2 |
| 16a | 98.7 | 132.4 | 265.6 | 105.5 | 257.5 |
| 16c | 207.9 | 330.5 | 498.0 | 227.3 | 501.5 |
| 17a | 192.4 | 309.5 | 490.1 | 239.3 | 492.2 |
| 32b | 64.1 | 103.4 | 311.6 | 232.7 | 314.8 |

A pass's first queries ran alike in all. 16a to 32b ran slowest in c and e, one process each, and
there c and e agree within 2%; d's second process began two queries before 16a, b's fourth at 13b
and its fifth at 30b, and the probe's process was new. In pass e, DuckDB on 32 threads was within 3%
of b through the pass's first two hours (the median of its queries' e/b 1.027 at 13Z and 1.026 at
14Z), beside other agents' builds (other processes 1.84 cores on average), and 1.17 to 1.58 from
15Z on, when other processes used 0.43 of a core on average and 1.72 at most: not the machine's
other work. Over the 109 queries, e against c is 0.981 by the median of best times (0.945 by query),
where e against b is 1.445. So b against d sets DuckDB's later queries in younger processes against
older ones, and its repeat is between passes timed alike, each by one process from start to end:
e against f.

**JOB, pass e against pass f**, each timed by one process from start to end with Docker Desktop
stopped (the same estimates and counts at all 1,090 of Amphora's counters), as run:

| engine | queries | median best ms, pass e | pass f | f/e | median of f/e by query |
|---|---:|---:|---:|---:|---:|
| amphora | 113 | 828.2 | 776.4 | 0.9375 | 1.0041 |
| duckdb | 109 | 798.7 | 800.0 | 1.0017 | 1.0062 |
| duckdb-mt | 109 | 481.0 | 484.8 | 1.0078 | 1.0266 |
| sqlite | 113 | 814.5 | 750.5 | 0.9214 | 0.9839 |

and with the 33 records disturbed in either pass (18 in e, 15 in f) left out:

| engine | queries | median best ms, pass e | pass f | f/e | median of f/e by query |
|---|---:|---:|---:|---:|---:|
| amphora | 100 | 811.6 | 805.5 | 0.9924 | 1.0051 |
| duckdb | 102 | 813.7 | 813.6 | 0.9999 | 1.0064 |
| duckdb-mt | 105 | 492.2 | 489.2 | 0.9940 | 1.0254 |
| sqlite | 104 | 1137.2 | 1111.1 | 0.9770 | 0.9836 |

Within 5% for every engine by both measures once the disturbed records are left out: Amphora 0.992,
DuckDB on one thread 1.000, DuckDB on all 32 threads 0.994 and SQLite 0.977 (by query 1.005, 1.006,
1.025 and 0.984). As run, Amphora (0.938) and SQLite (0.921) are not: e's disturbed records, beside
other agents' builds, were 8 of Amphora's and 4 of SQLite's, and SQLite's best times have a gap at
their median in every pass (774.5 to 1,128.7 ms in b).

**TPC-H, SF 1, pass d against pass e**, each timed by one process from start to end with Docker
Desktop stopped (the same estimates and counts at all 135 counters), as run:

| engine | queries | median best ms, pass d | pass e | e/d | median of e/d by query |
|---|---:|---:|---:|---:|---:|
| amphora | 22 | 767.0 | 782.0 | 1.0196 | 1.0195 |
| duckdb | 22 | 208.6 | 206.5 | 0.9901 | 0.9975 |
| duckdb-mt | 22 | 101.1 | 99.4 | 0.9836 | 1.0111 |
| sqlite | 22 | 935.4 | 952.9 | 1.0187 | 1.0145 |

and with the 21 records disturbed in e left out:

| engine | queries | median best ms, pass d | pass e | e/d | median of e/d by query |
|---|---:|---:|---:|---:|---:|
| amphora | 17 | 764.8 | 751.5 | 0.9826 | 1.0119 |
| duckdb | 17 | 192.9 | 190.2 | 0.9860 | 0.9970 |
| duckdb-mt | 16 | 96.3 | 96.5 | 1.0016 | 1.0105 |
| sqlite | 17 | 1002.0 | 984.4 | 0.9824 | 1.0122 |

Within 5% for every engine either way. SF 1's earlier passes b and c, against d, were 0.86 to 0.94
for the one-thread engines (by query 0.90 to 0.97): b began at loads up to 1.75 and c ran beside other
agents' work without a monitor, so d and e, both quiet, are the pair.

**PostgreSQL, TPC-H SF 10, its share of pass b against its share of pass c**, each by a process of
its own on 2026-10-29, as run:

| engine | queries | median best ms, pass b | pass c | c/b | median of c/b by query |
|---|---:|---:|---:|---:|---:|
| postgres | 22 | 5627.8 | 5766.3 | 1.0246 | 1.0015 |

and with the 7 queries disturbed in either share left out (q01, q02, q04, q07, q08, q09, q10):

| engine | queries | median best ms, pass b | pass c | c/b | median of c/b by query |
|---|---:|---:|---:|---:|---:|
| postgres | 15 | 5256.8 | 5323.6 | 1.0127 | 0.9974 |

Within 5% either way. Against its own pass of 2026-10-27 (`tpch-sf10-pg`, the first after Docker
Desktop came back), the share of b is 1.079 by the median of best times (1.035 by query).

**PostgreSQL, JOB, its own pass (`job-pg`, 2026-10-27) against its share of pass d** (1a to 15c with
pass d on 2026-10-27, 15d on from 2026-10-29, 31a to 33c from their rerun), as run:

| engine | queries | median best ms, pass pg | pass d | d/pg | median of d/pg by query |
|---|---:|---:|---:|---:|---:|
| postgres | 113 | 259.1 | 254.3 | 0.9818 | 1.0078 |

and with the 6 records disturbed in pass d left out (1a, 6c, 6d, 16b, 16c, 17b):

| engine | queries | median best ms, pass pg | pass d | d/pg | median of d/pg by query |
|---|---:|---:|---:|---:|---:|
| postgres | 107 | 249.5 | 245.6 | 0.9843 | 1.0065 |

So the test is met: the estimates are the same in every pair of passes, and the median run times are
within 5% for every engine between passes timed alike on a quiet machine, with the records other work
disturbed left out or run again: TPC-H at SF 10 (b against c, PostgreSQL's shares of them too), TPC-H
at SF 1 (d against e; PostgreSQL's a against b, 1.030), and JOB (e against f, each timed by one process
from start to end; PostgreSQL's own pass against its share of d). Every earlier pair is kept in its
records: JOB's b against d (DuckDB on all 32 threads 1.155, the age of its process), JOB's pass c,
run beside other agents' work without a monitor (1.04 to 1.47 against b), and SF 1's b and c.

## Refusals, deadlines and PostgreSQL's passes

- **PostgreSQL** ran in Docker Desktop, which stopped during TPC-H SF 10's q06 in pass a on
  2026-10-26 ("Docker Desktop is unable to start"; its WSL 2 distribution `docker-desktop` stopped)
  and answered again by 2026-10-27 13:16Z. Neither restarting it nor running PostgreSQL 17.11's own
  Windows build instead was allowed in this session (the permission check refused both: "Interfere
  With Workloads", "Code from External"). Pass a's SF 10 record keeps PostgreSQL's q01 to q05, q06's
  failure ("db error") and q07 to q12's ("connection closed"); the passes after it ran without
  PostgreSQL (`TARGETS`, 4177f90). SF 10's 22 and JOB's 113 then ran on PostgreSQL alone in passes
  of their own (`tpch-sf10-pg`, `job-pg`), the same image on the same volume, and its times above
  are those. JOB was loaded into PostgreSQL by a process of its own just before its pass, on the
  same server: that pass ran right after the load, as the other engines' first passes did. Docker
  Desktop stopped again at about 18:16Z on 2026-10-27, during JOB's pass d, with PostgreSQL's share
  at 15c. Another session's lab on jupiter held Docker Desktop up from 2026-10-29 00:50Z, and
  PostgreSQL's share of pass d from 15d ran then, by `record.sh` with `TARGETS=postgres`, from 00:58
  to 01:21Z, until Docker Desktop stopped once more: 31a to 33c got no time and ran again, by a
  process of their own into `job-d-again.jsonl`, at 08:47 to 08:49Z. Docker Desktop was started for
  them at 08:24Z (Ada's word, 2026-10-28), and PostgreSQL's share of SF 10's passes b and c ran next,
  08:49 to 09:13Z and 09:13 to 09:28Z, each by a process of its own. Its SF 10 and JOB times in the
  table above stay those of its own passes of 2026-10-27; the later ones are its repeat.
- **SQLite at SF 10:** q07 passed the 600 s deadline in every pass. In pass a, q14 passed it on its
  first run and q15 on its third; in passes b and c both finished all three runs (q14's best 485.9
  s, q15's 474.4 s), so SF 10's best of 3 has SQLite's time for 21 of the 22.
- **DuckDB** refuses JOB's 15a to 15d as written (`at`, aka_title's alias there, is a word its
  parser keeps): the JOB passes hold its refusals, `job-at.jsonl` its times with the alias quoted.
- Nothing was refused on Amphora: every query's counted run finished, with a count at every prefix.

## Every query

`python3 bench/planner/summary.py --best 2026-10-26-jupiter-planner-job-b.jsonl 2026-10-26-jupiter-planner-job-at.jsonl 2026-10-26-jupiter-planner-job-pg.jsonl 2026-10-26-jupiter-planner-tpch-sf1-b.jsonl 2026-10-26-jupiter-planner-tpch-sf10-a.jsonl 2026-10-26-jupiter-planner-tpch-sf10-b.jsonl 2026-10-26-jupiter-planner-tpch-sf10-c.jsonl 2026-10-26-jupiter-planner-tpch-sf10-c-again.jsonl 2026-10-26-jupiter-planner-tpch-sf10-pg.jsonl`
(each engine's best ms, `timed_out` or `failed` where it has none; SF 10's the best of its passes):

### job: 113 queries, 113 with estimated and actual rows at every join prefix, 0 refused


| joins in the prefix | prefixes | median q-error | p90 | p95 | max |
|---:|---:|---:|---:|---:|---:|
| 0 | 113 | 1 | 1.342e+04 | 1.342e+04 | 1.924e+04 |
| 1 | 113 | 10.9 | 365 | 1.380e+04 | 1.866e+05 |
| 2 | 113 | 37.9 | 3.45e+03 | 1.237e+05 | 5.031e+05 |
| 3 | 113 | 1.07e+03 | 5.638e+05 | 7.855e+05 | 9.048e+06 |
| 4 | 110 | 1.14e+03 | 2.684e+05 | 1.302e+06 | 7.797e+06 |
| 5 | 90 | 4.69e+03 | 9.783e+05 | 4.961e+06 | 7.797e+06 |
| 6 | 88 | 3.44e+03 | 2.216e+05 | 8.815e+05 | 2.833e+06 |
| 7 | 72 | 2.97e+03 | 3.199e+05 | 8.372e+05 | 4.317e+06 |
| 8 | 51 | 2.5e+03 | 5.328e+05 | 9.339e+05 | 4.112e+07 |
| 9 | 37 | 1.56e+03 | 9.339e+05 | 1.268e+07 | 4.112e+07 |
| 10 | 30 | 743 | 4.502e+04 | 5.895e+04 | 1.897e+05 |
| 11 | 20 | 477 | 8.65e+03 | 1.146e+04 | 1.501e+04 |
| 12 | 9 | 6.14e+03 | 1.151e+06 | 1.151e+06 | 1.151e+06 |
| 13 | 9 | 1.62e+03 | 7.676e+05 | 7.676e+05 | 7.676e+05 |
| 14 | 3 | 30 | 803 | 803 | 803 |
| 15 | 3 | 4.89e+03 | 9.044e+04 | 9.044e+04 | 9.044e+04 |
| 16 | 3 | 1.62e+03 | 2.240e+04 | 2.240e+04 | 2.240e+04 |
| all | 977 | 273 | 1.651e+05 | 7.384e+05 | 4.112e+07 |

The largest intermediate result's q-error, a query each (113): median 322, p90 2.833e+06, p95
7.797e+06, max 4.112e+07.

| query | tables | max q-error | largest result's q-error | Amphora plan ms | amphora ms | sqlite ms | duckdb ms | duckdb-mt ms | postgres ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1a | 5 | 147 | 3.36 | 0.805 | 420 | 188 | 140 | 57.1 | 4.71 |
| 1b | 5 | 9.630e+04 | 2.7 | 0.69 | 369 | 195 | 91.6 | 45.7 | 1.33 |
| 1c | 5 | 7.4e+03 | 1.71 | 0.617 | 297 | 268 | 99.4 | 49.5 | 3.84 |
| 1d | 5 | 4.17e+03 | 1.25 | 0.625 | 392 | 201 | 90.5 | 46.6 | 1.42 |
| 2a | 5 | 9.441e+04 | 9.441e+04 | 0.48 | 212 | 134 | 260 | 61.8 | 238 |
| 2b | 5 | 9.441e+04 | 9.441e+04 | 0.487 | 213 | 134 | 263 | 63.5 | 249 |
| 2c | 5 | 9.441e+04 | 9.441e+04 | 0.576 | 209 | 130 | 48.8 | 36 | 223 |
| 2d | 5 | 9.441e+04 | 9.441e+04 | 0.585 | 216 | 158 | 284 | 80.2 | 330 |
| 3a | 4 | 1.671e+05 | 1.671e+05 | 0.496 | 2.41e+03 | 75.4 | 677 | 130 | 99.2 |
| 3b | 4 | 2.29e+03 | 2.29e+03 | 0.432 | 1.08e+03 | 35.5 | 672 | 175 | 62.1 |
| 3c | 4 | 7.25e+03 | 70.4 | 0.517 | 239 | 165 | 789 | 205 | 170 |
| 4a | 5 | 6.977e+05 | 6.977e+05 | 0.574 | 784 | 89.5 | 462 | 136 | 61.2 |
| 4b | 5 | 1.07e+03 | 66.3 | 0.623 | 46.1 | 82.5 | 170 | 70.2 | 34.3 |
| 4c | 5 | 2.013e+06 | 2.013e+06 | 0.615 | 1.77e+03 | 98.5 | 594 | 154 | 57.1 |
| 5a | 5 | 1.044e+04 | 1 | 0.521 | 210 | 80.8 | 191 | 278 | 73.6 |
| 5b | 5 | 2.09e+03 | 1 | 0.605 | 201 | 84.4 | 101 | 234 | 70 |
| 5c | 5 | 669 | 13.1 | 0.653 | 273 | 103 | 784 | 242 | 99.5 |
| 6a | 5 | 3.258e+04 | 950 | 0.616 | 11.2 | 416 | 525 | 168 | 9.84 |
| 6b | 5 | 383 | 131 | 0.631 | 94.3 | 578 | 585 | 149 | 156 |
| 6c | 5 | 9.124e+04 | 25.6 | 0.618 | 7.34 | 415 | 104 | 42.6 | 5.87 |
| 6d | 5 | 7.855e+05 | 7.855e+05 | 0.63 | 4.47e+03 | 613 | 1.14e+03 | 329 | 3.96e+03 |
| 6e | 5 | 3.258e+04 | 950 | 0.524 | 11.1 | 416 | 562 | 197 | 10.1 |
| 6f | 5 | 7.855e+05 | 7.855e+05 | 0.639 | 4.46e+03 | 2.49e+03 | 3.01e+03 | 643 | 3.92e+03 |
| 7a | 8 | 1.053e+05 | 1.053e+05 | 4.21 | 674 | 481 | 262 | 187 | 583 |
| 7b | 8 | 3.342e+04 | 3.342e+04 | 2.09 | 114 | 314 | 229 | 160 | 142 |
| 7c | 8 | 1.097e+06 | 1.097e+06 | 4.99 | 6.72e+03 | 5.71e+03 | 1.01e+03 | 456 | 1.29e+03 |
| 8a | 7 | 7.44e+03 | 2.13 | 1.33 | 2.42e+03 | 2.23e+03 | 545 | 234 | 1.7e+03 |
| 8b | 7 | 104 | 1.33 | 3.02 | 126 | 94.1 | 534 | 225 | 148 |
| 8c | 7 | 4.961e+06 | 4.961e+06 | 1.18 | 1.143e+04 | 4.638e+04 | 3.1e+03 | 747 | 2.63e+03 |
| 8d | 7 | 7.528e+05 | 7.528e+05 | 1.73 | 3.25e+03 | 4.144e+04 | 976 | 474 | 1.3e+03 |
| 9a | 8 | 1.047e+05 | 2.14 | 4.22 | 9.89e+03 | 7.88e+03 | 1.28e+03 | 603 | 259 |
| 9b | 8 | 2.699e+04 | 40.6 | 4.19 | 3.08e+03 | 3.02e+03 | 911 | 473 | 246 |
| 9c | 8 | 2.684e+05 | 1 | 2.08 | 1.96e+03 | 4.242e+04 | 1.58e+03 | 576 | 846 |
| 9d | 8 | 9.607e+05 | 9.607e+05 | 1.81 | 4.57e+03 | 4.267e+04 | 1.78e+03 | 412 | 1.62e+03 |
| 10a | 7 | 158 | 4.92 | 0.931 | 2.56e+03 | 615 | 648 | 184 | 438 |
| 10b | 7 | 2.61e+03 | 1 | 0.925 | 819 | 132 | 596 | 179 | 213 |
| 10c | 7 | 7.821e+05 | 1 | 0.949 | 2.810e+04 | 4.790e+05 | 1.33e+03 | 297 | 2.03e+03 |
| 11a | 8 | 6.86e+03 | 1 | 3.11 | 12.1 | 88.1 | 123 | 114 | 26.7 |
| 11b | 8 | 56 | 1 | 3 | 5.99 | 1.02 | 90.4 | 83.3 | 14.9 |
| 11c | 8 | 1.439e+05 | 1.439e+05 | 3.25 | 350 | 139 | 197 | 200 | 59.7 |
| 11d | 8 | 1.439e+05 | 1.439e+05 | 1.86 | 384 | 432 | 212 | 217 | 52.3 |
| 12a | 8 | 1.730e+04 | 10.5 | 1.6 | 164 | 1.41e+03 | 568 | 229 | 166 |
| 12b | 8 | 1.380e+04 | 1 | 4.6 | 125 | 2.06e+03 | 579 | 181 | 2.83 |
| 12c | 8 | 1.692e+05 | 2.86 | 1.91 | 1.05e+03 | 2.29e+03 | 677 | 290 | 666 |
| 13a | 9 | 5.163e+05 | 5.12 | 2.31 | 1.13e+03 | 642 | 617 | 252 | 1.17e+03 |
| 13b | 9 | 2.97e+03 | 5.12 | 4.68 | 859 | 1.62e+03 | 741 | 259 | 480 |
| 13c | 9 | 255 | 5.12 | 4.62 | 939 | 1.54e+03 | 599 | 236 | 457 |
| 13d | 9 | 4.317e+06 | 4.317e+06 | 1.99 | 3.94e+03 | 5.12e+03 | 876 | 354 | 1.8e+03 |
| 14a | 8 | 3.38e+03 | 273 | 1.32 | 175 | 329 | 613 | 333 | 211 |
| 14b | 8 | 287 | 287 | 2.57 | 49.7 | 230 | 533 | 261 | 85.1 |
| 14c | 8 | 1.779e+04 | 303 | 1.63 | 428 | 688 | 787 | 327 | 359 |
| 15a | 9 | 5.13e+03 | 1.69 | 3.15 | 509 | 1.32e+03 | 451 | 246 | 275 |
| 15b | 9 | 2.35e+03 | 4.06 | 3.77 | 29 | 19.7 | 307 | 159 | 18.9 |
| 15c | 9 | 2.185e+05 | 4.09 | 7.41 | 1.239e+04 | 1.248e+04 | 1.28e+03 | 335 | 213 |
| 15d | 9 | 2.185e+05 | 4.09 | 3.11 | 1.218e+04 | 1.088e+04 | 901 | 352 | 339 |
| 16a | 8 | 1e+03 | 5.4 | 1.8 | 345 | 64.4 | 310 | 132 | 132 |
| 16b | 8 | 3.711e+06 | 3.711e+06 | 1.67 | 1.135e+04 | 8.37e+03 | 3.6e+03 | 657 | 8.57e+03 |
| 16c | 8 | 3.199e+05 | 3.199e+05 | 1.6 | 890 | 723 | 1.27e+03 | 331 | 740 |
| 16d | 8 | 5.638e+05 | 54.8 | 1.89 | 2.28e+03 | 498 | 1.37e+03 | 322 | 597 |
| 17a | 7 | 2.833e+06 | 2.833e+06 | 1.1 | 5.76e+03 | 3.14e+03 | 1.81e+03 | 310 | 6.34e+03 |
| 17b | 7 | 7.797e+06 | 7.797e+06 | 1.2 | 1.258e+04 | 5.7e+03 | 3.23e+03 | 413 | 5.24e+03 |
| 17c | 7 | 7.797e+06 | 7.797e+06 | 0.871 | 1.315e+04 | 5.89e+03 | 3.06e+03 | 394 | 5.16e+03 |
| 17d | 7 | 7.797e+06 | 7.797e+06 | 0.906 | 1.269e+04 | 2.6e+03 | 3.13e+03 | 403 | 5.13e+03 |
| 17e | 7 | 2.833e+06 | 2.833e+06 | 1.18 | 5.47e+03 | 4.43e+03 | 2.1e+03 | 304 | 6.3e+03 |
| 17f | 7 | 7.797e+06 | 7.797e+06 | 1.14 | 1.344e+04 | 6.97e+03 | 3.3e+03 | 399 | 6.11e+03 |
| 18a | 7 | 6.930e+06 | 6.930e+06 | 1.18 | 1.125e+04 | 2.116e+04 | 1.56e+03 | 365 | 5.4e+03 |
| 18b | 7 | 358 | 10.5 | 1.43 | 227 | 757 | 1.11e+03 | 251 | 162 |
| 18c | 7 | 3.078e+05 | 3.078e+05 | 1.32 | 9.25e+03 | 2.699e+04 | 1.57e+03 | 385 | 6.91e+03 |
| 19a | 10 | 1.301e+05 | 45.4 | 23.4 | 9.4e+03 | 6.32e+03 | 1.74e+03 | 633 | 274 |
| 19b | 10 | 379 | 100 | 23.4 | 306 | 1.031e+04 | 1.12e+03 | 327 | 121 |
| 19c | 10 | 1.815e+05 | 1 | 14.8 | 6.79e+03 | 2.671e+04 | 2.31e+03 | 705 | 807 |
| 19d | 10 | 9.048e+06 | 9.048e+06 | 6.3 | 1.381e+05 | 2.674e+04 | 2.48e+03 | 840 | 5.4e+03 |
| 20a | 10 | 9.783e+05 | 9.783e+05 | 8.88 | 3.4e+03 | 9.88e+03 | 1.56e+03 | 502 | 1.87e+03 |
| 20b | 10 | 2.947e+05 | 2.947e+05 | 9.43 | 1.07e+03 | 1.240e+04 | 1.69e+03 | 567 | 1.51e+03 |
| 20c | 10 | 2.947e+05 | 2.947e+05 | 9.26 | 1.22e+03 | 2.84e+03 | 1.68e+03 | 563 | 806 |
| 21a | 9 | 2.386e+04 | 1 | 7.48 | 25.7 | 11.7 | 234 | 186 | 35.5 |
| 21b | 9 | 1.99e+03 | 1 | 7.66 | 16 | 10.7 | 201 | 158 | 30 |
| 21c | 9 | 3.304e+04 | 3.304e+04 | 7.73 | 34 | 12.8 | 273 | 197 | 31.7 |
| 22a | 11 | 1.157e+04 | 273 | 7.88 | 780 | 487 | 1.05e+03 | 828 | 274 |
| 22b | 11 | 273 | 273 | 7.78 | 169 | 346 | 1.02e+03 | 711 | 197 |
| 22c | 11 | 7.361e+04 | 7.361e+04 | 7.94 | 5.01e+03 | 2.15e+03 | 2.49e+03 | 1.1e+03 | 826 |
| 22d | 11 | 1.678e+05 | 1.678e+05 | 7.6 | 1.001e+04 | 5.1e+03 | 5.16e+03 | 1.5e+03 | 492 |
| 23a | 11 | 2.388e+04 | 4.55 | 19.7 | 466 | 775 | 843 | 570 | 155 |
| 23b | 11 | 1.22e+03 | 1.22e+03 | 7.91 | 93.6 | 2.36e+03 | 746 | 453 | 32.7 |
| 23c | 11 | 7.252e+04 | 7.252e+04 | 19.5 | 1.35e+03 | 1.38e+03 | 851 | 664 | 223 |
| 24a | 12 | 1.376e+04 | 1.376e+04 | 80.9 | 717 | 7.57e+03 | 1.75e+03 | 817 | 273 |
| 24b | 12 | 244 | 71.3 | 91 | 136 | 26.4 | 920 | 383 | 35.9 |
| 25a | 9 | 5.076e+04 | 225 | 3.55 | 3.85e+03 | 5.02e+03 | 1.6e+03 | 826 | 1.94e+03 |
| 25b | 9 | 365 | 365 | 3.05 | 178 | 1.39e+03 | 1.04e+03 | 497 | 333 |
| 25c | 9 | 6.773e+04 | 201 | 3.16 | 1.019e+04 | 1.055e+04 | 1.63e+03 | 830 | 5.84e+03 |
| 26a | 12 | 1.268e+07 | 1.268e+07 | 43.5 | 1.795e+04 | 1.566e+04 | 1.95e+03 | 1.12e+03 | 9.98e+03 |
| 26b | 12 | 9.339e+05 | 9.339e+05 | 40.2 | 1.4e+03 | 1.529e+04 | 918 | 759 | 69.2 |
| 26c | 12 | 4.112e+07 | 4.112e+07 | 44.8 | 5.785e+04 | 1.750e+04 | 2.14e+03 | 1.09e+03 | 3.182e+04 |
| 27a | 12 | 8.079e+04 | 20.4 | 89.5 | 722 | 1.16e+03 | 393 | 368 | 33.4 |
| 27b | 12 | 4.48e+03 | 4.48e+03 | 75.3 | 89.8 | 58.7 | 366 | 323 | 26.5 |
| 27c | 12 | 9.404e+04 | 9.404e+04 | 92.1 | 1.31e+03 | 1.32e+03 | 423 | 396 | 43.9 |
| 28a | 14 | 7.384e+05 | 7.384e+05 | 1.06 | 1.09e+03 | 2.23e+03 | 1.7e+03 | 1.41e+03 | 413 |
| 28b | 14 | 1.599e+04 | 9.1 | 1.06 | 192 | 698 | 1.23e+03 | 1.11e+03 | 298 |
| 28c | 14 | 1.151e+06 | 1.151e+06 | 1.05 | 1.7e+03 | 1.38e+03 | 1.7e+03 | 1.44e+03 | 353 |
| 29a | 17 | 4.89e+03 | 15.9 | 1.39 | 33.7 | 9.48 | 1.3e+03 | 737 | 44.3 |
| 29b | 17 | 2.44e+03 | 15.9 | 1.08 | 32.1 | 9.58 | 1.38e+03 | 924 | 42.7 |
| 29c | 17 | 7.676e+05 | 7.676e+05 | 1.22 | 3.46e+03 | 570 | 1.48e+03 | 949 | 479 |
| 30a | 12 | 7.18e+03 | 322 | 12 | 1.01e+03 | 2.89e+03 | 1.87e+03 | 1.13e+03 | 1.25e+03 |
| 30b | 12 | 322 | 322 | 31.5 | 247 | 2.37e+03 | 1.42e+03 | 595 | 1.69e+03 |
| 30c | 12 | 8.372e+05 | 8.372e+05 | 8.02 | 2.47e+03 | 2.31e+03 | 1.94e+03 | 795 | 5.87e+03 |
| 31a | 11 | 6.196e+05 | 6.196e+05 | 9.46 | 1.3e+03 | 1.49e+03 | 1.39e+03 | 584 | 1.33e+03 |
| 31b | 11 | 322 | 322 | 26.9 | 236 | 1.13e+03 | 1.47e+03 | 576 | 333 |
| 31c | 11 | 6.196e+05 | 6.196e+05 | 9.46 | 1.52e+03 | 1.81e+03 | 1.59e+03 | 631 | 1.26e+03 |
| 32a | 6 | 2.882e+05 | 2.882e+05 | 0.656 | 179 | 57.9 | 24.5 | 30.5 | 5.92 |
| 32b | 6 | 2.882e+05 | 2.882e+05 | 0.635 | 182 | 58.7 | 236 | 103 | 70 |
| 33a | 14 | 1.4e+03 | 4.64 | 0.816 | 28.6 | 22 | 569 | 525 | 40.8 |
| 33b | 14 | 1.14e+03 | 4.64 | 0.84 | 27.2 | 2.78 | 889 | 631 | 39.3 |
| 33c | 14 | 4.502e+04 | 1.94 | 0.885 | 692 | 30.9 | 637 | 567 | 69.6 |
| all (sum of those run) | | | | | 514502 (113 run) | 1008562 (113 run) | 123392 (113 run) | 48547 (113 run) | 163535 (113 run) |

| engine | queries both ran | its time over Amphora's (geometric mean) | queries it was faster on |
|---|---:|---:|---:|
| sqlite | 113 | 1.101 | 63 |
| duckdb | 113 | 1.013 | 64 |
| duckdb-mt | 113 | 0.435 | 75 |
| postgres | 113 | 0.354 | 83 |

### tpch SF 1: 22 queries, 22 with estimated and actual rows at every join prefix, 0 refused


| joins in the prefix | prefixes | median q-error | p90 | p95 | max |
|---:|---:|---:|---:|---:|---:|
| 0 | 41 | 9.4 | 2.5e+03 | 2.054e+04 | 4.991e+04 |
| 1 | 22 | 9.4 | 2.5e+03 | 3.099e+04 | 1.183e+05 |
| 2 | 12 | 15.3 | 2.5e+03 | 4.991e+04 | 4.991e+04 |
| 3 | 8 | 60.6 | 4.991e+04 | 4.991e+04 | 4.991e+04 |
| 4 | 5 | 60.6 | 4.991e+04 | 4.991e+04 | 4.991e+04 |
| 5 | 4 | 30.8 | 4.991e+04 | 4.991e+04 | 4.991e+04 |
| 6 | 1 | 6.94 | 6.94 | 6.94 | 6.94 |
| 7 | 1 | 6.94 | 6.94 | 6.94 | 6.94 |
| all | 94 | 18.8 | 2.004e+04 | 4.991e+04 | 1.183e+05 |

The largest intermediate result's q-error, a query each (22): median 30.3, p90 3.099e+04, p95
4.991e+04, max 1.183e+05.

| query | tables | max q-error | largest result's q-error | Amphora plan ms | amphora ms | sqlite ms | duckdb ms | duckdb-mt ms | postgres ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| q01 | 1 | 2.46 | 2.46 | 0.454 | 4.2e+03 | 5.01e+03 | 287 | 42.5 | 2.45e+03 |
| q02 | 5 | 2.5e+03 | 2.5e+03 | 0.841 | 23.8 | 337 | 129 | 89 | 172 |
| q03 | 3 | 3.93 | 1.21 | 0.419 | 1.61e+03 | 1.16e+03 | 144 | 53.8 | 428 |
| q04 | 2 | 7.63 | 7.63 | 0.47 | 349 | 256 | 130 | 37.1 | 201 |
| q05 | 6 | 1.21e+03 | 30.3 | 0.959 | 814 | 3.52e+03 | 227 | 102 | 625 |
| q06 | 1 | 1.9e+03 | 1.9e+03 | 0.351 | 815 | 617 | 56.7 | 13 | 303 |
| q07 | 6 | 60.6 | 60.6 | 1.3 | 1.55e+03 | 1.527e+04 | 226 | 110 | 909 |
| q08 | 8 | 122 | 61 | 1.04 | 1.69e+03 | 2.038e+04 | 259 | 159 | 277 |
| q09 | 6 | 4.991e+04 | 4.991e+04 | 0.732 | 1.95e+03 | 2.6e+03 | 563 | 166 | 2.19e+03 |
| q10 | 4 | 30.6 | 30.6 | 0.728 | 749 | 2.43e+03 | 312 | 124 | 534 |
| q11 | 3 | 2.53 | 2.53 | 0.526 | 153 | 72.3 | 183 | 46.7 | 52.2 |
| q12 | 2 | 3.099e+04 | 3.099e+04 | 0.448 | 864 | 1.56e+03 | 154 | 52.5 | 589 |
| q13 | 2 | 3.16 | 3.16 | 0.416 | 3.12e+03 | 2.29e+03 | 1.52e+03 | 134 | 586 |
| q14 | 2 | 2.53 | 2.53 | 0.418 | 811 | 1.075e+04 | 88.8 | 35.3 | 313 |
| q15 | 2 | 999 | 7.53 | 0.566 | 1.74e+03 | 1.031e+04 | 1.03e+03 | 70.2 | 297 |
| q16 | 2 | 1.183e+05 | 1.183e+05 | 0.474 | 268 | 128 | 600 | 147 | 261 |
| q17 | 2 | 2.02e+03 | 2.02e+03 | 0.552 | 142 | 78.4 | 261 | 97.5 | 379 |
| q18 | 3 | 21.5 | 1 | 0.478 | 2.11e+03 | 1.01e+03 | 7.080e+04 | 221 | 984 |
| q19 | 2 | 2.004e+04 | 2.004e+04 | 0.518 | 362 | 44.8 | 147 | 46.5 | 30.6 |
| q20 | 2 | 44 | 31.2 | 0.591 | 200 | 139 | 190 | 164 | 138 |
| q21 | 4 | 450 | 1.63 | 0.955 | 1.36e+03 | 817 | 410 | 130 | 863 |
| q22 | 2 | 9.4 | 1.57 | 0.567 | 127 | 56.1 | 184 | 139 | 74.3 |
| all (sum of those run) | | | | | 25011 (22 run) | 78843 (22 run) | 77907 (22 run) | 2178 (22 run) | 12661 (22 run) |

| engine | queries both ran | its time over Amphora's (geometric mean) | queries it was faster on |
|---|---:|---:|---:|
| sqlite | 22 | 1.417 | 12 |
| duckdb | 22 | 0.481 | 16 |
| duckdb-mt | 22 | 0.126 | 20 |
| postgres | 22 | 0.535 | 19 |

### tpch SF 10: 22 queries, 22 with estimated and actual rows at every join prefix, 0 refused


| joins in the prefix | prefixes | median q-error | p90 | p95 | max |
|---:|---:|---:|---:|---:|---:|
| 0 | 41 | 8.08 | 1e+04 | 1.999e+05 | 3.108e+05 |
| 1 | 22 | 19.1 | 1e+04 | 3.108e+05 | 1.187e+06 |
| 2 | 12 | 3.68 | 60.9 | 2.5e+03 | 2.5e+03 |
| 3 | 8 | 38.2 | 1e+03 | 1e+03 | 1e+03 |
| 4 | 5 | 60.9 | 121 | 121 | 121 |
| 5 | 4 | 8.08 | 1.52e+03 | 1.52e+03 | 1.52e+03 |
| 6 | 1 | 8.08 | 8.08 | 8.08 | 8.08 |
| 7 | 1 | 8.08 | 8.08 | 8.08 | 8.08 |
| all | 94 | 9.39 | 2.5e+03 | 1.999e+05 | 1.187e+06 |

The largest intermediate result's q-error, a query each (22): median 7.65, p90 1.999e+05, p95
3.108e+05, max 1.187e+06.

| query | tables | max q-error | largest result's q-error | Amphora plan ms | amphora ms | sqlite ms | duckdb ms | duckdb-mt ms | postgres ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| q01 | 1 | 2.46 | 2.46 | 0.476 | 5.230e+04 | 6.516e+04 | 2.85e+03 | 240 | 2.164e+04 |
| q02 | 5 | 2.5e+03 | 2.5e+03 | 1.05 | 343 | 3.46e+03 | 247 | 161 | 2.47e+03 |
| q03 | 3 | 6.4 | 1.35 | 0.587 | 2.830e+04 | 4.017e+04 | 1.7e+03 | 201 | 8.52e+03 |
| q04 | 2 | 7.65 | 7.65 | 0.547 | 9.88e+03 | 6.38e+03 | 1.36e+03 | 187 | 6.52e+03 |
| q05 | 6 | 1.52e+03 | 30.3 | 1.08 | 1.634e+04 | 1.739e+05 | 1.77e+03 | 282 | 8.05e+03 |
| q06 | 1 | 1.9e+03 | 1.9e+03 | 0.374 | 2.215e+04 | 1.210e+04 | 702 | 73.7 | 2.41e+03 |
| q07 | 6 | 60.9 | 60.8 | 1.36 | 4.334e+04 | timed_out | 1.65e+03 | 255 | 5.02e+03 |
| q08 | 8 | 121 | 60.8 | 1.06 | 3.112e+04 | 4.989e+04 | 1.56e+03 | 301 | 2.19e+03 |
| q09 | 6 | 3.68 | 1 | 0.85 | 1.585e+05 | 9.159e+04 | 6.09e+03 | 428 | 2.380e+04 |
| q10 | 4 | 38.2 | 38.2 | 0.617 | 1.368e+04 | 4.412e+04 | 7.2e+03 | 468 | 6.15e+03 |
| q11 | 3 | 2.5 | 2.47 | 0.552 | 1.74e+03 | 966 | 8.68e+03 | 91.8 | 1.06e+03 |
| q12 | 2 | 3.108e+05 | 3.108e+05 | 0.583 | 2.440e+04 | 2.397e+04 | 1.43e+03 | 181 | 5.42e+03 |
| q13 | 2 | 3.16 | 3.16 | 0.473 | 3.716e+04 | 3.054e+04 | 1.620e+05 | 682 | 6.41e+03 |
| q14 | 2 | 2.5 | 2.5 | 0.47 | 2.500e+04 | 4.859e+05 | 1.17e+03 | 150 | 2.66e+03 |
| q15 | 2 | 1e+04 | 7.55 | 1.3 | 4.600e+04 | 4.744e+05 | 4.444e+04 | 342 | 3.1e+03 |
| q16 | 2 | 1.187e+06 | 1.187e+06 | 0.478 | 2.96e+03 | 1.66e+03 | 6.309e+04 | 502 | 2.6e+03 |
| q17 | 2 | 1.934e+04 | 1.934e+04 | 0.532 | 1.35e+03 | 1.07e+03 | 1.81e+03 | 303 | 5.55e+03 |
| q18 | 3 | 6.21 | 1 | 0.575 | 3.948e+04 | 1.643e+04 | 4.097e+05 | 1.51e+03 | 3.075e+04 |
| q19 | 2 | 1.999e+05 | 1.999e+05 | 0.716 | 1.011e+04 | 612 | 1.17e+03 | 171 | 448 |
| q20 | 2 | 141 | 99.4 | 0.797 | 7.93e+03 | 6.1e+03 | 1.32e+03 | 419 | 4.69e+03 |
| q21 | 4 | 360 | 3.95 | 0.869 | 5.309e+04 | 3.100e+04 | 3.54e+03 | 531 | 1.195e+04 |
| q22 | 2 | 9.39 | 1.57 | 0.718 | 1.4e+03 | 696 | 1.98e+03 | 297 | 647 |
| all (sum of those run) | | | | | 626567 (22 run) | 1560178 (21 run) | 725375 (22 run) | 7772 (22 run) | 162058 (22 run) |

| engine | queries both ran | its time over Amphora's (geometric mean) | queries it was faster on |
|---|---:|---:|---:|
| sqlite | 21 | 1.198 | 13 |
| duckdb | 22 | 0.289 | 16 |
| duckdb-mt | 22 | 0.021 | 22 |
| postgres | 22 | 0.328 | 20 |
