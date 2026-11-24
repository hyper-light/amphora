# Lane S7 (bench), 2026-11-24 05:55Z to TODO-END: Amphora against Turso and SQLite, in process, main 0101aa38, adacachy2

Ada, 11-24: "Then lets do another bench vs our sqlite competitors." Goal: one measured, fair table of how far Amphora is from its SQLite-family competitors at today's main, on adacachy2. Rule 0 holds here: numbers say where Amphora is ahead or behind, not why. No cause is claimed and no profile was taken.

## Verdict, losses first

TODO-VERDICT

## What is in, and what is not

- In: **amphora** (one group, in process), **turso** (upstream Turso at 3e09a82, `turso_core` 0.9.0-pre.1, a connection a client) and **sqlite** (SQLite 3.53.2, bundled by rusqlite 0.40.2, WAL with `synchronous = FULL`, a connection a client). One binary (`amphora-bench`), one client code path (`bench/suite.sh` through `bench/run.sh`), the same 200,000-row data set, all three engines in the driver's process, on the same machine.
- Not in: **libsql**. The suite has no target for it (`bench/src/suite_db.rs` opens amphora, mvcc, turso, sqlite, postgres, orioledb, cedardb, mysql and clickhouse); its comparison is in the replication ladder (`bench/servers/repl`). `mvcc` is Amphora's own engine in MVCC mode, not a competitor. The PostgreSQL-family servers and MySQL are not SQLite-family and need a server, so they are left to S4's `suite-servers`.
- Statements: 17 of the suite's 52, the ones lane S6 ran (`--only`, in `rounds.sh`). Lane S4 ran all 52 (13:07:27Z to 14:29:16Z, 82 minutes for 5 rounds); this lane had 70 minutes including a build, so five rounds of all 52 could not fit and the S6 list was used instead. Items (200,000), clients (1, 32, 128), seconds (1) and warm-up (0.5) are S4's and S6's.

## How it ran

- Commit: 0101aa386850d1e8e641c6254238567adf9d8aa7 (origin/main when the lane started, 2b0d5e7c in its history). Clone `~/.cache/amphora-perf/bench-s` on adacachy2 (runner 00aad8daff2fa3c9daa65d53c425d450), HEAD at that SHA, `git status` empty. No tracked file was changed.
- Build (kiln run r561, 05:59:42Z to 06:04:05Z, 4 min 23 s at 16 jobs): `cargo build --release --offline --manifest-path bench/Cargo.toml --bin amphora-bench`, rustc and cargo 1.99.1, the bench workspace's release profile with lane S4's environment: `LTO=thin CODEGEN_UNITS=1 OVERFLOW_CHECKS=true DEBUG=line-tables-only INCREMENTAL=0`. No debug build. Binary kept at `~/.cache/amphora-perf-bin/bench-s7/amphora-bench-0101aa38...`, sha256 9587ed003a24ed89d6525606a0e84e378770b2730407e09fe32ad7ddfd3e118e (checked byte for byte against the build before and after the rounds).
- The first build attempt used `--locked`, as lane S4 did, and cargo refused it: `bench/Cargo.lock` is git-ignored and had not been brought up to date with main's new path dependencies (`amphora-pg-catalog`, and `tempfile` in the bench). `--offline` wrote it: the lock's diff is 9 lines (`bench-cargo-lock.diff`: `tempfile` and the new `amphora-pg-catalog` entry), and no registry crate changed version. The engines' versions come from `bench/Cargo.toml`, unchanged since S4's commit: turso 3e09a82, rusqlite 0.40.2.
- The binary is the one `bench/run.sh` starts from the clone: `target/release/amphora-bench` in the lane's target dir, compared with the saved copy before the run and after it.
- Runs (kiln run r584): `ROUNDS=5 bash rounds.sh all`, the scripts taken from branch perf/bench-s7b (commit 9155f400) and checked by sha256 before use. It asked for 16 cores and was refused (15 of 32 idle), then 12 cores and 24 GiB and was placed (`room for 12 CPUs`). That is a reservation, not a limit: the run's cgroup has no `cpu.max` limit the bench process in round 5 had `cpu.max` = `max 100000` and no cpuset file in its cgroup. `nice -n 10` from the suite, as S4 and S6 (the bench process in round 5 showed NI -4, so kiln's run sets a base below it; S4's and S6's runs were not checked for that). Data in the default `~/.cache/amphora-bench-data/<target>` (btrfs, zstd:1, no file attributes), as S4's records say. No environment variables set beyond those `rounds.sh` shows.
- Order: amphora, turso, sqlite in rounds 1, 3 and 5; sqlite, turso, amphora in rounds 2 and 4. A smoke pass ran first (clients 1 and 128, 0.3 s windows): every target answered all 34 statement-and-client cells with no error, and its answers were compared (`suite_answers.py`, 1,026 read answers of Amphora and of Turso against SQLite's, 0 differ).
- Each round's answers were compared the same way before any write of that round; the verdicts are in `roundN-answers.txt`: TODO-ANSWERS.
- Load, CPU pressure and I/O pressure sampled every 5 s (`phases.log`, `PHASES.md`), the machine's top processes at each round's start (`procs.txt`).

## Changes in the harness and the competitors since lane S4 (7ac4153d)

- **Turso waits on its own busy handler** (lane F3, b02b2759, the code; landed on main at 81b3cb3d, a64549cc names it). S4's harness answered Turso's `StepResult::Sleep` with a thread sleep and retried a busy write after a 200 µs sleep of its own; now the bench sets Turso's busy timeout (120 s) and, on Sleep, steps again, as Turso's SDK does. Every Turso write cell at 32 and 128 clients is therefore a different measurement from S4's.
- **SQLite's settings are the same** (WAL, `synchronous = FULL`, a 120 s busy timeout, now set through `busy_timeout(WAIT)` instead of a PRAGMA).
- **The suite checks every read's answers before it times anything** (lane S7/S8, d2a9e6ee): one untimed read per statement and client before the first timed run, which warms every target alike and was not in S4.
- **Amphora's checkpointer is told what its log holds** (lanes W1/S7, e57a028b, b83da2fe, 795faa83), **bench's sleeps wait on events** (F2, 534877d3), **`run.sh` hands the report through a file** (S8, 701486af), **the engine under test is Amphora at today's main**: 100+ commits of engine change since S4's commit, which this lane did not look at.

## Load that was not this lane's

TODO-LOAD
