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
three failures are main's own or the load's; they were not run against main alone in this hour. The `storage::`
filter is wider than B6's (`storage::wal storage::log_index`): it takes btree's and versioned's tests too. Run r1180
(macOS, needs `macos/arm64,cpus=8,mem=24g`, rule 6's disk guard; cold) has the same clippy and storage tests on
926c3003 (perf/pagepath-d45 with main 945b633a merged); queued at 16:16Z.
