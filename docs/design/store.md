# The store's commit pipeline and readers (CP-T1.1)

`crates/amphora/src/store.rs`, `crates/amphora/src/turso.rs`. Four decisions, each with its sources,
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
there. With the flusher, the writer is busy about a third of the time and a write's p50 is
5.4–6.1 ms, under two of that disk's flushes. Turso's `OFF` mode was measured and refused: a
checkpoint under `OFF` never records what it copied as safe, so the log never starts again and every
checkpoint copies all of it again (130 writes/s).

**Falsified by.** An acknowledged write missing after a crash or power cut (T-T1.1.1: crash
injection at every write, 10,000 seeds; CP-T1.4's power-cut harness).

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
connection of its own copies the log into the file every 25 ms without waiting for anyone
(passive). A log only starts again once nothing reads from it, which readers that never pause
prevent: so once the log holds `LOG_FRAMES` (64 MiB of 4 KiB pages), the writer starts it again
between commits, holding the readers and the checkpointer back for the moment it takes (what little
the checkpointer left, and a flush of the file).

**Sources.** Checkpoint starvation under continuous readers: sqlite.org/wal.html §"Avoiding
Excessively Large WAL Files". Turso's checkpoint modes and the writer's restart
(`vendor/turso/core/storage/wal.rs`, `CheckpointMode`, `try_restart_log_before_write`).

**Evidence.** Checkpoints inline on the writer: write p99 23 ms, p99.9 28 ms. Off the writer with
passive checkpoints alone: write p99 18 ms, but the log grew past 2 GB in 30 s, and past 700 MB with
restarts asked of a reader-busy log. With the writer starting it again and the readers held back:
the log under ~90 MB, write p99 18 ms; with the checkpointer every 25 ms instead of 250, write p99
11–12 ms and p99.9 25–26 ms (the restart has little left to copy).

**Falsified by.** A log that grows past its bound under writes and reads that never pause (the
store's tests), or a write p99.9 a restart's pause explains.

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
