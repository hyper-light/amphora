# Bench lane S on adacachy2: competitor families outside the reserved Mac (2026-11-23, hour 1)

Ada's goal is Amphora about 10x faster than every competitor across query types. The proof is benchmarks
run with the same client on the same platform. This lane covers every competitor except SQLite and DuckDB
(the reserved Mac Studio runs those). The families are replication, single-node SQL, scale-out, backup and
restore, and Vitess.

## Verdict for hour 1

There are no numbers yet. Hour 1 went to setup on adacachy2: a clone, the competitors' binaries, and a release
build of amphora-bench with the shipping profile. That build had not finished when the hour ended. A
successor starts from the state below.

## Host (adacachy2, runner 00aad8daff2fa3c9daa65d53c425d450)

- 32 cores, 122 GiB of memory, `~/.cache` on btrfs with 322 GB free. Docker 29.9.3.
- The load is heavy because the perf swarm's builds share the machine. The 1-minute load average was 113.65 at
  02:49Z, 86.9 to 111.4 between 03:04Z and 03:15Z, and 151.4 at 03:33Z. Every rung must alternate the
  engines, and every record carries `load1`.

## Shipping profile vs the bench's profile

- The root `Cargo.toml` `[profile.release]` sets `lto = "thin"`, `codegen-units = 1`, `overflow-checks = true`
  and `debug = "line-tables-only"`.
- `bench/Cargo.toml` `[profile.release]` sets only `debug = "line-tables-only"`, which is weaker.
- So every lane-S build adds `CARGO_PROFILE_RELEASE_LTO=thin`, `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1` and
  `CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true`. Overflow checks stay on, per Ada's decision of 02:05Z.
- `CARGO_HOME=~/.cache/cargo-home-perf-bench-s`, with registry, git and bin symlinked to `~/.cargo`.
  `CARGO_TARGET_DIR=~/.cache/amphora-perf-target/bench-s`, `CARGO_BUILD_JOBS=8`, `nice -n 10`.
- The build started at about 03:02Z (`CARGO_NET_OFFLINE=true` first, falling back to online). It was still
  compiling the bundled `libduckdb-sys` C++ at 03:33Z, with the machine's load at 151. Its kiln run stops at
  its 40-minute timeout. Crates that finished stay in the target dir, so the next build picks up from there.

## State left on adacachy2

- `~/.cache/amphora-perf/bench-s` holds a clone of `git@github.com:hyper-light/amphora.git` at e4fd7876. kiln's
  sync from adacachy timed out twice ("did not answer the sync in 60s"), so the clone was made on adacachy2
  directly. Run commands there with `sync: false`.
- `~/.cache/amphora-repl-bin` comes from `bench/servers/repl/setup.sh`: rqlite v10.7.4,
  libsql-server-v0.24.32, litefs v0.5.14, consul 2.1.0, go1.27.4, and libdqlite v1.18.7 built there. The
  dqlite shim's Go build was still running at 03:33Z, so check `versions.txt` and `dqlite.version`.
- The only competitor image already present is `postgres:16-alpine`. Lane S has started no containers and
  pulled no images.

## Fairness, per bench/baselines/run.sh's header

- **Replication.** Use `bench/repl_ladder.sh`, not `bench/repl.sh`. `repl.sh` runs Amphora's three members in
  the bench's own process and the competitors as processes, which mixes the two platforms.
  `repl_ladder.sh` runs `amphora-procs-*` (three member processes on 127.0.0.1) beside rqlite, dqlite,
  libSQL and LiteFS as processes, natively, with one client. It reports the quorum systems (rqlite,
  dqlite, amphora-procs-persisted) apart from the primary-only ones (libSQL, LiteFS). The failover and
  catchup measures come from `bench/repl_members.sh` (amphora-procs) and `repl.sh` with
  `SYSTEMS="rqlite dqlite litefs libsql"`. Both run as processes.
- **Single-node SQL.** `run.sh` runs amphora, sqlite and turso in the driver's process. It runs postgres,
  mysql, clickhouse, orioledb and cedardb as local servers in containers. Those are two comparison groups
  and two record files. Amphora carries no server, so there is no fair head-to-head between Amphora and a
  containerised server under this rule. The server group's file stands as their own numbers.

## Planned commands (the successor runs these)

Admission (coordinator, 03:40Z): every timed run goes through kiln `run` with `on` 00aad8daff2fa3c9daa65d53c425d450 and
needs "linux/amd64,cpus=16,mem=48g", left queued until that much is idle. Builds take needs "cpus=8,mem=24g" with
CARGO_BUILD_JOBS=8. Record the load average beside every run. Hour 1 took no timed numbers, so nothing needs relabelling.

Workload inputs and where each comes from:

- `--batch 1`: one event a write, the commit path at its most demanding.
- `--text-bytes 600`: K0's event size, the bench's own default (`bench/src/main.rs` opts).
- `--relay-records 128`: the ladder's top rung, so a member's relay queue holds every writer's outstanding
  record and nothing drops at the queue.
- `--spanning 2`: a quorum of three.
- `--deadline-s 10`: a timeout that bounds only failures.

The rungs are 1, 32 (the core count) and 128 (beyond it). Each round reverses the system order so that
neither side always goes first.

    cd ~/.cache/amphora-perf/bench-s
    export CARGO_HOME=$HOME/.cache/cargo-home-perf-bench-s CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/bench-s \
      CARGO_BUILD_JOBS=8 CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true
    R=bench/records/2026-11-23-adacachy2-repl; mkdir -p $R
    A="amphora-procs-persisted amphora-procs-replicated rqlite dqlite libsql litefs"
    B="litefs libsql dqlite rqlite amphora-procs-replicated amphora-procs-persisted"
    for i in 1 2 3 4 5; do s=$A; [ $((i % 2)) = 0 ] && s=$B
      echo "round $i $(date -u +%FT%TZ) $(cat /proc/loadavg)" >>$R/rounds.txt
      SYSTEMS="$s" LADDER="1 32 128" nice -n 10 bash bench/repl_ladder.sh $R --batch 1 --text-bytes 600 \
        --deadline-s 10 --warmup-s 1 --seconds 5 --relay-records 128 --spanning 2 >$R/round$i.tsv 2>>$R/round$i.err
    done

For the in-process SQL group, run `bench/suite.sh` with the targets `amphora,turso,sqlite`, rotated each
round. Use `--items 200000 --clients 1,32,128 --seconds 1 --warmup 0.5` with
`SUITE_OUT=bench/records/2026-11-23-adacachy2-suite-inproc.jsonl`. One pass runs about 60 statements × 3
client counts × 1.5 s, roughly 5 minutes for each target. Run the server group the same way into
`...-suite-servers.jsonl`. Remove each image afterwards by ID.

## Not reached in hour 1

- Families 3 to 5 have not been started.
  - `bench/scale.sh` needs `bench/servers/scale/fetch.sh` and `--hosts`.
  - `bench/backup_restore.sh` builds its own image and builds Amphora inside rust:1-bookworm.
  - `bench/vitess` holds `cluster/` and `schema/` only, with no driver script at its top level.
- No harness change was needed so far.
