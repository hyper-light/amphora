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

## Third hour (perf-E3): the zero-allocation probe; the A/B started, not finished

**AUDIT 3.1's probe allocations (verified, changed).** On 90a81a80 a probe row of an in-memory hash
join allocated: the probe keys' `Vec<Value>` and a clone of each text or blob key (execute.rs
`hash_probe_run`), a `Vec<ValueRef>` to hash them and another to compare them (`HashTable::probe`),
and one more on every `next_match` and `merge_next_match` call; `probe_partition` and
`partition_for_keys` did the same on a spilled table. Now:

- `HashTable::probe_keys_from` hands the last probe's key buffer back refilled in place
  (`Value::try_clone_from`, so text and blob keys reuse their own allocations);
- the hash (`hash_join_values`) and the comparisons (`keys_equal_values`) read the keys as held,
  through one per-value hasher (`hash_key_part`) shared with `hash_join_key`, so a build row and a
  probe row hash alike by construction;
- `probe_partition` keeps its keys in the same buffer.

Test: `vdbe::hash_table::hashtests::audit_3_1_probe_rows_allocate_nothing` counts the thread's
allocations (a counting global allocator in the engine's lib tests) over 1,000 probe rows with an
integer key and a NOCASE text key, half hits (two matches each, walked with `next_match`) and half
misses, and asserts **0 allocations** (before the change the same flow made at least 3 per row plus
one per `next_match` call, by the code above; not counted on the old tree). Verified on adacachy2
(kiln run, `.perf-vdbe/check.sh`, the committed tree): `cargo clippy -p amphora-engine --all-targets
--locked -- -D warnings` exit 0; `cargo test -p amphora-engine --lib --locked -- vdbe::hash_table`
46 passed, 0 failed, the new test among them. Not run: the SQL-level join suites (tests/hash_*.rs,
join_methods.rs) that drive the changed `HashProbe` opcode.

Not done: a row probing a spilled table's loaded partition from `HashProbe` still allocates its kept
buffer once (the register refill took the buffer; `probe_partition` takes a slice); the
open-addressed arena layout (entries still `Vec<HashEntry>` per bucket, two `Vec<Value>` per entry).

**A/B: builds proven, timing not run.** main before the vdbe work (b817508b) built in release with
`--offline` under the pinned rustc 1.99.1: the predecessor's `AtomicX::try_update` failure did not
recur (both trees pin 1.99.1 in rust-toolchain.toml and use `try_update`; the failure was not the
toolchain pin as read here). A three-arm script (`.perf-vdbe/ab2.sh` in the vdbe clone: base
b817508b, head 90a81a80 in its own clean clone, probe = head + this change; TPC-H SF1 with
`--answers`, arms in turn, then SQLite and DuckDB through head's driver) was started at 02:46Z and
stopped by me at 03:05Z: adacachy2's load average was 88-111 on 32 cores (2 cores idle), so no
timing taken then would be a number. The successor runs it when the host is quiet:
`bash .perf-vdbe/ab2.sh` through kiln `run` on adacachy2 (PASSES, RUNS, TIMEOUT, CRUNS env).
