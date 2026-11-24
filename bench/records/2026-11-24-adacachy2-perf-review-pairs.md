# Paired release benchmarks on adacachy2, November 24, 2026

All 150 primary runs exceeded the one-minute load limit. The historical versioned point-read arms measured **549.94× at one client and 112.66× at 64 clients** by the earlier record's ratio of medians. Eight-client Plain primary-key reads measured **0.98×**. These results leave the earlier 1,150×, 210× and 1.26× figures unverified under Amphora's release conditions. Slots delivered more throughput at both checkout client counts. Pool had lower p99.9 latency in all six pairs. The load limit leaves the checkout default undecided.

Kiln r105 used the reserved 03:00–03:45 UTC window. 162 of 162 scheduled arms produced complete, validated measurements. Of 81 scheduled pairs, **0** had two valid arms below load 2 throughout their sampled intervals. Every run has its own load observations, threshold flag, CPU and I/O pressure, and outcome. [Per-run measurements](2026-11-24-adacachy2-perf-review-pairs.jsonl) · [Executed driver](2026-11-24-adacachy2-perf-review-run.py) · [Benchmark rules](../../CLAUDE.md#7-benchmarks-are-recorded)

## Versioned point reads

The timing gate opened after the reporter started its clock. All 251,616 requested values were checked. The default-storage controls used the same fixture and timing gate. The historical after arm includes CopyId and predates the durability fence in `b6cb6aa8`.

| Storage | Clients | Before reads/s | After reads/s | After / before medians | Median paired ratio | Pairs below load 2 | Best before / after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| versioned | 1 | 338 | 185,880 | 549.94× | 566.69× | 0/3 | 343 / 194,478 |
| versioned | 64 | 4,614 | 519,835 | 112.66× | 112.66× | 0/3 | 4,648 / 598,463 |
| default | 1 | 920,843 | 917,253 | 1.00× | 0.96× | 0/3 | 1,052,347 / 929,951 |
| default | 64 | 1,273,995 | 1,210,197 | 0.95× | 0.95× | 0/3 | 1,290,681 / 1,306,022 |

## New databases, reopens and calibration

The first four rows summarize five paired runs of 200 files per arm. Each value in the before and after columns is the median of the run medians. The CPU comparison started 25 fresh processes per arm. The first after process measured calibration and wrote one 2,362-byte record. The following 24 shared that cache. Their median first open was 706.7 µs. Their matching before processes took 30,964.6 µs.

| Measurement | Before µs | After µs | Before / after medians | Median paired ratio | Pairs below load 2 | Best before / after µs |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| New database | 360.9 | 294.7 | 1.22× | 1.26× | 0/5 | 344.9 / 280.3 |
| Reopen | 240.0 | 192.1 | 1.25× | 1.23× | 0/5 | 200.8 / 166.6 |
| First CREATE on new database | 8,335.7 | 8,420.1 | 0.99× | 1.00× | 0/5 | 8,232.0 / 8,218.9 |
| First CREATE on reopen | 14.2 | 11.1 | 1.28× | 1.23× | 0/5 | 10.7 / 9.7 |
| First fresh process, empty calibration cache | 55,704.8 | 88,037.7 | 0.63× | 0.63× | 0/1 | 55,704.8 / 88,037.7 |
| Following fresh processes, one shared calibration cache | 30,964.6 | 706.7 | 43.82× | 43.07× | 0/24 | 3,761.5 / 642.5 |

## Plain reads

The three runs per cell used the archived binaries that produced the earlier Plain comparison. The 60 measured cells returned 251,646,907 answers. Every cell reported 1.0 rows per operation, zero errors and zero retries. The independent fixture scan checked values outside the timed loop. The previous eight-client primary-key rate rose from 1.42M to 1.79M reads/s. In these three pairs the arm medians were 1.68M and 1.65M reads/s.

| Read | Clients | Before reads/s | After reads/s | After / before medians | Median paired ratio | Pairs below load 2 | Best before / after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Primary key | 1 | 336,059 | 333,301 | 0.99× | 0.99× | 0/3 | 344,323 / 336,066 |
| Primary key | 8 | 1,683,670 | 1,648,952 | 0.98× | 0.98× | 0/3 | 1,743,098 / 1,687,361 |
| Primary key | 32 | 974,141 | 934,846 | 0.96× | 0.95× | 0/3 | 983,368 / 983,724 |
| Primary key | 64 | 892,048 | 908,226 | 1.02× | 1.02× | 0/3 | 896,834 / 915,205 |
| Primary key | 128 | 836,012 | 844,354 | 1.01× | 0.99× | 0/3 | 855,857 / 849,299 |
| Secondary index | 1 | 277,109 | 279,246 | 1.01× | 1.01× | 0/3 | 282,903 / 282,028 |
| Secondary index | 8 | 1,249,230 | 1,220,980 | 0.98× | 0.94× | 0/3 | 1,298,599 / 1,315,847 |
| Secondary index | 32 | 914,659 | 929,072 | 1.02× | 0.99× | 0/3 | 942,192 / 930,891 |
| Secondary index | 64 | 768,478 | 804,727 | 1.05× | 1.03× | 0/3 | 806,295 / 818,395 |
| Secondary index | 128 | 610,557 | 639,278 | 1.05× | 1.03× | 0/3 | 634,144 / 655,645 |

## Memory admission

Room cost fell from **66,641.4 to 2,917.8 ns per call** at the arm medians. The best before and after costs were 66,409.4 and 2,913.8 ns. At 1,024 clients the arm medians were 3,376 and 78,119 completed calls/s. Across the entire ladder the before arm completed 809,029 requests and the after arm completed 3,797,608. All admission refusal and unexpected-error counters were zero.

| Clients | Before calls/s | After calls/s | Median paired ratio | Best calls/s before / after | Completed before / after | Refused before / after | Unexpected before / after | Pairs below load 2 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 5,061 | 33,933 | 6.70× | 5,105 / 35,464 | 45,544 / 308,663 | 0 / 0 | 0 / 0 | 0/3 |
| 4 | 16,626 | 70,964 | 4.37× | 16,884 / 72,667 | 149,050 / 637,477 | 0 / 0 | 0 / 0 | 0/3 |
| 16 | 30,444 | 79,584 | 2.62× | 30,465 / 81,541 | 274,024 / 720,219 | 0 / 0 | 0 / 0 | 0/3 |
| 64 | 23,837 | 78,950 | 3.32× | 24,692 / 79,127 | 216,710 / 709,859 | 0 / 0 | 0 / 0 | 0/3 |
| 256 | 10,356 | 79,384 | 7.67× | 10,560 / 79,413 | 93,038 / 713,946 | 0 / 0 | 0 / 0 | 0/3 |
| 1,024 | 3,376 | 78,119 | 23.08× | 3,393 / 78,120 | 30,663 / 707,444 | 0 / 0 | 0 / 0 | 0/3 |

## Reader checkout

The 12 checkout invocations completed 55,613,463 reads with zero errors and zero retries. At 32 clients, slots delivered 2.05× pool's throughput by the ratio of medians, compared with 1.735× in the earlier busy-host record. At 1,024 clients the ratio was 1.31×. Slots had a lower p50 at both counts and a lower p99 at 1,024. Pool had the lower p99.9 in each of the six matched pairs. Each percentile below is the median of the three run percentiles for that arm. Checkout runs began at loads of 10.77–15.06. Another quiet window is needed to choose a default.

| Clients | Checkout | Reads/s | p50 ms | p99 ms | p99.9 ms | Best reads/s | Pairs below load 2 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 32 | Pool | 898,854 | 0.020 | 0.190 | 0.364 | 974,455 | 0/3 |
| 32 | Slots | 1,846,231 | 0.009 | 0.256 | 0.568 | 1,858,751 | 0/3 |
| 1,024 | Pool | 431,428 | 1.040 | 14.080 | 26.112 | 434,826 | 0/3 |
| 1,024 | Slots | 566,871 | 0.030 | 12.288 | 35.840 | 571,135 | 0/3 |

## Load and pressure

The opening load was 25.50. The driver waited 157.72 seconds before running any benchmark, when load was still 15.46. The measured invocations spanned 03:02:37.725 to 03:43:33.760 UTC. Across all runs, 0 began below 2 and 162 reached 2 or more. Load and pressure include each invocation's setup, warm-up and measured work.

| Workload | Runs reaching load 2 / started | Sampled peak load range | CPU stall % median / max | I/O stall % median / max |
|---|---:|---:|---:|---:|
| First open across processes | 50/50 | 15.46–37.74 | 2.14 / 7.16 | 1.91 / 3.96 |
| New files and reopens | 10/10 | 15.67–37.74 | 1.99 / 4.49 | 3.09 / 3.33 |
| Point reads and controls | 24/24 | 18.79–35.52 | 3.61 / 8.70 | 0.90 / 5.51 |
| Plain reads | 60/60 | 21.67–65.27 | 42.82 / 73.88 | 0.47 / 2.72 |
| Memory admission | 6/6 | 35.40–65.27 | 7.87 / 12.01 | 0.46 / 0.50 |
| Pool and slots | 12/12 | 14.41–20.19 | 42.56 / 48.25 | 0.45 / 4.48 |

Pressure percentages are the share of elapsed time during which at least one task was stalled for CPU or I/O, calculated from kernel pressure-counter deltas. The raw record also includes full-stall counters and the start, end and peak of each ten-second pressure average. Load was sampled before process launch, approximately once per second while it ran, and at completion. The after arm always followed the matching before arm. Pool always preceded slots. No arms ran concurrently.

The tables give arm medians and the best observed run with all measurements retained. The median paired ratio is calculated within each repetition before taking the median. Rate ratios divide after by before. Time ratios divide before by after. The raw JSONL SHA-256 is `7bb76ca597e70c2c73228442e92ea00a2e7d377d295ff42210f623802e5be31c`. [Per-run measurements](2026-11-24-adacachy2-perf-review-pairs.jsonl)

## Workloads and binaries

All runs used adacachy2, an AMD Ryzen 9 9950X3D2 with 32 logical CPUs, Linux 7.3.3-1-cachyos, and release binaries at niceness 10. The eight rebuilt examples used rustc 1.99.1 and Cargo's locked dependency graph. Both sides of each example used the same harness source from `40d6acaa`. The archived suite binaries were copied byte for byte from the original lane's directory. Each JSONL entry names its binary, SHA-256, arguments and environment overrides.

| Workload | Before | After | Common harness or archive |
|---|---|---|---|
| Versioned and default point reads | `b817508b` | `cb47a7b5` | `versioned_point_reads.rs` blob `0f46485b` |
| New files and reopens | `b817508b` | `31720420` | `open_time.rs` blob `95f932e2`. The original `39aa5763` base added the example to `b817508b`. |
| First open across processes | `dcd6ce4c` | `ffa3dc88` | Same `open_time.rs` blob |
| Plain primary and secondary keys | `b817508b`, archive label `base` | `a02e4b20`, archive label `after2` | Original `bench-base` and `bench-after2` binaries from the Plain record |
| Memory budget and admission | `b817508b` | `2b23f1e0` | `admission.rs` blob `bec10785` |
| Pool and slots | `pool` | `slots` | One archived `bench-hdr-after` binary. The original record identifies merge `369141e8`. SHA-256 `84955f8953b7ff4d7e3fe12913f360bd9546599828d3b66c73a79b22ff2c403b`. |

The versioned after arm includes the CopyId change and predates the C3 durability fence. The Plain comparison also spans the WAL change. These pairs measure the historical binaries together with each corrected timing harness. A performance figure for current main requires a pair that includes its recovery and durability changes. [Versioned record](2026-11-23-adacachy2-perf-versioned.md) · [Page and checkout record](2026-11-23-adacachy2-perf-pagepath.md)

Each point-read process populated 10,000 rows on a private path. Clients prepared their statements before the timed start. Each client requested 4,200 values at one client or 262 values at 64 clients and checked the value for every key. Each cold-open invocation used its own empty directory on the same filesystem, opened 200 new files and reopened those same files. Each fresh-process pair shared the CPU family's initially empty calibration cache. Its first pair includes cache creation. [Cold-start record](2026-11-23-adacachy2-perf-coldstart.md)

Plain and checkout reused one fixture with 200,000 rows. Before timing, an independent scan of an immutable copy checked all primary keys, secondary keys, prices and names against the fixture generator, verified that all secondary keys were distinct, and confirmed the secondary index lookup plan. Each timed statement consumed its result set. The suite reported completions, rows per operation, errors and retries. The suite used one second of warm-up and counted answers completed in the following five seconds. It recorded latency to microsecond resolution in its histogram. Pool and slots used the default page path and changed only `AMPHORA_READ_CHECKOUT`. Plain used `AMPHORA_PAGE_PATH=plain`. [Suite implementation](../../bench/src/suite.rs)

Each admission arm first timed five rounds of 200,000 memory-budget reads and reported the lowest ns/call. One fresh database then served an empty closure to 1, 4, 16, 64, 256 and 1,024 clients. Each cell warmed for one second and counted requests started in the following three seconds through their final response. Its reported rate includes the time to drain those outstanding requests. Each raw cell gives the exact number completed, refused and unexpectedly failed. [Admission record](2026-11-23-adacachy2-perf-cross.md)
