# CERT clean: the oracles on step 5 of the planner's plan, 2026-10-27

Kiln plan 01a16eab7f457673b4cf9933adacfc4d, step 5 ("The oracles' CERT clean"). Every oracle of
`amphora-bench --oracle` (TLP, NoREC, PQS, DQP, EET, CERT, the order check, the differential check
and the tables' rows) against the oracles' SQLite 3.54.0 (`bench/oracles/sqlite.sh`, built with
SQLITE_LIKE_DOESNT_MATCH_BLOBS), six runs at once, 1,800 s each: seeds 11, 12 and 13
(`--seed N --seconds 1800 --reduce-seconds 20`) and `bench/oracles/nightly.sh` with ORACLE_SEED
20261026, 20261027 and 20261028, three nights' seeds. The causes are filed in
`bench/oracles/findings.md` ("CERT clean").

**Host.** adas-mac-studio-2: Apple M4 Max, 14 cores, 36 GiB, macOS 26.2 (25C56). Each round a
release build (`cargo build --release --bin amphora-bench`) of the tree named below: an APFS clone of
the d762dc2 tree with `git diff 9fdd3a3 <tree> -- crates bench/src bench/oracles` applied (the patch's
SHA-256 prefix below). The six runs share its 14 cores, and the replays below ran beside the third
round for a few minutes (load average 9.0 at 08:17 and 9.1 at 08:44). These are correctness runs:
their counts depend on the machine's speed and load, their findings do not.

| round | tree | ran (UTC) | per run: CERT checks, failed | other oracles' findings |
|---|---|---|---|---:|
| `first-runs/` | 9fdd3a3 and step 5's CERT changes before their commits (patch e986053c2a8e8f92) | 2026-10-27 05:29 to 05:59 | 417,403 to 432,440, one failed (cause BN) | 98 |
| `second-runs/` | 19a78a5 (patch 601de2f99d8fcf8c) | 2026-10-27 07:02 to 07:32 | 418,760 to 432,615, none failed | 46 |
| `third-runs/` | 10f2461 (patch 247fb5be8fd5a3db) | 2026-10-27 08:15 to 08:45 | 431,435 to 438,746, none failed | 13 |

Each run's directory holds its `summary.json` and its findings, each cut down to its smallest form
(`findings/*.md`, replayed with `--replay-check`), and for the second and third rounds `causes.tsv`,
each finding's cause in `bench/oracles/findings.md` (the first round's causes are counted there).

**The third runs, on the step's last tree.** CERT failed none of its 2,607,858 checks; no query was
planned without an estimate (`skipped` holds only queries SQLite refuses too, 41 to 52 a run) and
Amphora refused none. The other oracles found 13, each the differential check's and each AH, SQLite's
LIKE optimization dropping an outer join's rows (a cause campaign 2 found): the same 13 as the
second runs'.

**Replays, on the third runs' binary.** `replay-cert.tsv`: every CERT finding of campaigns 1 to 4
and the injected run (126, `bench/records/2026-10-24-adacachy-oracles/`) and of step 5's own runs
(5), each passing `--replay-check` (exit 0). `replay-round2.tsv`: the second runs' 46 findings, the
28 of causes BY, BZ and CA passing; the 18 left are AH's 13 (SQLite's), Q's 4 and AP's 1, which the
oracles now skip rather than Amphora answering otherwise.

`atanh/`: the evidence for decision 2 of `docs/design/semantics.md` (cause BY), its own README.
