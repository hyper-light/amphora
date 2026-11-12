# Workload traces for the follower-write candidates (2026-11-07)

Step 2 of kiln plan 01a1b8d174dc72d18b9d2289b6e8fe11 (goal 9c63f7d4; docs/research/follower-write-starvation.md, f2, f5,
f8, f9, f10). What each transaction of six worst-case workloads read, wrote and allocated, page by page, prepared as
the replicated owner prepares (`Db::prepare_traced`: `BEGIN IMMEDIATE`, the session, capture, `ROLLBACK`), then applied
as a replica applies it (`Db::apply_pages`, published by a marker write as `replication.rs`'s `marker` does). These
are inputs to the candidates' model (step 3), not timings: CLAUDE.md §7's quiet-host rule doesn't bear on them.

- **Host:** adas-mac-studio.local-2 (runner d1323898), M4 Max, macOS, load 9.7 to 10.2 (other agents' builds).
- **Rerun with `grow`** (two writers growing different tables, added for the model, step 3) on 2026-11-08 with the
  same code: the six other workloads' summaries unchanged, 12,200 transactions in all, stray pages 0; sha-256 of the
  uncompressed file 93f2ea829282bcfd94d1bd11774d7375009175e47a7cef2e88b1b0793ab2d538.
- **Build:** debug test profile, `cargo test --locked -p amphora --lib db::access -- --include-ignored`, commit
  7713063 plus its fmt-only follow-up (the traced code unchanged), `AMPHORA_TRACES` set. 35.9 s.
- **File:** `2026-11-07-studio-local2-write-starve-traces.jsonl.gz`, one JSON line per transaction: `workload`, `seq`,
  `role` (`L`, or `F` for every tenth), `read`, `written`, `allocated` (page numbers), `stray` (written, neither read nor
  allocated), `page1` (page 1's parts it changed: `size`, `freelist_trunk`, `freelist_count`, `schema_cookie`,
  `change_counter`, `schema_body`, `reserved`, `header_other`), `keys_read`, `keys_written` (`table#key`; `#max` the
  highest key read to append after it, `#new` a key appended). A `summary` line ends each workload.

| workload | transactions | what each does | read pages (median, max) | allocating | page 1 written | stray pages |
|---|---|---|---|---|---|---|
| append | 2,000 | one 120-byte row appended to one table | 3, 4 | 64 | 64 (size, reserved) | 0 |
| hot | 2,000 | one of four counters incremented | 2, 2 | 0 | 0 | 0 |
| random | 2,000 | one of 20,000 rows updated, its indexed tag too | 6, 7 | 0 | 0 | 0 |
| big | 200 | 200 random updates and 300 appends | 236, 245 | 200 | 200 (size, reserved; 1 size only) | 0 |
| disjoint | 2,000 | leader updates table a, follower table b | 3, 5 | 1 | 1 (size, reserved) | 0 |
| grow | 2,000 | leader appends 400-byte rows to table a, follower to table b | 3, 4 | 221 | 221 (size, reserved) | 0 |
| kiln | 2,000 | leader: an event appended, a session updated; follower: its runner row updated, an event appended | 7, 11 | 120 | 120 (size, reserved) | 0 |

What they show:
- **Every transaction reads page 1** (all 12,200): the header's schema cookie. Page-level validation that treats page 1
  as one page makes every allocating entry conflict with every concurrent transaction.
- **A write to page 1 changes only the database size and the page's checksum (its reserved bytes)** in every trace:
  no freelist, schema or cookie change. Merging page 1 therefore needs only the size field merged, and the checksum
  recomputed, plus a rule for two writers allocating the same page number from the same size.
- **No transaction wrote a page it neither read nor allocated** (the self-check, 0 of 10,200): the read set covers the
  write set, so validating reads alone also guards the page images' overwrite.
- Appends conflict on their table's rightmost leaf whatever page 1 does: 3.2% of appends and 6% of the kiln mix
  allocate, but every append writes the last leaf.

Tests (crates/amphora/src/db/access.rs, AC-T1.8 follow-up): `a_prepared_transaction_records_the_pages_it_read_and_wrote_nothing_unread`
(red with the read hook removed: run on the same host, "assertion failed: !first_leaves.is_empty()"),
`an_access_record_past_its_bound_is_refused`, `workload_traces` (the self-check).
