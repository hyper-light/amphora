# The store's commit pipeline and readers (CP-T1.1)

`crates/amphora/src/store.rs`, `crates/amphora/src/turso.rs`. Its decisions, each with its sources,
its evidence and what would falsify it (CLAUDE.md §5). The evidence is kiln's K0 workload (1,000
sessions of 100 events, then 30 s of 32 writers appending one event per durable write and 4 readers
reading a session's latest 50 events), run on fedora (32 cores, Samsung SSD 9100 PRO 8 TB under
btrfs with zstd) through kiln's conversation store, which these files take from. Those runs were
at load 8–35 with other builds running: CP-T1.2 measures them again on a quiet machine, best of 3,
into `bench/`.

## 1. A group commit with no timer

**Decision.** One writer thread owns the engine and commits as soon as nothing more waits in its
queue (or a commit reaches its row bound); what came while it committed shares the next commit.
Nothing waits for a timer.

**Sources.** Group commit in DeWitt et al., "Implementation Techniques for Main Memory Database
Systems" (SIGMOD 1984); Helland et al., "Group Commit Timers and High Volume Transaction Systems"
(HPTS 1987), on why a fixed timer adds latency without throughput once commits arrive faster than
they complete.

**Evidence.** kiln's store committed every 50 ms and its writes' syncs each forced a commit of their
own: 195 durable writes/s at p50 159 ms (2026-10-19). With the timer gone and a sync joining the
commit that covers it: 3,613 writes/s at p50 7.3 ms (2026-10-20).

**Falsified by.** A workload where a short timer gives more throughput at no more latency than
committing on an empty queue.

## 2. Durable beside the writer: a flusher

**Decision.** A commit writes the log and doesn't flush it (Turso's `NORMAL`); a flusher thread
flushes the log (`fdatasync`) over and over, and a write is durable, and acknowledged by
`Store::sync`, once a flush that began after its commit ends. A checkpoint still flushes the log
before it copies it and the file after (Turso's checkpoint in `NORMAL`), so nothing the flusher
made durable is lost to a checkpoint. This replaces `SyncMode::Full` (a flush inside every
commit), which the design's README names until CP-T1.8: the guarantee to whoever is told a write is
durable is the same; the flush leaves the commit, so commits no longer wait for the disk one by one.

**Sources.** The flush-pipelining argument in Johnson et al., "Aether: A Scalable Approach to
Logging" (VLDB 2010): commits acknowledge once the log is durable, without holding the log's
writer through the flush. `fdatasync(2)` and `fsync(2)` (Linux man-pages): a flush makes durable
what was written to the file before it began, through any descriptor of the file. Turso's commit
path (`vendor/turso/core/storage/pager.rs`, `CommitState::WaitSync`): it flushes only in `FULL`.

**Evidence.** On fedora's disk a durable 4 KiB write takes 2.6–3.3 ms (`dd oflag=dsync`; no
copy-on-write and direct I/O with `O_DSYNC` measured no faster), the floor of any durable write
there. `bench/disk.py` (2026-10-20, btrfs with zstd on the Samsung 9100 PRO): one thread, 2.72 ms a
write, appended or in place, flushed or written through; two to sixteen threads, on one file or a
file each, 4.9–5.0 ms a write at 200 writes a second a thread. btrfs commits its log one commit at a
time, so a flush that arrives during another's waits for it and then for its own: under
concurrency a durable write there costs about two of the disk's flushes, whoever asks. With the flusher, the writer is busy about a third of the time and a write's p50 is
5.4–6.1 ms, under two of that disk's flushes. Turso's `OFF` mode was measured and refused: a
checkpoint under `OFF` never records what it copied as safe, so the log never starts again and every
checkpoint copies all of it again (130 writes/s).

The flusher flushes through the database's own IO (`Db::flusher`), so a simulator's disk sees every
flush the store counts on.

**Falsified by.** An acknowledged write missing after a crash or power cut (T-T1.1.1: crash
injection at every write, 10,000 seeds, `crates/amphora/tests/crash.rs`; CP-T1.4's power-cut
harness). 2026-10-20: 10,000 seeds, none lost; with the flusher made to acknowledge without
flushing, 157 of 200 seeds lost acknowledged batches, so the test sees a loss when there is one.

## 3. Readers of their own, on the caller's thread, waiting only for their key's writes

**Decision.** A read takes a connection from a pool (opened as reads want them, up to a bound) and
runs on its caller's thread: no thread between a read and its answer. It waits until the writes of
its key handed over before it began are committed, counted per key in shards (handed and committed
counts, each entry numbered so a reader can tell when its entry was settled and replaced), and no
longer: not for other keys' writes, and not behind the writer.

**Sources.** Snapshot reads on a write-ahead log: SQLite's WAL (sqlite.org/wal.html) and Turso's
(`vendor/turso/core/storage/wal.rs`, `begin_read_tx`): a reader sees what was committed when its
read began, and never blocks the writer.

**Evidence.** kiln's store ran every read on its writer thread: reads p50 159 ms under K0's writes
(2026-10-19). On connections of their own: history reads p50 0.11–0.15 ms, p99 0.9–1.0 ms. With no
writes, Amphora's reads (p50 0.050 ms, p99 0.081 ms) beat vanilla Turso's (0.055, 0.091) on the
same reads; with readers on sessions no writer writes (`--disjoint`), p50 0.057 ms against vanilla
Turso's 0.068 while writing 14 times as many rows. What reads of written sessions pay beyond that
is waiting for their own session's commit and reading the pages it changed.

**Falsified by.** A read that misses a write of its key handed over before it began (the store's
tests), or a read that waits for another key's write.

## 4. No checkpoint in a commit's way

**Decision.** The writer never checkpoints (`wal_autocheckpoint = 0`). A checkpointer with a
connection of its own copies the log into the file every `Config::checkpoint_every` (25 ms) without
waiting for anyone (passive). A log only starts again once nothing reads from it, which readers that
never pause prevent: so once the log holds more than `Config::log_frames` (16,384 frames, 64 MiB of
4 KiB pages), the checkpointer copies without pausing until less than a 64th of the bound is left,
and the writer then starts the log again between commits, holding the readers and the
checkpointer back for the moment it takes (what little is left, and a flush of the file). The
checkpointer starts no pass while a restart is pending. At twice its bound the log is started
again however much is left.

**Sources.** Checkpoint starvation under continuous readers: sqlite.org/wal.html §"Avoiding
Excessively Large WAL Files". Turso's checkpoint modes and the writer's restart
(`vendor/turso/core/storage/wal.rs`, `CheckpointMode`, `try_restart_log_before_write`).

**Evidence.** Checkpoints inline on the writer: write p99 23 ms, p99.9 28 ms. Off the writer with
passive checkpoints alone: write p99 18 ms, but the log grew past 2 GB in 30 s, and past 700 MB with
restarts asked of a reader-busy log. With the writer starting it again and the readers held back:
the log under ~90 MB, write p99 18 ms; with the checkpointer every 25 ms instead of 250, write p99
11–12 ms and p99.9 25–26 ms (the restart has little left to copy).

Copying until little is left before a restart (2026-10-20, fedora, 512 writers): the longest restart
went from 69–150 ms to 30–34 ms. The checkpointer starting no pass while a restart waits: the
writer's longest wait for the gate went from 57–61 ms to under 0.5 ms.

What checkpoints still cost (2026-10-20, fedora, 4,000 sessions, 20 s, load 3–7, one run each):
with no checkpoints at all (the log left to grow) 512 writers write 38,178 a second at p99 21 ms,
against 25,500–25,900 at p99 86–89 ms with them; 32 writers write 5,600 at p99 7.9 ms against
4,900–5,100 at 10.7–12.7. Checkpointing every second with a 262,144-frame log takes back most of
that (512 writers: 33,200–36,400 a second, p99 41–47 ms) but raises the longest write at 32 writers
from about 40–90 ms to 97–127 ms, and keeps 10 MB more of log index in memory. The cause, measured
by sampling where the writer and the flusher wait in the kernel (`bench/wchan.py`): a checkpoint's
flush of the file, and of the log before it, are btrfs log commits; the flusher's flush of the log
waits behind them (`wait_log_commit`, once 118 ms), and the writer's next append to the log waits for
the flusher's hold on the log's inode (`btrfs_inode_lock`), so a commit stalls (Turso's COMMIT, up to
97 ms). Starting the file's writeback while the checkpoint copies (`sync_file_range`, as
PostgreSQL's `checkpoint_flush_after` does) didn't shorten the stalls: the copy comes in one burst.
The defaults stay at 25 ms and 16,384 frames until checkpoints stop competing with the flusher for
the disk's log (STATUS.md: one thread making everything durable).

## 5. Page budgets by what a connection does

**Decision.** The writer keeps 1,024 pages of the database in memory, each reader 128 (Turso keeps
2,000, 8 MiB, on every connection). A reader's miss is a read of the file through the system's page
cache; the writer keeps its indexes' inner pages.

**Sources.** Turso's page cache (`vendor/turso/core/storage/page_cache.rs`): one per connection, 2,000
pages by default, kept consistent with the log by the connection itself.

**Evidence.** Amphora's bench (`bench/`), fedora, 2026-10-20, K0's load: at Turso's 2,000 pages a
connection, 78–86 MB resident and reads p99 0.74 ms; at 1,024/128, 47–49 MB and reads p99 0.27 ms
(p50 0.085 ms against 0.101); writes the same. Smaller caches read faster under writes: the pages
writes change are fewer to bring up to date.

**Falsified by.** A workload whose reads slow down past a bigger budget's (a working set of hot
sessions larger than what readers keep, read faster from memory than from the system's cache).

## 6. One flusher

**Decision.** One flusher thread (`Config::flushers`), though the store can run more.

**Evidence.** With two or four flushers a commit landing during a flush starts one of its own; on
fedora's disk the flushes serialize anyway: at 300 writes/s the write p50 stayed at 6.6–6.7 ms with
one, two and four (2026-10-20), and four once held a write 1.1 s.

**Falsified by.** A disk whose flushes overlap (a write's latency then falls toward one flush).

## 7. The writer keeps no statement journal; a commit that fails is retried batch by batch

**Decision.** The writer's connection keeps no statement journal (Turso's
`Connection::set_statement_journals(false)`, Amphora's change to `vendor/turso`): a write statement in
a transaction that fails, or is abandoned unfinished, poisons the transaction, and COMMIT then rolls
it back whole and says so. `Db::transaction` rolls back on any failure, and after a COMMIT that
fails, so nothing a failed statement half-wrote is ever committed, and the next transaction can
begin. A commit that fails is tried again batch by batch, each batch its own transaction: the
batches that commit alone are kept, and one that can't is refused to its sender
(`Refused::Failed`), nothing of it written.

**Sources.** SQLite's statement journal and when it is skipped (`usesStmtJournal = isMultiWrite &&
mayAbort`, `vdbe.c`, `sqlite3VdbeHalt`), which Turso mirrors
(`vendor/turso/core/translate/stmt_journal.rs`): an UPSERT whose DO UPDATE can fail is both, so every
event and session written keeps one, a 4 KiB copy of each page it first touches.

**Evidence.** Amphora's bench (2026-10-20, fedora, 128 writers): the statement journal was 13.6% of
the writer's time applying rows; without it, the writer's µs per row went from 30.6 to 25.4 and
writes per second from 13,580 to 14,405. The store's tests: a batch with a row that fails a CHECK is
refused and writes nothing, the batches committed with it are kept and made durable; a transaction
holding a failed statement is never committed, whether its failure was returned or ignored.

**Falsified by.** A row of a failed batch found after its sender was refused, a row of a kept batch
missing, or a statement's half-write committed (the store's tests; T-T1.1.1's crash injection).

## 8. Durable by ticket

**Decision.** `Store::put` and `Store::put_wait` return a `Ticket`; `Store::durable(ticket)` waits
until that batch's commit is followed by a flush that began after it, and refuses it if its commit
failed or the writer stopped before committing it. `Store::sync` still waits for everything handed
over so far, without saying which batches failed.

**Sources.** Group commit with per-transaction acknowledgement (DeWitt et al., 1984; Helland et
al., 1987): each committer learns its own outcome, not the group's.

**Evidence.** Before tickets, a batch whose commit failed was let go and `Store::sync` reported it
durable: a silent drop, against AC-T1.1.1. The store's tests cover a ticket made durable only after
a flush that began after its commit, and a failed batch's ticket refused.

**Falsified by.** A ticket reported durable whose batch isn't in the database after a crash
(T-T1.1.1), or a failed batch's ticket reported durable.

## 9. A failed flush is the last

**Decision.** When a flush fails, the store makes nothing more durable: the flusher stops, every
sender waiting for a batch to be durable is refused (`Refused::Disk`), and new batches are refused,
until the store is started again on what its disk holds.

**Sources.** Rebello et al., "Can Applications Recover from fsync Failures?" (USENIX ATC 2020):
after a failed `fsync`, Linux file systems may mark the pages they couldn't write clean, so a later
`fsync` succeeds without them. PostgreSQL stops on a failed `fsync` since 2018 for this
(`data_sync_retry`, off by default).

**Evidence.** The store retried a failed flush every 10 ms: a retry that then succeeded would have
acknowledged writes the disk never took. The crash test cuts a disk's power mid-flush: the senders
waiting are refused at once, where before they waited out their deadline.

**Falsified by.** A file system that keeps failed pages dirty and reports a later flush truthfully,
on every platform Amphora runs on (then a bounded retry could be safe).

## Crash injection (T-T1.1.1)

`crates/amphora/tests/crash.rs` runs the store on a simulated disk whose power goes at a change
drawn from each seed's workload (writes, flushes and truncations to the database file and the log).
What a flush made durable survives; each change made since survives whole, torn at a 512-byte
boundary, or not at all, in any combination. The workload hands over 150 batches of 1 to 4 rows, a
tenth of them holding a row the engine refuses, a few at a time so they share commits, with the log
checkpointed every millisecond and restarted past 16 frames. After the restart on what survived:
every batch acknowledged durable is there, every batch the engine refused isn't, every other batch is
there whole or not at all, and Turso's integrity check passes. Each run checks it isn't vacuous: the
power goes out midway in most seeds, acknowledged and refused batches are checked, and checkpoints
and restarts happened. Every gate runs 200 seeds; the full 10,000 run with
`cargo test --release --test crash -- --ignored` (32.7 s on fedora).

## 10. Groups: writes on as many cores

**Decision.** A store can run in groups (`crates/amphora/src/groups.rs`): stores on one node, each
a database file with a writer, a log, a flusher, a checkpointer and readers of its own, keys spread
over them by FNV-1a, so writes go on in parallel. A batch is one group's: a batch with rows in more
than one group is refused (`Refused::Groups`); keeping operations across groups atomic is
CP-T1.10's. A read of a key goes to its group; a read of every key to each group. A database's
number of groups is its own for good. The commit's row bound comes down to 512 (`Config::commit_rows`).

**Sources.** Partitioned single-writer engines: H-Store (Kallman et al., "H-Store: a
High-Performance, Distributed Main Memory Transaction Processing System", VLDB 2008) and VoltDB, one
thread a partition, no latches between them.

**Evidence.** One writer was the ceiling: at 1,000 to 1,000,000 writers it wrote 21,000-23,000 rows a
second, its thread busy throughout (fedora, 2026-10-20). With commits of 512 rows instead of 2,048 (a
commit's pages then fit its writer's cache): 35,000-41,000. In groups, the files without copy-on-write
or compression (`chattr +C`; on btrfs with both, a checkpoint's flush held every log's for 2-4 s),
10,000 writers, one run each: 1 group 34,918 a second (p99 401 ms), 4 groups 105,377 (223 ms), 8
groups 161,789 (207 ms), 12 groups 254,430 (110 ms). At 1,000,000 writers 12 groups wrote 221,417 a
second and 16 groups 320,451 (p50 3.08 s, max 3.82 s: a million writers ÷ the rate), none failed and
none unanswered, the machine's 32 cores busy (load 48). Groups cost at low concurrency on btrfs: at
32 writers 8 groups wrote 3,640 a second at p50 8.7 ms where one group wrote 5,249 at 5.7 ms, eight
logs flushed at once costing two of btrfs's log commits each.

**Falsified by.** A key whose rows are found in another group than its own, a batch spanning groups
written (`crates/amphora/tests/groups.rs`), or a node where groups write no faster than one writer.

## 11. Told when durable

**Decision.** `Store::put_then` hands a batch over with what to tell once it is durable (or why it
won't be). The writer files each committed batch with a sender to tell by its commit, and a notifier
thread tells them once a flush covers their commit; refusals come the same way. No sender's code runs
on the flusher, and a client needs no thread waiting per write: the bench drives a million writers
from 16 threads.

**Evidence.** The store's test: a sender told only after a flush begun after its commit, and a
failing batch's sender told it failed. The bench at 1,000,000 writers on 16 submitter threads, every
write answered.

**Falsified by.** A sender told durable before a flush covering its commit ended, or told twice, or
never.
