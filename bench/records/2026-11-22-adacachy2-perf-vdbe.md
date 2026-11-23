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
