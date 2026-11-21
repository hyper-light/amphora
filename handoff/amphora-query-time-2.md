# amphora-query-time-2 handoff (goal 7788ef29's implementation steps)

## Step 3 (plan step 01a1898b534070889eb4b2ff967c51d3): implemented, handed off 2026-11-11
- Branch t1/query-time-2 (local, unpushed), worktree ~/Projects/amphora-t1-query-time-2.
  Head c943d4cffaf7f564a4f1f55930ae06396fba649e, tree 6b7558ed263f65284251475e1f6419e44372468c,
  two commits on 490ea83 (d7e10be step 3; c943d4c the hit path made safe, per your item 1).
  Rebase onto b151af0 at merge, as you said.
- What: docs/design/executor.md decision 7; docs/requirements.toml [[own]] QT.3 (AC-QT.3.1..4 the
  plan's criteria, T-QT.3.1..5); crates/engine/ORIGIN.md row.
  (a) A per-cursor parse of the row's record header (crates/engine/vdbe/row_header.rs, a field of
      BTreeCursor), bound = the cursor's column count from the schema (no picked constant); dropped
      with noted_payload (every move/write/save) and whenever another payload is noted.
      Column/ColumnRange read through BTreeCursor::record_payload_with_header.
  (b) Page-cache hits under the shared lock: PageCache::get_shared + Pager::cache_lookup
      (read_page_into's hit path and cache_get); ref_bit an AtomicU8 bumped by load+store; the
      sweep stays under the write lock. Flush path (cacheflush_collect) unchanged.
  (c) Non-covering index loops test the index-covered WHERE terms first
      (translate/main_loop/conditions.rs, DeferredIndex), SQLite's wherecode.c first pass.
  (d) Column's panic! on a cursor id and register slicing -> typed errors; ratchet lowered:
      panic 201->199, indexing_slicing 3214->3207, arithmetic_side_effects 3401->3396 (delta counted
      on adas-mac-studio.local-2, base 490ea83 vs this tree, with the gate's own counting pass;
      unwrap/expect/etc. unchanged). The Linux gate's check-production.sh confirms the exact lines.
- Tested on adas-mac-studio.local-2 (aarch64 macOS):
  - cargo test -p amphora-engine --locked: all pass (lib 2,678 passed, 21 ignored; every
    integration file ok).
  - cargo clippy -p amphora-engine --all-targets --locked -- -D warnings: clean. cargo fmt --check: clean.
  - New: vdbe::row_header tests (T-QT.3.1), storage::btree a_cursor_reads_each_row_it_moves_to_from_its_own_header
    and tests/row_header.rs (T-QT.3.2; mutant "forget is a no-op" fails all three),
    tests/index_filters.rs (T-QT.3.4; red before: "table rows fetched: 200; the index's term keeps
    20 of the range's 200").
  - Shuttle (T-QT.3.3): RUSTFLAGS="--cfg shuttle" cargo test -p amphora-engine --lib -- page_cache::shuttle_tests
    passes (2,000 interleavings each), BUT only with a temporary local swap in
    crates/engine/storage/shared_pages.rs:128 (try_update -> fetch_update): see the finding below.
- Not run (yours): the seven gates on the batch tree; the benches.
- Bench for step 3 (AC-QT.3.1..4): bench/planner/record.sh on jupiter, change and parent (b151af0),
  release amphora-bench from each, TARGETS=amphora, best of 3, each run begun at load < 2:
    TARGETS=amphora bench/planner/record.sh <amphora-bench.exe> <out>/<rev> <data dir> <JOB queries> tpch-sf1-a tpch-sf10-a job-a
  AC-QT.3.1: q06 SF 1 in cache best time / 6,001,215 <= 68 ns; AC-QT.3.2: JOB 5a, 5b best time /
  2,609,129 <= 40 ns; AC-QT.3.3: JOB median change/parent <= 0.5; AC-QT.3.4: SF 10 median <= 4,192 ms.
  The plan says the x2 is the analysis's assumption; step 1's profiles (still yours) bound it.
- Finding (not mine, not fixed): crates/engine/storage/shared_pages.rs:128 (552c0a1, the replica's
  local readers) calls AtomicUsize::try_update, which shuttle's atomics lack, so every
  `--cfg shuttle` build of amphora-engine fails to compile (E0599). ORIGIN.md already notes
  fetch_update/try_update's split across toolchains. The branch that owns shared_pages.rs should
  route it through crate::sync (or fetch_update under cfg(shuttle)).
- Unsafe (your item 1), fixed in c943d4c: get_shared reads only the map's slot (a Weak<Page> beside the
  entry pointer, and the ref count moved there as an AtomicU8); no unsafe on the shared path.
  page_cache.rs: 19 unsafe blocks against the base's 20 (inherited derefs edited in place, ref_of's gone).
  Cost: the eviction sweep does one map lookup per entry examined. After it, on local-2:
  cargo test -p amphora-engine 2,962 passed / 0 failed, clippy -D warnings clean, fmt clean, shuttle
  page_cache tests pass with --test-threads=1 (eed5a1a's fix applied by hand for the run only).
- Disallowed methods (item 6): none in the diff (then_some, thread::sleep, env::args, env::temp_dir).
- Ratchet: the Linux lines I lowered are from a macOS delta; you recount on Linux at merge.
- Nothing left running; no lock held.

## Next: step 5 (hash joins where a seek exists, by cost, with predicate transfer).

## Step 5: in progress. Answered by the Amphora agent: build PL.5's star-join filters inside step 5 (planner.md
## decision 23 for PL.5, executor.md decision 8 for step 5, PL.5's targets from the planner plan's step 3); don't wait
## for step 1(d). The questions as first asked:
(agents_send to it was refused three times with backpressure, 429 pending 3 / authority 2; this file is the record)
1. Predicate transfer is planned to come "after PL.5's star-join filters (its own goal)"; PL.5 is not built and is on no
   agent's branch. Build PL.5's filters inside step 5 (recommended: one Bloom-filter path serves both), or leave
   predicate transfer until PL.5 lands?
2. Step 1(d), JOB and SF 10 with rhs_has_selective_seek forced false, was to direct step 5's cost choice and hasn't run.
   I'll build the cost-based choice anyway, and your step 5 bench against its parent is that measurement, unless you want
   1(d) first.
Found so far: the cost comparison already exists (join.rs, `hash_join_method.cost < best_access_method.cost`); the gate
only keeps a hash join from being weighed. MAX_MATERIALIZED_BUILD_ROWS (join.rs:75, 200,000) guards no memory: a
materialized build input goes to an ephemeral B-tree on its own temp pager. Replacing it needs a derived spill/disk
budget for the statement, which doesn't exist yet. The hash table's DEFAULT_MEM_BUDGET (64 MiB; 32 KiB on wasm),
MIN/MAX_PARTITIONS (16/128) and the planner's materialize cost 0.003 and Bloom ratio 2.0 are picked constants that
step 5 has to derive (Ada's rule). SQLite derives its sorter's in-memory limit from the connection's cache_size
(vdbesort.c); the same rule would give the hash join's budget.

## Step 5, 2026-11-12: first part built, NOT committed: it exposes a finding that needs your call
Uncommitted in the worktree (branch head stays c943d4c, clean for merge): the hash/group tables' memory budget derived,
no picked constant. Pager::statement_memory_budget = the connection's own cache capacity x page size (SQLite's sorter
rule, sqlite3VdbeSorterInit in vdbesort.c; the shared pool left out, else every statement gets all of it); each of a
statement's hash and group tables holds an equal share (PreparedProgram::memory_tables, counted at build;
hash_table::table_share); the planner costs with the same budget (CostModelParams::statement_memory, via the resolver).
DEFAULT_MEM_BUDGET (64 MiB release / 32 KiB debug) is gone; the 32 KiB stays only as a #[cfg(test)] fixture.

Finding: with it, records_plans.rs a_one_row_lookup_needs_no_sort_for_its_columns fails in debug:
  SELECT p.id, q.v FROM p JOIN q ON q.a = p.x AND q.b = p.y ORDER BY p.id, q.c  (2,000 rows each, ANALYZEd)
  got  ["HASH JOIN p", "SCAN q", "USE SORTER FOR ORDER BY"]
  want ["SCAN p", "SEARCH q USING INDEX sqlite_autoindex_q_1 (a=? AND b=?)"]   (SQLite's plan)
The base 490ea83 fails it the same way in RELEASE (cargo test --release -p amphora-engine --test records_plans, on
local-2): production already plans a hash join + sort here; the suite passes only because debug builds used a 32 KiB
budget that costs the hash join as spilling. Root cause: the cost model prices a 2,000-row hash join plus a sort below
2,000 unique index seeks, against the plan's own evidence that index nested loops win under 100,000 rows (f36). That is
step 5's "chosen by cost" core, and removing rhs_has_selective_seek would spread the same mis-pricing to every seek.
Fixing it at its cause means the hash join's and the seek's costs measured (PL.7's ground, "the cost model against
measured time"), not picked; I can't produce those numbers without benchmarks, which are yours.
Question: how do you want this sequenced? I recommend: (1) you (or I, if you allow a measurement run) time the two
plans across build sizes on one quiet host (index nested loops vs hash join, 10^2..10^7 build rows), (2) I derive the
hash join's and seek's cost terms from those times in executor.md decision 8, which fixes this test at its cause,
(3) then the gate goes and the rest of step 5 (builds over joins, PL.5, predicate transfer) follows. Until then the
budget change stays uncommitted, since committing it means a failing test.
Other picked constants step 5 still has to derive: hash_table MIN/MAX_PARTITIONS 16/128 (candidate: max = budget share
/ page size, the partition write buffers that fit; min from the planner's build estimate), the hash table's initial
1,024 buckets (from the build estimate), join.rs's materialize cost 0.003 and Bloom ratio 2.0 (from the same timings),
MAX_MATERIALIZED_BUILD_ROWS 200,000 (guards no memory: the input is an ephemeral B-tree; needs a derived disk budget).

## Step 5, 2026-11-13: the join-methods measurement is running (your go)
- Harness: bench/join_methods/ (its own workspace, engine only, the product's release profile): a(id,k), b(id, key UNIQUE, v)
  of n rows, n = 10^2..10^7; the same join planned NestedLoopsOnly (SCAN a | SEARCH b USING INDEX) and HashJoinsOnly
  (HASH JOIN b | SCAN a), plus each table's scan, interleaved, best of 3, each run's start load recorded, plans and answers
  recorded; cache = 2x the file's pages. run.sh takes the heavy lock, builds, waits for load1 < 2, runs.
- To force the method, a new connection setting (uncommitted, with the budget change): Connection::set_join_methods
  (JoinMethods::{ByCost, NestedLoopsOnly, HashJoinsOnly}); NestedLoopsOnly also allows no automatic index; HashJoinsOnly
  skips both seek-preference gates (rhs_has_selective_seek in join.rs and the probe/build index check in
  try_hash_join_access_method); ByCost is today's behavior. Step 5's own test (every join both ways) uses it too.
- Running: kiln run r25 on adas-mac-studio-3 (2768d0d5), heavy lock taken by the script (owner amphora-query-time-2),
  output ~/.cache/qt2-jm-out/{runs.jsonl,host.txt,uncommitted.diff} there. Studio-2 was refused: 7.1 GB disk free.
  Studio-3 at launch: load 4.1 (another session's clippy, and a VM holding one core for 12 h); the script waits for < 2.
- Quick debug check on local-2 (not a record): plans and answers right at 10^2..10^5.
