# S9: perf/hillclimb (b5dd793c) against its base (8fc85651), main, SQLite and Turso, on Mac Studio 2 (2026-11-24)

Ada, 11-24: "Examine our work on perf/hillclimb and benchmark on a studio or other free runner." Lane S9. Reads and writes of the suite's 17 statements at 1, 32 and 128 clients (lane S7's record), in process, one client code path, on the Mac Studio. What the numbers are and aren't is under "Durability" and "What limits the rows".

## Verdict

- **B against A:** 29 of 51 cells ahead (B's every round above A's every round), 3 behind, 19 no measured difference; A and B have 5 and 5 rounds, SQLite 3, Turso 3, main 2, Bd 1, Sf and Tf 1.
- **Read gains.** At 32 and 128 clients B is 7.57 to 28.66x A in the reads (table_count excepted); at 1 client 1.63 to 3.35x (writes 1 client 0.95 to 1.02x, 32 and 128 0.90 to 1.75x).
- **Where the gain is.** Neither the defaults nor the code alone. Bd, B's code with A's four defaults put back, is 0.94 to 1.11x A in the reads at 32 clients (1 round), and A's code with its own shared pages and Slots switched on is 1.29x A (point_pk at 32: 571,463 against 441,758 ops/s, one pass); B with its defaults is 13.5x A. The ablation (one pass a cell, `ablate.txt`): point_pk at 32 on B is 5.93M ops/s by default, 0.70M with shared pages off, 0.59M with the Pool checkout, 6.62M with the Today page path, 6.03M with 2,000 reader pages, 6.04M with a 1,024-page writer. So the 32-client gain needs shared pages and Slots together, on B's code; the Plain page path and the writer cache don't show in it. At 128 clients Bd is 0.24 to 1.01x A (its Pool checkout is slower than A's there). At 1 client Bd is 0.98 to 2.62x A: that much is the code, the rest the defaults.
- **Regression:** table_count (COUNT(*)) is 0.35x at 1, 0.28x at 32, 0.21x at 128 clients of A, the only cells where every B round is below every A round, and 0.16 to 0.29x of Turso. The ablation names the default: with AMPHORA_READER_PAGES=2000 (2,000 private pages a reader, shared pages still on) B's table_count is 63,074 at 1 client (21,338 by default, A 61,642) and 911,773 at 32 (80,722 by default, A 265,119); with shared pages off 61,656 and 437,833. The reader cache sized from the L1 data cache (a floor of 200 pages against A's 2,000; commit aee3dae8) is what costs it: one switch, one pass each. With that default at 2,000, table_count at 32 clients would be 3.4x A and 2.3x Turso. (A with its own shared pages and Slots on answered 56 table_counts a second at 1 client, one pass: a fault of A's opt-in combination, not of B.)
- **Against the best competitor (same durability):** B reaches 10x it in 30 of 51 cells (A in 14), and is behind it (every round) in 5; at 1 client the read ratios are 0.29 to 4.67. The 10x cells are the 32- and 128-client rows, where SQLite's and Turso's own CPU time is mostly the kernel's on macOS (below), which Linux doesn't show (lane S7): not a Linux number.
- **Memory.** A whole pass, peak resident set (median MiB): A 2,149, B 416, Bd 641, C 2,172, S 400, T 1,245, Sf 90, Tf 192. B is below A over a pass and at 128, 512 and 1,024 clients (278, 321 and 350 MiB against 1,543, 1,767 and 3,054 for A), and ABOVE it at 1 client: 186 MiB against 88 (`many.jsonl`), a finding. At 512 and 1,024 clients A nearly stops: point_pk 20,769 and 127 ops/s against 2,105,561 and 1,575,234 for B (one pass).
- **Correctness:** 0 of the read answers differ from SQLite: 1,026 answers a pass for A, B, Bd, C and Turso in every round, 0 errors and 0 retries in every cell, no failed pass (`answers.txt`, TABLES.md 5). Round 4 and 5 compare with round 1's SQLite answers (no SQLite pass in them); the answers are functions of the row ids and the clients, the same in every pass. Write statements' final states were not compared.

## Losses first

### B slower than A: 3 of 51 cells have B's every round below A's every round (A and B: 5 and 5 rounds)

- table_count at 128 clients: 0.21x [0.21, 0.22]. A 344,841 (343,446-347,571), B 73,125 (72,447-75,560) ops/s, Bd (B with A's defaults, 1 rounds) 81,258 (81,258-81,258). p50 0.068 (0.066-0.082) -> 2.40 (2.34-2.40) ms, p99 5.89 (5.63-6.02) -> 3.23 (3.20-3.30) ms.
- table_count at 32 clients: 0.28x [0.27, 0.30]. A 283,579 (266,921-298,514), B 80,428 (80,004-83,819) ops/s, Bd (B with A's defaults, 1 rounds) 265,756 (265,756-265,756). p50 0.089 (0.081-0.099) -> 0.360 (0.348-0.364) ms, p99 0.336 (0.336-0.336) -> 0.752 (0.688-0.776) ms.
- table_count at 1 clients: 0.35x [0.34, 0.37]. A 61,248 (59,956-62,450), B 21,631 (21,210-21,948) ops/s, Bd (B with A's defaults, 1 rounds) 59,961 (59,961-59,961). p50 0.016 (0.015-0.016) -> 0.045 (0.045-0.046) ms, p99 0.021 (0.020-0.022) -> 0.056 (0.053-0.058) ms.

### B behind the best competitor (reads: SQLite and Turso, 3 and 3 rounds; writes: SQLite and Turso with F_FULLFSYNC, 1 round each, so Amphora's own durability)

- table_count at 128 clients: B is 0.16x [0.15, 0.17] of T (62.3x short of 10x); B 73,125 (72,447-75,560), T 455,553 (442,958-487,194) ops/s; verdict behind.
- table_count at 32 clients: B is 0.21x [0.20, 0.22] of T (48.3x short of 10x); B 80,428 (80,004-83,819), T 388,339 (373,640-401,925) ops/s; verdict behind.
- table_count at 1 clients: B is 0.29x [0.28, 0.30] of T (34.1x short of 10x); B 21,631 (21,210-21,948), T 73,839 (73,224-77,020) ops/s; verdict behind.
- update_indexed at 1 clients: B is 0.88x [0.84, 0.94] of Tf (11.3x short of 10x); B 200 (191-213), Tf 227 (227-227) ops/s; verdict behind.
- aggregates_range at 1 clients: B is 0.90x [0.88, 0.98] of S (11.1x short of 10x); B 6,643 (6,510-6,752), S 7,368 (6,893-7,412) ops/s; verdict behind.
- update_plain at 1 clients: B is 0.91x [0.88, 1.04] of Tf (11.0x short of 10x); B 195 (188-222), Tf 214 (214-214) ops/s; verdict no measured difference.
- filter_no_index at 1 clients: B is 0.92x [0.89, 1.02] of S (10.8x short of 10x); B 48.00 (48.00-49.00), S 52.00 (48.00-54.00) ops/s; verdict no measured difference.
- delete_insert at 1 clients: B is 0.93x [0.93, 1.05] of Tf (10.7x short of 10x); B 175 (174-198), Tf 188 (188-188) ops/s; verdict no measured difference.
- json_field at 1 clients: B is 0.93x [0.92, 1.00] of T (10.7x short of 10x); B 432,692 (427,894-449,255), T 463,684 (447,584-463,687) ops/s; verdict no measured difference.
- point_text at 1 clients: B is 0.94x [0.92, 1.09] of T (10.6x short of 10x); B 374,024 (365,937-397,576), T 396,323 (351,730-396,776) ops/s; verdict no measured difference.
- upsert at 1 clients: B is 0.94x [0.92, 1.01] of Tf (10.6x short of 10x); B 189 (183-203), Tf 200 (200-200) ops/s; verdict no measured difference.
- txn_read_write at 1 clients: B is 0.96x [0.95, 1.03] of Sf (10.4x short of 10x); B 178 (176-190), Sf 185 (185-185) ops/s; verdict no measured difference.
- point_secondary at 1 clients: B is 0.97x [0.94, 1.08] of T (10.3x short of 10x); B 414,969 (405,291-438,070), T 425,636 (397,368-437,802) ops/s; verdict no measured difference.

### Cells where B reaches 10x the best competitor of the same durability

30 of 51: table_group_by@32 none answered, table_group_by@128 none answered, filter_no_index@128 none answered, point_secondary@128 115.7x, update_plain@128 83.5x, point_text@128 63.5x, upsert@128 61.8x, join_three@32 61.2x, update_indexed@128 57.8x, insert_one@128 57.0x, delete_insert@128 55.6x, txn_read_write@128 48.4x, join_three@128 44.6x, blob_read@32 42.4x, json_field@32 41.0x, json_field@128 38.7x, point_pk@32 36.3x, blob_read@128 33.2x, point_pk@128 31.9x, point_text@32 31.9x, point_secondary@32 31.1x, filter_no_index@32 29.6x, aggregates_range@128 25.7x, update_plain@32 24.1x, upsert@32 23.5x, aggregates_range@32 23.4x, insert_one@32 22.6x, update_indexed@32 21.8x, delete_insert@32 19.0x, txn_read_write@32 16.7x.

### Memory, a whole pass (ru_maxrss, MiB; median (range))

- A: max RSS 2,149 (2,139-2,249), harness heap peak 1,421 (1,420-1,426), CPU user/sys s 288 (285-288) / 1,070 (1,057-1,072).
- B: max RSS 416 (398-420), harness heap peak 285 (285-288), CPU user/sys s 899 (883-903) / 19.30 (17.10-19.90).
- Bd: max RSS 641 (641-641), harness heap peak 352 (352-352), CPU user/sys s 194 (194-194) / 1,209 (1,209-1,209).
- C: max RSS 2,172 (2,124-2,220), harness heap peak 1,421 (1,420-1,422), CPU user/sys s 290 (289-290) / 1,076 (1,066-1,086).
- S: max RSS 400 (400-402), harness heap peak 19.50 (19.50-19.60), CPU user/sys s 74.80 (74.40-77.50) / 4,073 (3,932-4,084).
- T: max RSS 1,245 (1,236-1,246), harness heap peak 1,090 (1,090-1,090), CPU user/sys s 1,645 (1,533-1,763) / 1,771 (1,676-1,888).
- Sf: max RSS 89.80 (89.80-89.80), harness heap peak 16.80 (16.80-16.80), CPU user/sys s 2.20 (2.20-2.20) / 17.60 (17.60-17.60).
- Tf: max RSS 192 (192-192), harness heap peak 133 (133-133), CPU user/sys s 1,128 (1,128-1,128) / 5.40 (5.40-5.40).


## What was compared

| arm | what | commit | binary |
|---|---|---|---|
| A | Amphora, the base the hillclimb started from | 8fc856513284ff5c8dfcb107515c68ece6096c0b | sha256 da05bc47932645bd54531a1e042b2fbe4d0058f0168b3d4d3d6f1c5816a0aef4 |
| B | Amphora, perf/hillclimb | b5dd793c570c4df8e69b159bec2eb7acb0fa7ead | sha256 f47ee32864fae1609078f38e55ead58f3253e9c581a91c74a26c8fd9160cb32e |
| Bd | B's binary with A's defaults put back by its switches | b5dd793c | same binary as B |
| C | Amphora, main's tip when the lane started (aef6fc8a), for context | aef6fc8a4363c1a60a0fd4471b96190be396419d | sha256 in the build log (`build-c2.log`) |
| S | SQLite 3.53.2 (bundled by rusqlite 0.40.2), WAL, `synchronous = FULL`, a connection a client | from A's binary | |
| T | Turso (`turso_core` at 3e09a82), as the suite opens it | from A's binary | |
| Sf, Tf | S and T with every `fsync()` made `fcntl(F_FULLFSYNC)` by `libs9sync.dylib` (`s9sync.c`); the seven write statements only | same binary as A | |

B is 17 commits over A (48 files, +7,208/-1,829). Main (C) is 352 commits past A. bench/src is the same code in A, B and C except `oracle/` (C) and the two lines of `amphora_sharing()` (B: the default is the process's shared pool, A and C: off).

**Defaults.** A runs at its own defaults: no shared pages, the `Today` page path, the `Pool` reader checkout, a writer cache of 1,024 pages and reader caches of 2,000. B runs at its own: the process's shared pages, the `Plain` page path, `Slots`, a writer cache of 16,384 pages (the 64 MiB cap; this machine has 96 GB) and reader caches sized from the L1 data cache (128 KiB on these P-cores). That is what a user of each gets. Bd is B with `AMPHORA_SHARED_PAGES=0 AMPHORA_READ_CHECKOUT=pool AMPHORA_PAGE_PATH=today AMPHORA_WRITER_PAGES=1024`, so A's four defaults on B's code. Everything else the branch changed (recycling, hash tables, row headers, lock-free read transactions) is still on in Bd.

## How it ran

- Machine: Mac Studio 2 (kiln runner d1323898b255a3ed435eb83fa0f7b0cf): Apple M3 Ultra, 24 performance and 8 efficiency cores, 96 GB, macOS 26.4 (25E246), on AC power, APFS on the internal SSD. kiln runs at nice 20 there; the single-thread check in `rounds.log`'s start used 3.98 GHz, a performance core.
- Build: `cargo build --release --offline --manifest-path bench/Cargo.toml --bin amphora-bench` (rustc and cargo 1.99.1, the toolchain file's), with the workspace root's release profile on the bench workspace as lanes S4 and S7 built it: `CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true CARGO_PROFILE_RELEASE_DEBUG=line-tables-only CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=8` (8 across the three arms), one `bench/Cargo.lock` copied to all three arms (`Cargo.lock-a` and `-b` are identical; C's differs only in the path crates main added). Arm C's first build (2 of the 8 jobs) was stopped after 8 minutes to save time and started again at 8 jobs in the same target directory, with A's DuckDB objects copied in; cargo compiled DuckDB again anyway (`build-c2.log`). Same lock, profile and toolchain.
- The suite is `bench/src/suite.rs`: 17 statements (S7's `--only` list), `--items 200000 --clients 1,32,128 --seconds 1 --warmup 0.5`, one process a pass, every pass loading the data again. `rounds.py` runs the binary as `bench/run.sh` does, with `--target` and `--dir` per arm, under `ulimit -n 61440`. `run.sh` itself needs `nproc`, `lscpu` and `/proc`, which macOS lacks. Orders by round are in `rounds.log`; a Williams-type design, so no arm always goes first or follows the same one.
- Rounds: A and B have 5 (the bar); SQLite (S) and Turso (T) 3, main (C) 2, Bd, Sf and Tf 1. The lane's 75 minutes didn't hold 5 rounds of every arm: a Turso pass takes 149 s and a SQLite pass 201 s here against 82 s (B) and 95 s (A), because their whole-table statements at 128 clients overrun the window. The arms left out of a round are stated with their round counts in every table. Orders as run (Williams-type, `rounds.log`): round 1 A,B,Bd,C,T,Tf,S,Sf; 2 B,C,A,S,T; 3 S,B,A; 4 A,T,B; 5 B,A. A first start of `rounds.py` failed before any pass (kiln's runs sit at nice 20, which nothing may raise); it left no data.
- In-process: the engines run inside the benchmark's process, so the clients' threads and the engine share the cores.
- Before the rounds: `smoke.sh` (untimed) printed every arm's plan for the 17 statements and counted what each engine's commit calls (`sync-*.err`). Plans of A, B and Bd were the same for all 17. On macOS no CPU calibration is stored between processes (`$XDG_CACHE_HOME/amphora/calibration` stays empty), so each Amphora process measured its own at open; rule 5e's sharing has nothing to copy.
- Correctness: every read's answers (1,026 a pass, taken before any write of that pass) against SQLite's of the same round, value by value as text (`s9_answers.py`, `bench/suite_answers.py`'s rule); errors and retries counted per cell.

## B with one default put back at a time (`ablate.sh`; one pass each, 200,000 items, after the rounds)

| statement | clients | default | A_default | A_shared+slots | shared=0 | page_path=today | checkout=pool | reader_pages=2000 | writer_pages=1024 | all_old |
|---|---|---|---|---|---|---|---|---|---|---|
| point_pk | 1 | 509,580 | 315,551 | 437,524 | 404,922 | 520,690 | 539,040 | 547,135 | 537,916 | 358,629 |
| point_pk | 32 | 5,933,316 | 441,758 | 571,463 | 695,312 | 6,621,151 | 587,432 | 6,030,208 | 6,044,886 | 487,777 |
| table_count | 1 | 21,338 | 61,642 | 56 | 61,656 | 6,737 | 20,384 | 63,074 | 19,105 | 60,919 |
| table_count | 32 | 80,722 | 265,119 | 67,829 | 437,833 | 62,065 | 81,794 | 911,773 | 80,264 | 279,074 |

ops/s. `default` is B as it ships; `shared=0` is AMPHORA_SHARED_PAGES=0, `page_path=today`, `checkout=pool`, `reader_pages=2000` (shared pages on, 2,000 private pages a reader), `writer_pages=1024`; `all_old` is Bd.

## Profiles (samply, no root, 2,000 Hz, on the 4 s window of a run that reused a database; shares are on-CPU samples, and a share is not a cause)

### B-aggregates_range-1.txt

```
B-aggregates_range-1.json.gz: 2348 on-CPU samples; names by atos on amphora-bench-b
Top self (on-CPU):
   49.8%  _RNvNtNtCs5ymjRPkuaB8_14amphora_engine4vdbe7execute15op_column_fetch
   20.5%  _RNvMs0_NtCs5ymjRPkuaB8_14amphora_engine9statementNtB5_9Statement10step_inner
   11.1%  _RNvNtNtCs5ymjRPkuaB8_14amphora_engine4vdbe7execute11op_agg_step
    2.7%  libsystem_kernel.dylib
    2.3%  _RNvMs1_NtCsdnfh2UXVlgZ_9hashbrown3mapINtB5_7HashMapNtNtNtCs5ymjRPkuaB8_14amphora_engine7storage10page_cache12PageCacheK
    1.4%  _RNvMs3_NtNtCs5ymjRPkuaB8_14amphora_engine7storage10page_cacheNtB5_9PageCache19insert_shared_plain
    1.3%  _RNvMsg_NtNtCs5ymjRPkuaB8_14amphora_engine7storage5btreeNtB5_11BTreeCursor15get_next_record
    0.9%  _RNvMsh_NtNtCs5ymjRPkuaB8_14amphora_engine7storage5pagerNtB5_5Pager12cache_lookup
    0.8%  _RNvNtNtCs5ymjRPkuaB8_14amphora_engine4vdbe7execute19apply_real_affinity
    0.7%  _RNvNtNtCs5ymjRPkuaB8_14amphora_engine4vdbe7execute16op_real_affinity
    0.7%  _RNvMs3_NtNtCs5ymjRPkuaB8_14amphora_engine7storage10page_cacheNtB5_9PageCache15evict_one_entry
Top total (on-CPU):
   98.0%  libsystem_pthread.dylib
   98.0%  _RNvNvMs0_NtNtNtCsdnanKjkF2cC_3std3sys6thread4unixNtB7_6Thread3new12thread_start
   96.0%  _RINvNtNtCsdnanKjkF2cC_3std3sys9backtrace28___rust_begin_short_backtraceNCNCNvNtCs5vmn3g7JtKG_13amphora_bench5suite6run_
   95.9%  _RNSNvYNCINvNtNtCsdnanKjkF2cC_3std6thread9lifecycle15spawn_uncheckedNCNCNvNtCs5vmn3g7JtKG_13amphora_bench5suite6run_op0s
   95.8%  _RNvXs1_NtNtCs5vmn3g7JtKG_13amphora_bench8suite_db14amphora_targetNtB5_14AmphoraSessionNtB7_7Session3run
   95.5%  _RNvNtNtCs5vmn3g7JtKG_13amphora_bench8suite_db14amphora_target8read_all
   95.2%  _RNvMs0_NtCs5ymjRPkuaB8_14amphora_engine9statementNtB5_9Statement5__step
   95.1%  _RNCINvNtCs4BZFvWJmg0a_7amphora2db12step_betweenINtNtNtNtCs3bJEMBXHbwl_4core4iter8adapters3map3MapINtNtNtBS_5slice4iter4
   95.0%  _RNvMs0_NtCs5ymjRPkuaB8_14amphora_engine9statementNtB5_9Statement10step_inner
   49.8%  _RNvNtNtCs5ymjRPkuaB8_14amphora_engine4vdbe7execute15op_column_fetch
   11.1%  _RNvNtNtCs5ymjRPkuaB8_14amphora_engine4vdbe7execute11op_agg_step
```

### B-table_count-1.txt

```
B-table_count-1.json.gz: 2333 on-CPU samples; names by atos on amphora-bench-b
Top self (on-CPU):
   16.0%  _RNvMs3_NtNtCs5ymjRPkuaB8_14amphora_engine7storage10page_cacheNtB5_9PageCache19insert_shared_plain
    9.3%  _RNvMs3_NtNtCs5ymjRPkuaB8_14amphora_engine7storage10page_cacheNtB5_9PageCache15evict_one_entry
    8.0%  _RNvMsh_NtNtCs5ymjRPkuaB8_14amphora_engine7storage5pagerNtB5_5Pager14read_page_into
    7.6%  _RNvMs1_NtCsdnfh2UXVlgZ_9hashbrown3mapINtB5_7HashMapNtNtNtCs5ymjRPkuaB8_14amphora_engine7storage10page_cache12PageCacheK
    6.6%  _RNvXsj_NtNtCs5ymjRPkuaB8_14amphora_engine7storage5btreeNtB5_11BTreeCursorNtB5_11CursorTrait5count
    5.7%  _RNvNtNtCs5ymjRPkuaB8_14amphora_engine7storage14sqlite3_ondisk15read_btree_cell
    5.0%  _RNvMsh_NtNtCs5ymjRPkuaB8_14amphora_engine7storage5pagerNtB5_5Pager12cache_lookup
    4.1%  _RNvXsb_NtNtCs5ymjRPkuaB8_14amphora_engine7storage3walNtB5_24InProcessWalCoordinationNtB5_15WalCoordination10find_frame
    4.1%  _RNvMs_NtNtCs5ymjRPkuaB8_14amphora_engine7storage12shared_pagesNtB4_11SharedPages10take_count
    3.9%  _RNvMsq_NtNtCs5ymjRPkuaB8_14amphora_engine7storage5btreeNtB5_9PageStack5__push
    3.6%  _RNvMs0_NtNtNtCs5ymjRPkuaB8_14amphora_engine7storage5pager10page_innerNtB5_9PageInner18reset_plain_shared
Top total (on-CPU):
   98.8%  libsystem_pthread.dylib
   98.7%  _RNvNvMs0_NtNtNtCsdnanKjkF2cC_3std3sys6thread4unixNtB7_6Thread3new12thread_start
   96.9%  _RINvNtNtCsdnanKjkF2cC_3std3sys9backtrace28___rust_begin_short_backtraceNCNCNvNtCs5vmn3g7JtKG_13amphora_bench5suite6run_
   96.8%  _RNSNvYNCINvNtNtCsdnanKjkF2cC_3std6thread9lifecycle15spawn_uncheckedNCNCNvNtCs5vmn3g7JtKG_13amphora_bench5suite6run_op0s
   96.5%  _RNvXs1_NtNtCs5vmn3g7JtKG_13amphora_bench8suite_db14amphora_targetNtB5_14AmphoraSessionNtB7_7Session3run
   96.1%  _RNvNtNtCs5vmn3g7JtKG_13amphora_bench8suite_db14amphora_target8read_all
   95.1%  _RNCINvNtCs4BZFvWJmg0a_7amphora2db12step_betweenINtNtNtNtCs3bJEMBXHbwl_4core4iter8adapters3map3MapINtNtNtBS_5slice4iter4
   95.0%  _RNvMs0_NtCs5ymjRPkuaB8_14amphora_engine9statementNtB5_9Statement10step_inner
   95.0%  _RNvMs0_NtCs5ymjRPkuaB8_14amphora_engine9statementNtB5_9Statement5__step
   93.4%  _RNvNtNtCs5ymjRPkuaB8_14amphora_engine4vdbe7execute8op_count
   93.4%  _RNvXsj_NtNtCs5ymjRPkuaB8_14amphora_engine7storage5btreeNtB5_11BTreeCursorNtB5_11CursorTrait5count
```

### B-table_count-32.txt

```
B-table_count-32.json.gz: 134623 on-CPU samples; names by atos on amphora-bench-b
Top self (on-CPU):
   24.4%  _RNvMs_NtNtCs5ymjRPkuaB8_14amphora_engine7storage12shared_pagesNtB4_11SharedPages10take_count
   17.2%  _RNvMs3_NtNtCs5ymjRPkuaB8_14amphora_engine7storage10page_cacheNtB5_9PageCache19insert_shared_plain
   15.8%  _RNvMs3_NtNtCs5ymjRPkuaB8_14amphora_engine7storage10page_cacheNtB5_9PageCache15evict_one_entry
    8.8%  _RNvMsq_NtNtCs5ymjRPkuaB8_14amphora_engine7storage5btreeNtB5_9PageStack3pop
    5.9%  _RNvMs0_NtNtNtCs5ymjRPkuaB8_14amphora_engine7storage5pager10page_innerNtB5_9PageInner18reset_plain_shared
    5.2%  _RNvMsh_NtNtCs5ymjRPkuaB8_14amphora_engine7storage5pagerNtB5_5Pager14read_page_into
    4.6%  _RNvMs1_NtCsdnfh2UXVlgZ_9hashbrown3mapINtB5_7HashMapNtNtNtCs5ymjRPkuaB8_14amphora_engine7storage10page_cache12PageCacheK
    4.3%  _RNvXsj_NtNtCs5ymjRPkuaB8_14amphora_engine7storage5btreeNtB5_11BTreeCursorNtB5_11CursorTrait5count
    3.3%  _RNvNtNtCs5ymjRPkuaB8_14amphora_engine7storage14sqlite3_ondisk15read_btree_cell
    1.4%  _RNvMs8_NtCsai5bauRwpLg_11parking_lot10raw_rwlockNtB5_9RawRwLock16lock_shared_slow
    1.3%  _RNvMsh_NtNtCs5ymjRPkuaB8_14amphora_engine7storage5pagerNtB5_5Pager12cache_lookup
Top total (on-CPU):
  100.0%  libsystem_pthread.dylib
  100.0%  _RNvNvMs0_NtNtNtCsdnanKjkF2cC_3std3sys6thread4unixNtB7_6Thread3new12thread_start
   99.9%  _RINvNtNtCsdnanKjkF2cC_3std3sys9backtrace28___rust_begin_short_backtraceNCNCNvNtCs5vmn3g7JtKG_13amphora_bench5suite6run_
   99.9%  _RNSNvYNCINvNtNtCsdnanKjkF2cC_3std6thread9lifecycle15spawn_uncheckedNCNCNvNtCs5vmn3g7JtKG_13amphora_bench5suite6run_op0s
   99.8%  _RNvXs1_NtNtCs5vmn3g7JtKG_13amphora_bench8suite_db14amphora_targetNtB5_14AmphoraSessionNtB7_7Session3run
   99.6%  _RNvNtNtCs5vmn3g7JtKG_13amphora_bench8suite_db14amphora_target8read_all
   99.3%  _RNCINvNtCs4BZFvWJmg0a_7amphora2db12step_betweenINtNtNtNtCs3bJEMBXHbwl_4core4iter8adapters3map3MapINtNtNtBS_5slice4iter4
   99.3%  _RNvMs0_NtCs5ymjRPkuaB8_14amphora_engine9statementNtB5_9Statement5__step
   99.3%  _RNvMs0_NtCs5ymjRPkuaB8_14amphora_engine9statementNtB5_9Statement10step_inner
   98.5%  _RNvNtNtCs5ymjRPkuaB8_14amphora_engine4vdbe7execute8op_count
   98.5%  _RNvXsj_NtNtCs5ymjRPkuaB8_14amphora_engine7storage5btreeNtB5_11BTreeCursorNtB5_11CursorTrait5count
```

## Many connections (`many.py`: the three statements below, each on its own, at each client count; a fresh process and load per row)

| arm | clients | peak RSS MiB | harness heap peak MiB | statement: ops/s (p50 / p99 ms; errors) |
|---|---|---|---|---|
| A | 1 | 87.6 | 21.3 | point_pk 319,921 (0.003 / 0.004; 0); insert_one 201 (4.992 / 9.6; 0); txn_read_write 180 (5.184 / 9.984; 0) |
| B | 1 | 185.8 | 134.4 | point_pk 534,320 (0.001 / 0.002; 0); insert_one 195 (5.184 / 10.368; 0); txn_read_write 196 (5.12 / 10.24; 0) |
| A | 128 | 1,542.6 | 1346.0 | point_pk 435,285 (0.039 / 5.248; 0); insert_one 12,870 (9.344 / 14.464; 0); txn_read_write 10,319 (12.288 / 17.664; 0) |
| B | 128 | 278.2 | 219.3 | point_pk 3,679,070 (0.005 / 0.896; 0); insert_one 12,796 (10.112 / 13.312; 0); txn_read_write 12,136 (10.24 / 12.672; 0) |
| B | 512 | 321.4 | 237.4 | point_pk 2,105,561 (0.002 / 0.007; 0); insert_one 29,696 (15.488 / 67.584; 0); txn_read_write 18,944 (26.624 / 28.928; 0) |
| A | 512 | 1,766.9 | 1488.5 | point_pk 20,769 (5.632 / 1097.728; 0); insert_one 24,029 (20.992 / 29.696; 0); txn_read_write 8,827 (36.352 / 48.128; 0) |
| B | 1024 | 350.3 | 258.2 | point_pk 1,575,234 (0.002 / 0.007; 0); insert_one 40,098 (24.832 / 32.768; 0); txn_read_write 21,504 (46.592 / 51.2; 0) |
| A | 1024 | 3,053.6 | 2641.8 | point_pk 127 (26.88 / 1359.872; 0); insert_one 25,707 (38.912 / 62.464; 0); txn_read_write 19,345 (54.784 / 79.872; 0) |

## Profiles, in words (a share is not a cause; the ablation is the test)

- table_count at 1 client on B (0.35x of A): `op_count`/`BTreeCursor::count` is 93.4% of on-CPU time, `Pager::read_page_into` 73.4% total (8.0% self), `PageCache::insert_shared_plain` 39.7% total (16.0% self), `evict_one_entry` 9.3% self, the page cache's hash map 7.6% self, `SharedPages::take_count` 4.1% self. The index holds more pages than the reader's private cache, so each page of the scan is read from the shared pool into the private cache and an older one evicted.
- table_count at 32 clients on B (0.28x of A, 0.21x of Turso): the same path with `SharedPages::take_count` first (24.4% self, 26.1% total), `insert_shared_plain` 44.5% total (17.2% self), `evict_one_entry` 15.8% self.
- aggregates_range at 1 client on B (0.90x of SQLite, 6,643 against 7,368 ops/s): `op_column_fetch` 49.8% self, `Statement::step_inner` 20.5% self, `op_agg_step` 11.1% self.
- The ablation is the evidence for a cause: with a 2,000-page reader cache and everything else as B ships, table_count is 63,074 (1 client) and 911,773 (32).

## Durability: what the write rows compare

On macOS `fsync()` leaves the drive's own cache unflushed; `fcntl(F_FULLFSYNC)` flushes it. Amphora's commits flush with `F_FULLFSYNC` (`DataSync` in `crates/engine/io/unix.rs`); the suite's SQLite (`synchronous = FULL`, no `PRAGMA fullfsync`) and Turso commit with plain `fsync()`. Counted on `insert_one` at 1 client (0.5 s): A and B 125-131 `F_FULLFSYNC` and 5 `fsync`; SQLite 6,147 `fsync` and no `F_FULLFSYNC`; Turso 5,753 and none (`sync-*.err`). So the `S` and `T` write rows don't compare the same durability, and the claim for writes is made against `Sf` and `Tf`, the same two binaries with every `fsync()` made `F_FULLFSYNC` by `libs9sync.dylib`: 190 and 210 ops/s there at 1 client against 9,918 and 9,190 with plain `fsync` (one-off, 0.5 s). One flush takes about 5.2 ms on this SSD, so any engine commits about 190 to 230 times a second from one client, and no engine can be 10x another there. `Sf` and `Tf` ran in round 1 only; their write rows say so.

## What limits the rows

- 1-client writes: the flush (5.2 ms p50 for every engine that calls `F_FULLFSYNC`; Amphora's p50 is 5.12 to 5.25 ms).
- Reads at 32 and 128 clients: SQLite's and Turso's process time is system time (S: 4,084 s of system and 74 s of user CPU in a 201 s pass; T: 1,888 s and 1,645 s; `cpu_sys_s` in `round*.jsonl`), as A's is (1,072 s and 288 s); B's is user time (899 s user, 20 s system). That is a measured fact about CPU time, not a cause. It also means the gap to SQLite and Turso at 32 and 128 clients is macOS's: on Linux lane S7 (adacachy2) had Turso at 1,032,340 point reads a second with 32 clients, where here it has about 156,000. Don't carry these ratios to Linux.
- Latency resolution: the harness records in whole microseconds (log-linear buckets within 1/64), so a p50 under about 5 us moves in steps; read those rows by ops/s.
- A statement that takes longer than the 1 s window at 128 clients answers none in it (0 ops/s, no latency). That is a property of the window, not an error: it is marked `inf` where the other arm answered.

## Conditions

- Load, CPU idle, memory and the other processes' CPU at each round's start and end are in TABLES.md section 8, every pass's in section 7, and every 5 s in `phases.log`. Thermal state: no warning recorded at any round's start or end. Memory: 93% free, pressure level 1 (normal) throughout. Between rounds the machine was 95 to 98% idle.
- Other people's load: six samples over 200% of a core, all within round 2's SQLite pass (16:58:22Z to 16:59:03Z, another user's `rustc`, up to 8 of 32 cores for about 40 s: the end of that pass, its write statements). Round 2's SQLite write cells are noisy for that reason and are marked so here; its median is over three rounds. Round 2's Turso pass and round 3's SQLite pass had one other core busy (`rustc`, 1 to 1.8 cores). No other pass had another process over 2 cores. The bench ran at nice 20 (kiln's), the same for every arm.
- Run time of a pass (s, median): A 95, B 82, Bd 98, C 96, SQLite 201, Turso 149, Sf 58, Tf 62. The analytics statements of A, C, SQLite and Turso at 128 clients overrun the 1 s window, which is most of that.

## Not done

- Extra scenarios (b) a cold open and first query and (c) a transaction larger than the writer cache; (a) ran in part (`many.jsonl`: each statement on its own, not mixed).
- 5 rounds for every arm: SQLite and Turso 3 (their passes take 149 and 201 s here), main 2, Bd, Sf and Tf 1. The ablation, the many-connections run and the profiles are single passes.
- Linux was not run; the competitor gap at 32 and 128 clients is macOS's (see above).
- No fix was made and no switch of B's was changed.

## Files

- `TABLES.md`: every cell (ops/s, ratios and intervals, p50, p99, p99.9), the rankings, errors, memory, conditions per pass and per round. `COMPACT.md`: the medians table.
- `round1.jsonl` ... `round5.jsonl`: the raw passes (`rounds.py`), the read answers removed after being compared (their count is kept). `rounds.log`: host state at each round's start and end; `phases.log`: load and other processes' CPU every 5 s. `answers.txt`: the answer check's output.
- `scripts/`: the scripts as run (`rounds.py`, `smoke.sh`, `go.sh`, `after.sh`, `profile.sh`, `ablate.sh`, `many.py`, `s9sync.c`, `build.sh`, `build-c2.sh`, `s9_tables.py`, `s9_compact.py`, `s9_answers.py`, `samply_top.py`, `strip.py`, the README makers). `build-*.log` (arm builds), `Cargo.lock-*`, `explain-*.json` (plans), `sync-*.err` (flush counts), `ablate.jsonl` and `ablate.txt` (B with one switch put back at a time), `prof/*.txt` (profiles), `many.jsonl` (many connections).

## Run it again

On a Mac with rustup's 1.99.1, a clone of this repository with `git worktree add` at 8fc85651, b5dd793c and main's tip under `~/.cache/amphora-s9/{a,b,c}`:

```
build.sh a 8; build.sh b 8; build.sh c 8     # env in build.sh: the release profile above, one bench/Cargo.lock for all
clang -dynamiclib -O2 -o bin/libs9sync.dylib s9sync.c
smoke.sh                                      # plans, flush counts
python3 rounds.py OUT 1 5 "A,B,Bd,C,T,Tf,S,Sf/B,S,A,C/A,T,B/S,B,A/T,A,B"
python3 s9_answers.py OUT; python3 s9_tables.py OUT > TABLES.md; python3 s9_compact.py OUT > COMPACT.md
```

