# The WAL regression, measured on a quiet Mac (2026-11-06)

Kiln goal 60805ec4, from the unify audit's follow-up (`findings/followup-r83.md`, adacachy at load 13-30, under
`ionice -c3 nice -n 10`). The audit found that decision 30 of docs/design/store.md (a spill writes a page back over its
frame, and the commit chains the checksums again) cut one big transaction's log but slowed it: 17.7 s against 10.9 s
at 4 MiB of log text, and 8 MiB past a 30 s deadline it had met in 21.1 s. It also found many small commits writing
778-1552 times the size of the log text.

- host: adas-mac-studio-3, Mac Studio (Mac16,9), Apple M4 Max, 14 cores, 36 GB, APFS on its internal SSD, macOS 26.2.
- binaries: the audit's `zz-audit`, release, rustc 1.99.1, against main 521e4bc ("new") and 542f5a9 ("old", before
  decision 30). sha256 begins 20056e430addcc95 and 9915bf11a7a6dfd2. The harness isn't in this repository: it is
  `crates/zz-audit` of the audit's tree on adacachy (main.rs 08f83d63df096ae0, batched.rs a298f9aafd83e736), with its
  corpus of kiln's build and test logs (`kiln-unify-logs.txt`, dcc7c852d76f6b42).
- load: 1.69 when measuring began, after three looks 30 s apart found it under 2 with no rustc, cc1, clang or ld. It
  rose under the runs' own work: 1.71, 1.86, 2.03, 2.26, 2.27, 2.29 and 2.40 at the start of each later phase, 2.69 at
  the end.
- runs: 2026-11-06 04:55:13Z to 04:58:13Z, two rounds, alternating new and old. A leg indexes that much log text in one
  `Database::call`, with a 120 s deadline. The batched leg posts 1 MiB in 1,665 calls of 128 grams, as kiln does.
- "disks wrote" is the change in `IOBlockStorageDriver`'s bytes written across a run, so it counts all the machine
  wrote.

## One big transaction

| leg | new, rounds 1 and 2 | old, rounds 1 and 2 | new, best against old |
|---|---|---|---|
| 1 MiB | 1.20 s, 1.19 s | 1.21 s, 1.19 s | 1.19 s against 1.19 s |
| 4 MiB | 6.51 s, 6.49 s | 6.39 s, 6.40 s | 1.6% slower |
| 8 MiB | 14.24 s, 14.16 s | 14.66 s, 14.60 s | 3.0% faster |

| | 1 MiB | 4 MiB | 8 MiB | disks wrote, all three legs |
|---|---|---|---|---|
| new, database and log grew | 39.9 MiB | 178.2 MiB | 360.9 MiB | 765 MiB, 646 MiB |
| old, database and log grew | 102.3 MiB | 2,774.4 MiB | 7,188.3 MiB | 10,133 MiB, 10,122 MiB |

The audit's slowdown doesn't reproduce here. Every leg answered, the 8 MiB leg in 14.2 s, and decision 30 costs at most
1.6%, at 4 MiB, while the disks write 13-16 times less.

## A spill that writes over computes no checksum (d2c7395)

Ada asked that the 1.6% at 4 MiB be cut. A spill that writes a page back over its frame computed the
frame's whole checksum, which the commit computes again (decision 30). Since d2c7395 it computes none.
Measured the same way, the fix against 542f5a9, 2026-11-06 07:36:52Z to 07:39:55Z, load 1.11 when
measuring began, 1.32 to 2.13 at the start of each later phase, 2.37 at the end. The fix's binary's
sha256 begins 3d82bd3c8ddf7b60.

| leg | fix, rounds 1 and 2 | old, rounds 1 and 2 | fix, best against old |
|---|---|---|---|
| 1 MiB | 1.19 s, 1.21 s | 1.21 s, 1.18 s | 1.19 s against 1.18 s |
| 4 MiB | 6.39 s, 6.39 s | 6.67 s, 6.45 s | 0.9% faster |
| 8 MiB | 14.24 s, 14.16 s | 15.11 s, 15.46 s | 6.3% faster |
| batched 1 MiB | 18.4 s, 18.2 s | 20.6 s, 18.5 s | 1.6% faster |

The disks wrote 786 and 658 MiB for the fix's three legs against 10,106 and 10,101 MiB, and 593 and
590 MiB for its batched leg against 619 and 598. At 1 MiB the two differ by 0.01 s, less than either
varied between the two runs (1.17 to 1.21 s).

## Many small commits

| | call p50 | call p99 | max | time | disks wrote, for 1 MiB of text |
|---|---|---|---|---|---|
| new, round 1 | 10.94 ms | 16.06 ms | 20.07 ms | 18.7 s | 616 MiB |
| new, round 2 | 10.95 ms | 15.90 ms | 18.91 ms | 18.7 s | 626 MiB |
| old, round 1 | 10.92 ms | 15.10 ms | 30.04 ms | 18.5 s | 622 MiB |
| old, round 2 | 10.93 ms | 15.98 ms | 24.76 ms | 18.6 s | 625 MiB |

The amplification reproduces at 616-626 times the text, the same on both, so decision 30 didn't cause it. The audit's
1,552 times counted `/proc/self/io` on btrfs at load 13-30, a different counter on a different file system. Each run
ended with 4 MiB of database and 208 MiB of log.

## The same A/B on a second Mac (adas-mac-studio-2)

The first A/B again, main 521e4bc ("new") against 542f5a9 ("old"), on a second machine of the same kind, by the
carrier's `wal-ab-mac.sh` under launchd.

- host: adas-mac-studio-2, Mac Studio (Mac16,9), Apple M4 Max, 14 cores, 36 GB, APFS on its internal SSD, macOS 26.2.
- binaries: the same harness and corpus, built there 2026-11-06 04:47:28Z and 04:49:28Z, release, rustc 1.99.1. sha256
  begins 3f99d26cadb4b381 (new) and 4c892b862936965d (old); they differ from studio-3's because the debug info names
  the checkout's path.
- load: the script waited from 04:49Z to 06:58:07Z for three looks 30 s apart under 2 with no rustc, cc1, clang or ld.
  1.53 when measuring began; 3.34, 3.36, 2.97, 2.69, 3.25, 3.06 and 2.56 at the start of each later phase, under the
  runs' own work and the host's; 2.13 at the end.
- runs: 2026-11-06 06:58:07Z to 07:01:01Z, two rounds, alternating new and old. Log: `~/.cache/wal-ab-studio2.log`
  there.

| leg | new, rounds 1 and 2 | old, rounds 1 and 2 | new, best against old |
|---|---|---|---|
| 1 MiB | 1.19 s, 1.19 s | 1.18 s, 1.18 s | 1.19 s against 1.18 s |
| 4 MiB | 6.30 s, 6.45 s | 6.41 s, 6.56 s | 1.7% faster |
| 8 MiB | 14.26 s, 14.24 s | 14.51 s, 14.21 s | 0.2% slower |
| batched 1 MiB | 18.4 s, 17.9 s | 18.2 s, 18.4 s | 1.6% faster |

The database and log grew as on studio-3: 39.9, 178.2 and 360.9 MiB new against 102.3, 2,774.4 and 7,188.3 MiB old.
The disks wrote 839 and 711 MiB for new's three legs against 10,090 and 10,110 MiB, and 581 and 577 MiB for its
batched leg against 580 and 588. The batched calls' p50 was 10.05-10.17 ms on both, their p99 15.82-17.04 ms.

So studio-2 agrees with studio-3: no leg is slower by more than the runs varied among themselves (old's 8 MiB leg took
14.21 to 14.51 s), and the small commits' amplification is the same on both builds. Its 4 MiB leg doesn't show
studio-3's 1.6% for 521e4bc: here new was the faster, which puts that 1.6% within what the two machines vary by.

## The same legs under load (fedora, for comparison)

fedora-2, btrfs, 2026-11-05 06:35-06:44Z, load 25-43 from other work, `nice -n 10`, one round of new then old, twice.

| leg | new | database and log grew | old | database and log grew |
|---|---|---|---|---|
| 1 MiB | 2.64 s, 34.43 s | 39.9 MiB | 2.34 s, 2.59 s | 102.3 MiB |
| 4 MiB | 15.33 s, 54.57 s | 178.2 MiB, 178.4 MiB | 38.89 s, 18.68 s | 2,774.4 MiB |
| 8 MiB | 37.29 s, 63.32 s | 360.9 MiB | UnknownOutcome at 120 s, twice | 2,290.1 MiB, 5,737.5 MiB |

Not a record of speed: new's 34.43 s at 1 MiB is a stall of the host's. At 1 and 4 MiB the noise is larger than any
difference. At 8 MiB old didn't finish, so there the old log's gigabytes cost more than decision 30 does.

## Not counted

A first run on this host, 04:38:58Z to 04:42:02Z, gave much the same times, but its load check read nothing:
launchd's PATH had no `/usr/sbin`, so `sysctl` wasn't found and an empty load passed as calm. It is left out.
