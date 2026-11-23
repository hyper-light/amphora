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

## AUDIT 2.1 (bf23ccb1): header guard publishes only after a write; WAL locks padded. Default path (PagePath::Today, LOG_READ_INDEX=locked)

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
