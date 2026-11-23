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
