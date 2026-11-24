# Lane Q3: what limits point reads, and the spin lock every read took twice (adacachy2, 2026-11-23)

Deliverable status, losses first.

- Not shown faster in statements a second. The alternated A/B (r1903, 5 rounds, Turso, SQLite and
  PostgreSQL 19 in each) is inconclusive: adacachy2 ran at 1-minute load 66-148 on 32 cores (CPU
  pressure 34-87%), and in every one of the 15 cells A's and B's ranges overlap. The 1-client cells,
  where no lock is contended and the change can't act, swung 0.60x to 1.78x: that is the noise floor.
  The change can make a cell at most 1/(1 - share) faster, 1.10-1.26x by the shares below, so the
  medians' 0.43x to 1.93x are noise, not the change.
- Shown, in the profile: the limiter's cost is gone. Alternated A, B, A, B at 32 and 128 clients
  (`abprof`, below), the read's begin and end in the log took 8.7-20.7% of the CPU samples on main
  and 0.1-1.6% with the fix, in all 10 cells; all spinning fell from 11.2-29.3% to 5.2-13.7%.
- Still behind in the A/B: Amphora trails PostgreSQL 19 at 32 and 128 clients in every point
  statement (B at 0.21x-0.68x of it), and the best competitor at one client in four of five
  (0.34x-0.56x of Turso or SQLite; point_pk's 2.36x of Turso there sits inside the noise above).
- Merged to main (gate below). The next spin site, find_frame's lock (decision 40's `Locked`
  default), now takes most of what spinning is left: 5.6-13.1% of the samples with the fix.

## Where the CPU goes and where clients wait (main c6f915b1)

Binary `amphora-bench-c6f915b1` (main with Q2's cache fix), a fresh 200,000-item load into the
lane's own folder (`q3_profile.sh`, r1813), then `--reuse` (`q3_trace.sh`, r1819). The host was
shared and loaded throughout: 1-minute load 65-99 on 32 cores, CPU pressure (some, avg10) 40-81%.
Profile shares and thread-state shares are what this section rests on; the throughputs beside them
are single runs on that host.

### 1 client: AMPHORA_PROFILE (997 Hz), inclusive shares

| statement | samples | ops/s | read_page | of it: miss (read_page_uncached) | cache insert (eviction) | index seek's read_varint | statement reset |
|---|---|---|---|---|---|---|---|
| point_pk | 868 | 204,283 | 25.1% | 13.6% | 6.1% (5.8%) | 6.0% (table interior) | 12.0% |
| point_secondary | 896 | 143,891 | 23.8% | 8.8% | 7.5% (6.6%) | 21.8% (16.6% index seek) | 6.8% |
| point_text | 533 | 63,567 | 25.7% | 10.1% | 8.1% (7.7%) | 18.6% (13.1% index seek) | 8.4% |
| json_field | 545 | 98,151 | 31.2% | 11.2% | 13.0% (12.5%) | 4.8% | 9.7% |
| blob_read | 776 | 211,427 | 32.5% | 15.7% | 10.7% (10.2%) | 7.9% | 9.7% |

At one client no lock spins. The time is the page fetch (a fifth to a third, misses in a fresh
reader's cache with their eviction and insert), the index binary search's varint decoding (point_secondary
and point_text: `cell_read_payload_at` in `indexbtree_seek_impl`), and the statement's reset.

### 32 and 128 clients: where the CPU spins (AMPHORA_PROFILE, `q3_callers.py` over the folded stacks)

Share of all CPU samples that end in `_mm_pause`, by the lock spun on:

| clients | statement | all spin | segments lock (begin_read + end_read) | find_frame's frame-index lock |
|---|---|---|---|---|
| 32 | point_pk | 15.0% | 9.7% (6.2 + 3.5) | 4.1% |
| 32 | point_secondary | 12.2% | 7.4% (2.9 + 4.5) | 3.6% |
| 32 | point_text | 12.9% | 7.2% (2.4 + 4.8) | 5.0% |
| 32 | json_field | 15.6% | 9.1% (5.0 + 4.1) | 4.1% |
| 32 | blob_read | 16.6% | 11.5% (7.4 + 4.1) | 2.9% |
| 128 | point_pk | 15.4% | 9.9% (4.3 + 5.6) | 4.5% |
| 128 | point_secondary | 26.5% | 16.2% (7.7 + 8.5) | 9.7% |
| 128 | point_text | 30.7% | 14.5% (5.8 + 8.7) | 15.7% |
| 128 | json_field | 30.4% | 20.6% (16.4 + 4.2) | 8.5% |
| 128 | blob_read | 28.7% | 20.6% (16.1 + 4.5) | 6.8% |

The segments lock is `SharedLog::segments`, a test-and-test-and-set `SpinLock<Segments>` that every
read transaction takes at its begin (`try_begin_read_tx` -> `SharedLog::begin_read`) and at its end
(`end_read_tx` -> `SharedLog::end_read`): two acquisitions a statement on one line every reader writes.
It is the larger spin site in 9 of the 10 multi-client cells; find_frame's lock (`frame_cache.lock()`
in `find_frame_locked`, taken on every private-cache miss because `LogReadIndex` defaults to `Locked`,
decision 40's prototype switch) is the larger in point_text at 128.

### 128 clients: where clients wait (`q1_sample.py`, every thread's state from /proc, no profiler)

| statement | running or runnable | futex wait | other |
|---|---|---|---|
| point_pk | 12.9% | 64.5% | mprotect 14.2% (threads' first allocations), pread 3.6% |
| point_secondary | 35.8% | 58.0% | |
| point_text | 70.0% | 24.6% | |
| json_field | 75.2% | 20.0% | |
| blob_read | 76.1% | 18.3% | |

The futex is not named by this trace (it records the syscall, not the address). The store's read
path waits on a futex in one place on this route: `Readers::with` (crates/amphora/src/store.rs)
parks a client on the pool's condition variable while all of its readers (`machine_readers()`, at
most the 32 cores here) are out. That reading is from the code, not from a trace: it is a lead.

## The fix

`log_segments::SegmentsCell` (36ed445e): the log's segments, their flags and both read counts in one
`AtomicU64` (flags in bits 60-63 as `Segments::pack` already laid them out for the multi-process
memory, 30 bits a count), changed by compare-and-swap, each change one step of the pure `Segments`
state machine applied whole. `SharedLog::begin_read` and `end_read` take no lock; a read past a
count's 30 bits is refused (`try_begin_read_tx` answers None, as for a busy read mark). No answer
changes: the steps are the state machine's own, and every read still counts itself on the segments
its snapshot reaches. Decision 45 (docs/design/store.md) says so.

Tests (storage::log_segments): `q3_the_cell_steps_as_the_state_machine_does` (20,000 seeded steps,
reads begun and ended, switches, copies and shipments, on a plain state and a cell: same state and
same answer after each), `q3_reads_on_many_threads_leave_no_count_and_free_the_closing_segment`
(eight threads, 20,000 reads each, beside a switch, copy and shipment: no read left counted and the
closing segment free), `q3_a_read_past_what_the_word_counts_is_refused`.

Gate at 36ed445e (r1887, adacachy2): `cargo test -p amphora-engine --lib -- storage::log_segments
storage::wal` 118 passed, 0 failed, 1 ignored; `cargo clippy -p amphora-engine --all-targets --locked
-- -D warnings` exit 0; `scripts/check-production.sh` exit 0 (engine ratchet 10,908, as it holds).
Re-checked on each merge with main: dc9d8575 (r1925: 118 passed, clippy 0, check-production 0) and
ac1547d6, after lane B16's decision 45 unit 4 landed and its reads of the segments were moved onto the
cell (121 passed, 0 failed, 1 ignored; clippy 0; check-production 0, ratchet 10,903). Landed on main as
bfa2d3f2 (main had moved only in crates/amphora, which amphora-engine doesn't depend on).

## The A/B (r1903)

`q3_ab.sh c6f915b1 36ed445e`: A is main before the change, B is it with the change. Five rounds
alternated (A,B then B,A), the five point statements at 1, 32 and 128 clients, 1 s after 0.5 s of
warm-up; Amphora, Turso and SQLite on one load each (`--reuse`), PostgreSQL 19.0 (Debian
19.0-1.pgdg13+1, servers/postgres.sh's settings, its own container on port 55491) loading its table
each run. One calibration for both arms (RULES 5e). Errors: 0 in every cell of every run. Statements
a second, median of 5 (range):

| statement | clients | A (c6f915b1) | B (36ed445e) | B / A | Turso | SQLite | PostgreSQL 19 | B / best competitor |
|---|---|---|---|---|---|---|---|---|
| point_pk | 1 | 87,579 (13,930-246,300) | 155,554 (26,699-240,199) | 1.78x | 65,863 (17,512-131,400) | 61,324 (17,392-275,345) | 46,185 (23,045-51,858) | 2.36x of turso |
| point_pk | 32 | 348,140 (332,997-784,371) | 486,971 (374,620-851,599) | 1.40x | 945,673 (898,978-1,071,916) | 223,890 (152,825-274,174) | 1,067,527 (980,074-1,161,433) | 0.46x of postgres |
| point_pk | 128 | 341,807 (231,673-710,319) | 444,317 (283,632-680,457) | 1.30x | 591,344 (540,221-664,460) | 162,566 (124,459-200,545) | 1,102,433 (1,007,445-1,179,051) | 0.40x of postgres |
| point_secondary | 1 | 125,657 (34,928-208,556) | 163,107 (54,590-200,190) | 1.30x | 288,883 (254,757-326,388) | 126,232 (82,046-181,853) | 50,350 (40,759-56,946) | 0.56x of turso |
| point_secondary | 32 | 309,220 (188,536-727,077) | 595,296 (286,874-778,914) | 1.93x | 882,262 (752,484-957,282) | 80,041 (61,198-93,518) | 1,048,459 (936,197-1,290,001) | 0.57x of postgres |
| point_secondary | 128 | 336,148 (117,550-527,807) | 298,250 (165,721-569,356) | 0.89x | 320,202 (269,976-515,419) | 51,323 (41,530-60,026) | 1,221,086 (1,203,699-1,257,918) | 0.24x of postgres |
| point_text | 1 | 163,112 (65,765-195,212) | 121,289 (54,796-166,388) | 0.74x | 266,866 (200,962-319,013) | 171,193 (127,252-196,844) | 44,527 (41,983-52,962) | 0.45x of turso |
| point_text | 32 | 598,492 (197,985-729,868) | 570,138 (160,174-633,101) | 0.95x | 873,827 (761,665-927,150) | 66,398 (58,258-103,898) | 1,050,211 (779,376-1,266,498) | 0.54x of postgres |
| point_text | 128 | 433,863 (120,220-507,119) | 320,883 (172,598-518,590) | 0.74x | 301,274 (272,133-305,542) | 50,651 (46,105-67,125) | 1,193,982 (1,129,150-1,293,137) | 0.27x of postgres |
| json_field | 1 | 202,899 (115,001-237,062) | 153,411 (57,410-232,760) | 0.76x | 350,915 (111,445-396,372) | 406,707 (363,251-474,751) | 49,013 (41,796-56,981) | 0.38x of sqlite |
| json_field | 32 | 757,433 (253,472-812,921) | 443,060 (356,398-783,751) | 0.58x | 938,739 (727,972-977,184) | 245,873 (214,479-331,319) | 972,427 (725,455-1,276,644) | 0.46x of postgres |
| json_field | 128 | 604,026 (220,580-669,567) | 259,253 (204,362-673,816) | 0.43x | 302,343 (288,656-336,447) | 156,675 (146,750-236,103) | 1,229,560 (1,116,964-1,353,003) | 0.21x of postgres |
| blob_read | 1 | 248,983 (104,201-279,933) | 148,339 (58,341-273,870) | 0.60x | 434,716 (234,631-480,137) | 426,439 (245,484-495,580) | 48,601 (45,996-58,347) | 0.34x of turso |
| blob_read | 32 | 714,236 (281,915-816,558) | 690,711 (259,231-865,428) | 0.97x | 895,412 (686,580-973,302) | 282,529 (234,246-327,080) | 1,020,854 (760,400-1,284,938) | 0.68x of postgres |
| blob_read | 128 | 579,236 (203,275-673,365) | 482,615 (233,508-768,388) | 0.83x | 293,569 (250,835-335,863) | 153,912 (150,089-173,982) | 1,219,328 (1,068,912-1,369,007) | 0.40x of postgres |

Load before each run (1-minute, CPU pressure some avg10): load-amphora 106.4 (36.1%), calib-A 109.0
(44.8%), calib-B 113.9 (52.1%); A-1 139.2 (59.9%), B-1 123.6 (58.5%), turso-1 131.2 (59.7%), sqlite-1
114.2 (65.1%), postgres-1 109.5 (47.7%); B-2 140.6 (82.7%), A-2 145.6 (87.4%), turso-2 134.0 (57.9%),
sqlite-2 147.8 (74.5%), postgres-2 119.9 (51.5%); A-3 119.5 (70.0%), B-3 119.1 (67.2%), turso-3 108.8
(50.4%), sqlite-3 114.2 (51.0%), postgres-3 92.6 (34.0%); B-4 70.0 (58.6%), A-4 84.4 (42.3%), turso-4
77.6 (39.6%), sqlite-4 84.4 (55.1%), postgres-4 76.2 (38.8%); A-5 71.4 (61.4%), B-5 69.8 (42.3%),
turso-5 72.1 (46.7%), sqlite-5 66.7 (51.2%), postgres-5 89.2 (53.4%); end 75.6. Q2's A/B (r1798) and
this lane's gate (r1887) ran beside rounds 1 and 2.

## Where the CPU went after the fix (abprof, alternated A, B, A, B)

AMPHORA_PROFILE at 32 and 128 clients on the A/B's database, each arm twice, alternated (00:43:44Z
to 00:44:54Z, load 42-66). Share of all CPU samples: `log` is SharedLog's begin_read and end_read
(the segments lock on A, the compare-and-swap on B), `spin` every sample ending in `_mm_pause`,
`find_frame` the frame lookup with its lock.

| statement | clients | log A / A2 | log B / B2 | spin A / A2 | spin B / B2 | find_frame A / A2 | find_frame B / B2 |
|---|---|---|---|---|---|---|---|
| point_pk | 32 | 14.6 / 14.7 | 1.6 / 1.2 | 17.3 / 18.7 | 6.6 / 8.3 | 6.4 / 6.2 | 5.9 / 7.2 |
| point_pk | 128 | 16.3 / 16.7 | 0.7 / 1.0 | 20.2 / 24.2 | 8.6 / 8.1 | 6.5 / 8.8 | 10.6 / 7.9 |
| point_secondary | 32 | 8.7 / 9.3 | 0.5 / 0.5 | 11.2 / 13.5 | 6.1 / 6.2 | 4.1 / 5.0 | 6.5 / 6.3 |
| point_secondary | 128 | 12.3 / 15.5 | 0.4 / 0.8 | 18.2 / 20.1 | 12.7 / 7.5 | 7.9 / 7.7 | 13.1 / 7.9 |
| point_text | 32 | 11.0 / 10.9 | 0.6 / 0.5 | 12.1 / 13.3 | 7.3 / 5.2 | 4.2 / 5.4 | 6.9 / 5.6 |
| point_text | 128 | 12.4 / 11.1 | 0.1 / 0.7 | 18.7 / 17.3 | 10.3 / 7.6 | 8.2 / 8.9 | 12.0 / 10.0 |
| json_field | 32 | 10.7 / 12.8 | 0.5 / 1.0 | 11.6 / 15.5 | 7.8 / 5.7 | 5.0 / 5.0 | 8.3 / 6.1 |
| json_field | 128 | 20.3 / 20.7 | 0.8 / 0.8 | 29.3 / 26.9 | 11.7 / 10.1 | 11.7 / 8.3 | 12.8 / 9.8 |
| blob_read | 32 | 12.4 / 15.8 | 1.3 / 1.2 | 15.5 / 19.7 | 8.2 / 8.3 | 6.4 / 5.1 | 7.7 / 7.3 |
| blob_read | 128 | 18.6 / 18.1 | 1.0 / 0.7 | 25.5 / 23.5 | 13.7 / 9.7 | 10.0 / 8.3 | 12.9 / 10.0 |

Command (a foreground kiln run on adacachy2, cpus=2,mem=8g): for each of A, B, A, B, `AMPHORA_PROFILE=~/.cache/analytics-q3/abprof-<arm>
amphora-bench-<sha> --suite --items 200000 --target amphora --dir ~/.cache/analytics-q3/ab-data/amphora
--clients 32,128 --seconds 1 --warmup 0.5 --only '<the five>' --reuse`, then the shares over the
`.folded` stacks (frames `begin_read`/`end_read`, `_mm_pause`, `find_frame*`).

## Commands

All on adacachy2 (runner 00aad8daff2fa3c9daa65d53c425d450, sync false), scripts in
`2026-11-23-adacachy2-perf-reads-q3/`, the lane's folder `~/.cache/analytics-q3`:
- r1813: `BIN=…/amphora-bench-c6f915b1… CLIENTS=1 TAG=main q3_profile.sh` (needs cpus=1,mem=6g after
  a refusal at cpus=2): the fresh load and the 1-client profiles.
- r1819: `BIN=… TAG=main q3_trace.sh` (cpus=3,mem=12g after a refusal at 8): the 128-client thread
  states (`~/.cache/analytics-q1/q1_sample.py`), then AMPHORA_PROFILE at 32 and 128 clients.
- `q3_callers.py prof-main-amphora-<statement>-<clients>.folded _mm_pause 3` for the spin table.
- r1887: `JOBS=6 q3_check.sh 36ed445e` (cpus=6,mem=20g): the bench built, the tests, clippy and
  check-production. r1925: the same three on the merge with main (dc9d8575), at cpus=2 (0 idle at 6
  and 4).
- r1903: `q3_ab.sh c6f915b1 36ed445e` (cpus=1,mem=12g after a refusal at cpus=16: 1 of 32 cores idle),
  then `q3_table.py ~/.cache/analytics-q3/ab/runs.jsonl`.
