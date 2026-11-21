# amphora-record-wal handoff (60805ec4 part B, step 2's bench records; goal 370cb46a), 2026-11-12

- Worktree ~/Projects/amphora-t1-record-wal, branch t1/record-wal. Head 9aad7737f95fd3accc7491fdeeacc9f2b53c8d05,
  tree 51f7952fd597878f71bfc77d4107a1f123e8326b, clean. Two commits on b151af0, both signed (x509):
  - ef6210f: bench/records/2026-11-12-studio3-small-commits-step2.md (merged into the batch as 6e0f59e).
  - 9aad773 (new):
    - bench/records/2026-11-12-studio-local-small-commits-step2-interleaved.md
    - a pointer section at the end of the studio-3 record
    - docs/design/store.md decision 37: a new **Evidence.** paragraph before "Falsified by".
- Not pushed, not merged. STATUS.md still has no line for the WAL's small commits; unchanged.
- The rerun:
  - Host: cec37f60 (adas-mac-studio.local), 2026-11-12 07:13:08Z-07:26:54Z, kiln background run r9, exit 0.
  - Command: `bash ./rerun-s2.sh` in the synced carry ~/.cache/claude-scratch/wal-redo-carry, at e301082
    (rerun-s2.sh + tree-check.sh + trees/, committed there).
  - Heavy lock: taken and freed (gone after).
  - Load gate off; loads 2.10-6.57, none under 2.
  - Trees checked file by file against each commit's ls-tree. Every file is the commit's except the harness,
    the two lockfiles, and base's bench/Cargo.toml (= the commit's + [patch.crates-io], exact).
  - Binaries already built on that host; cargo compiled 0 crates.
  - Outputs on the host: ~/.cache/wal-redo-rerun/ (log, per-run json, leg8 txt, prof/ folded/svg/txt).

## Against goal 370cb46a, both records together
1. Batched leg within decision 37's bound: MET (studio-3 record): disks 217-218 MiB vs 261; not rerun.
2. 1/4/8 MiB legs no slower than 13e385d:
   - 1 and 4 MiB met (studio-3 record).
   - 8 MiB MET interleaved: best of 3 14.25 s vs 14.28 s, each round within 1%. The earlier 1.7% slower doesn't hold.
3. Bulk statements no slower than 13e385d: NOT MET, the slowdown holds (best of 3, interleaved).
   - insert_batch_100: 80.0 vs 125.4/s at 1 client (-36%, 3 of 3 rounds); 92.8 vs 544.2 at 32 (-83%, 2 of 3).
   - delete_range_insert at 32: 974.4 vs 3,082.6 (-68%, 3 of 3); at 1 client +11%.
   - The earlier run's single-row speedups (+18-110%) don't hold: interleaved, -13% to +35%, mostly slightly slower.
   - Decision 37's falsifier holds.

## Where the time goes (from the run)
- Counters: step 2's writer spent 9.5-9.7 s of insert_batch_100's 12 s call in commits.
  - insert_batch_100: 12.6-22.4 ms a commit vs 2.7-3.7.
  - delete_range_insert: 4.3-5.3 ms vs 1.6-2.2.
  - It flushed less than base.
- Profile, writer thread, 32 clients:
  - 82.9% (insert_batch_100) and 72.0% (delete_range_insert) are under begin_read_record, mostly pread.
    - RecordRead::issue (4acb85d crates/engine/storage/wal.rs:614) reads the whole record that holds a page
      on a cache miss.
    - Base's page-frame reads are 65.9% / 46.8%.
  - prepare_records is 12.8-25.3%, vs prepare_frames 0.2-1.6%.
  - LZ4/delta decode is 0.0-0.8%.
- Not measured: the bytes the record reads took, the records' sizes, registry hit rate.

## Missing / open
- No load-under-2 run (gate off on Ada's word).
- The design answer for decision 37's bulk-statement cost: what a miss reads from a record, and the commit's
  record packing. Not proposed here.
