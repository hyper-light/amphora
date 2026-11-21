# amphora-wal-read handoff: decision 37's read path (Ada, 2026-11-12: "Fix the read path first")

Agent amphora-wal-read, a subagent of the Amphora agent (kiln 01a1ce3a7f547793aa239248162c3d3f). Worktree
~/Projects/amphora-t1-wal-read, branch t1/wal-read from the batch head 930ac68. Built for merge: not merged, not
pushed. All commits signed by kiln (`git commit -S`).

## Head
- b815782, tree 97abfa719579d9e16c83bd8431df61e30784770a.
- Commits:
  - e66f6ab: the decision and the change, with the red run recorded.
  - 737ec75: packing allocates less.
  - 443b174: pages new to a chain go straight into their frames, and diff compares cache lines.
  - b815782: decision 39's evidence.
- Code: crates/engine/storage/packed.rs (`MIXED_FRAMES_FORMAT` 0x414d_5007, `images_as_frames`, `one_slot_records`,
  `OneSlot`, `decoded_budget`, `Record::index`, a streaming `diff` with `SAME_BLOCK`, `put_run`) and
  crates/engine/storage/wal.rs (`Logging`, `chains_from`/`extend`/`delta`, `lay_out`, `Held` and a `RecordRead` whose
  chain may begin at a page frame, `cache_records` publishing a batch's page frames before its records).
- Tests: crates/engine/tests/packed_log.rs.
- Docs: docs/design/store.md: decision 39 is new (constants holds 38); decision 37 gets one sentence pointing at 39.

## What was measured (step 1)
adas-mac-studio.local-2, 2026-11-12, at 930ac68 with counters (measurement-only patches, never committed; they are
in ~/.cache/claude-scratch/probe-carrier at 9915082: wal-read-measure.patch, -whatif.patch, -whatif2.patch). The
suite's insert_batch_100 and delete_range_insert, 200,000 items, each statement its own call:
- The bytes a miss reads. insert_batch_100 at 32 clients had 94,517 misses on records. Each needed 4,176 bytes of
  chain entries (1.19 entries, deepest 9) and read 171 KB. In all, 16.19 GB in 89,304 reads of whole records, 4.78 s
  of the 6 s run, 50.6 µs a miss. delete_range_insert at 32: 86,769 misses read 11.87 GB for 357 MB of entries.
- The records' sizes. 42 slots a record on average. 95% compressed (any delta made the whole record LZ4'd). 75% of
  a bulk commit's pages were images. 72% of all pages had no version to chain from: they were read back past the
  writer's cache.
- How often a read found its record decoded. 20% of chain entries at 32 clients, 40% at 1 client. Nearly every record
  kept was dropped again within the run (99,954 of 100,074).
- Packing. LZ4 was 689 of prepare_records' 857 ms, over 589 MB of raw entries.
- Misses by kind. 86-87% of misses on records were of pages new to a chain, 13-14% of pages in chains.
- The batched leg's layouts, simulated on the same commits (bound: 14,218 slots):
  - decision 37: 10,830 slots;
  - all images as page frames: 16,290 (over the bound);
  - new pages as frames, chains' entries grouped: 12,853;
  - one-slot records packed greedily in page order: 14,958 (over).
  - With t1/constants' rule applied: 9,541 / 14,326 / 11,593 / 13,330.

## The decision (store.md 39)
A mixed generation begun now is 0x414d_5007 (what Db::open begins).
- A page the log can't chain from (`Logging::New`) is a page frame, transformed straight into it as page-a-frame does.
- The entries of chains (deltas, and the images that end a chain) go in records of one slot each: first-fit
  decreasing, each record LZ4'd where smaller.
  - An image LZ4 can't fit in a slot becomes a page frame.
  - A delta too large for a slot gets a record of its own.
- A chain may begin at a page frame. A read past the cache reads one frame for each entry of its chain.
- The decoded records' budget is the page cache's default: 2,000 pages times the page size, the budget Amphora's
  readers keep. It was a picked 4 MiB.
- Generations begun 0x414d_5002 or 0x414d_5003 keep their layout. There, a chain that reaches a page frame is a fault.
- Tuning constant: `SAME_BLOCK` = 64 bytes, cited in its comment: x86-64's cache line, half of the M3 Ultra's 128
  (`sysctl hw.cachelinesize`).

## Tests (adas-mac-studio.local-2, debug unless named, 2026-11-12)
- Red at 930ac68 with this packed_log.rs:
  - `a_page_new_to_the_log_is_read_in_one_frame`: "looking up row 74 read 263680 bytes of the log in 1 reads,
    263680 at once";
  - `a_pages_chain_is_read_one_frame_an_entry`: "reading the table read 1429640 bytes of the log in 9 reads, 263680
    at once".
- Green at 443b174: storage:: 537 (packed 18, three new), packed_log 15, shared_pages 9, spilled_log 2, reader_cache 6.
- Release: amphora amplification 2 (the batched leg's log 3.9 bytes a byte of rows; bound 4; 3.0 at 930ac68),
  store 14, crash 5 (3 ignored: the heavy seeds), lib db::/store:: 7.
- packed_log also gains `a_mixed_generation_begun_before_keeps_its_records`: a log restamped 0x414d5003 keeps its
  records, and the next generation is 0x414d5007. The existing mixed tests take the new layout's expectations.
- cargo fmt --check clean. clippy -p amphora-engine -p amphora --all-targets --locked -D warnings clean, natively on
  macOS.
- None of the engine ratchet's counted lints on the lines changed (clippy with those lints as warnings, matched
  against the diff's lines, on macOS). The ratchet itself is x86_64 Linux only: not run.
- Not run (yours): the seven gates, the crash seeds, the record bench.

## Quick check (not a record)
adas-mac-studio.local-2 (M3 Ultra), 443b174 against 13e385d, interleaved, best of 3, loads 1.6 to 7.9, the suite at
200,000 items, 5 s after 1 s of warm-up. Writes a second, 13e385d / 443b174:

| statement | 1 client | 32 clients |
|---|---|---|
| insert_batch_100 | 83.6 / 92.0 | 446.0 / 411.6 (slower in all 3 rounds) |
| delete_range_insert | 114.4 / 124.2 | 2,288.2 / 2,191.6 |

At 930ac68 on the same host the same four ran 65.2, 76.6, 113.2 and 822.4.

The writer, from the store's counters and a profile:
- Per commit: 3.8-4.2 ms against base's 3.5-3.7.
- Samples a commit: 3.05 against 2.54.
- Reads past the cache: 1.94 against 1.81. Page frames take 1.63 of that; chains' records 0.23.
- The commit: 0.90 against 0.59. Of that, diff is 0.13 and packing 0.07.

What is left is the deltas' own cost: making them and rebuilding pages from them. It is no longer the reads of
records.

## The record run (yours: wal-redo-carry's rerun-s2.sh on adas-mac-studio.local, runner cec37f60)
rerun-s2.sh runs its "step2" tree (4acb85d) against base. To run b815782 in its place, on cec37f60, in the synced copy
of ~/.cache/claude-scratch/wal-redo-carry, with an amphora checkout holding b815782 at $REPO:
```
W=$HOME/.cache/wal-redo-bench-l2; T=$W/s39
rm -rf -- "${T:?}"; mkdir -p "$T"
git -C "$REPO" archive b815782 | tar -x -C "$T"
cp -R harness/zz-audit "$T/crates/" && mkdir -p "$T/walrepro" && cp harness/kiln-unify-logs.txt "$T/walrepro/"
echo 97abfa719579d9e16c83bd8431df61e30784770a > "$T/.tree"
git -C "$REPO" ls-tree -r b815782 | awk '{print $3, $4}' > trees/s39.ls
sed 's/step2/s39/g' rerun-s2.sh > rerun-s39.sh
bash ./rerun-s39.sh
```
- b815782's bench/Cargo.toml already has [patch.crates-io], so nothing is appended.
- The script builds s39's release amphora-bench and zz-audit and checks both trees. It takes the heavy lock and runs
  3 interleaved rounds of the 8 write statements and the 8 MiB leg, then the profiled bulk statements.
- The batched leg's disk bytes are bench-l2.sh's `zz-audit batched`.

## Merging with t1/constants (a56f23c, 18c48cd, aef3637; none undone here)
- Format values.
  - Mine is 0x414d_5007. Constants' tests take 0x414d_5006 as "a value no binary writes", which is why I didn't
    use it.
  - `LogFormat::of` should hold 5003, 5005 and 5007 as Mixed, and `file_format(Mixed)` should be 5007.
  - `most_deltas(5007)` falls to constants' default arm (MOST_DELTAS). `images_as_frames` is true only for 5007.
  - A 5005 generation (constants-only binaries) keeps the old records layout with constants' chain rule.
- `delta()`.
  - Constants returns (depth, chain bytes, runs) and checks `against.deepest`. Mine splits it into
    `chains_from` + `extend` and returns `Logging`.
  - Resolve as `Logging::Delta(depth, chain, runs)`, `extend` checking `deepest` and `left = most - chain`.
  - In `lay_out`, carry the chain bytes into `depths` as constants' prepare_records does. `depths` becomes
    Vec<(u8, u32)>, with page frames (0, 0).
- `DECODED_BUDGET` is gone. Constants edited its comment.
- packed_log.rs: both branches changed MIXED_FORMAT, the helpers and the mixed tests.
- On my branch the batched leg is 3.9 bytes a byte of rows. Constants' rule should leave more room, simulated at
  about 3.3-3.5.

## Outside findings
- `cargo clippy -p amphora` warns twice in vendor/hyper-raft/crates/hyper-rt/src/sync/mod.rs (:1033, :1136):
  `fetch_update` is deprecated as `try_update`. I didn't check whether it is the same at 930ac68.
- Heavy lock: taken and freed by my runs on local-2 (owner "amphora-wal-read t1/wal-read ..."). None left.

## For Ada's word
insert_batch_100 at 32 clients is still 8% below page-a-frame at best (446.0 against 411.6), slower in every round of
the quick check.

What is left is deltas' CPU on the writer: decision 17's diff at commit, and pages rebuilt from chains on misses.

If the record run confirms it, the choice is hers:
- (a) Recommended: page-a-frame as the default before main moves (her fallback), keeping this read path for an
  opt-in mixed log.
- (b) Accept the trade: the bulk statements at 32 clients 8% slower, for the batched leg's 3.9 bytes of log a byte
  of rows against 16.6.
