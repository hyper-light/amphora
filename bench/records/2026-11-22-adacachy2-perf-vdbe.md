# perf/vdbe, 2026-11-22: AUDIT section 3 items 2 and 3 (first hour)

Checked against the code at origin/main b817508b before changing anything.

## AUDIT 3.2: the statement memory budget read per row (verified, changed)

`op_hash_build`, `op_hash_distinct` and `op_group_find` called `pager.statement_memory_budget()`
(the page cache's `RwLock` read plus a SeqCst load) and `connection.get_temp_store()` on every
input row, though the value is only used when the table is made. Now `table_memory()` reads both
once, inside the branch that makes the table. Behaviour is unchanged: the value was never used
after the table existed.

## AUDIT 3.3: side tables and the sampling counter (verified, changed)

- `ProgramState::{hash_tables, group_tables, bloom_filters, star_orders}` were SipHash
  `HashMap`s keyed by the program's dense ids (hash table ids from `alloc_hash_table_id` or a
  table's internal id, cursor ids, instruction addresses). They are now `vdbe::id_map::IdMap`, a
  `Vec<Option<T>>` indexed by id. `op_star_probes` did 2K+2 SipHashes per fact row; now indexing.
- `timed_run()` called `LearnedCosts::sampled`, a `fetch_add` on one database-wide `AtomicU64` per
  Seek, HashBuild and HashProbe. Each connection now counts its own runs
  (`Connection::cost_runs`, a relaxed load and store); the shared `every` is only read. One in
  `every` runs is still timed, so the systematic sample and the rates it moves are as before.

## A/B

Not run in this hour (the one-hour limit; the macOS runners were queued). The successor runs:
TPC-H SF1 (`bench/planner/tpch/dbgen.sh 1`, `amphora-bench --planner --answers`), JOB and
`bench/baselines/run.sh` join kinds, release builds, origin/main b817508b vs this branch, on one
machine, repeated to know the noise.

## Second hour (perf-E2): why the filtered suite was killed, AUDIT 3.1 begun

**The kill was the branch's.** The filtered lib suite (`vdbe:: learned_costs id_map group hash
star bloom`, `--test-threads=1`, each in a `systemd-run --user --scope -p MemoryMax=` scope reading
its `memory.peak`) on adacachy2 (32 cores, 128 GB; load 30-69 from other work):

| tree | outcome |
|---|---|
| perf/vdbe 00d2c70f | `vdbe::leaf_batch::tests::ns_2_1_*` terminated; with leaf_batch skipped, `vdbe::morsel_scan::tests::t_qt_6_10_*` OOM-killed at a **61.4 GiB** peak in 30 s (journal: `run-p1496583 scope: Failed with result 'oom-kill'`) |
| main 8a84d370 | ran past both; then sat in `t_qt_6_18_join_workers_run_a_copy_of_the_loop_body_as_the_serial_loop` for 30+ min (stopped by me): main's own hang |
| perf/vdbe + fix 8dfd4c58 | 538 run, **peak 179 MiB**, no OOM; fails only tests main fails identically |

Cause: `IdMap` indexed hash/group tables by raw id, and `alloc_hash_table_id` counts from
`HASH_TABLE_ID_BASE` = 1 << 30, so the first allocated table made 2^30 slots. Fixed by giving each
id range its own dense count (interleaved slots).

Failing on main 8a84d370 and identically on the branch (same panic lines): `limbo_error_stays_small`,
`t_qt_6_24` (FULL JOIN answer differs, morsel_scan_tests.rs:1659), `t_qt_6_31` and `t_qt_6_32`
(builder.rs:2131), `t_qt_6_33` (morsel_scan_tests.rs:3259), `t_qt_6_35`; `t_qt_6_18` hangs.
All are perf-G's morsel/leaf-batch area.

**AUDIT 3.1 (verified, partly changed).** The in-memory table was fixed at `initial_buckets` =
1,024 chained buckets, never grew, indexed `% len`. Now a power of two doubling at load factor 1
(Knuth TAOCP vol. 3 6.4: chaining visits 1 + a/2 entries on a hit, a + e^-a on a miss), indexed by
the hash's low bits, the grown array's bytes counted in the table's budget and freed at a spill.
Not done: open addressing in an arena, and the zero-allocation probe.

**A/B: not run.** `bench/` has no Cargo.lock, so `--locked` refused; with `--offline` main
8a84d370's release `amphora-bench` failed to compile (an `AtomicX::try_update` call), and the hour
ran out. The successor runs TPC-H SF1 (`--answers`) and `bench/baselines/run.sh join-kinds`, main
before 2d939193 vs 2d939193, alternating on adacachy2. A script for it: `.perf-vdbe/ab.sh` in the
perf/vdbe clone (untracked).
