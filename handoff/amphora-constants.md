# amphora-constants handoff (Ada, 2026-11-11: "We don't tolerate arbitrary constants, learned and stat derived values only")

Worktree ~/Projects/amphora-t1-constants, branch t1/constants from 490ea83. Not pushed, not merged.
The list of every picked value and its derivation: handoff/amphora-constants-list.md (36 rows).

## Area 2: the WAL and the packed log (done, 18c48cd)
Branch head now: e644cba.
Commits (on 490ea83):
- a56f23c: a page's chain of deltas ends where its bytes reach a page, not at a picked 8 (store.md decision 17).
  One rule replaces both MOST_DELTAS 8 and the half-page-a-delta fraction. A delta is written while the chain's
  delta bytes since its image, this one included, stay under a page (the ski-rental break-even, Karlin et al. 1988).
  Logged gains `chain`, packed into the 24 bits above the generation in logged_as. MOST_DELTAS is now u8::MAX, the
  depth field's width (a format bound). Readers look back up to 256 frames; the walk is lazy, so only the chain's
  records are read.
- 6504d62: clippy's absurd-comparison fix (`==` at u8::MAX).
- 18c48cd: the store's log is copied once it passes its bound; Config::copy_after (8,192) is removed (decision 35,
  "No count of frames"). The store tests' Events engine now reports `uncopied`, as database::Local does. The crash
  seeds take bounds of 8 to 23 frames instead of copy_after 0 to 7.

Red runs (adas-mac-studio.local-2):
- packed_log pages_changed_again_* : "chains of small deltas end at 8" (both formats).
- store a_log_within_its_bound_is_not_copied: "passes with 10242 frames in a log of bound 12288" (recorded with
  20-byte keys; the test now uses 1,000-byte keys to run in 3.5 s, same assertion).

Green (local-2, debug): engine packed_log 12, spilled_log 2, `--lib storage::` 534; amphora store 15, amplification 2,
crash 5 (default seeds). clippy -p amphora -p amphora-engine --all-targets -D warnings: clean (macOS).

Heavy runs for the batch:
- Crash injection, packed every other seed: `AMPHORA_CRASH_FROM=0 cargo test --release -p amphora --test crash -- --ignored`
  over 10,000 seeds as for decisions 16/17/35 (the seeds' bounds changed).
- Decision 4's bench at 32 writers: tails with copies only past the bound against 490ea83 (the falsifier in
  decision 35, "No count of frames").
- Decision 17's falsifier: a cold read at the end of a long chain of small deltas, against chains cut at 8.
Format note: a packed or mixed log written by this branch can hold deltas deeper than 8, and an older binary refuses
it as corrupt. The packed log is opt-in (decision 16) and the page log is unchanged. Downgrading with such a log
needs a checkpoint first. Is that acceptable, or does it want a new header format value?

## Area 1, part one: the planner's sample budget (done, 1f697b4)
- MAX_OPS 64 and SAMPLE_STEPS (64 × 1,024 × 16) are removed (planner.md decision 21). A term's program holds one
  operation per node and runs once per sample row. The sample has no more rows than the table, so the work never
  exceeds one scan running the same terms. The walk down parentheses takes the parser's MAX_EXPR_DEPTH.
  Red: sample_filters a_long_term_is_estimated_on_the_sample ("estimated 20000, counted 10000"). Green:
  sample_filters 8, planner 14, join_search 2, column_stats 6, optimizer unit tests 70.
- Heavy run for the batch: the planner bench (JOB planning time, decision 21's falsifier) and the CERT oracles.

## Area 4: the local owner (done, a802a84)
- FIRST_PAUSE 50 µs / LONGEST_PAUSE 1 ms → learned (the probe's own cost, doubling, capped at the last drain
  window this process measured). DRAINED_MAX → OWNERS_MAX, named once. Green: database:: 128; guards+remote 19/19
  ×3; clippy clean. Sent to amphora-seam-owner-2 for review (J1 was the previous owner's decision).

## Area 3, part one: values that follow from another bound (done, 36bd4c6 + f566854; seam owner's review asked)
- class.rs ENTRY_BYTES 64 KiB → 1,317 bytes from postcard's varint widths × MEMBERS. Red:
  a_class_entry_holds_the_most_members_and_no_more. patience PEERS → MEMBERS. CHANGE_BYTES → size_of::<Input>().
  Green: amphora lib 173; clippy clean. Notice: codex-notices/t1-constants.md (with design questions on
  QUORUM_CEILING, GENERATION, FLOOR_SESSION, SNAPSHOT_BYTES, CONNECTION_MAX).

## amphora-seam-owner-2's rulings (2026-11-11, codex-notices/t1-constants.reply.md)
- Code: ENTRY_BYTES, PEERS and CHANGE_BYTES agreed. a802a84 agreed with one change, made in f7e9c94: the pause cap
  is max(learned window, wait so far). Red "23259 probes in a second" with the old cap, green with the fix.
- (a) QUORUM_CEILING and GENERATION aren't to be learned at runtime (that's circular). For the batch's bench owner:
  re-derive both from recorded runs on a quiet and a loaded host of each OS class, take the largest p99.9, and
  record it in bench/. GENERATION's 200 ms is written as election_tick × TICK (ff65b87).
- (b) FLOOR_SESSION and the seed's 60 s: agreed with corrections and done in ac1fe63. The lease is each role's raft
  timing (a leader 2 × (election_tick + quorum_patience) ticks, a follower 2 × election_tick + patience). Red "60s vs
  400ms". Green: lib 175, tests/replication 3. Agreed by the owner (amphora-owner-t1-constants-ac1fe63-20261111).
  c892b99 names its event replica.own_session_expired, and the event is the record (agreed,
  amphora-owner-t1-constants-c892b99-20261111). The name, and an optional
  counter amphora_replica_own_sessions_expired_total, were agreed by Kiln Metrics (amphora-constants/names/v1).
  721acf2 adds the counter, panel and monitor. Note: c892b99 alone fails the doc-truth test
  meter::the_replica_metrics_are_the_ones_the_design_names; 721acf2 fixes it, so take them together.
- (c) SNAPSHOT_BYTES stays a memory bound, stated as picked until a learned process memory budget exists (its own
  goal). log-bound's disk reserve should use the database's own size instead (told).
- (d) CONNECTION_MAX, OWNERS_MAX and READERS: one process descriptor budget from RLIMIT_NOFILE, with a named
  Windows source (the handle quota or Winsock's socket limit). The owner recommends its own goal after this batch:
  yours to decide.
- MEMBERS = 64 is picked too (added to the list).

## After the Amphora agent's answers (2026-11-12), head 2026-11-12 below
- aef3637 (c): generations begun now carry 0x414d_5004 / 0x414d_5005; one begun under 0x414d_5002 / 0x414d_5003 keeps its
  format and its bound of 8 (packed::most_deltas). An older binary refuses the new values by name; to downgrade,
  checkpoint first (decisions 16, 17). Red: a_generation_keeps_the_chain_bound_of_the_format_it_began_with. Green:
  packed_log 13, shared_pages 9, spilled_log 2, storage:: 534, clippy clean.
- 6cb6168 (b): NAME_BYTES 128 is removed; api::family's FAMILY_MAX (64) already bounded names. FAMILY_MAX is list row 38.
- The latest commit: compaction's PAGE_ENTRY and MIN_ROOM written as const expressions (format, unchanged).
- The pre-window inventory is in the list's "Pre-window values" section: area 5 (now), area 3 (after g-slot and
  log-bound land), and the API's limits (a question for Ada).
- Questions open for area 5 (my recommendation first): the lock-shard counts (store.rs SHARDS, shared_pages SHARDS,
  64): available_parallelism with a stated contention target. Is a target such as "expected collisions under one
  per round" acceptable as stat-derived? Db's busy and lock waits (BUSY_WAIT/BUSY_TRIES, LOCK_WAIT, the open's 100 ms):
  thread the caller's deadline through Db's API, which changes every caller. Is that change acceptable?

## Seam owner's state (2026-11-12)
Every t1/constants commit that touches its files is agreed through e644cba (amphora-owner-t1-constants-e644cba-20261112). SNAPSHOT_BYTES stays stated as picked until a memory budget exists. Its final review comes on the batch's merged tree after the gates.

## Questions for Ada (the planner's statistical choices; my recommendation first in each)
1. JOIN_SEARCH_BUDGET 10,000 (Neumann & Radke's pick): spend no more on the exact search than the greedy plan in
   hand would cost to run (ski-rental). The greedy search runs first. The search continues while its subsets ×
   the measured time per subset stay under the greedy plan's cost × the measured time per cost unit, learned
   from finished statements (elapsed ÷ estimated cost). Until a statement has been measured, the prior is the
   bench's recorded ratio on that host class. MEMO_SETS then follows the budget in force. Alternative: keep 10,000
   as the cited literature value.
2. The backoff exponents 1, 1/2, 1/4, 1/8 (SQL Server's): where the sample has fewer than its hit floor, take the
   conjunction's posterior on the sample instead. A Beta prior centred on the backed-off product, updated by the
   hits observed, becomes the estimator, so no exponents are needed. Its prior weight would be the sample's own
   effective size, derived, not picked.
3. SAMPLE_ROWS 1,024, SAMPLE_HITS 10, HLL PRECISION 11: size each table's sample at ANALYZE so the sample's
   standard error resolves the cost model's index-or-scan crossover selectivity for that table (computed from
   its row and page counts) within half its distance. The hit floor is the count at which a share's error is
   that tolerance. The sketch's precision is the smallest whose 1.04/√m is within the same error, at most the
   rows counted (the sketch header already carries its precision). This changes the sample format's size per
   table, and the bench must show no estimate got worse.
4. SAMPLE_BYTES 1 MiB and LOADED_SAMPLE_BYTES 64 MiB: charge samples to the process's page budget in force (the
   Account the readers already charge), refused past it, so no separate number is needed.
5. constraints.rs's z = 2 (two standard deviations, the common-value cut): Bonferroni over the values tested, at
   a family error stated once for the planner. One stated significance level is still a choice: is that
   acceptable as a "stat-derived" value?
6. The packed log's chains past 8 deltas: an older binary refuses them as corrupt. Is "checkpoint before a
   downgrade" acceptable (the packed log is opt-in), or should it get a new header format value?

## Left
- Area 1: questions 1-5 above (Ada's word), then JOIN_SEARCH_BUDGET, MEMO_SETS, backoff, the sample's sizes and
  budgets, and the z cut.
- Area 3: the rest, after g-slot and log-bound land. QUORUM_CEILING/GENERATION are a bench
  re-derivation (above). SNAPSHOT_BYTES and the descriptor budget are their own goals.
- The store's log_frames, keep_log 2× and /64, after amphora-log-bound's rule is agreed (t1/log-bound 004f682).
  Its note: that rule learns when to restart and how far the log may grow, not when to copy. If the store takes
  it, the copy trigger (since 18c48cd, "past the bound") needs its own source ("a restart is due, or past the
  room"), or the store copies only near the disk room.
- NAME_BYTES 128 (application names): no grammar bound to derive it from. Admission policy or Ada's question.
- Flagged and older than the window, not done: CONNECTION_MAX 32, OWNERS_MAX 4,096, TICK/POLL/DRIVE_QUANTUM,
  OUTGOING_MESSAGES, the store's queue 4,096/commit_rows 512/readers 4/checkpoint_every 25 ms, DECODED_BUDGET
  4 MiB, RECORD_BYTES 256 KiB, the engine's fixed range/LIKE selectivities (0.4, 0.005, 0.2).
