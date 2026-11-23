# Bench lane S2 on adacachy2: hand-over (2026-11-23, 03:58Z to 04:53Z)

Ada's goal is Amphora about 10x faster than every competitor, proven with the same client on the same platform.
Lane S2 carries lane S's plan (bench/records/2026-11-23-adacachy2-bench-s-competitors.md). This hour produced no
numbers. It finished the competitor setup and queued units 1 and 2 as background kiln runs that write and commit
their own records.

## Done this hour

- The dqlite shim now builds. Setup run r251 had failed with `compile: version "go1.27.3" does not match go tool
  version "go1.27.4"`. adacachy2's shell exports `GOROOT=~/.gvm/gos/go1.27.3`, which overrides the Go that
  setup.sh fetched. Rerun r307 with GOROOT unset built `~/.cache/amphora-repl-bin/shim` (go-dqlite v3.0.4 against
  libdqlite 1.18.7). bench/servers/repl/setup.sh now sets `GOROOT="$BIN/go"` for the shim build. Unit 1's last
  commit carries that one-line change.
- The amphora-bench release build (shipping profile: thin LTO, one codegen unit, overflow checks on) is run r298,
  started 04:16:53Z at load 120. It had not finished at 04:50Z.

## Queued runs (on adacachy2, sync false, scripts in ~/.cache/amphora-perf/bench-s2)

Both take `flock ~/.cache/amphora-perf/bench-s2/timed.lock`, so they run one after the other, never together.
Each starts with `cargo build`, which waits on r298's build lock.

- r325, `unit1.sh`, needs cpus=16,mem=48g. It runs the replication ladder for 5 rounds with S's command, with the
  system order reversed each round and the load at each round's start and end in `rounds.txt`. It writes
  `bench/records/2026-11-23-adacachy2-repl/` (with `TABLE.md`: median, min-max and n of p50, p99, p99.9 and
  commits/s per system and writers) and commits it. It then runs failover and catchup through
  `repl_members.sh` (persisted and replicated) and through `repl.sh` with `SYSTEMS="rqlite dqlite litefs
  libsql"` into `...-repl-failover/`, and commits it.
  - Failover and catchup inputs: `--writers 32` (the core count, as the ladder's middle rung), `--kills 100`
    (AC-T1.8.4: "kill -9 of the leader 100 times under load"), `--settle-s 1` and `--poll-us 1000` (the
    values the repository already uses for these flags). `--behind 100000` has no source. It is a pick, so
    rerun catchup with a cited value before quoting it.
- r326, `GROUP=inproc unit2.sh`, needs cpus=16,mem=48g. It runs the SQL suite on amphora, turso and sqlite (in
  process), `--items 200000 --clients 1,32,128 --seconds 1 --warmup 0.5`, for 5 rounds in alternating order.
  It writes `bench/records/2026-11-23-adacachy2-suite-inproc.{jsonl,md,rounds.txt}`, where the .md table
  gives each op's median ops/s with its range, p50, p99, p99.9, errors, and the best target's ops/s over this
  one. Then it commits.
- Not yet queued: `GROUP=servers unit2.sh` (postgres, mysql, clickhouse, orioledb, cedardb). It records
  container and image IDs before the run and afterwards removes, by ID, only the `amphora-bench-*`
  containers and the images of those five engines that appeared during it.

`commit.sh` adds only the paths it is given, merges origin/main, and pushes HEAD:main, retrying up to five
times. The clone's detached HEAD and its user.name/user.email are set for that.

## For the successor

- Read r325 and r326 with kiln's `output`. If either failed, rerun the same script.
- Unit 3 (an Amphora server target in the single-node suite): bench/scale.sh's header says Amphora's server runs
  natively only ("in containers (docker; not Amphora's server)"). run.sh's rule is containers for all or for
  none. So either Amphora's server gets a container image (bench/backup_restore.sh already builds Amphora inside
  rust:1-bookworm), or all five servers run natively. The first keeps the existing server records comparable.
- Units 4 to 6 have not been started.
