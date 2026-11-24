# Lane S7 (bench), 2026-11-24 05:55Z to 2026-11-24T06:54:03Z: Amphora against Turso and SQLite, in process, main 0101aa38, adacachy2

Ada, 11-24: "Then lets do another bench vs our sqlite competitors." Goal: one measured, fair table of how far Amphora is from its SQLite-family competitors at today's main, on adacachy2. Rule 0 holds here: numbers say where Amphora is ahead or behind, not why. No cause is claimed and no profile was taken.

## Verdict, losses first

- **Counts (Amphora's throughput against the best competitor's, 17 statements x 1, 32 and 128 clients).** Counts over 51 cells: ahead 23, behind 16, overlapping ranges 12. By median ratio alone: 28 above 1x, 23 below.
- **By family and client count (geometric mean of Amphora/best).**

| family | 1 client | 32 clients | 128 clients |
|---|---|---|---|
| point reads | 0.59x over 5 ops | 0.81x over 5 ops | 1.92x over 5 ops |
| OLTP writes | 0.85x over 7 ops | 8.12x over 7 ops | 36.75x over 7 ops |
| analytics | 0.73x over 5 ops | 1.28x over 5 ops | 1.27x over 3 ops, 2 left out |

- **The ten largest gaps (Amphora furthest behind):**
  1. join_three at 1 clients: 0.25x (4.0x slower) of sqlite (542 (512-554) vs 2,159 (2,074-2,246) ops/s; p50/p99 ms 1.79 / 2.66 vs 0.448 / 0.688; behind; min samples 512 [<1000])
  2. table_count at 32 clients: 0.48x (2.1x slower) of turso (338,099 (285,697-347,421) vs 703,154 (663,269-766,716) ops/s; p50/p99 ms 0.048 / 0.744 vs 0.030 / 0.384; behind; min samples 285697)
  3. aggregates_range at 32 clients: 0.48x (2.1x slower) of turso (15,960 (10,977-17,950) vs 33,100 (25,402-35,635) ops/s; p50/p99 ms 1.87 / 4.48 vs 0.768 / 3.26; behind; min samples 10977)
  4. upsert at 1 clients: 0.49x (2.0x slower) of sqlite (120 (115-238) vs 243 (49.00-315) ops/s; p50/p99 ms 8.83 / 12.67 vs 3.04 / 12.80; overlap; min samples 49 [<1000])
  5. table_count at 128 clients: 0.50x (2.0x slower) of turso (295,153 (233,943-316,555) vs 584,889 (576,774-606,571) ops/s; p50/p99 ms 0.057 / 5.18 vs 0.030 / 3.20; behind; min samples 233943)
  6. blob_read at 1 clients: 0.53x (1.9x slower) of sqlite (295,294 (267,416-307,416) vs 562,081 (512,718-570,622) ops/s; p50/p99 ms 0.003 / 0.005 vs 0.001 / 0.003; behind; min samples 267416)
  7. json_field at 1 clients: 0.54x (1.8x slower) of sqlite (260,010 (246,809-262,478) vs 477,536 (433,158-483,932) ops/s; p50/p99 ms 0.003 / 0.006 vs 0.001 / 0.003; behind; min samples 246809)
  8. filter_no_index at 1 clients: 0.56x (1.8x slower) of sqlite (25.00 (24.00-27.00) vs 45.00 (41.00-50.00) ops/s; p50/p99 ms 38.40 / 44.54 vs 21.76 / 26.62; behind; min samples 24 [<1000])
  9. delete_insert at 1 clients: 0.57x (1.8x slower) of sqlite (118 (114-320) vs 208 (70.00-301) ops/s; p50/p99 ms 9.09 / 14.72 vs 4.29 / 9.60; overlap; min samples 70 [<1000])
  10. point_pk at 1 clients: 0.59x (1.7x slower) of turso (289,111 (266,794-304,329) vs 491,360 (477,224-504,863) ops/s; p50/p99 ms 0.003 / 0.005 vs 0.001 / 0.003; behind; min samples 266794)
- **Order check:** amphora odd/even 1.150 over 50 cells; turso odd/even 1.149 over 50 cells; sqlite odd/even 0.971 over 48 cells
- **Beside S4's table (per target, cells over the 17 statements x 3 client counts):**
  - amphora: 31 cells no measured change, 12 above S4's range, 8 below.
  - turso: 34 cells no measured change, 0 above S4's range, 17 below.
  - sqlite: 44 cells no measured change, 2 above S4's range, 5 below.
- **Largest changes outside S4's range (median of rounds; a Turso cell also changed engine wait, see below):**
  - amphora table_count at 1 clients: 2,309 (2,244-28,852) -> 33,614 (32,815-35,005) ops/s (14.56x, up)
  - amphora table_count at 32 clients: 81,227 (3,879-139,094) -> 338,099 (285,697-347,421) ops/s (4.16x, up)
  - amphora filter_no_index at 32 clients: 36.00 (32.00-86.00) -> 95.00 (64.00-95.00) ops/s (2.64x, up)
  - amphora table_count at 128 clients: 130,543 (60,777-197,943) -> 295,153 (233,943-316,555) ops/s (2.26x, up)
  - amphora update_plain at 32 clients: 4,256 (3,680-5,921) -> 2,311 (1,760-5,152) ops/s (0.54x, down)
  - turso insert_batch_100 at 128 clients: 124 (60.00-130) -> 26.00 (20.00-31.00) ops/s (0.21x, down)
  - turso update_plain at 128 clients: 292 (137-297) -> 123 (106-238) ops/s (0.42x, down)
  - turso update_plain at 1 clients: 308 (143-317) -> 134 (111-285) ops/s (0.44x, down)
  - turso insert_batch_100 at 32 clients: 148 (76.00-165) -> 66.00 (39.00-71.00) ops/s (0.45x, down)
  - turso txn_read_write at 128 clients: 138 (123-143) -> 78.00 (0.000-206) ops/s (0.57x, down)
  - sqlite filter_no_index at 32 clients: 26.00 (10.00-32.00) -> 8.00 (6.00-32.00) ops/s (0.31x, down)
  - sqlite delete_insert at 32 clients: 147 (106-229) -> 277 (145-306) ops/s (1.88x, up)
  - sqlite table_group_by at 1 clients: 8.00 (7.00-9.00) -> 6.00 (6.00-7.00) ops/s (0.75x, down)
  - sqlite point_pk at 1 clients: 516,533 (497,340-572,406) -> 467,506 (455,339-470,460) ops/s (0.91x, down)
  - sqlite insert_batch_100 at 32 clients: 91.00 (69.00-98.00) -> 100 (61.00-121) ops/s (1.10x, up)

**Reading the table (numbers from this run; no cause is claimed).**

- **By family (geometric means above, Amphora over the better of Turso and SQLite).** Point reads: 0.59x at 1 client, 0.81x at 32, 1.92x at 128. Writes: 0.85x at 1 client, 8.12x at 32, 36.75x at 128. Analytics: 0.73x at 1 client, 1.28x at 32, 1.27x at 128, where two of the five 128-client analytics cells have no ratio because one side answered nothing inside its 1 s window (the mean leaves them out).
- **Counts.** Of the 51 cells, Amphora is ahead in 23, behind in 16 and in an overlap of rounds (no measured difference) in 12; by median alone it is above 1x in 28 and below in 23.
- **Answers.** Every compared read answer of Amphora and of Turso equals SQLite's, value for value (`suite_answers.py`: 1,026 a pass, in the smoke pass and in all five rounds, 0 differ), so the speeds compared are of the same answers.
- **Order.** The odd rounds (Amphora, then Turso, then SQLite) were faster than the even ones for Amphora (1.15x) and for Turso (1.15x), and about equal for SQLite (0.97x): a drift of the machine shared by two of the three targets, not the same shift for all. Per-round ratios (the table above) take each round's own pair. A cell whose rounds' ranges touch is called overlap, not ahead or behind.
- **Limiter, not named.** The benchmark checklist asks for a named limiter before a difference is called a finding. None was taken here (the brief leaves causes to the next lanes' traces), so "behind" means "measured slower in this suite", not "because of X". The suite is a closed-loop, per-statement micro-benchmark inside the driver's process; it claims nothing about any end-to-end path.
- **Beside S4's table.** Amphora: 31 cells inside S4's range (no measured change), 12 above, 8 below. Turso: 34 inside, 0 above, 17 below. SQLite: 44 inside, 2 above, 5 below. The list above names the largest moves of each. Turso now waits on its own busy handler (lane F3, b02b2759), which changes how its contended writes run here; Turso's 1-client write cells are also below S4's and no busy wait applies there, so no single cause is claimed. The engine under Amphora moved 100+ commits since S4.

## What is in, and what is not

- In: **amphora** (one group, in process), **turso** (upstream Turso at 3e09a82, `turso_core` 0.9.0-pre.1, a connection a client) and **sqlite** (SQLite 3.53.2, bundled by rusqlite 0.40.2, WAL with `synchronous = FULL`, a connection a client). One binary (`amphora-bench`), one client code path (`bench/suite.sh` through `bench/run.sh`), the same 200,000-row data set, all three engines in the driver's process, on the same machine.
- Not in: **libsql**. The suite has no target for it (`bench/src/suite_db.rs` opens amphora, mvcc, turso, sqlite, postgres, orioledb, cedardb, mysql and clickhouse); its comparison is in the replication ladder (`bench/servers/repl`). `mvcc` is Amphora's own engine in MVCC mode, not a competitor. The PostgreSQL-family servers and MySQL are not SQLite-family and need a server, so they are left to S4's `suite-servers`.
- Statements: 17 of the suite's 52, the ones lane S6 ran (`--only`, in `rounds.sh`). Lane S4 ran all 52 (13:07:27Z to 14:29:16Z, 82 minutes for 5 rounds); this lane had 70 minutes including a build, so five rounds of all 52 could not fit and the S6 list was used instead. Items (200,000), clients (1, 32, 128), seconds (1) and warm-up (0.5) are S4's and S6's.

## How it ran

- Commit: 0101aa386850d1e8e641c6254238567adf9d8aa7 (origin/main when the lane started, 2b0d5e7c in its history). Clone `~/.cache/amphora-perf/bench-s` on adacachy2 (runner 00aad8daff2fa3c9daa65d53c425d450), HEAD at that SHA, `git status` empty. No tracked file was changed.
- Build (kiln run r561, 05:59:42Z to 06:04:05Z, 4 min 23 s at 16 jobs): `cargo build --release --offline --manifest-path bench/Cargo.toml --bin amphora-bench`, rustc and cargo 1.99.1, the bench workspace's release profile with lane S4's environment: `LTO=thin CODEGEN_UNITS=1 OVERFLOW_CHECKS=true DEBUG=line-tables-only INCREMENTAL=0`. No debug build. Binary kept at `~/.cache/amphora-perf-bin/bench-s7/amphora-bench-0101aa38...`, sha256 9587ed003a24ed89d6525606a0e84e378770b2730407e09fe32ad7ddfd3e118e (checked byte for byte against the build before and after the rounds).
- The first build attempt used `--locked`, as lane S4 did, and cargo refused it: `bench/Cargo.lock` is git-ignored and had not been brought up to date with main's new path dependencies (`amphora-pg-catalog`, and `tempfile` in the bench). `--offline` wrote it: the lock's diff is 9 lines (`bench-cargo-lock.diff`: `tempfile` and the new `amphora-pg-catalog` entry), and no registry crate changed version. The engines' versions come from `bench/Cargo.toml`, unchanged since S4's commit: turso 3e09a82, rusqlite 0.40.2.
- The binary is the one `bench/run.sh` starts from the clone: `target/release/amphora-bench` in the lane's target dir, compared with the saved copy before the run and after it.
- Runs (kiln run r584): `ROUNDS=5 bash rounds.sh all`, the scripts taken from branch perf/bench-s7b (commit 9155f400) and checked by sha256 before use. It asked for 16 cores and was refused (15 of 32 idle), then 12 cores and 24 GiB and was placed (`room for 12 CPUs`). That is a reservation, not a limit: the run's cgroup has no `cpu.max` limit the bench process in round 5 had `cpu.max` = `max 100000` and no cpuset file in its cgroup. `nice -n 10` from the suite, as S4 and S6 (the bench process in round 5 showed NI -4, so kiln's run sets a base below it; S4's and S6's runs were not checked for that). Data in the default `~/.cache/amphora-bench-data/<target>` (btrfs, zstd:1, no file attributes), as S4's records say. No environment variables set beyond those `rounds.sh` shows.
- Order: amphora, turso, sqlite in rounds 1, 3 and 5; sqlite, turso, amphora in rounds 2 and 4. A smoke pass ran first (clients 1 and 128, 0.3 s windows): every target answered all 34 statement-and-client cells with no error, and its answers were compared (`suite_answers.py`, 1,026 read answers of Amphora and of Turso against SQLite's, 0 differ).
- Each round's answers were compared the same way before any write of that round; the verdicts are in `roundN-answers.txt`: 
  - smoke: amphora vs sqlite: 1026 answers compared, 0 differ or failed; turso vs sqlite: 1026 answers compared, 0 differ or failed (suite_answers rc=0)
  - round1: amphora vs sqlite: 1026 answers compared, 0 differ or failed; turso vs sqlite: 1026 answers compared, 0 differ or failed (suite_answers rc=0)
  - round2: amphora vs sqlite: 1026 answers compared, 0 differ or failed; turso vs sqlite: 1026 answers compared, 0 differ or failed (suite_answers rc=0)
  - round3: amphora vs sqlite: 1026 answers compared, 0 differ or failed; turso vs sqlite: 1026 answers compared, 0 differ or failed (suite_answers rc=0)
  - round4: amphora vs sqlite: 1026 answers compared, 0 differ or failed; turso vs sqlite: 1026 answers compared, 0 differ or failed (suite_answers rc=0)
  - round5: amphora vs sqlite: 1026 answers compared, 0 differ or failed; turso vs sqlite: 1026 answers compared, 0 differ or failed (suite_answers rc=0).
- Load, CPU pressure and I/O pressure sampled every 5 s (`phases.log`, `PHASES.md`), the machine's top processes at each round's start (`procs.txt`).

## Changes in the harness and the competitors since lane S4 (7ac4153d)

- **Turso waits on its own busy handler** (lane F3, b02b2759, the code; landed on main at 81b3cb3d, a64549cc names it). S4's harness answered Turso's `StepResult::Sleep` with a thread sleep and retried a busy write after a 200 µs sleep of its own; now the bench sets Turso's busy timeout (120 s) and, on Sleep, steps again, as Turso's SDK does. Every Turso write cell at 32 and 128 clients is therefore a different measurement from S4's.
- **SQLite's settings are the same** (WAL, `synchronous = FULL`, a 120 s busy timeout, now set through `busy_timeout(WAIT)` instead of a PRAGMA).
- **The suite checks every read's answers before it times anything** (lane S7/S8, d2a9e6ee): one untimed read per statement and client before the first timed run, which warms every target alike and was not in S4.
- **Amphora's checkpointer is told what its log holds** (lanes W1/S7, e57a028b, b83da2fe, 795faa83), **bench's sleeps wait on events** (F2, 534877d3), **`run.sh` hands the report through a file** (S8, 701486af), **the engine under test is Amphora at today's main**: 100+ commits of engine change since S4's commit, which this lane did not look at.

## Load that was not this lane's

The window was to have no other swarm work on adacachy2. The machine ran other people's processes during the whole run; they were read-only to this lane and nothing was touched. `procs.txt` holds what was using the most CPU at each round's start.

- `kiln-5b8c64ebd8e73358 --test-threads=8`, the debug test binary of lane u2-kiln (PID 873176, nice 10): 6.4 cores (636-638% CPU) at every snapshot, from 05:56Z (its parent test run, PID 2377911, `cargo test --locked -p kiln --lib`, was seen at 06:03Z; the binary had run 10.7 hours by the smoke pass).
- `build_script_build` of kiln in the reach checkout `agy-int-f7b7b83737` (PID 2428752, nice 10): 1.1 to 1.7 cores in the smoke and round 1-3 snapshots.
- Slates lanes (through kiln's reach: `w-wall`, `w-ci-target`, `w-tsan*`): `cargo clippy --workspace`, `cargo test -p slates-server`, its rustc and rust-lld, at up to 2 cores each snapshot.
- kiln's `cargo check -p kiln --all-targets` (06:02Z), a kiln test server (PID 2777668, 0.3 cores), kiln's agents, Docker and a QEMU VM (under 0.1 cores).

Load, CPU and I/O pressure per phase (from `phases.log`, 5 s samples; the load average counts this run's own threads too):

| phase | order | start | end | samples | load1 median (range) | CPU pressure some avg10 % | IO some avg10 % | IO full avg10 % |
|---|---|---|---|---|---|---|---|---|
| smoke |  | 2026-11-24T06:06:58Z | 2026-11-24T06:11:26Z | 54 | 79.3 (18.2-110.0) | 42.8 (2.1-79.4) | 2.8 (1.4-8.6) | 0.4 (0.0-2.7) |
| round 1 | amphora,turso,sqlite | 2026-11-24T06:11:26Z | 2026-11-24T06:19:16Z | 94 | 75.2 (31.3-111.1) | 39.2 (1.7-83.7) | 1.6 (0.3-5.2) | 0.4 (0.0-3.3) |
| round 2 | sqlite,turso,amphora | 2026-11-24T06:19:16Z | 2026-11-24T06:27:25Z | 98 | 56.1 (28.2-102.3) | 34.0 (2.1-83.9) | 2.1 (0.5-5.0) | 0.4 (0.0-2.8) |
| round 3 | amphora,turso,sqlite | 2026-11-24T06:27:25Z | 2026-11-24T06:35:32Z | 98 | 56.3 (25.4-88.8) | 32.9 (1.9-82.6) | 2.4 (0.7-12.4) | 0.8 (0.0-7.7) |
| round 4 | sqlite,turso,amphora | 2026-11-24T06:35:32Z | 2026-11-24T06:44:03Z | 101 | 61.3 (24.4-121.8) | 33.0 (2.0-89.5) | 2.8 (1.1-5.4) | 0.8 (0.0-3.0) |
| round 5 | amphora,turso,sqlite | 2026-11-24T06:44:03Z | 2026-11-24T06:52:47Z | 105 | 67.3 (37.1-113.6) | 33.1 (2.5-84.4) | 3.8 (0.7-14.9) | 0.9 (0.0-4.8) |
Whole run: 549 samples, load1 63.7 (18.2-121.8), CPU pressure 34.7 (1.7-89.5), IO some 2.4 (0.3-14.9), IO full 0.7 (0.0-7.7).

## 2. Amphora against the best SQLite-family competitor

ratio = Amphora's median ops/s over that competitor's. 'ahead' = Amphora's slowest round beats the competitor's fastest; 'behind' = the reverse; 'overlap' = the two ranges of rounds touch, so no measured difference.

| op | clients | Amphora ops/s | Turso ops/s | SQLite ops/s | best competitor | Amphora/best | range verdict | Amphora p99 ms | best p99 ms | samples (Amphora/best, min) |
|---|---|---|---|---|---|---|---|---|---|---|
| point_pk | 1 | 289,111 (266,794-304,329) | 491,360 (477,224-504,863) | 467,506 (455,339-470,460) | turso | 0.59x | behind | 0.005 (0.005-0.006) | 0.003 (0.003-0.003) | 266794/477224 |
| point_pk | 32 | 896,806 (883,766-931,850) | 1,032,340 (1,022,602-1,125,872) | 237,091 (224,313-259,190) | turso | 0.87x | behind | 0.226 (0.194-0.234) | 0.412 (0.340-0.508) | 883766/1022602 |
| point_pk | 128 | 788,396 (779,670-813,947) | 564,800 (491,044-622,411) | 139,869 (123,914-199,840) | turso | 1.40x | ahead | 1.28 (1.15-1.36) | 5.57 (4.48-6.27) | 779670/491044 |
| point_secondary | 1 | 223,963 (213,689-233,072) | 351,083 (325,837-364,019) | 192,905 (172,844-200,668) | turso | 0.64x | behind | 0.007 (0.007-0.008) | 0.004 (0.004-0.005) | 213689/325837 |
| point_secondary | 32 | 785,146 (744,927-813,999) | 1,031,007 (994,187-1,084,141) | 53,342 (47,671-55,176) | turso | 0.76x | behind | 0.296 (0.276-0.344) | 0.412 (0.364-0.484) | 744927/994187 |
| point_secondary | 128 | 534,259 (506,740-562,743) | 318,634 (295,974-323,927) | 41,115 (39,144-43,009) | turso | 1.68x | ahead | 1.86 (1.78-1.97) | 8.96 (8.19-9.47) | 506740/295974 |
| point_text | 1 | 204,761 (194,009-210,093) | 320,089 (293,451-333,497) | 178,393 (170,757-205,705) | turso | 0.64x | behind | 0.008 (0.007-0.009) | 0.005 (0.005-0.006) | 194009/293451 |
| point_text | 32 | 769,307 (742,259-785,194) | 1,018,706 (952,783-1,100,433) | 60,628 (58,881-63,093) | turso | 0.76x | behind | 0.316 (0.288-0.332) | 0.428 (0.380-0.552) | 742259/952783 |
| point_text | 128 | 514,862 (471,425-543,633) | 285,363 (278,781-339,592) | 48,975 (41,833-54,577) | turso | 1.80x | ahead | 1.98 (1.90-2.05) | 9.09 (7.81-10.75) | 471425/278781 |
| json_field | 1 | 260,010 (246,809-262,478) | 422,861 (407,262-434,274) | 477,536 (433,158-483,932) | sqlite | 0.54x | behind | 0.006 (0.006-0.007) | 0.003 (0.003-0.004) | 246809/433158 |
| json_field | 32 | 870,272 (830,210-898,398) | 1,062,445 (984,569-1,164,719) | 231,871 (225,187-243,731) | turso | 0.82x | behind | 0.236 (0.220-0.296) | 0.360 (0.324-0.648) | 830210/984569 |
| json_field | 128 | 777,486 (746,034-801,836) | 305,124 (295,597-357,382) | 168,812 (159,775-204,989) | turso | 2.55x | ahead | 1.33 (1.23-1.82) | 8.70 (7.74-11.01) | 746034/295597 |
| blob_read | 1 | 295,294 (267,416-307,416) | 546,792 (474,229-561,670) | 562,081 (512,718-570,622) | sqlite | 0.53x | behind | 0.005 (0.005-0.008) | 0.003 (0.002-0.003) | 267416/512718 |
| blob_read | 32 | 894,561 (835,023-914,108) | 1,026,983 (856,425-1,090,594) | 245,345 (223,825-268,822) | turso | 0.87x | overlap (no measured difference) | 0.234 (0.204-0.268) | 0.380 (0.336-0.824) | 835023/856425 |
| blob_read | 128 | 778,556 (764,328-850,023) | 316,959 (286,804-333,496) | 182,405 (165,614-203,463) | turso | 2.46x | ahead | 1.30 (1.09-1.49) | 8.45 (8.19-10.37) | 764328/286804 |
| insert_one | 1 | 266 (111-272) | 260 (119-286) | 234 (102-281) | turso | 1.02x | overlap (no measured difference) | 12.42 (5.70-22.02) | 10.88 (8.58-25.09) | 111/119 [<1000] |
| insert_one | 32 | 3,286 (1,888-5,088) | 252 (139-301) | 215 (104-305) | turso | 13.04x | ahead | 22.27 (10.62-69.63) | 32.51 (9.09-46.59) | 1888/139 [<1000] |
| insert_one | 128 | 11,961 (5,853-17,874) | 159 (62.00-217) | 134 (87.00-264) | turso | 75.23x | ahead | 25.09 (13.18-126) | 1,163 (655-1,278) | 5853/62 [<1000] |
| insert_batch_100 | 1 | 134 (75.00-167) | 64.00 (49.00-89.00) | 81.00 (47.00-95.00) | sqlite | 1.65x | overlap (no measured difference) | 16.13 (10.62-26.11) | 47.10 (32.77-117) | 75/47 [<1000] |
| insert_batch_100 | 32 | 288 (256-416) | 66.00 (39.00-71.00) | 100 (61.00-121) | sqlite | 2.88x | ahead | 147 (113-176) | 1,163 (1,040-1,262) | 256/61 [<1000] |
| insert_batch_100 | 128 | 384 (384-512) | 26.00 (20.00-31.00) | 73.00 (55.00-113) | sqlite | 5.26x | ahead | 369 (295-389) | 1,376 (1,262-1,458) | 384/55 [<1000] |
| update_plain | 1 | 126 (108-310) | 134 (111-285) | 133 (126-295) | turso | 0.94x | overlap (no measured difference) | 12.93 (5.70-16.64) | 13.82 (7.49-20.74) | 108/111 [<1000] |
| update_plain | 32 | 2,311 (1,760-5,152) | 153 (130-288) | 121 (91.00-321) | turso | 15.10x | ahead | 23.81 (11.78-35.33) | 14.08 (8.00-20.48) | 1760/130 [<1000] |
| update_plain | 128 | 8,289 (5,525-18,944) | 123 (106-238) | 126 (8.00-312) | sqlite | 65.79x | ahead | 24.06 (13.31-35.84) | 14.21 (6.02-1,442) | 5525/8 [<1000] |
| update_indexed | 1 | 125 (101-275) | 167 (107-277) | 118 (106-320) | turso | 0.75x | overlap (no measured difference) | 12.67 (10.37-19.71) | 14.46 (9.09-57.86) | 101/107 [<1000] |
| update_indexed | 32 | 1,952 (1,831-4,497) | 251 (131-305) | 123 (108-312) | turso | 7.78x | ahead | 24.32 (19.97-63.49) | 16.90 (7.68-737) | 1831/131 [<1000] |
| update_indexed | 128 | 6,272 (4,753-14,397) | 175 (110-214) | 249 (81.00-302) | sqlite | 25.19x | ahead | 45.57 (13.95-97.28) | 934 (6.14-1,229) | 4753/81 [<1000] |
| upsert | 1 | 120 (115-238) | 161 (113-311) | 243 (49.00-315) | sqlite | 0.49x | overlap (no measured difference) | 12.67 (9.34-15.36) | 12.80 (5.18-590) | 115/49 [<1000] |
| upsert | 32 | 2,117 (1,857-4,489) | 133 (118-209) | 280 (107-325) | sqlite | 7.56x | ahead | 23.55 (13.44-48.64) | 8.58 (5.50-23.55) | 1857/107 [<1000] |
| upsert | 128 | 7,808 (5,069-11,648) | 124 (94.00-195) | 141 (118-316) | sqlite | 55.38x | ahead | 46.08 (25.09-348) | 13.82 (5.70-844) | 5069/118 [<1000] |
| delete_insert | 1 | 118 (114-320) | 101 (84.00-234) | 208 (70.00-301) | sqlite | 0.57x | overlap (no measured difference) | 14.72 (5.63-27.39) | 9.60 (5.82-32.77) | 114/70 [<1000] |
| delete_insert | 32 | 1,873 (1,351-4,704) | 137 (75.00-152) | 277 (145-306) | sqlite | 6.76x | ahead | 27.65 (11.01-111) | 15.36 (6.27-827) | 1351/145 [<1000] |
| delete_insert | 128 | 6,912 (3,456-15,004) | 94.00 (74.00-219) | 112 (38.00-272) | sqlite | 61.71x | ahead | 31.23 (16.64-119) | 1,130 (541-1,343) | 3456/38 [<1000] |
| txn_read_write | 1 | 125 (113-312) | 112 (101-237) | 129 (58.00-293) | sqlite | 0.97x | overlap (no measured difference) | 13.31 (5.70-18.18) | 42.50 (15.10-336) | 113/58 [<1000] |
| txn_read_write | 32 | 2,016 (1,856-5,032) | 116 (94.00-175) | 196 (60.00-309) | sqlite | 10.29x | ahead | 25.60 (11.65-51.20) | 52.74 (5.89-942) | 1856/60 [<1000] |
| txn_read_write | 128 | 5,499 (4,016-11,486) | 78.00 (0.000-206) | 136 (80.00-279) | sqlite | 40.43x | ahead | 31.23 (24.32-77.82) | 1,229 (934-1,327) | 4016/80 [<1000] |
| table_count | 1 | 33,614 (32,815-35,005) | 40,956 (40,545-41,961) | 8,591 (8,124-8,975) | turso | 0.82x | behind | 0.041 (0.038-0.042) | 0.031 (0.029-0.032) | 32815/40545 |
| table_count | 32 | 338,099 (285,697-347,421) | 703,154 (663,269-766,716) | 2,630 (2,340-2,680) | turso | 0.48x | behind | 0.744 (0.696-1.02) | 0.384 (0.312-0.484) | 285697/663269 |
| table_count | 128 | 295,153 (233,943-316,555) | 584,889 (576,774-606,571) | 2,019 (1,688-2,079) | turso | 0.50x | behind | 5.18 (4.67-7.74) | 3.20 (3.07-3.42) | 233943/576774 |
| table_group_by | 1 | 17.00 (16.00-18.00) | 4.00 (4.00-5.00) | 6.00 (6.00-7.00) | sqlite | 2.83x | ahead | 67.58 (58.37-68.61) | 166 (150-172) | 16/6 [<1000] |
| table_group_by | 32 | 92.00 (81.00-96.00) | 11.00 (0.000-19.00) | 0.000 (0.000-0.000) | turso | 8.36x | ahead | 406 (365-434) | 1,442 (1,393-1,458) | 81/0 [<1000] |
| table_group_by | 128 | 2.00 (0.000-82.00) | 0.000 (0.000-0.000) | 0.000 (0.000-0.000) | turso | inf (none answered in the window) | overlap (no measured difference) | 1,475 (1,458-1,475) | - | 0/0 [<1000] |
| filter_no_index | 1 | 25.00 (24.00-27.00) | 40.00 (36.00-43.00) | 45.00 (41.00-50.00) | sqlite | 0.56x | behind | 44.54 (42.50-52.22) | 26.62 (25.09-33.28) | 24/41 [<1000] |
| filter_no_index | 32 | 95.00 (64.00-95.00) | 119 (92.00-123) | 8.00 (6.00-32.00) | turso | 0.80x | overlap (no measured difference) | 422 (397-492) | 565 (504-655) | 64/92 [<1000] |
| filter_no_index | 128 | 0.000 (0.000-1.00) | 0.000 (0.000-4.00) | 0.000 (0.000-0.000) | turso | inf (none answered in the window) | overlap (no measured difference) | 1,475 (1,475-1,475) | 1,360 (1,343-1,376) | 0/0 [<1000] |
| aggregates_range | 1 | 4,026 (3,745-4,122) | 5,497 (4,956-5,647) | 6,222 (5,616-6,486) | sqlite | 0.65x | behind | 0.420 (0.384-0.496) | 0.300 (0.268-0.340) | 3745/5616 |
| aggregates_range | 32 | 15,960 (10,977-17,950) | 33,100 (25,402-35,635) | 4,600 (4,104-8,095) | turso | 0.48x | behind | 4.48 (4.16-7.62) | 3.26 (2.98-3.84) | 10977/25402 |
| aggregates_range | 128 | 12,541 (11,881-13,983) | 9,120 (7,434-10,155) | 3,490 (3,299-3,711) | turso | 1.38x | ahead | 22.27 (20.99-23.81) | 65.54 (54.27-76.80) | 11881/7434 |
| join_three | 1 | 542 (512-554) | 65.00 (47.00-70.00) | 2,159 (2,074-2,246) | sqlite | 0.25x | behind | 2.66 (2.50-2.72) | 0.688 (0.624-0.712) | 512/2074 [<1000] |
| join_three | 32 | 2,198 (868-2,975) | 966 (579-1,013) | 976 (939-1,072) | sqlite | 2.25x | overlap (no measured difference) | 37.38 (31.49-68.61) | 41.98 (38.40-42.50) | 868/939 [<1000] |
| join_three | 128 | 2,389 (1,417-4,628) | 811 (683-862) | 722 (683-811) | turso | 2.95x | ahead | 207 (164-451) | 422 (319-463) | 1417/683 [<1000] |

Counts over 51 cells: ahead 23, behind 16, overlapping ranges 12. By median ratio alone: 28 above 1x, 23 below.

### By family and client count: geometric mean of Amphora/best (cells where either side answered nothing left out, and counted)

| family | 1 client | 32 clients | 128 clients |
|---|---|---|---|
| point reads | 0.59x over 5 ops | 0.81x over 5 ops | 1.92x over 5 ops |
| OLTP writes | 0.85x over 7 ops | 8.12x over 7 ops | 36.75x over 7 ops |
| analytics | 0.73x over 5 ops | 1.28x over 5 ops | 1.27x over 3 ops, 2 left out |

### Order check: each target's odd rounds (order 1) against its even rounds (reversed order)

Each target's cells: the geometric mean of (median of odd rounds) / (median of even rounds) over the cells where both are above 0. Near 1.0 means the order of the targets moved nothing systematic; the rounds are alternated so a drift in the machine falls on both orders.

| target | geometric mean odd/even | cells |
|---|---|---|
| amphora | 1.150 | 50 |
| turso | 1.149 | 50 |
| sqlite | 0.971 | 48 |

### Per round: geometric mean of Amphora/best competitor in that round (competitor fixed by the overall median), by client count

Round i's own pair: Amphora's ops/s over the same competitor's in the same round, so each round's ratio is taken under one machine state. Rounds 1, 3 and 5 ran Amphora first (odd), rounds 2 and 4 ran it last (even). Cells where either answered nothing are left out of that round.

| clients | round 1 (first) | round 2 (last) | round 3 (first) | round 4 (last) | round 5 (first) |
|---|---|---|---|---|---|
| 1 | 0.63x over 17 cells | 0.78x over 17 cells | 0.94x over 17 cells | 0.74x over 17 cells | 0.82x over 17 cells |
| 32 | 1.96x over 17 cells | 2.67x over 17 cells | 3.05x over 17 cells | 2.71x over 17 cells | 2.48x over 16 cells |
| 128 | 5.42x over 16 cells | 6.51x over 15 cells | 8.65x over 15 cells | 5.90x over 15 cells | 11.04x over 15 cells |

## 3. The ten largest gaps (Amphora furthest behind the best competitor)

| rank | op | clients | Amphora/best | best competitor | Amphora ops/s (median, range) | best ops/s (median, range) | Amphora p50 / p99 ms | best p50 / p99 ms | range verdict | min samples |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | join_three | 1 | 0.25x (4.0x slower) | sqlite | 542 (512-554) | 2,159 (2,074-2,246) | 1.79 / 2.66 | 0.448 / 0.688 | behind | 512 [<1000] |
| 2 | table_count | 32 | 0.48x (2.1x slower) | turso | 338,099 (285,697-347,421) | 703,154 (663,269-766,716) | 0.048 / 0.744 | 0.030 / 0.384 | behind | 285697 |
| 3 | aggregates_range | 32 | 0.48x (2.1x slower) | turso | 15,960 (10,977-17,950) | 33,100 (25,402-35,635) | 1.87 / 4.48 | 0.768 / 3.26 | behind | 10977 |
| 4 | upsert | 1 | 0.49x (2.0x slower) | sqlite | 120 (115-238) | 243 (49.00-315) | 8.83 / 12.67 | 3.04 / 12.80 | overlap | 49 [<1000] |
| 5 | table_count | 128 | 0.50x (2.0x slower) | turso | 295,153 (233,943-316,555) | 584,889 (576,774-606,571) | 0.057 / 5.18 | 0.030 / 3.20 | behind | 233943 |
| 6 | blob_read | 1 | 0.53x (1.9x slower) | sqlite | 295,294 (267,416-307,416) | 562,081 (512,718-570,622) | 0.003 / 0.005 | 0.001 / 0.003 | behind | 267416 |
| 7 | json_field | 1 | 0.54x (1.8x slower) | sqlite | 260,010 (246,809-262,478) | 477,536 (433,158-483,932) | 0.003 / 0.006 | 0.001 / 0.003 | behind | 246809 |
| 8 | filter_no_index | 1 | 0.56x (1.8x slower) | sqlite | 25.00 (24.00-27.00) | 45.00 (41.00-50.00) | 38.40 / 44.54 | 21.76 / 26.62 | behind | 24 [<1000] |
| 9 | delete_insert | 1 | 0.57x (1.8x slower) | sqlite | 118 (114-320) | 208 (70.00-301) | 9.09 / 14.72 | 4.29 / 9.60 | overlap | 70 [<1000] |
| 10 | point_pk | 1 | 0.59x (1.7x slower) | turso | 289,111 (266,794-304,329) | 491,360 (477,224-504,863) | 0.003 / 0.005 | 0.001 / 0.003 | behind | 266794 |

## Files in this directory

- `TABLES.md`: the full tables: section 1 (every op, client count and target, with ops/s, p50, p99, p99.9, errors and retries, median and range over rounds), section 2 and 3 (below in this record), section 4 (beside lane S4's table, every target, every cell), and the order check.
- `PHASES.md`: load, CPU pressure and I/O pressure per phase, from `phases.log` (every 5 s). `rounds.txt`: the load, CPU and I/O pressure as each round started and ended. `procs.txt`: the processes using the most CPU when each round started (command lines redacted for anything that looks like a secret).
- `round1.jsonl` .. `round5.jsonl`, `smoke.jsonl`: the raw suite records (`bench/suite.sh` as S4 wrote them), with each record's answers removed (they are read back by `suite_answers.py`, value for value, before the timed runs; the verdicts are `roundN-answers.txt`, `smoke-answers.txt`). The full raw files with answers stay on adacachy2 in `~/.cache/amphora-perf/bench-s7-run/out`.
- `rounds.sh`, `s7_tables.py`, `phases_summary.py`, `smoke_summary.py`, `finish.sh`: the scripts as run. `rounds.sh` ran at sha256 98d1bdae... (checked before the run).
- `bench-cargo-lock.diff`: what `cargo build --offline` added to the untracked `bench/Cargo.lock`. `build.log.tail`: the end of the build's log.

## Not done in this lane

- No cause was looked for: no profile, trace or log of the engines was taken, so every loss here is a measured gap, not an explanation (RULES.md 0; the next lanes' traces own the causes).
- The 35 statements S6's list leaves out (the analytics beyond the five, the text, range and JSON operations, the windowed and set functions) were not measured: see the statement list in `rounds.sh`.
- No single machine-state control: the machine ran other people's work throughout (see "Load that was not this lane's"), so a cell whose ranges of rounds touch is marked "overlap", and only cells whose ranges don't touch are called ahead or behind.
