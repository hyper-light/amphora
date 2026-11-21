# Handoff: amphora-log-bound (2026-11-11), goal 7582f00a's bound

## Branch
- `t1/log-bound` in ~/Projects/amphora-t1-log-bound (local; not pushed, not merged).
- Head `3f7e76a0028ae8fc7e907ba1acdbdbf4223fca27`, tree `a6be4d944e1dc1bbbb62c2e8fc53d8e3fc3e0e46`. Six commits on
  b151af0 (the batch head: 490ea83 re-signed, plus t1/planner-stats):
  - `4a274ec`: the change, with its red run (the rule put back to 2 x 131,072 frames: 7 tests fail).
  - `854d552`: the red test for the seam owner's point 1 (a restart's hold never ending under steady load).
  - `004f682`: the fix, and the leader's limit and the Windows same-serial limit written.
  - `d3b9d35`: the red test for the seam owner's 004f682 point (the restart's queue read on another basis than
    each step's).
  - `90840aa`: the fix: one reading at each step's start, which a restart takes.
  - `3f7e76a`: the gauge renamed to Kiln Metrics' agreed `amphora_replica_database_log_limit_bytes` (a rename only).

## What it does
The replica's 2 x LOG_FRAMES bound (~1.08 GB) and the 64th restart threshold are gone. In their place are two
rules, both from runtime measurements (replication.md, "A replica's database log bounded by what it measures"):
- **Restart spacing, counted.** Entries applied, entries held by a restart (from its start to the owner's first
  idle step), and entries over AC-T1.3.4's 15 ms in a long owner step. A restart is due while (held + over + worst
  restart) / applied < 1 - 0.999, over patience's two generations. A restart then waits for a pass that copied
  every durable frame.
- **Room, from the filesystem.** H = (file bytes + (free - one snapshot image) / N) / (frame + page). The free
  bytes come from fs4 on the flushing thread after each pass; N is the live replica logs on that filesystem.
  Past H: a restart is due whatever the share, reads are held, and new submits are refused `Unavailable`. A
  follower applies on and fences typed if its disk fills.
- LOG_FRAMES is removed from replication.rs. The store's own default is untouched.
- New gauge: `amphora_replica_database_log_limit_bytes`.

## Seam owner (amphora-seam-owner-2)
- amphora-owner-t1-log-bound-design-20261111: direction agreed, changes asked.
- amphora-owner-t1-log-bound-rev1-20261111: revision agreed, code may proceed (four additions, all in the doc).
- amphora-owner-t1-log-bound-4a274ec-20261111: both departures agreed (the room in bytes; Windows by volume
  serial, provided Windows clippy and tests pass natively on jupiter in the batch). Two changes asked, made in
  854d552/004f682. Follow-up sent under amphora-log-bound-followup-004f682-20261111. Then amphora-owner-t1-log-bound-004f682-20261111 asked
  one change (made in d3b9d35/90840aa), and **amphora-owner-t1-log-bound-90840aa-20261111 agreed b151af0..90840aa: the
  head goes to the batch.** Its final comes on the batch's merged tree once the seven gates pass (native Windows
  clippy and tests included).

## Kiln Metrics
- `amphora_replica_database_log_limit_bytes` was asked under amphora-log-bound-names-v1-20261111
  (amphora-log-bound/names/v1). **Agreed 2026-11-12, renamed to `amphora_replica_database_log_limit_bytes`; dashboard row "Database log against its limit", unit B.** The name is in the code and the doc. If Kiln Metrics renames
  it, change meter.rs and replication.md together; the doc-truth test checks them against each other.

## Tested (adas-mac-studio.local-2, macOS aarch64, 2026-11-11, load 3.4-11; at 4a274ec, then again at 004f682)
- Red: this tree with flush()'s rule put back to 2 x 131,072 frames. All 7 owner rule tests fail.
- lib `database::`, `replication::`, `meter::`, `db::`: 157 passed, 2 ignored at the head. The power-cut nemesis tests in
  both classes are among them; the 12-seed ones pass.
- Integration tests replication, kiln_seam, application, transport, store and files pass.
- The 8 owner tests (log room, reads, deadline, busy pass, first restart, past room, follower fence, dropped
  replica, steady queue) and the never-idle core test repeated 10 times: none failed.
- `cargo clippy -p amphora --all-targets --locked -D warnings`, `scripts/check-production.sh` and `cargo fmt
  --check` pass.

## Left for the batch
1. The seven gates on the merged tree. Linux clippy compiles fs4's rustix path, and Windows clippy compiles the
   `cfg(windows)` `Machine::filesystem` (winapi-util `Handle::from_path_any` + `file::information`) for the first
   time. Neither has been compiled. The seam owner's agreement to the Windows path is on condition that Windows
   clippy and the Windows tests pass natively on jupiter.
2. AC-T1.4.3's 1 ms is held by the bench, not by the rule (seam owner's point 2): commit p99.9 with restarts on
   against off, at a load where restarts are due by their share. The run that would show it: Amphora's write bench
   at K0's load on a quiet Studio, this tree against the same tree with restarts never due. A "never due" arm
   needs a patch: `due` false unless past the room.
3. Reopen time at a large log (seam owner's falsifier): a reopen reads the whole log to rebuild its index (wal.c
   `walIndexRecover`), and readers' wal-index lookups grow with it. The room bounds disk, not that time. Measure a
   replica's reopen (`Machine::open` on its files) with its log near a real room, in GB, on a quiet Studio, to see
   whether a time bound is needed beside the disk's.
4. Optional, heavy: the persisted and replicated 10,000-seed power-cut runs.
   `AMPHORA_POWER_SEEDS=0:10000 cargo test -p amphora --lib -- --ignored
   a_member_power_cut_loses_no_acknowledged_replicated_write_over_10000_seeds`. The simulated disk stands in as a
   filesystem with all of it free, so these runs don't exercise the room.

## Findings for the Amphora agent
- ENOSPC mid-install is untested. A snapshot install's writes have no fault seam, so nothing shows that an install
  meeting a full disk fails typed rather than leaving a torn image. One image is kept back per filesystem, so
  several groups installing at once can still meet a full disk. What's needed: a fault seam on the install's
  writes, then the test (seam owner, rev1 point 3).
- Picked values, noted to amphora-constants:
  - The store's `Config::log_frames` (131,072), `Db::keep_log`'s default and the store's 64th.
  - `SNAPSHOT_BYTES` (256 MiB).
  - `CHECKPOINT_EVERY` (25 ms) stays, cited to store.md decision 4's measurement. It is the store's measured
    period, reused for the pass cadence while no restart is due.
- The process disk budget (amphora-owner-disk-budget-goal-20261109) has its hook here: `log_bound::log_room`'s
  `available`.

## Goal
- 7582f00a's bound: code, design and tests done. The seam owner's code agreement and Kiln Metrics' name
  agreement are pending.

## 2026-11-12: jupiter's native failure, fixed at 7f5447b (tree 4e09bc18), on 930ac68
- jupiter failed `a_restart_due_waits_for_a_reads_deadline_and_no_longer` with "the log held 0 frames, room 17".
  The cause was the test's own: its open read's deadline clock was the wall clock until the test passed it, 60 s
  (HANG_READ) from the read's start. Windows' `park_timeout` granularity (~15.6 ms a 1 ms POLL) and a queue of 61
  stretched the 39 settled writes past 60 s, so the read refused itself. The owner, past its room with no read
  out, then restarted the log, leaving 0 frames. The production rule did what it should.
- Fix: the read's clock stands at its start until the test passes its deadline. An assertion that the read is
  still out after the writes names the cause if it recurs.
- Red off Windows: a scratch copy with the read's window at 2 s fails the same assertion with the wall clock
  ("held 24 frames, room 28") and passes with the standing clock. Green on local-2: the 6 tests 10x, clippy,
  check-production and fmt.
- What the native Windows run must check: `cargo test -p amphora --lib --locked --
  a_restart_due_waits_for_a_reads_deadline_and_no_longer` passes. If it fails, read which assertion: "the read
  ended before its deadline was passed" means something else ended the read; "the log held N frames" with the read
  still out would be a real finding.
- Not mine: `database::import::tests::u06_import_reopens_preserving_baselines_tombstones_and_family_heads` ("the
  deadline elapsed") on jupiter.

## 2026-11-12: W1 on ace2d76 is not this branch's; keep_log changed on its own evidence (86c78f8, tree a043b227)
- I first attributed W1's "Database snapshot is stale" to this branch's file cuts, on three arms of 6 runs each,
  and that was wrong. With 10 runs each on adacachy2: ace2d76 with keep_log = room fails 2 of 10; the same with no
  share-based restart of mine fails 2 of 10; t1/g-slot c6ad588 alone, without this branch, fails 3 of 10. All
  fail with the same error. It is G's, or older than both branches; reported to g-slot and the Amphora agent.
- 86c78f8 (on 7f5447b): keep_log = the room instead of min(frames, room), so a file is cut only past what the disk
  allows (store.md decision 31's intent; a cut is the dearer restart by its measurement). The follower fence test
  had leaned on the cut; it now writes until the learner is past its room. This departs from the seam owner's
  design ruling, so its word is asked. Green on local-2: lib 157 passed, 2 ignored; integration; 8 tests 10x;
  clippy, check-production and fmt. On adacachy2: the 20 log-bound and room tests at ace2d76 with the change.
- Seam owner agreed 86c78f8 (amphora-owner-t1-log-bound-86c78f8-20261112). Its asked citation, decision 31's run,
  is in 2f337a0. **Head for the batch: see the last line.**
- **Finding for whoever root-causes W1 (g-slot):** a replica's apply that meets `BusySnapshot` on its read-to-write
  upgrade (wal.rs `begin_write_tx`: the coordination snapshot moved since the connection's read began) isn't
  retried. The machine fails and the owner fences. "Database snapshot is stale" on c6ad588 alone shows the apply
  path meets it with no cut. Candidates for the other side: the flushing thread's passive pass on its copier
  connection, run beside the owner's apply, or a write on another connection. A cut under disk pressure may be one
  trigger too, but that is not shown. Whether the apply should retry a BusySnapshot taken before it wrote anything
  (a bounded retry: nothing was written), or the engine should not move the snapshot, is that root cause's
  decision.

- Head: `2f337a09964e3fae0ac0ae5c41f482cbcb857761`, tree `5ec5e04ba5bd90e35f4c7ad83216d396ce70d1c5` (on 930ac68: 7f5447b, 86c78f8, 2f337a0).
