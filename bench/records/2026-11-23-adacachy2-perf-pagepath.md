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

`.git/perf-ab5.sh` (rewritten): after = HEAD, base = HEAD's tree with only ae729b06's `crates/` diff
reversed; 5 rounds, arm order alternated, load recorded; output `ab5-<HHMM>.jsonl`, written once both
builds finish. Under load 90-136 the after build took 20m22s. Its kiln run is r211 on adacachy2.
