# Small durable commits on a quiet Mac: decisions 35 and 37 against 13e385d (2026-11-07)

Kiln goal 60805ec4, part B (docs/design/store.md decisions 35 and 37). The unify audit's legs, as
`bench/records/2026-11-06-studio3-wal-regression.md` measured them, on three trees: 13e385d (base,
the branch's start), da367e6 (step 1: the checkpoint copies a page once for many commits, decision
35) and 40c9c84 (step 2: a commit logs its pages' changes in a mixed log, decision 37).

- host: adas-mac-studio-3 (kiln runner 2768d0d5d2974f1e21d348edadff9d1b; `hostname` said
  Adas-Mac-Studio-4.local), Mac Studio, Apple M4 Max, 14 cores, 36 GB, APFS on its internal SSD,
  macOS 26.2. Ada accepted the quiet Mac Studios for these measurements.
- trees: each the commit's tree exactly, checked by `git write-tree` (base f3257134, step 1 d42fbd9e,
  step 2 80b8794e), made from the wal-new tree (521e4bc) with a patch, with the audit's harness
  copied in: `crates/zz-audit` of the audit's tree on adacachy (main.rs 08f83d63df096ae0, batched.rs
  a298f9aafd83e736) and its corpus of kiln's build and test logs (`kiln-unify-logs.txt`,
  dcc7c852d76f6b42), the same harness and corpus as the 2026-11-06 record.
- binaries: `zz-audit`, release, rustc 1.99.1, sha256 beginning 8d5d5ce8e9533b59 (step 2),
  e4d2b7420693dbc5 (step 1), fe8b7613f85dae64 (base).
- load: the host's heavy lock held; measuring began once the load had stayed under 2 for 90 s with
  nothing compiling (1.38 at 01:03:09Z), and rose under the runs' own work: 1.79 to 2.80 at the start
  of each later phase, 2.58 at the end (01:07:21Z), as in the 2026-11-06 record (1.69 rising to 2.69).
- runs: two rounds, each step 2, step 1, base in turn: the legs of 1, 4 and 8 MiB of log text in one
  `Database::call` each (deadline 120 s), then the batched leg of 1 MiB (1,665 calls of 128 grams, each
  waiting until durable, as kiln's indexer posts). "Disks wrote" is the change in
  `IOBlockStorageDriver`'s bytes written across a run (all the machine wrote).

## Many small commits: the batched leg

| tree | time | call p50 | call p99 | disks wrote for 1 MiB of text | log at the end |
|---|---|---|---|---|---|
| base, round 1 | 15.2 s | 9.04 ms | 14.01 ms | 599 MiB | 208 MiB |
| base, round 2 | 14.8 s | 8.97 ms | 13.95 ms | 624 MiB | 208 MiB |
| step 1, round 1 | 14.9 s | 9.00 ms | 13.05 ms | 442 MiB | 208 MiB |
| step 1, round 2 | 15.1 s | 9.02 ms | 13.99 ms | 427 MiB | 208 MiB |
| step 2, round 1 | 13.2 s | 6.05 ms | 13.35 ms | 221 MiB | 24 MiB |
| step 2, round 2 | 15.0 s | 8.97 ms | 14.06 ms | 232 MiB | 24 MiB |

Decision 35's bound for step 1 is at most 481 MiB for a MiB of text (its log, about 232 MiB; at most a
quarter of that copied into the file; and about 107 KiB of the file system's own writes for each of
its 1,822 durable flushes, measured on this host's APFS). Step 1 wrote 427-442 MiB, within it, against
the 433 it expected; base wrote 599-624, as on 2026-11-06 (616-626). Step 2 wrote 221-232 MiB, 2.7
times less than base: its log ended at 24 MiB instead of 208 (decision 37's bound is on the log, at
most 4 bytes a byte of rows; `crates/amphora/tests/amplification.rs` counts 3.0, against 16.6). About
190 MiB of step 2's writes are the per-flush floor of decision 35: one `F_FULLFSYNC` for each durable
call, which no format of the log removes.

## One big transaction: the 1, 4 and 8 MiB legs

| leg | base, rounds 1 and 2 | step 1 | step 2 | step 1, best against base | step 2, best against base |
|---|---|---|---|---|---|
| 1 MiB | 1.28 s, 1.24 s | 1.27 s, 1.20 s | 1.31 s, 1.29 s | 3.2% faster | 4.0% slower |
| 4 MiB | 7.04 s, 6.99 s | 6.97 s, 6.40 s | 6.99 s, 7.14 s | 8.4% faster | the same |
| 8 MiB | 15.20 s, 15.26 s | 14.61 s, 15.57 s | 15.67 s, 15.78 s | 3.9% faster | 3.1% slower |

| | 1 MiB | 4 MiB | 8 MiB | disks wrote, the three legs, rounds 1 and 2 |
|---|---|---|---|---|
| base, database and log grew | 39.9 MiB | 178.2 MiB | 360.9 MiB | 682 MiB, 648 MiB |
| step 1 | 26.7 MiB | 178.4 MiB | 361.0 MiB | 720 MiB, 677 MiB |
| step 2 | 19.9 MiB | 146.9 MiB | 296.2 MiB | 512 MiB, 637 MiB |

Step 1's legs are no slower than base's at any size. Step 2's are slower at 1 MiB (1.29 s against
1.24 s) and 8 MiB (15.67 s against 15.20 s), each round of it slower than each of base's at those
sizes, while its log and file grow 18% less: its commit writes the transaction's pages as compressed
records (LZ4) rather than whole frames. Decision 37's falsifier names this; it is the next thing
step 2 must answer.

## Not measured here

The comprehensive suite's write statements (decision 37's falsifier: the bulk statements against
page-a-frame) wait for a calm window on adas-mac-studio-2, whose load stayed at 3 to 8 from
2026-11-06 21:38Z to 2026-11-07 01:11Z (other products' benchmarks, kiln CI, Docker Desktop).
