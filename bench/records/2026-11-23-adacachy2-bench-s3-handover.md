# Bench lane S3 on adacachy2: hand-over (2026-11-23, 04:43Z to 05:35Z)

Ada's goal is Amphora about 10x faster than every competitor, proven with the same client on the same platform.
Lane S3 carries lane S2's plan (bench/records/2026-11-23-adacachy2-bench-s2-handover.md). This hour produced no
timings either. Every timed run is still waiting on one build step, described first below. It also found two
fairness problems in the replication measures that any quoted failover or catch-up number must account for.

## Losses first

1. **No numbers yet: the build is stuck on disk metadata.** r298 (pid 3139877) and r325's own build (pid 201788)
   both wait on `release/.cargo-build-lock` in `~/.cache/amphora-perf-target/bench-s`. The holder is pid 3247380,
   a `cargo build --release --manifest-path bench/Cargo.toml` started about 03:04Z with the shipping profile's
   env. Its libduckdb-sys build script has run `ar cqD .../out/libduckdb.a` since about 03:54Z, in state D.
   The archive grew from 1,687,232,512 bytes at 04:50Z to 1,764,827,136 bytes at 04:56Z, about 13 MB/min.
   An earlier 2,238,700,148-byte temp file `stthhjBT` sits beside it. If the finished archive is about that
   size (a guess), `ar` has about 35 minutes left. After that come the link with thin LTO and one codegen
   unit, then the ladder.
   The holder uses the same profile env as r298 and units 1 and 2, so its output serves them and nothing
   rebuilds afterwards.
2. **Load average says nothing about CPU on adacachy2 now.** Load ran from 117 to 272 during this hour, but
   CPU pressure was about 0.9% (`/proc/pressure/cpu` some avg60). 283 threads were in D state, and I/O
   pressure was some 5 to 9%, full 4 to 8%. The machine's limit is disk I/O, mostly btrfs metadata, which has
   no unallocated space. S2's scripts log only `/proc/loadavg`. Every replication commit is an fsync, so
   when other lanes do I/O, those numbers get noisier for every system alike. Reversing the system order
   each round removes the bias but not the spread. S3's catch-up script logs CPU and I/O pressure beside
   every run (`load.txt`). Read each round's spread before quoting a ratio.
3. **Amphora's members are never killed in `amphora-procs`.** `procs::Procs::kill` and `start`
   (bench/src/repl.rs) call `set_cut`: the member process keeps running and only its links are cut. The
   competitors' `kill N` is kill -9 of the node's process group, and `start N` starts a new process.
   - Failover: the survivors can't tell a cut from kill -9, so the time to a new acknowledged write is
     comparable. But `lost = 0` for Amphora never tests the crash: the cut member keeps its memory, its
     open log and its unflushed state.
   - Catch-up: from `c.start(member)` on, Amphora pays only for reconnecting. Each competitor also pays its
     process start and recovery. **Amphora's catch-up time is favoured by its missing restart.** Don't
     compare it with the others until members are really killed.
   - Fix (bench/ only, not started): have each member report its pid on `leader`. `kill(n)` sends SIGKILL to
     that pid, and `start(n)` spawns `amphora-bench --repl-member` again on the same `--dir`, with the same
     args repl_members.sh used. That works for members on this host. For remote ones, return a typed
     refusal. `member_main` also runs `remove_dir_all(&m.dir)` at every start, so a restart would wipe the
     member's data. It needs a flag that reopens the member on its own data instead. repl_members.sh
     `wait`s on the original pids and counts any non-zero exit as a failure, so a member that was killed
     on purpose must not count. Don't edit bench/src in `~/.cache/amphora-perf/bench-s` on adacachy2 while the
     timed runs build from it. Use a separate clone, and build after they finish.
   - Related: each member's relay thread loops on `handle.drain` and calls `std::thread::yield_now()` when
     nothing moved, so every member keeps one core busy for its whole life. That is a polling loop, which
     CLAUDE.md's event-driven rule forbids. It costs Amphora three cores the competitors don't pay. It
     matters wherever cores are the limit, and it isn't in `client_use`.
4. **Picked inputs still in unit 1.** `--behind 100000` (no source), `--poll-us 1000` and `--settle-s 1`. Unit 1
   will still run catch-up with `--behind 100000` into `...-repl-failover/`. Don't quote any `*-catchup.jsonl`
   from that folder. The `--seconds 5` ladder window and the suite's `--seconds 1 --warmup 0.5` limit the
   tails. A p99.9 rests on at least 1,000 samples (`latency_ms.n` in each record). S2's `table.py` doesn't
   print n, so check it before quoting a p99.9.

## Done this hour

- **Catch-up with a derived backlog, queued as r360** (`~/.cache/amphora-perf/bench-s3/catchup.sh` and
  `derive.py`, sha256 e9494ee5... and 3f55816c...). It takes the same `timed.lock`, so it runs after units 1
  and 2. Records go to `bench/records/2026-11-23-adacachy2-repl-catchup/`, committed after each stage.
  1. Member restart time: catch-up at `--behind 0`, so the time runs from the member's start to its first full
     answer. 5 rounds, every system, order reversed each round.
  2. `derived.json`: behind = ceil(median commits/s at W writers, from unit 1's ladder records, x median
     restart time), for each system and W in 1, 32 and 128. These are the writes a member misses while it
     restarts under that load.
  3. Catch-up at each system's derived backlogs, 5 rounds.
  4. A ladder by decade from 1 to the first decade at or past the largest derived backlog, for every
     system, 5 rounds, so systems compare at equal backlogs.

  Inputs: `--batch 1 --text-bytes 600 --relay-records 128 --spanning 2` (unit 1's) and `--deadline-s 10`
  (unit 1's, a failure bound, not a measured quantity). `--poll-us 0` makes the poll closed-loop, one read
  outstanding, and drops unit 1's picked 1 ms interval. With loss 3 unfixed, Amphora's restart time is
  near 0, which makes its derived backlog small. Its rows measure reconnection only.
- **Server group queued as r361**: `GROUP=servers bash ~/.cache/amphora-perf/bench-s2/unit2.sh` (postgres, mysql,
  clickhouse, orioledb, cedardb), needs linux/amd64,cpus=16,mem=48g, behind the same lock.
- **Scale-out binary fetch tried and failed** (`bench/servers/scale/fetch.sh` into `~/.cache/amphora-scale-bin`,
  run r364). The versions were the newest stable tags from `git ls-remote --tags`: cockroach v26.2.12 and tidb
  v26.3.24. GitHub's "latest release" API gave tidb v7.5.8 (an LTS patch) and nothing for cockroach, so it
  was not used. The cockroach download hit its 900 s timeout after 60 MB. The TiUP mirror answers 404 for
  `tidb-v26.3.24-linux-amd64.tar.gz`, so tidb's git tags don't match the mirror's versions. Take tidb's
  version from the TiUP mirror's own manifest. The partial folders were removed. Retry the cockroach
  download outside the timed runs, because it competes with them for the same disk.

## Not done, and why

- **Amphora single-node server entry (unit 4): blocked by the product, not the bench.** `amphora serve`'s client
  protocol (crates/amphora/src/serve.rs) takes `exec SQL`, which runs `execute_batch` with no parameters and
  returns no rows, and `count SQL`, which returns one integer. Most of the suite's ops return rows, and every
  op is parameterized. So a server entry could run only the write ops and the count-shaped reads, with SQL
  literals inlined. The other entries bind parameters, so that is not the same client. The container image
  is the easy part (backup_restore.sh's pattern: build in rust:1-bookworm, copy into the image). The real
  work is a row-returning, parameterized request in serve.rs, or a wire protocol the other servers' drivers
  speak (pgwire). That is a crates/amphora change, outside this lane's bench-only scope. A decision for
  Ada is listed below.
- **Scale-out runs (unit 5)**: `bench/scale.sh --hosts` needs two or more Linux machines, each reachable as a
  Docker or ssh endpoint. An earlier note says nothing reaches adacachy2 by ssh (TCP 22 timed out); this lane didn't retest it. They also need Amphora's
  server built natively (`AMPHORA_BENCH_SCALE_AMPHORA`). Not started.
- **Vitess**: a driver already exists (bench/src/vitess.rs, target `vitess` in the convo bench, with
  bench/vitess/cluster). It isn't in `--suite`, whose schema (items, owners, regions) would need a vschema
  and sharding keys. Not started.
- **backup_restore.sh**: not run. It builds its own image and would contend for the disk the timed runs
  use. Run it after them, under the same lock.

## Decision for Ada

- An Amphora server entry beside postgres, mysql, clickhouse, orioledb and cedardb needs Amphora's server to
  take parameterized queries that return rows. One option is a new client request in `amphora serve`
  (appended, so the protocol only grows). Another is a pgwire listener, which would let the postgres driver
  measure Amphora on the same client. Until one of them exists, the server group's only Amphora comparison
  is the in-process group, which skips the network hop the servers pay.
