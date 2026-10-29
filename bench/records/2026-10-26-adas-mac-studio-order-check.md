# The order check over two oracle campaigns: one estimate per joined set

Step 4 of the planner plan (kiln plan 01a16eab7f457673b4cf9933adacfc4d; `docs/design/planner.md`,
decision 13; CLAUDE.md §7). `amphora-bench --oracle`, every oracle in turn, the order check among
them (`bench/src/oracle/checks.rs`, `Case::Order`).

- **Hardware:** adas-mac-studio: Apple M4 Max, 14 cores, 36 GiB, macOS 26.2.
- **Binary:** amphora-bench in release from a clean export of d762dc2 (`git archive`), the change's
  last commit.
- **Reference:** SQLite 3.54.0's amalgamation (SHA3-256 7b670a62…2d93, as `bench/oracles/sqlite.sh`
  checks it), compiled with `sqlite.sh`'s options; the hash checked with Python's hashlib, since
  macOS's openssl has no SHA3.
- **Runs:** `--oracle --seconds 1800 --seed S --reduce-seconds 20 --features bench/corpus/features.json
  --sqlite <library>`, seeds 11 and 12 side by side, 2026-10-26 from about 10:20Z.

## The campaigns

| seed | databases | checks | order checks | order failures | order skips: no estimate | CERT checks | CERT failures |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 11 | 75,042 | 3,041,524 | 375,206 | **0** | 34,464 | 375,206 | 1 |
| 12 | 77,052 | 3,123,170 | 385,255 | **0** | 35,433 | 385,255 | 1 |

Each oracle's checks, passed, failed (seed 11 / seed 12): diff 375,208 / 385,256 checks, 6 / 9
failed; dqp 0 / 0; eet 0 / 1; norec 0 / 0; pqs 8 / 8; tlp 4 / 2; cert 1 / 1; order 0 / 0. A base
query the order check skips for "no estimate" is one Amphora refuses or plans without a join
estimate (a compound select, no table); a form Amphora refuses where SQLite answers it would be a
failure (`derived`), and none was. The summaries (`summary.json`) and findings are on adas-mac-studio
under `~/.cache/order-runs/final-s11` and `final-s12`.

## The same campaigns again, with their summaries here

adas-mac-studio went offline before its summaries could be copied, so both campaigns were run again
on adas-mac-studio-2 (Apple M4 Max, 14 cores, 36 GiB, macOS 26.2) with a release build of the same
clean export of d762dc2 and the same reference SQLite, 2026-10-26: their `summary.json` files are
`2026-10-26-adas-mac-studio-2-order-check/seed11/summary.json` and `seed12/summary.json`, byte for
byte as the runs wrote them (SHA-256 b5b2400c…f2f4 and a421ce5f…b8c2).

| seed | databases | checks | order checks | order failures | order skips: no estimate | CERT failures |
|---:|---:|---:|---:|---:|---:|---:|
| 11 | 100,528 | 4,074,293 | 502,632 | **0** | 46,131 | 1 |
| 12 | 106,159 | 4,302,451 | 530,789 | **0** | 48,610 | 1 |

## The campaigns' other findings, replayed on both builds

Every finding of the two adas-mac-studio-2 campaigns, replayed with `--replay-check` (each of its
queries on Amphora against the reference SQLite) by release binaries of dbef789 and of d762dc2 on the
same machine (`2026-10-26-adas-mac-studio-2-order-check/replay.jsonl`, SHA-256 58f89c9a…9a6c): of
43 findings, 36 fail on both (the two CERT findings, step 5's, and 34 others: wrong answers, errors
where SQLite answers, and three panics, "cursor id N is None"), 7 pass on both (their replay doesn't
reproduce what the campaign saw), and none fails on d762dc2 alone. The change brings none of them:
they predate it, and wait for fixes of their own.

## Before the change, and on the way

| build | run | order checks | order failures |
|---|---|---:|---:|
| dbef789 (before) | order only, seed 11, 180 s, adas-mac-studio-3 | 435 | 33 |
| a2f7ca6 | every oracle, seed 11, 1,800 s, adas-mac-studio-3 | 173,138 | 1 |
| a2f7ca6 | every oracle, seed 12, 1,800 s, adas-mac-studio-3 | 184,538 | 1 |
| 878db51 | every oracle, seed 11, 1,800 s, adas-mac-studio-3 | 201,838 | 0 |
| 878db51 | every oracle, seed 12, 1,800 s, adas-mac-studio-3 | 233,635 | 0 |
| d762dc2 | every oracle, seeds 11 and 12, 1,800 s each (above) | 375,206 and 385,255 | 0 and 0 |

Each failure found was cut down, filed under its cause in `bench/oracles/findings.md` ("The order
check"), and fixed with a test in `crates/engine/tests/planner.rs` that fails before its fix.

## CERT beside it

`--oracles cert --seed 20261025 --seconds 300 --reduce-seconds 10`, adas-mac-studio-2 (same chip):
dbef789 found 71 findings in 231,717 checks; every one of them passes `--replay-check` on d762dc2. A
run of d762dc2 found 6 in 2,060,680 checks, and each of the 6 fails `--replay-check` on dbef789 too:
none is new. The counts of checks differ because a failure's reduction spends the run's time.
