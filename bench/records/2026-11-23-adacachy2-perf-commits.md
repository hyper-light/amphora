# Lane V (commits) on adacachy2: why replicated commits don't grow with writers (2026-11-23, 13:09Z to 14:04Z)

Ada's goal is Amphora about 10x faster than every competitor, proven with the same client on the same machine.
Lane S4 measured the opposite for replicated commits (`2026-11-23-adacachy2-repl/TABLE.md`): Amphora 14 to 19
commits/s flat from 1 to 128 writers, dqlite 145 to 188, rqlite 25 to 1390. This lane's hour named the limiter
with measurements. The fix is designed below but not built.

## Losses first

1. **No fix landed.** The limiter is measured and the fix is designed (below), but it changes the replicated
   owner's core state machine (`crates/amphora/src/database/replicated.rs`, `Pending`, `prepare`, the answer on
   apply). Building, clippy and the replicated suite could not finish inside the hour on adacachy2: kiln refused
   this lane's runs eight times for room (0 to 5 of 32 cores idle).
2. **The numbers come from a busy machine.** Load averages ran from 49 to 739 (1 min), CPU pressure "some" from 6 to
   38%, and I/O pressure "some" from 0.4 to 3.7%. Windows were 5 s with 1 or 2 rounds a phase. They size the gap, but
   they are not a ladder. Disk-backed rates swing with load (16.2 to 39.6 commits/s at 1 writer). The RAM-disk
   rates don't (382.4 twice).
3. **The 2.4 ms that is not flushing is not split by step.** The bench has no per-step instrumentation, so this
   hour gives thread CPU and wait states, not a timeline of barrier, proposal, replication, apply and answer.
4. **The replicated class waits on disks it says it doesn't need.** The leader's log flush and one follower's are
   on every commit ("Whose flush" below). Which rule causes that is not found yet.

## What limits a replicated commit (unit 1)

The binary is S4's measured build, `~/.cache/amphora-perf-target/bench-s/release/amphora-bench` (7ac4153d,
the shipping profile). Members run on 127.0.0.1:29490-29492 with family a's flags (`--batch 1 --text-bytes 600
--deadline-s 10 --relay-records 128 --spanning 2`, class replicated), `--warmup-s 1`, and 5 s or 8 s windows.
`bench/thread_sample.py` (this lane) starts the members itself, so it may read their threads under Yama, and
samples each thread's state, syscall and wchan (3,900 to 8,900 sweeps a window) plus each thread's CPU.
"spread" is the ladder's routing (writer w at member w % 3). "leader" sends every writer to the leader.

**r635** (13:17Z, load 50 to 70), members on btrfs (`~/.cache`):

| writers | routing | commits/s | p50 | p90 |
|---|---|---|---|---|
| 1 | spread (a follower's write) | 26.5 | 37.9 ms | 44.0 ms |
| 1 | leader | 38.3 | 28.9 ms | 37.4 ms |
| 32 | spread | 36.3 | 389 ms | 1,933 ms |
| 32 | leader | 44.4 | 713 ms | 786 ms |

**r674** (13:22Z to 13:27Z, load 89 to 739), leader routing, members' folders on btrfs or on tmpfs (`/dev/shm`,
where a flush costs nothing), in the order disk, shm, disk, shm, then reversed:

| writers | btrfs commits/s (p50) | tmpfs commits/s (p50) |
|---|---|---|
| 1 | 16.2 (65.5 ms), 39.6 (26.6 ms) | 382.4 (2.4 ms), 382.4 (2.4 ms) |
| 32 | 47.6 (664 ms), 45.0 (705 ms) | 407.8 (78.8 ms), 354.6 (87.0 ms) |

What the numbers say:

- **One write at a time (suspect 1): confirmed by measurement.** With flushes free, 1 writer gets 382 commits/s
  and 32 writers get 355 to 408. p50 at 32 writers (79 to 87 ms) is 32 times the 1-writer p50 (2.4 ms): each write
  waits for the 31 ahead of it. The code says the same. `pending: Option<Pending>` holds one write. `prepare`
  runs its closure in a speculative transaction, captures the pages, rolls them back and proposes them. The
  next write prepares only once that entry has applied. `limits.proposals = 1`.
- **A flush per commit (suspect 4): confirmed, on the leader and one follower.** At 1 writer, 2.4 ms of a commit
  is everything but flushing.
  On btrfs a commit takes 27 to 66 ms, so 91 to 96% of it waits on a flush. Each follower's `hyper-log-io` thread
  sits in `fdatasync` on its log (fd 16) in 77 to 85% of samples (`wait_log_commit`, `folio_wait_bit_common`,
  `wait_dev_flush`). The leader's busiest `amphora.replica` thread waits on a futex in 78 to 92% of them. No group
  commit spans writers, because only one write is ever in flight. (Thread names are cut at 15 characters, so
  the owner and the flusher both read `amphora.replica`. The busiest one also shows `fsync` on fd 6.)
- **The bench's client and relay (suspect 5): ruled out.** The same driver, members and relay give 408
  commits/s on tmpfs. At 32 writers the leader holds 35 threads parked in futex waits inside `Database::call`:
  the writers queue in Amphora, not in the client. The relay threads spin at 0.90 to 0.96 of a core each
  (S3's note). That costs cores but doesn't serialize.
- **Send window (suspect 2): not the limiter now.** With one entry in flight, no window binds. It would bind once
  several entries are in flight. Not measured separately.
- **Flusher's timed gather (suspect 3): not the limiter.** The leader's flushing thread (`amphora.replica`) is in
  `fsync` on fd 6 in 0.8 to 8% of samples. Decision 14's wait is bounded by half a flush.
- **Bytes a commit:** a follower writes 105 KB through `write` and 330 to 420 KB to the device per 600-byte
  commit (`/proc/<pid>/io`, `wchar` and `write_bytes` over commits). The pages ship through the raft log, the
  database log and the checkpoint. That makes each flush slower on btrfs. It's a second lever, not the limiter.

**Per commit at 32 writers on btrfs** (r674, 45.0 to 47.6 commits/s, so 21 to 22 ms of service a commit):

| part | time | how known |
|---|---|---|
| queueing behind the one write in flight | p50 664 to 705 ms | p50 minus service; matches 31 x 21.5 ms |
| waiting on the leader's and one follower's log flush | about 19 ms (88%) | service minus the tmpfs per-write time; "Whose flush" |
| barrier, prepare, propose, replication round trip, apply, answer | about 2.4 ms (12%) | tmpfs at 1 writer, p50 |
| of which the leader's busiest `amphora.replica` thread's CPU | 1.27 ms | 0.487 cores at 382 commits/s |
| of which each follower's busiest `amphora.replica` thread's CPU | 0.92 ms | 0.35 cores at 382 commits/s |

At this machine's flush time, the ceiling for one write at a time is the flush rate (16 to 48 commits/s). With
flushes free, the next bound is likely the leader's busiest `amphora.replica` thread at 1.27 ms of CPU a commit,
about 790 commits/s (inferred, not measured).

## Whose flush (r722, r742)

One writer at the leader, each member's folder on btrfs or tmpfs. Node 3 led every valid phase: the core's
election seed is the node ID (`seed: id`), so a placement by node is a placement by role. r742's first phase had
node 2 leading and no record (most likely r722's stopped members still held a port), so it is left out.

| run | leader (node 3) | followers | commits/s | p50 | which log I/O threads sit in `fdatasync` |
|---|---|---|---|---|---|
| r674 (2 rounds) | btrfs | btrfs, btrfs | 16.2, 39.6 | 65.5, 26.6 ms | both followers', 77 to 85% |
| r742 splits 3, 5 (load 97 to 125) | tmpfs | btrfs, btrfs | 58.8, 40.0 | 12.8, 26.1 ms | both followers', 67 to 75% |
| r742 splits 2, 4, 6 (load 103 to 161) | btrfs | tmpfs, tmpfs | 107.8, 97.8, 89.6 | 9.0, 10.0, 10.0 ms | the leader's, 59 to 67% |
| r722 split 1 (load 837) | btrfs | tmpfs, btrfs | 82.8 | 11.8 ms | the leader's 49%, the btrfs follower's 73% |
| r722 split 2 (load 964) | tmpfs | btrfs, tmpfs | 282.6 | 3.5 ms | the btrfs follower's |
| r674 (2 rounds) | tmpfs | tmpfs, tmpfs | 382.4, 382.4 | 2.4, 2.4 ms | none |

**A commit is fast only when the leader's log and at least one follower's log flush for free.** With the leader
on tmpfs and both followers on btrfs it runs at 40 to 59 commits/s. With either follower's flush free as well, it
runs at 283. With the leader on btrfs, 90 to 108 even when both followers are free. So today's replicated class
waits for the leader's log flush and one follower's log flush on every commit, the same quorum of disks as the
persisted class. The followers' held answers (hyper-raft R-8, "promise memory alone: they leave at once, before
this Ready's write") don't shorten the path in this setup. The class's statement in replicated.rs ("a commit on
entries held in memory across fates") and the bench's ("replicated: once members spanning --spanning fates hold
it") say it shouldn't wait for either flush.

What isn't known yet is which rule makes it wait. Candidates to check, by reading and with a test that counts
flushes before an answer:
- `held_quorum_index` counts the leader by its disk (`own = self.log.persisted()`), so a held quorum without
  the leader's flush needs both followers' held answers.
- A follower's held answer reaches only as far as `held_cover()` (its durable promise and those in flight) and
  `answers_held()`. If either stops it at what the follower's disk holds, held answers trail the flush.
- The leader's answer waits for hyper-durable's durable application (`behind_fence`, `fence_covered`).

Held commits do happen in a driven group: the test at replicated.rs about 9380 to 9478 asserts `commits_held`
rises once each voter has stated a durable promise. So the mechanism exists, and the question is why it doesn't
shorten the real members' path. The cheapest next measurement: a `counters` op on the bench member
(`ReplicaHandle::counters`, `commits_held` against `commits_classic`, `flush_backlog`), read before and after a
window on btrfs and on tmpfs. It needs a bench rebuild, which didn't fit this hour.

The fix for this is a correctness question about R-8 in vendored hyper-raft, so it is the hyper-raft owner's call
before anyone changes it. It is listed under the decisions below.

## The fix (unit 2), designed for the successor

The data shape: a **batch** in place of one `Pending`. The leader's owner already holds the waiting writes
(`waiting: VecDeque<Envelope>`, at most `Control::command_bound`).

1. **Several writes per entry.** When the pending write passes its barrier and prepares (`Actor::prepare`,
   replicated.rs about 5131 to 5345), take the waiting `Input::Submit`s of the same namespace whose operation is
   `Operation::Call`, whose deadline has not passed, and whose claim passes (`claim_refused` at the same base).
   Run each one's closure inside the same `prepare_db` transaction, each under its own savepoint
   (`SAVEPOINT w<n>`, `RELEASE` on success, `ROLLBACK TO` + `RELEASE` on its own error). A failing write then
   refuses only itself, as today. Capture the pages once and propose one entry. `Pending` keeps each write's
   response, value and reservation. On apply (`answer_committed` about 4760, the apply answer about 4575) each
   is answered with its own value and the entry's token. Durability is unchanged: no answer leaves before its
   entry applies, as today.
2. **How many in a batch: derived, never picked.** Take every eligible waiting write while the entry stays within
   `Actor::entry_room` (the core's message room, `MESSAGE_BYTES`), the bound the entry already obeys. Nothing
   waits to gather. A batch is what queued during the last entry's flush (group commit, as decision 14 and
   PostgreSQL do). There is no timer and no count of its own. One writer stays one write an entry, so its latency
   is unchanged.
3. **Followers' writes** (the write slot, `Phase::Answered`) keep their path in the first step: batch the
   leader's own queue first. A follower may batch its own queue into one forwarded preparation in a second step.
4. **Then entries in flight** (`limits.proposals`, the barrier's "applied its whole log" rule): a second entry
   prepared on the first one's speculative pages. Gated on the base fence and the window (lane K4's
   `Window::Carried`). It's a later step, with its own A/B, after batching shows where the next limit is.

Predicted, not measured: at 32 writers on btrfs, a batch per flush raises the rate from 45 to 48 commits/s
toward 32 x that, bounded by the leader's CPU per write (each one's prepare). The A/B is family a's ladder
(`families.sh` fam_a: 5 rounds, order reversed, 1/32/128 writers, against dqlite and rqlite) with load and
pressure beside each round.

**Tests the change needs** (behaviour, never time): two writes queued behind a barrier commit as one entry and
each gets its own answer; one of them failing refuses only itself and the other applies; a batch spanning a
leader change answers every write unknown or requeues it, as one write does today; and a deadline passing
mid-batch times out only that write.

## Commands

    # on adacachy2, kiln run on 00aad8daff2fa3c9daa65d53c425d450, needs linux/amd64,cpus=8,mem=24g, sync false
    BIN=~/.cache/amphora-perf-target/bench-s/release/amphora-bench
    python3 -I bench/thread_sample.py $BIN 29490 DIR WRITERS spread|leader 8 3 5 replicated   # r635
    python3 -I bench/thread_sample.py $BIN 29490 DIR WRITERS leader 5 2 3 replicated          # r674, DIR on ~/.cache or /dev/shm
    python3 -I bench/thread_sample.py $BIN 29490 "DISK/m1,DISK/m2,SHM/m3" 1 leader 5 2 3 replicated   # r722, r742 (needs cpus=4,mem=4g): a folder a member

## Decisions for Ada

1. **Should the replicated class answer before any disk flush, as its statement says?** Today it waits for the
   leader's log flush and one follower's. At 1 writer on this machine a commit takes 27 to 66 ms that way,
   against 2.4 ms with no flush to wait for (11 to 27 times). Changing it touches R-8 in vendored hyper-raft. If the answer is yes, the
   hyper-raft owner should find the rule that waits and fix it there, with a test that counts flushes before an
   answer. If the answer is no, the class's statement and the bench's column should say it commits on a quorum
   of disks.
2. **Batching several writes into one entry (above) needs no decision.** Durability is unchanged and every write
   keeps its answer. It's the successor's next unit, with family a's ladder as its A/B.

# Lane V2 (commits) on adacachy2: held commits form, and each write's barrier still waits on two log writes (2026-11-23, 13:51Z to 14:46Z)

Ada's goal is Amphora about 10x faster than every competitor on the same client and machine. V found every
replicated commit waiting on the leader's log flush and one follower's. The approved design (replication.md, "Fast
acknowledgement") answers a replicated-class write once the entry is held in memory across fates and the proposer
has applied it. This hour asked why that doesn't show in the bench.

## Losses first

1. **No product fix landed.** The cause is found with counters and code (below). The fix touches vendored
   hyper-raft or the owner's barrier rule. Its test, clippy and the replicated suite could not finish by 14:46Z:
   the bench build alone took 8 min 46 s (r820), and kiln refused two runs for room.
2. **Batching (unit 3) and the ladder A/B (unit 4) were not started.**
3. **The bench's replicated class runs on fates that don't exist.** Three members on one host each declare their
   own fate (`bench/src/repl.rs`, `Fates { of: ids.map(|i| (i, i)) }`, both the in-process group and `--repl-member`).
   They share one host and one power feed. The design says a configuration that can't span the fates asked
   refuses the class, and the runbook says "Each member is its own fate, so put each on its own machine". The fair
   replicated measurement is three machines on a LAN, as AC-T1.8.3 states. The one-host column is a trial of the
   path, not of the class's guarantee. Held commits form here only because of those declared fates.
4. **main's bench did not build.** The root patches `hyper-rt` to `vendor/` since f8f65110; `bench/Cargo.toml`
   didn't, so cargo looked for hyper-rt on crates.io ("no matching package named `awak`"). Fixed in 7f2650a9.

## Do real members commit on held quorums? Yes (unit 1)

r851, binary 7f2650a9 (`bench/` release, V's `counters` op), `python3 -I bench/thread_sample.py BIN 29490 DIR W
leader 5 2 3 replicated`, members' folders on btrfs (`~/.cache`) or tmpfs (`/dev/shm`), one 3 s span a phase,
needs cpus=4,mem=4g. Counters are the leader's over the span (followers count no commits, by design).

| folders | writers | commits/s | held | classic | held share | load (1 min) | CPU some / I/O some (avg10) |
|---|---|---|---|---|---|---|---|
| btrfs | 1 | 26.0 | 34 | 49 | 41% | 72.6 | 19.6% / 4.1% |
| tmpfs | 1 | 272.6 | 403 | 396 | 50% | 51.1 | 8.4% / 2.0% |
| btrfs | 32 | 43.2 | 105 | 24 | 81% | 74.5 | 51.4% / 0.3% |
| tmpfs | 32 | 286.4 | 395 | 152 | 72% | 82.6 | 48.4% / 0.7% |

The work happened: held plus classic over the span (27.7/s at btrfs, 1 writer) matches the driver's rate. Each
member's flushing thread made one database flush a commit (83 flushes for 83 commits; 5.3 ms each on btrfs). That
thread is off the answer's path (design item 4).

So most commits are held, and the rate is still the flush rate. The commit is not what waits.

## What waits: each write's barrier (cause, from the code)

A write at the leader crosses a ReadIndex barrier before it prepares ("One command at a time crosses ReadIndex
before evaluation", replicated.rs head). At 1 writer, write N+1's barrier starts as soon as write N is answered on
its held commit, while every member's log write of entry N is still out. The barrier then waits on two of them:

1. **The leader's own log write.** Amphora states `depth: 1` to `Limits::derive` (replicated.rs, the core's
   settings), so the core's `readies_in_flight` is 1. `RawNode::ready_given` refuses a second `Ready` while one is
   issued (hyper-raft node.rs, `Capacity("readies in flight")`). Reads are asked in a round when a `Ready` is given
   (`Raft::ask_reads`), and their confirmations reach the shell only in a `Ready` (hyper-durable replica.rs,
   `take_read_states` in `take_ready`), never in `light_now`. So the barrier can't start or finish until the
   leader's write of entry N is durable.
2. **One follower's log write.** A follower's heartbeat answer is an ordinary message. A follower's messages leave
   at once only "with no write out and nothing left to write" (node.rs `releases_now`, I2). Its write of entry N is
   out, so the answer the ReadIndex quorum needs leaves after that flush. Only held answers skip this
   (`take_held_messages`).

That is V's "Whose flush" table: a commit is fast only when the leader's log and at least one follower's log flush
for free. The held commit shortens the commit; the next write's barrier puts both flushes back.

## The fix, designed for the successor (unit 2)

The test first (red today): in the driven group, three fates, two to span, every member's log flushes held
(`pump.hold_durable()`). Two writes at the leader one after the other: both are answered while no log write has
completed, and the leader's counters show held commits and zero flushes before either answer. Today the second
hangs at its barrier until the hold is let go.

Two changes, each at its cause:
- **Reads without a `Ready`.** A read's confirmation depends on no write. Give read states through `light_now` (core)
  and `take_light` (shell), released as today once applied, and ask the round there too. Vendored hyper-raft, as a
  recorded change (`scripts/vendor.py record hyper-raft`, then `check hyper-raft`).
- **A follower's heartbeat answer at once when it states nothing its disk doesn't hold:** its term and vote durable
  (the leader's I1 rule) and no position past its durable log. That answer confirms the leader's term for the
  ReadIndex quorum and promises nothing about the log. Vendored hyper-raft as well. Amphora keeps the core's
  default `HeartbeatAnswers::Position` (raft.rs `Config::new`), so such an answer's position must name the
  durable end of its log, not its last index (`handle_heartbeat` sets `answer.index = last_index` today).

Either alone leaves the other flush in the path. Neither changes what a commit or an answer promises.

## Commands

    # on adacachy2: kiln run, on 00aad8daff2fa3c9daa65d53c425d450, sync false, cwd ~/.cache/amphora-perf/commits-remote
    (cd bench && CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits cargo build --release)   # r820, 8m46s
    python3 -I bench/thread_sample.py ~/.cache/amphora-perf-bin/commits/7f2650a9 29490 DIR W leader 5 2 3 replicated   # r851, cpus=4,mem=4g

# Lane V3 (commits) on adacachy2: the barrier's two flushes removed on a branch, not merged (2026-11-23, 14:38Z to 15:33Z)

Ada's goal is replicated commits about 10x faster than dqlite and rqlite. AC-T1.8.3 asks p99 at most 2 ms for 3
nodes on a LAN. V2 found each write's ReadIndex barrier waiting on the leader's log write and one follower's.
This hour built V2's two fixes and the red test, on branch `perf/commits-v3`.

## Losses first

1. **Nothing merged to main.** The fixes change vendored hyper-raft's consensus core. Their own crate's
   invariant tests (`tests/pipeline.rs`, `tests/strategies.rs`, the lib tests) never ran: kiln refused this lane's
   runs for room four times (0 to 3 of 32 cores idle). The new heartbeat-position oracle lives in those tests, so the
   fix for (2) is unproven by its own test. It waits for that run.
2. **The replicated suite has one failure, and it is main's.** At 6237d769: 136 passed, 1 failed, 4 ignored, 131 s,
   load 36 to 71 (r926). The failure is `a_write_at_a_follower_is_answered_on_its_groups_commit`, at
   `assert_eq!(taken.len(), 1)`. It fails the same way without the fix, at 4f2626de (main 9a7b14bb plus the new test
   alone): 1 run of 1, 123 s (r939, stopped after that run). One run is not a rate. It is not this change's, by that
   A/B.
3. **Units 3 (the ladder against dqlite and rqlite) and 4 (batching) were not started.**

## Unit 1, the red test: red on main, green with the fixes

`v3_two_replicated_writes_in_a_row_answer_with_every_log_flush_held` (replicated.rs): three voters behind log gates,
three fates, two to span. Warm writes run until each follower states a durable promise. Then every member's raft-log
flushes are held, and two writes go to the leader one after the other.

| commit | first write | second write | run |
|---|---|---|---|
| 4f2626de (main plus the test) | answered while held | answered only once the gates let the flushes go | r922 |
| 6237d769 (the fixes) | answered while held | answered while held | r926, 2.88 s |

## Unit 2, the fixes (6237d769, vendored hyper-raft change 0009)

1. **Reads without a `Ready`.** `RawNode::light_now` asks the round for the reads that wait (`Raft::ask_reads`). It
   gives the reads confirmed since the last `Ready` (`LightReady::read_states`). `Replica::take_light` queues them as
   `take_ready` does, and they're released once applied, as before.
2. **A heartbeat answer at once.** A member whose term and vote are durable answers a heartbeat beside the held
   answers (`Raft::answer_beat_now`). Its position is `(Log::persisted, its term)`, the durable end of its log, not
   `last_index`. Its commit is the durable commit. A leader takes a position of its term as an append's answer, for
   the classic quorum and for the reads it confirms, so it never counts one past that member's durable log. Where
   the persisted entry's term can't be read (a snapshot not yet durable), the answer names no position. The durable
   term and vote moved from `RawNode` to `Raft` (`durable_vote`), so the answer is decided where it is made.
   Answers made otherwise keep today's path and position.
3. **The test for (2)** is an oracle in the lagged harness (`tests/support/lagged.rs`). Every heartbeat answer that
   names a position of the current term must name an index the member's disk holds, whenever it leaves. The harness
   also takes the reads a light ready gives. It runs under `pipeline.rs`'s random interleavings, including
   `held_commits_across_fates_keep_every_invariant`, which also checks that every answered read saw what was
   committed when it was asked. **Not run yet.**
4. **An Amphora-level guard for (2)**, `v3_a_leader_counts_no_follower_position_past_its_durable_log` (81043799):
   both followers' log flushes held, two writes at the leader, the second's barrier answered by heartbeats while the
   followers hold the first in memory alone. Each follower must hold entries past its durable log, and the
   leader's progress for it must stay at or below that durable end. It should be red on main (the second write
   stalls) and green with the fixes. **Not compiled or run yet.**

Checks at 6237d769: clippy `-p amphora --all-targets` clean, clippy `-p hyper-raft -p hyper-durable` (lib) clean
(r926). `scripts/vendor.py check hyper-raft`: 8 recorded changes, 0 differences.

A side effect to measure, a guess not seen: with positions at the durable end, a follower in the middle of a long
flush answers heartbeats without news. A leader whose window to it is full then probes it after a beat
(`Progress::heard_heartbeat`). That costs a resend, not correctness.

## Commands

    # on adacachy2: kiln run, on 00aad8daff2fa3c9daa65d53c425d450, needs linux/amd64,cpus=8,mem=24g, sync false,
    # cwd ~/.cache/amphora-perf/commits-remote, CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits, CARGO_INCREMENTAL=0
    cargo test -p amphora --lib --locked -- --exact database::replicated::tests::v3_two_replicated_writes_in_a_row_answer_with_every_log_flush_held   # r922, r926
    cargo test -p amphora --lib --locked -- database::replicated:: --test-threads=8                                                                  # r926
    cargo test -p amphora --lib --locked -- database::replicated::tests::v3_ --test-threads=2                                                     # not run: refused for room
    cargo test --manifest-path vendor/hyper-raft/Cargo.toml -p hyper-raft --lib --test pipeline --test strategies -- --test-threads=8              # not run: refused for room

# Lane V4 (commits) on adacachy2: hyper-raft's own tests run on change 0009, and they refuse it (2026-11-23, 15:26Z to 16:21Z)

Ada's goal is replicated commits about 10x faster than dqlite and rqlite; AC-T1.8.3 asks p99 at most 2 ms for 3
nodes on a LAN. V3 built change 0009 (reads confirmed and heartbeats answered without a write) on
`perf/commits-v3`. This hour ran its proof: hyper-raft's and hyper-durable's own tests, the two v3 tests, the
replicated suite and clippy.

## Losses first

1. **0009 is not merged.** hyper-raft's own tests, run on it for the first time, fail in two suites that pass
   without it: `tests/differential.rs` 2 of 8 pass (base 8 of 8), `tests/pipeline.rs` 7 of 12 run pass (base 12 of 12).
   One defect was found and fixed (the incarnation, below). The pipeline failure's cause is not found, and the
   differential failure needs a design choice. Both are open.
2. **Units 3 (the ladder) and 4 (batching) were not started.** The bench did not build on main (fixed in
   `bba65c73`, merged with this record).
3. **The proof runs on upstream's tree, under rustc 1.99.1.** The vendored copy can't build hyper-raft's tests:
   `vendor/MANIFEST.toml` takes 15 crates, not `hyper-check`, which hyper-raft's dev-dependencies need. The runs
   clone upstream on adacachy2, check out the pin `d5d5f8f6`, apply `vendor/patches/hyper-raft/*.patch` in order
   and test there. Base is the same tree without 0009. Upstream pins 1.98.1, and Amphora builds these crates
   with 1.99.1. Clippy there needs `-A deprecated`: 1.99 deprecates `fetch_update` in `hyper-rt`, which base fails
   the same way (r1037).

## What the runs found

Load 45 to 100 (1 min) on 32 cores, CPU pressure "some" 12 to 58%, I/O "some" under 8% (avg10), every step.

| suite | base: pin + 0001..0008 (r1037, r1081) | 0009 at V3's `6237d769` (r1037) | 0009 at `3abd6ec7` (r1081, r1083) |
|---|---|---|---|
| hyper-raft lib | 161 of 161 | 153 of 161 | 160 of 161 (the last fixed in `6a5d0a13`, see below) |
| `differential.rs` | 8 of 8 | not run (cargo stopped at lib) | 2 of 8 |
| `pipeline.rs` | 12 of 12, 1 ignored | not run | 7 of 12, 1 ignored |
| `strategies.rs` | 12, 37 ignored (campaigns, release) | not run | 12, 37 ignored |
| `check`, `backlog`, `fast`, `group`, `repair`, `suspicion` | pass | not run | pass |
| hyper-durable, every target | not run | pass | pass (lib 6, directed 5, hyperlog 14, keep 2, liveness 21 + 5 ignored, shell 47, sim 5, threads 1) |
| clippy hyper-raft + hyper-durable, all targets | fails in hyper-rt only (deprecated) | fails in hyper-rt only | clean with `-A deprecated` |
| amphora `v3_` tests | | 2 of 2 | 2 of 2 (V3's guard test `v3_a_leader_counts_no_follower_position_past_its_durable_log` compiled and run for the first time: green) |
| amphora replicated suite | | | 138 of 139, 4 ignored, 130 s (the one failure is main's, below) |
| clippy `-p amphora --all-targets -D warnings` | | | clean |

**1. A defect, fixed (`3abd6ec7`).** `Raft::send` states the member's incarnation on every append and heartbeat
answer (`held_asking`, R-8): a member marked through a promise of its term asks its leader where its log ends.
`Raft::answer_beat_now` built its answer beside `send` and left it out, so such a member's mark was never narrowed
(`a_member_opened_with_a_promise_is_marked_and_its_leader_narrows_the_mark`). The early answer states it now.

**2. The core's tests, moved to the new rule (`3abd6ec7`, `6a5d0a13`).** Their `drain` helpers dropped `Ready::held`,
where 0009's answers leave. They take it now, as an owner does. Three tests held the old rule, that a beat's answer
waits for the write that holds what it says. They hold the new one: the answer leaves at once, naming the durable
end of the log and the durable commit. `a_followers_answer_waits_for_the_write_that_holds_what_it_says` is renamed
`a_followers_heartbeat_answer_names_only_what_its_disk_holds`. A beat in a new term, whose vote isn't durable, still
waits for the write (`an_answer_states_no_commit_that_no_durable_write_stated` holds that).

**3. `differential.rs`: the cores differ on the commit a heartbeat answer states (open, a design choice).** For
example, "seed 0, step 180: member 2 messages: raft-rs MsgHeartbeatResponse 2->1 term 1 commit 7, hyper-raft commit 4"
(r1081). The early answer states the durable commit (4), as upstream's rule says every answer must
(`an_answer_states_no_commit_that_no_durable_write_stated`). Without 0009 the answer waited for the write, by which
time 7 was durable. The answer can't wait only when its commit would be clamped: a beat that moves the commit is
exactly the barrier's case, so that would put the follower's flush back. The choice for the successor:
- (a) the early answer is an owner's opt-in (a `Config` field, off by default), so the core in its default,
  raft-rs-like settings agrees with raft-rs, and Amphora's replicated owner turns it on; the lagged pipeline
  harness then opts in to keep 0009 under its oracles;
- (b) the differential harness stops comparing the commit of heartbeat answers.
(a) keeps the differential oracle whole; recommended.

**4. `pipeline.rs`: the new oracle sees answers past the disk (open, cause not found).** Five schedules
(`faults_at_rest_lose_nothing_acknowledged`, `faults_at_rest_on_two_members_at_once_lose_nothing_acknowledged`,
`held_commits_across_fates_keep_every_invariant`, `members_of_one_fate_make_no_held_commit_under_any_schedule`,
`random_interleavings_by_suspicion_keep_every_invariant`) fail V3's oracle in `tests/support/lagged.rs`. r1106
widened its message:

| schedule | named | disk's last | answer's log_term | answer's term | disk's term there | disk hard (term, vote, commit) |
|---|---|---|---|---|---|---|
| faults at rest, one member and two | 26 | 9 | 1 | 4 | none | (4, 0, 2) |
| held commits across fates | 26 | 10 | 2 | 3 | none | (3, 0, 5) |
| members of one fate | 24 | 8 | 2 | 3 | none | (3, 0, 3) |
| interleavings by suspicion | 15 | 4 | 1 | 4 | none | (4, 0, 0) |

In every one, the named position is of an older term than the answer, and the disk holds nothing there. A leader
takes a heartbeat's position only when it is of the leader's own term (`handle_heartbeat_response`,
`message.log_term == self.term`), so no leader would count these. But the answer names `Log::persisted()`, which
should never pass the disk (`Log::maybe_persist` checks the storage's term at the index), and here it is 26 against
9. The oracle checks every `log_term != 0`, stricter than V3's stated rule ("a position of the current term").
Narrowing it to that rule would likely pass these five, but it would leave `persisted` ahead of the disk
unexplained, so it was not narrowed. Candidates, none checked: a write noticed durable whose entries the
harness's disk then lost (faults at rest, or a fate's loss) without the member restarting; `Log::new` counting
entries the store keeps in memory (in-place readies, `on_persist_keeping`) as persisted; the disk's tail replaced
in the same term between the answer and its leaving. The next step is one run: log `raw.raft.log().persisted()` and
`disk.last_index()` at each step of `faults_at_rest_lose_nothing_acknowledged` and find the first step where the
first passes the second.

The tip `6a5d0a13` holds 1 and 2. Every hyper-raft target there (r1149, 16:05Z to 16:08Z, load 72 to 83): lib
161 of 161, `differential.rs` 2 of 8 (3), `pipeline.rs` 7 of 12 with 1 ignored (4), and backlog 1, check 11, fast 15,
group 12, repair 7, strategies 12 (37 ignored), suspicion 35, timed 2 (1 ignored) and toolchain 1, all passing.

## Main's own failures (for a lane)

1. **`a_write_at_a_follower_is_answered_on_its_groups_commit` fails every run, on main and on this branch**: 6 of 6
   on main `ea04b5b7` and 6 of 6 at `3abd6ec7`, 122 to 128 s each, one at a time from copied test binaries (r1083).
   It is not a flake: the results are `[Err(Serialization { what: "prepared transaction", why: "the database
   advanced since preparation" }), Ok([2])]`. Exactly one write applies, as designed, but the test expects the
   loser's refusal as `Refusal::Invalid`. `876e68e2` (T-T1.8.W1) and `4add0134` (lane W3) made it
   `Refusal::Serialization`, retryable. Still stale on origin/main `1df94b59` (replicated.rs line 7896). For lane W3.
2. **bench/ did not build on main** since `80c3a431` (lane perf-A8 made `store::Config::queue` an `Option<usize>`):
   `bench/src/store.rs:195`, E0308. bench/ is outside the root workspace, so main's checks never built it. Fixed in
   `bba65c73`: `AMPHORA_QUEUE` sets `Some(q)` and the default (no count bound) stays otherwise.

## The patch number

Lane P8's `perf/serve-p7` also records hyper-raft `0009` (hyper-durable's `Replica::hold_apply`). origin/main
`1df94b59` holds 0001 to 0008. Whichever merges second renumbers its change to 0010: merge origin/main, move its
0009 aside, `python3 -I scripts/vendor.py record hyper-raft "<title>" --why "<why>"` (it takes the next number),
update the file name in its `vendor/hyper-raft/UPSTREAM.md` row, then `python3 -I scripts/vendor.py check hyper-raft`
clean.

## Commands

    # on adacachy2: kiln run, on 00aad8daff2fa3c9daa65d53c425d450, needs linux/amd64,cpus=8,mem=24g, sync false
    # hyper-raft's proof, in an upstream clone (~/.cache/amphora-perf/commits-hyper-raft), RUSTUP_TOOLCHAIN=1.99.1,
    # CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits-hyper-raft, CARGO_INCREMENTAL=0
    git checkout -f --detach d5d5f8f66368bb8a08822518dd45d257e95e2dbf && git clean -fd
    git -C ~/.cache/amphora-perf/commits-remote archive <sha> vendor/patches/hyper-raft | tar -x -C <P>
    for p in <P>/vendor/patches/hyper-raft/*.patch; do git apply --whitespace=nowarn $p; done
    cargo test -p hyper-raft --lib --tests --no-fail-fast -- --test-threads=8                       # r1081, r1149
    cargo test -p hyper-durable --lib --tests --no-fail-fast -- --test-threads=8                    # r1081
    cargo clippy -p hyper-raft -p hyper-durable --all-targets -- -D warnings -A deprecated          # r1081
    git apply -R <P>/vendor/patches/hyper-raft/0009-*.patch   # base, then the same tests            # r1037, r1081
    # Amphora, cwd ~/.cache/amphora-perf/commits-remote, CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits
    cargo test -p amphora --lib --locked -- database::replicated::tests::v3_ --test-threads=2       # r1083
    cargo test -p amphora --lib --locked -- database::replicated:: --test-threads=8                 # r1083
    cargo clippy -p amphora --all-targets --locked -- -D warnings                                   # r1083
    (cd crates/amphora && ~/.cache/amphora-perf-bin/commits/tests-<sha> --exact database::replicated::tests::a_write_at_a_follower_is_answered_on_its_groups_commit)   # r1083, 6 a side

# Lane V5 (commits) on adacachy2: change 0009's two open items closed, every hyper-raft suite green (2026-11-23, 16:19Z to 17:19Z)

Ada's goal is replicated commits about 10x faster than dqlite and rqlite; AC-T1.8.3 asks p99 at most 2 ms for 3
nodes on a LAN, never at the cost of safety. V4 left two failures in hyper-raft's own suites with change 0009. This
hour closed both on `perf/commits-v5` (`2431406e`).

## Losses first

1. **V4's premise for `pipeline.rs` was wrong, and the oracle was narrowed, not the core fixed.** No step ever had
   `Log::persisted()` past the disk (below). The five schedules' answers came from the default path, which 0009
   doesn't change, and named positions of older terms that no leader takes. The oracle now holds every position
   of the answer's own term to the disk: V3's stated rule and the leader's (`handle_heartbeat_response` takes a
   position only when `log_term == term`). The coordinator said not to loosen the check; this is the evidence for
   doing it, and the stricter alternative is in "Decisions".
2. **No A/B numbers yet.** The replication ladder (S4's family (a): Amphora persisted and replicated at
   `924fbe5c` and at `2431406e`, dqlite and rqlite, writers 1/32/128, 5 rounds, order reversed each round) was
   queued as r1238 at 16:59Z with cpus=16, behind this lane's checks. Its records land on adacachy2 under
   `~/.cache/commits-ab/ladder-v5/` (one folder an arm, `rounds.txt` with load and pressure beside each round); its
   output ends with a median table. The successor reads them. Each arm's CPU cost calibration (RULES 5e) was not
   copied across arms; the commit path is plain inserts, so a guess is that it doesn't matter, not checked.

## Where the named positions came from (diagnostics, branch `diag/commits-v5-watch`)

Three runs of the five failing schedules (`pipeline.rs`), 0009 at `e0f2aaf7` plus a test-only watch:

| run | what it added | what it found |
|---|---|---|
| r1205, 16:31Z | after every harness op (step, tick, take, durable, notify, compact, ...), `persisted() <= disk.last_index()` and the disk's term there equal to the log's | fired only where a member restored a snapshot not yet durable: `persisted` 17, the log's first index 37, `log.term(17)` 0, the disk holding 17 of term 2. Not past the disk |
| 16:34Z (`736d472e`) | the same watch, `persisted() <= disk.last_index()` alone | never fired; the oracle fired as in V4 (26 with 9, 26 with 10, 24 with 8, 15 with 4) |
| 16:36Z (`8f6aeaf6`) | each heartbeat answer tagged by the path that built it | every one of the five was built by the default path (term and vote not yet durable), naming the log's last index, with `persisted()` equal to it when built (tag 2000026 for "26 with 9") |

The mechanism: a follower of term 1 holds 1..26, durable. A heartbeat of a new leader moves it to term 4. Its vote
isn't durable, so it answers by the default path, naming `(26, term 1)`, and the answer waits for the write of
term 4. The new leader's append arrives first and replaces the tail from 9. That same write puts 9 of term 4 on
the disk, and the answer leaves with it: "named 26 with 9 durable". A leader of term 4 ignores a position of term
1. The early answer (`answer_beat_now`) never named a position past the disk.

## The early answer is an owner's option (V4's option (a))

`Config::answer_beats_now`, off by default as raft-rs's rule. Off, a heartbeat's answer waits for the write that
holds what it says, as before 0009, so the core in raft-rs's settings agrees with raft-rs. Amphora's replicated
owner turns it on (`replicated.rs`, beside `fates` and `apply_unpersisted`). The core's three tests keep the
default rule again; a fourth, `a_followers_heartbeat_answer_names_only_what_its_disk_holds`, holds the new rule
under the option. The tests' `Settings::answer_beats_now` sets it, and the lagged harness turns it on, so 0009
stays under its oracles. Reads confirmed without a `Ready` stay as V3 built them: an owner gets them only by asking
`RawNode::light_now`, which hyper-durable's `Replica::take_light` and the lagged harness do and the differential
harness doesn't.

## Runs at `2431406e`

| suite | result | run |
|---|---|---|
| hyper-raft lib (upstream clone at the pin, patches 0001..0009, rustc 1.99.1) | 162 of 162 (V4: 161; the new opt-in test) | r1216 |
| `differential.rs` | 8 of 8 (V4: 2 of 8) | r1216 |
| `pipeline.rs` | 12 of 12, 1 ignored (V4: 7 of 12) | r1216 |
| backlog, check, fast, group, repair, strategies, suspicion, timed, toolchain | 1, 11, 15, 12, 7, 12 (37 ignored), 35, 2 (1 ignored), 1: all pass | r1216 |
| hyper-durable, every target | lib 6, directed 5, hyperlog 14, keep 2, liveness 21 (5 ignored), shell 47, sim 5, threads 1: all pass | r1216 |
| clippy hyper-raft + hyper-durable, all targets, `-D warnings -A deprecated` | clean | r1216 |
| clippy `-p amphora --all-targets -D warnings`; `-p hyper-raft -p hyper-durable` (lib) | clean | r1217 |
| amphora replicated suite and the `v3_` tests | 140 of 141, 5 ignored, 131.7 s; both `v3_` tests pass. The one failure is main's own, `a_write_at_a_follower_is_answered_on_its_groups_commit` (V4: 6 of 6 on main; unchanged on main `5e6c619f`) | r1217 |
| `scripts/vendor.py check hyper-raft` | 8 recorded changes, 0 differences | local |
| merged with origin/main (`faaf2a7d`): clippy `-p amphora --all-targets`, clippy `-p hyper-raft -p hyper-durable`, the `v3_` tests | clean, clean, 2 of 2 | r1236 |
| merged with origin/main again (`32f9a6ce`, lane W5's hyper-raft 0011 beside 0009; `vendor.py check`: 9 recorded changes, 0 differences): hyper-raft lib, `differential.rs`, `pipeline.rs`, clippy hyper-raft all targets, clippy `-p amphora --all-targets`, the `v3_` tests | 163 of 163, 8 of 8, 12 of 12 (1 ignored), clean, clean, 2 of 2 | r1244 |

Load on adacachy2 during r1216: 9 to 33 (1 min), CPU pressure "some" avg10 3 to 7%, I/O under 5%.

## Decisions for Ada (plain choices)

1. **The heartbeat oracle's scope.** It now checks every position of the answer's own term, the only positions a
   leader counts. The stricter form (every nonzero position) fails on the default path, which names its log's last
   index and can leave after a new leader's entries replaced that tail. To keep the stricter form, the default
   path would have to name no position of an older term. That changes the core's default answers, and no leader
   reads those positions today. Recommended: keep it as merged.
2. **A position of term 0.** With a snapshot restored and not yet durable, `answer_beat_now` names `persisted` with
   log term 0 (`Log::term` gives 0 below the snapshot). No leader takes it, but its comment says it "states no
   position". A one-line `term != 0` guard would make the code say what the comment says. Not changed here.

## Commands

    # hyper-raft's proof: kiln run on 00aad8daff2fa3c9daa65d53c425d450, needs linux/amd64,cpus=8,mem=24g, sync false;
    # upstream clone ~/.cache/amphora-perf/commits-hyper-raft at d5d5f8f6 plus vendor/patches/hyper-raft/*.patch of
    # the commit named, RUSTUP_TOOLCHAIN=1.99.1, CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits-hyper-raft
    cargo test -p hyper-raft --test pipeline --no-fail-fast -- --test-threads=8 faults_at_rest held_commits_across_fates members_of_one_fate random_interleavings_by_suspicion   # r1205 and the 16:34Z and 16:36Z runs, diag/commits-v5-watch
    cargo test -p hyper-raft --lib --tests --no-fail-fast -- --test-threads=8        # r1216 at 2431406e
    cargo test -p hyper-durable --lib --tests --no-fail-fast -- --test-threads=8     # r1216
    cargo clippy -p hyper-raft -p hyper-durable --all-targets -- -D warnings -A deprecated   # r1216
    # Amphora: cwd ~/.cache/amphora-perf/commits-remote, CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits
    cargo clippy -p amphora --all-targets --locked -- -D warnings                    # r1217, r1236
    cargo test -p amphora --lib --locked -- database::replicated:: --test-threads=8  # r1217
    cargo test -p amphora --lib --locked -- database::replicated::tests::v3_ --test-threads=2   # r1236 at faaf2a7d
    # the ladder: after arm built in a worktree at 2431406e (~/.cache/commits-ab/2431406e, release, 1 min 42 s),
    # copied to ~/.cache/amphora-perf-bin/commits/2431406e; bench/repl_ladder.sh and repl_members.sh take ARM_BIN
    # (a sed in that worktree, undone after); needs cpus=16                         # r1238

# Lane V6 (commits) on adacachy2: r1238 collected, the term-0 guard merged, batching not started (2026-11-23, 17:22Z to 18:17Z)

Ada's goal is replicated commits about 10x faster than dqlite and rqlite; AC-T1.8.3 asks p99 at most 2 ms for 3
nodes on a LAN, never at the cost of safety. This hour read V5's ladder (r1238), merged the coordinator's term-0
guard into change 0009, and left batching (V's unit 2) to the successor.

## Losses first

1. **At 128 writers Amphora fails every round, both classes, both arms.** All 20 Amphora runs at 128 writers
   (persisted and replicated, `924fbe5c` and `2431406e`, rounds 1 to 5) end with "no live node acknowledged a write
   within --deadline-s" (10 s), the last refusal "invalid prepared transaction: the database advanced since
   preparation". That refusal is the follower's write slot losing to the leader's own queue (a write prepared on a
   base that moved; the test near replicated.rs 7885 asserts it). The client retries it until the deadline. At 32
   writers p99 is already 4.7 to 7.0 s, so at 128 a write at the back of the queue nears 10 s before any retry
   (inferred from 128 / 13 to 16 commits/s, not measured). This is a functional failure, not only a slow rate:
   writes are refused under load. rqlite runs the same 128-writer step at 1,195 to 2,459 commits/s.
2. **Change 0009 made no measurable difference at 1 writer, and replicated at 32 writers is likely slower.**
   Replicated at 32 writers: 16.0 commits/s before (14.8 to 17.0), 13.2 after (11.4 to 15.4), -18%, exact rank-sum
   p = 0.016 over 5 rounds a side; without round 1 (it overlapped builds), 15.8 against 12.6, p = 0.057. The
   limiter of that loss isn't named: no profile ran. So the verdict is a probable loss, cause unknown. At 1 writer
   the replicated p50 is 71.7 ms before and 72.7 ms after: the barrier's two flushes that 0009 removes didn't
   shorten a lone write on this disk.
3. **No dqlite numbers.** dqlite's node 2 never started in any round: "create node: failed to set bind address
   127.0.0.1:54252: 1". Nothing listened on 54252 when checked afterwards (`ss -ltn`), and the error is the same in
   every round, so it is not a port left over from the round before (inferred). The bench's ports (54200 to 54252)
   sit inside adacachy2's ephemeral range (`ip_local_port_range` 32768 to 60999), so any outbound connection can hold
   one as its local port and refuse a bind; no socket of any state held 54252 at 17:55Z (`ss -tan`). That is a
   hazard, not a proven cause. Not diagnosed further this hour.
4. **rqlite has 3 rounds, not 5.** Rounds 1 and 2 couldn't bind 127.0.0.1:54210 ("address already in use"), most
   likely members the failing Amphora arm left running (round 1's own Amphora run hit "Address already in use"
   too). Rounds 3 to 5 ran.
5. **Round 1 overlapped builds.** Its first arm ran from 16:59:41Z to 17:08:31Z, about 9 minutes against about one
   for every later arm, with I/O pressure "some" avg10 9.3%. Round 1's before arm has no persisted 1-writer row.
6. **Batching (V's unit 2) was not started.** The hour went to the ladder, the guard and its proof. Its tests and
   design are below for the successor, with one more test from loss 1.

## r1238: the ladder (S4's family (a), 5 rounds, arm order reversed each round)

Load (1 min) 19 to 31 across the rounds, CPU pressure "some" avg10 0 to 3.9%, I/O pressure "some" avg10 3.9 to
10.2% (`rounds.txt`). 5 s windows, 1 s warmup, `--deadline-s 10`. Medians over the rounds that produced the row, range
in brackets, exact two-sided rank-sum p for before against after:

| system | writers | before `924fbe5c` commits/s | after `2431406e` commits/s | change, p | p50 ms before, after | p99 ms before, after |
|---|---|---|---|---|---|---|
| amphora persisted | 1 | 18.8 (16.4 to 22.2), 4 rounds | 19.2 (17.4 to 22.2) | +2%, 0.95 | 49.9, 49.7 | 102.9, 92.2 |
| amphora persisted | 32 | 12.4 (10.8 to 13.0) | 13.6 (11.6 to 20.4) | +10%, 0.38 | 1,802, 1,458 | 5,898, 6,226 |
| amphora persisted | 128 | refused, 5 of 5 | refused, 5 of 5 | | | |
| amphora replicated | 1 | 14.0 (9.4 to 14.2) | 14.0 (12.2 to 15.6) | 0%, 0.48 | 71.7, 72.7 | 204.8, 159.7 |
| amphora replicated | 32 | 16.0 (14.8 to 17.0) | 13.2 (11.4 to 15.4) | -18%, 0.016 | 885, 1,049 | 5,898, 6,291 |
| amphora replicated | 128 | refused, 5 of 5 | refused, 5 of 5 | | | |
| rqlite | 1 / 32 / 128 | 25.6 / 367.0 / 1,235.4 (3 rounds) | | | 38.4 / 79.9 / 90.1 | 58.4 / 249.9 / 237.6 |
| dqlite | all | unavailable, 5 of 5 | | | | |

Against rqlite, Amphora is 0.55x at 1 writer (14.0 against 25.6 commits/s) and 0.036x at 32 (13.2 against 367).
The goal is 10x.

Vetting (benchmark checklist):
- Why not double: V's limiter (one write in flight, each waiting on the leader's and one follower's log flush on
  btrfs) fits both arms' flat rate from 1 to 32 writers. 0009 changes neither, so equal rates at 1 writer are the
  expected result. The 32-writer replicated loss has no named limiter.
- Tuned: both arms are the release build of the same bench, same flags. Calibration (RULES 5e) wasn't copied across
  arms. It feeds only the optimizer's access-method costs (`translate/optimizer/learned_costs.rs`), and the bench's
  one statement, `INSERT INTO t VALUES (?1, ?2, ?3)` (`bench/src/repl.rs`), has no access method to choose. So
  calibration can't change either arm's plan (inferred from the code), and no round was rerun for it.
- Limits: 13 to 22 commits/s of 600-byte rows is far below any disk or network bound. The flush is the bound.
- Errors: every Amphora 128-writer run refused (loss 1). The 1- and 32-writer rows report no refusals. n per window
  is 47 to 111 commits, so p99 at 32 writers rests on fewer than 2 samples a window: read p99 there as the max.
- Reproduces: 5 rounds a side, interleaved and reversed. Ranges overlap in every cell except replicated at 32 writers.
- Happened: each row's `latency_ms.n` counts acknowledged commits. Rows were written by the members.

## Unit 2: an early heartbeat answer names no position of term 0 (merged)

The coordinator's decision on V5's question 2. With a snapshot restored and not yet durable, `Log::term` gives 0
for the disk's end, and `Raft::answer_beat_now` named `(persisted, 0)`. No leader took it, but the comment said
it states no position. Now it adds `&& term != 0` to the `if let` on `self.log.term(durable)`, so the code says
what the comment says. The heartbeat oracle stays as V5 merged it.

- Test `an_early_heartbeat_answer_names_no_position_below_an_unwritten_snapshot` (hyper-raft `src/tests.rs`): a
  follower under `answer_beats_now` steps a snapshot at 7, issues its write without making it durable, and answers
  a heartbeat. The answer leaves at once and names `(0, 0)`. Red with the guard removed (r1285: it named `(2, 0)`), green with it.
- r1285 at `cd43a18e`: hyper-raft lib 164 of 164 (V5: 163), `pipeline.rs` 12 of 12 (1 ignored), `differential.rs` 8 of
  8, clippy hyper-raft all targets clean. r1271 at `5ea5c95c` (the same code; `cd43a18e` changes only the new test's
  setup): clippy `-p amphora --all-targets` clean, the `v3_` tests 2 of 2. Load on adacachy2 16 to 29 (1 min).
- Recorded in `0009` (regenerated from upstream `d5d5f8f6` plus `0001` to `0008`). `vendor.py check hyper-raft`: 9
  recorded changes, 0 differences. UPSTREAM.md's 0009 row says it.

## Unit 3, batching, for the successor

V's design stands (one Raft entry carrying several queued writes, each under its own savepoint with its own answer;
a batch is what queued during the last flush, capped by `Actor::entry_room`, no timer; one writer stays one write an
entry). V's four behaviour tests stand. Loss 1 adds a fifth: **at 128 writers spread over three members, every
write is answered before its deadline** (today none is). Batching only the leader's own queue (V's step 3) won't
pass it, because the refusals are the followers' write slots losing to the leader's queue. A follower's write has
to join the leader's batch rather than race it.

## Unit 4, for Ada's later decision (not acted on): machines for a fair replicated-class setup

The fair setup is three machines on one LAN, a member on each, for Amphora, dqlite and rqlite alike. The one-host
bench puts three "fates" on one disk and one kernel, so it fakes them. From `runners` at 17:40Z, every machine is
on 192.168.1.0/24:

| machine | what it is | can host | what it lacks |
|---|---|---|---|
| adacachy2 (`00aad8da…`) | Linux x86-64, 32 cores, 128 GB, btrfs | all three systems | it is the only free Linux machine: two more like it are needed |
| adacachy (`451397fc…`) | Linux x86-64, 32 cores, 128 GB | all three | it runs production; RULES 6 forbids benchmarks there |
| fedora | Linux | unknown | stale in kiln: "never said its machine id" |
| jupiter (`f766f092…`) | Windows | rqlite natively, the rest through Docker Desktop | a Linux kernel and disk of its own (a VM's flush isn't the host's) |
| Mac Studio 2 (`d1323898…`) | macOS arm64, 32 cores, 96 GB | Amphora, rqlite | dqlite (libdqlite builds for Linux only: from memory, not checked this hour); reserved for the competitor benchmark, alone |
| Mac Studios `cec37f60…`, `b24c5c95…`, `2768d0d5…` | macOS arm64, 14 cores, 36 GB | Amphora, rqlite | dqlite; 0.06 to 7 GB of memory free at 17:40Z; `2768d0d5` is off limits |
| MacBook Pros `e30189ea…`, `f3f618f2…`, `75694786…` | macOS arm64, 15 to 18 cores | Amphora, rqlite | dqlite; laptops (Wi-Fi or dock, power states) |

No three alike machines here can host all three systems on equal terms. Three like Linux machines (adacachy2's
class, wired) would. Three like Macs could compare Amphora with rqlite only.

## Commands

    # r1238 read in ~/.cache/commits-ab/ladder-v5/ on adacachy2 (one folder an arm, roundN/*-commit.jsonl, err.txt, rounds.txt)
    # unit 2's proof: kiln run on 00aad8daff2fa3c9daa65d53c425d450, needs linux/amd64,cpus=4,mem=12g, sync false;
    # ~/.cache/commits-v6/hr.sh SHA: the upstream clone at d5d5f8f6 overlaid with the commit's vendor/hyper-raft,
    # RUSTUP_TOOLCHAIN=1.99.1; the new test with the guard removed (red), then restored:
    cargo test -p hyper-raft --lib -- --test-threads=4
    cargo test -p hyper-raft --test pipeline --test differential -- --test-threads=4
    cargo clippy -p hyper-raft --all-targets -- -D warnings -A deprecated
    # ~/.cache/commits-v6/am.sh SHA FILTER: cwd ~/.cache/amphora-perf/commits-remote, CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits
    cargo clippy -p amphora --all-targets --locked -- -D warnings
    cargo test -p amphora --lib --locked -- --test-threads=4 database::replicated::tests::v3_

# Lane V7 (commits) on adacachy2: 0009's loss is the followers' writes, and A and B designed, not built (2026-11-23, 17:57Z to 18:52Z)

Ada's goal is replicated commits about 10x faster than dqlite and rqlite, never refusing writes under load, at agentic
Meta scale. V6 found every Amphora run at 128 writers refused, and change 0009 a probable loss at 32 replicated
writers. This hour profiled that loss, moved the bench's ports out of the ephemeral range, and designed the two write
paths Ada asked for. Neither path is built.

## Losses first

1. **Neither design is built, and no ladder ran (units 2 and 3).** The hour went to unit 1's profile and the port fix.
   Both designs are sketched below with the data shape, the switch, where each change goes and the tests, so the
   successor can build them. The architect skill's multi-model arena did not run, so this is one design pass. Test
   5 is written and red on `perf/commits-v7` (below), not merged.
2. **0009 is worse than V6 measured.** At 32 writers spread over three members, 6 of 6 runs of `2431406e` (0009)
   aborted with "the database advanced since preparation" (a write still refused at its 10 s deadline), against 1 of
   6 for `924fbe5c` (before 0009), over r1312 and r1346. V6's r1238 still got 13.2 commits/s from it. Both arms sit
   at the deadline's edge: before 0009 the slowest write took 6.7 to 10.4 s.
3. **0009's mechanism is inferred, not traced.** The profile shows where the loss is (followers' writes, not the
   leader's own path). The line in the code that makes followers lose more often is a guess, labelled below.
4. **The profile ran at cpus=8, not 16.** kiln refused cpus=16 (14 of 32 cores idle). The three members used about
   3.2 cores, most of it one spinning thread each; the driver wasn't measured. Load (1 min) 23 to 30, CPU pressure "some" avg10 1.2 to 7.6%, I/O "some" 3.0 to 8.4%.
5. **The dqlite fix has one smoke run, not a ladder.** dqlite started in 2 of 2 phases on the new ports, after 0 of 5
   rounds in r1238 on the old ones. One run per system and writer count measures nothing yet.

## Unit 1: where 0009's 32-writer loss goes (r1312, r1346)

`bench/thread_sample.py`, members on btrfs (`~/.cache`), 32 writers, class replicated, family a's flags, 8 s windows
sampled from 3 s to 8 s, the two arms' release binaries from `~/.cache/amphora-perf-bin/commits/`, 3 rounds, order
reversed each round. "spread" routes writer w to member w % 3 (the ladder's). "leader" sends every writer to the
leader, the control: it takes the followers' write path out.

| route | arm | commits/s, per round | median | p99 ms | slowest ms | aborted |
|---|---|---|---|---|---|---|
| leader | `924fbe5c` before | 23.25, 20.75, 45.75 | 23.25 | 1,688, 1,917, 1,622 | 1,659 to 1,954 | 0 of 3 |
| leader | `2431406e` 0009 | 45.25, 23.25, 25.25 | 25.25 | 1,229, 1,589, 1,376 | 1,250 to 1,612 | 0 of 3 |
| spread | `924fbe5c` before | 19.25, 18.75, 19.88 | 19.25 | 7,799, 9,175, 6,029 | 6,740 to 10,357 | 0 of 3 |
| spread | `2431406e` 0009 | none | none | none | none | 3 of 3 |

What the numbers say:

- **The leader's own path didn't change.** Leader-routed, the two arms overlap (medians 23.25 and 25.25, both
  bimodal near 23 and 45). 0009 didn't slow the leader's writes, and it didn't speed them up either.
- **The loss is the followers' writes.** With two thirds of the writers at followers, 0009 turns a slow run into a
  failing one. Every abort's last refusal is the stale base.
- **No thread is busy with the work.** The owner's `amphora.replica` threads use 0.03 to 0.13 of a core on every member
  in both arms. Each follower's `hyper-log-io` sits in `fdatasync` on its log (fd 16). Every member has one thread
  named after the binary at 0.90 to 0.99 of a core, `R`, in both arms and both routes. That matches V's note on the
  bench relay's spinning threads (inferred from the name and the note, not traced), and it doesn't differ by arm. The
  time goes to waiting: on flushes, and on followers' writes that commit and are then refused at apply.
- **Wasted entries.** Before 0009, spread, round 1: the leader committed 149 entries in the 5 s span (113 held, 36
  classic) while the driver acknowledged 19.25 writes/s, about 96 in that span. So about a third of the entries
  carried a write that was refused at apply and retried (inferred: a refused entry still commits).

r1346 repeats spread routing at 16 writers, where 0009's runs finish, and at 32. "unacked" is the share of the
leader's committed entries (held plus classic, over the span) that no caller got as an acknowledgement (driver's
writes/s over the window). The two windows differ (5 s span, 8 s window), so the share is approximate:

| writers | arm | acks/s, per round | median | leader's entries/s | unacked share | p99 ms | aborted |
|---|---|---|---|---|---|---|---|
| 16 | `924fbe5c` before | 17.0, 18.6, 16.6 | 17.0 | 24.0, 28.2, 24.8 | 0.29, 0.34, 0.33 | 3,539 to 4,981 | 0 of 3 |
| 16 | `2431406e` 0009 | 11.6, 12.3, 11.1 | 11.6 | 19.6, 22.2, 18.0 | 0.41, 0.45, 0.38 | 6,423 to 8,913 | 0 of 3 |
| 32 | `924fbe5c` before | 17.3, aborted, 13.0 | | 25.4, 25.0, 21.8 | 0.32, none, 0.40 | 6,750 to 8,651 | 1 of 3 |
| 32 | `2431406e` 0009 | aborted 3 times | | 23.6, 22.0, 20.8 | none | none | 3 of 3 |

At 16 writers 0009 is 32% slower (17.0 to 11.6 acks/s), and every round of it is below every round before it (exact
rank-sum p = 0.1, the smallest 3 rounds a side can give). Both factors move: the leader commits fewer entries (24.8
to 19.6/s), and more of them carry a write refused at apply (0.33 to 0.41).

**Why 0009 hurts (a guess that fits both factors, not traced).** 0009 lets a ReadIndex barrier finish without the
leader's log flush and one follower's. A barrier answers with the leader's commit index, not its last index. Before
0009 the barrier waited a flush, and the leader's entries in flight mostly committed meanwhile. Now a follower's
barrier returns while they are still in flight. The follower prepares on a base the leader's log has already passed,
and its write is refused at apply. Each refusal costs a wasted entry and then a write slot, in which the leader appends
nothing of its own. That would explain both factors: more refused entries, and fewer entries a second. The leader-routed
control has no follower writes, so it has neither cost, and it doesn't move. Every write in this bench inserts into
one table, so read sets can't rebase any of them (they share the rightmost leaf and page 1's size). And each member
forwards one write at a time while about 10 of its writers queue behind it, so a write at the back of a follower's
queue waits through about 10 rounds, plus a refusal and a slot for each stale base. Past 10 s it is refused for good.

**Half of the guess tested on driven time (18:36Z, adacachy2, cpus=4).** Amphora turns on 0009's early heartbeat
answer with `answer_beats_now: true` (replicated.rs, the core's config). Test 5 (below) ran 3 times with it on and 3
with it off, alternating, from one tree with that line flipped. Stale-base refusals with it on were 55, 72 and 78
(persisted) and 84, 83 and 84 (replicated). With it off they were 58, 72 and 78, and 83, 84 and 85. No difference, so
the early heartbeat answer alone doesn't raise refusals on driven time. The other half, reads confirmed without a
`Ready`, has no switch and wasn't tested. Driven time may not model the flush timing the guess rests on. The guess
stays a guess, and the bench's numbers above stand as measured.

**Recommendation on 0009: revert it from main, with this evidence.** No gain was measured anywhere on this disk: at 1
writer 14.0 against 14.0 commits/s (r1238), leader-routed at 32 writers 25.25 against 23.25 (r1312, ranges overlap).
It has a functional loss: 1 of 6 to 6 of 6 runs aborted at 32 spread writers (r1312, r1346), and 17.0 to 11.6
acknowledged writes/s at 16 (r1346), on top of r1238's -18% at 32. Under
design B no follower prepares, so the way 0009 hurts goes away. 0009 can come back after B, with its own A/B. The
revert drops recorded change 0009 (`scripts/vendor.py`, then `check hyper-raft`) and the two `v3_` tests that rest
on it. The term-0 guard (V6) lives in 0009 and goes with it.

## The bench's ports below the ephemeral range (c8ff608f)

rqlite listened on 54200 to 54212, libsql on 54220 to 54230, dqlite on 54240 to 54252, litefs and its Consul on 54260
to 54283, and the ladder's Amphora members on 54290 to 54292. All of them sit inside adacachy2's
`ip_local_port_range` (32768 to 60999), where an outbound connection's local port can take a node's address. Every
default moves to 29200 to 29292, below Linux's range and macOS's and Windows' (49152 to 65535). Lane S4's
`families.sh` had already moved Amphora's members to 29290. `bench/servers/repl/lib.sh` states the rule.

Smoke run r1337 (one run each, the 2431406e binary, family a's flags, 5 s windows): dqlite 209.2 commits/s at 1 writer
(p50 4.6 ms, p99 8.0 ms) and 170.0 at 32 (p50 130 ms, p99 860 ms). rqlite 26.2 at 1 (p50 37.4 ms) and 280.0 at 32
(p50 90.1 ms). One run each, so these show dqlite starts. They are not rates to compare.

## Unit 2, designed for the successor: A (prepare at the follower) and B (leader-executed), behind one switch

The shared core comes first, because both designs need it: **the leader's batch.** `Actor::pending: Option<Pending>`
holds one write today. It becomes one entry that carries several writes:

    struct Pending { context, term, leader, namespace, phase, writes: Vec<Write> }   // the entry's own fields
    struct Write { until: Instant, author: Author, response: Option<Response<Answered>>, value: Option<Answer>, _reservation: Reservation }

In `prepare_barrier` (replicated.rs about 5224), once the head write passes its barrier, take from `waiting` every
`Input::Submit` of the same namespace with an `Operation::Call` whose deadline hasn't passed and whose claim passes.
Keep taking while the entry stays within `Actor::entry_room` (about 5107), the bound the entry already obeys. Run
each one in the one `prepare_db` under `SAVEPOINT w<n>`: `RELEASE` on success, `ROLLBACK TO` and `RELEASE` on its own
error, so a failing write refuses only itself. The batch is what queued during the last entry's flush. There is no
timer and no count of its own, and one writer stays one write an entry. On apply (`answer_committed`, about 4794, and
the apply answer) each write gets its own value and the entry's token. No answer leaves before its entry applies.

**The switch** is one field, `FollowerWrites { Prepare, Forward }`, on the member's replica configuration, defaulting
to `Prepare` (today). It is per member, not per group. Every leader serves both forms, so a group can run mixed while
it upgrades, as long as a member asks for `Forward` only of a leader whose version reads it. It goes away, with the
losing path, once the ladder decides.

**A, `Prepare`: today's path, plus batching on both sides.** A follower still crosses its barrier, prepares at its
log tail and forwards the pages, and the leader still validates by exact base or read set. Same-page writes like the
bench's can't join the leader's batch, because their pages were computed on an older base. So under A "a
follower's write joins the leader's batch" means the follower batches its own queue the same way, into one forwarded
preparation, prepared in its write slot at its log tail (the slot's rule makes that base current). A follower can't
retry a refused write itself: preparation consumes the `FnOnce` closure ("SQL session already executed"). So for A
to pass test 5, a follower with writes queued asks for its slot before its first try, not after a refusal as today.
A cycle then carries every queued write in about three entries: one for each follower's slot, plus the leader's own
batch in its turn.

**B, `Forward`: the leader executes.** A follower doesn't prepare and doesn't cross a barrier. It forwards the request.
The leader runs it in its single writer's order as one more write in its batch, so no request ever has a stale base.
Two facts shape it:

- **A closure can't cross the wire.** `Operation::Call` holds `Box<dyn FnOnce(&mut Db)>`. B needs a request stated as
  data: `Request::Statements(Vec<(String, Vec<Value>)>)`, through a new `Database::execute(until, statements)`. A
  named procedure that every member registers, `Request::Procedure { name, args }`, could follow later. A
  `call` closure at a follower keeps A's path under either switch. The bench member's `exec` op already holds its
  statements as data.
- **Peer bytes are hyper-raft `Message`s** (`Actor::step`, about 4149: `Message::decode`). There is no Amphora envelope
  to add a variant to. So the request rides a forwarded proposal, whose entry data is a new appended `Request`
  form. The leader's `step_forward` (about 4254), which already intercepts forwards for the slot, runs it as a local
  write instead of appending it. The answer rides the log: the leader's batch entry carries, as a postcard trailer
  after its `Prepared` as the read sets do, each forwarded write's context and outcome (`Ok(value)` or its refusal).
  A follower answers its caller when it applies that entry. If the lead moves before then, it answers
  `UnknownOutcome`, as a forward that meets an election does today. At its own deadline it answers `Timeout`. The
  leader gets the caller's remaining time, not an `Instant`, since clocks differ across machines.

What each costs, before the data (inferred from the code, not measured): A spends a ReadIndex round, the follower's
preparation and a forward a write, and a slot whenever the leader's log moved. B spends a forward and the leader's
CPU for every write, since the leader prepares them all. V measured that at 1.27 ms of the leader's busiest thread
for each entry, at one write an entry.

**Tests, behaviour only, each run under both switches:**
1. two writes queued behind a barrier commit as one entry, and each gets its own answer;
2. one of them failing refuses only itself, and the other applies;
3. a batch spanning a leader change answers each of its writes `UnknownOutcome` or requeues it, as one write does today;
4. a deadline passing mid-batch times out only that write;
5. at 128 writers spread over three members, every write is answered before its deadline, none is refused for a
   stale base, and the run ends (no livelock). Today that run fails (V6's r1238 and this hour's 32-writer spread runs).

Test 5 is built and red, on `perf/commits-v7` at `bd3f6153` (not merged, since it fails by design):
`v7_a_hundred_and_twenty_eight_spread_writers_are_each_answered_with_no_stale_base` (replicated.rs, beside
`a_hundred_and_twenty_eight_writers_each_land_and_every_acknowledged_row_is_held`, on driven time). Each class runs
128 writers, writer w at member w % 3, all into one table. A stale-base refusal is retried at the same member, as the
bench's driver does, and counted. The test asserts at the end that no class refused one. Run at 18:30Z on
adacachy2 (cpus=4): red, with 52 stale-base refusals over 50 writes (at most 2 for one) in the persisted class, and 86 over
45 writes (at most 6 for one) in the replicated class. Every write still landed, and the existing 128-writer test passed
beside it. clippy `-p amphora --all-targets` is clean. On driven time no write reached its deadline, so the red part is
the refusals, not the bench's timeouts. The successor makes it green under each switch.

## Unit 3: the ladder (not run)

Family a (`families.sh` fam_a on adacachy2): 1, 32 and 128 writers, persisted and replicated, A against B against
main, with rqlite and dqlite on the new ports. 5 rounds with the order reversed each round, load and pressure beside
each, needs cpus=16. Report p50, p99, p99.9 and commits/s. Keep the winner and delete the other path with its switch.

## Commands

    # on adacachy2, kiln run on 00aad8daff2fa3c9daa65d53c425d450, needs linux/amd64,cpus=8,mem=8g, sync false
    B=~/.cache/amphora-perf-bin/commits    # 924fbe5c before 0009, 2431406e with it
    python3 -I bench/thread_sample.py $B/<arm> 29490 DIR 32 spread|leader 8 3 5 replicated   # r1312, 3 rounds, order reversed
    python3 -I bench/thread_sample.py $B/<arm> 29490 DIR 16|32 spread 8 3 5 replicated       # r1346
    . ~/.cache/amphora-perf/bench-s2/env.sh; cd ~/.cache/amphora-perf/commits-remote   # at c8ff608f
    $B/2431406e --repl commit --system dqlite|rqlite --scripts bench/servers/repl --record R --batch 1 --text-bytes 600 \
      --deadline-s 10 --relay-records 128 --spanning 2 --warmup-s 1 --seconds 5 --writers 1|32   # r1337

# Lane V8 (commits) on adacachy2: a follower's write takes its slot from its first try (2026-11-23, 18:47Z to 19:42Z)

Ada's goal is replicated commits about 10x faster than rqlite and dqlite, never refusing writes under load. At
19:16Z the coordinator set rule 0: this lane's one deliverable is test 5
(`v7_a_hundred_and_twenty_eight_spread_writers_are_each_answered_with_no_stale_base`) passing on main in both
classes with no stale-base refusals. Measurement stopped at that order.

## Losses first

1. **Only test 5 and the 18 tests named `slot` ran, not the module's suite.** Every follower barrier now takes a
   slot, so other `database::replicated::` tests may move. Green at 6f072bef, 9521643c and 9c0ece85 (main): 19 of 19,
   clippy `-p amphora --all-targets` clean. r1460 at 8600350e failed one: the slot's control test expected the old
   first-try refusal, and now expects each try applied (6f072bef).
2. **0009's split was measured in part and decided nothing.** Cell 1 ran (below) and is inside its noise. Cell 3's
   point reads all failed with "no such table: t", so it measured nothing (the driver's table is not where the
   member's `get` read looks; not traced). Cell 2 (20 MiB writes, beats on and off) never ran. Rule 0 parks all of it.
3. **The batching core and path B are not built.** Under rule 0 they wait unless test 5 needs them.

## The change (8600350e): slot first, not after a refusal

A follower's barrier answered with the leader's commit. The follower then prepared at its log tail while the
leader's own entries were in flight, and the stale-base rule refused the write at apply. Only then did the leader
queue the follower for a write slot (`Slots::refused`). Now the leader queues every follower's barrier
(`Slots::ask`), except the holder's second barrier, which still ends its slot. A slot's barrier steps once the
leader's commit reaches its last entry, and the leader appends nothing of its own while a slot is held, so the base
a follower prepares on is current when its write appends. A follower whose work forwarded nothing always sends the
ending barrier, so the per-refusal `slotted` term is deleted. Nothing new on the wire.

What it costs (inferred from the code, not measured): every follower operation, reads through `Database::query`
included, waits its turn for a slot. Followers' writes serialize with the leader's turns. That is rule 0's order:
working first, fast after.

## 0009's reads half, cell 1 (r1440, before rule 0)

One A/B binary (`perf/commits-v8-ab` 08812f1b, never merged): `AB_READS_OFF` turns off a new hyper-raft switch,
`Config::confirm_reads_now`, that gates `light_now`'s `ask_reads` and its read states. `bench/v8_ab.py` on that
branch: three members on btrfs, the commit driver at writer w to member w % 3, replicated class, 8 s windows,
3 rounds, arm order alternated. needs cpus=8; load 23.5 to 29.5, CPU "some" avg10 0.5 to 13.5%, I/O 2.7 to 6.6%.

| writers | reads half | commits/s, per round | median |
|---|---|---|---|
| 16 | on | 29.9, 11.9, 13.0 | 13.0 |
| 16 | off | 25.6, 15.1, 14.6 | 15.1 |
| 32 | on | 17.3, 11.9, 12.6 | 12.6 |
| 32 | off | 12.9, 13.9, 14.4 | 13.9 |

No measurable difference: each arm's range holds the other's median. No run aborted in either arm at 32, unlike
V7's 6 of 6 with 0009 (V7 sampled threads during its runs and ran an older main).

## Commands

    # r1440: kiln run on 00aad8daff2fa3c9daa65d53c425d450, needs linux/amd64,cpus=8,mem=24g, sync false,
    # cwd ~/.cache/amphora-perf/commits-remote at 08812f1b, CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits
    cargo build --release --manifest-path bench/Cargo.toml     # 1 min 08 s; copied to ~/.cache/amphora-perf-bin/commits/08812f1b
    [AB_READS_OFF=1] python3 -I bench/v8_ab.py $B 29490 DIR 16|32 8 3 0 replicated
    # r1460 at 8600350e, same needs and folders
    cargo test -p amphora --lib --locked -- --test-threads=8 v7_a_hundred_and_twenty_eight_spread_writers slot
    cargo clippy -p amphora --all-targets --locked -- -D warnings

# Lane V9 (commits) on adacachy2: V8's slot change made robust, half done (2026-11-23, 20:02Z to 21:05Z)

Ada's goal is replicated commits about 10x faster than rqlite and dqlite, never refusing writes under load. This
lane's one deliverable: V8's slot change made robust on main, (1) the `database::replicated::` and `replication::`
suites passing as before V8, and (2) a follower's read taking no write slot.

## Losses first

1. **Part 2 is not built.** V8's reading is right (traced below): every follower barrier takes a slot, reads
   included. The fix is designed below, not coded, and its test is not written.
2. **Three tests still fail now and then after V8, never before it.** Over V8's two arms (9c0ece85 and a8f1d857,
   whose product code is the same), 3 runs each: `writers_through_five_leader_crashes_each_land_and_every_acknowledged_row_is_held`
   2 of 6 ("the writers all landed before the crashes ran", 3 crashes of 5), `seed_and_delta_hold_through_an_election_at_each_step`
   1 of 6, `a_read_on_a_wholly_copied_log_holds_passes_off_and_a_busy_pass_holds_reads` 1 of 6. eb365f3d: 0 of 6
   each. Not traced.
3. **Lane P12's six were not traced.** They passed 3 of 3 here on 9c0ece85 (r1500), so V8 alone doesn't fail them.
   P12's post-V8 binaries (e931b6dd, f5d1208b) also hold B11's 416fe658 (pages keyed by segment generation) and P12's
   own pages merge (a52a337a); its pre-V8 ones (10d19e4e, 11725d28) hold neither, so P12's A/B doesn't isolate V8.
   Of the six, only `seed_and_delta_hold_through_an_election_at_each_step` failed here, once (loss 2).
4. **The merge with origin/main went to main without a run on the merged tree** (the hour's end). Clippy and the
   five tests passed on a8f1d857 before it.
5. **The brief's base was read as the commit without V8.** 9c0ece85's first parent, 9521643c, already holds V8
   (8600350e). Main just before V8 is its second parent, eb365f3d: eb365f3d..9c0ece85 is V7's test 5 and V8's two
   commits.

## Part 1: the suites before and after V8 (r1500, r1545)

`database::replicated:: replication::`, 8 threads, debug test binaries built in turn in one target dir, arm order
reversed each round, needs cpus=8,mem=24g; load (1 min) 17 to 40, CPU "some" avg10 4 to 31%, I/O under 2%.

| arm | rounds | failures per round | time per round |
|---|---|---|---|
| eb365f3d, main before V8 | 6 (r1500, r1545) | 0, 0, 0, 0, 0, 0 (167 passed, 5 ignored) | 33 to 42 s |
| 9c0ece85, V8 on main | 3 (r1500) | 5, 5, 6 | 131 to 135 s |
| a8f1d857, the five tests' fix | 3 (r1545) | 0, 2, 1 | 35 to 39 s |

The five that failed 3 of 3 on 9c0ece85 build a state the slot now prevents, so each turns the slot off on the
leaders where it builds it (a8f1d857), and asserts what it did:

| test | failure on 9c0ece85 | why (V8's slot from the first try) |
|---|---|---|
| `a_write_at_a_follower_is_answered_on_its_groups_commit` | `[Ok([1]), Ok([2])]`, no refusal | the follower's write takes the slot, so no two writes share a base |
| `an_overlapping_write_is_refused_serialization_and_a_malformed_one_invalid` | "no append was refused Serialization" in 20 rounds | the same |
| `follower_origin_captures_once_and_reads_after_local_durable_apply` | `Option::unwrap()` on None | the second follower's barrier waits behind the first's slot, whose forward the test holds; driven time never expires the slot |
| `follower_origin_captures_hold_through_an_election_at_each_step` | the same | the same |
| `a_floor_session_waits_for_its_log_as_it_was_not_for_writes_behind_it` | "not reached in 60000 driven rounds" | the slot's barrier steps once the leader's commit reaches its last entry, which the test's losses never let commit |

**The 3x slowdown is those five tests, not the product.** a8f1d857's product code is 9c0ece85's; only the five
tests' bodies differ. Its suite runs in 35 to 39 s, eb365f3d's time, against 131 to 135 s at 9c0ece85. So the extra
time was the five tests spinning to their round budgets (60000 driven rounds) before failing.

## Part 2: V8's reading is right (traced, not built)

- At the leader, `Actor::step` (replicated.rs, the `MsgReadIndex` arm under "The write slot holds...") calls
  `self.slots.ask(from)` for every follower barrier, then `Slots::read`, which queues it behind the slot held.
- At a follower, `Input::Submit` sends a barrier for every operation: Records reads (`Get`, `Scan`, `Changes`,
  `Head`, `ImportProgress`) and `Database::query`, which on a replicated database runs as `Operation::Call`
  (`query` to `session` to `call_db_session`), as writable as `call`.
- After any operation that forwarded nothing, `prepare()` sends a second barrier to end the slot.

So a follower's read waits behind other members' slots, and holds the leader's writes for its own round trip.

**The fix, for the successor.** The barrier says whether a write follows; nothing new on the wire.
- Follower: an operation that writes nothing marks its barrier's context, the 32 bytes plus one. These are `Get`,
  `Scan`, `Changes`, `Head`, `ImportProgress`, and a new `Operation::Query` that `Database::query` sends on a
  replicated database, run with the connection query-only as `run_query` runs it locally (its documented
  contract). A read never proposes, so its context never becomes an entry's. Old leaders echo it unread.
- Leader: a marked barrier doesn't `ask`. A new `Slots::pass(member, at, message)` ends a slot the member holds
  (released), steps the member's older waiting barrier, and steps this one now. A queue entry left by `refused`
  waits for the member's next write.
- The ending barrier goes only after a write's barrier whose work forwarded nothing, and is marked.
- Design: replication.md's write slot says the slot is for "a member's writes"; its section needs V8's rule (every
  write's barrier asks) and this one (a read's never does).
- Test: member 3 holds the slot, its forward held (`begin_captures`); member 2 reads (`Head`, and a query). Expect
  member 2's read answered while `slots.holds()` and the capture is held, `driven.lapses` 0. Red on V8.

## Commands

    # kiln run on 00aad8daff2fa3c9daa65d53c425d450, needs linux/amd64,cpus=8,mem=24g, sync false
    # ~/.cache/commits-v9/suites.sh ROUNDS SHA...: cwd ~/.cache/amphora-perf/commits-remote, CARGO_TARGET_DIR=
    # ~/.cache/amphora-perf-target/commits, CARGO_INCREMENTAL=0, CARGO_BUILD_JOBS=8; binaries copied to
    # ~/.cache/amphora-perf-bin/commits/v9-tests-<sha>, logs ~/.cache/amphora-perf/commits-v9-logs/<time>/
    cargo test -p amphora --lib --locked --no-run
    v9-tests-<sha> --test-threads=8 database::replicated:: replication::      # r1500: eb365f3d 9c0ece85; r1545: eb365f3d a8f1d857
    ~/.cache/commits-v6/am.sh a8f1d857   # cargo clippy -p amphora --all-targets --locked -- -D warnings: clean (r1546)

# Lane V10 (commits) on adacachy2: one of V9's three failures traced and its test fixed (2026-11-23, 21:07Z to 22:01Z)

Ada's goal is replicated commits about 10x faster than rqlite and dqlite, never refusing writes under load. This
lane's deliverable: V9's, finished (the three V8-only failures fixed and 6 of 6, and part 2 built). An API outage
stopped the lane from about 21:30Z to 21:50Z.

## Losses first

1. **The deliverable is not finished.** One test's fix (81530482) is merged: 6 of 6 alone with 5 crashes each, clippy
   clean (r1602). The suite was not run on it against eb365f3d, 6 rounds; that is the successor's first run.
2. **Only `writers_through_five_leader_crashes_each_land_and_every_acknowledged_row_is_held` was traced.** The other
   two (`seed_and_delta_hold_through_an_election_at_each_step`, `a_read_on_a_wholly_copied_log_...`) did not fail in
   r1570's 4 rounds and were not traced.
3. **Part 2 is not started.** V9's design below it stands.

## r1570: the suite on main b85ca877 against eb365f3d (4 rounds, arm order reversed each round)

`database::replicated:: replication::`, 8 threads, needs cpus=8,mem=24g; load (1 min) 23.5 to 37.5, CPU "some"
avg10 2.2 to 6.1%, I/O under 1.6%.

| arm | failures per round | time per round |
|---|---|---|
| eb365f3d, main before V8 | 0, 0, 0, 0 (167 passed) | 33 to 36 s |
| b85ca877, main | 1, 0, 1, 1 (168 or 169 passed) | 31 to 33 s |

Each failure was the five-crash test, "the writers all landed before the crashes ran", 3 crashes of 5.

## r1591: the five-crash test traced, alone, 6 runs each arm, alternated

Diagnostic branches (not for main): `diag/commits-v10-a-eb` (f5239515, eb365f3d plus the trace) and
`diag/commits-v10-a-main` (5174b409, b85ca877 plus the trace). The trace prints each sweep's acknowledgments and
resends (with the refusal), the driven clock, and each crash's round and count.

| arm | result | crashes at (round: acknowledged) | resends: stale base, unknown outcome |
|---|---|---|---|
| eb365f3d | 6 of 6 pass | r43-56: 6; then 15-17, 22-25, 24-26, 27 | 13 to 15, 1 to 2 |
| b85ca877 (V8) | 6 of 6 fail | r38-42: 6; then 24; then 32 (3 crashes) | 0, 0 |

The first difference is the sweep after crash 1. The test crashes the leader once a sixth of the writers more has
been acknowledged, and checks once a sweep. The crash's restart and election run `Driven::tick` and `quiet`, which
settle the group: every write that can land without a clock moving lands there. Before V8, 13 to 15 follower writes
were refused as stale and sent again only at the next sweep, so acknowledgments came a few per sweep and five
crashes ran with writes outstanding. Under V8's slot no write is refused, so 18 writers land in crash 1's settle and
the other 8 in crash 2's. Nothing is lost or refused: the product does what V8 promises ("a follower's write
prepares in a slot from its first try", replication.md's write slot, CP-T1.8 lane V8). What broke is the test's
pacing, which relied on refusals to spread the acknowledgments.

**The fix (81530482, test only, citing V8's design).** Writes stay outstanding through all five crashes: each
writer, once acknowledged and while crashes remain, writes its next row (id `writer + WRITERS * k`, `INSERT OR
IGNORE` as before); every replica must hold exactly the rows written, once. r1602, the test alone at 81530482:
6 of 6 pass, 5 crashes each, 21 to 65 sent again, 171 to 421 rounds; `cargo clippy -p amphora --all-targets
--locked -- -D warnings` clean. Next: the suite 6 rounds against eb365f3d (`suites.sh`), and the other two tests
traced the same way if they fail.

## Commands

    # kiln run on 00aad8daff2fa3c9daa65d53c425d450, needs linux/amd64,cpus=8,mem=24g, sync false
    ~/.cache/commits-v10/suites.sh 4 b85ca877 eb365f3d          # r1570 (V9's suites.sh, logs ~/.cache/amphora-perf/commits-v10-logs/211521)
    # r1591: built f5239515 and 5174b409 in turn (one target dir), each run 6 times alternated:
    v10-diag-<sha> --exact database::replicated::tests::writers_through_five_leader_crashes_each_land_and_every_acknowledged_row_is_held --nocapture
    # logs ~/.cache/amphora-perf/commits-v10-logs/diag-a-213229; the patch: ~/.cache/amphora-perf/commits-v10-scripts/diag_a.py (on adacachy)

# Lane V11 (commits) on adacachy2: part 2, a read's barrier asks for no write slot (2026-11-23, 22:03Z to 22:58Z)

Ada's goal is replicated commits about 10x faster than rqlite and dqlite, never refusing writes under load. This
lane's deliverable: V9's part 2, merged to main. A replicated `Database::query` refuses writing statements, and a
read's barrier never asks for a write slot.

## Losses first

1. **No red run of the new test.** Its doc says lane V8's rule held member 2's reads behind member 3's slot; that
   is read from the code (V9's trace), not run. Branch `diag/commits-v11-red` (168dc72d, the leader asking a slot
   for every barrier again) is pushed for that run and was not run.
2. **The suite ran on ad4c0922, not the final head.** The head adds the review's bootstrap fix (f05b2c05) and
   main's merges (engine, store flush W3). Clippy and the focused tests ran on each merged tree (below).
3. **Mixed versions, not handled (the design's, noted):** an old leader takes a new follower's marked read
   barrier as a write's and holds a slot for it, and the new follower's read sends no second barrier, so that
   slot ends by its length. The leader's own reads still wait while any slot is held (`slot_deferred`).

## The change (ad4c0922, f05b2c05)

V9's design, as written. `Operation::Query` runs a replicated query past its barrier with `Db::query_only`.
`Operation::writes` classifies each operation; work that writes nothing marks its barrier's context with
`READ_BARRIER`, one byte past the 32-byte identity. The leader's `MsgReadIndex` arm steps a marked barrier at once
(`Slots::pass`); an unmarked one asks as V8 made it. The second barrier after a write that forwarded nothing is
marked; a read sends none. Review (one subagent, source only) found one bug: at the source-leader, a read submitted
while a bootstrap is pending proposed the seed with its marked context; f05b2c05 truncates it to the identity.

## Runs

| run | tree | what | result |
|---|---|---|---|
| r1619 | main 98bd0fa9 vs eb365f3d | `suites.sh 6` | 98bd0fa9: 2 failures in 6 rounds (`seed_and_delta_hold_through_an_election_at_each_step` r1, `session_reads::t_t1_10_r1_a_session_reads_its_write_on_a_lagging_follower` r6); eb365f3d: 0 |
| r1658 | ad4c0922 | focused tests | 21 of 21 |
| r1660 | ad4c0922 vs eb365f3d | `suites.sh 6` | ad4c0922: 0 failures in 6 rounds (171 passed); eb365f3d: 0 (167 passed); 35 to 57 s a round, load 21 to 77 |
| r1661 | 3d9c41b9 (merged main) | clippy, focused tests | clean; 21 of 21 |
| r1670 | df199602 (merged main) | clippy, focused tests | clean; 21 of 21 |

Focused tests: `database::slot::`, the two new tests, and every test named `slot`.

## Commands

    # kiln run on 00aad8daff2fa3c9daa65d53c425d450, needs linux/amd64,cpus=8,mem=24g, sync false,
    # cwd ~/.cache/amphora-perf/commits-remote at the SHA, CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits
    ~/.cache/commits-v10/suites.sh 6 <sha> eb365f3d       # r1619, r1660
    cargo clippy -p amphora --all-targets --locked -- -D warnings
    cargo test -p amphora --lib --locked --no-run
    v9-tests-<sha> --test-threads=8 database::slot:: a_follower_read_completes_while_another_member_holds_the_write_slot \
      a_replicated_query_that_writes_is_refused_and_writes_nothing slot

# Lane V12 (commits) on adacachy2: where a replicated commit's time goes on main dd9c3ce7 (2026-11-23, 23:05Z to 00:00Z)

Ada's goal is replicated commits about 10x faster than rqlite and dqlite, never refusing writes under load. This
lane's deliverable is a measurement, with nothing fixed: (1) the ladder on current main against rqlite and dqlite on
one machine, and (2) a per-stage trace of one commit at 1 writer and of the writes at 32 spread writers, naming the
top limiter at each from the breakdown.

## Losses first

1. **The apply stage isn't split.** It is 11.2 ms, 47% of a 1-writer persisted commit. The shim's records carry no
   file for a write or a flush (it names a file only on `open`), and no fdatasync was attributed to a stage by its
   thread and time. The raw records (`stages-m*.log`, `shim-m*.rec` with thread ids) are on adacachy2 under
   `~/.cache/amphora-perf/commits-v12-out/trace-233603/` for the next lane.
2. **One traced run a cell, not alternated rounds,** and the traced members ran under the shim and the stage log.
   Their rates sit inside or just under the ladder's rounds: persisted 1 writer leader-routed 41.4 (ladder 44.0 to
   50.0), spread 38.2 (34.8 to 47.0), 32 spread 35.8 (14.2 to 38.2). The traced 1-writer leader-routed p50 is 24.1 ms
   against the ladder's 18.7 to 21.0 ms, so the tracing costs some of each commit (not split).
3. **128 writers were not traced** (the brief traces 1 and 32), so the limiter there is inferred from the ladder.
4. **A shared machine.** Load (1 min) 24.6 to 54.0 over the ladder, CPU "some" up to 41.3%. Competitor rounds spread
   up to 2.6x (dqlite at 1 writer leader-only, 178.2 to 467.8). At 1 writer spread, Amphora against rqlite is inside
   that noise.
5. **Leader-only isn't strictly leader-only.** A write the leader fails for any reason but a stale base moves to the
   next node, as spread writes do. In one Amphora run (replicated, 128 leader) 14 of 221 writes were acknowledged by
   a follower.
6. **Stale-base refusals remain on main** at 32 and 128 spread writers (9 to 41 a run, both classes). Not traced.
7. **The replicated class's slow follower path at 1 writer** (slot wait 7.9 ms, forward 10.0 ms) is measured, not
   explained.

## The ladder (r1716): main dd9c3ce7 against rqlite and dqlite

`ladder.sh` (this folder) at `dcdf192d` (main dd9c3ce7 plus the driver's `--route`, below). One host, adacachy2 (32
cores, btrfs `~/.cache`), every system's three nodes as processes on 127.0.0.1, S6's flags (`--batch 1 --text-bytes
600 --deadline-s 10 --relay-records 128 --spanning 2 --warmup-s 1 --seconds 5`). 1, 32 and 128 writers, "spread"
(writer w at node w % 3, the ladder's routing) and "leader" (every writer at the node the cluster names its leader
once the schema is in), 3 rounds with the cell order reversed in round 2. Amphora's two classes: persisted
(acknowledged once a quorum's disks hold it, as rqlite's and dqlite's are) and replicated (`--spanning 2`). rqlite
and dqlite have one class each. needs cpus=16; each cell's load (1 min), CPU "some" and I/O "some" avg10 at its
start are in the last column, one triple a round. Stale-base refusals are the driver's `Serialization` count, each
retried at the same node; a deadline miss is a write not acknowledged within 10 s, sent again.

r1716 ran 23:22:24Z to 23:32:41Z: 72 cells, each with a record, none refused as a whole, every driver exit 0.
Load (1 min) 24.6 to 54.0 on 32 cores, CPU "some" avg10 3.2 to 41.3%, I/O "some" 1.6 to 11.1%. Other lanes ran
beside it.

Medians of the 3 rounds, commits/s. The ratios are against Amphora's persisted class, the class whose acknowledgement
matches rqlite's and dqlite's. "Goal" is the factor Amphora's persisted class must gain to be 10x the faster competitor
in that cell.

| writers | route | Amphora persisted | Amphora replicated | rqlite | dqlite | rqlite / Amphora | dqlite / Amphora | goal |
|---|---|---|---|---|---|---|---|---|
| 1 | spread | 41.2 | 27.4 | 62.6 | 241.0 | 1.5x | 5.8x | 58x |
| 1 | leader | 46.0 | 46.2 | 66.8 | 214.4 | 1.5x | 4.7x | 47x |
| 32 | spread | 31.8 | 30.4 | 623.4 | 183.2 | 19.6x | 5.8x | 196x |
| 32 | leader | 36.8 | 45.4 | 668.0 | 293.4 | 18.2x | 8.0x | 182x |
| 128 | spread | 34.4 | 38.6 | 2,528.4 | 291.4 | 73.5x | 8.5x | 735x |
| 128 | leader | 46.4 | 46.2 | 2,386.2 | 233.0 | 51.4x | 5.0x | 514x |

Every cell, each round's value. Latencies are the driver's, from a write's first send to its acknowledgement. The last
column is load (1 min) / CPU "some" avg10 % / I/O "some" avg10 % at the cell's start, one triple a round.

| system | writers | route | commits/s per round | median | p50 ms per round | p99 ms per round | stale-base refusals | deadline misses | run refused | load1 / cpu some / io some per round |
|---|---|---|---|---|---|---|---|---|---|---|
| Amphora persisted | 1 | spread | 34.8, 47.0, 41.2 | 41.2 | 27.9, 20.5, 22.5 | 48.6, 39.4, 46.6 | 0, 0, 0 | 0, 0, 0 | none | 29.02/13.31/5.22; 31.65/21.09/2.47; 30.95/13.35/2.58 |
| Amphora persisted | 1 | leader | 44.0, 50.0, 46.0 | 46.0 | 21.0, 18.7, 18.9 | 63.5, 28.7, 44.0 | 0, 0, 0 | 0, 0, 0 | none | 27.35/5.79/1.74; 24.91/3.62/2.51; 30.84/9.65/5.55 |
| Amphora persisted | 32 | spread | 38.2, 31.8, 14.2 | 31.8 | 557.1, 786.4, 1310.7 | 2621.4, 3342.3, 4718.6 | 41, 27, 9 | 0, 0, 0 | none | 31.84/20.08/2.19; 26.82/5.30/3.09; 29.85/5.20/7.33 |
| Amphora persisted | 32 | leader | 36.8, 40.2, 10.2 | 36.8 | 868.4, 778.2, 2359.3 | 1114.1, 901.1, 3768.3 | 0, 0, 0 | 0, 0, 0 | none | 36.15/18.35/2.22; 30.92/5.58/3.01; 25.90/13.06/2.73 |
| Amphora persisted | 128 | spread | 30.4, 46.4, 34.4 | 34.4 | 2359.3, 1622.0, 2588.7 | 7405.6, 5898.2, 7274.5 | 26, 40, 31 | 0, 0, 0 | none | 43.96/41.25/2.91; 34.38/7.04/4.03; 25.48/13.29/5.43 |
| Amphora persisted | 128 | leader | 46.4, 51.4, 31.0 | 46.4 | 2752.5, 2490.4, 4128.8 | 2883.6, 2588.7, 4587.5 | 0, 0, 0 | 0, 0, 0 | none | 47.73/20.79/3.36; 40.42/8.30/3.94; 29.98/19.70/5.74 |
| Amphora replicated | 1 | spread | 27.4, 32.2, 25.8 | 27.4 | 35.8, 30.0, 35.3 | 51.2, 48.1, 88.1 | 0, 0, 0 | 0, 0, 0 | none | 30.54/15.37/4.09; 33.84/34.63/2.25; 30.32/12.40/2.48 |
| Amphora replicated | 1 | leader | 45.6, 50.6, 46.2 | 46.2 | 16.6, 14.7, 15.5 | 78.8, 42.0, 200.7 | 0, 0, 0 | 0, 0, 0 | none | 27.70/7.08/2.19; 24.56/3.20/2.33; 30.21/8.84/4.52 |
| Amphora replicated | 32 | spread | 34.4, 30.4, 25.0 | 30.4 | 475.1, 512.0, 1228.8 | 3276.8, 3538.9, 4194.3 | 38, 38, 16 | 0, 0, 11 | none | 32.73/19.39/1.98; 27.32/6.67/2.37; 28.04/3.43/11.14 |
| Amphora replicated | 32 | leader | 47.8, 45.4, 26.2 | 45.4 | 679.9, 679.9, 827.4 | 778.2, 794.6, 1671.2 | 0, 0, 0 | 0, 0, 0 | none | 37.20/19.48/4.05; 32.74/7.25/2.80; 25.90/9.72/3.58 |
| Amphora replicated | 128 | spread | 38.6, 35.8, 43.4 | 38.6 | 1753.1, 1851.4, 1753.1 | 6815.7, 7405.6, 6357.0 | 29, 30, 41 | 0, 0, 0 | none | 47.44/32.83/6.37; 35.97/11.33/5.56; 24.85/10.21/5.17 |
| Amphora replicated | 128 | leader | 52.4, 46.2, 44.2 | 46.2 | 1884.2, 2457.6, 2818.0 | 3080.2, 3014.7, 3014.7 | 2, 2, 4 | 0, 0, 0 | none | 46.46/15.41/3.97; 43.19/9.03/3.87; 34.31/25.22/5.32 |
| dqlite | 1 | spread | 287.2, 240.6, 241.0 | 241.0 | 3.2, 3.9, 3.8 | 6.8, 10.6, 10.2 | 0, 0, 0 | 0, 0, 0 | none | 28.08/6.82/2.86; 26.43/4.09/2.61; 33.16/14.91/2.28 |
| dqlite | 1 | leader | 178.2, 214.4, 467.8 | 214.4 | 5.2, 4.4, 1.9 | 10.9, 7.2, 4.8 | 0, 0, 0 | 0, 0, 0 | none | 30.14/14.75/4.25; 26.84/4.70/3.55; 30.19/6.64/7.15 |
| dqlite | 32 | spread | 160.2, 183.2, 424.2 | 183.2 | 133.1, 128.0, 52.2 | 827.4, 720.9, 360.4 | 0, 0, 0 | 0, 0, 0 | none | 36.36/12.29/2.77; 30.36/5.20/3.33; 25.66/7.78/5.67 |
| dqlite | 32 | leader | 164.8, 293.4, 410.0 | 293.4 | 131.1, 73.7, 55.8 | 925.7, 499.7, 331.8 | 0, 0, 0 | 0, 0, 0 | none | 43.05/39.34/4.77; 32.93/5.73/3.24; 25.25/7.97/5.60 |
| dqlite | 128 | spread | 224.6, 291.4, 340.4 | 291.4 | 344.1, 303.1, 260.1 | 2293.8, 1835.0, 1671.2 | 0, 0, 0 | 0, 0, 0 | none | 49.82/24.50/5.56; 38.74/6.69/3.56; 29.65/12.07/4.89 |
| dqlite | 128 | leader | 233.0, 201.8, 337.2 | 233.0 | 385.0, 413.7, 266.2 | 2162.7, 2654.2, 1654.8 | 0, 0, 0 | 0, 0, 0 | none | 45.96/13.97/5.63; 44.53/12.35/2.70; 34.52/16.15/3.26 |
| rqlite | 1 | spread | 62.6, 43.0, 70.8 | 62.6 | 14.6, 21.5, 13.1 | 54.3, 52.7, 29.7 | 0, 0, 0 | 0, 0, 0 | none | 29.54/12.94/3.60; 29.82/12.30/1.69; 34.82/28.07/2.14 |
| rqlite | 1 | leader | 51.2, 66.8, 72.2 | 66.8 | 17.4, 13.4, 12.8 | 59.4, 63.0, 23.8 | 0, 0, 0 | 0, 0, 0 | none | 27.73/11.04/4.73; 25.04/3.29/1.92; 30.73/8.52/6.98 |
| rqlite | 32 | spread | 647.8, 623.4, 403.2 | 623.4 | 45.1, 46.1, 41.0 | 126.0, 172.0, 933.9 | 0, 0, 0 | 0, 0, 0 | none | 37.70/14.69/2.38; 30.00/10.23/1.85; 26.53/6.64/4.72 |
| rqlite | 32 | leader | 391.0, 668.0, 708.8 | 668.0 | 59.4, 47.1, 38.9 | 974.8, 67.6, 231.4 | 0, 0, 0 | 0, 0, 0 | none | 39.43/32.07/3.25; 34.19/10.36/1.59; 25.18/8.94/5.14 |
| rqlite | 128 | spread | 2528.4, 2933.6, 2347.4 | 2528.4 | 42.5, 37.9, 48.1 | 256.0, 186.4, 161.8 | 0, 0, 0 | 0, 0, 0 | none | 53.98/38.52/3.93; 38.12/13.11/2.47; 29.54/11.40/6.72 |
| rqlite | 128 | leader | 2386.2, 2014.0, 2662.2 | 2386.2 | 44.5, 60.4, 44.5 | 516.1, 182.3, 89.1 | 0, 0, 0 | 0, 0, 0 | none | 48.59/16.14/6.54; 44.95/13.39/1.88; 34.87/20.96/3.65 |

What the numbers say:

- **Amphora doesn't grow with writers.** Both classes' medians sit at 27.4 to 46.4 commits/s in every cell, 1 to 128
  writers, either route (rounds 10.2 to 52.4; the two lowest are the persisted class's third round at 32 writers). rqlite grows 40x from 1 to 128 writers (62.6 to 2,528.4), and dqlite about 1.2x.
- **At 1 writer the gap is smallest, and it is still a loss.** rqlite is 1.5x faster on both routes. Leader-routed, no
  round overlaps: Amphora 44.0 to 50.0, rqlite 51.2 to 72.2. Spread, the ranges overlap (Amphora's 47.0 above
  rqlite's 43.0), so that cell's 1.5x is inside 3 rounds' noise. dqlite is 4.7x to 5.8x faster, with no overlap.
- **From 32 writers on, every round of every competitor is above every round of Amphora** (Amphora's best 52.4,
  rqlite's worst 391.0 and dqlite's worst 160.2).
- **Writes are still refused for a stale base on main, spread over the three members.** Each run at 32 and 128 spread
  writers had 9 to 41 `Serialization` refusals in both classes, and the replicated class 2 to 4 at 128 leader-only.
  The driver resends each refusal at the same node, so every write landed, but 11 writes in one replicated run at 32
  spread missed their 10 s deadline before landing. Leader-only, the persisted class had none. V8's test 5 has no
  stale-base refusal on driven time; these members on real time still have them (not traced, one line for the
  successor).
- **Against S6 (cfa0107c, the same flags, load 23 to 37).** S6 had Amphora persisted at 6.8, 6.4 and 10.8 commits/s
  (spread, 1, 32 and 128) against today's 41.2, 31.8 and 34.4, and refusals at 32 and 128 there too. The competitors'
  own rates moved as much between the runs (rqlite at 128: 452 then 2,528), so only ratios within one run compare:
  rqlite over Amphora 4.4x, 104x and 42x in S6, 1.5x, 19.6x and 73.5x now.

## One commit's stages (r1728, branch `diag/commits-v12-stages`, never for main)

`diag/commits-v12-stages` (d1c80fb0) adds an env-gated stage log to `replicated.rs`: with `AMPHORA_STAGES` set, the
owner writes one line an event with std's `Instant` as CLOCK_MONOTONIC nanoseconds (the shim's clock): a write's
arrival (`submit`), its barrier sent, prepare's start and end, its proposal, each drive's log `last`, `persisted`,
`committed` and `applied` index when one moves, each applied entry's context and index, and the answer. Lane W2's
LD_PRELOAD shim (`2026-11-23-adacachy2-perf-oltp/stages.c`) timestamps each log write and flush syscall. Members are
started by `trace.py` (port 29490), the driver runs untraced, S6's flags, 5 s after a 1 s warmup; `stages_table.py`
joins each write's events across the three members by its context and its entry's index, and drops the warmup.

How each stage is measured, per write:

| stage | from | to |
|---|---|---|
| request queue | the owner takes the write (`submit`) | its barrier is sent |
| slot wait | barrier sent | prepare starts (the barrier's read index answered, the slot held, the tail applied) |
| prepare | `prepare_db` starts | it returns |
| propose | prepare's end | `propose` returned |
| to the leader's log | proposed | the leader's drive first shows `last` at or past the entry (a follower's forward hop) |
| leader append + fsync | the leader's log holds it | the leader's `persisted` covers it |
| follower append | the leader's log holds it | the quorum follower's log holds it (the follower whose `persisted` covered it first) |
| follower fsync | that follower's log holds it | its `persisted` covers it |
| commit | the leader's log holds it | the leader's `committed` covers it |
| commit known at the origin | the leader's commit | the origin member's `committed` covers it |
| apply | the origin knows it committed | the origin's `applied` covers it |
| answer | the origin applied it | the answer is sent (a follower answers on the commit when `fates` is set) |

Each "first shows" is the drive that saw it, so a stage includes the wait for the owner's next drive.

r1758's traced runs ran 23:39:57Z to about 23:40:40Z (load 1 min 26.0 to 29.4, CPU "some" 5.0 to 8.5%, I/O "some" 3.6 to 5.1%), one run a
cell, members on btrfs. Its own tables were printed by the diag branch's first `stages_table.py`; the numbers below
are `stages_table.py` at b67ea4df (two fixes: Rust's `true`, and the warmup cut at the run's first write) on the same
logs. Medians in ms; each run's p10 and p90 are in r1758's output.

**Persisted class** (acknowledged once a quorum's disks hold it; rqlite's and dqlite's class):

| stage | 1 writer, leader (n=210) | 1 writer, spread: a follower's write (n=193) | 32 spread, all (n=185) | 32 spread, the leader's own (n=97) | 32 spread, followers' (n=88) |
|---|---|---|---|---|---|
| request queue | 0.008 | 0.006 | **489.0** | 451.7 | 847.7 |
| slot wait | 0.128 | 0.183 | 32.5 | 25.3 | 44.6 |
| prepare | 0.220 | 0.222 | 0.205 | 0.212 | 0.199 |
| propose | 0.021 | 0.014 | 0.019 | 0.022 | 0.014 |
| to the leader's log | 0.018 | 1.384 | 0.105 | 0.043 | 0.243 |
| leader append + fsync | 11.92 | 11.48 | 12.08 | 11.92 | 12.15 |
| follower append | 0.061 | 0.062 | 0.067 | 0.069 | 0.065 |
| follower fsync | 12.01 | 11.12 | 11.76 | 11.85 | 11.23 |
| **commit** (log holds it to the leader's commit) | **12.24** | **11.52** | 12.16 | 12.12 | 12.18 |
| commit known at the origin | 0 | 0.058 | 0 | 0 | 0.061 |
| **apply** | **11.20** | **11.34** | 11.31 | 11.09 | 11.39 |
| answer | 0.159 | 0.145 | 0.159 | 0.159 | 0.157 |
| total in the member | 24.06 | 25.14 | 535.0 | 501.6 | 925.6 |
| the run's driver p50, commits/s | 24.06, 41.4 | 25.09, 38.2 | 565.2, 35.8 | | |

**Replicated class** (`--spanning 2`):

| stage | 1 writer, leader (n=215) | 1 writer, spread (n=138) | 32 spread, all (n=161) |
|---|---|---|---|
| request queue | 0.008 | 0.007 | 501.0 |
| slot wait | 0.119 | 7.931 | 41.1 |
| prepare | 0.211 | 0.259 | 0.213 |
| propose | 0.017 | 0.015 | 0.017 |
| to the leader's log | 0.028 | 9.988 | 0.540 |
| leader append + fsync | 12.80 | 10.98 | 11.91 |
| follower append | 6.874 | 0.075 | 0.154 |
| follower fsync | 19.44 | 10.62 | 15.72 |
| commit | 16.00 | 13.54 | 13.47 |
| commit known at the origin | 0 | 2.901 | 0 |
| apply | 0 | 0 | 0 |
| answer | 0.152 | 0.155 | 0.150 |
| total in the member | 16.97 | 35.21 | 559.4 |
| the run's driver p50, commits/s | 16.64, 42.4 | 34.82, 27.2 | 565.2, 30.6 |

**Syscalls (lane W2's shim, each member over the measured window).** In the persisted class the three members agree
within 10%. In the replicated class the leader flushes about half as often as its followers. The shim names a file only
on `open`, so writes and flushes are not split by file.

| syscall | calls a second, each member | median ms | p90 ms |
|---|---|---|---|
| fdatasync | 196 to 210 (persisted), 85 to 216 (replicated; the leader's are the lower) | 5.25 to 6.24 | 6.8 to 7.9 |
| fsync | 22 to 39 | 5.45 to 6.25 | 7.7 to 8.5 |
| pwrite | 309 to 590 | 0.003 to 0.004 | 0.007 to 0.011 |
| write | 103 to 449 | 0.002 to 0.004 | 0.004 to 0.006 |
| pwritev | 87 to 106 | 0.005 to 0.007 | 0.008 to 0.013 |

What the trace says:

- **The trace covers the commit.** At 1 writer the in-member total equals the driver's p50 within 0.1 ms (24.06 against
  24.06, 25.14 against 25.09). At 32 writers the driver's p50 is 30 ms above it (565 against 535): time before the
  owner takes the write, not split here.
- **Appends cost microseconds, flushes milliseconds.** pwrite takes 3 to 4 us. Each fdatasync takes 5.3 ms, and each
  member runs about 5.1 a commit (209.5 a second on the leader at 41.4 commits/s, 1 writer leader-routed).
- **The leader's and the follower's log flushes run side by side** (11.9 and 12.0 ms) and the commit waits for both
  (12.2 ms). The follower's append arrives 0.06 ms after the leader's.
- **The commit isn't the end.** The persisted class applies 11.2 ms after its commit, as long again. The replicated
  class applies in the same drive as its commit (0 ms), and its commit takes longer and spreads wider (16.0 ms,
  p10 2.9, p90 39.2).
- **Everything that isn't a flush or a queue is 0.55 ms** at 1 writer leader-routed (queue, barrier, prepare,
  propose, the hop, answer): 2.3% of the commit. A follower's write adds its forward, 1.4 ms in the persisted class.
- **In the replicated class a follower's write waits more before it reaches the leader** at 1 writer: slot wait 7.9 ms
  and forward 10.0 ms, against 0.18 and 1.4 ms in the persisted class. Not explained by this trace.
- A replicated follower answers on the commit, before its apply (`fates`), so "answer" can be negative at its p10.

## The top limiter at each writer count

- **1 writer: the commit's flushes, in two serial halves.** Of the 24.1 ms a persisted commit spends in the member
  (leader-routed), 12.2 ms (51%) is the commit: the leader's log flush and the quorum follower's, side by side. Then
  11.2 ms (47%) passes between the commit and the leader's apply. The other stages total 0.55 ms (2.3%). The CPU is
  idle meanwhile: these are waits. The commit's half is measured as flushes (appends take 4 us). What fills the
  apply's 11.2 ms is not split by this trace. Each member runs 5.1 fdatasyncs a commit at 5.3 ms, enough for two in
  each half, but no call was attributed to a stage (inferred, not traced). The top limiter at 1 writer is the commit
  stage, 12.2 ms of log flush, with the apply stage, 11.2 ms, second and within 1 ms of it.
- **32 spread writers: one write at a time.** 489 of 535 ms (91%) is the request queue: a write waits at its member
  before its barrier is sent, because the owner holds one write (`pending`) and queues the rest behind it. The slot
  wait is another 32.5 ms (6%). Each write's own service is the 1-writer commit and apply (12.2 + 11.3 ms), and the
  group commits 35.8 a second, one write each 28 ms. No two writes share an entry or a flush. The top limiter at 32
  writers is that serialization: the queue's length is the 24 ms of flushes a write times the writes ahead of it.
  A follower's write waits longer (848 ms against the leader's own 452 ms).
- **128 writers: not traced.** The ladder's flat 34 to 46 commits/s, with p50 1.6 to 4.1 s, is of the order of the same
  one-at-a-time service (128 writes at 24 to 28 ms each is 3.1 to 3.6 s). That is inferred from the ladder, not traced.

## Commands

    # r1716: kiln run on 00aad8daff2fa3c9daa65d53c425d450, needs linux/amd64,cpus=16,mem=24g, sync false,
    # cwd ~/.cache/amphora-perf/commits-remote at dcdf192d, CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits
    # (cold: the release build took 10 min 24 s), CARGO_INCREMENTAL=0, CARGO_BUILD_JOBS=8
    nice -n 10 bash bench/records/2026-11-23-adacachy2-perf-commits-v12/ladder.sh ~/.cache/amphora-perf/commits-v12-out/ladder-231200
    python3 -I bench/records/2026-11-23-adacachy2-perf-commits-v12/ladder_table.py ~/.cache/amphora-perf/commits-v12-out/ladder-231200
    # r1758: the same runner and folders at d1c80fb0 (diag/commits-v12-stages), needs linux/amd64,cpus=4,mem=12g
    # (cpus=8 refused twice: 3 and 4 of 32 cores idle), CARGO_BUILD_JOBS=4; the diag build took 3 min 54 s
    bash bench/records/2026-11-23-adacachy2-perf-commits-v12/trace.sh ~/.cache/amphora-perf/commits-v12-out/trace-233603
    python3 -I bench/records/2026-11-23-adacachy2-perf-commits-v12/stages_table.py ~/.cache/amphora-perf/commits-v12-out/trace-233603/<class>-<writers>-<route>
    # clippy on the bench crate (the one this lane changed: --route in bench/src/repl.rs), needs linux/amd64,cpus=4,mem=12g:
    # red on main dd9c3ce7 itself, 166 errors (disallowed println/args/temp_dir, items after a test module, ...); the
    # same 166 at b67ea4df and at the merged 2e662d8b, the 9 in repl.rs main's own, none in the lines changed (r1766, then two foreground runs whose ids were not shown)
    cargo clippy --release --manifest-path bench/Cargo.toml --all-targets --locked -- -D warnings

# Lane V13 (commits) on adacachy2: the stale-base refusals traced to their first wrong decisions, and fixed (2026-11-24, 00:02Z to 01:00Z)

Ada's goal is replicated commits about 10x faster than rqlite and dqlite, with no write refused under load. This
lane's deliverable: no write refused for a stale base at 32 and 128 spread writers in either class, traced first,
fixed where the first wrong decision is made, with a test that fails first, proved on the ladder and the replicated
suite, merged to main.

## Losses first

1. **The deliverable is met in the persisted class only.** After the fix, the replicated class still had 0 to 5
   stale-base refusals a run (r1896's ladder: 1 of 3 runs at 32 spread writers, 3 of 3 at 128). They are not traced
   here. Their trace (r1942: `diag/commits-v13-residual` 01fd4979, the fix plus the trace, replicated class at 128
   spread twice and 32 once, 00:53Z, load 16 to 19) names the same first wrong decision: every stale forward (21, 18
   and 24 a run) followed a slot ended by `expire`, with no detector verdict. The fix's floor is too short in real
   time: `Bounds::election` follows the owner's measured pace (`slot_bounds(election_tick, pace.period)`, re-bound as
   the pace moves), so slots were 28 to 49 ms long once the pace settled (p50 44 and 49 ms; only the first slots had
   200 ms), and expired ones had held 28 to 53 ms. On driven time the pace stays at the 10 ms tick, which is why the
   new test passes. The election timeout measures a member's silence; a holder that answers heartbeats while its
   forward is slow is not silent. For the successor (a design, not built): end a slot by time on the detector's
   verdict alone; make the holder end its slot on every path its write ends without a forward (today its
   barrier-phase deadline sends no ending barrier); and bound a live holder's lost forward by something it states,
   such as its write's remaining deadline on its barrier (a wire addition). Logs: adacachy2,
   `~/.cache/amphora-perf/commits-v13-out/residual/`.
2. **One suite failure on the fix arm is not explained by its own output.** `every_falsifier_of_the_floor_holds_under_a_nemesis`
   failed in round 3 on the fix. Main's product fails it too (1 of 4 alone in r1920, "a driven operation was
   unanswered after 60000 rounds"), and the fix passed it 4 of 4 alone, but the fix arm's own panic was filtered out,
   so it isn't shown to be the same failure.
3. **The fix changes a recorded decision** (replication.md, Q3, "the slot's length is learned, not picked"): the
   core's election timeout, the slot's first value, is now also its shortest. It adds no number, but it is Ada's to
   keep or reverse (below).
4. **The ladder is Amphora alone, one run a cell a round, on a loaded host** (load 70 to 131 on 32 cores; other lanes
   ran beside it). It shows the refusals; it claims no speed.
5. **The kiln server was unreachable from about 00:24Z to 00:28Z,** so the red and green runs started late.
6. Main's `replicated.rs` has five rustfmt differences in older tests (test 5, the five-crash test and others), so
   `cargo fmt --all --check` fails on main itself. Not changed here.

## The trace (r1814, branch `diag/commits-v13-refusals` 7907920e and 0d2e3e70, never for main)

The diagnostic branch adds an env-gated log (`AMPHORA_TRACE13`, one line an event, CLOCK_MONOTONIC ns, the same clock
on all three members) at the leader's slot (each barrier and forward with the slot's holder, turn, queue and held
forwards before and after; each start with its last index, commit and length; each end by its site), at each
forward's append (its base, the index it lands at, the verdict, and for a stale one every entry between its base and
its append with its origin), at each owner's proposal (its base against its log's last, commit and applied), and at
each member's barrier, read, prepare and apply result. `trace13.sh` runs 32 spread writers in both classes with V12's
flags (`--batch 1 --text-bytes 600 --deadline-s 10 --relay-records 128 --warmup-s 1 --seconds 5`, `--spanning 2`
for the replicated class), and `refusals.py` joins each refused write across the members and counts its first wrong
decision. r1814 ran on adacachy2 at needs cpus=4 (cpus=8 refused twice: 1 of 32 cores idle), 00:13:08Z to 00:13:40Z,
load (1 min) 90 to 99 on 32 cores, CPU "some" avg10 55 to 72%, I/O "some" 3.1 to 4.5%. Leader node 3 in every run.

| class | round | commits/s | p50 ms | refusals by node (1, 2, 3) | forwards appended | stale | expired slots | detector verdicts |
|---|---|---|---|---|---|---|---|---|
| persisted | 1 | 26.4 | 679.9 | 11, 10, 3 | 121 | 33 | 37 | 0 |
| replicated | 1 | 31.0 | 573.4 | 12, 12, 7 | 135 | 37 | 39 | 0 |
| persisted | 2 | 27.6 | 720.9 | 13, 13, 1 | 132 | 41 | 45 | 0 |
| replicated | 2 | 32.0 | 532.5 | 16, 9, 2 | 144 | 44 | 47 | 0 |

**First wrong decision 1, 148 of 155 stale forwards: the leader ended a live holder's slot at its learned length.**
Every slot end at the leader's own step was `expire`, 37 to 47 a run, none by the detector, a term, a change or a
hand-off. The other 7 forwards arrived with no slot of their own after an earlier expiry (held, then stepped behind
another member's entry). One, from round 1 persisted:

| ms | member | event |
|---|---|---|
| 1250.68 | leader | member 2's slot begins: last 27, commit 26, learned length 29.2 ms |
| 1274.59 | leader | the commit reaches 27 and member 2's barrier steps: 23.9 ms of its slot gone before it could start |
| 1279.82 | leader | the slot expires (29.2 ms); the leader's turn is owed |
| 1289.83 | leader | member 1's forward appends at 28 |
| 1289.96 | member 2 | its read answers at 27 (applied 27); it prepares on 27 and forwards |
| 1290.97 | leader | member 2's forward arrives, held through the leader's turn |
| 1319.18 | leader | the leader's own write appends at 29 |
| 1320.14 | leader | member 2's forward appends at 30 on base 27: stale, refused at apply |

The slot's length is the longest of its member's last one or two gaps, timed from the slot's start, so it includes
the leader's commit wait before the holder's barrier steps (24 ms here) as well as the holder's read round, apply and
forward. Each is a wait that varies with the flushes, so a holder's next gap outruns that estimate often: about a
third of the slots (37 to 47 of 121 to 144 a run). Every such slot refused its live holder's write.

**First wrong decision 2, the leaders' own refusals (1 to 4 a run): the turn was given up while the holder's entry was
uncommitted.** One, from round 1 persisted: at 2455.56 ms member 1's forward ends its slot and steps, with member 2's
late forward held through that slot (since 2431.77 ms) released behind it; the leader's write arrives 6 ms later and
prepares on its applied state, 66, with its log's last at 68 and `turn=false`, so the turn had been given up (by a
start that found no barrier waiting and no write of the leader's), and it is refused at apply. Which forward holds 67
and which 68 is inferred from that order, not logged. In its turn the write would have waited for its log's tail.

## The fix (ff4433ba, red tests c3ebea5d)

Both changes are in the slot's pure core (`database/slot.rs`), each where its decision is made. Nothing on the wire
or in an entry changes.

- `Slots::length`: a slot is at least the core's election timeout (`Bounds::election`, `election_tick` times the
  owner's pace, the slot's first value since its design), and longer when its member's learned gap is longer. A slot
  ends by time only once its holder is as late as the core takes a member for gone. The host's detector still ends a
  gone holder's slot at once (`keep`), and a slow member's learned gap still lengthens its next slot.
- `Slots::start`: a turn the leader has no write for passes only to a slot that begins. With no barrier waiting the
  turn stays owed and only the forwards it held go, so the leader's next write prepares in its turn, behind its log's
  tail, not on its applied state ahead of the holder's uncommitted entry.

What it costs (inferred from the code, not measured): a holder that dies with no verdict from the detector holds the
leader's writes up to an election timeout (200 ms at the 10 ms tick), where its learned gap held them about 30 ms.

Tests, each "do X, expect Y" citing CP-T1.8: `slot::a_slot_outlasts_a_learned_gap_shorter_than_the_election_timeout`
(new); `slot::forwards_wait_through_the_leaders_turn` and
`slot::a_refused_members_barrier_begins_its_slot_and_its_forward_goes_first` (their expectations now these rules);
`slot::a_learned_gap_is_believed_for_one_to_two_of_its_members_generations` (its gaps moved past the election
timeout, so it still tests generations); and
`replicated::v13_thirty_two_spread_writers_on_moving_time_are_each_answered_with_no_stale_base` (new, beside test 5):
32 writers spread over the three members, three rows each, both classes, every member's clock moving a tick of its
pace a round. A driven round holds every clock still, so no slot can end by time in test 5, which is why it stayed
green on main while real time refused.

## Runs

r1896 on adacachy2, needs cpus=8 (00:28Z to 00:40Z; load (1 min) 70 to 131 on 32 cores, CPU "some" avg10 26 to 59%,
I/O "some" 2.2 to 6.8%: a loaded host). Arms: c3ebea5d (main's product with the new tests: the red run) and
2332e2ae (the fix, formatted), debug test binaries built in turn in the lane's one target dir (88 s, then 33 s).

| check | c3ebea5d (main's product) | 2332e2ae (the fix) |
|---|---|---|
| focused: `database::slot::`, the new test, test 5 | 4 of 15 fail: the three slot tests (5 ms where 200 ms; `Expired` after 6 ms; the turn given up) and the new test, `stale-base refusals: [(7, "Persisted: in 617 rounds"), (5, "Replicated(...): in 540 rounds")]` | 15 of 15 pass |
| `cargo clippy -p amphora --all-targets --locked -- -D warnings` | | exit 0 (36.6 s) |
| `database::replicated:: replication::`, round 1 | 170 passed, 2 failed: the new test, `session_reads::t_t1_10_r1_a_session_reads_its_write_on_a_lagging_follower` (63 s) | 172 passed (50 s) |
| round 2 (arm order reversed) | 171 passed, 1 failed: the new test (60 s) | 172 passed (67 s) |
| round 3 | 171 passed, 1 failed: the new test (67 s) | 171 passed, 1 failed: `every_falsifier_of_the_floor_holds_under_a_nemesis` (52 s) |

The new test on driven time with clocks moving gives main's product the refusals real time gives it: 7 and 5 over
96 writes, and none with the fix. The suite's one fix-arm failure is main's own flake: r1920 ran that test alone 4
times on each arm, alternated (load 64 to 90). The fix passed 4 of 4, and main's product failed 1 of 4, "a driven
operation was unanswered after 60000 rounds" with its leader mid-change (`changing true`). Each arm failed it once in
7 runs. The fix arm's round 3 panic was not kept (the suite's output was filtered), so whether it is the same
failure is not shown. `t_t1_10_r1_...` failed on main's product in round 1, as lane V11 saw it fail on main.

**The ladder (r1896, after the suites; the fix's release bench, 47.8 s warm).** `ladder13.sh`: Amphora alone, V12's
flags, 32 and 128 spread writers, both classes, 3 rounds, cell order reversed in round 2. Refusals are the
driver's `Serialization` count by node (node 3 led every run). Load (1 min) 70 to 90, CPU "some" avg10 26 to 59%.

| class | writers | refusals by node, r1; r2; r3 | commits/s per round | p50 ms per round | misses |
|---|---|---|---|---|---|
| persisted | 32 | 0 0 0; 0 0 0; 0 0 0 | 36.8, 41.4, 39.0 | 737, 844, 598 | 0 |
| persisted | 128 | 0 0 0; 0 0 0; 0 0 0 | 36.6, 43.2, 37.8 | 2,818, 2,228, 3,441 | 0 |
| replicated | 32 | 0 0 0; 0 1 0; 0 0 0 | 48.0, 40.4, 46.8 | 631, 565, 582 | 0 |
| replicated | 128 | 0 2 0; 3 1 1; 0 1 0 | 38.8, 42.8, 41.4 | 2,228, 3,572, 2,327 | 0 |

Against V12's r1716 on main (9 to 41 refusals a run at 32 and 128 spread in both classes) and this lane's trace
(24 to 31 at 32 spread): the persisted class has none left in 6 runs; the replicated class has 0 to 5 a run, its
128-writer runs in each round. The rates are inside V12's (31.8 to 46.4 persisted, 30.4 to 38.6 replicated), one
run a cell a round on a loaded host, so no speed is claimed either way.

## Commands

    # r1814: kiln run on 00aad8daff2fa3c9daa65d53c425d450, needs linux/amd64,cpus=4,mem=12g, sync false,
    # cwd ~/.cache/amphora-perf/commits-remote at 7907920e, CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits,
    # CARGO_INCREMENTAL=0, CARGO_BUILD_JOBS=4 (the release build took 1 min 19 s, warm)
    bash bench/records/2026-11-23-adacachy2-perf-commits-v13/trace13.sh ~/.cache/amphora-perf/commits-v13-out/trace-001149 2
    python3 -I bench/records/2026-11-23-adacachy2-perf-commits-v13/refusals.py ~/.cache/amphora-perf/commits-v13-out/trace-001149/<class>-32-spread-r<n>
    # r1896: the same runner and folders, needs linux/amd64,cpus=8,mem=24g, CARGO_BUILD_JOBS=8, THREADS=8
    bash bench/records/2026-11-23-adacachy2-perf-commits-v13/tests13.sh c3ebea5d 2332e2ae 0   # builds, focused, clippy
    ~/.cache/amphora-perf-bin/commits/v13-tests-<sha> --test-threads=8 database::replicated:: replication::   # 3 rounds, alternated
    bash bench/records/2026-11-23-adacachy2-perf-commits-v13/ladder13.sh ~/.cache/amphora-perf-bin/commits/v13-2332e2ae \
      ~/.cache/amphora-perf/commits-v13-out/ladder-003832 3
    # r1920: the floor test alone, 4 times each arm, alternated, needs cpus=4
    ~/.cache/amphora-perf-bin/commits/v13-tests-<sha> --exact database::replicated::tests::every_falsifier_of_the_floor_holds_under_a_nemesis --nocapture
    # r1941: the merged tree a45edff6 (main 90c1ff8d), needs cpus=4: clippy exit 0, focused 15 of 15
    cargo clippy -p amphora --all-targets --locked -- -D warnings
    cargo test -p amphora --lib --locked -- --test-threads=4 database::slot:: v13_thirty_two v7_a_hundred_and_twenty_eight

# Lane V14 (commits) on adacachy2: a live holder's slot ends by its forward or its close; by time only at the deadline its barrier states or on the detector's verdict (2026-11-24, 00:59Z to 01:54Z)

Ada's goal is replicated commits about 10x faster than rqlite and dqlite, with no write refused under load. This
lane's deliverable: V13's design for the replicated class's last stale-base refusals, built, with a test that fails
first, zero refusals on the ladder at 32 and 128 spread writers in both classes, the suite at parity, merged to main.

## Losses first

1. **The kiln runs r1987 and r1993 were collected when the coordinator's session restarted (about 01:45Z).** r1987
   had finished: its suite logs are read from disk (adacachy2, `~/.cache/amphora-perf/commits-v14-out/run-013537/`).
   r1993's output (the merged tree's re-check) was lost, so that check was run again at 01:51Z.
2. **What it costs (inferred from the code, not measured).** A holder whose forward is lost and whom the host's
   detector never suspects holds the leader's own writes and the other members' forwards until its write's own
   deadline (10 s with the ladder's driver), where V13's floor held them about an election timeout. A live holder
   whose preparation is slow holds them as long as it prepares: that is the design (its write would otherwise be
   refused), and the ladder's rates below show no fall, but no speed is claimed either way.
3. **The ladder is Amphora alone, one run a cell a round, on a loaded host** (load 39 to 78 on 32 cores, CPU "some"
   avg10 5 to 42%). It shows the refusals; it claims no speed.
4. The merge commit 4656c99c's message names main's moves in crates/engine and crates/pg-parser; main also moved in
   crates/amphora's store.rs and tests/store.rs (lanes A16/A18). The re-check below ran on that merged tree.
5. P14's proposal (a close the leader decides by a mark) is already on main by V11's read mark: a marked close goes
   through `Slots::pass`, which never queues or starts a slot, so a late close ends nothing new. Folded in from P14:
   a member sends the close only while its barrier's leader leads its term.

## The change (7d08d4ff, red test alone on main's product 8752f453)

- **The wire, append-only.** A write's barrier context carries, past the 32-byte request identity, a mark
  (`WRITE_DEADLINE`, 2) and the time its write has left as it was sent, in nanoseconds (`u64`, little-endian). A
  leader before it reads the context as a write's (anything but a read's mark) and echoes it unread. The write's
  entry carries the request identity alone (the context is cut back to it before the proposal), so nothing in the
  log changes.
- **The leader (`Slots::read`, `Slots::start`).** A waiting barrier keeps its stated end on the leader's clock
  (arrival plus the time left: later than the holder's own end by the link's one-way time, never earlier). The slot
  ends by time only at that end, which bounds a lost forward; before it, only the holder's forward, its close, or the
  host's detector (`Slots::keep`, unchanged) ends it. A barrier stating no end (a member before this) keeps V13's
  learned length and floor.
- **The holder (`Actor::expire`, `Actor::close`).** A follower whose write's deadline ends it at its barrier (before
  it prepared: waiting for the read, or for its own log's tail) now sends its close, a barrier marked a read's, as
  V11's path does for a write that prepared nothing. It is sent at the next step that isn't paused, and only while
  the barrier's term and leader hold.

Tests, each "do X, expect Y" citing CP-T1.8:
`replicated::v14_a_slow_live_holder_keeps_its_slot_past_the_election_timeout` (new, red on main's product):
owner 2 leads, member 1 writes and its preparation moves the leader's clock two election timeouts on, the leader's
own write waits on the slot; both answered Ok and both rows on every replica, in both classes.
`slot::a_live_holders_slot_ends_by_its_forward_or_its_stated_deadline_not_the_election_timeout` (new): at a 30 ms
election timeout (real time's pace) a slot stating 10 s holds through 6 election timeouts and ends forwarded; one
stating 500 ms with no forward ends expired just past it. `slot::a_dead_holders_slot_ends_when_the_detector_suspects_it`
(new): a slot stating 10 s holds 10 election timeouts with no verdict, then ends released on the detector's
suspicion, the held forward ready and the leader's turn owed. The old slot tests pass `None` (no stated end) and keep
testing V13's length.

## Runs

r1987 on adacachy2, needs cpus=4 (cpus=8 refused: 7 of 32 cores idle), 01:35Z to 01:45Z, `v14.sh` (in this
record's folder): debug lib tests of 8752f453 and 7d08d4ff built in turn in the lane's one target dir (22 s, 23 s),
the focused tests on each, clippy, the release bench (149 s), V13's ladder, then the replicated suites 3 rounds,
arm order alternated.

| check | 8752f453 (main's product + the red test) | 7d08d4ff (the change) |
|---|---|---|
| focused: `database::slot::`, the V14, V13 and V7 driven tests | 15 passed, 1 failed: the V14 test, `Persisted: [("the leader", Ok(())), ("member 1", Err(Serialization { what: "prepared transaction", why: "the database advanced since preparation" }))]` | 18 passed |
| `cargo clippy -p amphora --all-targets --locked -- -D warnings` | | exit 0 |
| `database::replicated:: replication::`, round 1 (the change first) | 172 passed, 1 failed: the V14 test (44 s) | 173 passed (45 s) |
| round 2 (arm order reversed) | 172 passed, 1 failed: the V14 test (45 s) | 173 passed (44 s) |
| round 3 | 172 passed, 1 failed: the V14 test (47 s) | 173 passed (45 s) |

Over 3 rounds no test that main's product passes fails on the change; main's product fails only the new test, each round.

**The ladder (r1987, the change's release bench).** V13's `ladder13.sh`: Amphora alone, V12's flags (`--batch 1
--text-bytes 600 --deadline-s 10 --relay-records 128 --warmup-s 1 --seconds 5`, `--spanning 2` for the replicated
class), 3 rounds, cell order reversed in round 2. Refusals are the driver's `Serialization` count by node (node 3
led every run).

| class | writers | refusals by node, r1; r2; r3 | commits/s per round | p50 ms per round | misses |
|---|---|---|---|---|---|
| persisted | 32 | 0 0 0; 0 0 0; 0 0 0 | 39.0, 54.0, 53.2 | 786, 606, 664 | 0 |
| persisted | 128 | 0 0 0; 0 0 0; 0 0 0 | 42.4, 53.0, 54.4 | 3,146, 2,425, 2,490 | 0 |
| replicated | 32 | 0 0 0; 0 0 0; 0 0 0 | 37.2, 42.2, 43.4 | 680, 877, 541 | 0 |
| replicated | 128 | 0 0 0; 0 0 0; 0 0 0 | 40.4, 44.0, 60.8 | 2,228, 1,933, 1,589 | 0 |

Against V13's r1896 (replicated class 0 to 5 refusals a run, its 128-writer runs in each round): none in 12 runs.
The rates are inside or above V13's (36.6 to 48.0), one run a cell a round on a loaded host (load 77 in round 1, 39
in round 3), so no speed is claimed.

**The merged tree 4656c99c** (01:51Z, needs cpus=8, load 11): focused 18 of 18 passed, `cargo clippy -p amphora
--all-targets --locked -- -D warnings` exit 0.

## Commands

    # r1987: kiln run on 00aad8daff2fa3c9daa65d53c425d450, needs linux/amd64,cpus=4,mem=12g, sync false,
    # cwd ~/.cache/amphora-perf/commits-remote, CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits,
    # CARGO_INCREMENTAL=0, CARGO_BUILD_JOBS=4, test threads 4
    bash bench/records/2026-11-23-adacachy2-perf-commits-v14/v14.sh 8752f453 7d08d4ff 3 ~/.cache/amphora-perf/commits-v14-out/run-<hhmmss>
    # r1993: the merged tree 4656c99c, the same runner and needs
    cargo test -p amphora --lib --locked -- --test-threads=4 database::slot:: v14_a_slow v13_thirty_two v7_a_hundred_and_twenty_eight
    cargo clippy -p amphora --all-targets --locked -- -D warnings

# Lane V15 (commits) on adacachy2: lane V14's two gaps, one red on main and one measured; neither closed (2026-11-24, 01:59Z to 02:58Z)

Ada's goal is replicated commits about 10x faster than rqlite and dqlite, with no write refused under load. This
lane's deliverable: lane V14's two robustness gaps closed and merged. Gap 1, a driven test in which a follower's
write passes its deadline in its barrier phase and its close ends the leader's slot released, with no clock moved
for it, red on main before lane V14. Gap 2, a real-time run that loses the holder's forward, with the wait of the
leader's other writes timed per stage, bounded by an event the leader can see if it is the holder's whole deadline,
and the rule in replication.md. Then the replicated suites three rounds and the ladder's zero refusals.

## Losses first

1. **The deliverable is not met.** Gap 1's test is red on main too: lane V14's close does not reach the leader when
   the write's barrier answer was lost, so the slot holds until the leader's clock passes it, as before lane V14.
   The test is not on main (branch `perf/commits-v15`, 4b473a4d). Gap 2's wait is measured, the holder's whole
   10 s deadline, and no bound is built. The suites' three rounds and the ladder were not run for a change, since
   nothing in `crates/` landed. What landed is docs only: replication.md's write-slot rule (lane V14's, which the
   doc lacked), the open defect, the measured wait, and the two designs for a bound.
2. **Where lane V14's close stops is not traced past the owner.** r19's instrumented run shows member 3's `expire`
   taking the barrier-phase branch (deadline passed, term 1 and leader 1 unchanged, not leading) and `Actor::close`
   past its checks (term 1/1, leader 1/1), and no error from its replica's `read`; yet the pump saw no read-marked
   ReadIndex from member 3. hyper-raft's follower forwards a ReadIndex as it comes (`step_follower`, `forward`), so
   the stop is between the replica's accepted read and the wire. r37 (79fd65c8, cpus=2, 02:41Z) printed every
   message member 3 sent through the pump and the close's result: `Actor::close`'s read returned Ok, and member 3
   sent nothing at all in the rounds after it, so the close is held inside member 3 (its replica's drive or its
   owner's release), not lost on the way. Not found further in this hour.
3. **Time lost to the platform.** The coordinator's kiln restarted twice and stopped this session (about 02:01Z and
   02:18Z), kiln's server was unreachable for 5 of this lane's run requests, and adacachy2 refused cpus=8 once and
   cpus=4 once (3 of 32 cores idle), so runs went at cpus=4 and cpus=2.
4. **Two of this lane's runs shared its remote clone at once** (r3 and r16, about 02:34Z): r16 checked out the
   merged tree while r3 built its red arm. r3's red result names its own tree's line (18865, against 19010 on the
   green arm), so it built the red tree, but the overlap was a process error; r3 and r16 were then stopped.
5. **The stages run measured the persisted class only.** It panicked in its teardown after that class (the
   diagnostic skipped the harness's drain), before the replicated class.
6. **The first two versions of the test were confounded** (r28, r65): member 3's floor keeper attests once it
   applies the table, and its session (an owner's write, through a barrier) held member 3's slot as a write's would;
   the test's write waited behind it in member 3's queue and timed out there, sending no close by design. The test
   now waits for that session to end and loses only the write's barrier answers (r3).

## Gap 1: a write its deadline ends at its barrier (red on main)

`replicated::v15_a_write_its_deadline_ends_at_its_barrier_closes_its_slot` (branch `perf/commits-v15`; CP-T1.8), in
each class: owner 0 leads; member 3's floor session ends; the leader's answers to member 3's barriers are lost while
member 3 writes with one tick of its pace left; its barrier begins its slot (`slot::holder`, a test's view, names
member 3, and member 3's pending is that write's barrier); the leader's own write waits on the slot; member 3's
clock passes the deadline. Expect member 3 refused `Timeout`, member 3's close sent and the slot free after it, the
leader's write Ok, one slot ended `released` and none `expired`, `Driven::lapses` 0. The red arm is the same test on
7d6cd7f0, main before lane V14 (`diag/commits-v15-red`).

| run (adacachy2) | arm | result |
|---|---|---|
| r28, 02:10Z, cpus=8, load 70 to 80 | 6f37fccf red, d0094bd3 on main (first version) | both: slots ended (released, expired) (0, 1) |
| r65, 02:14Z, cpus=4, load 58 | 6e495458 on main (holder named) | (released, expired, lapses) (0, 1, 1) |
| r3, 02:33Z, cpus=4, load 19 to 44 | 6adfa5f8 on main (floor session first) | Timeout as expected; member 3 closed false, slot still member 3's |
| r3 | ba2b2351 red | the same |
| r19, 02:37Z, cpus=2, load 66 | 193ff5dc (6adfa5f8 with `eprintln` in `expire`, `close`, `prepare_barrier`) | `expire id=3 phase_barrier=true expired=true changed_term=false leader=1 is_leader=false`; `close id=3 term=1/1 leader=1/1 is_leader=false`; no close counted |
| r37, 02:41Z, cpus=2, load 63 | 79fd65c8 (r19's, and every message member 3 sends, and the close's read) | `close read Ok(())`; no message from member 3 reached the pump after it |

The other slot tests ran beside it in r3: `database::slot::`, `v14_a_slow_live_holder_keeps_its_slot_past_the_election_timeout`
and `a_slot_holder_whose_write_fails_ends_its_slot_without_waiting_its_length` passed on main's arm (17 passed, the
new test failed).

## Gap 2: a lost forward on real time (measured, not bounded)

r65's stages run (`diag/commits-v15-stages` d64cff0b, 02:16:50Z, cpus=4, load (1 min) 41 on 32 cores, CPU "some"
avg10 17%). The diagnostic opens three members in one process whose owners read the real clock, relayed by the
test pump, whose `losing` hook lost member 3's first forward to the leader. Persisted class; tick 10 ms,
election_tick 20, heartbeat_tick 2; member 3's write with the bench driver's 10 s deadline. Times are from member
3's call, by the real clock (617,145 relay rounds).

| ms | stage |
|---|---|
| 0.00 | member 3's write is called, 10 s deadline |
| 0.74 | the leader's slot for member 3 begins at last 3, commit 3 |
| 2.70 | member 3's forward is lost |
| 2.75 | the leader's own write and member 2's are called |
| 10,000.21 | member 3 answered `UnknownOutcome` |
| 10,006.74 | the slot ends `expired` (forwarded, expired, released: 0 0 0 to 0 1 0); the leader's log grows past the slot's start |
| 10,008.24 | the commit passes the slot's start |
| 10,030.61 | the leader's write answered Ok (waited 10,027.9 ms) |
| 10,064.82 | member 2's write answered Ok (waited 10,062.1 ms) |

Each of the leader's other writes waited the holder's whole deadline: the slot ended `expired` 6.5 ms after member 3
was answered at its own deadline. No event ended it sooner: the holder sends no barrier or forward after its forward, and the test's detector suspected no one. Two
designs for a bound are in replication.md ("What bounds a lost forward"); neither is built or measured.

## Commands

    # every run: kiln run on 00aad8daff2fa3c9daa65d53c425d450, sync false, cwd ~/.cache/amphora-perf/commits-remote,
    # CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits, CARGO_INCREMENTAL=0, CARGO_BUILD_JOBS = the needs' cpus
    cargo test -p amphora --lib --locked --no-run    # each arm in turn, the binary kept as ~/.cache/amphora-perf-bin/commits/v15-tests-<sha>
    v15-tests-<sha> --test-threads=4 v15_a_write database::slot:: v14_a_slow a_slot_holder_whose_write_fails
    v15-tests-d64cff0b --ignored --nocapture --test-threads=1 v15_stages    # r65, gap 2's stages
    v15-tests-193ff5dc --test-threads=1 --nocapture v15_a_write             # r19, gap 1's trace (r37: 79fd65c8)

# Lane V16 (commits) on adacachy2 and macOS: lane V15's gap 1 traced to its line and fixed; the close reaches the leader (2026-11-24, 02:43Z to 03:48Z)

Ada's goal is replicated commits about 10x faster than rqlite and dqlite, with no write refused under load. This
lane's deliverable: lane V15's gap 1 closed, so that a follower's close reaches the leader when its write's barrier
answer was lost; found by a trace, fixed, robust over the suites and the ladder, merged to main.

## Losses first

0. **Not on main.** The branch `perf/commits-v16` (6140be06, with main 9fbfdbaf merged and re-checked) was green
   at 03:50Z, past the hour's end (03:48Z); main had moved again to fac69083 in crates/engine (lane PG13), which
   needs one more re-check before the push (RULES 6, landing while main moves). The lander's steps: merge
   origin/main, `cargo test -p amphora --lib --locked -- v15_a_write_its_deadline database::slot:: v14_a_slow
   a_slot_holder_whose_write_fails`, `cargo clippy -p amphora --all-targets --locked -- -D warnings`, push.
1. **The ladder and the suites against main's product were not run on the landed tree.** adacachy2 was the verifier's
   quiet window from 03:00Z to 03:45Z, and the macOS runs (r121's ladder) had not finished its cold release build at
   load 64 to 140 on 18 cores when the hour ended; it was stopped. So the deliverable's robust
   half is shown only by the three suite rounds on b9dd342b (r73), not by the ladder.
2. **The suites' comparison arm on Linux was the harness change alone (aedd6cef), not main's product.** Main's own
   product was run only on the four tests both arms failed (the runs at 02:57:57Z and 02:58:25Z), not the whole suite; the fix's one failure
   that neither of those runs covers (`a_local_read_holds_off_the_logs_restart_and_one_after_waits_for_it`, once in
   three rounds, in the slowest round) is not shown to fail on main.
3. **Four replicated tests fail on main itself** (lane B18's decision 45 is the only change in crates/ between a
   main that passes them and one that fails them): listed under Runs. Not this lane's; not traced.
4. **r73 likely ran past 03:00Z into the verifier's quiet window on adacachy2**, by about 20 to 45 s: its six
   suite runs took 377 s in all from about 02:54:10Z, and its 300 s timeout did not stop it (inferred from its
   rounds' times; kiln's end time not read).
5. **kiln's sync to the MacBook Pro timed out twice** (r102, r107), and the Mac Studios were queued behind other
   lanes, so the merged tree's checks ran from a GitHub clone on adas-macbook-pro-2 at load 64 to 83 on 18 cores.
6. **The fix changes the driven harness every driven test runs on** (`Driven::quiet`): an owner left driving is
   now stepped before the group counts as quiet. The suites above are its only check.
7. **Where else the same fault may be.** `run_step` makes other work for the core after its one drive (a waiting
   write's dispatch, `bootstrap`, `keep_floor`, `propose_class`, `prepare`); each waits likewise for the next wake
   unless its drive left more. Read from the code, not measured; not changed (not this lane's deliverable).

## Where the close stopped (r57, branch `diag/commits-v16-close` cd720fde, never for main)

The diagnostic traced member 3 from its close on: `Actor::close` printed the core's queue after its read, and
from then on each of member 3's steps (paused or not, and each drive's messages) and each of its replica's drives
(`has_ready`, the core's queue, the writes out, `ready_due`'s parts, the `Ready` taken and the messages a durable
write gives out). r57 on adacachy2 (cpus=2, load 47 to 50 on 32 cores, 02:48Z), lane V15's test, persisted class:

    V16 close id=3 read=Ok(()) has_ready=true msgs=[(MsgReadIndex, 1)] in_flight=0 at=10ms
    panicked at crates/amphora/src/database/replicated.rs:19046:13: Persisted: member 3 closed, and the slot's holder after
      left: (false, Some(3))

No other V16 line follows the close: member 3 never stepped again. The close's ReadIndex to the leader (member 1)
sat in member 3's core (`Raft::msgs`), with no write out and a `Ready` due. The line is `run_step`'s
`self.close()?` (replicated.rs, after the step's one `self.replica.drive`): the close is asked of the core after
the step's only drive, and `self.driving` was set from that drive (`driven.more`) before it, so nothing drives the
close out until the owner's next wake. On a real clock that is its next tick or input (read from `run_loop`'s wait,
not measured); in the driven harness, whose clocks stand still, it never comes: `Driven::quiet` stops at a round
with nothing to relay and no write out on any disk, and member 3's queued close is neither.

## The fix (aedd6cef, b9dd342b; merged tree d7df6b30)

- **`Actor::close`** (b9dd342b): once its read is queued, the owner is marked driving, so `run_loop` steps again at
  once and that step's drive takes the `Ready` the close is in. One drive a step stays the rule.
- **`Driven::quiet`** (aedd6cef): an owner whose last step left more to drive (`Actor::driving`) is not quiet, as
  its thread steps again at once (`run_loop`); lane QB2's test already waited for it by hand. Alone, this is the red
  arm: lane V15's test fails on it as on main.
- replication.md's write-slot rule says so, in place of lane V15's open defect.

## Runs

Linux runs on adacachy2 ended by 03:00Z but r73 (below); macOS runs on adas-macbook-pro-2 (e30189ea), after two
kiln syncs to it timed out (r102, r107: "did not answer the sync in 60s"), from a GitHub clone there (sync false).

| run | where, needs, load | tree | result |
|---|---|---|---|
| r57 | adacachy2, cpus=2, 47 to 50, 02:48Z | cd720fde, the diagnostic | the trace above; lane V15's test fails `(false, Some(3))` |
| r68 | adacachy2, cpus=4, 31 to 43, 02:52Z | b9dd342b, the fix | focused: 18 passed (`v15_a_write_its_deadline`, `database::slot::`, `v14_a_slow`, `a_slot_holder_whose_write_fails`) |
| r68 | | aedd6cef, the harness alone | 17 passed, 1 failed: lane V15's test, `(false, Some(3))` at replicated.rs:19037 |
| r76 | adacachy2, cpus=4, 34 to 42 | b9dd342b | `cargo clippy -p amphora --all-targets --locked -- -D warnings` exit 0 (29 s) |
| r79 | adacachy2, cpus=4, 34 to 41 | f76ebe98 | `scripts/check-production.sh` exit 0; the engine's ratchet holds (10,903 left) |
| r113 | adas-macbook-pro-2, cpus=8, load 65 to 80 on 18 cores, 03:13Z to 03:41Z (cold build 1,510 s) | d7df6b30, the merged tree | focused: 18 passed |
| r113 | | 6f3eafa1, the merged tree with the fix reverted | 17 passed, 1 failed: lane V15's test, `(false, Some(3))` at replicated.rs:19038 |
| r121 | the same, cpus=4, from 03:23Z | d7df6b30, the release bench and the ladder | the cold release build had not finished at 03:45Z; stopped |
| (id not read) | adacachy2, cpus=8, load 16, 03:45Z to 03:47Z | d7df6b30 | `cargo clippy -p amphora --all-targets --locked -- -D warnings` exit 0; `scripts/check-production.sh` exit 0, the ratchet holds (10,903) |
| (id not read) | adacachy2, cpus=4, load 42, 03:47Z to 03:50Z | 4de6e2ff, with main 9fbfdbaf (lane R9's engine changes) merged | focused: 18 passed; clippy exit 0. Main then moved to fac69083 (lane PG13, crates/engine and pg-catalog): not landed |
**The replicated suites, 3 rounds** (r73, adacachy2, cpus=4, `--test-threads=4 database::replicated:: replication::`,
arm order alternated, load 24 to 37 on 32 cores):

| round | b9dd342b (the fix) | aedd6cef (the harness alone) |
|---|---|---|
| 1 | 170 passed, 4 failed (53 s) | 168 passed, 6 failed (50 s) |
| 2 | 169 passed, 5 failed (88 s) | 168 passed, 6 failed (52 s) |
| 3 | 170 passed, 4 failed (48 s) | 168 passed, 6 failed (86 s) |

- On both arms nearly every round: `a_local_read_open_across_an_image_install_sees_one_side_whole`,
  `a_replicas_database_log_stays_within_its_room`, `past_its_room_a_replica_refuses_submits_and_restarts_whatever_its_share`,
  `a_checkpoint_pass_copies_no_frame_past_the_last_finished_flush`. Main's own product fails them too: a run at 02:57:57Z (its kiln id not
  read) ran the four on cd720fde (main a5ae048c and lane V15's test, with the diagnostic's prints and neither change of
  this lane): 2 failed (`past_its_room`, `a_local_read_open`); one at 02:58:25Z (id not read) ran the other two on all three trees: each
  failed both, with the same values (`left: 77, right: 17` at the checkpoint test). On lane V15's 6adfa5f8 (main
  46773985) all four passed (the 02:57:57Z run). The only commits in crates/ between those mains are lane B18's (decision 45).
- On the fix only, once (round 2, its slowest): `a_local_read_holds_off_the_logs_restart_and_one_after_waits_for_it`;
  and `a_restart_due_waits_for_a_reads_deadline_and_no_longer`, which the harness alone failed in round 3 too.
  Neither was run on main's product here.
- On the harness alone only: lane V15's test (each round), `a_group_upgrading_to_the_floor_tracks_joins_only_once_every_member_attests`
  (rounds 2 and 3), `window_slow_flooded_fast_starved` (round 1).

## Commands

    # Linux runs: kiln run on 00aad8daff2fa3c9daa65d53c425d450, sync false, cwd ~/.cache/amphora-perf/commits-remote,
    # CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits, CARGO_INCREMENTAL=0, CARGO_BUILD_JOBS = the needs' cpus
    cargo test -p amphora --lib --locked --no-run    # each arm in turn, kept as ~/.cache/amphora-perf-bin/commits/v16-tests-<sha>
    v16-tests-cd720fde --test-threads=1 --nocapture v15_a_write_its_deadline                         # r57, the trace
    v16-tests-<sha> --test-threads=4 v15_a_write_its_deadline database::slot:: v14_a_slow a_slot_holder_whose_write_fails
    v16-tests-<sha> --test-threads=4 database::replicated:: replication::                            # r73, 3 rounds, arms alternated
    cargo clippy -p amphora --all-targets --locked -- -D warnings; bash scripts/check-production.sh
    # macOS runs: kiln run with needs macos/arm64,cpus=8,mem=24g, sync true from the lane's clones
    bash bench/records/2026-11-23-adacachy2-perf-commits-v16/v16.sh <red> <fix> 0 <out> 8       # builds, focused tests
    bash bench/records/2026-11-23-adacachy2-perf-commits-v16/ladder16.sh <release amphora-bench> <out> 3

# Lane V17 (commits) on adacachy2: lane V16's fix on main; its one-round failure is main's own; the ladder on main (2026-11-24, 03:52Z to 04:53Z)

Ada's goal is replicated commits about 10x faster than rqlite and dqlite, with no write refused under load. This
lane's deliverable: lane V16's fix (a follower's close driven at once, aedd6cef and b9dd342b) merged with main,
checked and pushed; the test it failed once in three suite rounds (r73) run alone as many times on main as on the
merge; V16's ladder at 32 and 128 spread writers, both classes, on the landed tree.

## Losses first

LADDER_LOSS
1. **The one-round failure is main's own, and it is not traced.** `a_local_read_holds_off_the_logs_restart_and_one_after_waits_for_it`
   fails alone 5 times in 16 on main fac69083 and 5 times in 16 on the merge 1e07a639 (r188), each at its outer
   `read_local`'s `unwrap` with `Timeout` after the read's 60 s (`HANG_READ`). The brief said to trace and fix it
   only if the fix alone failed it, so it was neither traced nor fixed. The only wait in `read_local` that ends in
   `Timeout` is `Local::with`'s loop for a reader (read from the code), so the read waited there its whole 60 s;
   whether for a log restart's hold (`ledger.restarting`) or for a charge is not traced.
2. **The alone runs compared 1e07a639 with fac69083, not the landed tree with its main.** Main moved twice inside
   crates/ before the push (lanes T19, R10 in crates/engine; lanes D4 and P20 in crates/amphora, replicated.rs
   among them), and the alone runs were not repeated on c39adf56.
3. **Two landings were checked, passed and not pushed:** 1e07a639 (with main fac69083, r188) and 73004633 (with main
   c67455f6, r240). Main moved inside crates/ before each push; the third, c39adf56 (with main 9ed5b097, r263), is on
   main since 04:19:57Z.
4. **Most runs had less room than RULES 6 names:** adacachy2 had 1 to 7 of 32 cores idle, at load 44 to 78; cpus=8
   was refused at 03:56Z, cpus=4 and cpus=2 at 04:10Z, cpus=4 at 04:16Z and 04:17Z. r263 and r268 ran at cpus=2.
5. **I stopped r188 at 04:10:15Z**, after its alone section: its ladder section would have built 1e07a639, no longer
   the tree to land. I stopped r237 (cpus=1) seconds after its start at 04:11:49Z, when 7 cores came idle, and asked again
   at cpus=4 (r240).

## Runs

All on adacachy2 (00aad8daff2fa3c9daa65d53c425d450), sync false, from the GitHub clone ~/.cache/amphora-perf/commits-remote,
CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits, CARGO_INCREMENTAL=0, CARGO_BUILD_JOBS = the needs' cpus.

| run | needs, load, time | tree | result |
|---|---|---|---|
| r188 | cpus=4; load 45 to 67 on 32 cores, CPU pressure avg10 13 to 40, I/O 4 to 13; 03:56Z to 04:10Z | 1e07a639, V16's branch with main fac69083 | lib tests built in 71 s; focused: 18 passed; clippy exit 0; check-production exit 0, the engine's ratchet holds (10,903) |
| r188 | | fac69083, main without the fix | lib tests built in 23 s |
| r188 | | both, the test alone | 16 pairs from 03:59:39Z, the arm order alternated: fac69083 11 passed, 5 failed; 1e07a639 11 passed, 5 failed |
| r240 | cpus=4; load 65 to 78; 04:12Z to 04:16Z | 73004633, with main c67455f6 (lane T19) | built in 102 s; focused: 18 passed; clippy 0; check-production 0 (10,903). Main then moved in crates/ |
| r263 | cpus=2; load 44 to 52; 04:16:45Z to 04:19:45Z | c39adf56, with main 9ed5b097 (lanes D4, P20, R10) | built in 78 s; focused: 18 passed; clippy 0; check-production 0 (10,903); pushed to main at 04:19:57Z |
| r268 | cpus=2; load 37 to 46, CPU pressure avg10 18, I/O 9 to 11; 04:17:12Z to 04:23:26Z | c39adf56 (main) | the release bench built in 237 s; V16's ladder, 3 rounds: no refusal in 12 runs; deadline misses in 3 of the 6 runs at 128 writers (below) |

clippy re-checked amphora-engine in each run (its log shows "Compiling amphora-engine"), and `cargo fmt --all
--check` passed on each merge before its run.

## The test alone (r188)

| pair | 2 | 4 | 5 | 10 | 13 | 14 | 15 | 16 |
|---|---|---|---|---|---|---|---|---|
| fac69083 (main) | | failed | failed | | failed | | failed | failed |
| 1e07a639 (the merge) | failed | | | failed | failed | failed | | failed |

Every other run passed. Passing runs took 1.0 to 2.0 s (pairs 1 and 2); each failure took 60.36 to 61.0 s, and
each said the same (the merge's pair 2, from its saved log; main's at 16879:14, the same `unwrap`):

    panicked at crates/amphora/src/database/replicated.rs:16884:14:
    called `Result::unwrap()` on an `Err` value: Timeout

## The ladder on main (r268)

V16's `ladder16.sh` (V13's `cell13.py`): Amphora alone, V12's flags (`--batch 1 --text-bytes 600 --deadline-s 10
--relay-records 128 --warmup-s 1 --seconds 5`, `--spanning 2` for the replicated class), 32 and 128 spread writers,
both classes, 3 rounds, cell order reversed in round 2. Refusals are the driver's count by node; node 3 led every
run but round 2's persisted 128 (node 2).

| class | writers | refusals by node, r1; r2; r3 | commits/s per round | p50 ms per round | misses per round |
|---|---|---|---|---|---|
| persisted | 32 | 0 0 0; 0 0 0; 0 0 0 | 20.4, 17.4, 20.6 | 1,458, 1,901, 1,769 | 0, 0, 0 |
| persisted | 128 | 0 0 0; 0 0 0; 0 0 0 | 23.0, 17.4, 10.8 | 6,095, 7,340, 7,733 | 0, 0, 9 |
| replicated | 32 | 0 0 0; 0 0 0; 0 0 0 | 22.0, 12.4, 19.8 | 1,229, 1,376, 1,720 | 0, 0, 0 |
| replicated | 128 | 0 0 0; 0 0 0; 0 0 0 | 19.2, 20.6, 5.2 | 4,456, 5,374, 9,699 | 0, 3, 16 |

No write was refused in 12 runs, as in V14's r1987. But the rates are 5.2 to 23.0 commits a second against
r1987's 37.2 to 60.8, the p50s at 128 writers 4,456 to 9,699 ms against its 1,589 to 3,146, and 28 writes missed
their 10 s deadline where V14's and V13's ladders had none. The driver's last lines in round 2's replicated runs:
`node 1 stop: node 1 closed the connection`. This run had cpus=2 of room at load 37 to 46; r1987 had cpus=4 at load
39 to 77, on main as of 01:35Z.

AB_SECTION

## Commands

    bash bench/records/2026-11-23-adacachy2-perf-commits-v17/v17.sh <merge> <main> <window s> <ladder rounds> <out> <threads> [parts]
    # r188: 1e07a639 fac69083 600 3 <out> 4 (parts 1 and 2; stopped before part 3)
    # r240: 73004633 c67455f6 0 3 <out> 4 1;  r263: c39adf56 9ed5b097 0 3 <out> 2 1;  r268: c39adf56 9ed5b097 0 3 <out> 2 3
    # part 1: cargo test -p amphora --lib --locked --no-run; the binary with --test-threads=<threads> v15_a_write_its_deadline
    #   database::slot:: v14_a_slow a_slot_holder_whose_write_fails; cargo clippy -p amphora --all-targets --locked -- -D warnings;
    #   bash scripts/check-production.sh
    # part 2: each arm's binary --exact database::replicated::tests::a_local_read_holds_off_the_logs_restart_and_one_after_waits_for_it
    # part 3: (cd bench && cargo build --release); ladder16.sh <that amphora-bench> <out>/ladder <rounds>

# Lane V18 (commits) on adacachy2: a lost forward is bounded by the close behind it; V17's checks stand as recorded (2026-11-24, 04:59Z to 05:5xZ)

Ada's goal is replicated commits about 10x faster than rqlite and dqlite, with no write refused under load. This
lane's deliverable: lane V17's checks finished (its record, committed here as it was left), and lane V15's gap 2, a
forward lost on the way, bounded by an event, with the cost of the bound on the normal path measured and stated.

## Losses first

1. **Done-when 3 is not met: the replicated suites' three rounds and the ladder did not run.** adacachy2 had 0 or 1 idle
   core (32) from 05:13Z on: cpus=8 at 05:13Z, cpus=4 at 05:13Z (accepted, r363) and at 05:17Z, 05:18Z (refused: 0 to
   1 idle), cpus=2 at 05:17Z and 05:18Z (refused: 1 idle) and 05:25Z (accepted, r403), and at 05:26Z (suites, refused:
   0 idle) and 05:26Z (release bench and ladder, refused: 0 idle). Nothing after that ran before this record's end.
   Before merging, the touched tests passed, and the suites' 3 rounds and the ladder at 32 and 128 spread writers on the
   landed tree are owed (they go first in the next run).
2. **`a_local_read_holds_off_the_logs_restart_and_one_after_waits_for_it` failed once in the one focused run on the
   landed change** (r403, b7c93867, 60.8 s: the Timeout of `read_local`'s unwrap at replicated.rs:17488:14). V17's alone runs
   (r188) had it fail 5 in 16 on main fac69083 and 5 in 16 on the merge alike, so it is main's own; not traced here, and
   nor was the alone run repeated with the change. It passed once on ee640f36 (r363).
3. **The normal path's cost is measured in the driven harness, in records a write (no loss), not as latency or CPU, and
   not on a real network**; the 10 ms-tick stage run measures the lost forward only, and the owner now drives again at
   once behind a follower's forward (V16's rule for a close), which is not timed. Latency and CPU of the normal path:
   not measured.
4. **The second design (b) was not built**; its cost is stated from the size of what it would keep (4,133 entry bytes of
   the forward, a write in flight) and from its not adding records, not from a run. It also needs the host transport's
   report of a reset or a refused record, which no host gives today (the API between an owner and its host drains and
   receives records; nothing reports a loss).
5. **A loss that takes both a forward and its close is not bounded by an event** (a link reset between them, a whole
   `Ready` lost): the deadline the holder's barrier stated, or the host's detector, ends the slot as before.
6. The bound assumes the link delivers in order (the design's own premise, replication.md): a close that overtook its
   forward would end the slot `released` early, and that forward would append on a passed base (refused stale and
   asked again), as any early end would.

## V17's checks

V17's own record is above (committed here as it was left, its LADDER_LOSS and AB_SECTION placeholders included). V17's
checks 2 and 3 did run: the alone test, 16 pairs on main fac69083 and the merge 1e07a639, 5 failed in 16 on each (r188);
the ladder on main c39adf56, 12 runs, no refusal, and 28 writes past their 10 s deadline at 128 writers (r268: persisted
9, replicated 19, at cpus=2 and load 37 to 46, where V14's runs at cpus=4 and load 39 to 77 had none). Neither is traced
here: no A/B of that ladder against the tree without the fix was run.

## Gap 2: a lost forward is bounded by the close behind it

A follower forwards its write inside its write slot (V8); the leader holds its own and the other members' writes while
the slot holds. A forward lost on the way left the slot to end by the write's stated deadline (V14) or the detector,
which suspects no member a live link loses one record of. V15 measured that on real time: the other writes waited the
holder's whole 10 s (10,027.9 ms, r65). Built: design (a). The follower sends a close, a barrier marked a read's, right
behind the forward, in the same drive and so on the same ordered link (`Actor::close_behind`). The leader steps a
marked barrier at once and ends the slot its member holds `released` (`Slots::pass`). A close behind a forward that
arrived has nothing to end, and its read's answer is read by nothing, so the leader consumes it unstepped (`follows`:
the identity of the last forward it took from the member, the next serial in the same term and incarnation, a read's
mark), so the close costs its one record and no read round.

Red and green (driven, no clock moved; in each class): `replicated::v18_a_lost_forward_is_bounded_by_the_close_behind_it`.
The red arm is the same tree with the close not sent (df0bfe01, r363): "the leader's write, with member 3's forward lost
(1 lost) and 0 closes seen", its answer none, `None` after ROUNDS. Green on ee640f36 and b7c93867 (r363, r403), with
`v18_a_close_behind_a_stepped_forward_costs_no_read_round` and the pure `v18_a_close_follows_its_own_forward_only`.

**Stages on real time (V15's diagnostic, its teardown's drain fixed here; three members in one process, the pump's
`losing` hook losing member 3's first forward to the leader, 10 ms ticks; times from member 3's call; both classes).**

| class | stage | main 6d8e7dcd + diagnostic (r308, 05:03:41Z, cpus=8, load 24 to 37) | the close, not consumed (r363, 05:15:05Z, cpus=4, load 30) | the close, consumed (r403, 05:27:56Z, cpus=2, load 50) |
|---|---|---|---|---|
| persisted | forward lost at | 2.46 | 2.84 | 13.67 |
| persisted | slot ends | 10,003.78 (expired) | 2.92 (released) | 13.75 (released) |
| persisted | the leader's other write answered | 10,021.16 | 12.29 | 29.59 |
| persisted | member 2's write answered | 10,051.83 | 35.98 | 69.53 |
| persisted | member 3's own write (its entry never reached the leader) | 10,000.03 UnknownOutcome | 10,000.03 UnknownOutcome | 10,000.02 UnknownOutcome |
| replicated | forward lost at | 4.10 | 2.63 | 6.45 |
| replicated | slot ends | 10,003.63 (expired) | 2.75 (released) | 6.70 (released) |
| replicated | the leader's other write answered | 10,017.28 | 17.94 | 30.40 |
| replicated | member 2's write answered | 10,077.74 | 57.92 | 100.02 |

The wait of the others after the loss is the close's delivery (0.1 ms after the loss) then the rest of their own path, where it
was the holder's whole 10 s. Slot ends counted over the run (forwarded, expired, released): 1, 1, 0 on main and 1, 0, 1
with the close in each class (one other slot forwarded).

**What each design costs the normal path** (driven harness, no loss, clocks still: 20 writes one at a time at member 3,
counts by record type through the pump, main's behaviour against the close; records a write, both classes):

| arm | persisted | replicated |
|---|---|---|
| main (the close not sent) | 15.00 | 16.90 |
| the close, the leader stepping it (a read round) | 24.40 | 26.00 |
| the close, consumed at the leader | 16.00 | 17.90 |

- The close is 1 record a write (the follower's MsgReadIndex to the leader: 2.00 a write against main's 1.00; its
  context's entry, 33 bytes). Stepped, it cost 9.4 records a write: the read's heartbeats and their answers, its
  response, and 0.8 to 0.9 more MsgAppend a write (a read round's effect, not traced).
- Design (b) (a forward sent again when its transport reports a reset or refused record) adds no record on the normal
  path but keeps the forward's entries for each write in flight: 4,133 entry bytes a write here (the MsgPropose counted),
  measured as a size, not a run, and needs the host's report (no host gives one) and a typed drop of a copy at the leader.
- Why (a): it needs nothing of a host, no wire change (a leader of any version, or one that missed the forward, steps
  the close as the read it is), and its cost is 1 small record a write; (b)'s cost is bytes held and a host API. (b) bounds a
  loss of a whole Ready, which (a) doesn't.

## Runs

All on adacachy2 (00aad8daff2fa3c9daa65d53c425d450), sync false, GitHub clone ~/.cache/amphora-perf/commits-remote (a second
worktree, -b and -c, for runs that overlap), CARGO_TARGET_DIR ~/.cache/amphora-perf-target/commits, CARGO_INCREMENTAL=0,
CARGO_BUILD_JOBS = the needs' cpus.

| run | needs, load, time | tree | result |
|---|---|---|---|
| r308 | cpus=8, 05:02:45Z to 05:04:03Z, load 14 to 37 | 01b45d40 (main 6d8e7dcd + V15's stages, teardown drained) | built 56 s; the stage runs (before) |
| r357, r370, r372, r376 | 05:13Z to 05:18Z | none | refused: 0 to 2 of 32 cores idle |
| r363 | cpus=4, 05:14:02Z to 05:16:16Z, load 32 to 50 | ee640f36; red df0bfe01; stages 1b76bdeb | 20 of 20 focused passed (v18, v15, slot, v14, holder, a_local_read); red: v18 failed; stages after; clippy of amphora finished |
| r379 | cpus=1, 05:18:47Z to 05:20:46Z | 3807c7c7 and e27752f9 | the cost arms (the close not consumed, and not sent) |
| r403 | cpus=2, 05:25:00Z to 05:28:30Z, load 54 to 58 | b7c93867; cost e6ed7152; stages cfdae334 | focused: 23 passed, 1 failed (a_local_read_holds_off, main's own flake); cost (consumed); stages; clippy exit 0 |
| r404, r406 | 05:26Z | suites (3a58a13f and b7c93867); release bench and ladder | refused: 0 of 32 cores idle |

## Commands

    # every build: cd <clone>; git checkout <sha>; cargo test -p amphora --lib --locked --no-run
    v18-tests-<sha> --test-threads=N v18_ v15_a_write_its_deadline database::slot:: v14_a_slow a_slot_holder_whose_write_fails a_read_barrier a_local_read_holds_off
    v18-tests-<sha> --ignored --nocapture --test-threads=1 v18_normal_path_cost        # cost.py inserts the test into replicated.rs
    v18-tests-<sha> --ignored --nocapture --test-threads=1 v15_stages                   # the stages
    cargo clippy -p amphora --all-targets --locked -- -D warnings
    # owed, not run: three rounds of v18-tests-<sha> --test-threads=2 database::replicated:: replication:: on main 3a58a13f and on the landed change,
    # and (cd bench && cargo build --release); ladder16.sh <that amphora-bench> <out> 3 (V16's script, 32 and 128 spread writers, both classes)
