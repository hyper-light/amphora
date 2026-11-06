# Small durable commits: where the bytes go (kiln goal 60805ec4, part B)

The source notes for docs/design/store.md decision 35.

## The workload

The unify audit's batched leg (its harness `crates/zz-audit/src/batched.rs`, at kiln e2e67247: kiln's log index
`post`, 128 grams a `Database::call`, each call waiting until it is durable). Its 1 MiB of kiln's build and test logs
(`kiln-unify-logs.txt`, the first 1 MiB repeated as the harness does): 157 documents of 100 lines, 1,318 distinct
trigrams a document at the median, 15,639 distinct in all, 202,483 (gram, document) pairs, 27,914 postings
(gram, block of 64 documents), 1,665 posting calls and 157 more that open a document. About 174,000 of the postings'
upserts change a row that is already there.

`crates/amphora/tests/amplification.rs` draws a stand-in (no kiln text in the repository): 157 documents of 1,290
grams, 92% of them from 4,000 common grams and the rest from 17,000 others (about 15,000 distinct and 27,000
postings, 1,884 calls in all), with the harness's statements.

## Where Amphora writes (the test's counting disk)

Each write to the log or to the file counted, each flush taking 3 ms. Bytes a byte of the rows the calls commit
(13.97 MiB: 8 bytes a number, a text's or blob's length).

fedora (2026-11-06, load 12 and rising; sizes, not times):

| | log | file | all | file flushes |
|---|---|---|---|---|
| a copy at every 25 ms period | 16.6 | 10.4 | 27.0 | 359 |
| a 2 s period | 16.6 | 0.7 | 17.3 | 7 |
| packed log with deltas (decisions 16 and 17), 25 ms | 3.2 | 10.4 | 13.6 | 377 |
| packed log with deltas, 2 s | 3.1 | 0.7 | 3.8 | 7 |

adas-mac-studio-3 (2026-11-06 15:41-15:48Z, load 1.8-2.3), copies once `copy_after` frames wait (decision 35):

| copy_after | log | file | all | file flushes |
|---|---|---|---|---|
| 0 (every period, as before) | 16.6 | 9.9 | 26.4 | 270 |
| 1,024 | 16.6 | 3.7 | 20.3 | 53 |
| 2,048 | 16.6 | 2.2 | 18.8 | 28 |
| 8,192 | 16.6 | 0.7 | 17.3 | 7 |
| 32,768 | 16.6 | 0.2 | 16.8 | 1 |
| 8,192, packed log with deltas | 3.0 | 0.2 | 3.3 | 1 |

The log is about 31 page frames a call (231.8 MiB for 1,884 calls). The file the leg builds is about 1,000 pages.

## What a flush costs the disks (APFS)

`apfs-flush-bytes.py` (the probe carrier), adas-mac-studio-3, internal SSD, APFS, macOS 26.2, 2026-11-06 15:48Z, load
2.1-2.3: 1,000 writes into a 64 MiB file written before (overwritten in place, as the log is, decision 13), the change
in IOBlockStorageDriver's bytes written:

| each write | flushed | the disks wrote a write |
|---|---|---|
| 4 KiB | `F_FULLFSYNC` each | 113.4 KiB, 3.59 ms a write |
| 128 KiB | `F_FULLFSYNC` each | 226.4 KiB, 3.65 ms |
| 128 KiB | once, at the end | 126.5 KiB |
| 4 KiB | `fsync` each (not durable on Apple's disks) | 32.1 KiB, 0.04 ms |

So a durable flush (`F_FULLFSYNC`, Amphora's on Apple, decision 13) costs the disks about 100-110 KiB beyond the data
it makes durable: the file system's own writes, whatever the data. A caller that waits for each call to be durable,
as kiln's indexer does, pays one flush a call: the batched leg's 1,822 calls about 190 MiB of its disks' 616 MiB at
13e385d, a floor no format of the log removes. Only fewer flushes do: more rows a call.

## The bound on a quiet Mac

The leg's disks wrote, per MiB of text: the log (about 232 MiB), the file (at most a quarter of the log with decision
35: 58 MiB; 138 MiB before it), and about 107 KiB a flush (1,822 for the calls, a few for the checkpoints; 270 more
before). Before: 232 + 138 + 225 = 595 MiB (measured 616-626). With decision 35: at most 232 + 58 + 191 = 481 MiB,
expected 232 + 10 + 191 = 433.
