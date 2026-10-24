# Read-path profiles: point reads and scans, Amphora against SQLite (2026-10-23)

G1 steps 4 and 5 (kiln plan "G1 Ground truth"). Ten million rows of the suite's schema, loaded once
into each engine (`bench/src/suite.rs --reuse`), each statement's measured window profiled alone
(`AMPHORA_PERF_CTL`): 17 statements at 1 client and 5 at 32, Amphora and SQLite.

- host: adacachy, AMD Ryzen 9 9950X3D2 (16 cores, 32 threads, 192 MiB L3), Samsung 9100 PRO
  (btrfs), kernel 7.2.9-2-cachyos; a shared machine, load 33–85 during the session
  (2026-10-23T20:00:17-07:00 to 2026-10-23T20:15:34-07:00), so absolute rates are noisy (CLAUDE.md §7 asks a quiet machine).
- binaries: the bench at 41bba4e (Amphora's engine as committed in 4acff7d), built with frame
  pointers in Rust and in the bundled SQLite's C (`-C force-frame-pointers=yes`,
  `-fno-omit-frame-pointer`) so `perf record --call-graph fp` copies no stacks: none of the 44
  recordings lost a sample (a first session with DWARF call graphs lost 8–81% and was set aside).
- perf_event_paranoid is 2 here: perf counts and samples user mode only; each statement's kernel
  share comes from the process's own CPU accounting (`cpu_sys_s` / (`cpu_user_s` + `cpu_sys_s`)).
- counters: cycles, instructions, branches and misses, L1 data loads and misses (this session);
  dTLB and cache misses and AMD's PipelineL1 top-down metrics (the first session, same binaries'
  code, counts unaffected by its sampling losses), each divided by the statements its own run
  answered.
- Amphora's database kept its load's 7.5 GB log: its `wal_checkpoint(TRUNCATE)` copied the
  1,834,934 frames into the file but left the log whole (checkpoint sequence 0, every frame's salts
  valid), so reads consult a 1.8-million-frame index and read pages from the log. The v6 matrix's
  database is in the same state (its log is the same size): this is what Amphora's reads cost as
  measured there.

## Repeat test

`point_pk` at 1 client, Amphora, three runs in a row inside a bench job, both files warm:

| run | cycles/statement | instructions/statement | statements/s |
|---:|---:|---:|---:|
| 1 | 22,652 | 25,436 | 145,134 |
| 2 | 21,985 | 25,223 | 148,450 |
| 3 | 19,262 | 24,245 | 201,048 |

Instructions per statement repeat within 4.9%; cycles per statement vary
17.6% with what shares the cores (load 64–80), so comparisons below lean on
instructions and IPC, cycles beside.

## Counters per statement

| statement | clients | engine | statements/s | cycles/stmt | instructions/stmt | IPC | L1d misses/stmt | dTLB misses/stmt | cache misses/stmt | branch misses | frontend | backend | retiring | kernel share | bound by |
|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| blob_read | 1 | amphora | 319,854 | 13,098 | 20,914 | 1.60 | 242.7 | 11.42 | 199.3 | 0.58% | 35.0% | 30.2% | 14.0% | 28% | frontend |
| blob_read | 1 | sqlite | 399,903 | 5,626 | 9,713 | 1.73 | 140.6 | 33.70 | 731.8 | 1.13% | 69.8% | 13.6% | 6.3% | 56% | frontend |
| case_aggregate | 1 | amphora | 2,026 | 1,540,547 | 2,580,146 | 1.67 | 22,438.8 | 73.12 | 3,692.4 | 0.46% | 31.0% | 36.6% | 26.8% | 32% | backend stalls |
| case_aggregate | 1 | sqlite | 10,002 | 306,246 | 1,028,486 | 3.36 | 2,468.0 | 4.33 | 1,362.6 | 0.67% | 49.5% | 11.0% | 29.7% | 45% | frontend |
| count_distinct | 1 | amphora | 4 | 909,757,307 | 1,195,209,637 | 1.31 | 24,241,650.8 | 183,633.78 | 7,482,423.2 | 0.63% | 34.2% | 38.9% | 15.5% | 34% | backend stalls |
| count_distinct | 1 | sqlite | 8 | 354,248,245 | 591,528,427 | 1.67 | 10,645,536.6 | 23,237.00 | 2,565,458.7 | 1.71% | 39.9% | 22.3% | 25.0% | 52% | frontend |
| count_range | 1 | amphora | 1,221 | 3,782,165 | 10,168,830 | 2.69 | 37,783.2 | 154.79 | 11,084.3 | 0.18% | 24.7% | 29.4% | 36.7% | 15% | instructions |
| count_range | 1 | sqlite | 2,702 | 1,420,036 | 4,216,717 | 2.97 | 13,864.0 | 61.17 | 8,925.7 | 0.36% | 25.6% | 24.6% | 40.3% | 24% | instructions |
| filter_no_index | 1 | amphora | 1 | 6,363,967,020 | 10,742,530,188 | 1.69 | 77,181,951.3 | 974,945.00 | 37,019,772.5 | 0.21% | 27.9% | 40.6% | 20.8% | 33% | backend stalls |
| filter_no_index | 1 | sqlite | 1 | 2,152,031,996 | 5,709,164,769 | 2.65 | 27,193,739.0 | 72,183.29 | 9,150,109.1 | 0.61% | 21.2% | 31.1% | 40.9% | 61% | instructions |
| group_by_range | 1 | amphora | 142 | 20,655,069 | 34,779,363 | 1.68 | 383,320.3 | 694.72 | 33,321.5 | 0.23% | 24.4% | 39.8% | 28.5% | 33% | backend stalls |
| group_by_range | 1 | sqlite | 359 | 10,866,637 | 25,822,337 | 2.38 | 125,230.8 | 366.45 | 22,770.7 | 1.06% | 41.6% | 12.2% | 30.2% | 23% | frontend |
| in_list_10 | 1 | amphora | 27,645 | 135,090 | 182,496 | 1.35 | 2,372.8 | 25.71 | 428.5 | 0.76% | 33.2% | 41.0% | 19.3% | 36% | backend stalls |
| in_list_10 | 1 | sqlite | 54,598 | 40,679 | 68,647 | 1.69 | 1,333.8 | 4.97 | 217.2 | 1.50% | 44.8% | 18.0% | 24.6% | 60% | frontend |
| json_field | 1 | amphora | 204,977 | 20,182 | 28,663 | 1.42 | 448.1 | 3.52 | 53.0 | 0.51% | 39.0% | 24.2% | 16.0% | 29% | frontend |
| json_field | 1 | sqlite | 316,633 | 8,066 | 13,254 | 1.64 | 207.1 | 45.64 | 751.3 | 0.90% | 70.7% | 14.2% | 8.7% | 53% | frontend |
| order_by_sort | 1 | amphora | 5 | 801,842,842 | 1,166,574,828 | 1.45 | 16,182,183.8 | 67,592.03 | 1,808,994.1 | 0.44% | 37.3% | 37.1% | 23.4% | 36% | frontend |
| order_by_sort | 1 | sqlite | 10 | 191,092,720 | 379,037,677 | 1.98 | 4,748,662.6 | 12,668.50 | 939,175.0 | 1.00% | 33.2% | 25.0% | 30.8% | 65% | frontend |
| point_pk | 1 | amphora | 267,158 | 15,809 | 23,319 | 1.48 | 332.3 | 3.73 | 54.5 | 0.59% | 38.7% | 28.9% | 15.6% | 27% | frontend |
| point_pk | 1 | sqlite | 343,788 | 8,115 | 14,097 | 1.74 | 206.1 | 61.37 | 600.4 | 0.94% | 68.9% | 13.9% | 7.3% | 48% | frontend |
| point_pk_one_column | 1 | amphora | 279,981 | 14,945 | 20,995 | 1.40 | 331.6 | 5.07 | 49.4 | 0.61% | 36.1% | 33.2% | 15.8% | 29% | frontend |
| point_pk_one_column | 1 | sqlite | 408,714 | 5,861 | 9,412 | 1.61 | 148.4 | 39.49 | 811.4 | 1.16% | 69.9% | 14.6% | 6.1% | 55% | frontend |
| point_secondary | 1 | amphora | 158,530 | 26,403 | 37,530 | 1.42 | 576.5 | 11.54 | 127.0 | 0.63% | 33.0% | 38.1% | 15.4% | 28% | backend stalls |
| point_secondary | 1 | sqlite | 158,938 | 22,318 | 49,889 | 2.24 | 641.1 | 61.20 | 2,317.6 | 0.46% | 69.6% | 11.4% | 10.0% | 34% | frontend |
| point_text | 1 | amphora | 155,860 | 27,399 | 41,814 | 1.53 | 596.6 | 7.10 | 91.7 | 0.65% | 33.7% | 38.7% | 18.7% | 26% | backend stalls |
| point_text | 1 | sqlite | 158,681 | 22,154 | 47,630 | 2.15 | 750.7 | 59.21 | 1,771.7 | 0.50% | 69.7% | 13.0% | 9.9% | 34% | frontend |
| range_pk_100 | 1 | amphora | 29,716 | 135,769 | 242,435 | 1.79 | 2,383.4 | 16.43 | 508.4 | 0.32% | 34.5% | 30.4% | 19.4% | 30% | frontend |
| range_pk_100 | 1 | sqlite | 53,839 | 58,143 | 158,544 | 2.73 | 557.6 | 3.27 | 149.2 | 0.25% | 37.2% | 17.2% | 40.4% | 41% | instructions |
| table_count | 1 | amphora | 23 | 151,634,172 | 198,595,680 | 1.31 | 1,979,296.6 | 8,872.02 | 579,834.2 | 0.35% | 31.8% | 43.7% | 18.4% | 39% | backend stalls |
| table_count | 1 | sqlite | 86 | 10,911,637 | 21,456,129 | 1.97 | 363,674.7 | 386.36 | 176,262.7 | 0.78% | 44.1% | 20.5% | 31.8% | 81% | frontend |
| table_group_by | 1 | amphora | 0 | 10,551,482,588 | 26,348,198,202 | 2.50 | 174,217,420.5 | 668,971.50 | 25,710,159.5 | 0.12% | 28.7% | 32.7% | 24.3% | 27% | backend stalls |
| text_read | 1 | amphora | 294,576 | 14,165 | 21,138 | 1.49 | 276.1 | 9.84 | 147.6 | 0.59% | 35.9% | 31.3% | 13.0% | 28% | frontend |
| text_read | 1 | sqlite | 482,419 | 4,920 | 9,688 | 1.97 | 112.6 | 54.08 | 602.6 | 1.10% | 63.8% | 14.9% | 11.0% | 55% | frontend |
| point_pk | 32 | amphora | 409,656 | 178,744 | 28,591 | 0.16 | 768.0 | 24.85 | 315.5 | 0.98% | 37.7% | 33.9% | 3.0% | 12% | frontend |
| point_pk | 32 | sqlite | 164,258 | 49,686 | 15,631 | 0.31 | 714.5 | 39.39 | 312.7 | 3.07% | 54.8% | 21.3% | 8.6% | 90% | frontend |
| point_secondary | 32 | amphora | 390,139 | 235,863 | 41,508 | 0.18 | 1,130.6 | 44.48 | 504.4 | 1.04% | 34.5% | 37.1% | 3.2% | 11% | backend stalls |
| point_secondary | 32 | sqlite | 50,591 | 170,318 | 53,667 | 0.32 | 2,161.3 | 109.86 | 1,627.3 | 3.69% | 56.6% | 21.9% | 8.1% | 85% | frontend |
| range_pk_100 | 32 | amphora | 66,812 | 1,587,573 | 289,779 | 0.18 | 3,878.5 | 95.13 | 1,617.8 | 0.49% | 38.5% | 29.3% | 3.4% | 8% | frontend |
| range_pk_100 | 32 | sqlite | 33,671 | 246,673 | 166,802 | 0.68 | 2,953.2 | 223.88 | 1,598.0 | 1.26% | 52.0% | 27.1% | 9.9% | 88% | frontend |
| table_count | 32 | amphora | 35 | 3,190,867,334 | 310,577,853 | 0.10 | 5,619,707.7 | 163,455.79 | 2,693,585.6 | 0.89% | 34.1% | 38.8% | 2.0% | 8% | backend stalls |
| table_count | 32 | sqlite | 21 | 270,899,208 | 41,918,737 | 0.15 | 3,593,767.0 | 175,760.66 | 892,354.0 | 5.04% | 51.0% | 33.6% | 5.3% | 92% | frontend |

## Where the user cycles go (frame-pointer call graphs, leaf frames by part)

Parts come from `bench/prof_parts.py`'s symbol patterns (Amphora's Rust paths, SQLite's C
functions); "libc internals (no symbols)" is libc's stripped internal code on this system
(memcpy's variants, lock waits), "unresolved addresses" frames outside any mapped object.
Every profile below attributes at least 90.7% of its samples to named parts.

### blob_read at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| checksums | 4.1% | 0.0% |
| atomics and reference counts | 2.2% | 0.3% |
| I/O completion | 3.1% | 0.0% |
| write-ahead log: frame lookups and reads | 28.3% | 2.9% |
| page cache and pager | 19.2% | 4.6% |
| B-tree search | 13.1% | 11.4% |
| record decoding | 0.1% | 4.3% |
| interpreter | 10.1% | 7.0% |
| values and rows | 0.2% | 1.4% |
| statement and binding | 2.1% | 3.6% |
| store: reader checkout | 1.5% | 0.0% |
| locks | 1.6% | 32.2% |
| allocator | 2.2% | 2.3% |
| bench: timing and harness | 7.6% | 19.5% |
| libc internals (no symbols) | 1.8% | 1.6% |
| unresolved addresses | 1.7% | 6.5% |
| other | 0.8% | 2.4% |
| (user samples) | 20,231,551,558 | 11,104,887,748 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 27% | 57% |
| statements a second in the profiled run | 293,572 | 378,583 |

### case_aggregate at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| aggregation | 5.5% | 12.9% |
| checksums | 4.8% | 0.0% |
| atomics and reference counts | 3.2% | 0.0% |
| I/O completion | 2.6% | 0.0% |
| write-ahead log: frame lookups and reads | 27.1% | 0.1% |
| page cache and pager | 9.0% | 0.8% |
| file reads (user side) | 0.5% | 0.0% |
| B-tree search | 5.3% | 11.5% |
| record decoding | 0.0% | 2.8% |
| interpreter | 13.7% | 45.0% |
| values and rows | 0.0% | 5.4% |
| statement and binding | 22.2% | 0.1% |
| locks | 1.6% | 14.4% |
| allocator | 1.1% | 0.0% |
| bench: timing and harness | 0.3% | 0.7% |
| libc internals (no symbols) | 1.5% | 0.1% |
| unresolved addresses | 1.3% | 3.3% |
| other | 0.2% | 3.0% |
| (user samples) | 20,150,510,363 | 14,940,894,694 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 28% | 47% |
| statements a second in the profiled run | 3,231 | 9,992 |

### count_distinct at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| std hashing (SipHash) | 0.7% | 0.0% |
| hash tables (GROUP BY, DISTINCT, joins) | 4.9% | 0.0% |
| checksums | 4.7% | 0.0% |
| atomics and reference counts | 2.8% | 0.0% |
| I/O completion | 3.7% | 0.0% |
| write-ahead log: frame lookups and reads | 32.1% | 0.0% |
| page cache and pager | 15.3% | 5.8% |
| B-tree search | 15.6% | 33.0% |
| record decoding | 0.5% | 4.7% |
| interpreter | 4.7% | 6.6% |
| statement and binding | 1.4% | 0.0% |
| locks | 1.7% | 30.3% |
| allocator | 3.0% | 3.2% |
| libc internals (no symbols) | 6.0% | 3.8% |
| unresolved addresses | 1.7% | 3.0% |
| other | 0.8% | 9.3% |
| (user samples) | 18,465,856,326 | 10,015,584,948 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 36% | 56% |
| statements a second in the profiled run | 4 | 5 |

### count_range at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| aggregation | 5.5% | 7.4% |
| statement: parse and plan | 0.0% | 1.1% |
| checksums | 2.5% | 0.0% |
| atomics and reference counts | 0.9% | 0.0% |
| I/O completion | 1.4% | 0.0% |
| write-ahead log: frame lookups and reads | 11.2% | 0.0% |
| page cache and pager | 7.5% | 2.3% |
| B-tree search | 43.7% | 47.5% |
| interpreter | 4.4% | 23.9% |
| values and rows | 2.6% | 0.0% |
| statement and binding | 12.5% | 0.1% |
| locks | 0.4% | 12.3% |
| allocator | 1.2% | 0.1% |
| libc internals (no symbols) | 1.7% | 0.1% |
| unresolved addresses | 0.7% | 1.3% |
| other | 3.8% | 3.6% |
| (user samples) | 23,249,259,703 | 15,779,104,896 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 15% | 30% |
| statements a second in the profiled run | 1,332 | 1,865 |

### filter_no_index at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| checksums | 6.1% | 0.0% |
| atomics and reference counts | 3.6% | 0.0% |
| I/O completion | 4.3% | 0.0% |
| write-ahead log: frame lookups and reads | 37.0% | 0.0% |
| page cache and pager | 12.6% | 2.0% |
| B-tree search | 8.2% | 21.7% |
| record decoding | 1.6% | 3.3% |
| interpreter | 10.6% | 44.4% |
| statement and binding | 7.7% | 0.0% |
| locks | 2.1% | 19.0% |
| allocator | 1.2% | 0.0% |
| libc internals (no symbols) | 2.5% | 0.0% |
| unresolved addresses | 1.8% | 6.8% |
| other | 0.4% | 2.7% |
| (user samples) | 18,946,329,960 | 10,757,608,708 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 32% | 61% |
| statements a second in the profiled run | 0 | 1 |

### group_by_range at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| std hashing (SipHash) | 4.2% | 0.0% |
| errors made and dropped | 1.0% | 0.0% |
| hash tables (GROUP BY, DISTINCT, joins) | 3.5% | 0.0% |
| sorting | 0.1% | 48.2% |
| aggregation | 5.1% | 4.7% |
| checksums | 4.1% | 0.0% |
| atomics and reference counts | 2.0% | 0.0% |
| I/O completion | 3.0% | 0.0% |
| write-ahead log: frame lookups and reads | 23.4% | 0.0% |
| page cache and pager | 7.2% | 0.2% |
| B-tree search | 4.5% | 3.1% |
| record decoding | 0.0% | 2.2% |
| interpreter | 24.9% | 31.4% |
| values and rows | 0.4% | 0.5% |
| statement and binding | 9.7% | 0.1% |
| locks | 0.8% | 3.2% |
| allocator | 0.5% | 0.0% |
| libc internals (no symbols) | 2.5% | 3.0% |
| unresolved addresses | 1.2% | 1.5% |
| other | 1.4% | 1.6% |
| (user samples) | 21,721,324,594 | 19,743,994,945 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 24% | 24% |
| statements a second in the profiled run | 265 | 396 |

### in_list_10 at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| errors made and dropped | 1.3% | 0.0% |
| checksums | 4.0% | 0.0% |
| atomics and reference counts | 2.6% | 0.1% |
| I/O completion | 3.7% | 0.0% |
| write-ahead log: frame lookups and reads | 29.4% | 0.4% |
| page cache and pager | 17.6% | 5.0% |
| B-tree search | 23.5% | 18.8% |
| record decoding | 0.0% | 6.8% |
| interpreter | 4.3% | 7.6% |
| values and rows | 0.0% | 1.4% |
| statement and binding | 2.7% | 2.3% |
| locks | 1.6% | 38.5% |
| allocator | 2.6% | 0.5% |
| bench: timing and harness | 1.0% | 6.3% |
| libc internals (no symbols) | 2.8% | 5.0% |
| unresolved addresses | 1.3% | 4.9% |
| other | 0.7% | 2.4% |
| (user samples) | 18,739,895,527 | 11,291,562,576 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 35% | 59% |
| statements a second in the profiled run | 28,703 | 55,161 |

### json_field at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| JSON | 10.6% | 9.6% |
| checksums | 2.9% | 0.0% |
| atomics and reference counts | 1.6% | 0.3% |
| I/O completion | 2.9% | 0.0% |
| write-ahead log: frame lookups and reads | 23.1% | 2.3% |
| page cache and pager | 15.6% | 4.1% |
| B-tree search | 11.9% | 8.4% |
| record decoding | 0.1% | 3.7% |
| interpreter | 8.8% | 6.8% |
| values and rows | 0.6% | 2.1% |
| statement and binding | 1.8% | 2.8% |
| store: reader checkout | 2.1% | 0.0% |
| locks | 1.4% | 29.4% |
| allocator | 4.3% | 1.1% |
| bench: timing and harness | 6.1% | 12.0% |
| libc internals (no symbols) | 3.0% | 7.1% |
| unresolved addresses | 1.3% | 4.8% |
| other | 1.2% | 5.4% |
| (user samples) | 20,829,376,023 | 12,223,275,154 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 25% | 56% |
| statements a second in the profiled run | 227,982 | 311,448 |

### order_by_sort at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| sorting | 2.1% | 0.0% |
| checksums | 5.3% | 0.0% |
| atomics and reference counts | 3.6% | 0.0% |
| I/O completion | 4.1% | 0.0% |
| write-ahead log: frame lookups and reads | 36.3% | 0.0% |
| page cache and pager | 16.9% | 5.9% |
| B-tree search | 16.9% | 19.3% |
| record decoding | 1.0% | 6.2% |
| interpreter | 6.0% | 11.7% |
| statement and binding | 1.1% | 0.0% |
| locks | 1.5% | 50.0% |
| allocator | 0.9% | 0.0% |
| libc internals (no symbols) | 1.7% | 0.0% |
| unresolved addresses | 1.7% | 4.8% |
| other | 0.4% | 2.0% |
| (user samples) | 17,990,883,937 | 9,938,567,949 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 37% | 65% |
| statements a second in the profiled run | 5 | 11 |

### point_pk at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| std hashing (SipHash) | 0.7% | 0.0% |
| checksums | 3.4% | 0.0% |
| atomics and reference counts | 2.1% | 0.3% |
| I/O completion | 2.7% | 0.0% |
| write-ahead log: frame lookups and reads | 25.1% | 2.3% |
| page cache and pager | 18.6% | 3.2% |
| B-tree search | 13.7% | 9.0% |
| record decoding | 0.0% | 4.2% |
| interpreter | 11.2% | 11.1% |
| values and rows | 1.4% | 5.0% |
| statement and binding | 3.3% | 3.5% |
| store: reader checkout | 2.2% | 0.0% |
| locks | 1.2% | 26.5% |
| allocator | 3.3% | 1.6% |
| bench: timing and harness | 6.9% | 25.5% |
| libc internals (no symbols) | 1.5% | 1.0% |
| unresolved addresses | 1.7% | 4.8% |
| other | 0.8% | 2.0% |
| (user samples) | 20,418,676,614 | 13,493,739,029 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 25% | 48% |
| statements a second in the profiled run | 270,132 | 363,619 |

### point_pk_one_column at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| checksums | 3.7% | 0.0% |
| atomics and reference counts | 1.9% | 0.5% |
| I/O completion | 2.9% | 0.0% |
| write-ahead log: frame lookups and reads | 27.5% | 3.2% |
| page cache and pager | 18.8% | 4.5% |
| B-tree search | 15.1% | 11.5% |
| record decoding | 0.1% | 3.9% |
| interpreter | 10.2% | 5.7% |
| values and rows | 0.0% | 1.0% |
| statement and binding | 2.5% | 2.8% |
| store: reader checkout | 1.2% | 0.0% |
| locks | 1.7% | 35.1% |
| allocator | 1.7% | 1.3% |
| bench: timing and harness | 7.3% | 19.7% |
| libc internals (no symbols) | 2.1% | 1.9% |
| unresolved addresses | 1.7% | 6.9% |
| other | 1.1% | 1.9% |
| (user samples) | 20,258,570,917 | 11,579,687,196 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 26% | 54% |
| statements a second in the profiled run | 274,290 | 413,485 |

### point_secondary at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| statement: parse and plan | 0.0% | 14.7% |
| checksums | 3.8% | 0.0% |
| atomics and reference counts | 2.0% | 0.1% |
| I/O completion | 3.6% | 0.0% |
| write-ahead log: frame lookups and reads | 24.9% | 0.9% |
| page cache and pager | 20.9% | 1.8% |
| B-tree search | 20.2% | 7.9% |
| record decoding | 0.1% | 1.1% |
| interpreter | 7.5% | 2.4% |
| values and rows | 0.2% | 0.7% |
| statement and binding | 2.0% | 1.0% |
| store: reader checkout | 1.0% | 0.0% |
| locks | 1.3% | 51.5% |
| allocator | 2.1% | 0.9% |
| bench: timing and harness | 4.4% | 5.9% |
| libc internals (no symbols) | 2.8% | 2.4% |
| unresolved addresses | 1.7% | 1.9% |
| other | 0.8% | 6.9% |
| (user samples) | 20,740,572,199 | 17,897,720,572 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 27% | 32% |
| statements a second in the profiled run | 160,245 | 150,620 |

### point_text at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| statement: parse and plan | 0.0% | 14.3% |
| checksums | 3.4% | 0.0% |
| atomics and reference counts | 2.3% | 0.1% |
| I/O completion | 3.3% | 0.0% |
| write-ahead log: frame lookups and reads | 25.4% | 0.9% |
| page cache and pager | 18.4% | 2.1% |
| B-tree search | 23.0% | 7.2% |
| record decoding | 0.4% | 1.3% |
| interpreter | 5.8% | 2.1% |
| statement and binding | 1.9% | 1.1% |
| store: reader checkout | 1.1% | 0.0% |
| locks | 1.5% | 48.4% |
| allocator | 2.8% | 1.9% |
| bench: timing and harness | 4.1% | 5.2% |
| libc internals (no symbols) | 3.8% | 4.6% |
| unresolved addresses | 1.4% | 2.8% |
| other | 1.1% | 7.6% |
| (user samples) | 20,485,933,453 | 17,233,793,925 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 26% | 35% |
| statements a second in the profiled run | 153,818 | 165,003 |

### range_pk_100 at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| checksums | 4.3% | 0.0% |
| atomics and reference counts | 2.8% | 0.0% |
| I/O completion | 3.8% | 0.0% |
| write-ahead log: frame lookups and reads | 31.1% | 0.4% |
| page cache and pager | 12.6% | 1.9% |
| B-tree search | 7.1% | 7.1% |
| record decoding | 0.0% | 2.7% |
| interpreter | 9.7% | 26.7% |
| values and rows | 0.0% | 9.3% |
| statement and binding | 19.5% | 6.2% |
| store: reader checkout | 1.4% | 0.0% |
| locks | 1.6% | 10.4% |
| allocator | 0.8% | 0.1% |
| bench: timing and harness | 1.1% | 30.3% |
| libc internals (no symbols) | 2.2% | 0.2% |
| unresolved addresses | 1.5% | 3.1% |
| other | 0.3% | 1.6% |
| (user samples) | 19,854,660,441 | 15,658,741,177 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 30% | 43% |
| statements a second in the profiled run | 29,473 | 50,567 |

### table_count at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| checksums | 7.6% | 0.0% |
| atomics and reference counts | 4.7% | 0.0% |
| I/O completion | 5.5% | 0.0% |
| write-ahead log: frame lookups and reads | 51.4% | 0.0% |
| page cache and pager | 16.4% | 2.0% |
| file reads (user side) | 0.6% | 0.4% |
| B-tree search | 3.6% | 1.0% |
| locks | 2.7% | 69.2% |
| allocator | 1.0% | 0.0% |
| libc internals (no symbols) | 3.2% | 0.5% |
| unresolved addresses | 2.9% | 25.5% |
| other | 0.4% | 0.7% |
| (user samples) | 17,887,249,196 | 4,381,698,575 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 39% | 82% |
| statements a second in the profiled run | 22 | 82 |

### table_group_by at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| std hashing (SipHash) | 6.4% | 0.0% |
| errors made and dropped | 1.3% | 0.0% |
| hash tables (GROUP BY, DISTINCT, joins) | 4.5% | 0.0% |
| aggregation | 6.2% | 4.2% |
| checksums | 3.8% | 0.0% |
| atomics and reference counts | 2.5% | 0.0% |
| I/O completion | 2.7% | 0.0% |
| write-ahead log: frame lookups and reads | 24.2% | 0.0% |
| page cache and pager | 7.1% | 4.5% |
| B-tree search | 4.3% | 14.4% |
| record decoding | 0.5% | 5.0% |
| interpreter | 25.9% | 13.9% |
| values and rows | 0.5% | 0.9% |
| statement and binding | 6.1% | 0.0% |
| locks | 0.7% | 49.3% |
| libc internals (no symbols) | 1.1% | 0.0% |
| unresolved addresses | 1.0% | 5.0% |
| other | 0.8% | 2.6% |
| (user samples) | 20,912,461,440 | 8,947,853,905 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 25% | 69% |
| statements a second in the profiled run | 0 | - |

### text_read at 1 client

| part | amphora | sqlite |
|---|---:|---:|
| checksums | 3.3% | 0.0% |
| atomics and reference counts | 2.1% | 0.6% |
| I/O completion | 2.7% | 0.0% |
| write-ahead log: frame lookups and reads | 27.3% | 3.8% |
| page cache and pager | 21.4% | 4.5% |
| B-tree search | 13.2% | 10.4% |
| record decoding | 0.0% | 4.1% |
| interpreter | 9.4% | 5.7% |
| values and rows | 0.3% | 1.5% |
| statement and binding | 2.2% | 2.0% |
| store: reader checkout | 2.2% | 0.0% |
| locks | 1.6% | 36.6% |
| allocator | 1.3% | 1.6% |
| bench: timing and harness | 8.1% | 20.1% |
| libc internals (no symbols) | 2.1% | 1.8% |
| unresolved addresses | 1.3% | 5.7% |
| other | 1.1% | 1.4% |
| (user samples) | 20,440,928,143 | 12,025,458,466 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 27% | 52% |
| statements a second in the profiled run | 312,218 | 456,548 |

### filter_no_index at 32 clients

| part | amphora | sqlite |
|---|---:|---:|
| atomics and reference counts | 1.2% | 0.0% |
| write-ahead log: frame lookups and reads | 90.7% | 0.0% |
| page cache and pager | 3.4% | 8.8% |
| B-tree search | 0.7% | 4.9% |
| record decoding | 0.0% | 0.7% |
| interpreter | 0.7% | 12.9% |
| statement and binding | 0.6% | 0.0% |
| locks | 1.3% | 38.2% |
| libc internals (no symbols) | 0.1% | 10.6% |
| unresolved addresses | 0.6% | 21.8% |
| other | 0.0% | 1.6% |
| (user samples) | 475,188,341,981 | 21,231,095,176 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 9% | 87% |
| statements a second in the profiled run | - | - |

### point_pk at 32 clients

| part | amphora | sqlite |
|---|---:|---:|
| atomics and reference counts | 0.8% | 0.3% |
| write-ahead log: frame lookups and reads | 66.0% | 0.8% |
| page cache and pager | 7.3% | 10.8% |
| B-tree search | 2.9% | 4.5% |
| record decoding | 0.0% | 2.6% |
| interpreter | 2.4% | 4.3% |
| values and rows | 0.3% | 1.1% |
| statement and binding | 0.8% | 0.9% |
| store: reader checkout | 0.9% | 0.0% |
| locks | 14.6% | 35.7% |
| allocator | 0.5% | 1.5% |
| bench: timing and harness | 1.1% | 8.3% |
| libc internals (no symbols) | 0.3% | 6.5% |
| unresolved addresses | 1.3% | 20.6% |
| other | 0.2% | 1.9% |
| (user samples) | 319,251,081,641 | 35,333,233,407 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 11% | 90% |
| statements a second in the profiled run | 468,989 | 161,497 |

### point_secondary at 32 clients

| part | amphora | sqlite |
|---|---:|---:|
| statement: parse and plan | 0.0% | 7.1% |
| atomics and reference counts | 1.0% | 0.1% |
| write-ahead log: frame lookups and reads | 63.1% | 0.2% |
| page cache and pager | 8.5% | 6.4% |
| B-tree search | 5.2% | 5.2% |
| record decoding | 0.0% | 0.5% |
| interpreter | 1.8% | 2.5% |
| statement and binding | 0.4% | 0.8% |
| locks | 16.1% | 41.6% |
| allocator | 0.3% | 2.7% |
| bench: timing and harness | 0.7% | 3.1% |
| libc internals (no symbols) | 0.4% | 6.9% |
| unresolved addresses | 1.2% | 17.7% |
| other | 0.1% | 4.9% |
| (user samples) | 362,124,244,435 | 33,455,809,488 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 11% | 85% |
| statements a second in the profiled run | 362,778 | 47,731 |

### range_pk_100 at 32 clients

| part | amphora | sqlite |
|---|---:|---:|
| atomics and reference counts | 1.0% | 0.1% |
| write-ahead log: frame lookups and reads | 86.7% | 0.4% |
| page cache and pager | 4.6% | 8.9% |
| B-tree search | 0.8% | 3.2% |
| record decoding | 0.0% | 1.2% |
| interpreter | 0.9% | 9.8% |
| values and rows | 0.0% | 2.5% |
| statement and binding | 1.9% | 1.9% |
| locks | 1.8% | 32.0% |
| bench: timing and harness | 0.1% | 12.1% |
| libc internals (no symbols) | 0.4% | 7.5% |
| unresolved addresses | 0.7% | 18.3% |
| other | 0.1% | 1.6% |
| (user samples) | 440,453,695,822 | 35,708,184,268 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 9% | 89% |
| statements a second in the profiled run | 62,032 | 32,840 |

### table_count at 32 clients

| part | amphora | sqlite |
|---|---:|---:|
| atomics and reference counts | 1.2% | 0.0% |
| write-ahead log: frame lookups and reads | 92.3% | 0.0% |
| page cache and pager | 3.5% | 11.6% |
| B-tree search | 0.2% | 1.1% |
| locks | 1.4% | 48.4% |
| allocator | 0.1% | 0.7% |
| libc internals (no symbols) | 0.2% | 11.4% |
| unresolved addresses | 0.5% | 25.7% |
| other | 0.0% | 0.6% |
| (user samples) | 514,659,890,841 | 21,401,298,910 |
| kernel share of CPU (cpu_sys_s / (user + sys)) | 8% | 92% |
| statements a second in the profiled run | 34 | 21 |

