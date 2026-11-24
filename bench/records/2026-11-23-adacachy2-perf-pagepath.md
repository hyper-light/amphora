# perf/pagepath: point reads, AUDIT 2.1 and 2.3 (2026-11-23, adacachy2)

Host: adacachy2 (runner 00aad8daff2fa3c9daa65d53c425d450), 32 cores, x86_64 Linux. NOT quiet: load
average 60-87 from other work through every run (CLAUDE.md §7 asks under 2); the ranges below are that
noise. Base: origin/main b817508b. Release bench binary (`cargo build --release --manifest-path
bench/Cargo.toml`), each binary copied aside, the same database (`--reuse`), base and after
alternating, three reps each:

    amphora-bench --target amphora --dir <data>/amphora --suite --reuse \
      --only 'point_pk$,point_secondary$' --clients 1,8,32,64,128 --seconds 5 --warmup 1 --items 200000

(script: perf/pagepath's `.git/perf-ab.sh`, records `~/.cache/amphora-perf-bin/pagepath/ab-0051.jsonl`
on adacachy2). Queries a second, median of 3 [min-max]; SQLite one run, same binary and client.

## AUDIT 2.1. WAL header publication and lock padding on the default path

The change landed as `e74813c8`. The original record labels the measured after binary
`bf23ccb1`; that object is absent from the fetched history. The runs below exercised
`PagePath::Today` with `LOG_READ_INDEX=locked`. In the raw `ab-0051.jsonl`, all 60
reported Today cells have zero errors, zero retries and 1.0 rows per operation.

For AUDIT 2.3, the distribution test in `a02e4b20` covers the first pages of 64 files and
consecutive pages of one file. The later record in `820614f8` selects `AMPHORA_PAGE_PATH=plain`
and alternates three runs per arm. Eight-client point reads increased from 1.42M to 1.79M/s
in that sample. The base predates the WAL change and the host load ranged from 42 to 71, so the
sample does not isolate the single-lookup change. In the raw `ab2-0110.jsonl`, all 60
Plain cells also have zero errors, zero retries and 1.0 rows per operation. The exact
eight-client primary-key medians are 1,424,040.6 and 1,794,026.6 reads/s.

| op | clients | base | after | after/base | SQLite |
|---|---|---|---|---|---|
| point_pk | 1 | 192,905 [159,502-279,074] | 249,427 [185,384-310,574] | 1.29 | 523,323 |
| point_pk | 8 | 1,048,047 [345,875-1,060,594] | 1,004,391 [992,818-1,214,045] | 0.96 | 769,260 |
| point_pk | 32 | 829,257 [808,473-913,400] | 881,652 [740,888-995,658] | 1.06 | 261,247 |
| point_pk | 64 | 809,632 [696,891-868,279] | 921,433 [878,925-928,751] | 1.14 | 225,140 |
| point_pk | 128 | 790,786 [761,445-792,039] | 838,570 [822,718-843,976] | 1.06 | 185,441 |
| point_secondary | 1 | 235,117 [167,361-241,638] | 247,138 [240,215-254,132] | 1.05 | 207,777 |
| point_secondary | 8 | 754,345 [748,200-906,522] | 860,024 [633,072-982,555] | 1.14 | 91,092 |
| point_secondary | 32 | 756,335 [599,812-857,805] | 674,282 [588,109-753,973] | 0.89 | 55,781 |
| point_secondary | 64 | 670,076 [439,000-741,801] | 705,136 [573,247-746,240] | 1.05 | 47,730 |
| point_secondary | 128 | 453,377 [389,654-468,188] | 439,142 [435,431-452,205] | 0.97 | 45,469 |

Reading: no regression outside the noise; point_pk at 64 and 128 clients is above base's whole range
(+14%, +6%). The 10x isn't here: the header was already read lock-free under the non-default read
indexes (decision 40), and the default's per-read cost is elsewhere (AUDIT 2.1's frame_cache lock,
2.2). To re-measure on a quiet host.

## AUDIT 2.3 (a02e4b20): the plain page path looks in the shared pages once; shards by file and page, padded

`AMPHORA_PAGE_PATH=plain`, base (origin/main b817508b) against a02e4b20's code, alternating, three reps
(script `.git/perf-ab2.sh`, records `ab2-0110.jsonl`; load 42-71). Median [min-max], queries a second:

| op | clients | base | after | after/base |
|---|---|---|---|---|
| point_pk | 1 | 365,827 [294,068-374,791] | 340,442 [330,981-376,127] | 0.93 (noise) |
| point_pk | 8 | 1,424,041 [1,312,275-1,660,093] | 1,794,027 [1,774,896-1,860,297] | 1.26 |
| point_pk | 32 | 934,830 [898,827-1,024,831] | 951,298 [925,215-960,041] | 1.02 |
| point_pk | 64 | 896,028 [792,993-994,306] | 935,551 [914,302-943,583] | 1.04 |
| point_pk | 128 | 880,984 [838,169-884,149] | 906,576 [898,926-945,065] | 1.03 |
| point_secondary | 8 | 1,010,083 [1,004,339-1,040,666] | 998,582 [970,545-1,007,210] | 0.99 |
| point_secondary | 32 | 921,690 [912,099-961,359] | 929,467 [893,977-953,320] | 1.01 |

## AUDIT 2.1, the log's read index (decision 40): the four ways on the default page path (perf-B2)

One binary (a02e4b20's), `AMPHORA_LOG_READ_INDEX` = locked / optimistic / published / filtered
alternating within each of three reps (script `.git/perf-ab3.sh`, records `ab3-0141.jsonl`; load
25-68), clients 1, 32, 128. Median [min-max]:

| op | clients | locked | optimistic | published | filtered |
|---|---|---|---|---|---|
| point_pk | 1 | 301,236 [279,146-330,995] | 328,648 [308,566-330,651] | 321,882 [313,962-323,446] | 307,418 [285,029-321,327] |
| point_pk | 32 | 949,151 [888,678-1,013,616] | 1,000,693 [998,833-1,035,090] | 928,678 [924,817-993,940] | 943,381 [878,449-953,500] |
| point_pk | 128 | 846,119 [768,536-851,280] | 862,263 [835,523-889,842] | 891,461 [763,330-909,631] | 851,883 [769,625-890,271] |
| point_secondary | 1 | 256,019 [239,485-257,272] | 271,788 [183,452-272,614] | 266,118 [238,855-278,832] | 261,473 [256,778-267,815] |
| point_secondary | 32 | 723,235 [658,390-723,502] | 732,026 [681,783-778,036] | 752,286 [687,720-781,980] | 676,965 [652,817-824,767] |
| point_secondary | 128 | 556,668 [468,883-596,688] | 396,963 [306,489-468,065] | 492,604 [323,461-562,226] | 410,350 [397,573-604,968] |

Reading: the ways are within each other's noise; optimistic's point_pk is +9% at 1 client and +5% at
32, nothing holds for point_secondary. AUDIT 2.1's "27-79% of cycles in the frame lookup" is NOT
verified by the suite, and can't be: the suite's settle runs `PRAGMA wal_checkpoint(TRUNCATE)`
(bench/src/suite_db.rs), the log is 12 KB through every read run, so `find_frame` is reached with
next to no frames. What the suite does show: every path and every way flattens at ~0.9-1.0 M/s from 32
clients on (the plain path too, which reaches 1.79 M/s at 8), with the kernel taking 55-60% of the
CPU at 32 and 128 clients (`cpu_sys_s` / total; 16-19% at one client). That ceiling is shared by every
page path and log way, so it lies outside them (the bench's reader hand-off, futexes or preads; adacachy2
has no perf or strace and its /proc syscall files aren't readable, so not yet named).

### The lookup itself with the log full: `storage::wal::tests::g3_2_log_lookup_bench` (perf-B2)

An ignored release micro-bench (no SQL, no pager): the log holds 32,612 pages (the suite's database)
written twice, 65,224 frames; each thread looks up random pages, half not in the log, every answer
checked against the locked index's. Lookups a second, all threads together (`cargo test -p
amphora-engine --release --lib g3_2_log_lookup_bench -- --ignored --nocapture`; threads = CPUs
allowed via `taskset`; 32 = the whole machine; records `lookup-0201.txt`, `scale-0207.txt`; load 32-73).
Reps in brackets:

| threads | locked | filtered | published | optimistic |
|---|---|---|---|---|
| 1 | 44.2 M, 34.5 M, 40.6 M (22.6-29.0 ns) | 23.4 M, 21.3 M, 20.4 M | 28.1 M, 20.6 M, 22.4 M | 28.9 M, 25.1 M, 24.6 M (34.6-40.6 ns) |
| 2 | 15.1 M, 16.1 M | 14.9 M, 15.2 M | 33.5 M, 35.4 M | 27.5 M, 38.2 M |
| 4 | 9.7 M, 8.9 M | 10.2 M, 10.0 M | 33.4 M, 35.4 M | 35.4 M, 37.2 M |
| 8 | 6.7 M, 6.8 M | 6.7 M, 7.0 M | 17.5 M, 17.1 M | 16.0 M, 16.7 M |
| 16 | 2.8 M, 2.8 M | 2.6 M, 2.6 M | 7.7 M, 7.5 M | 7.0 M, 7.9 M |
| 32 | 1.89 M, 1.65 M, 1.81 M | 1.90 M, 1.82 M, 1.77 M | 4.88 M, 5.09 M, 5.19 M | 4.63 M, 4.28 M, 4.75 M |

Reading: with the log full the lock is the collapse AUDIT 2.1 describes (one reader 22-29 ns, 32
readers 17-19 µs a lookup each, total down 23x); published and optimistic hold 2.2-3.9x more from 2
threads on. But they are 1.4-1.7x slower than the lock at one reader, and they collapse too past 4
threads (35 M/s at 4, 4.8 M/s at 32): a read-only lookup with no writer should scale with the cores,
so something shared is still written on each one. Not yet named (no perf on adacachy2); the
candidates are `ArcSwap::load` on `bits` and `latest` each lookup (its debt slots' SeqCst stores and
the node list) and the lines the ReadIndex's fields share. The fix to try next: a connection keeps its
`Arc<PageBits>`/`Arc<Latest>` and checks them against `generation` (one plain load) instead of an
`ArcSwap::load` per lookup, then this bench again at 1-32 threads. Default left at `locked`: the
graduation is Ada's decision (below), and the suite shows no end-to-end gain while its log is empty.

## AUDIT 2.1, decision 40 revised (Ada): the default way reads the header lock-free (ae729b06, perf/pagepath only)

Quick A/B (script `.git/perf-ab5.sh`, records `ab5-0229.jsonl`, load 59-90): ae729b06's bench binary
against bench-after2 (a02e4b20: header under its lock). CONFOUNDED: the after binary also carries
main's merges since (planner `learned_costs.rs`, vdbe). Median [reps]:

| op | clients | header locked (a02e4b20) | header lock-free (ae729b06) |
|---|---|---|---|
| point_pk | 1 | 294,357 [294,357 327,192 254,550] | 331,801 [328,539 331,801 333,732] |
| point_pk | 32 | 1,007,478 [1,040,832 912,363 1,007,478] | 948,410 [1,020,096 891,522 948,410] |
| point_pk | 128 | 799,975 [908,250 762,278 799,975] | 843,650 [863,913 794,409 843,650] |
| point_secondary | 1 | 265,780 [278,549 265,780 220,180] | 260,021 [260,021 215,147 272,757] |
| point_secondary | 32 | 787,423 [810,096 752,693 787,423] | 682,090 [391,727 706,333 682,090] |
| point_secondary | 128 | 578,757 [578,757 583,735 464,105] | 337,271 [380,134 307,379 337,271] |

Reading: point_secondary at 128 clients falls 42%, outside the noise (after's best is below base's
worst); A/B 3 above, one binary, also had every lock-free-header way under `locked` there (397-493k vs
557k). Not landed on main until a same-tree A/B (`.git/perf-ab5.sh`, rewritten by perf-B3; see its section below)
says whether the header or main's merge does it.
## The ~1 M/s ceiling from 32 clients on: the reader pool's std lock (perf-B3, AUDIT 2)

Tool: `bench/syscall_sample.py`. It starts the bench itself, so Yama's `ptrace_scope=1` lets it read
`/proc/<pid>/task/*/syscall`. It samples each thread's state, syscall and futex address, plus CPU
and context switches over the span. Binary: `bench-hdr-after` (ae729b06 merged with main 369141e8).
Records on adacachy2 are in `~/.cache/amphora-perf-bin/pagepath/`: `b3-sample-0256.jsonl`,
`b3-sample2-0305.jsonl` and `b3-checkout-0307.jsonl`. Load was 73-95 on 32 cores throughout.

Sampled at 32 clients (`point_pk`, 6 s span): 975k ops/s, 47.0 s system CPU against 48.2 s user,
676k voluntary context switches a second (0.69 an op) and 160k involuntary. The threads that sleep
wait in `futex(FUTEX_WAIT_BITSET_PRIVATE)`. That is Rust std's `Mutex`/`RwLock`/`Condvar` wait. The
engine's locks are parking_lot, which park each thread on its own address with `FUTEX_WAIT_PRIVATE`;
those addresses show 0.02% each. One heap address took 22,610 of the futex samples, from every
client thread. At 1 client: 308k ops/s and 48 voluntary switches a second.

Source: every read goes `Groups::read_committed` -> `Store::read_committed` -> `Readers::with`
(`crates/amphora/src/store.rs` 1152-1228) on the default checkout, `ReadCheckout::Pool`. Each read
takes the std `Mutex` `pool` twice (take, `give_back`), reads the std `RwLock` `gate`, and calls
`returned.notify_all()`. std's futex `Condvar` makes a `FUTEX_WAKE` syscall on every `notify_all`,
waiters or none.

A/B, one binary, `AMPHORA_READ_CHECKOUT=pool` against `slots` (the prototype already on main),
`point_pk` at 32 clients, alternating, 3 reps each, 0 errors in every run:

| checkout | ops/s (reps) | median | system share | voluntary switches/s | hot heap futex samples | p99 ms |
|---|---|---|---|---|---|---|
| pool (default) | 957,586 577,739 956,953 | 956,953 | 0.585 0.455 0.546 | 805,905 660,209 781,451 | 30,608 262,180 30,247 | 0.164 0.276 0.172 |
| slots | 1,660,041 1,677,008 1,535,603 | 1,660,041 | 0.143 0.135 0.190 | 427,106 381,198 643,297 | none | 0.260 0.264 0.224 |

Reading: the ceiling is the reader pool's std `Mutex` and `Condvar`. The sampler names it, and the
switch moves it: 1.73x at the median (slots' worst is above pool's best), with the kernel's share
falling from 46-59% to 14-19%. It is not in the storage engine (`crates/amphora`), so this lane did
not change it. Making `slots` the default is the decision (store.md's checkout decision). Left with
`slots`: 0.23-0.42 voluntary switches an op with no single hot address. That fits parking_lot waits
spread over per-thread addresses, plus the host's load. Sample it next. Not yet measured:
`point_secondary` and 128 clients under `slots`.

## AUDIT 2.1 header, the fair A/B (perf-B3): running, not finished in the hour

`.git/perf-ab5.sh` (rewritten): after = 4314cb0c's bench binary (`bench-hdr-after-4314`), base =
4314cb0c's tree with only ae729b06's `crates/` diff reversed, so main's later merges can't confound
it; 5 rounds, arm order alternated, load recorded; output `ab5-<HHMM>.jsonl`, written once the base
build finishes. Under load 90-136 the after build took 20m22s (run r211). r211's base build then
failed: `git archive` leaves out the untracked `bench/Cargo.lock`. Relaunched as run r266 with the
lock files copied in.

## perf-B4: the lock-free ways' "collapse past 4 threads" is the bitmap sending half the reads to the lock (AUDIT 2.1)

`PageBits::holds` said "ask under the lock" for a page past its words. The bench reads pages
1..2*pages; at 32,612 pages the bitmap's 512 words reach page 32,767, so 32,457 of the 32,612
absent pages (half of all reads) took the shared `RwLock` and the frame index's lock. At 32,768
pages the words double (1,024: to 65,535) and every read is answered lock-free. One binary (perf-B2's
release test binary, copied to `lookup-old-2b58`), only the page count changed, alternated, threads
pinned with `taskset -c 0..n-1`, kiln run r318 (needs cpus=16), records `b4-bits-0435.txt`, load
78-100 on 32 cores. Lookups a second, all threads, reps:

| threads | Published 32,612 | Published 32,768 | Optimistic 32,612 | Optimistic 32,768 | Locked (either) |
|---|---|---|---|---|---|
| 1 | 29.1 M, 28.9 M, 29.4 M | 36.6 M, 36.2 M, 33.2 M | 29.9 M, 30.9 M, 31.2 M | 39.0 M, 39.1 M, 37.5 M | 46-51 M |
| 2 | 38.0 M, 36.7 M, 38.2 M | 73.9 M, 72.3 M, 71.4 M | 39.8 M, 39.7 M, 37.6 M | 76.2 M, 75.3 M, 74.9 M | 15.7-17.0 M |
| 4 | 29.2 M, 29.7 M, 28.9 M | 136.6 M, 130.1 M, 128.5 M | 32.9 M, 34.9 M, 32.3 M | 92.3 M, 144.6 M, 145.3 M | 10.2-11.1 M |
| 8 | 18.3 M, 18.2 M, 18.1 M | 196.1 M, 220.0 M, 208.2 M | 17.9 M, 17.6 M, 17.6 M | 203.2 M, 273.7 M, 195.4 M | 7.0-7.1 M |
| 16 | 8.47 M, 8.37 M, 8.27 M | 407.8 M, 382.1 M, 382.9 M | 8.27 M, 8.24 M, 8.24 M | 370.1 M, 495.5 M, 388.4 M | 2.9-3.4 M |

Reading: with every read answered lock-free, both ways scale with the threads (27-42 ns a lookup a
thread from 1 to 16), 120-150x the lock at 16. So nothing a lookup writes is shared, and
`ArcSwap::load` is not the collapse. What remains against the lock is one reader: 27-29 ns against
20 ns, the three `ArcSwap` loads. The fix (4da616cf) answers a page past the words as absent; its
own A/B (this bench, its binary against `lookup-old-2b58`, 32,612 pages) is still to run, as are its
clippy and the release storage suite (run r335, queued at hand-over; r312/r314 died on a sync link error).

## perf-B5: the header's A/B not yet run; B4's fix checked; decision 45 written (AUDIT 2.1, P4's finding)

**Checks of 76a62c3d** (B4's 4da616cf and the header ae729b06, with origin/main merged; run r864, needs cpus=8):
`cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` exit 0, and `cargo test -p amphora-engine
--lib -- storage::wal storage::log_index` 105 passed, 0 failed, 1 ignored. That run includes
`audit_2_1_a_page_past_the_logs_highest_is_absent_without_the_lock`,
`g3_2_lock_free_page_reads_find_the_frame_the_locked_index_finds` and
`audit_2_1_default_header_reads_race_a_writer_and_stay_current`.

**The header's A/B (5b: it lands only with its A/B): not run.** The script is `.git/perf-ab6.sh` in the pagepath
clone. It builds after = 639245cc (perf/pagepath: the header, B4's fix, origin/main e56f44e7) and base = the same tree
with only ae729b06's `crates/` diff reversed (checked: `git apply -R --check` holds on that tree). Both are built one
after the other in the lane's target dir. It then runs 5 alternated rounds of `point_pk` and `point_secondary` at 1,
32 and 128 clients, with one shared `XDG_CACHE_HOME` (5e), recording load and CPU and I/O pressure before and after
each run, and errors and rows per op. Run r836's first build of 76a62c3d failed in `amphora`: main's `bench/Cargo.toml`
did not patch `hyper-rt`, which the postgres listener (f8f65110) uses, so cargo took crates.io's. Main has patched it
since, and 639245cc has that patch. After that, adacachy2 refused the run for room five times between 14:25 and
14:45Z, with 0-7 of 32 cores idle. Run r902 (placed 14:46:39Z) built both arms: after in 1m44s, base in 1m25s,
with the revert's diff stat 3 insertions and 72 deletions in `log_index.rs` and `wal.rs`. The binaries are on
adacachy2 at `~/.cache/amphora-perf-bin/pagepath/b5-after-639245cc` and `b5-base-639245cc`. No number came out: every
timed run stopped at `--reuse: no database`, because the data folder was new. The timed half alone is
`.git/perf-ab6-timed.sh`: it loads the suite's database once without `--reuse`, then runs the 10 alternated runs. At
14:53Z it was refused for room (11 of 32 cores idle). Run it with needs `linux/amd64,cpus=16,mem=24g`, `on` adacachy2,
sync false, and retry until it is placed.

**B4's fix alone on main.** 3a30e5f1 (branch `perf/pagepath-b4-pick`) is 4da616cf cherry-picked onto origin/main
9a7b14bb, clean. On that tree (run r901) clippy exited 0 and `storage::wal storage::log_index` passed 104, failed 0,
ignored 1. It is on main as e956abf0, pushed at 14:50Z, with the checks in its message. Its own binary A/B
(the lookup bench, this binary against perf-B2's `lookup-old-2b58`) is still to run.

**Decision 45** (docs/design/store.md, commit 4d0ead88) is the two-segment log's design. Nothing is built yet.

## perf-B6 (2026-11-23, 15:13-16:13Z)

**The header's A/B, timed: the lock-free header regresses at one client; not landed.** Run r950 (adacachy2, needs
`linux/amd64,cpus=12,mem=24g`, placed 15:21Z with 25 of 32 cores idle) ran `.git/perf-ab6-cal.sh` in the pagepath
clone: perf-ab6-timed.sh on r902's arms (after = 639245cc, base = 639245cc with ae729b06's `crates/` diff reversed),
plus RULES 5e. The after arm loaded the suite's database (200,000 items) and measured its calibration; the base arm
ran once to make its own file; the after arm's body was then written under both keys (sha256 3a6055409e46d1b2 in
both before and after the timed runs). Then 5 rounds, arm order alternated, `point_pk` and `point_secondary` at 1,
32 and 128 clients, 5 s each after 1 s of warm-up, `--reuse`. Errors 0 in every run, rows per op 1.0. Records:
adacachy2 `~/.cache/amphora-perf-bin/pagepath/ab6-cal-1521.jsonl`.

Ops/s, median of 5 [range], after against base:

| op | clients | after | base | after/base |
|---|---|---|---|---|
| point_pk | 1 | 205,977 [194,875-255,343] | 243,986 [208,228-319,526] | 0.84 |
| point_pk | 32 | 809,217 [445,461-890,909] | 708,804 [563,967-924,958] | 1.14 |
| point_pk | 128 | 561,634 [312,712-632,921] | 524,428 [394,636-794,965] | 1.07 |
| point_secondary | 1 | 167,428 [131,383-204,039] | 190,610 [181,014-270,593] | 0.88 |
| point_secondary | 32 | 531,572 [277,713-606,997] | 534,746 [420,924-627,768] | 0.99 |
| point_secondary | 128 | 289,239 [240,622-335,637] | 345,481 [262,280-419,937] | 0.84 |

p99.9 at one client, per round (after / base, ms): `point_pk` 0.32/0.568, 0.048/0.013, 0.6/0.007, 0.162/0.008,
0.296/0.182 (after worse in 4 of 5); `point_secondary` 0.428/0.022, 0.32/0.061, 0.512/0.01, 0.088/0.009, 0.728/0.085
(after worse in 5 of 5, by 4x to 50x). At one client the after arm's throughput is below base's in all 5 rounds for
both queries. At 32 and 128 clients the rounds disagree, and they follow the host more than the arm: load was 55-138
on 32 cores and CPU pressure (some, avg10) 53-63%, except while base ran rounds 3 and 4, when it fell to 20-26% and
base's 128-client p99.9 fell to 1.8-2.1 ms against 13-41 ms in every other run. The one-client regression is beyond
the run-to-run noise; its cause is not found (no profile was taken: a guess is the seqlock's validation or its
fallback under the lock, unmeasured). Per the brief, the header (ae729b06) stays on perf/pagepath and the design
merges alone.

**Decision 45, revised.** Ada (15:11Z): the two segments replace the one log outright, with no upgrade step, no
reading of a one-segment format and no switch between the two, and the one-segment restart deleted. The doc now says
so: each segment's header is today's log header, so a log with no valid `-wal2` is a state of the two-segment log;
bytes 18 and 19 carry 3 (wal2's value) so SQLite without wal2 refuses the file rather than read `-wal` without
`-wal2`'s commits. Found while checking the brief's bounded-log test against the shape: every read counted itself on
the closing segment whole, so reads that overlap with no gap would never let its count reach zero, and the writer
could never switch again. A read begun once the closing segment is copied into a flushed file now takes the current
segment alone and reads the rest from the file. The reset's events are three: the last read's give-back, the copy
with the file's flush, and a backed-up database's ship.

**Decision 45's first unit: the segments' state machine, on branch `perf/pagepath-d45` (cdf48fe9), not merged.**
`crates/engine/storage/log_segments.rs`: always one current segment, the other free or closing (`Segments`, so two
current segments can't be written down). `switch` goes only into a free segment and returns None, never waits, while
the other closes. `begin_read` counts on the current segment, and on the closing one only until it is copied;
`end_read` takes the read's span by value (not `Copy`, so a read can't be given back twice). `end_read`, `copied` and
`shipped` each return the segment they reset, when theirs was the last of its three events. `recover` takes the
segment whose checkpoint sequence is one past the other's as current, wrap-aware, and refuses a pair no switch leaves.
Run r1061 (adacachy2, needs cpus=8): `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` exit 0;
`cargo test -p amphora-engine --lib -- storage::log_segments` 7 passed, 0 failed. Nothing calls it yet. The next
units, each checked by the storage suite before the next: the log's per-generation state (`WalSharedMetadata`'s
header, frames and checksums; `WalSharedRuntime`'s file, frame cache, read marks, records, changes, read index and
durable mark) made one per segment, with `Segments` beside them and every path on the current one; recovery and
`database.rs` reading `-wal2` through `Segments::recover`; reads looking up by `ReadSpan::lookup_order`; the
checkpointer copying the closing segment first and calling `copied` after the file's flush; the switch in place of
`restart_wal_header`, with `begin_restart`, `end_restart`, `try_restart_log_for_write`'s restart and the store's
`restart_if_asked` gate and `Slots::take_all` deleted; the backup's ship calling `shipped`. Then the engine- and
store-level tests the doc names, the crash tests, and the slow-reader A/B (brief unit 3, not started).

## perf-B7 (2026-11-23, 15:49-16:44Z)

**Losses first: none of the four units is built.** Unit 1 (the segments wired into the log) is not started in code,
so units 2-4 (the clean close and the mark, the tests, the slow-reader A/B) are not either. No A/B ran, so this
section has no numbers. The hour went to the coordinator's rules in decision 45, the checks' base, and sizing unit 1.

**Decision 45, what plain SQLite sees: on main as 1df94b59** (0ac0d8d0, doc only). Amphora opens SQLite's WAL-mode
files as the one-segment state. An open or crashed Amphora database carries 3 in bytes 18 and 19 from its first
commit on, in the file and in every later frame of page 1, so plain SQLite refuses it (it reads page 1 through `-wal`
whenever that file exists, so the mark has to be in both). A clean close folds both segments in, syncs the file,
truncates both segments and syncs them, and only then writes 2 in place and syncs again. A close that cannot fold leaves 3. The crash
test now covers each step of the close in place of the deleted upgrade, and a new test has SQLite itself read a
cleanly closed file and refuse an open one. The engine's workspace has no SQLite for that test: `rusqlite` (bundled)
is only in `bench/`.

**Unit 1, sized on d4df9ba7** (perf/pagepath-d45: B6's state machine with origin/main merged).
- In `crates/engine/storage/wal.rs` (14,048 lines), the per-generation fields B6's list makes one per segment have
  about 650 uses: `max_frame` 179, `last_checksum` 79, `nbackfills` 69, `wal_header` 65, `transaction_count` 45,
  `read_locks` 37, `frame_cache` 33, `records` 22, `read_index` 22, `epoch` 18, `min_frame` 17,
  `frame_cache_high_water` 14, `changes` 10, `runtime.file` 8, `overflow_fallback_coverage` 6, `durable` 3. The
  shared state they live in, `self.shared` (`Arc<RwLock<WalFileShared>>`), has 56.
- A smaller route, not built and not decided (the doc's "each becomes one per segment" allows it): today's
  `WalFileShared` is the segment. Two of them, one per file, under a log that holds `Segments` and the few log-wide
  fields (`write_lock` 3 uses, `checkpoint_lock` 4, `vacuum_lock` 3, `released` 12). Each segment keeps today's
  one-log machinery whole: header, file, frame index, read marks, read index, records, changes, backfill. The routing
  sits at the 56 `self.shared` sites: writes on the current segment, reads by `ReadSpan::lookup_order`, the
  checkpointer on the closing segment and then the current one. A switch starts the free segment with
  `restart_wal_header`'s logic applied to that segment: no read holds one of its marks, because it is `Free`, so it
  takes them at once and waits for nothing. Unchecked risk: the pager's page cache forgets pages by the one log's
  generation and its change log (decision 18), and with two segments a snapshot's identity is the pair.
- Frame numbers must stay per segment. Read marks hold the frame as a u32 (`PaddedLock::get_value`;
  `try_begin_read_tx` asserts `max_frame <= u32::MAX`), so one number space across both segments would overflow them
  after 2^32 frames.
- Two restarts go, not one. The store's (`Readers::restart_if_asked`, store.rs 1168, called at 1736 and 1748 with
  `Db::restart_log`, db.rs 513) takes the gate for writing and every slot, so it waits for every read. The engine's
  own at the writer's next write (`try_restart_log_for_write`, wal.rs 1759) never waits, but it is skipped while any
  read holds a mark, so under overlapping reads it never happens. The switch replaces both. `begin_restart` and
  `end_restart` have two implementations each (wal.rs 1731 and 2953).
- B6's list deletes `Slots::take_all` with the restart. Only the restart's call can go: the store's `release`
  (store.rs, before `restart_if_asked`) takes every slot at shutdown too.
- The clean close starts at `Connection::close` (connection.rs 3424) and `Pager::checkpoint_shutdown` (pager.rs
  6421): a TRUNCATE checkpoint today, given up silently after three `Busy` answers. A backed-up writer turns it off
  (`Db::hold_log`, db.rs 564).
- Unit 4's slow reader exists as a test, not as a bench: `cp_t1_13_a_sql_read_to_a_client_that_stops_reading_ends_at_its_deadline`
  (`crates/amphora/src/serve.rs` on main) streams a read with `Spill::Sent` to the client end of `link()`, which never
  reads, until its deadline. The A/B's harness can hold one such read open, start it again each time it ends, and
  time the writers' commits beside it.

**Checks.** Run r1146 (adacachy2, needs `linux/amd64,cpus=4,mem=16g`; refused for room three times at 15:52-16:02Z
with 1-3 of 32 cores idle, placed at 16:04Z; load 109-114 while it ran) on d4df9ba7, whose `crates/` differ from main
only by `log_segments.rs` and its `mod` line. `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings`
exit 0 (37 s, the lane's target dir warm). `cargo test -p amphora-engine --lib --locked -- storage::` 694 passed,
3 failed, 14 ignored: `storage::btree::tests::wal_reuses_freelist_leaf_after_abandoned_overflowing_insert`
(btree.rs 11226), `storage::versioned::columns_sql::tests::cb4_a_read_only_transaction_reads_the_copy_at_its_snapshot`
(columns_sql.rs 1263) and `storage::versioned::columns_sql::tests::ab::cb4_ab_both_rebuy_policies_at_the_extremes`
(columns_sql.rs 1875). `cargo test -p amphora --locked --test store` 15 passed. Nothing calls `log_segments`, so the
three failures are not this change's. Run r1184 (adacachy2, needs `linux/amd64,cpus=4,mem=16g`, load 22-23) ran
the three alone on main 3f7624a3, one at a time: all three fail there, so they are main's own. The btree test panics
"expected a row, got Yield"; 29cd28f4 ("The integrity check yields at each read from the file", T-T1.12.C8) is a
lead, not checked. `cb4_a_…` panics "the copy didn't answer" (`unserved: 1`); `cb4_ab_…` reports 64 statements
"column copy refused: no generation answers the statement's snapshot". The `storage::`
filter is wider than B6's (`storage::wal storage::log_index`): it takes btree's and versioned's tests too. Run r1180
(macOS, needs `macos/arm64,cpus=8,mem=24g`, rule 6's disk guard; cold) has the same clippy and storage tests on
perf/pagepath-d45 as synced when it is placed (478716f8: main 3f7624a3 with `log_segments`); queued at 16:16Z
and stopped by the lane at 16:26Z before it started: no Mac had room, and a cold build would not have ended within the
hour. The Mac target dir for this lane is still cold.

## perf-B8 (2026-11-23, 16:46-17:41Z)

**Losses first: no code, so no numbers.** None of the brief's four deliverables is built. That covers the rerouting (unit 1), the
switch and per-segment reads with their four tests, the clean close and the SQLite mark, and the slow-reader A/B. The
hour went to the check the brief put first (the pager's page cache), a map of the 56 `self.shared` sites by role, and a
baseline of the merged tree. The rerouting changes the type every open path and every WAL test holds
(`Arc<RwLock<WalFileShared>>`), so it could not be built and checked inside the hour, and a half-built one was not pushed.

**The page cache, checked. Decision 45's "Where it lands" now says so.**
- The shared pages' key is the risk. `pager.rs` 4787-4790 keys a version as `Version::Frame { generation:
  wal.connection_wal_pos().0, frame }`, the connection's generation, which is the current segment's. With frames
  numbered per segment, frame n of the closing segment and frame n of the current one would share a key, and a read
  could be handed the other segment's page. So `find_frame` has to answer with its frame's segment, and the key takes
  that segment's checkpoint sequence. Unit 1 alone (no switch) is not exposed. Unit 2 is.
- Decision 18's forget rule needs no change (`read_change`, `wal.rs` 5114-5136). It forgets everything when the
  checkpoint sequence differs, and a switch changes the current segment's sequence, so a connection whose last read was
  in the other generation clears its cache, as after a restart today. The cost is one full clear per connection per
  switch, the same as one per restart today.
- A packed delta chains only from a version of the same generation (`chains_from`, `wal.rs` 4430-4436, called by
  both of the commit's paths), so none chains across a switch.

**The 56 `self.shared` sites by role** (`wal.rs` on fbdca171; `python3 -I ~/.cache/amphora-perf/pagepath-b8/sites.py
crates/engine/storage/wal.rs` lists them with their functions).
- Log-wide, to the log (7): the write lock 1410 and 1414, the checkpoint lock 1420 and 1424, `lock_release` 1450,
  and `shared_wal_state` 2063 and 3028, through which `WalFile` takes the vacuum lock (5042, 6095, 6866).
- Read marks, one set per segment (10): the eight helpers 1350-1404, `try_begin_read_tx` 1537 and the shm one 2708.
- Reads by `ReadSpan::lookup_order` (2): `find_frame` 1483 and `find_frame_locked` 1331.
- The writer's current segment (18): `load_snapshot` 1432; `publish_commit` 1436, 2596 and 2618; `wal_header` 1803;
  `wal_file` 1807; `wal_is_initialized` 1819 and 3046; `prepare_wal_header` 1827, where the switch writes the free
  segment's header; `mark_initialized` 1861; `cache_frame` 1869; `cache_record` 1930; `latest_frame` 1987 and
  `page_frames` 1995 (the packed chains, which stay in one generation); `rollback_cache` 2008; `changed_pages` 2044;
  `durable_through` 1779; `prepare_truncate` 1786.
- The checkpointer, closing segment first (6): `publish_backfill` 1454 and 2627, `iter_latest_frames` 1501,
  `checkpoint_epoch` 1517, and `bump_checkpoint_epoch` 1521 and 2695.
- Deleted with the restart (1): `begin_restart` 1741.
- The shm backend's sync with another process's authority (10): 2098-2512.
- Tests only (2): `shared_ptr` 2059 and 3121.

**The handle's type, everywhere it goes** (what unit 1 changes in one pass): `database.rs` 758 (the field), 868, the
open paths 2372-2554 and 3028; recovery's `BuildSharedWal` (`sqlite3_ondisk.rs` 1555-1774); in `wal.rs`, the two
backends' constructors (1302, 1317, 2137), `WalFile::new` and `new_with_shared_coordination` (6765-6790),
`OpenSharedWal`, `open_shared_if_exists` (7987, 7999), `new_noop` (8025), `new_shared` (8073), `VacuumLockGuard`, and
the tests' `make_test_wal_from_shared` (8764); the tests in `pager.rs` (7665, 7855) and `btree.rs` (13363).

**Open: the multi-process log.** The shm backend (`ShmWalCoordination`, `wal.rs` 2080) is compiled on every 64-bit
Unix and Windows target (`build.rs` 9). It is chosen only by `DatabaseOpts::with_multiprocess_wal(true)`, and only the
engine's own tests set that (`multiprocess_tests.rs` 132, `database.rs` 5026 and 5054, `pager.rs` 8308, `wal.rs`
8286). Its authority keeps one log header in shared memory. Decision 45 doesn't say what it does with two segments.
The choice is the coordinator's: the authority carries both headers and `Segments`, or a multi-process open is refused
with a typed error.

**Checks.** Run r1233 (adacachy2, needs `linux/amd64,cpus=4,mem=16g`, placed at once with 23 of 32 cores idle, load
18-27) on fbdca171: perf/pagepath-d45 (478716f8) with origin/main 5e6c619f merged. Main changed nothing in
`crates/engine/storage`, `store.rs` or `db.rs` since 3f7624a3. `cargo test -p amphora-engine --lib --locked --
storage::` passed 694, failed 3 and ignored 14 in 194 s. The three are main's own: btree's
`wal_reuses_freelist_leaf_after_abandoned_overflowing_insert`, `cb4_a_read_only_transaction_reads_the_copy_at_its_snapshot`
and `cb4_ab_both_rebuy_policies_at_the_extremes`. `cargo test -p amphora --locked --test store` passed 15. No new
failure. This lane changed docs only: decision 45's list and this record.

## perf-B9 (2026-11-23, 17:20-18:20Z)

**Losses first: step 2 (the switch) is not built, and step 1 routes three roles to the current segment only.** No
writer moves to a free segment yet. Reads are not counted per segment, `find_frame` does not carry its segment, neither
restart is deleted, and none of the five step-2 tests exists. Of B8's roles, three get their two-segment behaviour only
with the switch, because before it there is never a closing segment to act on: the lookups (2 sites) search the current
segment alone, the checkpointer (6) copies the current segment alone, and the multi-process authority (10) still
carries one header. No A/B ran. Step 1 changes no behaviour, and decision 45's slow-reader A/B needs step 2.

**Step 1: the log handle is `SharedLog`** (perf/pagepath-d45 0fbf0de8, 4871ad86, 779b6f0c; merged to main with
origin/main as below).
- `SharedLog` (`crates/engine/storage/wal.rs`) holds two of today's `WalFileShared`: `first` for `-wal`, `second` for
  `-wal2`. It also holds B6's `Segments` under a spin lock and the four log-wide locks, moved out of `WalSharedRuntime`
  (`write_lock`, `checkpoint_lock`, `vacuum_lock`, `released`). The second segment is free and empty (`new_noop`): a
  log with no valid `-wal2` is that state (decision 45, on disk).
- `current()` reads a published flag (`second_current`), not the spin lock. Every access to the current segment's state
  then costs a plain load and writes no shared cache line. The flag is to be written only with `Segments`, by the
  switch.
- Every `Arc<RwLock<WalFileShared>>` that stood for the log is now `Arc<SharedLog>`: `Database::shared_wal`, both
  coordination backends, `WalFile::new` and `new_with_shared_coordination`, `VacuumLockGuard`, `shared_wal_state` and
  the tests' `new_shared`. Recovery stays per segment (`BuildSharedWal`, `open_shared_if_exists`,
  `open_shared_from_authority_if_exists` and `new_noop` each build one segment), and `Database` puts the recovered
  segment in a log (`SharedLog::new`).
- B8's 56 sites, by role. The log-wide locks (7) now take the log's locks without any segment's lock. The read marks
  (10), the lookups (2), the writer's sites (18), the checkpointer's (6), the restart (1) and the shm sync (10) are on
  `current()`. The two test-only sites return the log. `database.rs`, `connection.rs`, the pager's, btree's, mvcc's and
  multiprocess tests reach a segment through `shared_wal.current()`.
- A script did the rerouting: `~/.cache/amphora-perf/pagepath-b9/d45_step1.py`, then `d45_step1_tests.py`, run from
  the clone's root on f2c8a85c. It fails on any site it does not find exactly as expected.

**Checks.** Run r1297 (adacachy2, needs `linux/amd64,cpus=4,mem=16g`, load 18-32) on 779b6f0c: step 1 with
origin/main 93b6458c merged. `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` exit 0.
`cargo test -p amphora-engine --lib --locked -- storage::` passed 694, failed 3 and ignored 14 in 204 s. The three are
main's own, as in r1233: btree's `wal_reuses_freelist_leaf_after_abandoned_overflowing_insert`, `cb4_a_…` and
`cb4_ab_…`. `cargo test -p amphora --locked --test store` passed 15. That is r1233's baseline exactly. Run r1279 (on
0fbf0de8) and r1288 (on 4871ad86) found the compile errors that 4871ad86 and 779b6f0c fixed.
Run r1309 (same needs, load 22-28) on b7d2cd79, after main a7160678 was merged (it changed btree.rs): clippy exit 0;
storage passed 695, failed 2 and ignored 14 in 204 s. Main's btree change fixed the freelist test, and the two left are
`cb4_a_…` and `cb4_ab_…`, main's own. Main then moved to 0d87fcdd in files this change does not touch (replicated.rs,
hyper-raft), merged without a re-run (RULES 5).

**Step 2, what is left, in order.**
1. The switch on `SharedLog`, under the write lock: `Segments::switch`, then `second_current`, then the free segment's
   header (`restart_wal_header`'s salts, the checkpoint sequence one past the current one's) written and synced
   before any frame. The second segment needs its file: `database.rs` opens `-wal2` beside `-wal`.
2. A read's span. `WalSnapshot` is `Copy` and `ReadSpan` is not, so the connection state keeps the span, and
   `ReadGuardKind::ReadMark` names its segment so that a read mark is given back on the segment that holds it.
3. The in-process backend's `read_index` is taken from the current segment when the backend is built. Decision 40's
   index is per segment, so it must follow the segment a lookup searches.
4. `find_frame` answers with its segment, and the pager's `Version::Frame` key takes that segment's checkpoint sequence
   (B8's cache risk).
5. The checkpointer copies the closing segment, then the current one, and calls `copied` after the file's flush.
6. Recovery reads both files and orders them with `Segments::recover`. The shm authority carries both headers.
7. `begin_restart`, `end_restart`, `try_restart_log_for_write`'s restart, the store's `restart_if_asked` gate and its
   call of `Slots::take_all` are deleted.
8. The five tests in the brief.

## perf-B10 (2026-11-23, 18:11-19:10Z)

**Losses first: the switch is not built, so the slow-reader A/B has no numbers.** Of B9's eight step-2 items one is
done: item 3, a lookup searches the lock-free index of its segment. The writer still never switches. Reads keep no
span, `find_frame` does not answer with its segment, the checkpointer and recovery act on one segment, neither restart
is deleted, and none of the five tests exists. The slow-reader A/B needs the switch and did not run.

**RULES 5b's point-read A/B of step 1: no measurable difference.** after = main a3642e93 (step 1), base = its other
parent 0d87fcdd. Run r1404 (adacachy2, needs `linux/amd64,cpus=16,mem=24g`) ran
`~/.cache/amphora-perf/pagepath-b10/ab10-point.sh`: both arms built in the lane's target dir one after the other
(release, 1 min 16 s and 1 min 12 s; the bench workspace has no lock file, so no `--locked`, as in B6's build), then
RULES 5e: the after arm loaded the suite's database (200,000 items) and measured its calibration, the base arm ran
once to make its own file, and the after arm's body was written under both keys (sha256 bf32a3a2c0fc1a3a under both,
before and after the timed runs). 5 rounds, arm order alternated, `point_pk` and `point_secondary` at 1, 32 and 128
clients, 5 s each after 1 s of warm-up, `--reuse`. Errors 0 in every run, rows per op 1.0. Records: adacachy2
`~/.cache/amphora-perf-bin/pagepath/ab10-point-1844.jsonl`.

Ops/s, median of 5 [range], after against base:

| op | clients | after | base | after/base |
|---|---|---|---|---|
| point_pk | 1 | 309,360 [289,560-345,211] | 312,065 [234,853-332,458] | 0.99 |
| point_pk | 32 | 895,238 [821,563-925,904] | 913,247 [404,866-971,470] | 0.98 |
| point_pk | 128 | 775,065 [751,062-792,204] | 758,801 [378,465-838,439] | 1.02 |
| point_secondary | 1 | 259,576 [214,477-269,074] | 242,200 [217,028-260,871] | 1.07 |
| point_secondary | 32 | 645,361 [599,164-707,642] | 674,087 [648,965-848,525] | 0.96 |
| point_secondary | 128 | 376,260 [308,387-458,261] | 363,879 [314,469-439,039] | 1.03 |

Every median is inside the other arm's range. p99.9 at one client (ms, after / base, per round): `point_pk`
0.007/0.035, 0.014/0.021, 0.016/0.011, 0.01/0.009, 0.007/0.007; `point_secondary` 0.023/0.022, 0.013/0.023,
0.011/0.011, 0.009/0.013, 0.011/0.011. The host was busy: load 18-82 on 32 cores, CPU pressure (some, avg10) 1-34%, I/O
pressure 4-45%; base's two low rounds at 32 and 128 clients (404,866 and 378,465) came at load 45-81. No profile was
taken. The change is one acquire load of a flag written only by a switch, against a point read of 2-3 us at one
client (p50 0.002-0.003 ms), so no measurable difference is the expected result; the limiter of the read itself is
not named here.

**Item 3, built and checked** (perf/pagepath-d45 f1351a6c, be348834, then bae557bb, rustfmt only). The in-process
backend took the current segment's `ReadIndex` once, when it was built, so after a switch it would search the other
segment's index. `SharedLog` now holds both segments' indexes outside their locks (`read_index(SegmentId)`), and
`find_frame`, `find_frame_locked` and the header's lock-free copy take the segment they search (`current_id()`, the
published flag). The backend keeps only whether it may go lock-free (`lock_free`, false for the shared-memory backend's
fallback as before). Each index's header is now set where the log is made: the header `Arc` is replaced only by the
restart and the restore, and both set it again there. No behaviour change, since the searched segment is always the
current one until the switch. A lookup now loads the flag once more than before; the A/B above measures step 1's
load, not this one.

**Two rules decision 45 does not state** (found reading the read and checkpoint paths for the switch; the coordinator
decides the first).
1. *The checkpointer must not copy the current segment while a read begun before the switch is under way.* Such a
   read searches the closing segment up to its mark and then the file. It never searches the current segment. If its
   mark is the closing segment's last frame, the closing segment's passive copy goes whole while it reads (the copy
   stops at the oldest mark, which is the end). If the checkpointer then copies the current segment, a page written
   only after the switch reaches the file, and the read finds it there: a version newer than its snapshot. One log has
   no such case, because a read mark bounds every backfill. Two ways to hold it: (a) copy the current segment only
   once the closing one is reset, which needs no new state and delays that copy by at most 2D plus one copy after a
   switch, inside the bound the decision already states; (b) count the reads begun before the switch in `Closing`
   and copy the current segment once that count is zero, which is sooner and costs one more counter.
2. *A read may take read mark 0 (the file alone, `ReadGuardKind::DbFile`) only when its span has no closing segment.*
   Right after a switch the current segment is empty, so `max_frame == nbackfills` (0 and 0), and today's test sends
   the read to the file alone, which lacks the closing segment's uncopied frames.

**Step 2 as units that each land alone** (behaviour unchanged until the last).
1. `find_frame` answers with a `FrameRef { segment, frame }`; `read_frame`, `read_frames_batch` and `read_frame_raw`
   take it; the pager's `Version::Frame` key takes that segment's checkpoint sequence (item 4). The `Wal` trait's
   `find_frame` has four production callers (pager.rs 4160, 4320, 4785; vacuum.rs 1527; database.rs 3566 calls the
   coordination's); a script like B9's moves the tests.
2. A read's span kept in the connection state beside its `WalSnapshot`, `ReadGuardKind::ReadMark` naming its segment,
   `begin_read` and `end_read` per read (item 2). This puts `Segments`' spin lock on every read transaction, so it
   lands with a point-read A/B at 1, 32 and 128 clients.
3. `-wal2` opened beside `-wal`, both recovered through `Segments::recover`, the shared-memory authority carrying both
   headers (item 6 and item 1's file).
4. The checkpointer: the closing segment first, `copied` after the file's flush, and the two rules above (item 5).
5. The switch, both restarts deleted, the five tests (items 1, 7 and 8). Then the slow-reader A/B.

**Checks.** Run r1344 (adacachy2, needs `linux/amd64,cpus=4,mem=16g`, load 22-31) on abb69a82, perf/pagepath-d45 with
origin/main e7d64bbb merged: `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` exit 0. Run r1380 on
f1351a6c found two test calls of `find_frame_locked` without the new argument (fixed in be348834). Run r1401 (same
needs, load 12-84) on be348834: clippy exit 0; `cargo test -p amphora-engine --lib --locked -- storage::
--test-threads=4` passed 695, failed 2 and ignored 14 in 221 s, the two main's own (`cb4_a_…` and `cb4_ab_…`), which
is the brief's baseline exactly; `cargo test -p amphora --locked --test store` passed 15. `cargo fmt --all --check` fails on
origin/main itself in seven files (69 places; wal.rs's 30 are step 1's rerouted chains, the rest not traced).

**Main's red clippy, fixed** (21850e25; the coordinator's word via lane K10). `cargo clippy --workspace --all-targets`
failed on main at wal.rs 8425: B9's step-1 script made `replace_after_external_restore_preserves_lock_identity`'s
restored state a `SharedLog`, which has no `read` or `into_inner`. The test is behind the `conn_raw_api` feature, which
`cargo clippy -p amphora-engine` leaves off and the workspace turns on, so B9's checks and this lane's first ones
missed it. The restored state is one segment's `WalFileShared` again. Step 1's lines in database.rs, lib.rs and wal.rs
are formatted. Run r1419 (same needs, load 54-57) on 1ae4d867 (this branch with origin/main d5e901f3 merged): `cargo
clippy --workspace --all-targets --locked -- -D warnings` exit 0; `cargo test -p amphora-engine --lib --locked
--features conn_raw_api -- replace_after_external_restore_preserves_lock_identity` passed 1. `cargo fmt --all --check`
then lists 35 places in five files: database.rs and wal.rs are clean, lib.rs has one left that is not step 1's export
line, and the rest are replicated.rs (12), amphora's database.rs (3) and the optimizer's two files (19), not traced.

## perf-B11 (2026-11-23, 19:04-20:04Z)

**Losses first: the hour failed its deliverable. The switch is not built.** Rule 0's deliverable was decision 45's switch
working end to end, all five units merged and the five tests passing. One unit of five is built and checked: unit 1.
Unit 2 (a read's span) is designed below but not written. Units 3, 4 and 5 are not started, neither restart is deleted,
none of the five tests exists, and no A/B ran (rule 0: none before the switch works).

**The branch.** The brief named `perf/pagepath`. That branch holds ae729b06 (the default path reads the log's header
lock-free), a choice Ada said to decide by data, which RULES 5b lets land only with its own A/B, and it is not on main.
Building on it would have merged it at this lane's first merge. This lane worked on `perf/pagepath-d45`, where steps 1 and
2 have been built since B9, which main contains.

**Unit 1: `find_frame` answers a `FrameRef`** (perf/pagepath-d45 416fe658). `Wal::find_frame` answers `FrameRef { segment,
frame }`. `read_frame` and `read_frames_batch` take one and read the file of its segment (`WalCoordination::segment_file`;
`current_segment` names the writer's). The pager's shared pages key a frame by `Wal::frame_generation`: the connection's
checkpoint sequence for the current segment, one before it for the closing one (a switch writes the current sequence plus
one, and the sequence wraps). Without it, frame n of each segment would share a key (lane B8's cache risk). VACUUM refuses a
target WAL whose pages it finds in two segments. `read_frame_raw` keeps its frame number, the current segment's. A script
moved the four production callers and the tests: `~/.cache/amphora-perf/pagepath-b11/d45_unit1.py`, run from the clone's
root on 9b8fa0fc. It fails on any site it does not find exactly as expected. No behaviour change: with no switch the segment
is always the current one, and its generation is the connection's checkpoint sequence, which the key used before.
Until unit 2, `frame_generation` and `find_frame` name the log's current segment at the lookup, not the read's. That is
the same segment only while no switch can happen, so unit 2 must give both the read's span.

**Decision 45's two rules are in the design doc** (store.md, The shape and Checkpoints). The checkpointer copies the current
segment only once the closing one is reset, which the coordinator chose from B10's two ways. A read takes read mark 0 only when
it counts on no closing segment. Neither is built: rule 2 belongs to unit 2, rule 1 to unit 4.

**Unit 2, the simplest correct span, for the successor** (not built).
- `WalFile` keeps the read's `ReadSpan` beside its connection state, in a field of its own (`WalConnectionState` is
  `Copy` and stored in atomics; `ReadSpan` is neither). The coordination gains `begin_read_span` and `end_read_span`, which
  take `SharedLog::segments`' spin lock (the in-process backend's `self.log`; the shared-memory backend delegates to its
  fallback), so `WalFile` never clones the log's `Arc` on a read.
- The span is taken inside the coordination's `try_begin_read_tx`, before the read mark is chosen, so that rule 2 can refuse
  read mark 0 when the span has a closing segment, and given back on every path that returns no guard.
- `ReadGuardKind::ReadMark` and `DbFile` name their segment. Today the guard is kept as a slot index in
  `max_frame_read_lock_index` (an `AtomicUsize`), and `unlock_read_mark` takes the current segment's lock: after a switch
  that would unlock a slot on the wrong segment. Encode the segment in that index, or keep the guard with the span.
- `find_frame` searches the span's segments in `lookup_order`, and `frame_generation` compares against the span's current
  segment, not `SharedLog::current_id`.
- The shared-memory backend counts only this process's reads in its fallback's `Segments`. Counting other processes' reads
  needs the authority (unit 3).

**Checks.** Run r1436 (adacachy2, needs `linux/amd64,cpus=8,mem=24g`, load 21-30) on main 4a62875e: `cargo clippy
--workspace --all-targets --locked -- -D warnings` exit 0 in 34 s. Run r1467 (same needs, load 26-36) on 416fe658: the
workspace clippy exit 0; `cargo test -p amphora-engine --lib --locked -- storage:: vdbe::vacuum --test-threads=8` passed 715,
failed 2 and ignored 14 in 205 s. The two are main's own (`cb4_a_…` and `cb4_ab_…`, as in B9's and B10's runs). `cargo test -p amphora --locked
--test store` passed 15. Run r1475 (same needs, load 25-54) on c5963de9, this branch with origin/main eb365f3d merged (main
touched none of this lane's files): the workspace clippy exit 0; the same engine tests passed 715, failed the same 2 and
ignored 14 in 203 s; the store test passed 15.

## perf-B12 (2026-11-23, 20:01-20:56Z)

**Losses first.** The brief's one unit is built and checked; nothing beyond it. The switch still does not exist, so
nothing in production can yet hold a guard on a segment that is not current: unit 2 changes no behaviour until
unit 5. No A/B ran (the brief: none). What unit 2 leaves for later, by the brief's scope:
- `find_frame` searches the read's current segment only. Searching the span's closing segment after it
  (`ReadSpan::lookup_order`) needs that segment's frame range in the read, and `find_frame`'s early return for a
  fully backfilled snapshot must then not skip the closing segment. Both belong with the switch.
- Every read transaction now takes `SharedLog::segments`' spin lock twice (begin and end), shared by every
  connection of the log. Not measured.
- The shared-memory backend counts only this process's reads in its fallback's `Segments`; the authority still
  holds one segment's frames (unit 3).

**Unit 2, built** (perf/pagepath-d45 89465d69, 62f5c1bc; merged with origin/main cf820fc5 as 55a805d0).
1. A read's span is kept in the connection state: the coordination's `try_begin_read_tx` takes it
   (`SharedLog::begin_read`) before it chooses a read mark and returns it with the guard, giving it back on every
   path that returns none; `WalFile` keeps it in `read_span` and `end_read_tx` gives it back with the guard.
2. `ReadGuardKind::DbFile` and `ReadMark` name their segment. The lock index `WalFile` stores keeps the bare slot for
   the first segment and sets the top bit for the second (bitwise only, so the ratchet's arithmetic count does not
   rise). `end_read_tx` unlocks the slot on that segment (`unlock_segment_read_mark`); the checkpointer's own
   read-mark 0 calls still act on the current segment (unit 4).
3. `find_frame` passes the read's segment to the coordination's `find_frame` (a new first argument) and
   `frame_generation` compares against it. With no read mark held (VACUUM's exclusive transaction) both use the
   current segment.
4. Rule 2: both backends take read mark 0 only when `span.closing()` is none.
The two read-mark bodies moved unchanged into `try_read_mark` on each backend, apart from the segment, rule 2 and
three `expect`s gone (`NonZeroUsize::new` mapped or `?`-ed).

New test: `storage::wal::test::d45_a_read_guard_unlocks_the_slot_of_the_segment_it_locked`. A read on the first
segment, a switch made by hand (`Segments::switch`, then `second_current`), two reads on the second (one on a fully
backfilled snapshot), then the first read ends. Its slot is free on the first segment, the second segment's slot is
still held, the backfilled read holds a read mark on the second segment (not read mark 0), and once all three end
both segments' counts are zero. Before this unit the first read's unlock went to the current (second) segment.

**Checks.** Run r1538 on 89465d69 found two test lookups (`exclusive_coordination`, `reopened_coordination`)
without the new argument (fixed in 62f5c1bc). Run r1540 (adacachy2, needs `linux/amd64,cpus=8,mem=24g`, load
32-27) on 62f5c1bc: `cargo clippy --workspace --all-targets --locked -- -D warnings` exit 0 in 37 s;
`cargo test -p amphora-engine --lib --locked -- d45_` passed 8; `cargo test -p amphora-engine --lib --locked -- storage:: vdbe::vacuum --test-threads=8` passed 716,
failed 2 and ignored 14 in 236 s: the two are main's own (`cb4_a_…` and `cb4_ab_…`), and 716 is main's 715 plus
the new test; `cargo test -p amphora --locked
--test store` passed 15. Run r1549 (same needs, load 13-17) on 55a805d0,
the merge with origin/main cf820fc5 (main touched no file under `storage/` or `vdbe/`): the workspace clippy exit 0
in 28 s; the `d45_`, coordination-guard and reader tests passed 16; the store test passed 15.

## perf-B14 (2026-11-23, 21:58-22:58Z)

**Losses first.** `scripts/check-production.sh` fails, and on main itself: `clippy::disallowed_methods` 41 against the 40 the
ratchet holds. Run r1663 counted 41 disallowed-method sites on this branch (853c72a6) and on origin/main (6a3e8ebb), the
per-file lists identical. The 41st is `.then_some(spread)` in `crates/engine/translate/optimizer/learned_costs.rs`, from
lane T13's 9ac9687b. This unit adds none. Not done by the brief's scope: the non-host build (no `host_shared_wal`: 32-bit,
wasm) still opens `-wal` alone, through its async driver, and recovers with a free second segment; a second driver
belongs with the switch. No A/B ran (the brief: none).

**Unit 3, built.** Reads are counted across processes in the reader slots (the coordinator's decision), not B13's packed
word:
- `SlotReads` (log_segments.rs): one word per reader slot, bits 0-31 the reads through it on the first segment, 32-63 on
  the second. One slot serves every read of a snapshot in its process, so it counts rather than flags them. A read past
  a count's 32 bits is refused and retried. The words sit in the `.tshm`'s fifth per-slot array, the one reserved for a
  future per-reader array, so the file's size is unchanged.
- Every path that frees a dead or unused slot zeroes its word before the slot's free bit is set (the two reclaims of a
  dead owner, the stale-slot probe, the inline reclaim in `live_reader_frames`, the exclusive-open repair), so a process
  that dies mid-read leaks no count. `MappedSharedWalCoordination::segments` reclaims dead slots, then sums the live words.
- The shared word keeps B13's four flags (which segment is current; the other closing, copied, shipped) without counts,
  and B13's other-segment header (format version 2).
- The shared-memory backend counts a read on its slot right after registering it and before the snapshot's re-check
  (`publish_shared_reader`), and takes it back at `end_read_tx` before the slot is released.
- At open, `-wal2` (`second_segment_path`) is recovered from its file if it exists, else free and empty: the writer's
  first switch makes the file, never an open (`open_second_segment_if_exists`, without `Create`). `SharedLog::recover`
  takes each valid header's checkpoint sequence to `Segments::recover`, and is Corrupt for a pair neither of which
  follows the other. `seed_or_sync_authority` publishes the recovered segments and the other segment's header when the
  authority is empty or repaired from this process's disk scan.

**Tests.** `d45_open_recovers_both_segments_of_the_log` (B13's: the `-wal` kept before a TRUNCATE checkpoint and the next
`-wal` as `-wal2`, its sequence one higher: the second current, the first closing and uncopied, each segment's last
commit frame its file's), `d45_two_mappings_of_one_file_see_each_others_reads`,
`d45_a_dead_readers_slot_is_reclaimed_and_its_read_stops_counting`, and in log_segments.rs
`d45_the_shared_word_holds_every_state` and `d45_a_slot_counts_each_segments_reads_and_refuses_one_past_its_bits`.

**Checks** (adacachy2, needs `linux/amd64,cpus=8,mem=24g`, script `~/.cache/amphora-perf/pagepath-b14/check.sh`). Run r1656
on 8d4f34c9 (origin/main b234421d merged): `cargo test -p amphora-engine --lib --locked -- d45_` passed 13;
`storage:: vdbe::vacuum` passed 722, failed 2, ignored 14 in 201 s, the two main's own (`cb4_a_read_only_transaction_reads_the_copy_at_its_snapshot`,
`cb4_ab_both_rebuy_policies_at_the_extremes`, as in B9-B12); `cargo test -p amphora --locked --test store` passed 15;
the workspace clippy failed on one lazy `then` closure and the ratchet counted one index more, both fixed in 853c72a6.
Run r1663 on 853c72a6: `cargo clippy --workspace --all-targets --locked -- -D warnings` exit 0; check-production as above.

## perf-B15 (2026-11-23, 22:58-23:58Z)

**Losses first: the hour failed its deliverable. Unit 4 is half built.** Built and checked: the brief's requirements 3 and 4
(a closing segment is reset only when no read of any process counts it; a read checks, after it is counted, that no reset
took a segment of its span). Not built: requirements 1 and 2. The checkpointer still copies only the current segment and
never the closing one, so nothing outside the tests calls the copy event (`MappedSharedWalCoordination::copied`, or
`Segments::copied` in one process). The brief's test "the current segment isn't copied while the closing one is unreset"
does not exist. The test of "a read counted during a reset" drives the shared memory (two mappings of one file), not a
query: a query-level one needs the checkpointer's copy. No A/B ran (the brief: none). The read path's added cost in the
shared-memory backend is not measured: one SeqCst fence and one load at each read's start, and one fence and one load at
each read's end.

**Why the checkpointer isn't built.** The checkpoint pass (`checkpoint_inner`, wal.rs) reaches the log through about 16
coordination calls that name the current segment today, each implemented by both backends. The in-process backend has 24
uses of `log.current()` or `current_id()`; the shared-memory backend has 90, its authority's calls included. Giving each
the pass's segment, with two check runs, did not fit the hour.

**Built** (perf/pagepath-d45 d0660c11).
- `MappedSharedWalCoordination` (shared_wal_coordination.rs):
  - `begin_span` takes a read's span from the shared word, with the word it was taken from.
  - `span_holds(word)` is asked once the read is counted: a SeqCst fence, then whether the word is unchanged.
  - `reset_if_unread` is asked at each reset event. It writes the word (`fetch_or(0)`), fences, sums every live slot's
    reads (dead slots reclaimed first), and resets by compare-and-swap if the closing segment is copied, shipped and
    read by nobody. A cheap check of the word comes first, so a read's end that can't reset costs a fence and a load.
  - `copied` publishes the copied flag, then asks `reset_if_unread`.
- `ShmWalCoordination` (wal.rs): `try_begin_read_tx` takes the span from `begin_span`. `publish_shared_reader` refuses
  the read when `span_holds` fails after its count, gives the count back and asks for the reset. `end_read_tx` gives the
  count back, asks for the reset, and unlocks the read mark of the segment it locked. The shared backend no longer counts
  reads in its process's `SharedLog::segments`: every process's reads are in the slots, and the local copy of the state
  could lag another process's reset, which would make a read retry with the same stale span for ever.
- `Segments::copied` (log_segments.rs) is now `mark_copied` then `reset_if_done` (public), so the shared memory can
  publish the flag before it counts the reads.

**Why the order holds (lane B14's race).** A read stores its count, fences, then loads the word. A reset stores the word,
fences, then loads the counts. With SeqCst fences on both sides, one of the two loads sees the other side's store. If the
reset sees the count, it leaves the segment. If the read sees the reset's store (the copied flag, or the reset itself),
the word differs from the span's, and the read starts over with a span from the new word. A span taken after the copied
flag never counts on the closing segment (decision 45, The shape). So a read proceeds on a closing segment only while
its count holds the reset back. The reset at a read's end does the same store, fence and loads (`fetch_or(0)` is its
store), so the same holds against a read whose count only that resetter could miss.

**Tests** (shared_wal_coordination.rs, two mappings of one `.tshm`, the switch made by hand as in B12):
- `d45_a_read_begun_before_the_switch_keeps_the_closing_segment`: the copy leaves the closing segment while a read in the
  other mapping counts on it; the read's end resets it; a second reset attempt does nothing.
- `d45_a_read_counted_during_a_reset_never_reads_the_reset_segment`: a span taken before the copy and reset, counted
  after, is refused, and the retry's span reaches only the current segment. In the other order, a read counted before
  the copy keeps the segment from its reset until it ends.

**Checks** (adacachy2, needs `linux/amd64,cpus=4,mem=12g`: refused at cpus=8 with 7 of 32 cores idle; script
`~/.cache/amphora-perf/pagepath-b15/check.sh`, B14's with the job count from `JOBS`). Run r1698 on main f91a9a92 (load
36-40): `cargo clippy --workspace --all-targets --locked -- -D warnings` exit 0 in 11 s. Run r1723 on d0660c11 (load
33-48): `cargo check -p amphora-engine --lib --tests` exit 0; `cargo test -p amphora-engine --lib --locked -- d45_` passed
15 (B14's 13 and the two above); the workspace clippy exit 0 in 35 s; `storage:: vdbe::vacuum --test-threads=4` passed
724, failed 2 and ignored 14 in 235 s, the two main's own (`cb4_a_read_only_transaction_reads_the_copy_at_its_snapshot`,
`cb4_ab_both_rebuy_policies_at_the_extremes`) and 724 being main's 722 and the two above; `cargo test -p amphora --locked
--test store` passed 15; `scripts/check-production.sh` exit 1, `clippy::disallowed_methods: 41, above the 40`, the count
main had then (T13's site in learned_costs.rs, which lane T14 has since fixed on main, 44515444). Run r1754 on 51cc395e
(d0660c11 with origin/main c75bb13f merged; load 24-51): `d45_` passed 15; the workspace clippy exit 0 in 61 s;
check-production exit 0 ("10908 left in the engine's production code across 17 lints, as the ratchet holds");
`storage:: vdbe::vacuum` passed 724, failed the same 2 and ignored 14 in 229 s. Main then moved again in `crates/`
(`storage/versioned`, `vdbe`, `amphora`, `pg-parser`, none of this lane's files): run r1767 on 4af592b6, the re-merge
(load 22-26): `d45_` passed 15; the workspace clippy exit 0 in 33 s; check-production exit 0, the ratchet holding. Main
moved once more (`amphora`'s store.rs and crash test, `pg-parser`): run r1774 on 940f1c8d (load 18-31): `d45_` passed
15; the workspace clippy exit 0 in 37 s; `cargo test -p amphora --locked --test store` passed 15. Run r1741 never
started (the kiln server timed out answering, then had no record of it).

**Rules 1 and 2, the design for the successor** (from `checkpoint_inner`, wal.rs, and the pager's phases, pager.rs).
- The pass's segment. `OngoingCheckpoint` gains `segment: SegmentId`, chosen once at `CheckpointState::Start`: the
  closing segment while there is one, else the current one (shared memory: `authority.segments()`; one process:
  `SharedLog::segments`). The current segment is copied only once no closing segment remains, which is after its reset
  (rule 1).
- The calls the pass makes that must take its segment: `load_coordination_snapshot` (max_frame, nbackfills),
  `determine_max_safe_checkpoint_frame` (only reads begun before the switch hold marks on the closing segment), the
  guard's `try_read_mark_exclusive(0)`, `ensure_local_frame_cache_covers`, `iter_latest_frames`, `durable_through` and
  `sync` (the closing segment's file), `wal_header`, `holds_record` and `issue_record_reads`,
  `issue_wal_read_into_buffer` (a `FrameRef` of the pass's segment), and `cache_get_for_checkpoint`, whose key is a
  frame and an epoch: skip it for the closing segment, or key it by `frame_generation`.
- The pager's `PublishBackfill` (after `SyncDbFile`, so the file is flushed first) publishes the closing segment's
  backfill. When that reaches the segment's last frame, it calls the copy event: `Segments::copied` under
  `SharedLog::segments` in one process, `MappedSharedWalCoordination::copied` across processes.
- A Restart or Truncate checkpoint whose pass copied the closing segment must not restart the log: the current segment
  is not copied yet. It answers Busy, or runs a second pass on the current segment once the reset is in.
- Shared memory: the authority holds the current segment's frames, not the closing one's. Until unit 5 there is no
  switch, so a closing segment exists only from recovery, and each process's own scan of that file is complete and the
  same: a closed segment is never appended to.
- Test (b), "the current segment isn't copied while the closing one is unreset", belongs with this part.

## perf-B16 (2026-11-23 23:59Z to 2026-11-24 00:55Z)

**Losses first.** Unit 4 is built and merged; what it leaves, by the brief's scope:
- The shared-memory backend holds a closing pass at every live reader's frame, whichever segment the frame is in: its
  reader slots don't say which segment a read's mark is on. That is safe (the pass copies less), but a read begun after
  the switch can hold the closing segment's copy back while reads overlap. Before unit 5 a closing segment comes only
  from recovery, where no read began before a switch.
- In the shared-memory backend a closing pass reads this process's own copy of the closing segment's backfill, so a
  process that didn't make the copy copies it again: the same pages, since the current segment isn't copied before the
  reset.
- Reads still look a page up in their current segment only (unit 2 left the closing segment's lookup to the switch). A
  page only the closing segment holds is read from the file, which lacks it until the closing segment's copy. The tests
  read such a page only after the copy.
- Under `synchronous=OFF` the pager publishes no backfill (it skips the file's sync and the publish), so a closing
  segment is never reported copied and the current segment is never copied. Unchanged.
- VACUUM's checkpoint (TRUNCATE, its lock held by the caller) answers Busy while a closing segment exists.
- A PASSIVE checkpoint's upper bound, in the current segment's frame numbers, bounds a closing pass too. That is safe
  (the pass can only copy less).
- No A/B (the brief: none). A pass now asks for the segments twice (at its start and once its locks are held); in the
  shared-memory backend each ask reclaims dead reader slots and sums every slot's reads.

**Built** (perf/pagepath-d45 06d01018, 886dbb54, 5d2e6023, 0ca4af89; merged with origin/main as e4434c95 and 3c075124).
1. *Segment choice.* A pass picks its segment once, in `CheckpointState::Start`: `Segments::to_copy` (log_segments.rs),
   the closing segment while one exists, else the current one, from `WalCoordination::segments` (the log's
   `Segments` in one process, `MappedSharedWalCoordination::segments` across processes). `OngoingCheckpoint` keeps it
   (`segment`, `closing`). Every call of the pass takes that segment: its snapshot (`segment_snapshot`), read mark 0 in
   the guard (`acquire_checkpoint_guard`, `CheckpointLocks` names its segment and a held guard is reused only on the
   same one), the safe frame (`determine_max_safe_checkpoint_frame`), the frames to copy (`iter_segment_frames`), the
   header (`segment_header`), the record reads (`holds_segment_record`, `issue_record_reads`, `begin_read_record` and
   `page_frames` take the segment), the frame reads (`issue_wal_read_into_buffer`, the segment's file) and the WAL's
   sync (the segment's file). A closing pass skips the page cache (`cache_get_for_checkpoint` keys a page by its frame
   in the current segment, and frame numbers start again in each), the shared memory's frame-cache fill (no process
   appends to a closing segment) and the durable backfill proof (the current segment's; a closing segment is copied
   again whole after a crash). The shared-memory backend answers the closing segment from its fallback, this
   process's scan of the file.
2. *Reporting the copy.* After the database file's sync, the pager's `PublishBackfill` publishes the backfill on the
   pass's segment (`publish_segment_backfill`). When it reaches the closing segment's last frame, it reports the copy
   (`closing_copied`: `Segments::copied` in one process, `MappedSharedWalCoordination::copied` across processes). A
   closing segment found already copied whole at a pass's start (or holding no frame) is reported there.
3. *No early restart.* A RESTART or TRUNCATE whose pass copied the closing segment restarts and truncates nothing; it
   publishes its backfill as a PASSIVE does, then answers Busy. Its caller asks again, and once the closing segment is
   reset the next pass copies the current one and restarts the log (the shutdown checkpoint asks up to four times).
4. *The pass's segment re-checked under its locks* (5d2e6023). Between a pass's choice and its locks, a read's end can
   reset the closing segment and another pass copy the current one; a closing pass after that would write older pages
   over newer ones in the file. With its locks held, a pass whose segment rule 1 no longer names answers Busy.

**Tests** (wal.rs, a real database whose log opens in two segments: the recovery test's copy, with a second table
whose page only the closing segment holds).
- `d45_the_current_segment_is_not_copied_while_the_closing_one_is_unreset`: with a read held, a PASSIVE copies the
  closing segment whole and reports it (closing, copied, one read); a second PASSIVE copies nothing and a TRUNCATE
  answers Busy, the current segment's backfill still 0; the read's end resets the closing segment, and only then does
  a PASSIVE copy the current segment. Every row is then read back.
- `d45_a_query_read_counted_during_a_checkpoints_reset_never_reads_the_reset_segment`: a query's read begun after the
  copy, while the reset waits on a held read, doesn't count on the closing segment (the held read's end resets it);
  with the reset segment's file then cut to nothing, the query reads the table only that segment had, from the file.
- `d45_a_checkpoint_copies_the_closing_segment_until_its_reset` (log_segments.rs): the segment a pass copies across a
  switch, a copy and a reset.

**Checks** (adacachy2, script `~/.cache/amphora-perf/pagepath-b16/check.sh`, B15's with its logs named b16; asks refused
at cpus=4, 2 and 1 with 0 of 32 cores idle at 00:18Z, then taken at cpus=4 and at cpus=3 when room came, each run held to
its ask by `JOBS`). Run r1790 on main 5cea6cde: `d45_` passed 15. Run r1825 on 06d01018 (load 64-101): `cargo check -p
amphora-engine --lib --tests` exit 0; `d45_` passed 18 (the 15 and the 3 above). Run r1846 on 5d2e6023 (origin/main
c6f915b1 merged; load 56-88): the workspace clippy exit 0 in 39 s; `d45_` passed 18; `storage:: vdbe::vacuum
--test-threads=4` passed 730, failed 2 and ignored 14 in 274 s, the two main's own (`cb4_a_read_only_transaction_reads_the_copy_at_its_snapshot`,
`cb4_ab_both_rebuy_policies_at_the_extremes`); 730 is B15's 724, lane R5's 2 and lane Q2's 1 (counted in the diffs, not run
on main this hour) and the 3 above; `cargo test -p amphora --locked --test store` passed 15; `scripts/check-production.sh`
exit 1: `clippy::disallowed_methods` 42 against 40 (this unit's two `then_some`), and 2 fewer `indexing_slicing`, 2 fewer
`cast_possible_truncation` and 1 fewer `disallowed_macros` than the ratchet held, all fixed in 0ca4af89. Run r1899 on
3c075124 (0ca4af89 with origin/main 2f29b288 merged, which moved only `crates/pg-parser`; load 95-139, at cpus=3):
check-production exit 0 ("10903 left in the engine's production code across 17 lints, as the ratchet holds"); the
workspace clippy exit 0 in 62 s; `d45_` passed 18. Main then moved in `crates/engine` (lane R6's column copy choice,
none of this lane's files): run r1919 on 7698a061, the re-merge with origin/main 50ab86c6 (load 70-111): check-production
exit 0, the ratchet holding; the workspace clippy exit 0 in 42 s; `d45_` passed 18. Merged to main as c044b33b (main had
moved in bench records only). After the merge, run r1928 on main c044b33b (needs refused at cpus=4, run at cpus=2 with
`--test-threads=2`; load 27-90): `storage:: vdbe::vacuum` passed 731, failed 1 and ignored 14 in 315 s, the one
`cb4_ab_both_rebuy_policies_at_the_extremes` (main's own; `cb4_a_read_only_transaction_reads_the_copy_at_its_snapshot`
passes on this main, after lane R6's columns_sql.rs change); `cargo test -p amphora --locked --test store` passed 15.

## perf-B17 (2026-11-24, 00:54-01:54Z)

**Losses first: the hour failed its deliverable. The switch is not built.** Of the brief's seven conditions, two are
built and checked: reads find their pages in the closing segment (condition 2) and the shared word counts its changes
(condition 4). Not built: the switch itself and the deletion of both restart paths (condition 1), the reader slots
recording which segment a read's mark is in (condition 3), and the five tests of decision 45 (condition 5). So a
reader that never finishes still stops the writer at a restart: the store's `restart_if_asked` still takes the gate
and every slot. No A/B ran (rule 0: none before the switch works). Lane Q3's segment word (bfa2d3f2) landed on main
mid-hour and was merged first, as the coordinator asked. Its in-process word has no free bit (30 + 30 read counts and
four flags) and needs no change count: a read's count and its span are one compare-and-swap there. The change count
went into the multi-process shared word, where B15's re-check compares whole words.

**Built.**
1. *A read finds its pages in the closing segment it counts on* (perf/pagepath-d45 554b248e). `WalFile` keeps the
   closing segment's last frame for the read under way (`closing_max_frame`), set with the span in `try_begin_read_tx`
   (that segment takes no frame after its switch) and cleared at the read's end. `find_frame` searches the read's own
   segment, then the closing one (`find_closing_frame`, frames 1 to that last frame), then the file. It searches the
   closing segment also when the read's own segment is wholly copied, which used to return at once. The
   shared-memory backend answers a closing segment from its process's scan of the file, as `iter_segment_frames`
   already did. Test `d45_a_read_finds_a_page_only_the_closing_segment_holds`: on B16's two-segment database, before
   any checkpoint, a read counts 11 rows of `other` (the file holds 1; the closing segment's page the other 10) and
   30 of `test`, and nothing of the closing segment was copied. Before this commit that read found `other`'s page in
   the file alone.
2. *The shared word counts its changes* (22173583). `Segments::shared_word` keeps the four flags in bits 60-63 and, in
   bits 0-59, a count raised by each change of them, wrapping at 60 bits. The copy (`copied`), the reset
   (`reset_if_unread`) and the seed at open (`seed_segments`, now an update of the word it finds) write through it.
   `Segments::unpack` reads the flags only, so lane Q3's cell and its three tests are untouched. Test
   `d45_two_switches_never_bring_the_shared_word_back`: from a switch, a reset and a switch twice bring the flags back,
   and the word differs from the one a read took its span from; a step that changes nothing leaves the word as it is;
   the count wraps with the flags kept.

**The switch, read but not built: what the successor builds** (wal.rs on 22173583 unless named).
- *Where.* The writer, holding the log's `write_lock`, between transactions. `SegmentsCell::update(|s| s.switch(backed_up))`
  answers None while the other segment is still closing: the writer appends on and asks at its next turn. Else, on
  the free segment: `restart_wal_header`'s body (sequence one past the current one's, salts, frame cache, records,
  changes, read index and read marks cleared; it is free, so no read holds one of its marks), `initialized` false so
  the next commit writes and syncs its header before any frame (decision 13), then `second_current` published right
  after the cell's change. A read whose snapshot came from the old flag fails `try_read_mark`'s re-check (its
  checkpoint sequence differs) and retries. The writer's connection then takes the new segment's snapshot
  (`apply_restart_snapshot`) and raises the checkpoint epoch.
- *The second file.* At open `-wal2` is opened only if it exists (B14, `open_second_segment_if_exists`). The first
  switch into it must create it and set the segment's `runtime.file`.
- *The two triggers.* The store's `Readers::restart_if_asked` (store.rs) drops `gate.write()` and
  `Slots::take_all`, and `Db::restart_log` (db.rs) switches through a new connection call in place of the RESTART or
  TRUNCATE checkpoint. The engine's `try_restart_log_before_write` switches where it restarted (the segment wholly
  copied): that segment can be reported copied at once, since `nbackfills` is published only after the file's sync
  (B16).
- *Deleted with them.* `begin_restart`, `end_restart` and `try_restart_log_for_write` on both backends,
  `WalFile::restart_log` and `unlock_after_restart`. A RESTART or TRUNCATE checkpoint then copies and switches, or
  answers Busy while a segment is closing, as B16's do.
- *The backup.* `Db::read_log` reads `-wal` by path (db.rs, `wal_path`). After a switch the shipper would find an
  older generation's header there and ship nothing (ship.rs `ship_durable`: "The log was started again and its header
  isn't written yet"). It must read the current segment's file. Today's rule (restart only once shipped, or past
  twice the bound) can stay as the switch's precondition, the closing segment then switched in as shipped.
- *Condition 3, the slots' segment.* A read live at a switch counts only on its own segment, since a switch needs the
  other one free. So in a slot, a mark is in segment C exactly when its reads count on C and not on the other
  segment (`SlotReads`). The closing pass's safe frame (`ShmWalCoordination::determine_max_safe_checkpoint_frame`,
  through `min_active_reader_frame`) then takes only those slots' frames. That needs the count beside the frame in
  `live_reader_frames`; the slot is registered before it is counted, but a read in that window fails `span_holds`
  and reads nothing.

**Checks** (adacachy2, script `~/.cache/amphora-perf/pagepath-b17/check.sh`, B16's with its logs named b17 and a `t:<filter>`
step; each run held to its needs by `JOBS`). Run r1954 (needs `linux/amd64,cpus=8,mem=24g`, load 24-42) on fe49ddf5
(this branch with origin/main merged): `cargo check -p amphora-engine --lib --tests` exit 0; `d45_` passed 18. Main then
took lane Q3's word (bfa2d3f2), merged as c7690cc2. Run r1979 (refused at cpus=8 with 2 of 32 cores idle, run at cpus=2;
load 21-47) on 554b248e: check exit 0; `d45_` passed 19 (the 18 and the new one); Q3's `q3_` passed 3; `cargo clippy
--workspace --all-targets --locked -- -D warnings` exit 0 in 40 s; `cargo test -p amphora-engine --lib --locked --
storage:: vdbe::vacuum --test-threads=2` passed 735, failed 1 and ignored 14 in 287 s, the one
`cb4_ab_both_rebuy_policies_at_the_extremes` (main's own, as in B16's r1928); `cargo test -p amphora --locked --test
store` passed 14 (main's store test has 14 since lane A18's change).
Run r1989 (needs `linux/amd64,cpus=4,mem=12g`, load 45-78) on 22173583: check exit 0; `d45_` passed 20; `q3_` passed 3;
`storage::shared_wal_coordination` passed 50 and `storage::log_segments` 14; the workspace clippy exit 0 in 45 s;
`scripts/check-production.sh` exit 0 ("10903 left in the engine's production code across 17 lints, as the ratchet
holds"). Main then moved in `crates/pg-parser` (lane PG10; not in amphora-engine's or amphora's trees by `cargo tree -e
all -i`) and in `crates/amphora` (lane W6's store.rs and store test, files this lane didn't touch). The re-merge a1f6337b
(load 12-20, cpus=4): the workspace clippy exit 0 in 30 s; `d45_` passed 20. Pushed to main as a1f6337b. Not run on the
merged tree: the engine's `storage:: vdbe::vacuum` and the store test (r1979 ran them on 554b248e); they are the
successor's first run on main.

**Notes, one line each.** `Db::read_log` reads `-wal` by path, so the backup will need the current segment's file once
the switch exists (above). Main's `store` test has 14 tests since lane A18 (15 at B16's merge).

## perf-B18 (2026-11-24, 01:53-02:53Z, two kiln restarts mid-hour)

**Losses first.** The switch is built and the store's restart no longer waits for any read, but the brief's
condition 1 is only partly met. *Not deleted:* `begin_restart`, `end_restart` and `try_restart_log_for_write` on both
backends, `WalFile::restart_log` and `unlock_after_restart`. Those restarts in place stay for a log that never switched;
once a log has its second segment's file they refuse (the checkpoint answers Busy, the writer's restart before a write
restarts nothing), because a restart in place would take the current segment's sequence two past the other segment's
header, a pair `Segments::recover` refuses as corrupt, and a truncate would leave the other segment's copied frames to
be read as the log. Turning those two triggers into switches needs the writer's read guard moved to the new segment
inside `begin_write_tx` (the trigger before a write) and TRUNCATE's meaning restated for two files (the checkpoint's):
the next lane's. *Not built:* the multi-process backend's switch: `Database::switch_log` refuses a multi-process log
with an error (its shared memory doesn't publish a switch). No A/B ran (rule 0). The engine's `storage::` and
`vdbe::vacuum` suites on the final code: see the checks below.

**Built** (perf/pagepath-d45).
1. *The switch* (`SharedLog::switch`, wal.rs; `Database::switch_log`, database.rs). With the log's write lock held and
   the other segment free: the current segment's file is flushed (so every frame of a closing segment is durable), the
   free segment gets its file (`-wal2` made at the first switch, from the path recovery passes in), is cut to nothing
   if longer than the caller's bound (decision 31, kept at each segment's reuse), takes the closing header's format with
   its sequence one past and new salts (`restart_wal_header`), the epoch and transaction count carried on, its read
   marks set under their exclusive locks, and is then published (`Segments::switch`, then `second_current`). It waits
   for no read: a read live at the switch keeps reading the closing segment.
2. *The store's trigger.* `Db::restart_log` copies what the reads let two passes copy (the closing segment, then the
   current one: with no read under way every frame is in the file, as before), then switches; false while a read
   still counts on the segment the last switch closed. `Readers::restart_if_asked` no longer takes the gate or every
   slot; the gate is deleted (nothing took it for writing).
3. *The backup and the flusher follow the current segment's file* (`segment_path` in db.rs): `Db::read_log`,
   `Db::log_frames` and `Db::flusher` read or flush `-wal` or `-wal2`, whichever is current.
4. *Fixes the switch needed.* A passive pass over the closing segment is not bounded by the current segment's
   flushes (its frames were all flushed before the switch). `read_frame` reads a frame that a commit's records hold
   from its own segment's header and records, not the current segment's (before this the store, files and backup
   tests read zeros after a switch: "Checksum mismatch on page 1: expected 0").
5. *Tests.* `d45_a_reader_that_never_finishes_holds_up_no_restart_and_no_commit` (store test): a read transaction left
   open, the writer restarts (switches) and commits 50 rows past it; a new reader reads 60 rows, the open read still
   10. `d45_a_backup_after_a_switch_holds_the_writes_made_after_it` (backup test): ship, base, switch, 20 rows, ship;
   the restore of the last second holds all 40. The files test's restart case is restated for two segments (the cut
   comes when a segment is switched to again).

**Checks** (adacachy2, `~/.cache/amphora-perf/pagepath-b18/check.sh`, B17's with logs named b18 and an
`it:<test>:<filter>` step for amphora's integration tests; needs `linux/amd64,cpus=8,mem=24g` unless named).
- Main's pass set, r5 on 5bc8d61e (load 13-55): `storage:: vdbe::vacuum` passed 736, failed 1
  (`cb4_ab_both_rebuy_policies_at_the_extremes`, main's own), ignored 14, in 275 s; the store test passed 16.
- r31 on 76880856 (the switch, first cut): clippy failed on one unused import (`RwLock`, the gate's); `d45_` passed
  20; the store test passed 16.
- r57 on c73eb843: clippy exit 0 in 50 s; the new store test failed: the switch set the free segment's read marks
  without their locks ("must hold exclusive lock"). Fixed in 808c8a83.
- r76 on 808c8a83: clippy exit 0 in 39 s; the store test failed 2 of 17, the files test's restart case and both backup
  tests failed, each "Checksum mismatch on page N: expected 0": `read_frame` read a closing segment's record frames
  from the current segment. Fixed in 4741b54e.
- r8 on 4741b54e (load 21-57): the store test passed 17 (the new one and `the_log_stays_bounded_under_writes_and_reads_
  that_never_pause` among them); the files test passed 3, ignored 1 (`what_cutting_the_log_costs_its_writer`, a timing
  check); `t_t1_12_i1_incremental_bases_keep_unchanged_chunks` (two restarts, two bases, restores) passed; the new
  backup test failed for want of a base ("no base precedes second 1040"), fixed in d15b4a09; the workspace clippy exit
  0 in 55 s; `d45_` passed 20. Stopped before the engine suite to test the merge with main instead.
- r32 on b4662671 (this branch with main merged, lane Q5's read path in wal.rs among it; load 45-57): the store test
  passed 17; `d45_a_backup_after_a_switch_holds_the_writes_made_after_it` passed; the files test passed 3, ignored 1;
  the workspace clippy exit 0 in 42 s; `d45_` passed 20. Main then moved only outside crates/ (lane V15's records),
  merged as 929355dd and pushed to main as a5ae048c without a new run. After the push (rule 5b), on the same
  b4662671: `storage:: vdbe::vacuum` passed 738, failed 1 (`cb4_ab_both_rebuy_policies_at_the_extremes`, main's own, as
  in r5) and ignored 14, in 261 s; `scripts/check-production.sh` exit 0 in 60 s ("10903 left in the engine's
  production code across 17 lints, as the ratchet holds").

**Notes, one line each.**
- The switch carries the epoch over unbumped, and a pass that finds the closing segment copied at its start resets it
  with no Finalize (so no epoch bump): check that no cached page's frame tag from one segment can pass for the other's.
- `-wal2` is made by the engine at the first switch without the no-copy-on-write flag `Db::open_made` gives `-wal`.
- `files.rs`'s ignored timing check `what_cutting_the_log_costs_its_writer` still asserts the one-file cut; it needs
  restating for two segments before it is run again.
- The store's checkpointer decides a pass by the current segment's uncopied frames (`Db::uncopied`): a closing
  segment's frames wait until the current segment passes its bound, so the log can hold up to about twice its bound.

## perf-B19 (2026-11-24, 02:53-03:53Z; main's red after B18 put first by the coordinator at 02:58Z)

**Losses first.** Nothing merged. Decision 45's switch is not finished: RESTART, TRUNCATE and the writer's restart
before a write don't switch, and the in-place restart is not deleted. The wrong-read test is red one way (below), not
fixed. Two failures on the branch are unexplained: `session_reads::t_t1_10_r1_a_session_reads_its_write_on_a_lagging_
follower` (session_reads.rs:122, `at_once` 1 where 0) and backup's `t_t1_12_d1_restore_drills_at_random_seconds`
("source.db-wal2 held 0 bytes at 0, not 32"); neither is known to fail on main. Checks ran only from 03:45Z: the Mac
run (r98, MacBook Pro 2 at load 57-97) took 19.6 min to build the engine's tests and gave nothing else in the hour.

**Seed 45: the cause.** A switched-to segment's header is written by its first commit. B18's switch also left a
segment no commit had written since the switch to it: its file still held its previous life's header, two sequences
back, and the other segment took the unwritten sequence plus one, so the headers on disk were three apart, a pair
`Segments::recover` refuses. Seed 45's pair is that: `-wal` 4, `-wal2` 1 (its life at 3 never written). An in-place
restart after a switch leaves two apart, not three, and is refused once `-wal2` is open, so deleting it is not the fix.
What asks for the second switch (store.rs 1044-1064, from the code): the checkpointer's pass copies the closing segment
first and reports its frames as `c.log`; past the bound it asks a restart, and the writer's next turn switches before a
commit reached the new segment.
- *Fix (94d01e88):* `SharedLog::switch` answers None while the current segment holds no frame.
- *Test:* `d45_a_switch_never_leaves_two_headers_recovery_refuses` (switch, write, copy, switch, copy, a third switch
  asked with nothing written, write, reopen a copy of the three files).

**The four replicated tests.** `a_local_read_open_across_an_image_install_sees_one_side_whole` asserted the
one-segment restart's Ok(false) under an open read; restated to decision 45's Ok(true) (64e77079, replication.md's
paragraph with it). `Db::log_frames_in_use` read the writer connection's own max frame, which a switch leaves on the
closed segment until its next transaction: now the shared current segment's count (e44b8868).

**Wrong reads across segments (the brief's condition 1).** Red closing-to-current:
`d45_a_page_cached_from_the_closing_segment_never_passes_for_the_current_ones` fails at its last check (the file alone
holds 0 rows of 'B'). A reader's page read from the closing segment's frame 1 after the switch gets the tag (1, epoch)
(`read_frame` tags a page read from any segment); that segment, copied before the switch, is reset with no Finalize and
so no epoch bump, and the pass over the current segment takes the cached page for its own frame 1. Green after reuse
(`..._for_its_frames_after_reuse`): the pass copying the other segment between bumps the epoch, and with 94d01e88 no
switch skips that copy. Fix for the next lane: tag only pages read from the current segment (the checkpoint's own
comment says the cache keys by the current segment's frames), then rerun both.

**Checks** (r146, adacachy2, needs `linux/amd64,cpus=8,mem=24g`, 64e77079, load 22-48).
- `d45_`: 22 passed, 1 failed (the red test above).
- tests/crash.rs power cut, 200 seeds: passed.
- `database::replicated::`: 148 passed, 1 failed (`t_t1_10_r1`, above); A20's four pass.
- tests/store.rs 17/17. tests/backup.rs 20 passed, 1 failed (`t_t1_12_d1`, above).
- Workspace clippy (`--all-targets -D warnings`): exit 0 in 59 s.
- The fix taken out (64e77079 less its three lines): the seed-45 test fails as seed 45 did, "the copy doesn't open
  (third switch made: true): ... checkpoint sequences Some(0) and Some(3), neither one past the other".
- Main before B18 (46773985) on the four tests and the power cut: still running at the hour's end (r146).

## perf-B20 (2026-11-24, 03:53-05:00Z; the wrong read across segments fixed, B19's branch landed)

**Losses first.** Three failures stand on main itself, and this lane fixes none of them (not its deliverable):
- `database::replicated::tests::a_checkpoint_pass_copies_no_frame_past_the_last_finished_flush` fails 3 of 3 on main
  5bd0f46e ("left: 77, right: 17" and "76, 18": the pass reports more frames than `log_frames`), 3 of 3 on the branch
  merged with it (a671b8f2), and 3 of 3 on that tree with this lane's fix reverted (r282, diag.sh). B19 saw it pass
  at r146 on an older main.
- backup's `t_t1_12_d1_restore_drills_at_random_seconds` fails 28 of 28 on main fac69083 and 28 of 28 on the branch
  (below).
- `window_slow_flooded_fast_starved` failed once ("Fenced: the state machine failed") in r221's
  `database::replicated::` run on 284e4806, passed in r282's on a671b8f2, and passed 3 of 3 alone on the merged
  tree, the tree less the fix, and main: a flake under load, not run on main inside the suite.

Decision 45's switch is still not made by RESTART, TRUNCATE and the writer's restart before a write, and the five
in-place restart functions are not deleted: the next lane's.

**The wrong read: what labelled the page (traced).** The checkpoint's pass over the current segment takes a cached page
in place of a frame when the page's label, (frame, epoch), is that frame and the current epoch
(`cache_get_for_checkpoint`). Frame numbers start again in each segment (decision 45). Two things let a label from one
segment pass for the other's frame:
- `read_frame` and `read_frames_batch` labelled a page read from the closing segment with that segment's frame number
  and the current segment's epoch;
- a switch carried the closing segment's epoch over unchanged (`start_after`), so a label taken while the closing
  segment was current also matched the new segment's frames until the next pass's bump. A closing segment copied
  whole before the switch gets no bump at all: its pass returns at once.

`d45_a_page_cached_from_the_closing_segment_never_passes_for_the_current_ones` under `RUST_LOG=amphora_engine::storage::wal=debug,amphora_engine::storage::pager=trace`
(r201, adacachy2, needs `linux/amd64,cpus=4,mem=12g`), before the fix (32a1adc5, the log line only), red at
wal.rs:11909 (the file held 0 rows of 'B'):
```
read_frame(page_idx = 2, frame_id = 1, segment = Second)
read_frame(page_idx = 2, frame_id = 1, segment = Second)
cache_get_for_checkpoint: page 2 frame 1 is valid
```
After it (73cbafdc), green:
```
read_frame(page_idx = 2, frame_id = 1, segment = Second, label epoch = None)
read_frame(page_idx = 2, frame_id = 1, segment = Second, label epoch = None)
cache_get_for_checkpoint: page 2 has frame/tag (17592186044415, 1048575): (dirty=false), need frame 1 and seq 4
```
The reader's page from the closing segment carries no label (the unset tag), so the pass reads the current segment's
frame 1 ('B') from the log. `..._for_its_frames_after_reuse` passes before and after (its trace shows a page read
from the first segment while it was closing, unlabelled).

**The fix (73cbafdc, 6c28b81c).** At both causes:
- `WalFile::label_epoch`: a page is labelled only when its frame is in the current segment, the epoch read before
  the segment, so a switch between the two reads leaves the page unlabelled. Both read paths use it.
- `WalFileShared::start_after`: the free segment starts one epoch past the closing one's, as a restart bumps it; its
  doc said the epoch was carried "so that it never comes back to a value a connection took from the closing
  segment", and carrying it unchanged did exactly that.
- A closing pass reads its own frames (the checkpoint's comment), so the closing segment's pages lose nothing.
- 6c28b81c: `bool::then_some` is a disallowed method; check-production's engine ratchet counted 41 against 40 (r221).
- The two tests run under `test_log` (32a1adc5), and `read_frame`'s debug line names the segment and the label: the
  signal that would have shown this at first look.

**The two failures B19 couldn't explain (r189, adacachy2, needs `linux/amd64,cpus=4,mem=12g`, load 55-62).** Main
fac69083 and the branch merged with it (b4811aca), both test binaries built once and run side by side, 28 rounds
each (the 29th stopped):
- backup's `t_t1_12_d1_restore_drills_at_random_seconds`: fails 28 of 28 on main and 28 of 28 on the branch, at
  backup.rs:172 (`end_base`), "source.db-wal2 held 0 bytes at 0, not 32" on both. Main's own, from before this
  branch (main lacks 94d01e88, so not its refusal). Not fixed here: not this lane's deliverable.
- `session_reads::t_t1_10_r1_a_session_reads_its_write_on_a_lagging_follower`: passes 28 of 28 on main and 28 of 28
  on the branch. Main has since renamed it `t_t1_10_r5_...` (358fbde2), body unchanged; it passed in r221's
  `database::replicated::` run on the merged tree.

**Ada's decisions applied.** replication.md section D restated (c50e0709): a replica's log is two segment files, each
within the room; the room counts the current segment's frames from the log's shared state. The image-install test
keeps B19's Ok(true) (64e77079).

**Checks.**
- r201 (73cbafdc): `d45_` 23 passed (B19: 22 and the red one); workspace clippy exit 0 in 53 s.
- r221 (284e4806, the branch with main 39ecf1df merged; needs `linux/amd64,cpus=3,mem=10g`, load 38-89): workspace
  clippy exit 0 in 61 s ("Checking amphora-engine" in its log); `d45_` 23 passed, the store and backup `d45_` 1 and
  1; the 200-seed power cut passed; `database::replicated::` 149 passed, 2 failed (below); tests/store.rs 17/17;
  tests/backup.rs 20 passed, 1 failed (`t_t1_12_d1`, main's own, above); `storage:: vdbe::vacuum` 750 passed,
  0 failed, 14 ignored in 291 s; check-production exit 1 (the ratchet, fixed in 6c28b81c).
- The final checks of a1c3dc95 (the third merge, main 0c13cf68) that this lane ran and didn't record before it was
  interrupted, from the logs it left on adacachy2 (`~/.cache/amphora-perf/pagepath-b20/*.log`, written 04:34Z to
  04:44Z after its checkout of a1c3dc95 at 04:33Z; the kiln run id wasn't read; filled in by lane B21): workspace
  clippy Finished in 44 s ("Checking amphora-engine" in its log, no error); check-production exit 0 ("10903 left ...,
  as the ratchet holds"); `d45_` 23 passed, the store and backup `d45_` 1 and 1; the 200-seed power cut passed;
  `database::replicated::` 151 passed, 1 failed (`window_slow_flooded_fast_starved`, "Fenced: the state machine failed",
  load high enough that the suite took 100 s); tests/store.rs 17/17; tests/backup.rs 20 passed, 1 failed (`t_t1_12_d1`,
  main's own); `storage:: vdbe::vacuum` 752 passed, 0 failed, 14 ignored in 249 s.

## perf-B21 (2026-11-24, 04:58-05:58Z; lane B20's branch landed on main)

**Losses first.** Failures that stand on main itself, which this lane doesn't fix:
- backup's `t_t1_12_d1_restore_drills_at_random_seconds` fails 5 of 5 on main a64549cc and 5 of 5 on the merged tree
  999fa608 (r307's `cmp`), and 28 of 28 each in B20's r189: at backup.rs:172, the drill's `end_base`, "source.db-wal2 held 0
  bytes at 0, not 32" (a base that ends while the log's current segment holds no frame finds no header in its file).
  It came with lane B18's switch (main a5ae048c, decision 45). A restore drill that fails every time is a real hole in
  the backup path.
- `a_checkpoint_pass_copies_no_frame_past_the_last_finished_flush` fails 5 of 5 alone on main a64549cc and 4 of 5 alone on
  the branch (r307 `cmp`, 'left: 1 right: 18' to 'left: 77 right: 17'), in every loaded suite round on both arms (r364:
  4 of 4), and passed in the quieter suites (r307's 152 of 152; B20's at a1c3dc95, 151 of 152). Main's own, as B20
  found (5bd0f46e): a pass whose count of frames and the test's disagree, not traced here.
- Under load (r364, below) `v13_thirty_two_spread_writers_on_moving_time_are_each_answered_with_no_stale_base` failed in
  every round on both arms at main a64549cc; on main 3a58a13f (a1edd3e2, lane P22's slot changes merged) it passed (r401).
- Still not done (decision 45, the next lane's): RESTART, TRUNCATE and the writer's restart switching, and the five
  in-place restart functions' deletion. A multi-process log refuses to switch (`Database::switch_log`). When one switches,
  the authority's shared epoch must move with it: the label reads that counter (`WalFile::label_epoch`, through
  `coordination.checkpoint_epoch`) while the switch moves only the segment's.

**What landed.** B19's and B20's commits (94d01e88, e44b8868, ba6ee7d8, 64e77079, 32a1adc5, 73cbafdc, 92fd292e, c50e0709,
6c28b81c) on main: 286 lines in five files (`crates/engine/storage/wal.rs`, `crates/amphora/src/db.rs`,
`crates/amphora/src/database/replicated.rs`, docs/design/replication.md and docs/design/store.md), merged with main up to ac2c429a (baa9d51c): each re-merge as main moved came in clean, with
formatting and the net diff of 286 lines in five files against main unchanged (lane P22's replicated, slot and serve
changes and hyper-raft 0009 at 3a58a13f; PG15's engine changes at 1f4a8e88; V18's and F6's at 731240ed; T21's at
ac2c429a). The landing fixes what B18's switch (main a5ae048c) broke: a committed write lost to later readers (a page
cached from the closing segment passing for the current segment's frame, copied into the file by the next pass), and a
database a power cut left unopenable (seed 45, a segment with no frame switched from). The fix read in full before landing,
three facts checked in the code: a switch stores the free segment's epoch, one past the closing one's, before it makes that
segment current (`SharedLog::switch`, then `segments.update`); `checkpoint_epoch` reads the current segment's epoch
(in process); the switch has one caller, `Database::switch_log`, which refuses in multi-process mode.

**The wrong-read tests and their log** (r307, 999fa608, adacachy2, needs `linux/amd64,cpus=8,mem=24g`, load 11,
`RUST_LOG=amphora_engine::storage::wal=debug,amphora_engine::storage::pager=trace`): both pass, and every read from a
closing segment is unlabelled:
```
d45_a_page_cached_from_the_closing_segment_never_passes_for_the_current_ones
  read_frame(page_idx = 2, frame_id = 1, segment = Second, label epoch = None)   (twice)
  cache_get_for_checkpoint: page 2 has frame/tag (17592186044415, 1048575): (dirty=false), need frame 1 and seq 4
d45_a_page_cached_from_a_segment_never_passes_for_its_frames_after_reuse
  read_frame(page_idx = 2, frame_id = 1, segment = Second, label epoch = None)
  read_frame(page_idx = 2, frame_id = 1, segment = First, label epoch = None)
  cache_get_for_checkpoint: page 2 has frame/tag (17592186044415, 1048575): (dirty=false), need frame 1 and seq 6
```
(17592186044415, 1048575) is the unset tag (2^44 - 1, 2^20 - 1): the pass takes the frame from the log, not the page cached
from the other segment. B20's trace before the fix showed "page 2 frame 1 is valid".

**The two unknown failures, the same number of runs on each arm** (r307 `cmp`, 5 rounds, both arms side by side on the same
machine, main a64549cc and the merged tree 999fa608, load 43): `t_t1_10_r5_a_session_reads_its_write_on_a_lagging_follower`
(main renamed B19's `t_t1_10_r1`, 358fbde2) fails 0 of 5 on each; backup's `t_t1_12_d1` fails 5 of 5 on each with the same
panic. No failure is the branch's alone, as B20's r189 (28 of 28 each) found.

**The replicated suite on both arms under load** (r364, adacachy2, needs `linux/amd64,cpus=4,mem=12g`, load 45 to 58,
`database::replicated:: --test-threads=4`, main a64549cc and 999fa608, each two rounds, alternated):
| arm | round 1 | round 2 |
|---|---|---|
| main a64549cc | 147 passed, 5 failed (124 s) | 147 passed, 5 failed (153 s) |
| branch 999fa608 | 149 passed, 3 failed (118 s) | 150 passed, 2 failed (158 s) |

On both arms: `a_checkpoint_pass_copies_no_frame...` and `v13_thirty_two_spread_writers_on_moving_time...`. On main only, as
before (B18's reds, which B19's e44b8868 and 64e77079 fix on the branch): `a_local_read_open_across_an_image_install_sees_one_side_whole`,
`past_its_room_a_replica_refuses_submits_and_restarts_whatever_its_share`, `a_replicas_database_log_stays_within_its_room`.
On the branch only, once (round 1): `every_falsifier_of_the_floor_holds_under_a_nemesis` (its pump's settle; lane W4
(perf-persisted) measured it alone at 1 of 16 on main and 1 of 16 with its change reverted, so it predates this branch).
`window_slow_flooded_fast_starved`, which failed twice in B20's suites, failed in neither arm here.

**Checks** (trees 999fa608 (main a64549cc merged), a1edd3e2 (3a58a13f), 1dc7e894 (1f4a8e88), baa9d51c (ac2c429a)):
- r307: `cargo fmt --all --check` exit 0; workspace clippy exit 0, and with the engine re-checked exit 0 in 37 s (r364:
  "Checking amphora-engine v0.1.0" in its log, by touching its lib.rs); check-production on Linux exit 0 ("Checking
  amphora-engine" in its log, "10903 left ... as the ratchet holds"); `d45_` 23 passed (engine) and 1 and 1 (tests/store.rs,
  tests/backup.rs); the 200-seed power cut (`seeds(200, 1)`, seed 45 in it) passed; `database::replicated::` 152 passed,
  0 failed, 5 ignored in 38 s (`--test-threads=8`, load 24); tests/store.rs 17 of 17; tests/backup.rs 20 passed, 1 failed
  (`t_t1_12_d1`, main's own, above); `storage:: vdbe::vacuum` 752 passed, 0 failed, 14 ignored in 279 s.
- The crates, Cargo and vendor files of a1c3dc95 and 999fa608 are identical (`git diff` empty), so B20's checks at a1c3dc95
  (above) are this tree's as well: 752, 0, 14.
- r401 (a1edd3e2, needs `linux/amd64,cpus=4,mem=12g`, load 54 to 65, 05:24Z to 05:30Z): `cargo fmt --all --check` exit 0;
  workspace clippy exit 0 in 15 s (amphora, amphora-api and hyper-durable re-checked; amphora-engine is the source r364
  re-checked); check-production exit 0 on Linux (amphora-engine re-checked, "10903 left ... as the ratchet holds"); `d45_`
  23 passed, the store and backup `d45_` 1 and 1; `database::replicated::` 154 passed, 1 failed (`a_checkpoint_pass...`,
  main's own) in 73 s; tests/store.rs 17 of 17; tests/backup.rs 20 passed, 1 failed (`t_t1_12_d1`, main's own); the
  200-seed power cut passed. The engine's `storage:: vdbe::vacuum` was not rerun: P22's merge changes nothing in
  `crates/engine` (r307's 752, 0, 14 at the same engine source).
- r428 (1dc7e894, main 1f4a8e88 merged: lane PG15's engine changes to vdbe/vacuum, vdbe/insn, storage/sqlite3_ondisk and
  the dialect; needs `linux/amd64,cpus=8,mem=24g`, load 41 to 65, 05:31Z to 05:41Z): workspace clippy exit 0 in 47 s ("Checking
  amphora-engine v0.1.0"); check-production exit 0 on Linux ("10903 left ... as the ratchet holds"); `d45_` 23 passed and 1 and 1
  (tests/store.rs, tests/backup.rs); the 200-seed power cut passed; `storage:: vdbe::vacuum` 752 passed, 0 failed, 14
  ignored in 263 s; `database::replicated::` 155 passed, 0 failed, 5 ignored in 56 s (`a_checkpoint_pass...`
  passed in it, as in r307's suite).
- r481 (baa9d51c, main ac2c429a merged: lane V18's replicated changes, lane F6's ratchet and engine connection.rs and mvcc,
  lane T21's optimizer/lip.rs; needs `linux/amd64,cpus=4,mem=12g`, load 28 to 45, 05:44Z to 05:48Z): workspace clippy exit 0 in
  35 s ("Checking amphora-engine v0.1.0"); check-production exit 0 on Linux ("10617 left ... as the ratchet holds", lane F6's
  counting); `d45_` 23 passed (engine); the replicated tests this branch touches
  (`a_local_read_open_across_an_image_install_sees_one_side_whole`, `past_its_room_a_replica_refuses_submits_...`,
  `a_replicas_database_log_stays_within_its_room`) 3 of 3. Main moved three times during the landing, so the whole suites were
  not rerun at the last merge (rule 6: the touched tests and clippy); r428 ran them whole on 1dc7e894, three merges earlier.
- r516 (e429f661, main 219603df merged: lane Q8's read-write lock on the log's frame cache in storage/wal.rs, read before the
  run: it changes neither the label nor the epoch; needs `linux/amd64,cpus=4,mem=12g`, 05:49Z to 05:52Z): workspace clippy
  exit 0 in 43 s (amphora-engine re-checked); `d45_` 23 passed; the three replicated tests this branch touches passed. The
  last merge on top (main c205dd7d: lane A21's memory-budget charge in replicated.rs, 250 lines, with its own tests, and
  records) wasn't re-run at the hour's end: A21's own landing checks (its record, r497) cover it, and this branch's
  replicated.rs change is one test's expectation and doc lines. Main moved four times during the landing.

**Commands.** `~/.cache/amphora-perf/pagepath-b21/` on this machine holds `check.sh` and the run commands (`cmd-r1.sh` r307,
`cmd-r3.sh` r364, `cmd-r4.sh` r401) and runs on adacachy2 in the lane's clone `~/.cache/amphora-perf/pagepath-remote`, target
`~/.cache/amphora-perf-target/pagepath`, CARGO_INCREMENTAL=0, nice 10, `JOBS` = the needs' cpus.

## perf-B22 (2026-11-24, 05:55-06:55Z; a backup across decision 45's switch)

**Losses first.**
- Not on main. The lead held the merge for check-production on Linux, which waits for the benchmark window's end; the branch
  `perf/pagepath-b22` holds the work, its tree equal to main 8fc85651 merged with this lane (f12728d2).
- Main answers wrong, silently, since lane B18's switch (a5ae048c), and this was not in the brief. The brief's defect refused
  a base; this one loses rows. Red on main 0101aa38 plus the tests (r579, below): a base begun after a switch, while a read
  under way kept the closing segment's later frames out of the file, restored 20 of 41 committed rows with no error.
- Clippy was red once and not re-run: at f12728d2 it checked amphora-engine and amphora and stopped at one error, in this
  lane's test (`chunks_exact` with a constant size, tests/backup.rs `log_files`), fixed in the branch's last test commit
  (`as_chunks`) with no run after it. Targets it hadn't reached when it stopped are unchecked.
- Not run in the hour: the engine's `storage::` and `vdbe::` suites, its `d45_` tests, tests/store.rs, the 200-seed power cut,
  and check-production. macOS gave one machine (MacBook Pro 2, load 40 to 86 of 18 cores): the first build took 10 min, and
  the runs queued with needs only (r550, r564, r575, r608) never started ("others ahead of it").
- The shipping thread's own test of a base that waits (dcd6769b) was not run. It is out of the branch's tip.
- A base that waits for a read begins in the first round after a commit that follows the read's end: an idle database with a
  waiting base gets none until it commits. No restore answers wrong meanwhile; seconds with no base are refused.

**1. Reproduced.** r579 (MacBook Pro 2, needs `macos/arm64,cpus=8,mem=16g`, ba250493 = main 0101aa38 plus this lane's tests,
load 66 of 18 cores, 06:05Z to 06:18Z). The drill's message now says which base and what the log's two files hold (its
expectation is unchanged):
```
seed 0x123456789abcdef1, step 55: BaseCopy { id: 1022862395980, start_gen: 1, chunk_bytes: 32768, file_bytes: 73728, next: 3, .. }
refused its end: source.db-wal2 held 0 bytes at 0, not 32; shipped through frame 106 of the archive's generation 1;
-wal: 436752 bytes, generation 0, 106 frames of it; -wal2: 0 bytes, no header
```
B21's inference holds. The base began in the generation `-wal` holds, every chunk was copied, all 106 frames were shipped,
and the writer had switched to `-wal2`, which no commit had reached: 0 bytes. Seeds 1 and 2 pass; the third fails at step
55.

**2. Wrong for a user.** Yes, three ways (the tests, red on main in r579; the table says what the restored database answered):
- *A base can't end while the current segment has no frame* (`d45_a_base_ended_while_the_current_segment_holds_no_frame_
  restores_every_commit`): "the base refused its end". No committed row is lost by it: every commit is in the archive's
  log. But the shipping thread throws the base away and starts it over (a round that fails), and with no base the archive
  restores nothing (read in the code: no restore is made on that arm). It lasts as long as the writer is idle: the store
  switches after a batch commits (`store.rs` 1674, 1684).
- *A restored database can't be backed up to its archive again* (`a_restored_database_is_backed_up_to_its_archive_again`):
  "the shipper refused to open: restored.db-wal held 0 bytes at 0, not 32". A restore's replay empties the log and keeps
  its file. `Shipper::open` reads the header the same way (ship.rs 454; the same line is in a5ae048c's parent, so it predates
  the switch: read in the code, not run there).
- *A base begun after a switch loses commits* (`d45_a_base_begun_while_a_read_holds_the_closing_segment_restores_every_commit`):
  "21 rows committed before the base began are lost in its restore: [20, ... 40]" of 41.
So the "32 bytes" isn't the drill's: it's `Db::read_at`'s, read by the shipper. Decision 45 says a segment's header is
written by its first commit (store.md, "The reset, on an event", its last bullet), so a file with no header is one of the
log's states, and the drill's expectation (a base ends at any step) is right. It stands as written.

**What the restored database answered, on both arms** (the same machine, MacBook Pro 2; main 0101aa38 plus the tests is
ba250493, r579; fixed is 38e42411, r621):

| test | main plus the tests | fixed |
|---|---|---|
| drill, seeds 1 and 2 (12 random seconds each) | no restored second differed by a row | no restored second differed by a row |
| drill, seed 3 (12 random seconds) | the base refused its end at step 55, so no restore was made | no restored second differed by a row |
| base ended on an empty current segment | refused its end, no restore made | 25 of 25 rows at the last commit's second, 25 at a second with no commit, then every second to 35 rows |
| restored database backed up again | its shipper refused to open | every second restores, to 50 of 50 rows |
| base begun while a read holds the closing segment | **20 of 41 rows, no error** (ids 20 to 40 gone) | 41 of 41 |

**3. The cause, and the fix.**
- *The header* (4c84dcb0). `ship_durable` and `Shipper::open` read the current segment's first 32 bytes and failed the
  round where the file holds fewer. `Db::log_header` answers None there, and the shipper ships nothing of that generation,
  as it already did where the file still held the header of the generation before.
- *The base's start* (38e42411; its thread test, unrun, at dcd6769b). A restore replays the log from the first frame of the generation its base began
  in, and backup.md rested on "a log only starts again once every frame is in the file". Decision 45's switch waits for no
  read, so a read keeps the closing segment's frames past its mark out of the file, and a base begun in the next generation
  copied the file without them. `Shipper::begin_base` refuses until the file holds every frame before the current segment
  (`Database::wal_closing_copied`: no segment closing, or the closing one's frames all backfilled, which is published after
  the file's flush). The shipping thread asks for a pass first; where a read still holds the segment the base waits
  (`base_waits`) and begins in a later round.
- backup.md restated: the header's state, a base's start, and what an incremental base's kept chunk rests on.

**Checks.**
- r579 (MacBook Pro 2, ba250493): the drill red, the three scenario tests red, `d45_a_backup_after_a_switch...` green (above).
- r621 (MacBook Pro 2, 38e42411, needs `macos/arm64,cpus=4,mem=12g`, load 41 to 81, 06:21Z to 06:33Z):
  tests/backup.rs 24 passed, 0 failed, 1 ignored (the benchmark) in 254 s. The drill passes as written, all three seeds; the
  three scenario tests pass; `t_t1_12_*` as before.
- r685 (MacBook Pro 2, f12728d2, the branch's tree, needs `macos/arm64,cpus=7,mem=14g`): clippy exit 101 in 645 s
  (load 54 to 86), "Checking amphora-engine" and "Checking amphora" in its log, the one error above. The steps after it
  (tests/backup.rs, `d45_`, tests/store.rs, the power cut, `storage:: vdbe::`, all at f12728d2) were still to run at the hour's
  end: the successor reads r685 (logs in `~/.cache/amphora-perf/pagepath-b22/*-land.log` on that machine).
- Local: `cargo fmt --all --check` exit 0.

**Found on the way, left (read in the code, no scenario run).**
- A restore is refused where a `-wal` is left at its path, not where a `-wal2` is (restore.rs 162): the open after it would
  take a leftover segment whose sequence is one from the restored log's as its own.
- `Shipper::send` reads frames from the current segment's file (`Db::read_log`), not the segment its generation is in
  (decision 45's "Where it lands"). A log restarted past its bound while a send of unshipped frames is under way makes it
  read the other segment's bytes.
- A round between `Db::switch_log` and the fence's word of it (`restart_log`) fails `end_base` ("ship the log's generation
  before ending a base in it"), and the base under way starts over.
- `Checker::admitted` counts `-wal` and not `-wal2` in the disk a restore needs.

**Commands.** `~/.cache/amphora-perf/pagepath-b22/` on this machine holds the red run's output and the check script (`mac.sh`);
on MacBook Pro 2 the same folder holds the logs, target `~/.cache/amphora-perf-target/pagepath`, CARGO_INCREMENTAL=0, nice 10.


## perf-B23 (2026-11-24, 06:54:32Z-07:54:32Z; lane B22's two backup fixes on main)

**Losses first.**
- The four side notes lane B22 left are not fixed. They are the next backup lane's, named at this section's end.
- The fixes are on main, and no run measured what they cost. No benchmark was run (rule 0: the deliverable is correctness).
- `storage:: vdbe::` is not clean: `t_qt_6_35_joins_over_rowid_ranges_run_on_morsels_as_the_serial_loop` fails on the landed
  tree (r977, Linux), as it did before it (r719, Linux; r685, macOS), and as lane C1's record has it failing on main ("the hook
  built no link", the message r977 prints). It is the only failure of 1156 tests. This lane adds twelve lines to the engine
  (`Database::wal_closing_copied`, database.rs), which no engine code and no engine test calls. Lane C4 changed that test on main
  after this landing (171e902c); this lane ran nothing on that.
- `t_qt_6_18_join_workers_run_a_copy_of_the_loop_body_as_the_serial_loop` had not finished when r719 was stopped, at least 4
  minutes past its start. It did not finish in 24 minutes on main alone either
  (2026-11-24-macstudio2-perf-durable.md). Lane C5's fix reached main in 60993be4, and it passes in r977.
- Run time lost to this lane's own script: it touched `crates/engine/lib.rs` (to have clippy re-check the engine) while
  builds waited on the folder's lock, so "Compiling amphora-engine" is in six of r719's logs where two builds needed it.
- r685's clippy is red at f12728d2 (lane B22's `chunks_exact`, fixed in 7ceb573a); the clippy of the landed tree is green (r953).

**What landed.** Main b53a29b1 at 07:27:38Z, a fast-forward from 60993be4: lane B22's 4c84dcb0 (a segment with no header yet is one of
the log's states, `Db::log_header`), 38e42411 (a base begins once the closing segment is in the file), the tests (ba250493, 7ceb573a), and the shipping thread's own test dcd6769b, which 95df7ed2 had taken out unrun and 25e2c075 puts back. No production
code changed in this lane. Main moved four times while it landed: 75efff54 and 04aee7c9 (records only), 95669341 (lane PG18: crates
pg-analysis and pg-catalog, the second of which amphora-engine builds on), 60993be4 (lane C5: one `#[cfg(test)]` engine file). Each
was merged and checked as the table says. The commits: ba250493, 4c84dcb0, 38e42411, dcd6769b, 69c506e4, d4c112d0, 95df7ed2,
7ceb573a (lane B22's) and 25e2c075 (this lane's).

**Checks** (adacachy2, `on` it, needs as named; the clone is `~/.cache/amphora-perf/pagepath-remote`, target
`~/.cache/amphora-perf-target/pagepath`, CARGO_INCREMENTAL=0, nice 10):

| run | tree | needs, load | result |
|---|---|---|---|
| r719 | 25e2c075 (main b417bb1f merged) | `cpus=12,mem=32g`, 14 to 89, 06:58Z to 07:15Z | `cargo fmt --all --check` 0; clippy `-p amphora -p amphora-engine --all-targets --locked -D warnings` exit 0 in 284 s, waits for the build folder's lock included ("Checking amphora-engine v0.1.0" in its log); amphora lib `backup::` 38 passed, 0 failed (the shipping thread's test among them); tests/backup.rs 24 passed, 0 failed, 1 ignored (the benchmark) in 21.5 s; tests/store.rs 17 passed; engine `d45_` 23 passed; the 200-seed power cut 1 passed; check-production exit 0 ("Compiling amphora-engine" in both passes, "10617 left ... as the ratchet holds"); `storage:: vdbe::` 1155: 1134 ok, 1 failed (`t_qt_6_35`), 18 ignored, 2 unfinished when stopped (`t_qt_6_18`, `g7_5_many_writers_answer_as_a_serial_replay`) |
| r906 | 861a3642 (main 95669341 merged) | `cpus=4,mem=12g`, 40 to 85, 07:17Z to 07:27Z | fmt 0; `backup::` 38 passed; tests/backup.rs 24 passed, 0 failed, 1 ignored in 48.7 s; tests/store.rs 17 passed; power cut 1 passed; clippy exit 0 in 47 s ("Checking amphora-engine"); check-production exit 0 in 74 s ("Compiling amphora-engine" twice, 10617); `d45_` 23 passed; `g7_5_many_writers_...` alone 1 passed in 219 s |
| r953 | b53a29b1, landed (main 60993be4 merged) | `cpus=8,mem=24g`, 37 to 50, 07:25Z to 07:27Z | fmt 0; clippy exit 0 in 40 s ("Checking amphora-engine"); `backup::` 38 passed; tests/backup.rs 24 passed, 0 failed, 1 ignored in 13.0 s; tests/store.rs 17 passed; power cut 1 passed; check-production exit 0 in 66 s ("Compiling amphora-engine" twice, 10617) |
| r977 | b53a29b1, main itself after the push | `cpus=8,mem=24g`, 40 to 70, 07:29Z to 07:37Z | check-production exit 0 in 70 s ("Compiling amphora-engine" twice, "10617 left ... as the ratchet holds"); the red arm (below); `storage:: vdbe::` 1137 passed, 1 failed (`t_qt_6_35`), 18 ignored in 256 s, `t_qt_6_18` among the passed |
| r1024 | main 980d7c76 (lane V20's `Db::restart_log` change landed on top) | `cpus=6,mem=16g`, 33 to 50, 07:37Z to 07:38Z | `backup::` 38 passed; tests/backup.rs 24 passed, 0 failed, 1 ignored in 20.3 s; tests/store.rs 17 passed; power cut 1 passed |
| r685 (lane B22's run) | f12728d2, the same production code | MacBook Pro 2, macOS, 41 to 86, 06:41Z to 07:29Z | clippy red on lane B22's test (above); tests/backup.rs 24 passed, 0 failed, 1 ignored in 118.7 s; `d45_` 23 passed; tests/store.rs 17 passed; power cut 1 passed; `storage:: vdbe::` 1155: 1134 ok, 1 failed (`t_qt_6_35`), 18 ignored, 2 unfinished (`t_qt_6_18`, `g7_5`) when this lane stopped it (the lead's word) |

The drill `t_t1_12_d1_restore_drills_at_random_seconds`, lane B22's three scenario tests and
`d45_a_backup_after_a_switch_holds_the_writes_made_after_it` are in tests/backup.rs's 24. `d45_` is the engine's 23; the one
`d45_` test of tests/store.rs, the three of tests/backup.rs and the shipping thread's are in their suites above. r719's are this
branch's first runs on Linux: lane B22's had been on macOS alone.

**The shipping thread's test is red without the fix.** `d45_a_due_base_waits_for_the_closing_segment` (dcd6769b) with 38e42411's start
condition taken out of ship.rs and shipping.rs (perf/pagepath-b23-red 60fd9336, never for main; r977): "panicked at
crates/amphora/src/backup/shipping.rs:789:9: the read keeps the closing segment back", 0 passed, 1 failed. It passes as written on
each tree above (38 of 38 in `backup::`).

**Found on the way, left for the next backup lane (lane B22's four, not touched here).**
- A leftover `-wal2`: a restore is refused where a `-wal` is left at its path, not where a `-wal2` is (restore.rs 162).
- The sends' segment: `Shipper::send` reads frames from the current segment's file, not the segment its generation is in.
- A round between the switch and the fence: `Db::switch_log` before `restart_log` tells the fence makes `end_base` fail ("ship the
  log's generation before ending a base in it"), and the base under way starts over.
- The disk estimate: `Checker::admitted` counts `-wal` and not `-wal2`.

**Commands.** `~/.cache/amphora-perf/pagepath-b23/` on this machine holds the first check script (`check.sh`) and this record's draft;
`~/.cache/amphora-perf/pagepath-b23/{check2.sh,check3.sh}` on adacachy2 hold the landing and post-landing scripts, and one folder of
logs a run (`linux-25e2c075`, `linux-861a3642`, `linux-b53a29b1`, `main-b53a29b1`, `main-980d7c76`).

## perf-B24 (2026-11-24, 07:39:40Z-08:39:40Z; lane B22's four backup suspects, each driven by a scenario)

**Losses first.**
- All four are real. One loses rows with no error (the sends' segment).
- A log started again under a send still breaks the archive's log there: the send ends whole, but the link after it is a
  gap (the fence's `forced`, as T-T1.12.P11 asserts), so seconds after it are refused until a base begun after it ends. The
  commits made between the send's drain and the switch are not shipped though the closing segment holds them.
- The forced restarts are made by the tests' store (`Db::restart_log` inside a put), not by a store thread under load, and the
  round within the switch by a hook compiled into the crate's own tests (`Fence::during_switch`). No run had the two threads
  meet there by chance.
- What an admitted check's restore may take is still an estimate from the source as it stands. A generation since the base that
  was longer than either segment's file is now (cut after a forced restart), or an older second with more pages than the
  database has now, takes more than is counted. Read in the code, not run.
- No benchmark was run. Each part sent reads 32 bytes more of the log, and each due check asks `PRAGMA page_count`: what that
  costs is not measured (rule 0: the deliverable is correctness).
- The engine's `storage:: vdbe::` suites were not run: no engine file changed here.
- The lane's own branch was force-pushed once, over its own commit b21a45a8 (the fixes before their message); the runs below that
  name b21a45a8 are of that tree.
- Looking for its own queued runs, this lane read the pending output of other lanes' runs in the same session (r1200, r1203,
  r1225 to r1232, r1247, r1248, r1261, r1263, r1266 to r1269): what it read of them is gone from their next read. Its start loop
  had taken a run still being placed for a refused one, and asked again.

**What each suspect was.** Red is main's code with the tests and the hook (17a18534; its restore test as 46e2480f has it); green is
the landed tree. Logs on adacachy2 in `~/.cache/amphora-perf/pagepath-b24/` (`red`, `arms1`, `final-8c4f29fa`).

| suspect | the scenario (test) | red: what the user lost | green |
|---|---|---|---|
| 1. a `-wal2` left at a restore's path | 20 rows and a base, a switch to `-wal2`, every row deleted a second later, the file and its `-wal` removed, `-wal2` left, the second before the delete restored there (`d45_a_restore_never_takes_a_log_segment_left_at_its_path`, and paced) | the restore ended in "Database is busy", and what stands at its path answers 0 of 20 rows: the delete recovered from `-wal2` | refused before it writes, as where a `-wal` is left; the segment removed, 20 of 20 |
| 2. a log started again under a send | 60 rows unshipped, shipped in 5 parts; as the first is stored the writer starts the log again past what it keeps and commits 80 (`d45_a_send_ships_its_own_generation_when_the_log_starts_again_under_it`) | the send said it ended; second 1044 restored with no error, 43 of its 44 rows (the first second that restored wrong: the test stops there) | the send ends from its own segment; every second of it restores with its rows |
| 2, twice | the same, the log started again twice, so the segment is written over (`d45_a_send_whose_segment_is_written_over_is_refused`) | the same 43 of 44, no error | the send is refused; no second restores with other rows than its own |
| 3. a round between the switch and the fence's word | a base under way, its last chunk copied by a round within the writer's restart (`d45_a_round_within_the_writers_switch_ends_the_base_under_way`) | the base refused its end ("ship the log's generation before ending a base in it") and was thrown away; the last commit's second is refused ("no base precedes second 1015 on an unbroken log") | the base ends there; the second restores, 15 of 15 rows |
| 4. the disk a check needs | 310 rows, 300 of them in `-wal2`; the check's room counted, its restore run and the room it takes measured at each step (`d45_a_check_begins_only_where_the_disk_holds_both_of_the_logs_segments`) | the restore took 1,861,288 bytes where 74,096 were counted (file 16,384, `-wal` 57,712; `-wal2` 1,693,352 left out) | counted above what it takes; the log's growth in both segments within the room left |

**The causes, and the fixes** (9de10ddd).
- *The restore* (`restore_within`): it refuses where the file or either segment is there, and names it. The engine recovers both
  segments at an open (decision 45), so a `-wal2` left is the restored log's next generation to it. Not the engine's fault: a
  database's files are its three, and the restore knew two. `db::log_paths` names the log's files once, for the restore, the
  check's scratch and the disk a check needs. A check's scratch files are removed with both segments
  (`d45_a_check_clears_the_segments_left_at_its_scratch_path`): the refusal would else fail each check after one is left there.
- *The send* (`Shipper::send`): it read each part from the segment current then (`Db::read_log`). It reads the segment its
  generation's header was in (`Db::log_header` answers both), and after each part's frames the segment's header again: a
  generation's header is synced before any frame of it (decision 13; `prepare_wal_finish`), so the generation's own header,
  still there, says the frames are its own. Else the send is refused before the part is put, and the log is broken there.
- *The fence* (`Fence::restart`): the switch was seen by a round before the fence was told how the generation ended. The fence is
  told first, beside what it was told of the restart before (a round that has not seen the switch asks about that one), and
  takes it back where no switch is made (`d45_the_fence_is_told_before_the_switch`).
- *The room* (`Checker::budget`): the database's pages, those only its log holds too (`PRAGMA page_count`; the file alone and both
  segments are still 93,840 bytes short of what the restore took, red arm 2), and both segments' files. The room left for the
  log's growth is what it keeps in each segment, as its peak on disk is two segment files (store.md, decision 45).
- backup.md restated: the fence, a send's segment, a restore's path, a check's room.

**What landed.** Main ecaee231 at 08:18:07Z, a fast-forward from b4ad7122: 17a18534 and 46e2480f (the tests and the hook), 9de10ddd (the
fixes), 8c4f29fa (the fence's own test and comments), and the merges of main 67cc7e69 (eb0c360d) and b4ad7122 (ecaee231).

**Checks** (adacachy2, `on` it, needs as named; the clone is `~/.cache/amphora-perf/pagepath-remote`, target
`~/.cache/amphora-perf-target/pagepath`, CARGO_INCREMENTAL=0, nice 10; the script is `~/.cache/amphora-perf/pagepath-b24/check.sh` there).

| run | tree | needs, load | result |
|---|---|---|---|
| red | 17a18534 (main a3f69b26 plus the tests) | `cpus=4,mem=12g`, 29 to 33, 07:51Z to 07:52Z | 3 of 3 of tests/backup.rs's new tests and 2 of 2 of the crate's fail, as the table above says |
| arms1 | 46e2480f, then b21a45a8 | `cpus=6,mem=16g`, 28 to 33, 07:55Z | 46e2480f: 4 of 4 fail (the restore test now reads what stands at its path, and runs paced too); b21a45a8: tests/backup.rs `d45_` 7 passed, `backup::` 40 passed |
| r1101 | eb0c360d (main 67cc7e69 merged) | `cpus=8,mem=24g`, 34 to 75, 07:58Z to 08:04Z | all ten steps exit 0: fmt; `backup::` 40 passed; tests/backup.rs 29 passed, 0 failed, 1 ignored (the benchmark) in 31 s; tests/store.rs 17; power cut 1 (its 200 seeds); engine `d45_` 23; clippy 49 s ("Checking amphora-engine"); check-production 91 s ("Compiling amphora-engine" twice, "10617 left ... as the ratchet holds") |
| r1145 | 8c4f29fa (eb0c360d plus the fence's test) | `cpus=4,mem=12g`, 49 to 58, 08:05Z to 08:09Z | all exit 0: `backup::` 41 passed; tests/backup.rs 29 passed, 0 failed, 1 ignored in 36 s; store 17; power cut 1; `d45_` 23; clippy 41 s; check-production 62 s, 10617 |
| r1199 | ecaee231, landed (main b4ad7122 merged) | `cpus=2,mem=8g`, 64 to 79, 08:11Z to 08:18Z | all exit 0: fmt; `backup::` 41 passed; tests/backup.rs 29 passed, 0 failed, 1 ignored in 45 s; store 17; power cut 1; `d45_` 23; clippy `-p amphora -p amphora-engine --all-targets --locked -D warnings` 81 s ("Checking amphora-engine"); check-production 76 s ("Compiling amphora-engine" twice, 10617) |

| after | 4931c0b1, main itself with lanes A26 and C7 landed on top | `cpus=4,mem=12g`, 46 to 57, 08:28Z to 08:30Z | fmt 0; `backup::` 41 passed; tests/backup.rs 29 passed, 0 failed, 1 ignored in 36 s; store 17; power cut 1 (clippy and check-production not run again) |

Refused for room: `cpus=8` at 08:05Z and 08:10Z, then 4, 3 and 2 until 08:11Z (0 to 3 of 32 cores idle).

**Each assertion is red without its part of the fix** (r1265, adacachy2, `cpus=2,mem=8g`, load 52 to 67, 08:21Z to 08:27Z, on the landed
tree; `perf/pagepath-b24-red2`, never for main).

| arm | what is taken out | red |
|---|---|---|
| 86b6bbb2 | the log's growth in the second segment | the log grew by 4,853,360 bytes where the check leaves 3,296,000 |
| 4b5581b4 | the pages only the log holds (the file and both segments counted) | the restore took 1,861,288 bytes where 1,767,448 were counted |
| 1af38127 | a check's scratch `-wal2` removed | a check of a whole archive failed: "check.db-wal2 exists: a restore makes a new database" |
| 1af38127 | the header read after the frames (the generation's segment still read) | second 1044 restored with no error, 43 of 44 rows, the log started again twice in the send |

**Suspect 4 on a real small disk** (`unshare --user --map-root-user --mount`, a tmpfs of 3,571,712 bytes holding the database and its
check's scratch, 1,798,144 bytes free once the log is in `-wal2`; adacachy2, `cpus=2,mem=8g`, load 49 to 50, 08:26Z; arms adf7c309 and
aa110114, the landed tree with `small_disk` added, never for main).

| admission | the check | the database |
|---|---|---|
| main b4ad7122's (file and `-wal`, one segment's growth) | counted 74,096 and 1,648,000: it begins; its restore ends in "no storage space", 0 bytes left | its next commit fails: "I/O error (pwrite): no storage space" |
| landed (pages, both segments, both segments' growth) | counted 1,919,000 and 3,296,000: it waits | its next 100 commits hold |

The first try gave the disk 3,653,632 bytes (1,880,064 free): the restore's 1,861,288 fitted and both arms' commits held (r1265).

**What the room costs a user.** A default database keeps 262,144 frames a segment, so a check now begins only with 2,160,066,560 bytes
free beside its restore (1,080,033,280 before): on a disk between the two, checks that ran are deferred, counted `disk`.

**Found on the way, left (read in the code, no scenario run).**
- A base begun before a forced restart and ended after it marks the archive mended (`end_base` sets `broken` false), though no
  restore takes a base begun before a gap (`plan::chain`): no new base is due until the log outweighs it, and the seconds after the
  break are refused until then.
- A shipper opened after a switch reads the current segment's header alone (`Shipper::open_within`): a process that ended between
  the switch and the next round opens on a gap, though the closing segment was shipped whole, and needs a new base.
- A generation the writer ended while a send of it was under way keeps a gap after it even where that send ships its last frame
  (`Fence::ended_whole` refuses it by `forced`, which T-T1.12.P11 asserts).
- A restore doesn't compare the frames its replay recovered with the frames it wrote (`replay`): an archive that already holds
  another generation's frames (shipped before this fix) restores short with no error.
- A new database opened at a path with a `-wal2` left and no file: the engine's rule for an orphan log covers `-wal` alone
  (database.rs, "discarding WAL"), then recovers `-wal2`.

**Commands.** `~/.cache/amphora-perf/pagepath-b24/` on this machine holds the focused script (`run.sh`) and this record's draft; on
adacachy2 the same folder holds `check.sh`, `arms2.sh` and one folder of logs a run (`red`, `arms1`, `merged-eb0c360d`, `final-8c4f29fa`,
`landing-ecaee231`, `arms2`, `arms3`).

## perf-B25 (2026-11-24, 08:31:32Z-09:31:32Z; lane B24's five backup notes, each driven by a scenario)

**Losses first.**
- Note 5 is real in one of the five ways tried: a pass of the checkpointer over a new database, before its first write. Opened and
  closed (three times), grown over the segment's pages (400 rows, its log copied on the way), and backed up (every second restored), a new
  database was its own on main too. The fix covers all; only that one way was shown wrong.
- A base an archive holds from before lane B22's fix (its file short of the closing segment's frames: 20 of 41 rows gone with no
  error, B22) still restores short: no frame and no manifest says what a base's file held, so a restore can't tell it. Read in the code, not run.
- A restore refused for another reason part of the way (the store's error, a deadline) still leaves its files, which open as a
  database as of the generation it reached. Only a short one's are removed (a check keeps the others "to look into"). Read in the code.
- A failed check of a short archive begins no base: what mends it is the gap that follows a short generation in an archive of a
  shipper before B24, which makes a base due. Read in the code.
- Left gapped, refused typed: the commits made between a send's drain and a forced switch, and an opening on a closed segment the
  archive holds short (tested). Both could be shipped from the closing segment; neither is one of the five notes.
- A restore's refusal now removes the files a short check would have kept in its scratch: what is left to look into is the refusal.
- No benchmark was run (rule 0). An opening reads 32 bytes more of the log, a restore compares two numbers a generation.
- The branch holds a WIP message (cefdfbb1); main's history does not: the landing commit is the same tree (e07ca62f), checked as 84cfb3d6.
- perf/pagepath-b25-red3 holds the lane's red arms and scripts (b25/), never for main.

**What each note was.** Red is main's code with the tests (be013de6's sources, 3e028a02, over main 1a468eeb); green is the landed tree. Logs
on adacachy2 in `~/.cache/amphora-perf/pagepath-b25/`.

| note | the scenario (test) | red: what the user lost | green |
|---|---|---|---|
| 1, an archive of a shipper before B24 (46e2480f, whose scenario's folder was kept: `old-archives/r1`, `r2`) | every second restored from the first base's on (`old_read.py`) | r1: **47 seconds short with no error** (second 1044: 43 rows of 44; second 1090: 43 of 90), 41 whole, 79 refused (no base); r2: the same 47 short | r1 and r2: 41 whole, 0 short, 47 refused, typed: "frames 30 to 30 of generation 3 of the log, which the archive says it holds, don't replay: its frames end at 29" |
| 1, the same made from today's archive | 60 rows in a generation of several parts, each part after the first made the next generation's frames, sealed (`a_restore_of_an_archive_short_of_its_frames_is_refused`) | second 1022 restored with no error, 21 of its 22 rows | every second to the last whole frame restores, each after it refused `Short` |
| 1, frames lost | its last two frames zeroed (`a_restore_of_a_truncated_generation_is_refused`) | second 1069: 68 of 69 rows, no error | refused `Short` |
| 1, a part shorter than its key | (`a_restore_of_a_part_shorter_than_its_key_is_refused`) | refused, untyped: "holds fewer frames than it says" | refused `Short` |
| 1, what a refused restore leaves | a last frame zeroed, then the base's second restored at the same path (`a_short_restore_leaves_no_database`) | no error; what stands at the path answers 19 of 20 rows | refused, no file left; the second restore holds its rows |
| 2, a base under way as the log breaks | rounds as the shipping thread runs them (`a_base_begun_before_the_log_broke_is_begun_again`), and `end_base` itself (`a_base_begun_before_a_break_is_refused_its_end`) | the last commit's second refused ("no base precedes second 1072"), no base begun; `end_base` Ok, no base needed | begun again in the same round; `end_base` refuses it |
| 2, an archive holding one | a base's manifest begun before a break and ended in the generation after it, opened again (`a_shipper_opened_on_a_base_begun_before_a_break_begins_another`) | "no base precedes second 1073" | a base begins; the second restores |
| 3, a process that ended between a switch and the next round | 45 rows shipped whole, the writer switches, 5 rows, the files as they stand opened (`d45_a_shipper_opened_after_a_switch_carries_on_from_the_closed_segment`) | 11 of 11 seconds after the opening refused, none wrong | 11 of 11 restore, no base ended |
| 3, its other case | the switch forced with 25 rows unshipped (`d45_a_shipper_opened_after_a_forced_switch_is_past_a_break`) | not run on main's code, where every opening after a switch was a gap | refused to the next base's end, then whole; none wrong |
| 4, a generation force-ended under a send | B24's scenario with no base after (`d45_a_generation_ended_by_force_is_whole_once_its_send_ships_it`) | 86 seconds refused (1091 to 1176) | 0 refused, every second with its rows |
| 5, a checkpoint pass over a new database where a `-wal2` is left (289 frames of a removed database) | (`d45_a_new_databases_checkpoint_copies_no_log_segment_left_at_its_path`, engine `d45_open_discards_a_second_segment_beside_a_database_with_no_pages`) | the new database holds the removed one's tables (`removed`, `t`); the engine opens on the second segment | it is empty; the segment's file is gone |

**The causes, and the fixes** (e69221de; its commit message says each in full).
- *Note 1* (`restore.rs`): the replay compared nothing. The frames carry their proof (salts, running sums), and the engine's recovery keeps
  those that hold, so the frames it recovered (`Db::log_end`) are compared with those the keys gave, before they are folded.
- *Note 2* (`ship.rs`): `broken`, a flag `end_base` cleared whatever the base, is the two generations it stood for.
- *Note 3* (`ship.rs`, `backup.rs`, engine `Database::wal_before`): the opening looks in both segments and tells the fence.
- *Note 4* (`backup.rs`): `ended_whole` asks what was shipped since a forced restart. T-T1.12.P11's assertions are unchanged.
- *Note 5* (engine `Database::open_second_segment`): the orphan rule, for `-wal2`.
- Old archives: nothing new is written, so one is not migrated: whole, it restores as it did; short, it is refused and says which frames.
  An older shipper's base begun before a break is in an archive still, and no restore takes it.

**What landed.** Main e69221de at 09:09:24Z, a fast-forward from 2b713d77: 822901a2 and ba31e62d (the first red tests), fa05a15a (the merge of
main 2b713d77), e69221de (the fixes, tests and docs); then 77f1e18f at 09:12:48Z (one test file). Main did not move under either push.

**Checks** (adacachy2, `on` it, needs as named; the clone is `~/.cache/amphora-perf/pagepath-remote`, target
`~/.cache/amphora-perf-target/pagepath`, CARGO_INCREMENTAL=0, nice 10; the scripts are `b25/` on perf/pagepath-b25-red3).

| run | tree | needs, load | result |
|---|---|---|---|
| r1338 | 822901a2 (main 1a468eeb plus the first tests) | `cpus=4,mem=12g`, 24 to 47, 08:41Z | 5 of 8 of tests/backup.rs's fail and 2 of 4 of `backup::`'s, as the table above says (note 1's, 3's, 4's; 2's); note 5's first scenario passes |
| r1347 | ba31e62d, then cefdfbb1 (the fixes) | `cpus=4,mem=12g`, 43 to 55, 08:44Z to 08:46Z | ba31e62d: the same 5 and 2 fail; cefdfbb1: 8 and 4 passed |
| r1352, r1362 | e218c7ad and ccfd9441, then c1e73c06 and ec22cc0d | `cpus=4` and `cpus=6`, 25 to 45, 08:47Z to 08:52Z | note 5's scenarios: three pass on main's code (opened and closed, grown over, backed up), one fails (the checkpoint pass); 4 of 4 with the fix |
| r1374 | aedd4bb6 (main 2b713d77 merged) | `cpus=8,mem=24g`, 24 to 36, 08:54Z to 08:58Z | nine of ten steps exit 0; check-production 1: `clippy::disallowed_methods` 41 above 40, a `then_some` in `wal_before` (fixed in 84cfb3d6) |
| r1397 | 46e2480f (writes the old archives), 3e028a02 (main's code, every scenario test), 84cfb3d6 | `cpus=5,mem=16g`, 32 to 36, 09:00Z to 09:04Z | as the table above says: 47 and 47 seconds short with no error on main's code, none with the fix; on main's code 8 of the 12 of tests/backup.rs's run fail (the other 4: note 5's three and a test of lane B22's), 3 of 3 of `backup::`'s and 1 of 1 of the engine's |
| r1409 | 84cfb3d6, whose tree (e07ca62f) is e69221de's, landed | `cpus=8,mem=20g`, 34 to 79, 09:04Z to 09:09Z | all ten steps exit 0: fmt; `backup::` 45 passed; tests/backup.rs 40 passed, 0 failed, 1 ignored (the benchmark) in 54 s, the drill among them; tests/store.rs 17; the power cut 1 (its 200 seeds); engine `d45_` 24; clippy `-p amphora -p amphora-engine --all-targets --locked -D warnings` 63 s ("Checking amphora-engine"); check-production 64 s ("Compiling amphora-engine" twice, "10617 left ... as the ratchet holds") |
| r1442 | 77f1e18f, main itself | `cpus=8,mem=20g`, 44 to 71, 09:09Z to 09:19Z | tests/backup.rs 41 passed, 0 failed, 1 ignored in 30 s; clippy `-p amphora --all-targets --locked -D warnings` 0; the mutants below |
| r1535 | 9b0a2ad7, main itself with lane A27 landed on top | `cpus=2,mem=8g`, 51 to 55, 09:21Z to 09:23Z | `backup::` 45 passed; tests/backup.rs 41 passed, 0 failed, 1 ignored in 53 s (clippy, check-production and tests/store.rs not run again) |

`cargo fmt --all --check` 0 on each tree pushed. Refused for room: `cpus=8` at 08:36Z and 09:00Z, `cpus=6` at 09:19Z (3, 5 and 3 of 32 cores idle). Of the engine's
other suites only `storage::wal::` and `storage::log_segments::` were run (r1521, 77f1e18f, `cpus=3,mem=12g`, load 52 to 57, 09:20Z to
09:21Z: 129 passed, 0 failed, 1 ignored): the engine changes are the open's second segment and one reader of the log's state.

**Each part of the fix is red when taken out** (r1442, on 77f1e18f, `mutants.py`; `--lib backup::` and the new tests of tests/backup.rs).

| arm | what is taken out | red |
|---|---|---|
| m1 | a short restore's files are removed | `a_short_restore_leaves_no_database` |
| m2 | what was replayed compared with what was written | `a_short_restore_...`, `a_restore_of_an_archive_short_...`, `a_restore_of_a_truncated_...` |
| m3 | `end_base`'s refusal | `a_base_begun_before_a_break_is_refused_its_end` |
| m4 | the round's drop of a base the log broke under | `a_base_begun_before_the_log_broke_is_begun_again` |
| m5 | an opening's count of a base from its beginning (its end instead) | `a_shipper_opened_on_a_base_begun_before_a_break_begins_another` |
| m6 | the current generation's unshipped frames, counted from nothing at an opening on the closed one | `d45_a_shipper_opened_after_a_switch_...` |
| m8 | what was shipped since a forced restart counted | `d45_a_generation_ended_by_force_is_whole_once_its_send_ships_it`, `..._once_shipped` |
| m9 | the fence told where a log is opened | `d45_a_shipper_opened_after_a_switch_...` |
| m10 | that only where its writer made no restart since | `d45_a_fence_told_where_a_log_is_opened` |

Note 5's fix has its red arm above (c1e73c06, 3e028a02: the checkpoint's test and the engine's).

**Found on the way, left (read in the code, no scenario run, unless said).**
- A restore refused midway for another reason than `Short` leaves files that open as a database of an earlier generation's end.
- A failed check begins no base; the archive restores no second after its short generation until one is due another way.
- The closing segment's frames the archive lacks after a forced switch, or at an opening, could be shipped from it (tested: refused, never wrong).
- A base of a shipper before lane B22's fix can't be told from a whole one (above); a base's manifest could say its file held the closing segment.

**Commands.** `~/.cache/amphora-perf/pagepath-b25/` on this machine holds the scripts and this record's draft; on adacachy2 the same folder holds
`check.sh`, `focus.sh`, `evidence.sh`, `mutants.{py,sh}`, `old_write.py`, `old_read.py`, the two old archives, and one folder of logs a run
(`red`, `red2`, `green1` to `green3`, `red3`, `red4`, `merged-aedd4bb6`, `landing-84cfb3d6`) and the evidence's and mutants' logs beside them.

## perf-B26 (2026-11-24, 09:24:29Z-10:24:29Z; lane B25's three backup notes: a short base, a failed restore's files, a failed check)

**Losses first.**
- Bases made before this lane restore nothing. A manifest of format 1 or 2 doesn't say what its file held, so every second whose base is
  one is refused until a new, whole base ends (the lead's decision; Amphora is unreleased). The first opening after an upgrade begins one.
- A killed restore's files stay until someone removes them. Its path is refused meanwhile, by an open and by a restore, naming them: they may be
  another process's restore under way, and nothing tells the two apart.
- What is left is refused by its name (`<database>.restoring`), by Amphora's opens. Its bytes are a database's: renamed by hand, or opened by another
  program, it opens as an older one. No database can take a name that ends so.
- A failed check begins a base only where its restore was refused for frames of a generation (`Short`, `Gap`) or for a base's proof (`Unproven`).
  A sum that fails, an object that is gone, `NoBase`, a failed integrity check and rows that differ begin none, as before.
- The bound on those bases is one for the same log's end. A store that spoils every copy, under a database that commits, gets a whole base for each
  check that fails.
- The log shipped again costs its bytes again (at most what the log keeps) beside the whole base. The second the check failed at, and those
  before it in the short generation, stay refused.
- A cut across groups that fails after its groups are at their paths (the coordinator's finishing) leaves them: each is whole at the second, and the next
  coordinator finishes it. A kill between two of the cut's last renames leaves the first group's alone; the others' paths are refused.
- The first landing (507ba0f6) had no test that the base a failed check begins is whole, and none that told the check's wake from a
  commit's: with either taken out (m14, m9) every test passed. bc008095 (main at 10:10:41Z, test code only) has both, each red on its mutant. Its
  message says the first "passed vacuously as landed"; that one was on the branch (04087dab), never on main.
- The two 60 s waits of the red arms end at the test's deadline: that is what "no base began" looks like there.
- The file system with no hard links (a rename then) was not run. Windows was not run or linted. macOS ran the suite once (r1664), no clippy.
- No benchmark was run (rule 0). An opening reads nothing more; a restore reads no object more; a database's open looks one name up, two where a restore's file is there.
- perf/pagepath-b26's history holds WIP messages; main's doesn't: 507ba0f6 is f46a2d7f's tree. perf/pagepath-b26-red holds the red arm and the scripts (`b26/`), never for main.

**What each note was.** Red is main's code (07d95147) with the lane's tests and the types they name (0429fe9c); green is f46a2d7f, whose tree is
507ba0f6's, or 04087dab, its tests alone changed. Logs on adacachy2 in `~/.cache/amphora-perf/pagepath-b26/`.

| note | the scenario | red: what the user got | green |
|---|---|---|---|
| 1, a base made before lane B22's fix | the drill: the archive and files that tree wrote (ba250493: 41 rows, a read keeping the closing segment back; `old_write.py`), each second restored, then opened again backed up to it, a base ended, 3 rows more (`old_read.py`) | **20 of 41 rows, no error** (seconds 1041, 1042); opened again, no base needed | refused, typed: "base 1041000000002 can't be shown to restore: its manifest, of an earlier format, doesn't say its file holds the log before generation 2, the one it began in"; a base needed, 0 chunks kept, seconds 1047 to 1050 whole |
| 1, the same made by the test | `a_base_that_does_not_say_what_its_file_holds_is_refused` | 20 of 41 rows, no error | refused `Unproven`, `Unsaid`; a whole base restores every second; `expire` lets the old one go |
| 1, a manifest and its log that disagree (no such manifest on main) | `a_base_whose_manifest_and_log_disagree_is_refused`: all but 5 of the closing segment's frames in the file; 5 fewer in it; none; another generation | with the comparison taken out (m1, m2): 30 of 30 rows, no error | each refused, typed, naming it (`Uncopied`, `Before` twice, `Own`), no file left |
| 1, a shipper that begins a base too early | `d45_a_base_begun_while_a_read_holds_...` with the shipper's checks taken out (m3) | main before B22: 21 of 41 rows lost | refused by the restore: "its file held 25 of the 48 frames of -wal, the segment closing as it began (the log's generation 0): frames 26 to 48 are missing" |
| 2, an error mid-restore | a restore of a base and four generations failing at each of its 38 steps | **35 left a database** of 10 to 49 of its 50 rows | 0 |
| 2, a read of the store refused | each of its 14 reads | **11 left a database** of 5 to 49 rows | 0 |
| 2, a kill between two writes | a process killed at each of the 38 steps (`a_killed_restore_leaves_no_database`, and the survey) | **35 left a database** of 10 to 49 rows | 0: the files left are refused by name, the path too; removed, the restore is whole |
| 2, a full disk | a tmpfs of 2 MiB, so much of it left free | 10 amounts, 8,192 to 118,784 bytes free ("No space left on device"; at the last, the engine's own write): **9 left a database** of 1 to 20 rows | 0 of 10, no file left; 131,072 free: restored, 50 of 50 rows (both arms) |
| 2, the same in the suite | `a_restore_a_full_disk_cuts_short_leaves_no_database`: a file that can't grow (`ulimit -f`) | files left at the first limit ("File too large") | no file left at any limit |
| 2, a missing frame | in `a_restore_that_fails_leaves_no_database`, and lane B25's `a_short_restore_leaves_no_database` | refused, no file (lane B25's) | refused, no file |
| 2, a cut across groups | `a_cut_one_group_fails_leaves_no_database` | the first group's database, 50 rows | no file |
| 3, a failed check | `a_failed_check_of_a_short_archive_begins_a_base`: its last frame zeroed, sealed again, under a database that then commits nothing | none in 60 s, 112 rounds run | a base begun in the next generation ends |
| 3, the check's own wake | `a_check_that_fails_wakes_the_round_that_begins_a_base`: the threads' wakes are the test's, one made before the check, none after | (no such code) with the wake taken out (m9): the check failed, no base in 60 s, 3 rounds run | ends |

**The causes, and the fixes** (507ba0f6; its message says each in full; docs/design/backup.md restated).
- *Note 1* (`object.rs` `Held`, `ship.rs` `begin_base`, `restore.rs`, engine `Database::wal_before`): the manifest's format 3 says the generation the base
  began in and the closing segment (file, generation, frames, frames copied); the restore compares it with the archive's link and header first.
  The shipper refuses to begin a base it would refuse. A generation that isn't the base's own asks for the log shipped again, as which of the two is wrong isn't known.
- *Note 2* (`restore.rs` `Making`, `db.rs` `Unrestored`): the file is made aside, linked at its path last. `Db::open` made an empty file at the path before it
  looked (`in_place`), so the path's own refusal stands before it (found by the kill test, r1608).
- *Note 3* (`shipping.rs` `Mends`, `ship.rs` `mend`): a base begun in the short generation would replay it, so that generation is shipped again, whole,
  as one of the archive's own; taken out (m10) the thread begins and drops bases without end (1,927,592 rounds in the 60 s wait).

**What landed.** Main 507ba0f6 at 09:55:22Z, a fast-forward from 07d95147 (main had moved from 925c73fb, where the lane began; merged at 09:46Z); then bc008095
at 10:10:41Z, a fast-forward from b0e68ba7 (lane A28 landed between the two; the tests were put on it and run there).

**Checks** (adacachy2, `on` it, needs as named; the clone is `~/.cache/amphora-perf/pagepath-remote`, target
`~/.cache/amphora-perf-target/pagepath`, CARGO_INCREMENTAL=0, nice 10; the scripts are `b26/` on perf/pagepath-b26-red).

| run | tree | needs, load | result |
|---|---|---|---|
| r1598 | 3e08f36d (main 925c73fb plus the change, before its tests) | `cpus=4,mem=12g`, 30 to 41, 09:42Z to 09:44Z | tests/backup.rs 41 passed, 0 failed, 1 ignored; `backup::` 46 |
| r1608 | d5092835 (with the tests) | `cpus=6,mem=16g`, 31 to 32, 09:45Z | 8 passed, 1 failed (`a_killed_restore_...`: the path opened; fixed in 55f5c333) |
| r1611 | 9d56159e (main 07d95147 merged) | `cpus=8,mem=20g`, 28 to 35, 09:47Z to 09:50Z | all eleven steps exit 0 |
| r1629 | f46a2d7f, whose tree is 507ba0f6's, landed | `cpus=8,mem=20g`, 27 to 72, 09:51Z to 09:55Z | all eleven steps exit 0: fmt; requirements; `backup::` 47 passed; tests/backup.rs 50 passed, 0 failed, 1 ignored (the benchmark) in 21.9 s, the drill among them; tests/store.rs 17; the power cut 1 (its 200 seeds); engine `d45_` 26; clippy `-p amphora -p amphora-engine --all-targets --locked -D warnings` 47 s ("Checking amphora-engine"); check-production 64 s ("Compiling amphora-engine" twice, "10616 left ... as the ratchet holds") |
| r1660 | ba250493, 0429fe9c, 04087dab | `cpus=8,mem=20g`, 51 to 81, 09:55Z to 09:59Z | the drill and the red arm, as above: on main's code 8 of the 9 tests fail (two of them, `a_base_whose_...` and `d45_a_base_begun_after_...`, as no manifest of format 3 is there to read); the ninth (`d45_a_check_clears_...`, a killed check's files) is red on m12 |
| r1683 | 0429fe9c, 04087dab | `cpus=4,mem=12g`, 53 to 84, 10:01Z to 10:07Z | the disk and the survey, as above (the disk drill at 3 amounts of room: 2 of 3 left a database, then 0 of 3; its own "more than 3 amounts" failed on both arms, so it was run again at finer steps, r1758); the mutants, below |
| r1721 | bc008095, main itself (main b0e68ba7 plus the two tests) | `cpus=4,mem=12g`, 44 to 51, 10:07Z to 10:12Z | fmt 0; `backup::` 48 passed; tests/backup.rs 50 passed, 0 failed, 1 ignored in 50.3 s; clippy `-p amphora --all-targets --locked -D warnings` 0 ("Checking amphora"; the engine was not touched); m9 and m14 red (check-production, tests/store.rs, the power cut and `d45_` not run again: test code only) |
| r1758 | bc008095, 0429fe9c | `cpus=2,mem=8g`, 48 to 115, 10:13Z to 10:15Z | the disk drill at 10 amounts of room, and the survey again, as above |
| r1664 | 04087dab | MacBook Pro 2 (macOS, `cpus=5,mem=12g`), 31 to 46, 09:59Z to 10:11Z | tests/backup.rs 50 passed, 0 failed, 1 ignored in 157.9 s; `backup::` 47 passed in 32.9 s |

Refused for room: `cpus=8` at 10:00Z (2 of 32 cores idle), `cpus=6` at 10:07Z (5), `cpus=4` at 10:12Z (2). r1678 was stopped by this lane within a minute of its start (its mutants ran one by one; r1683 runs them in sets).

**Each part of the fix is red when taken out** (r1683 on 04087dab in three sets, parts whose tests are apart; m9 and m14 again in r1721 on bc008095; `mutants.py`).

| arm | what is taken out | red |
|---|---|---|
| m1 | a restore's comparison of what the manifest says | `a_base_whose_manifest_and_log_disagree_is_refused` |
| m2 | its own generation's comparison | the same |
| m3 | the shipper's checks at a base's beginning | `d45_a_base_begun_while_a_read_holds_...` (refused by the restore), `d45_a_base_begun_after_an_opening_...` |
| m4 | a failed restore's files removed | `a_restore_that_fails_...`, `a_restore_a_full_disk_...`, `a_short_restore_...` |
| m5 | the database made aside | `a_killed_restore_leaves_no_database` |
| m6 | its path refused | the same |
| m7 | its name refused | the same ("restored.db.restoring opens, 10 rows", killed at step 3) |
| m8 | the check's ask | `a_failed_check_of_a_short_archive_begins_a_base` |
| m9 | the ask's wake | 507ba0f6's tests: none (a commit's wake came after the check); bc008095's: `a_check_that_fails_wakes_...` |
| m10 | the log shipped again | `a_failed_check_of_a_short_archive_begins_a_base` |
| m11 | a cut's groups made before any is put at its path | `a_cut_one_group_fails_leaves_no_database` |
| m12 | a killed check's files cleared | `d45_a_check_clears_the_segments_left_at_its_scratch_path` |
| m13 | one base for the same log's end | `a_check_that_finds_the_archive_lacking_...` |
| m14 | the base for it whole | 04087dab's tests: none; bc008095's: the same test (7 chunks, 6 of another base's) |

**Found on the way, left (read in the code, no scenario run).**
- A shipper opened on an archive marks the pages an incremental base must copy from its segments' frames without checking they are the generation's own.
  Where one is short (sealed, so a shipper's fault; those known were followed by a gap, which makes the base whole), the next incremental base would
  keep a chunk those frames changed, and its manifest's proof wouldn't say so.
- A check that fails for a sum, a missing object, its integrity or its rows begins no base (above).
- Still gapped, refused typed, as lane B25 left it: commits between a send's drain and a forced switch, and an opening on a closed segment the archive holds short.

**Commands.** `~/.cache/amphora-perf/pagepath-b26/` on this machine holds the scripts and this record's draft; on adacachy2 the same folder holds them, the old base (`old-base`)
and one folder of logs a run (`wip1`, `wip2`, `merged-9d56159e`, `final-f46a2d7f`, `mutants`, `last-land`) beside the evidence's logs.
