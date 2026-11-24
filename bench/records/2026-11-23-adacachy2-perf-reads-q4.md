# Lane Q4: decision 40's frame lookup, its four ways measured (adacachy2, 2026-11-23)

Deliverable status, losses first.

- Not done: the winner is not the code yet. Deleting the losing ways and the `AMPHORA_LOG_READ_INDEX`
  switch, writing the evidence into decision 40, and the checks (clippy, the touched tests,
  check-production) were not started inside the hour. Nothing under `crates/` changed. The
  coordinator's restart at 01:45Z stopped the lane for about 5 minutes.
- Not separated: `filtered`, `published` and `optimistic`. The suite's settle truncates the log, so the
  log holds a few frames through every read run. The page bitmap that all three put in front of their
  lookup answers nearly every read, and the profiles and throughputs below tie the three. They differ
  only where the log holds the pages read while commits move the snapshot on. The bench's K0 mixed
  phase is that scenario, but it failed at open on main 57fd89cd in every way, before any timing:
  `Error: this engine keeps no holding, so it can't be a group's` (r1995, `q4_mixed.sh`). No numbers.
- Shown: `locked` loses. On main's binary (57fd89cd), at 32 and 128 clients, find_frame's lock took
  3.4-7.2% of the CPU samples under `locked` and 0.0% in every cell of the other three ways. The
  lookup's whole share fell from 6.0-10.3% to 0.5-1.3%, and all spinning from 6.2-8.8% to 1.2-4.0%.
  At 1 client no way loses: medians 0.96x-1.11x of `locked`, every range overlapping `locked`'s.
- Answers unchanged: 0 errors in every run, and each way's 384 answers held to SQLite's on the same rows
  (`bench/suite_answers.py`): 0 differ, in all four ways.

## What was measured

One binary, `amphora-bench` at main 57fd89cd (built r1970, the bench's own release profile as Q3 built
it; main had moved since 14fbf220 only in crates/pg-parser, which amphora and amphora-engine don't
depend on). The way is `AMPHORA_LOG_READ_INDEX`, read once per process, so every way ran the same code
and the same calibration (RULES 5e). One fresh 200,000-item load (r1982), reopened with `--reuse` by
every run. point_pk, point_secondary and point_text at 1, 32 and 128 clients, 1 s after 0.5 s of
warm-up. adacachy2, kiln needs cpus=8,mem=24g (refused at cpus=16: 8 of 32 cores idle). Mac Studio 2
(load 1.4, 30 of 32 idle) did not answer twice, and it needs a full bench build of its own, so both
kinds of evidence come from adacachy2, the ways alternated.

### Profiles (AMPHORA_PROFILE, 997 Hz), two rounds: the ways in order, then reversed

01:33:38Z to 01:35:38Z, 1-minute load 20.0-26.4, CPU pressure (some, avg10) 25.5-37.8%. Shares of all
CPU samples, round 1 / round 2 (`q4_shares.py`). `lookup` is every sample under a `find_frame` frame
(the lookup in whichever way), `lookup spin` the samples ending in `_mm_pause` under it (its lock).

find_frame's lock (lookup spin):

| statement | clients | locked | filtered | published | optimistic |
|---|---|---|---|---|---|
| point_pk | 1 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| point_pk | 32 | 4.6 / 3.4 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| point_pk | 128 | 5.4 / 4.5 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| point_secondary | 1 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| point_secondary | 32 | 5.5 / 4.2 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| point_secondary | 128 | 7.2 / 5.4 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| point_text | 1 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| point_text | 32 | 4.5 / 4.8 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| point_text | 128 | 6.1 / 5.9 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |

The lookup, inclusive:

| statement | clients | locked | filtered | published | optimistic |
|---|---|---|---|---|---|
| point_pk | 1 | 0.6 / 0.4 | 1.1 / 0.9 | 0.9 / 0.9 | 0.3 / 0.8 |
| point_pk | 32 | 7.3 / 6.1 | 1.3 / 0.9 | 1.0 / 1.0 | 0.9 / 0.8 |
| point_pk | 128 | 9.1 / 6.0 | 0.7 / 0.9 | 0.9 / 1.2 | 0.6 / 0.8 |
| point_secondary | 1 | 0.8 / 0.6 | 1.0 / 1.3 | 0.9 / 0.8 | 0.7 / 0.2 |
| point_secondary | 32 | 8.0 / 6.2 | 0.8 / 0.7 | 0.7 / 0.5 | 1.1 / 1.2 |
| point_secondary | 128 | 10.3 / 6.9 | 0.8 / 0.7 | 1.0 / 0.6 | 0.9 / 0.6 |
| point_text | 1 | 0.3 / 0.8 | 0.6 / 1.2 | 0.9 / 0.8 | 0.4 / 1.3 |
| point_text | 32 | 7.1 / 7.7 | 0.8 / 0.9 | 0.9 / 1.2 | 0.7 / 1.1 |
| point_text | 128 | 9.3 / 8.1 | 1.2 / 1.0 | 0.6 / 0.6 | 0.5 / 1.2 |

All spinning (any lock):

| statement | clients | locked | filtered | published | optimistic |
|---|---|---|---|---|---|
| point_pk | 32 | 7.6 / 7.2 | 3.9 / 3.8 | 2.8 / 2.0 | 4.0 / 3.9 |
| point_pk | 128 | 7.6 / 7.0 | 2.6 / 2.6 | 2.4 / 2.5 | 2.3 / 3.2 |
| point_secondary | 32 | 7.9 / 6.3 | 2.0 / 2.3 | 1.9 / 1.9 | 1.6 / 2.1 |
| point_secondary | 128 | 8.8 / 6.2 | 2.1 / 1.6 | 1.5 / 1.4 | 1.7 / 1.8 |
| point_text | 32 | 7.0 / 7.8 | 2.2 / 1.5 | 1.6 / 1.8 | 2.4 / 1.4 |
| point_text | 128 | 7.5 / 7.6 | 1.5 / 1.6 | 2.0 / 1.2 | 1.4 / 2.4 |

No spin at 1 client in any way. Samples a cell: 854-1,061 at 1 client, 2,072-2,520 at 32 and 128.
`wal_header` frames took 0.0% in every cell, so the header's lock-free copy, which comes with the three
non-`locked` ways, made no difference these profiles can see.

### Statements a second, five rounds, the ways' order rotated each round

01:35:39Z to 01:40:12Z. Median of 5 (range), `q4_table.py`. Errors: 0 in every run.

| statement | clients | locked | filtered | published | optimistic | filtered / locked | published / locked | optimistic / locked |
|---|---|---|---|---|---|---|---|---|
| point_pk | 1 | 273,135 (239,230-296,309) | 261,171 (249,250-281,558) | 283,513 (233,885-301,611) | 275,678 (244,241-280,431) | 0.96x | 1.04x | 1.01x |
| point_pk | 32 | 853,116 (699,696-890,245) | 858,541 (826,219-882,149) | 860,843 (640,618-936,594) | 815,980 (765,235-951,743) | 1.01x | 1.01x | 0.96x |
| point_pk | 128 | 711,016 (696,286-800,623) | 731,191 (692,911-794,062) | 776,005 (527,524-814,600) | 767,275 (719,015-833,434) | 1.03x | 1.09x | 1.08x |
| point_secondary | 1 | 200,872 (188,055-231,011) | 201,021 (165,409-223,649) | 222,148 (159,925-231,743) | 213,564 (166,508-220,436) | 1.00x | 1.11x | 1.06x |
| point_secondary | 32 | 715,610 (652,119-804,776) | 729,738 (499,259-808,013) | 751,470 (527,157-818,389) | 752,594 (659,771-821,390) | 1.02x | 1.05x | 1.05x |
| point_secondary | 128 | 533,455 (443,590-572,941) | 572,268 (385,779-621,879) | 603,135 (382,196-663,275) | 566,253 (535,606-600,261) | 1.07x | 1.13x | 1.06x |
| point_text | 1 | 186,267 (170,252-215,095) | 181,187 (129,443-203,375) | 200,430 (174,524-209,869) | 188,781 (167,986-208,763) | 0.97x | 1.08x | 1.01x |
| point_text | 32 | 665,246 (640,949-787,922) | 709,862 (472,099-801,986) | 749,943 (654,109-786,149) | 729,581 (539,687-821,335) | 1.07x | 1.13x | 1.10x |
| point_text | 128 | 522,968 (467,488-597,662) | 563,677 (410,020-585,808) | 572,612 (566,191-604,332) | 566,151 (456,197-597,900) | 1.08x | 1.09x | 1.08x |

Every cell's ranges overlap, so no throughput difference is measurable on this host. The run's load
climbed as other work started: 1-minute load before each run, in run order, locked-1 22.8, filtered-1
21.8, published-1 27.1, optimistic-1 44.7, filtered-2 54.1, published-2 46.6, optimistic-2 47.4,
locked-2 59.0, published-3 62.5, optimistic-3 58.3, locked-3 65.9, filtered-3 77.1, optimistic-4 71.0,
locked-4 68.4, filtered-4 71.5, published-4 87.4, locked-5 74.9, filtered-5 70.0, published-5 77.6,
optimistic-5 73.9; end 62.8. CPU pressure (some, avg10) 36.1-41.6%, I/O 0.2-5.1%.

Why the throughputs can't move more: at 128 clients the lookup under `locked` is 6.0-10.3% of the CPU,
so taking it away makes a statement at most 1/(1 - 0.103) = 1.11x faster. The lock-free medians at 128
clients, 1.03x-1.13x, sit at that bound, inside this host's noise. What limits point reads at 128 clients
past the lookup is Q3's lead, the reader pool's futex wait, which a later lane's A/B owns.

## For the successor: what is left of the deliverable

1. Choose among `filtered`, `published` and `optimistic`. The suite can't (above). What separates them
   is the log holding the pages read:
   - perf-B2's log-full micro-bench (`storage::wal::tests::g3_2_log_lookup_bench`, on a02e4b20,
     bench/records/2026-11-23-adacachy2-perf-pagepath.md): `filtered` = `locked` there (1.77-1.90 M
     lookups/s at 32 threads); `published` 4.88-5.19 M and `optimistic` 4.28-4.75 M; both 1.4-1.7x
     slower than the lock at one reader.
   - Unmeasured: `published` copies the whole index (O(pages in the log)) under the index's lock each
     time a reader's snapshot passes its copy, so every commit a reader sees costs one copy, while
     `optimistic` updates one slot an append. A read beside writes is the case to measure. K0's mixed
     phase would do it once `--target amphora` opens again (it fails with "this engine keeps no
     holding, so it can't be a group's").
2. Then delete the other ways and the switch: `LogReadIndex`, `AMPHORA_LOG_READ_INDEX`,
   `set_process_default`, `ReadIndex::set_mode`. Also move the tests that loop over the ways in
   storage/wal.rs and crates/engine/tests/log_read_index.rs onto the kept way.
   `in_process_frame_lookups_compare_few_frames_when_a_page_has_many_frames` calls `find_frame`
   under `Locked`; it would call `find_frame_locked`. Update STATUS.md's paragraph, which describes
   the switch.
3. Write this record's numbers into decision 40, then clippy on amphora-engine, the touched tests
   (`storage::wal`, `--test log_read_index`) and scripts/check-production.sh before the merge.

## Commands

All on adacachy2 (runner 00aad8daff2fa3c9daa65d53c425d450, sync false), scripts in
`2026-11-23-adacachy2-perf-reads-q4/`, the lane's folder `~/.cache/analytics-q4`:
- r1970: `JOBS=8 q4_build.sh origin/main` (cpus=8,mem=24g): main 57fd89cd's bench, 78 s.
- r1982: `BIN=…/amphora-bench-57fd89cd… TOOLS=… q4_ways.sh` (cpus=8,mem=24g, after a refusal at 16):
  the load, the profiles with `q4_shares.py`, the timed rounds with `q4_table.py`, the answers.
- r1995: `q4_mixed.sh` (cpus=8,mem=24g): K0's mixed phase per way; every run failed at open (above).
