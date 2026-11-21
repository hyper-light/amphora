# amphora-planner: handoff 1 of 3, PL.1 and PL.3 on main db991a0 (2026-11-09)

Local branches, not pushed. Both rest on main db991a0 plus the t1/e5-row-value-seek and t1/tenx line they were built on.

## Heads
- PL.1 (b4c804c3): **t1/planner-stats-pl1 660b1ca8135f**, tree e7d0622c8a6c. 1fa7002 merges main db991a0 into 8f697df
  with no conflicts, then two bench fixes (below). t1/planner-stats itself goes on with PL.2: merge 660b1ca, not
  t1/planner-stats's head.
- PL.3 (f692c137): **t1/planner-search ce1bc84a8bbe**, tree e33c82f31cec (worktree ~/Projects/amphora-t1-planner-search).
  3f5f5a4 merges main db991a0 into 44faae7 with no conflicts, then the same two bench fixes (af2e604, ce1bc84).
- The benches' parent: **t1/planner-parent 2dc9a8cf7aea**, tree 3bb2806392a0: PL.1's parent eb999f8 merged with
  0d00af4 (db991a0 plus the two bench fixes). It is the same line and bench as both changes, without either change.
- PL.1 and PL.3 both touch optimizer/cardinality.rs and join.rs, so the batch merge of the two needs a look there.
  PL.3's f6fddfd lowered scripts/engine-ratchet.toml; the gate's check-production.sh on the merged tree checks it.

## Two fixes to what main has (both branches, and the parent)
- **main's bench does not build.** db991a0's bench/Cargo.toml has `[patch.crates-io]` twice: two copies, joined
  textually by 2472173's integration merge. `cargo metadata --manifest-path bench/Cargo.toml` fails with
  "duplicate key" at line 55, so the oracles and record.sh can't build. Fixed by 8289870 / af2e604 (the copy
  bench_workspace.rs's comment names is kept). main needs it too.
- **bench/oracles/sqlite.sh does not run on macOS.** `openssl dgst -sha3-256` is LibreSSL's there, which has no SHA3.
  It printed its usage and nightly.sh exited 2. Fixed by hashing with python3's hashlib (660b1ca / ce1bc84).

## Tested (adas-mac-studio.local-2, macOS arm64; the merged trees)
- t1/planner-stats 1fa7002 (PL.1's code is the same in 660b1ca): `cargo clippy -p amphora-engine --all-targets --locked
  -- -D warnings` exits 0. `cargo test -p amphora-engine --lib optimizer`: 66 passed. Integration tests: planner 14,
  oracle_findings 83, row_counts 7, records_plans 5, row_value_seek 5, top_n 3, glob_range 4, skip_scan 4,
  in_subquery 4, implicit_rowid_seek 4, without_rowid_seek 6, rowid_index_order 4. All pass, none fail.
- t1/planner-search 3f5f5a4 (PL.3's code is the same in ce1bc84): clippy as above exits 0. lib optimizer: 69 passed.
  join_search 2, planner 10, oracle_findings 83, row_counts 7, records_plans 5, top_n 3, skip_scan 4. All pass.
- Oracles' light leg: `ORACLE_SECONDS=300 ORACLE_SEED=11 bench/oracles/nightly.sh`, release amphora-bench, the
  reference SQLite 3.54.0 that sqlite.sh builds:
  - PL.1 (8289870's binary): 9,471 databases, 383,793 checks, **0 findings**; cert 47,350 checks 0 failed, order
    47,350 0 failed; diff, dqp, eet, norec, pqs, same and tlp 0 failed. Output in ~/.cache/t1-planner-stats-oracle-660b1ca
    on local-2.
  - PL.3 (af2e604's binary): 9,399 databases, 380,879 checks, **0 findings**; cert 0 failed, order 0 failed, the rest
    0 failed. Output in ~/.cache/t1-planner-search-oracle-ce1bc84.
  - Finding AH (LIKE in an outer join's ON with NOCASE/RTRIM) did not appear in either run.
- Not run (the batch's): the seven gates, the 1,800 s oracle seeds, the benches.

## The benches, on jupiter (yours)
jupiter's ~/.cache/planner-bench (the October data) is gone. The script makes it again once: dbgen SF 1 and 10, imdb.tgz,
and join-order-benchmark at a396036. It takes jupiter's heavy lock and frees it on exit. Each commit gets database dirs of
its own, so each one's ANALYZE is its own. Script: ~/.cache/claude-scratch/amphora/handoff/planner-bench.sh. Run its
text as the command of a kiln background run on jupiter, with cwd ~/Projects/amphora-t1-planner-stats (its .git
holds all three commits):

    SPECS="parent=2dc9a8c pl1=660b1ca pl3=ce1bc84" bash <the script>

It builds each commit's bench (`cargo build --release --manifest-path bench/Cargo.toml`; the parent's Cargo.lock
is copied to the others). Then `TARGETS=amphora,sqlite ANSWERS=... bench/planner/record.sh <bin> <records> <data> <JOB
queries> tpch-sf1-a tpch-sf10-a job-a` for each. Then `versus.py --change <pl's 3 records> --parent <parent's 3>`
and `answers.py <pl answers> <parent answers>` for each pass. Read against:
- PL.1: AC-PL.1.1 (JOB prefixes below one row counting 1,000 or more, under 35); AC-PL.1.2 (median q-error at three
  joins under 100, largest intermediates p95 under 1e+05); AC-PL.1.3 (JOB time over parent 0.85 or less by geomean;
  TPC-H SF 10 median q-error not above the parent's).
- PL.3: AC-PL.3.1 (SQLite over Amphora on JOB's 9 queries above 12 tables 1 or more; TARGETS defaults to
  amphora,sqlite for it); AC-PL.3.2 (every JOB plan in 92.1 ms or less, median 1.9 ms or less).
Then record in bench/records and put the numbers in decisions 18 and 19 and STATUS.
When PL.2 is handed off, add `pl2=<its head>` to SPECS: one load of the data serves all.

## Left, and what needs Ada
- Nothing needs Ada's word for this step. main's broken bench manifest is main's: whoever lands on main next should
  take 8289870's fix.
- Leftovers the previous agents listed (Ada's laptop ~/.cache/t1-planner-stats-bench-*, fedora units) I have not
  touched: they are on hosts I'm told not to use.

# Handoff 2 of 3: PL.2 (3492da3a), 2026-11-09

## Head
- **t1/planner-stats 5fc292f** (tree 6c6787eab93d). It holds PL.1 too: 1fa7002 and 8289870 are 660b1ca's line, and
  8eeadfa makes the same sqlite.sh change as 660b1ca. Then ff5fbb6 adds the failing tests, which were red at 8289870:
  0 of 5 passed. 5fc292f is the change, decision 20 in docs/design/planner.md. Merging 5fc292f brings PL.1 and
  PL.2.
- What it does: ANALYZE also keeps, in `__turso_internal_column_stats`, a HyperLogLog sketch a column (1,536 B, 2.3%) and
  a reservoir sample a table (1,024 rows, 1 MiB; past it, none). The schema loads them, the samples within 64 MiB.
  An `=` or IN list of constants reads them: a common value takes its sample share (PostgreSQL's MCV test decides
  which are common), another takes the rest over the distinct values left (var_eq_const). The connection's statements
  plan again after ANALYZE (SQLite's OP_Expire). A join column no index leads takes its sketch's distinct count.
- One existing test changed: without_rowid_analyze's `a_without_rowid_keys_statistics_reach_the_planner` now reads
  the key's sqlite_stat1 row through a join's domain. The equality it read now comes from each table's own sample,
  which differs between a WITHOUT ROWID table and its rowid copy (570.5 against 555.5 rows). The reason is in decision
  20 and the commit.

## Tested (adas-mac-studio.local-2, at 5fc292f)
- `cargo fmt --all --check` exits 0. `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` exits 0.
- `cargo test -p amphora-engine --locked --no-fail-fast`: **2,930 passed, 0 failed**. That covers lib, all integration
  tests, and the 6 column_stats.rs tests: 5 red at ff5fbb6, plus a WITHOUT ROWID guard. The 4 unit tests in
  column_stats.rs pass (T-PL.2.1).
- Oracle light leg, 300 s, seed 11, release bench at 5fc292f (the synthesizer runs ANALYZE): 9,956 databases,
  403,485 checks, **0 findings**; cert 49,778 checks 0 failed, order 0 failed, diff/dqp/eet/norec/pqs/same/tlp 0
  failed.
- Not run (yours): the seven gates (crates/amphora and the other crates were not tested here), the 1,800 s seeds, the
  benches.

## Bench (with step 1's, one data load)
    SPECS="parent=2dc9a8c pl1=660b1ca pl3=ce1bc84 pl2=5fc292f@pl1" bash <planner-bench.sh>
pl2 is measured over pl1, its parent. Read versus-pl2.json against AC-PL.2.1 (JOB first-table q-error p90 11 or less;
the 15 keyword prefixes within 1.1), AC-PL.2.2 (SF 10 q16's part within 10, q12's lineitem within 100) and AC-PL.2.3
(JOB and TPC-H no slower than pl1 beyond the passes' spread). Decision 20 also has a falsifier for ANALYZE's time:
more than twice the parent's on JOB or SF 10. The load-*.log/jsonl of each records/ dir holds it.

## For Ada or the Observe owner
- A new event name: `amphora.planner.stats`, `event = "sample_over_budget"` (a table's sample not loaded because the
  schema's samples would pass 64 MiB). CLAUDE.md §10 wants new names agreed with the Kiln Metrics agent. Not yet
  asked.
- The 64 MiB load budget is a stated bound, not yet measured on the bench's schemas. Decision 20 says so.

# Handoff 3 of 3: PL.4 (210578d2), 2026-11-09

## Head
- **t1/planner-stats 4b4befd** (tree 8ffbfa8ef124): PL.1 + PL.2 + PL.4. 885bda9 adds the failing tests
  (sample_filters.rs); a6a5fb0 is the change; 4b4befd samples inner-join ON terms as WHERE terms, and an outer join's
  ON terms apart and with its filters, fixing the oracle findings a6a5fb0 had. Decision 21 is in
  docs/design/planner.md. Merging 4b4befd brings all three.
- What it does:
  - JoinedSetRows runs each inner table's own WHERE terms over its sample, once a prepare, as flat programs within a
    counted step budget (optimizer/sample_filter.rs, no recursion). The share of the sample passing them all is their
    joint selectivity (≥10 rows), else the floor share capped by the terms' own estimates.
  - An OR term no constraint homes counts on the single table it reads; before this change it counted nowhere.
  - `=`/IN of constants on a sketched column stay PL.2's.
  - constraints_selectivity combines a table's filters (ranges, LIKE, others, the sampled share) by exponential
    backoff. Equalities, IN and join terms multiply, which keeps a whole key at one row.
- It changes estimates of every multi-filter table, with or without statistics: no-stats ranges now combine by
  backoff, not product. The engine's suite and the 300 s oracle pass (below). kiln's statements and the bench's 52
  should be measured as decision 21 says.

## Tested (local-2, at 4b4befd)
- fmt and `clippy -p amphora-engine --all-targets --locked -- -D warnings` exit 0.
- `cargo test -p amphora-engine --locked --no-fail-fast`: **0 failed** (2,937 passed). sample_filters.rs has 7 tests,
  all pass. The first 5 were red at 885bda9: 4 failed, with q06 0.2 against 339 counted and q19 4,627.7 against 611;
  the CERT guard passed there too. The other 2 hold the oracle findings below. The backoff unit test passes.
- Oracles, 300 s each:
  - a6a5fb0 (seed 11) had 30 order-check findings: an inner join's ON term was not sampled while the same WHERE term
    was. A middle step had 21 CERT findings, then 1 (outer join below its inner join).
  - **At 4b4befd: seed 11, 0 findings in 312,179 checks; seed 12, 0 findings in 543,604.** cert and order 0 failed.
  - Oracle findings are listed as "Estimate", which doesn't fail nightly.sh's exit. Read the summary, not the exit
    code.

## Bench
    SPECS="parent=2dc9a8c pl1=660b1ca pl3=ce1bc84 pl2=5fc292f@pl1 pl4=4b4befd@pl2" bash <planner-bench.sh>
Read versus-pl4.json against:
- AC-PL.4.1: SF 10 q19's part within 10 of its count; q19 under twice SQLite's 612 ms.
- AC-PL.4.2: q06's lineitem within 2; JOB first-table q-error p95 under 20.
- AC-PL.4.3: JOB median planning time at most 3.8 ms.
**Planning time is the risk**: the evaluator clones each value it reads, up to 1,024 rows a term a table on every
prepare. If AC-PL.4.3 fails, the fix is to evaluate on borrowed values (ValueRef) and stop a term's rows early once
its share is settled. The design's falsifier names it.

## For Ada or you
- Decision 21 cites the backoff exponents (1, 1/2, 1/4, 1/8 over the four most selective) as the research's rank 4
  gives them. Microsoft Learn confirms "a variation on exponential backoff, ordering the selectivities from most to
  the least selective", but not the exponents; I did not check them against Microsoft's own whitepaper.
- This is my last goal. I end after this handoff.
