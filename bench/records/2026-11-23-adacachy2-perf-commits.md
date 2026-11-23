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
  ReadIndex quorum and promises nothing about the log. Vendored hyper-raft as well. Check first whether
  `HeartbeatAnswers::Position` is on for Amphora: a position must then name the durable end, not the last index.

Either alone leaves the other flush in the path. Neither changes what a commit or an answer promises.

## Commands

    # on adacachy2: kiln run, on 00aad8daff2fa3c9daa65d53c425d450, sync false, cwd ~/.cache/amphora-perf/commits-remote
    (cd bench && CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=~/.cache/amphora-perf-target/commits cargo build --release)   # r820, 8m46s
    python3 -I bench/thread_sample.py ~/.cache/amphora-perf-bin/commits/7f2650a9 29490 DIR W leader 5 2 3 replicated   # r851, cpus=4,mem=4g
