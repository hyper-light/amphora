# Analytics vs SQLite and DuckDB on Mac Studio 2: lane M, hour 1 (2026-11-23)

Goal (Ada): Amphora about 10x faster than its competitors on every query type, on the same client and
platform. This lane owns the analytic comparison, TPC-H SF1 and JOB, Amphora vs SQLite vs DuckDB, on
Mac Studio 2 (runner d1323898b255a3ed435eb83fa0f7b0cf, hostname Adas-Mac-Studio.local, kiln label
adas-mac-studio.local-2; 32 cores, 96 GiB).

No timings from this hour yet. The full run, r327, is queued (see "Running").

## What r190's q05 "stall" was

r190 (Mac Studio 3, 2768d0d5, head 2d939193) stopped printing for about an hour after "q05 amphora: no
time". It was the harness's default, not a hang and not swapping. Before an Amphora target's timed
runs, `--planner` runs the query once on its row-counting connection with `--count-timeout-s`, which
defaults to 1800 s. Every other run uses `--timeout-s` (120 s in that script). From r190's own files:

| file (answers-tpch/tpch-sf1/q05/) | written |
|---|---|
| query.sql (q05 begins) | 18:47:17 |
| amphora.tsv (timed out) | 19:19:17, 32 min 00 s later = 1800 s counted + 120 s timed |
| amphora-adaptive.tsv (timed out) | 19:51:17, 32 min 00 s later again |
| sqlite.tsv, duckdb.tsv | 19:51:21, 19:51:28 |

Both timeouts were enforced to the second. Both Amphora targets' q05 records read
`counted: timed_out` and `runs: [timed_out]`. SQLite finished q05 in 1.73 s and DuckDB (1 thread) in
0.10 s. So Amphora, at that head and on that build, did not finish q05 in 1800 s: a loss of more than
1000x against SQLite. That is r190's build (bench/Cargo.toml's own release profile, no LTO, 16
codegen units). This lane's run re-measures it on the shipping profile.

Fix in the run script: `--count-timeout-s 120`, the same bound as a timed run.

## Second harness fault: no load average on macOS

`load_average()` read /proc/loadavg, which only Linux has. On macOS every run's `load` was null (r190's
records), and `--max-load` never held a run back. Fixed in f19d9a50: getloadavg(3), which Linux's and
macOS's C libraries both have. The run passes `--load-wait-s 0`, so every run records the load it
began at, and kiln's admission (`cpus=28` idle) is the gate. Each phase's start and end also log
`vm.loadavg`, free pages, swapouts and the top 5 processes by CPU to phases.log.

## How it's built

amphora-bench at the lane's commit, in its own clone on the Mac (~/.cache/amphora-bench-m/src), built
with Amphora's shipping release profile (the root Cargo.toml's) applied to the bench workspace:
`CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true`, with bench/Cargo.toml's `debug = "line-tables-only"`.
The bench has no Cargo.lock of its own. The resolved one is copied beside the output
(Cargo.lock-<sha>), as is profile-<sha>.txt.

## Each engine's settings, and whether production runs them so

| engine | settings (from the harness, bench/src/planner/engines.rs) | as production runs it? |
|---|---|---|
| amphora | one connection, `PRAGMA cache_size = -2097152` (2 GiB), one thread; the bench's global allocator counts bytes per thread (a thread-local add per allocation) over the system allocator | yes, except the counting allocator, a small tax on Amphora alone (SQLite and DuckDB allocate through C malloc) |
| amphora-adaptive | the same, with adaptive joins and re-planning on | (a variant of the same engine) |
| sqlite | bundled 3.53.2 (rusqlite 0.40.2); `journal_mode = WAL`, `synchronous = NORMAL`, `cache_size` 2 GiB, `mmap_size` 0 (its default), `temp_store` its default (file) | the same cache as Amphora. Not tuned: a read-heavy SQLite in production often sets `mmap_size`. That matters for JOB, whose database is larger than the 2 GiB cache. Open item below |
| duckdb | bundled v2.0.1, `SET threads = 1`, `memory_limit` its default (80% of RAM) | a one-thread baseline, not production |
| duckdb-mt | `threads` its default (every core, 32), `memory_limit` its default | yes, DuckDB's production default |
| ladder | Amphora: `ParallelMode::On`, parallel LIMIT, `set_parallel_cores(n)`; duckdb-mt: `SET threads = n`; n = 1, 2, 4, 8, 16, 32 | yes |

## What the harness does not do

- It doesn't alternate engines. For each query it runs each engine in turn: a warming run (Amphora:
  the counted run), then its 5 timed runs back to back. Drift between engines within a query is
  seconds apart, but it isn't spread across them.
- Under `--answers` the first timed run writes its rows, and its time includes that. bench/planner/ratios.py
  leaves that run out, so each median is over 4 runs.

## Machine state

kiln said the Mac was reserved for this lane, but at 04:20Z another agent's `slates_server` debug
tests ran there at about 21 cores (from ~/slates-work), and Spotlight's mds_stores ran at 85% of one
core. kiln wouldn't start the run with `on` while fewer than 28 cores were idle: it refused instead of
queueing. So r327 asks for `macos/arm64,cpus>=32,cpus=28,mem=64g`, which only this Mac meets, and
queues. `mem=80g` could never be met: 69-78 GB were free with nothing of this lane's running.

## Running

r327: `bash .git/lane-m-run.sh` on Mac Studio 2, background, 12 h timeout. It runs prep (TPC-H dbgen
SF1, the IMDb fetch from CWI with retries, the shipping-profile build with 8 jobs), then the phases
below. Outputs go to ~/.cache/amphora-bench-m/out/<sha>/ on the Mac:

1. tpch.jsonl, tpch.log: TPC-H SF1, amphora, amphora-adaptive, sqlite, duckdb, duckdb-mt, 5 runs,
   answers in answers/tpch-sf1/
2. tpch-cores.jsonl: the ladder, amphora vs duckdb-mt, 1 to 32 cores
3. job.jsonl, job.log: JOB (113 queries), the same five engines, answers in answers/job/
4. job-cores.jsonl: JOB's ladder
5. phases.log: each phase's start, end, exit code and load

Every phase passes `--resume`: rerunning `bash .git/lane-m-run.sh` from ~/.cache/amphora-bench-m/src
on the same commit continues where it stopped.

To read it: `python3 -I bench/planner/ratios.py <jsonl>` for the tables (losses first, geometric
means, timeouts), `python3 -I bench/planner/answers.py <out>/answers` for answer mismatches, and
`python3 -I bench/planner/summary.py <jsonl>` for estimates and planning time.

## Open

- SQLite's `mmap_size` and `temp_store`: decide whether production SQLite for analytics sets them and
  rerun SQLite tuned, before claiming a ratio against it on JOB.
- The counting allocator taxes Amphora alone. Measure it: one Amphora pass with it and one without.
- r190's partial q01 to q04 (untuned profile, old head, a loaded Mac with 479 MB free) are only for
  sanity-checking. Medians: q01 Amphora 1.96 s, SQLite 2.34 s, DuckDB-1t 0.21 s; q03 0.80 s, 0.50 s,
  0.08 s.
