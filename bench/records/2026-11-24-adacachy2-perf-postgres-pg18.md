# Lane PG18 (postgres), 2026-11-24: unit 8's analysis is on main

Hour: 2026-11-24T06:57:13Z to 07:57:13Z (from the lane's first command). Branch `perf/postgres-pg17`, with main
75efff54 merged in (it had moved only in records since the branch's last merge). Pushed to main as 95669341 at
07:06:12Z. No code changed in this lane. Not a speed change: no A/B, nothing timed.

The runs checked out 65983e90. Its message had no co-author line, so it was rewritten before the push:
95669341 has the same tree (108a8e68) and the same parents (3c3aa6c0, 75efff54).

## Losses first

- Only Linux ran the merged tree. The macOS ask with needs alone stayed 8 to 15 runs behind others and was
  stopped unstarted once the Linux run had passed. PG17's macOS runs were on 9c324607 and 25538fc1. No Windows
  run has been made of unit 8.
- amphora-engine ran only its `dialect::`, `vtab` and `alter` tests (35 of its 3,230 library tests).
- What unit 8 can't do is in PG16's record. Nothing of it changed here.
- The new filter drops 27 of the sweep's 14,382 calls, nine each of three functions initdb's scripts make. 19 answers
  `42P16` to four of them, `plpgsql_validator` of int4, int8, regclass and unknown, and `42883` to the other 23
  (read from PG16's functions.txt, which had no other row of the three). A client that makes those four calls is
  told `42883` by Amphora.
- 25 of the 37 functions with a default are checked against no server (PG17's record).
- `gh` isn't signed in on this machine, so whether a pull request used the deleted branch was not checked.

## Answers from a real 19 server

The server is PostgreSQL 19.0 (Debian 19.0-1.pgdg13+1), the postgres:19 image bb6e70add742, in a container of the
lane's own (removed when its run ended). The container was new, so nothing under its /tmp was from an earlier run.

- The regeneration (07:00:57Z, 10 s, exit 0, on 65983e90 in its own git worktree): `git status` and `git diff`
  were empty after it. operators.txt (710 lines), casts.txt (216), functions.txt (3,336) and statements.txt (441) are
  byte for byte what the server answered. The filter `oid < 12000` was in force: the server counted 14,355 calls
  asked, 3,336 of them other than `42883`.
- The control (07:13:45Z, 9 s, exit 0, same worktree): line 7 of each of the four files damaged, then regenerated.
  `git diff` was 4 files before and empty after, and each file's hash was the committed one's.
- The functions of pg_catalog from OID 12000 on but the RI_ ones, read off the server: `dsnowball_init`,
  `plpgsql_inline_handler` and `plpgsql_validator` (one argument: the 27 calls), `dsnowball_lexize` (four) and
  `plpgsql_call_handler` (none), which the sweep never asks. PG17 had inferred the first three.

## Runs

All on adacachy2, `sync: false` from the GitHub clone, CARGO_HOME, CARGO_TARGET_DIR
~/.cache/amphora-perf-target/postgres, CARGO_INCREMENTAL=0, nice -n 10. The load is at the run's start.

| Run | Tree | Command | Result |
|---|---|---|---|
| r729, 07:00:24Z, cpus=8,mem=24g, load 31.7 35.6 50.2 of 32 cores | 65983e90 | `bash scripts/check-production.sh` | exit 0 in 97 s. The log shows `Checking amphora-engine`, `Compiling amphora-engine` and `Checking amphora-engine-parser`. `engine-ratchet: 10617 left in the engine's production code across 17 lints, as the ratchet holds` |
| same | same | `cargo fmt --all --check` | exit 0 |
| same | same | `cargo clippy -p amphora-pg-analysis -p amphora-pg-catalog -p amphora-engine --all-targets --locked -- -D warnings` | exit 0 in 60 s. The log shows `Compiling amphora-engine`, `Checking amphora-pg-catalog`, `Checking amphora-pg-analysis` |
| same | same | `cargo test -p amphora-pg-analysis -p amphora-pg-catalog --locked --no-fail-fast -- --test-threads=8` | 8 and 7 passed, 0 failed |
| same | same | `cargo test -p amphora-engine --lib --locked --no-fail-fast -- dialect:: vtab alter --test-threads=8` | 35 passed, 0 failed, 3,195 filtered out (ended 07:05:02Z, load 65 at its end) |
| r731, 07:00:33Z, cpus=2,mem=4g | 65983e90, its own worktree | `scripts/postgres-answers.sh` on the 19.0 server above | exit 0, `git diff` empty |
| r791, 07:08:46Z, cpus=2,mem=8g, load 76.1 66.8 59.4 | main 95669341 after the merge, every .rs touched so cargo rechecks | `bash scripts/check-production.sh` | exit 0 in 118 s. The log shows `Compiling amphora-engine`, `Checking amphora-engine-parser`. 10617 left, as the ratchet holds |
| 07:13:45Z, cpus=1,mem=2g | 65983e90, the worktree | the answers control above | exit 0 |
| this machine | 65983e90 | `cargo fmt --all --check`, `python3 scripts/test_requirements.py`, `python3 scripts/vendor.py check postgres` | pass |

Waiting for room. r779 (8 cores) and r780 (1) got no runner, then kiln's server answered late once and no longer
knew them. adacachy2's log folder had nothing of theirs, so neither started. r790 (4 cores) and nine smaller asks were
refused (one of them seen only as a failed call): adacachy2 had 0 to 2 idle cores of 32. r791's two cores came a
minute after r790's refusal, and the control's one core at its tenth ask.

`perf/postgres-pg16-answers` was deleted from GitHub at 07:11Z. It was b3594de9 (PG16's work in progress, not for
merge). No file in it is missing from this branch, and its functions.txt differs only in the four
`plpgsql_validator` rows.
