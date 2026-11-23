# Bench lane S4 on adacachy2: hand-over (2026-11-23, 12:28Z to 13:20Z)

Ada's goal is Amphora about 10x faster than every competitor, proven with the same client on the same platform.
Lane S4 carries lanes S2 and S3 (their hand-overs are beside this file). This hour built the bench, merged S3's
fairness fix and lane P's server target, fixed two harness faults that had kept Amphora out of every replication
run, and started the timed families as one background run (r583) that commits each family's records and TABLE.md
to main as it ends.

## Losses first

Ladder numbers below are rounds 1 and 2 of 5 from r583 (12:57Z to 13:01Z, load 14 to 22, CPU pressure some
under 2%, I/O some 3 to 5%), runs at --seconds 5 after --warmup-s 1, quorum column unless said. Each is one run
per round, so read them as the size of the gap, not a final figure; family a's TABLE.md has all 5.

1. **Amphora's replicated commit rate does not grow with writers: dqlite is 6x to 12x ahead at 1 writer, rqlite 18x to 28x at 32 writers and 68x to 75x at 128.**

   | system (column) | 1 writer | 32 writers | 128 writers |
   |---|---|---|---|
   | amphora-procs-persisted (quorum) | 19.6, 20.2 /s, p50 39-49 ms | 13.8, 16.0 /s, p50 1.47-1.64 s | refused (loss 2) |
   | amphora-procs-replicated (quorum) | 13.0, 14.4, 17.0 /s, p50 56-76 ms | 18.0, 21.6 /s, p50 0.97-1.01 s | 17.6 /s, p50 7.8 s |
   | dqlite (quorum) | 131.4, 158.8 /s, p50 6.3-7.9 ms | 149.6, 153.2 /s, p50 141-150 ms | 134.8, 147.6 /s, p50 565-647 ms |
   | rqlite (quorum) | 23.8, 27.4 /s, p50 36-41 ms | 389.0, 397.8 /s, p50 79 ms | 1194.6, 1312.2 /s, p50 96-97 ms |
   | litefs (primary only) | 31.0 /s, p50 31 ms (one run) | refused | refused |
   | libsql (primary only) | refused | refused | refused |

   Amphora does 13 to 22 commits a second at every writer count, so each commit seems to pass alone through a
   step of about 50 to 70 ms. That is a guess from these rates, not a profile. dqlite fsyncs on the same disk at
   6 to 8 ms, so the disk alone does not explain it. The next step is a profile of one member at 32 writers
   (perf, or wchan.py in bench/) in a run that isn't reported. Sample counts: Amphora's rungs have 65 to 108
   samples a run and dqlite's 657 to 794, so no p99.9 is quotable. rqlite's rungs at 32 and 128 writers have
   1945 to 6561.
**Family a finished: all 5 rounds, committed as 5f312452** (`2026-11-23-adacachy2-repl/TABLE.md`). Medians of 5
runs (min to max), commits/s:

| rung | amphora persisted | amphora replicated | dqlite | rqlite |
|---|---|---|---|---|
| 1 writer | 19.8 (19.6-23.2) | 14.4 (13.0-17.0) | 145.4 (131.4-174.0) | 24.6 (23.8-27.4) |
| 32 writers | 13.8 (12.2-16.0) | 19.0 (18.0-21.6) | 153.2 (129.6-196.4) | 372.8 (359.2-397.8) |
| 128 writers | refused in every round | 17.3 (17.0-17.6), only 2 of 5 rounds answered | 187.8 (134.8-234.8) | 1389.8 (1194.6-1478.6) |

Taking Amphora's better class at each rung, dqlite is 7.3x ahead at 1 writer, 8.1x at 32 and 10.9x at 128.
rqlite is 1.24x ahead at 1 writer, 19.6x at 32 and 80x at 128. No range overlaps Amphora's. The only quotable
p99.9s are rqlite's: 131 ms at 32 writers (at least 1796 samples a run) and 168 ms at 128 (at least 5973).
Every other rung has fewer than 1000 samples a run, and the table withholds those. Why Amphora is slower is not
yet known (benchmark checklist question 1): the limiter is unnamed, so this records a measured gap, not a
diagnosis. libsql never came up. LiteFS came up in 3 of 15 rungs (1 writer, 15.4 commits/s, primary only).

2. **Amphora persisted refuses writes under concurrency, and again after a real kill (Amphora, not the
   bench).** At 128 writers, with no kill at all, both rounds' persisted commit rung failed with
   `no live node acknowledged a write within --deadline-s: node 2: refused a write: Invalid { what:
   "prepared transaction", why: "the database advanced since preparation" }`. The 5-kill failover smoke
   (32 writers, members really killed by perf/bench-s3) hit the same refusal after at least three kills. The
   same smoke on `replicated` passed (lost 0). This is a crates/amphora fault, outside this lane's bench-only
   scope. Family e (100 kills) will record it again.
2b. **libsql and LiteFS never came up, so they have no numbers yet.** libsql: `libsql primary didn't come up to
   set its durability`, beside `Address already in use` in its log. LiteFS: `consul didn't take leadership`,
   except one 1-writer rung. These are faults in the competitors' harness on adacachy2
   (bench/servers/repl/libsql.sh, litefs.sh), likely ports again (loss 4). Until they are fixed, the
   primary-only column has one LiteFS run and nothing for libsql.
3. **The ladder never measured Amphora before today.** `bench/repl_ladder.sh` runs `repl_members.sh`
   directly; the file was mode 644 on main since ff1b0235, so every amphora-procs rung failed with
   `Permission denied` and only the competitors were timed. Fixed on main as ed607607 (mode 755).
4. **Members on 54290-54292 can fail to bind.** Those ports sit in adacachy2's ephemeral range
   (32768-60999), and the first smoke failed with `Address already in use`. r583 runs Amphora's members on
   29290-29292 (`PROCS_PORT`, and the same addresses in catch-up and failover). The repository's default
   (repl_ladder.sh `PROCS_PORT=54290`) still sits in the range; a successor may move it below 32768.
5. **The suite has no amphora-serve target.** Lane P wired `amphora-serve` into main.rs's conversation-bench
   targets only; `bench/suite.sh amphora-serve` answers `the suite has no target amphora-serve`. So family c
   (SQL suite on servers) has the five engines only, and Amphora's server is compared in family c2: the
   conversation bench (run.sh's own example: `--sessions 1000 --preload 100 --writers 32 --seconds 30`) on
   postgres, orioledb, cedardb, mysql, clickhouse-durable and amphora-serve, all in containers. A suite entry
   for amphora-serve is the successor's code change in bench/src/suite.rs.
6. **clippy on the bench is not clean on main.** `cargo clippy --release --all-targets -- -D warnings` on
   bench/ at 7ac4153d gives 164 errors; 161 are outside the merged lines (main.rs 26, oracle 20, eqp 16, ...).
   Three are on lines S3's fix added (bench/src/repl.rs 1160 `std::env::args`, 1318 type complexity, 1379
   `eprintln!`), each the pattern the crate already uses at 579, 910, 1241 and 1373. Not changed: a change
   would have rebuilt the LTO binary before the timed runs.

## Done this hour

- **One shipping build serves every family.** perf/bench-s4 at 7ac4153d is main 364266aa + perf/bench-s3
  2a668d7e (members killed by kill -9, restarted on their data with `--reopen`) + perf/pgwire 2f976a33 (bench
  target `amphora-serve`, bench/servers/amphora.sh, run.sh). Both merged without conflicts and touch bench/ only.
  Built in `~/.cache/amphora-perf-target/bench-s` with S2's env.sh (thin LTO, codegen-units 1, overflow checks
  on) and `CARGO_INCREMENTAL=0`: 4 min 24 s, run r491. kiln refused cpus=16 twice (adacachy2 had 0, then 10 idle
  cores: the writes lane's A/B), so the untimed build ran at needs cpus=8 with 8 jobs. Load 24.8 at its start
  and 42.6 at its end; CPU pressure some avg10 0.7% then 24.4%; I/O some about 4%.
- **Checks:** the repl tests pass (6 of 6: `cargo test --release -- repl:: serve::`). Failover smoke
  (`--kills 5 --writers 32 --settle-s 1`, ports 29290-29292): replicated passed (lost 0, each failover 0.88
  to 3.42 s, n = 5: not a quotable number); persisted refused (loss 1). amphora-serve's image built in
  rust:1-bookworm with the workspace's release profile (the shipping profile).
- **Merged to main** as b906d0f0 (merge of 7ac4153d with origin/main), then ed607607 (loss 3).
- **Timed families, r583** (`~/.cache/amphora-perf/bench-s4/families.sh` on adacachy2, sha256 b90736d2...),
  needs linux/amd64,cpus=16,mem=48g, started 12:57:06Z at load 14.4, CPU some 0.4%, I/O some 2.9%. In order:
  a (replication ladder), b (in-process suite), c (server suite), c2 (conversation bench with amphora-serve),
  d (S3's catch-up with derived backlogs), e (failover, 100 kills, AC-T1.8.4). 5 rounds each, order reversed
  every round, load and CPU/IO pressure logged at each round's start and end (`rounds.txt`, `load.txt`).
  - Records go under `bench/records/2026-11-23-adacachy2-{repl,suite-inproc,suite-servers,convo-servers,repl-catchup,repl-failover}/`.
  - Commits come from a worktree at origin/main (`bench-s4/commit.sh`), never into the clone the runs build from.
    S2's commit.sh merged origin/main into that clone, so other lanes' engine changes would have been rebuilt
    and timed mid-series.
  - Every TABLE.md (S4's table.py) prints the smallest sample count per run and withholds a p99.9 resting on
    fewer than 1000 samples.
  - Containers: c and c2 remove, by ID, only the `amphora-bench-*` containers and the engines' images that
    appeared since c began, and the lane's `amphora-bench-amphora-image`.
- **TiDB's version comes from TiUP's manifest:** snapshot.json lists tidb.json version 13088, tikv 7353, pd
  7920 and ctl 12140. The newest stable release all four publish for linux/amd64 is **v8.5.8** (tidb released
  2026-08-27; tarballs of 114, 421, 66 and 565 MB). Run `bench/servers/scale/fetch.sh DIR tidb v8.5.8` after
  the timed runs, since it shares their disk. CockroachDB's download was not retried.

## Not done

- Scale-out runs, backup_restore and Vitess: not started (time). Scale-out still needs two or more Linux
  hosts reachable by Docker or ssh.
- Picked inputs still in the families: `--settle-s 1` (failover), the ladder's `--seconds 5`, the suite's
  `--seconds 1 --warmup 0.5`, the conversation bench's `--seconds 30` (run.sh's example). Short windows give
  fewer than 1000 samples at low client counts; the tables withhold those p99.9s.
- S3's note stands: each member's relay thread spins on `yield_now`, one core per member.

## Left on adacachy2 (this lane's)

`~/.cache/amphora-perf/bench-s4/` (scripts, smoke, smoke2, aborted-repl-r580, commit-wt worktree); docker
volumes `amphora-bench-amphora-cargo` and `amphora-bench-amphora-target` (amphora-serve's build cache);
`~/.cache/amphora-perf/bench-s` checked out at 7ac4153d with repl_members.sh +x and S2's setup.sh GOROOT
line uncommitted. That line goes to main with family a's commit.
