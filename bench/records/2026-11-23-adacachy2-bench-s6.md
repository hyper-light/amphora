# Lane S6 (bench), 2026-11-23 19:18Z to 20:13Z: one run of the competitor families at main cfa0107c

Ada's goal: Amphora about 10x faster than every competitor on every query type, on the same client
and platform, in a production build. This lane made one run of each competitor family that runs
today on main's head, with every answer held to the competitors' where the harness can.

## How it ran

- Commit: main cfa0107c5e45833823fc4f52e9bd238159ae9293, clone `~/.cache/amphora-perf/bench-s` on
  adacachy2 (runner 00aad8daff2fa3c9daa65d53c425d450: AMD Ryzen 9 9950X3D2, 32 cores, 122 GiB, btrfs on
  a Samsung 9100 PRO).
- Build: the workspace's release profile on the bench workspace, `CARGO_PROFILE_RELEASE_LTO=thin
  CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true` (debug =
  line-tables-only), `cargo build --release --offline --manifest-path bench/Cargo.toml --bin
  amphora-bench`, 4 min 59 s at 2 jobs (r1479). Binary kept at
  `~/.cache/amphora-perf-bin/bench-s6/amphora-bench-cfa0107c5e45833823fc4f52e9bd238159ae9293`.
- Room: kiln refused cpus=16 (9 of 32 cores idle) and then cpus=8 (2 of 32 idle), so the run held
  cpus=2, mem=8g. The host carried other lanes' work throughout: the 1-minute load average sits beside
  every number below.
- Two lanes side by side in one run, from 19:56:56Z to the run's cut at 20:07Z: lane A was TPC-H
  (load, then the 22 queries), lane B was the replication ladder, then the SQL suite in process, then
  the suite on PostgreSQL. Each lane's numbers ran beside the other lane's work. Read any ratio under
  2x as noise. Each case ran once: no alternation, no repeat rounds.
- Script: `bench/records/2026-11-23-adacachy2-bench-s6/families.sh`. Tables:
  `bench/records/2026-11-23-adacachy2-bench-s6/s6_tables.py` over the run's outputs in
  `~/.cache/amphora-perf/bench-s6-run/out` on adacachy2.

## Verdict, losses first

Amphora is not 10x faster than any competitor on any query type here except writes from many
clients. One run a case, on a shared host at load 23 to 118, so a ratio under 2x is noise.

- **Replicated commits: the largest loss.** Amphora's persisted class is 4.4x slower than rqlite at
  1 writer (6.8 against 30.2 commits/s), 104x slower at 32 (6.4 against 664) and 42x slower at 128
  (10.8 against 452). Against dqlite it is 27x, 17x and 15x slower. At 32 and 128 writers its members
  refused writes (list 2).
- **TPC-H SF1 (q01 to q18 reached, one thread each).** Slower than DuckDB on 13 of the 18 queries:
  q03 15.0x, q01 13.8x, q05 11.2x, q06 9.8x, q07 7.8x, q14 7.9x. Faster than both SQLite and DuckDB on
  2 (q02, and q08 by 1.04x over DuckDB). 10x faster than both: 0 of 18. Every answer matched:
  answers.py compared 18 queries, 0 differ.
- **Point reads at 1 client.** 1.1x to 1.8x slower than SQLite and Turso on all five statements.
  At 32 clients still 1.1x to 1.7x slower than Turso. Faster than SQLite only from 32 clients up
  (3x to 12x), where SQLite's connections contend.
- **OLTP writes at 1 client.** Slower than SQLite on all seven: insert_one 7.6x, txn_read_write
  5.3x, update_plain and delete_insert 2.25x, upsert 1.9x, update_indexed 1.9x, insert_batch_100
  1.2x.
- **Suite analytics.** table_count is 17x to 110x slower than Turso at every client count.
  filter_no_index at 128 clients answered nothing inside its window.
- **Wins.** Writes from 32 and 128 clients: 6x to 80x faster than SQLite and Turso (update_indexed
  at 128: 12,574 ops/s against Turso's 158). Those two serialize writers, so this is the expected
  shape. The 10x bar is met there and nowhere else in this run.

## 1. Tables per query type

Ratios are Amphora's speed over the competitor's (throughput, or the inverse of time). The load
beside each number is the 1-minute load average on 32 cores. Concurrency is in the client column:
1, 32 and 128 clients for the suite, 1, 32 and 128 writers for the ladder. JOB, cold start and 512
to 10k clients have no table: list 3 says why. TPC-H reached q01 to q18 (q18 on SQLite only) before
the run's cut at 20:07Z.

### Replicated commits (bench/repl_ladder.sh, 3 members on one host, writers 1/32/128)

Load (1-min average) over the ladder: 23-37 on 32 cores.

| writers | competitor | column | competitor p50 / p99 ms | competitor commits/s | Amphora persisted p50 / p99 ms | Amphora persisted commits/s | Amphora against it (commits/s) | Amphora replicated commits/s | refused |
|---|---|---|---|---|---|---|---|---|---|
| 1 | dqlite | quorum | 4.80 / 12.16 | 184 | 127 / 655 | 6.80 | 27.09x slower | 4.60 |  |
| 1 | litefs | primary only | 78.85 / 1,819 | 7.60 | 127 / 655 | 6.80 | 1.12x slower | 4.60 |  |
| 1 | rqlite | quorum | 25.34 / 97.28 | 30.20 | 127 / 655 | 6.80 | 4.44x slower | 4.60 |  |
| 32 | dqlite | quorum | 199 / 1,327 | 109 | 2,818 / 7,864 | 6.40 | 16.97x slower | 9.60 |  |
| 32 | litefs | primary only | 795 / 4,981 | 27.80 | 2,818 / 7,864 | 6.40 | 4.34x slower | 9.60 |  |
| 32 | rqlite | quorum | 43.01 / 100 | 664 | 2,818 / 7,864 | 6.40 | 103.69x slower | 9.60 |  |
| 128 | dqlite | quorum | 496 / 3,146 | 164 | 6,226 / 10,879 | 10.80 | 15.20x slower | 10.80 |  |
| 128 | litefs | primary only | 2,261 / 8,126 | 29.40 | 6,226 / 10,879 | 10.80 | 2.72x slower | 10.80 |  |
| 128 | rqlite | quorum | 252 / 504 | 452 | 6,226 / 10,879 | 10.80 | 41.85x slower | 10.80 |  |

Load (1-min average) before/after each suite target's pass: amphora: 31.17/39.2; sqlite: 74.91/36.24; turso: 39.2/74.91

### Point reads (bench/suite.sh, 200,000 items, 1 s window after 0.5 s warm-up)

| statement | clients | competitor | competitor p50 / p99 ms | competitor ops/s | Amphora p50 / p99 ms | Amphora ops/s | Amphora against it (ops/s) | rows/op Amphora vs it | load before/after (Amphora; it) |
|---|---|---|---|---|---|---|---|---|---|
| point_pk | 1 | sqlite | 0.001 / 0.003 | 475,836 | 0.003 / 0.005 | 275,198 | 1.73x slower | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| point_pk | 1 | turso | 0.001 / 0.003 | 482,167 | 0.003 / 0.005 | 275,198 | 1.75x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| point_pk | 32 | sqlite | 0.116 / 0.544 | 222,608 | 0.015 / 0.352 | 709,713 | 3.19x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| point_pk | 32 | turso | 0.012 / 0.444 | 1,023,093 | 0.015 / 0.352 | 709,713 | 1.44x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| point_pk | 128 | sqlite | 0.704 / 4.54 | 135,941 | 0.016 / 2.18 | 684,835 | 5.04x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| point_pk | 128 | turso | 0.011 / 4.29 | 594,074 | 0.016 / 2.18 | 684,835 | 1.15x faster | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| point_secondary | 1 | sqlite | 0.005 / 0.009 | 176,360 | 0.004 / 0.008 | 215,523 | 1.22x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| point_secondary | 1 | turso | 0.002 / 0.005 | 331,869 | 0.004 / 0.008 | 215,523 | 1.54x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| point_secondary | 32 | sqlite | 0.512 / 1.57 | 56,078 | 0.017 / 0.464 | 603,860 | 10.77x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| point_secondary | 32 | turso | 0.013 / 0.484 | 1,011,485 | 0.017 / 0.464 | 603,860 | 1.68x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| point_secondary | 128 | sqlite | 2.53 / 8.06 | 45,700 | 0.023 / 3.20 | 356,009 | 7.79x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| point_secondary | 128 | turso | 0.014 / 9.22 | 306,543 | 0.023 / 3.20 | 356,009 | 1.16x faster | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| point_text | 1 | sqlite | 0.005 / 0.008 | 187,250 | 0.004 / 0.008 | 197,797 | 1.06x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| point_text | 1 | turso | 0.002 / 0.005 | 305,831 | 0.004 / 0.008 | 197,797 | 1.55x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| point_text | 32 | sqlite | 0.484 / 1.47 | 59,660 | 0.014 / 0.372 | 737,668 | 12.36x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| point_text | 32 | turso | 0.012 / 0.520 | 952,485 | 0.014 / 0.372 | 737,668 | 1.29x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| point_text | 128 | sqlite | 2.30 / 7.17 | 51,058 | 0.216 / 2.82 | 280,921 | 5.50x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| point_text | 128 | turso | 0.013 / 9.86 | 291,989 | 0.216 / 2.82 | 280,921 | 1.04x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| json_field | 1 | sqlite | 0.002 / 0.003 | 415,037 | 0.003 / 0.006 | 241,619 | 1.72x slower | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| json_field | 1 | turso | 0.002 / 0.004 | 394,470 | 0.003 / 0.006 | 241,619 | 1.63x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| json_field | 32 | sqlite | 0.107 / 0.560 | 231,668 | 0.014 / 0.272 | 897,280 | 3.87x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| json_field | 32 | turso | 0.011 / 0.528 | 991,737 | 0.014 / 0.272 | 897,280 | 1.11x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| json_field | 128 | sqlite | 0.464 / 3.68 | 183,984 | 0.019 / 1.50 | 749,951 | 4.08x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| json_field | 128 | turso | 0.013 / 8.70 | 325,645 | 0.019 / 1.50 | 749,951 | 2.30x faster | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| blob_read | 1 | sqlite | 0.001 / 0.003 | 515,266 | 0.002 / 0.005 | 297,217 | 1.73x slower | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| blob_read | 1 | turso | 0.001 / 0.003 | 457,589 | 0.002 / 0.005 | 297,217 | 1.54x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| blob_read | 32 | sqlite | 0.103 / 0.568 | 236,644 | 0.019 / 0.230 | 829,749 | 3.51x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| blob_read | 32 | turso | 0.012 / 0.436 | 1,016,829 | 0.019 / 0.230 | 829,749 | 1.23x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| blob_read | 128 | sqlite | 0.688 / 3.14 | 153,199 | 0.031 / 1.20 | 757,747 | 4.95x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| blob_read | 128 | turso | 0.013 / 9.09 | 287,733 | 0.031 / 1.20 | 757,747 | 2.63x faster | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |

### OLTP writes (bench/suite.sh, 200,000 items, 1 s window after 0.5 s warm-up)

| statement | clients | competitor | competitor p50 / p99 ms | competitor ops/s | Amphora p50 / p99 ms | Amphora ops/s | Amphora against it (ops/s) | rows/op Amphora vs it | load before/after (Amphora; it) |
|---|---|---|---|---|---|---|---|---|---|
| insert_one | 1 | sqlite | 2.59 / 7.36 | 343 | 21.50 / 39.42 | 45.00 | 7.62x slower | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| insert_one | 1 | turso | 7.81 / 22.53 | 113 | 21.50 / 39.42 | 45.00 | 2.51x slower | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| insert_one | 32 | sqlite | 2.75 / 22.53 | 307 | 8.83 / 59.39 | 1,833 | 5.97x faster | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| insert_one | 32 | turso | 67.58 / 598 | 236 | 8.83 / 59.39 | 1,833 | 7.77x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| insert_one | 128 | sqlite | 3.17 / 729 | 255 | 46.59 / 65.54 | 2,944 | 11.55x faster | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| insert_one | 128 | turso | 356 / 1,442 | 218 | 46.59 / 65.54 | 2,944 | 13.50x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| insert_batch_100 | 1 | sqlite | 38.40 / 113 | 23.00 | 26.37 / 43.01 | 19.00 | 1.21x slower | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| insert_batch_100 | 1 | turso | 12.54 / 33.79 | 71.00 | 26.37 / 43.01 | 19.00 | 3.74x slower | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| insert_batch_100 | 32 | sqlite | 26.11 / 1,262 | 91.00 | 141 / 193 | 269 | 2.96x faster | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| insert_batch_100 | 32 | turso | 256 / 1,311 | 82.00 | 141 / 193 | 269 | 3.28x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| insert_batch_100 | 128 | sqlite | 69.63 / 1,360 | 50.00 | 573 / 582 | 174 | 3.48x faster | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| insert_batch_100 | 128 | turso | 713 / 1,393 | 59.00 | 573 / 582 | 174 | 2.95x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| update_plain | 1 | sqlite | 9.09 / 19.20 | 99.00 | 23.55 / 61.95 | 44.00 | 2.25x slower | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| update_plain | 1 | turso | 6.14 / 336 | 88.00 | 23.55 / 61.95 | 44.00 | 2.00x slower | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| update_plain | 32 | sqlite | 9.47 / 20.48 | 92.00 | 10.50 / 39.42 | 1,920 | 20.87x faster | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| update_plain | 32 | turso | 88.06 / 1,081 | 151 | 10.50 / 39.42 | 1,920 | 12.72x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| update_plain | 128 | sqlite | 9.09 / 18.18 | 117 | 18.69 / 38.40 | 6,656 | 56.89x faster | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| update_plain | 128 | turso | 524 / 1,425 | 148 | 18.69 / 38.40 | 6,656 | 44.97x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| update_indexed | 1 | sqlite | 4.54 / 9.73 | 193 | 9.09 / 23.30 | 104 | 1.86x slower | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| update_indexed | 1 | turso | 5.25 / 14.72 | 157 | 9.09 / 23.30 | 104 | 1.51x slower | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| update_indexed | 32 | sqlite | 4.67 / 10.37 | 189 | 8.19 / 17.92 | 3,245 | 17.17x faster | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| update_indexed | 32 | turso | 205 / 1,229 | 64.00 | 8.19 / 17.92 | 3,245 | 50.70x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| update_indexed | 128 | sqlite | 2.62 / 926 | 298 | 9.34 / 27.14 | 12,574 | 42.19x faster | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| update_indexed | 128 | turso | 532 / 1,442 | 158 | 9.34 / 27.14 | 12,574 | 79.58x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| upsert | 1 | sqlite | 2.72 / 6.78 | 321 | 4.86 / 9.34 | 170 | 1.89x slower | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| upsert | 1 | turso | 5.25 / 12.03 | 175 | 4.86 / 9.34 | 170 | 1.03x slower | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| upsert | 32 | sqlite | 2.72 / 6.78 | 333 | 5.70 / 14.46 | 4,608 | 13.84x faster | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| upsert | 32 | turso | 54.78 / 647 | 285 | 5.70 / 14.46 | 4,608 | 16.17x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| upsert | 128 | sqlite | 4.93 / 532 | 197 | 10.88 / 19.71 | 10,970 | 55.69x faster | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| upsert | 128 | turso | 315 / 1,376 | 260 | 10.88 / 19.71 | 10,970 | 42.19x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| delete_insert | 1 | sqlite | 2.69 / 5.82 | 331 | 5.57 / 10.62 | 147 | 2.25x slower | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| delete_insert | 1 | turso | 2.88 / 10.37 | 284 | 5.57 / 10.62 | 147 | 1.93x slower | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| delete_insert | 32 | sqlite | 2.78 / 6.98 | 235 | 14.59 / 28.16 | 2,033 | 8.65x faster | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| delete_insert | 32 | turso | 5.57 / 786 | 260 | 14.59 / 28.16 | 2,033 | 7.82x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| delete_insert | 128 | sqlite | 10.75 / 1,327 | 203 | 14.85 / 26.11 | 8,219 | 40.49x faster | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| delete_insert | 128 | turso | 311 / 1,393 | 263 | 14.85 / 26.11 | 8,219 | 31.25x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| txn_read_write | 1 | sqlite | 2.66 / 6.46 | 338 | 5.82 / 713 | 64.00 | 5.28x slower | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| txn_read_write | 1 | turso | 4.67 / 901 | 22.00 | 5.82 / 713 | 64.00 | 2.91x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| txn_read_write | 32 | sqlite | 5.70 / 836 | 268 | 19.20 / 38.40 | 1,608 | 6.00x faster | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| txn_read_write | 32 | turso | 20.74 / 819 | 278 | 19.20 / 38.40 | 1,608 | 5.78x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| txn_read_write | 128 | sqlite | 13.18 / 1,425 | 135 | 28.16 / 164 | 4,384 | 32.47x faster | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| txn_read_write | 128 | turso | 319 / 1,262 | 255 | 28.16 / 164 | 4,384 | 17.19x faster | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |

### Analytics in the suite (scans, aggregates, joins) (bench/suite.sh, 200,000 items, 1 s window after 0.5 s warm-up)

| statement | clients | competitor | competitor p50 / p99 ms | competitor ops/s | Amphora p50 / p99 ms | Amphora ops/s | Amphora against it (ops/s) | rows/op Amphora vs it | load before/after (Amphora; it) |
|---|---|---|---|---|---|---|---|---|---|
| table_count | 1 | sqlite | 0.118 / 0.226 | 8,111 | 0.448 / 0.648 | 2,183 | 3.72x slower | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| table_count | 1 | turso | 0.026 / 0.055 | 36,743 | 0.448 / 0.648 | 2,183 | 16.83x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| table_count | 32 | sqlite | 11.78 / 22.27 | 2,608 | 5.57 / 18.18 | 4,923 | 1.89x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| table_count | 32 | turso | 0.030 / 0.632 | 539,834 | 5.57 / 18.18 | 4,923 | 109.66x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| table_count | 128 | sqlite | 58.88 / 116 | 2,116 | 0.033 / 108 | 28,081 | 13.27x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| table_count | 128 | turso | 0.030 / 3.94 | 491,615 | 0.033 / 108 | 28,081 | 17.51x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| table_group_by | 1 | sqlite | 158 / 172 | 7.00 | 58.37 / 70.66 | 17.00 | 2.43x faster | 100 vs 100 | 31.17/39.2; 74.91/36.24 |
| table_group_by | 1 | turso | 229 / 270 | 5.00 | 58.37 / 70.66 | 17.00 | 3.40x faster | 100 vs 100 | 31.17/39.2; 39.2/74.91 |
| table_group_by | 32 | sqlite | - / - | 0.000 | 389 / 438 | 64.00 | - | 100 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| table_group_by | 32 | turso | 1,278 / 1,475 | 19.00 | 389 / 438 | 64.00 | 3.37x faster | 100 vs 100 | 31.17/39.2; 39.2/74.91 |
| table_group_by | 128 | sqlite | - / - | 0.000 | - / - | 0.000 | - | 0.000 vs 0.000 | 31.17/39.2; 74.91/36.24 |
| table_group_by | 128 | turso | - / - | 0.000 | - / - | 0.000 | - | 0.000 vs 0.000 | 31.17/39.2; 39.2/74.91 |
| filter_no_index | 1 | sqlite | 16.26 / 21.50 | 60.00 | 37.38 / 45.57 | 26.00 | 2.31x slower | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| filter_no_index | 1 | turso | 27.90 / 30.98 | 36.00 | 37.38 / 45.57 | 26.00 | 1.38x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| filter_no_index | 32 | sqlite | 1,360 / 1,442 | 32.00 | 369 / 512 | 77.00 | 2.41x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| filter_no_index | 32 | turso | 217 / 647 | 135 | 369 / 512 | 77.00 | 1.75x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| filter_no_index | 128 | sqlite | 1,393 / 1,393 | 1.00 | - / - | 0.000 | - | 0.000 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| filter_no_index | 128 | turso | 1,229 / 1,229 | 1.00 | - / - | 0.000 | - | 0.000 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| aggregates_range | 1 | sqlite | 0.146 / 0.280 | 6,535 | 0.238 / 0.440 | 3,963 | 1.65x slower | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| aggregates_range | 1 | turso | 0.188 / 0.360 | 5,003 | 0.238 / 0.440 | 3,963 | 1.26x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| aggregates_range | 32 | sqlite | 6.14 / 12.93 | 5,052 | 1.81 / 6.72 | 15,484 | 3.06x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| aggregates_range | 32 | turso | 0.784 / 7.94 | 25,294 | 1.81 / 6.72 | 15,484 | 1.63x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| aggregates_range | 128 | sqlite | 37.38 / 68.61 | 3,472 | 23.30 / 38.40 | 5,691 | 1.64x faster | 1.00 vs 1.00 | 31.17/39.2; 74.91/36.24 |
| aggregates_range | 128 | turso | 8.96 / 57.34 | 10,014 | 23.30 / 38.40 | 5,691 | 1.76x slower | 1.00 vs 1.00 | 31.17/39.2; 39.2/74.91 |
| join_three | 1 | sqlite | 0.508 / 0.824 | 1,947 | 1.98 / 3.07 | 495 | 3.93x slower | 20.00 vs 20.00 | 31.17/39.2; 74.91/36.24 |
| join_three | 1 | turso | 16.26 / 23.30 | 60.00 | 1.98 / 3.07 | 495 | 8.25x faster | 20.00 vs 20.00 | 31.17/39.2; 39.2/74.91 |
| join_three | 32 | sqlite | 32.00 / 46.08 | 992 | 22.27 / 71.68 | 1,207 | 1.22x faster | 20.00 vs 20.00 | 31.17/39.2; 74.91/36.24 |
| join_three | 32 | turso | 32.77 / 205 | 723 | 22.27 / 71.68 | 1,207 | 1.67x faster | 20.00 vs 20.00 | 31.17/39.2; 39.2/74.91 |
| join_three | 128 | sqlite | 170 / 248 | 745 | 131 / 401 | 984 | 1.32x faster | 20.00 vs 20.00 | 31.17/39.2; 74.91/36.24 |
| join_three | 128 | turso | 158 / 455 | 787 | 131 / 401 | 984 | 1.25x faster | 20.00 vs 20.00 | 31.17/39.2; 39.2/74.91 |

### Analytics: TPC-H SF1, 22 queries, one thread each (amphora-bench --planner)

Load (1-min average) over the TPC-H pass: 30-118 on 32 cores. Each query: the counted warm-up, then up to 3 timed runs; p50 = median of the timed runs, p99 column = the slowest of them (3 runs carry no p99).

| query | competitor | competitor ms p50 / slowest | Amphora ms p50 / slowest | Amphora against it | load at Amphora's runs |
|---|---|---|---|---|---|
| q01 | sqlite | 5,602 / 5,702 | 3,066 / 3,277 | 1.83x faster | 30.43-38.01 |
| q01 | duckdb | 222 / 226 | 3,066 / 3,277 | 13.84x slower | 30.43-38.01 |
| q02 | sqlite | 253 / 260 | 40.70 / 40.71 | 6.21x faster | 47.61-47.61 |
| q02 | duckdb | 98.27 / 98.92 | 40.70 / 40.71 | 2.41x faster | 47.61-47.61 |
| q03 | sqlite | 800 / 824 | 1,654 / 9,908 | 2.07x slower | 45.16-45.16 |
| q03 | duckdb | 110 / 117 | 1,654 / 9,908 | 15.01x slower | 45.16-45.16 |
| q04 | sqlite | 214 / 236 | 356 / 387 | 1.66x slower | 47.08-47.08 |
| q04 | duckdb | 87.52 / 94.49 | 356 / 387 | 4.07x slower | 47.08-47.08 |
| q05 | sqlite | 2,201 / 2,281 | 1,832 / 1,934 | 1.20x faster | 44.22-45.55 |
| q05 | duckdb | 164 / 178 | 1,832 / 1,934 | 11.17x slower | 44.22-45.55 |
| q06 | sqlite | 386 / 400 | 683 / 686 | 1.77x slower | 40.70-40.70 |
| q06 | duckdb | 69.66 / 75.71 | 683 / 686 | 9.81x slower | 40.70-40.70 |
| q07 | sqlite | 15,951 / 29,128 | 1,779 / 2,211 | 8.97x faster | 37.02-39.20 |
| q07 | duckdb | 227 / 232 | 1,779 / 2,211 | 7.84x slower | 37.02-39.20 |
| q08 | sqlite | 11,824 / 12,357 | 222 / 327 | 53.25x faster | 94.70-100 |
| q08 | duckdb | 230 / 237 | 222 / 327 | 1.04x faster | 94.70-100 |
| q09 | sqlite | 1,953 / 2,015 | 2,461 / 4,402 | 1.26x slower | 63.62-65.15 |
| q09 | duckdb | 434 / 457 | 2,461 / 4,402 | 5.68x slower | 63.62-65.15 |
| q10 | sqlite | 1,848 / 2,046 | 968 / 1,000 | 1.91x faster | 59.81-59.81 |
| q10 | duckdb | 209 / 224 | 968 / 1,000 | 4.62x slower | 59.81-59.81 |
| q11 | sqlite | 58.21 / 65.56 | 129 / 151 | 2.21x slower | 58.88-58.88 |
| q11 | duckdb | 152 / 152 | 129 / 151 | 1.18x faster | 58.88-58.88 |
| q12 | sqlite | 1,292 / 1,309 | 909 / 1,034 | 1.42x faster | 58.49-58.49 |
| q12 | duckdb | 157 / 168 | 909 / 1,034 | 5.79x slower | 58.49-58.49 |
| q13 | sqlite | 1,682 / 1,733 | 2,750 / 2,992 | 1.63x slower | 62.53-68.41 |
| q13 | duckdb | 1,055 / 1,151 | 2,750 / 2,992 | 2.61x slower | 62.53-68.41 |
| q14 | sqlite | 6,827 / 7,019 | 819 / 844 | 8.34x faster | 73.14-75.15 |
| q14 | duckdb | 104 / 111 | 819 / 844 | 7.89x slower | 73.14-75.15 |
| q15 | sqlite | 5,969 / 6,051 | 1,672 / 1,705 | 3.57x faster | 94.21-94.21 |
| q15 | duckdb | 524 / 527 | 1,672 / 1,705 | 3.19x slower | 94.21-94.21 |
| q16 | sqlite | 106 / 107 | 167 / 169 | 1.58x slower | 71.88-71.88 |
| q16 | duckdb | 340 / 341 | 167 / 169 | 2.03x faster | 71.88-71.88 |
| q17 | sqlite | 65.88 / 73.60 | 88.84 / 93.10 | 1.35x slower | 66.93-66.93 |
| q17 | duckdb | 102 / 102 | 88.84 / 93.10 | 1.15x faster | 66.93-66.93 |
| q18 | sqlite | 540 / 550 | 1,773 / 2,090 | 3.28x slower | 63.57-66.93 |

answers.py (last lines):

```
answers.py: 18 queries compared, 0 differ
answers.py rc=0
```

## 2. Every failure Amphora showed

Each line has a one-line command that reproduces it from a clone at cfa0107c, after the build in
"How it ran". `R=~/.cache/s6-repro`, a records folder of your own.

- **Replicated commits refused at 32 writers (persisted).** 21 writes refused across the three
  members (4, 11 and 6), p50 2,818 ms, p99 7,864 ms, 6.4 commits/s. Repro:
  `SYSTEMS=amphora-procs-persisted LADDER=32 bash bench/repl_ladder.sh $R --batch 1 --text-bytes 600 --deadline-s 10 --relay-records 128 --spanning 2 --warmup-s 1 --seconds 5`
- **Replicated commits refused at 128 writers (persisted).** 28 refused (10, 6 and 12), p50 6,226
  ms, p99 10,879 ms. Repro: the same command with `LADDER=128`.
- **Replicated commits refused at 32 writers (replicated class), and one writer missed its
  deadline.** 21 refused (2, 10 and 9), `writers_missed: 1`, p50 1,294 ms. Repro: the same command
  with `SYSTEMS=amphora-procs-replicated LADDER=32`. These are the stale-base refusals lanes V6 and V7
  described; this run shows them at main's head.
- **filter_no_index at 128 clients: no answer inside the 1 s window.** Amphora completed 0 runs;
  SQLite and Turso completed 1 each. Not a wrong answer (rows per run match at 1 and 32 clients), a
  statement slower than its window under 128 whole-table scans. Repro:
  `bench/run.sh amphora --suite --only 'filter_no_index$' --items 200000 --clients 128 --seconds 1 --warmup 0.5`
- **TPC-H q03 and q09: unstable run times, right answers.** q03's slowest timed run took 9,908 ms
  against a median of 1,654 ms; q09's took 4,402 against 2,461. Lane T3 traced plan changes between
  runs as learned weights move. Repro:
  `amphora-bench --planner --bench tpch --sf 1 --data ~/.cache/planner-bench/tpch-sf1 --queries bench/planner/tpch/queries --dir $R/tpch --targets amphora --only q03 --runs 3 --timeout-s 60 --count-timeout-s 60`

No wrong answer, error or crash from Amphora otherwise. The suite's one other mismatch flag
(table_group_by at 32 clients) is SQLite answering nothing inside its window, not Amphora.

## 3. What didn't run, and thin coverage

Each line is one family or one query type, with its cause.

- **JOB (113 queries vs SQLite and DuckDB): not run.** No JOB database loaded by a main-head build
  exists on adacachy2, and loading IMDb's 3.7 GB into three engines didn't fit the hour beside the
  other families. Lane M2 never reached it either.
- **Cold start against competitors: not run.** `bench/planner/cold.sh` drops the whole host's page
  cache before each query (through a privileged container), which would disturb every other lane's
  timed runs on the shared adacachy2. The `open_time` example (perf/coldstart) times Amphora alone.
- **Concurrency from 512 to 10k clients: not run.** The suite's clients are OS threads, one a client
  (`src/suite.rs`, `std::thread::scope`), and 10k busy threads on a shared 32-core host is the
  overload A6's 8,465 threads caused this morning (load 836). The ladder's rungs 512 to 4096 were left
  out because Amphora's replicated commits already refuse writes at 32 and 128. Coverage is 1, 32 and
  128 clients only: thin.
- **libSQL (replication): no numbers.** Its cluster didn't come up: a member was killed at start,
  then "the primary runs PRAGMA synchronous=1, not 2 (FULL): durability_mode=strong didn't take"
  (both rungs; lane S4 saw the same).
- **The server group (PostgreSQL, MySQL, ClickHouse, OrioleDB, CedarDB): not run.** PostgreSQL's
  pass was queued after the in-process suite and the run's cut came first. The other four images are
  gone from adacachy2 (lane S4's cleanup removed them) and weren't pulled again.
- **Amphora's own server against the servers (`amphora-serve`, the conversation bench): not run.**
  Its image is built in a rust:1-bookworm container first, which didn't fit.
- **Failover (kill -9 of the leader 100 times) and catch-up: not run.** Time.
- **Replicated commits across machines: not run.** No three Linux machines share a wired LAN, and
  the Mac Studios reach each other only over Wi-Fi (lane S5). The ladder's three members share one
  host, so their fates are faked on one machine.
- **scale.sh (CockroachDB, TiDB), backup_restore.sh (pgBackRest, Litestream), Vitess: not run.** No
  S lane has produced numbers from them yet, and they didn't fit.
- **TPC-H multi-threaded (DuckDB's default of every core, Amphora's parallel mode) and SF10: not
  run.** DuckDB's default takes all 32 cores, far past the 2 kiln granted. Lane M2's 11-23 numbers
  stand for it: Amphora 9.4x slower than DuckDB on every core by the geometric mean.
- **TPC-H q19 to q22, and q18 on DuckDB: not reached** before the run's cut at 20:07Z.
- **The raw TPC-H record in the records folder is empty**: the step that strips plans failed on a
  `run` record. The full file is `~/.cache/amphora-perf/bench-s6-run/out/tpch.jsonl` on adacachy2,
  and the tables above were built from it.
- **The suite's answer check is thin.** It compares rows per statement across engines, not the rows'
  values. TPC-H's check (`bench/planner/answers.py`) compares every row.
- **Harness note.** The first build (r1464) stopped with "cannot update the lock file
  bench/Cargo.lock because --locked was passed". The same build without `--locked` (offline) left
  bench/Cargo.lock unchanged. Not chased further.
