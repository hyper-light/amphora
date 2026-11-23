# The store's commit pipeline and readers (CP-T1.1)

`crates/amphora/src/store.rs`, `crates/amphora/src/db.rs`. Its decisions, each with its sources,
its evidence and what would falsify it (CLAUDE.md §5). The evidence is kiln's K0 workload (1,000
sessions of 100 events, then 30 s of 32 writers appending one event per durable write and 4 readers
reading a session's latest 50 events), run on fedora (32 cores, Samsung SSD 9100 PRO 8 TB under
btrfs with zstd) through kiln's conversation store, which these files take from. Those runs were
at load 8–35 with other builds running: CP-T1.2 measures them again on a quiet machine, best of 3,
into `bench/`.

## 1. A group commit with no timer

**Decision.** One writer thread owns the engine and commits as soon as nothing more waits in its
queue (or a commit reaches its row bound); what came while it committed shares the next commit.
Nothing waits for a timer, the flusher included (decision 14).

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
`Store::sync`, once a flush that began after its commit ends. A checkpoint copies only what a flush
has made durable (decision 15) and flushes the file after (Turso's checkpoint in `NORMAL`), so
nothing the flusher made durable is lost to a checkpoint. This replaces `SyncMode::Full` (a flush inside every
commit), which the design's README names until CP-T1.8: the guarantee to whoever is told a write is
durable is the same; the flush leaves the commit, so commits no longer wait for the disk one by one.

**Sources.** The flush-pipelining argument in Johnson et al., "Aether: A Scalable Approach to
Logging" (VLDB 2010): commits acknowledge once the log is durable, without holding the log's
writer through the flush. `fdatasync(2)` and `fsync(2)` (Linux man-pages): a flush makes durable
what was written to the file before it began, through any descriptor of the file. Turso's commit
path (`crates/engine/storage/pager.rs`, `CommitState::WaitSync`): it flushes only in `FULL`.

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
(`crates/engine/storage/wal.rs`, `begin_read_tx`): a reader sees what was committed when its
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
waiting for anyone (passive); since decision 35, only once `Config::copy_after` frames wait. A log
only starts again once nothing reads from it, which readers that never pause prevent: so once the log holds more than `Config::log_frames` (131,072 frames, 512 MiB
of 4 KiB pages), the checkpointer copies without pausing until less than a 64th of the bound is left,
and the writer then starts the log again between commits, holding the readers and the
checkpointer back for the moment it takes (what little is left, and a flush of the file). The
checkpointer starts no pass while a restart is pending. At twice its bound the log is started
again however much is left.

**Sources.** Checkpoint starvation under continuous readers: sqlite.org/wal.html §"Avoiding
Excessively Large WAL Files". Turso's checkpoint modes and the writer's restart
(`crates/engine/storage/wal.rs`, `CheckpointMode`, `try_restart_log_before_write`).

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
The checkpoints stay at 25 ms until they stop competing with the flusher for the disk's log
(STATUS.md: one thread making everything durable).

The bound, alone (2026-10-20, fedora, 32 writers, one group, checkpoints every 25 ms, three runs
each, load 4–9.5; `bench/records/2026-10-20-fedora-32-writers-log-bound-*.jsonl`): each restart
holds the writer 10–14 ms, and a commit at 32 writers writes about 73 frames, so a 16,384-frame log
restarted 27 times in 20 s: p99.9 12.0–15.6 ms, max 27–37 ms. At 131,072 frames it restarted 3–4
times: p99.9 9.1–10.6 ms, max 20–29 ms, 1–2 MB more memory. With no checkpoint at all: p99.9
8.8–11.8 ms, max 12–45 ms (the longest is the disk's: no checkpoint ran). So the default is now
131,072. A log keeps up to twice that many frames of disk (decision 13 keeps its length), and a
restart copies up to a 64th of it: at 1,000 writers and more, the matrix's restarts are to be read
against the 16,384-frame runs before.

## 5. Page budgets by what a connection does

**Decision.** The writer keeps 1,024 pages of the database in memory, each reader 2,000 (8 MiB,
Turso's default for every connection), the checkpointer next to nothing. A reader's miss is a read of
the file through the system's page cache; the writer keeps its indexes' inner pages. Readers kept 128
until decision 18: a reader that cleared its cache whole at every commit gained nothing from a larger
one under writes.

**Sources.** Turso's page cache (`crates/engine/storage/page_cache.rs`): one per connection, 2,000
pages by default, kept consistent with the log by the connection itself.

**Evidence.** Amphora's bench, fedora, 2026-10-20, K0's load, before decision 18: at 2,000 pages a
connection, 78–86 MB resident and reads p99 0.74 ms; at 1,024/128, 47–49 MB and reads p99 0.27 ms.
With decision 18 and 19 (commit f597f5a), 2026-10-21, K0's load (32 writers, 4 readers, 20 s), three runs
each in turn (`bench/records/2026-10-21-fedora-k0-reader-pages-{128,2000}.jsonl`): at 128 pages
reads p99 0.128–0.144 ms, 1.21–1.22 million reads, writes p99 6.5–7.0 ms, 47–51 MB resident; at
2,000 pages reads p99 0.122–0.138 ms, 1.24–1.27 million reads, writes p99 6.3–7.4 ms, 80–90 MB.
The comprehensive suite, each statement on its own, 200,000 rows
(`bench/records/2026-10-21-fedora-suite.jsonl` and `-suite-amphora-2000-pages.jsonl`): at 32
clients, 2,000 pages read `order_by_index` at 1,049,700 a second against 172,600 at 128,
`filter_is_null` 74,100 against 10,500, `range_pk_100` 394,700 against 139,200 and `table_count`
196,300 against 3,162; at 1 client the gains are smaller (`order_by_index` 142,000 against 63,100,
`point_secondary` 319,600 against 271,500). The suite's peak resident set at 32 readers: 494 MB
against 248 MB. Loads before the runs 1.3–6.4 (the K0 runs' own load included).

**Falsified by.** Reads under writes slower with the larger budget; a working set that fits in it
read slower than from the system's cache. Its memory, 8 MiB a reader, is the case for one cache all
readers share.

## 6. One flusher

**Decision.** One flusher thread (`Config::flushers`), though the store can run more.

**Evidence.** With two or four flushers a commit landing during a flush starts one of its own; on
fedora's disk the flushes serialize anyway: at 300 writes/s the write p50 stayed at 6.6–6.7 ms with
one, two and four (2026-10-20), and four once held a write 1.1 s.

**Falsified by.** A disk whose flushes overlap (a write's latency then falls toward one flush).

## 7. The writer keeps no statement journal; a commit that fails is retried batch by batch

**Decision.** The writer's connection keeps no statement journal (Turso's
`Connection::set_statement_journals(false)`, Amphora's change to its engine): a write statement in
a transaction that fails, or is abandoned unfinished, poisons the transaction, and COMMIT then rolls
it back whole and says so. `Db::transaction` rolls back on any failure, and after a COMMIT that
fails, so nothing a failed statement half-wrote is ever committed, and the next transaction can
begin. A commit that fails is tried again batch by batch, each batch its own transaction: the
batches that commit alone are kept, and one that can't is refused to its sender
(`Refused::Failed`), nothing of it written.

**Sources.** SQLite's statement journal and when it is skipped (`usesStmtJournal = isMultiWrite &&
mayAbort`, `vdbe.c`, `sqlite3VdbeHalt`), which Turso mirrors
(`crates/engine/translate/stmt_journal.rs`): an UPSERT whose DO UPDATE can fail is both, so every
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
until the store is started again on what its disk holds. With more than one flusher (decision 6), a
flush's commits are made durable only once it and every flush begun before it have ended well
(`Claims` in `store.rs`): a flush that failed never ends well, so another flusher's success after it
makes nothing durable, however the two flushes overlap.

**Sources.** Rebello et al., "Can Applications Recover from fsync Failures?" (USENIX ATC 2020):
after a failed `fsync`, Linux file systems may mark the pages they couldn't write clean, so a later
`fsync` succeeds without them. PostgreSQL stops on a failed `fsync` since 2018 for this
(`data_sync_retry`, off by default).

**Evidence.** The store retried a failed flush every 10 ms: a retry that then succeeded would have
acknowledged writes the disk never took. The crash test cuts a disk's power mid-flush: the senders
waiting are refused at once, where before they waited out their deadline. Lane W4, 2026-11-23: with
two flushers, each flusher made durable what its own flush covered. The crash test's two-flusher
case (`a_flush_that_succeeds_beside_a_failed_one_acknowledges_nothing_the_disk_dropped`) fails one
flush on the simulated disk, which drops what it was writing, while the other flusher's flush, begun
before the failure, succeeds: the store acknowledged both batches durable, and neither was on the
disk after a restart. Made durable in the order the flushes began, both are refused.

**Falsified by.** A file system that keeps failed pages dirty and reports a later flush truthfully,
on every platform Amphora runs on (then a bounded retry could be safe).

## Crash injection (T-T1.1.1)

`crates/amphora/tests/crash.rs` runs the store on a simulated disk whose power goes at a change
drawn from each seed's workload (writes, flushes and truncations to the database file and the log).
A flush takes time (0.2 ms) and makes durable what was written before it began; each change not
made durable survives whole, torn at a 512-byte boundary, or not at all, in any combination. The
workload hands over 150 batches of 1 to 4 rows, a tenth of them holding a row the engine refuses, a
few at a time so they share commits, with up to three left in flight so commits land while flushes
run, the log checkpointed every millisecond and restarted past 16 frames. After the restart on what survived:
every batch acknowledged durable is there, every batch the engine refused isn't, every other batch is
there whole or not at all, and Turso's integrity check passes. Each run checks it isn't vacuous: the
power goes out midway in most seeds, acknowledged and refused batches are checked, and checkpoints
and restarts happened. Seeds run side by side, each on a disk of its own; one that panics fails
alone. Every gate runs 200 seeds; the full 10,000 run with
`cargo test --release --test crash -- --ignored` (4.7 s on fedora's 32 cores). Before flushes took
time, a flush followed each commit at once, so no checkpoint ever met an unflushed commit, and a
flusher claiming the whole log durable passed 10,000 seeds; it now fails them (decision 15).

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

## 12. Written in place, every page checksummed

**Decision.** On Linux, a new database's file and log are made with btrfs's no-copy-on-write flag
before anything is written to them (`db.rs`, `in_place`; best effort: a file that exists keeps its
way, a file system without the flag declines it). The flag turns off btrfs's own data checksums for
those files, and its compression; Turso's page checksums (its `checksum` feature: an XXH3-64 of each
4 KiB page in the page's last 8 bytes, written with the page and checked whenever the page is read
from the file) take their place, so a page corrupted on disk is refused, never read as data.

**Sources.** btrfs(5), `nodatacow` and the `C` attribute: no copy-on-write, no compression, no data
checksums, for files written in place such as databases and disk images (as systemd-journald does
for its journals on btrfs). Turso's `storage/checksum.rs`.

**Evidence.** Amphora's bench (2026-10-20, fedora, btrfs with zstd, 10,000 writers, commits of 512
rows), the same runs in a copy-on-write directory and one with the flag: 1 group 25,051 against
28,774 rows a second, longest write 1.16 s against 0.57 s; 12 groups 97,150 against 194,941. The
store's tests: a new database's file and log carry the flag where the file system takes it; a byte
flipped in a page of the file is refused with a checksum error when read. The checksums' own cost
is to be measured on a quiet machine.

Crash injection found what checksums cost recovery: Turso reads page 1 from the file to open a
database before attaching its log, and after a power cut during a checkpoint that page can be torn
in the file while the log holds it whole: with checksums 3-12 of 10,000 seeds couldn't reopen
(without them the torn page was read, its header's bytes whole). Opening now checks page checksums
from when the log is attached (Turso, `Pager::defer_checksum`), when what it read before is dropped:
10,000 seeds twice, none failing (2026-10-20).

**Falsified by.** A page corrupted on disk read as data (`tests/files.rs`), a database that a power
cut leaves unopenable (`tests/crash.rs`), or, on btrfs, the flag making writes slower or their tails
longer than copy-on-write.

## 13. The log flushed by its data, and kept at its length

**Decision.** The flusher flushes the log's data (`FileSyncType::DataSync`, added to Turso:
`fdatasync` on Linux and the BSDs, `F_FULLFSYNC` on Apple's systems, whose `fsync` leaves the disk's
cache, `FlushFileBuffers` on Windows). And the log keeps its length when it starts again: the frames
from before the restart stay, to be overwritten in place, as SQLite's are. Turso cut the file back
to its header whenever it wrote a new one, so a log grew by appending after every restart; it now
does that only under its multi-process authority, which checks its snapshot against the file's
length and which Amphora doesn't use. What makes the frames left behind safe is unchanged: the new
header changes the salts and is synced before any frame follows it (`prepare_wal_finish`), and a
recovery stops at the first frame whose salts or checksum don't hold.

**Sources.** fdatasync(2): metadata not needed to read the data back (times) is not flushed.
SQLite's `os_unix.c` (`unixSync` uses `fdatasync` where the system has it) and `wal.c` (a restart's
header is synced before frames follow, ticket ff5be73dee; the log is cut only to
`journal_size_limit`). PostgreSQL's `wal_sync_method` (`fdatasync` by default on Linux), with
segments written once and recycled (`wal_init_zero`, `wal_recycle`), so a commit's flush finds no
new extents or length.

**Evidence.** On fedora's disk (Samsung 9100 PRO, btrfs with zstd) the change is neutral: a
durable write costs the disk's flush however it is made (`bench/disk.py`, decision 2; again
2026-10-20 with 16 KiB writes, load 3: p50 2.74–3.56 ms appended or overwritten, written first
or only allocated, `fsync` or `fdatasync`, with copy-on-write or without). It is the case file
systems that journal metadata reward (ext4, XFS: an `fsync` after an append commits the journal, an
`fdatasync` after an overwrite doesn't), to be measured on such a disk. Crash injection
(`tests/crash.rs`, 2026-10-20): 10,000 power cuts with the frames kept, none losing an acknowledged
batch; with the new header not synced (a mutant), 3 of 10,000 lost acknowledged batches, so the
test reaches frames left from before a restart.

**Falsified by.** An acknowledged batch lost to a power cut (`tests/crash.rs`); on ext4 or XFS, the
log's `fdatasync` no faster than its `fsync`.

## 14. A flush begins as soon as none is running

**Decision.** The flusher begins a flush as soon as something is committed that no flush has begun
for, and waits for nothing else. What is committed during a flush joins the next one. With one
flusher a commit waits for the flush under way, if any, and the next.

**Why.** Nothing in the store waits for a timer (decision 1). Until 2026-11-23 the flusher gathered
instead: when the writer had run out of work during a flush, the flusher waited until the writer
had been idle for a 32nd of a flush, or half a flush had gone by, before it began the next. That
wait was a timer, and its two fractions were chosen, never measured. With one client nothing can
join the flush its only sender waits for, so the wait timed out every time and added its whole
length to every write.

**Sources.** Helland et al. (HPTS 1987; decision 1): a timer adds latency without adding throughput
once commits arrive faster than they complete.

**Evidence.** adacachy2, 2026-11-23, insert_one at 1 client, traced with an `LD_PRELOAD` shim on the
log's write, open, flush and close and on the threads' futex hand-offs (lane W2,
`bench/records/2026-11-23-adacachy2-perf-oltp.md`). Between the log's write and its flush, the
flusher sat in a `FUTEX_WAIT_BITSET` of 191.8 to 192.8 µs that returned timed out, in an insert of
2.9 to 3.3 ms at p50. Outside the flush Amphora spent 292 µs an insert, SQLite 95 µs. The A/B
against the gathering flusher at 1, 32 and 128 clients is in the same record (lane W3).

What gathering bought, and what this decision answers for: fedora, 2026-10-20, 32 writers, one
group (`bench/records/2026-10-20-fedora-32-writers-flush-at-first-commit.jsonl`,
`...-gathering.jsonl`). Flushing at the first commit: 5,134–5,608 writes a second, p50
5.12–5.76 ms, p99 9.3–9.7 ms. Gathering: 7,390–7,605 writes a second, p50 4.06–4.16 ms, p99
7.2–8.1 ms. Writers that each wait for their own write send their next when told. A flush begun at
the first of those covers few of them, and the writers can settle into two halves that each commit
during the other's flush.

**Falsified by.** A flush begun on an event other than the last flush's end (never a timer) that
gives more writes a second at 32 writers and more, with no longer a write's p50 at one.

## 15. The checkpoint copies what the flusher made durable

**Decision.** Before a flush the flusher captures the log's header-object identity, generation
(its header's `checkpoint_seq`) and last committed frame together with any completed-flush proof,
under one shared-log guard (`Database::wal_prefix`). If that exact prefix is already known durable,
it owes no additional log sync. Otherwise it syncs the log and publishes the captured prefix only
after success (`Database::wal_prefix_durable`); a stale or foreign token is ignored. The checkpointer
copies only up to the proved frame (`Database::wal_durable_frame`, a passive checkpoint's upper
bound), and its own flush is omitted only for a covered prefix (`WalCoordination::durable_through`).
With no mark yet, a checkpoint copies everything and flushes the log first, as before. No local
proof is accepted or reused under the multi-process authority, whose log other processes write.

A normal log restart clears its proof. Reusing the 32-bit generation after wrap also renews header
identity. External replacement always renews header identity and clears proof, even if restored
bytes reuse the same generation and frame numbers; a pre-replacement sync cannot then publish a
proof for the replacement. The old `wal_end`/`wal_durable` tuple API has no object identity: after
external replacement or generation wrap, its proof publications are conservatively ignored for
that database instance, so legacy callers may incur extra checkpoint syncs. Typed publication
continues to work. External file replacement itself requires writers, flushers and checkpoints to
remain quiescent until reload completes; proof invalidation does not make file replacement atomic.

**Sources.** Turso's checkpoint (`CheckpointState::SyncWal`): a frame copied into the file must
be durable in the log first, or a crash could keep the copy and lose the frame, leaving a file that
matches no committed prefix. fdatasync(2): a flush makes durable what was written before it began.
Within a generation a committed frame's slot is never written again (`frame_cache_high_water`).

**Evidence.** fedora, 2026-10-20, 32 writers, one group, three runs each of one binary with the
mark on and off, alternating, load 5.6–8.5 (`bench/records/2026-10-20-fedora-32-writers-mark-*.jsonl`):
a checkpoint pass took 10.9–12.2 ms against 12.6–14.1 ms (the log's flush gone; the file's flush
is most of what remains), and the checkpointer flushed the log 27 fewer times a second. Writes
didn't move beyond the noise: 7,116–8,095 a second against 7,096–7,877, p99 6.5–8.3 ms against
6.5–8.2. A test holds a flush and commits while it runs: the checkpoint after it leaves that commit
uncopied (`tests/crash.rs`, `a_commit_that_lands_during_a_flush_is_not_counted_durable_by_it`). It
fails with the flusher noting the log's end after its flush instead of before, and with a flusher
claiming the whole log durable, which also fails crash injection (integrity check and checksum
failures in 10,000 seeds); the real flusher passes both.

**Candidate validation (UNRUN).** AC-T1.4.4:
`restored_wal_requires_a_fresh_proof_before_checkpoint_can_omit_sync` replaces a WAL with different
contents at the same numeric prefix, attempts stale, foreign and legacy proof publications, and
requires a refused WAL sync to stop checkpoint before backfill. A fresh successful sync and typed
publication must then permit backfill without another WAL sync. This proposed regression and the
candidate changes have not been compiled or run; the historical measurements above establish no
latency claim for this optimization.

**Falsified by.** A required WAL sync omitted without a matching completed-flush proof, a checkpoint
that copies a frame no flush made durable (the tests above), or an acknowledged batch lost or torn
by a power cut during checkpoints (`tests/crash.rs`).

## 16. The log packed: a commit's pages compressed into records

**Decision.** A log can be packed (`Db::pack_log`, `Database::wal_pack`; the format in
`crates/engine/storage/packed.rs`). Amphora doesn't pack by default: on the one bench run so far it
cost writes at 32 writers (Evidence), so it waits on the comprehensive bench to show where it helps
before it is turned on. In a packed log a commit's pages, or a
spill's, become entries (decision 17 says which are whole images and which are deltas), cut into
records of up to 256 KiB, each record's entries compressed together with LZ4, or kept as they are
where that doesn't make them smaller, and laid into as many of the log's fixed-size frame slots as
it takes. A record's first slot carries its slot count where a frame carries its page number, the
slots after it a continuation mark, and its last slot the commit's database size when it ends a
commit; the slots' checksums chain as frames' do, and every page of a record maps to its first
slot, so frame numbers keep their meaning for read marks, backfill, the durable mark (decision 15)
and restarts. The log header's file format says which a generation is, and a generation keeps the
format it began with, so a log is always read as it was written and a page-a-frame log from before
reads as ever. Packed generations begun since 2026-11-11 carry 0x414d_5004 (decision 17's chains
ended by their bytes), mixed ones 0x414d_5007 (decision 39's layout, its chains ended so too);
0x414d_5002, 0x414d_5003 and 0x414d_5005 (a mixed generation of chains ended by their bytes in
decision 37's layout) are read and continued as before, and an older binary refuses the new values
by name; to downgrade, checkpoint first. Records are kept decoded (a page cache's worth of bytes,
`packed::decoded_budget` (decision 39), the oldest dropped first past that; the
writer keeps those it writes), the checkpoint reads a record once for all the pages it copies from
it, and recovery reads record by record: a record whose slots don't all hold ends the log there, as
a torn frame does; one whose slots all hold but that doesn't decode refuses the log, since no torn
write leaves that. The raw frame interface (Turso's sync's) and the multi-process authority refuse
a packed log.

**Why.** At 10,000 writers in 12 groups a 200-byte row cost about 1.8 page frames (7 KiB) of log,
about 1.7 GB/s, and the checkpoint copies besides: the disk's write bandwidth bounded the writes.
Those pages compress: in a sample of 30,000 frames of that run, 123.6 MB of whole pages took 41.2
MB compressed (3.0x, zlib at level 1 standing in for LZ4).

**Sources.** The LZ4 block format (`lz4_Block_format.md`, lz4/lz4 on GitHub): a match reaches at
most 65,535 bytes back, so records much larger than 64 KiB compress little better while a read
decodes all of one. PostgreSQL's `wal_compression` (its documentation, "Write Ahead Log",
`runtime-config-wal`): the full-page images in its log compressed with pglz, LZ4 or zstd, for the
same reason. SQLite's WAL format (sqlite.org/fileformat2.html, "The WAL File Format"): the frame
header, the checksum chain from the header's salts, and recovery stopping at the first frame that
doesn't hold, all of which a record's slots keep.

**Evidence.** The bench, fedora, 2026-10-20, 32 writers in one group, one run each at load 2.9–3.0
(`bench/records/2026-10-20-fedora-packed-log.jsonl`): page-a-frame 7,903 writes a second, p50 3.97
ms, p99 6.5 ms, p99.9 9.6 ms; packed and compressed whole (this decision alone) 6,216, 4.93, 8.8,
10.4; with deltas (decision 17) 5,887, 5.12, 9.0, 14.2. The process wrote 23.4, 18.6 and 18.2 KB a
row: most of it the checkpoint's copies into the file, which packing doesn't shrink. It used 0.58,
0.73 and 0.78 ms of CPU a row. A profile of the deltas' run (load 18) finds LZ4's compression 90%
of what preparing records costs the writer, yet the writer is on a CPU 3% of the run, while its
commits took 2.3 s longer in all over 20 s: most of the cost is a wait not yet found. At 10,000
writers only page-a-frame ran (234,071 writes a second, 21 KB written a row) before other work
loaded the machine. The engine's tests (`crates/engine/tests/packed_log.rs`): commits of
one page, of many records and of more than one batch (1,024 pages), compressed and stored records,
read back on a connection with nothing cached, after a checkpoint, in the next generation and after
recovery; a transaction spilled into the log and rolled back, then one committed; a generation
keeping its format across a change of the setting; a record cut short taking only its commit; and a
record whose checksums hold but that doesn't decode refusing the log. Crash injection
(`tests/crash.rs`, 10,000 power cuts, 2026-10-20): none lost an acknowledged batch on packed
records; with a commit's size on a record's first slot instead of its last (a mutant), 4,318 of
10,000 did, and with continuation slots' checksums unchecked in recovery, 60 left a log refused as
corrupt (so the cuts land inside records).

The comprehensive suite's writes, fedora, 2026-10-22, packed (with deltas) against page-a-frame,
one binary (4a60ada), each run started at load under 2, one round each (checks, not recorded runs):
at 32 clients `insert_batch_100` 392 → 44 statements a second (p99 172 → 705 ms),
`delete_range_insert` 2,800 → 1,011, `update_plain` 7,787 → 6,134, `upsert` 7,531 → 6,288,
`update_indexed`, `update_text` and `txn_read_write` 12% fewer; at one client `insert_batch_100`
159 → 96, the single-row writes as before. By the falsifier below it is not the default: packed,
a statement that writes hundreds of pages waits on something page-a-frame doesn't, the wait not yet
found above.

**Falsified by.** A row read back other than as written, or an acknowledged batch lost or torn by a
power cut (`tests/crash.rs`, every other seed packed; `crates/engine/tests/packed_log.rs`); as a
default, fewer writes a second or longer tails packed than page-a-frame on the comprehensive bench.

## 17. Only the bytes that changed: deltas in the packed log

**Decision.** In a packed log (opt-in, decision 16) a page whose last version is in the log's
current generation is written as the bytes that changed since (a delta: runs of offset, length and
bytes, runs closer than a run's header merged) while the deltas since its last whole image, this
one included, take less than a page; otherwise its image (2026-11-11: this rule replaces a picked
8 deltas in a row and a picked half a page a delta). The depth field's width, 255
(`packed::MOST_DELTAS`), is the format's bound beside it. The base
is what the page's buffer held when the page was last made dirty: `Pager::add_dirty` keeps a copy
while the buffer holds a version as written to the log (`Page::logged`, set when a commit or a
spill is published), and the delta is made only while that version is still the page's latest
frame in the log's current generation. The page's checksum (its last 8 bytes, which its buffer
doesn't keep up to date) is carried in every delta. A read rebuilds a page from its chain, its
entry at the frame read back through its deltas (each the page's previous frame) to its image,
every record read once and kept decoded, and checks that the depths count down to the image; the
checkpoint reads the same way. Under a page codec (encryption) pages are written whole.

**Why.** In the sample of decision 16, 83% of the frames were later versions of pages already in
the log, changed by 311 bytes on average (p50 48, p90 326, p99 3,735). Deltas merged and compressed
took 9.7 MB where whole pages took 123.6 MB and compressed whole pages 41.2 MB (12.8x against
3.0x). Deltas against the database file's copy of a page's first version in a generation (10.1 MB)
gained nothing over compressing that version whole, so first versions stay whole, and no chain
reaches outside the log.

**Sources.** ARIES (Mohan, Haderle, Lindsay, Pirahesh and Schwarz, ACM TODS 17(1), 1992): redo
records describe the change made to a page, not the page. PostgreSQL's `full_page_writes` (its
documentation, `runtime-config-wal`): a page's first change after a checkpoint logs the whole page,
later ones only the change, the split used here at a generation's start.

**Evidence.** The engine's tests: a delta turns a page into its next version, the checksum's bytes
are carried whole even where the buffer's stale copy matches the new one (a test found that case:
a run merged with the carried tail had dropped its first byte, and the page's checksum refused the
rebuilt page), chains of up to 8 deltas rebuild every version, damaged records are refused, and
(`packed_log.rs`) rows changed commit after commit are logged mostly as deltas, at depths up to 8
and never past it (to 2026-11-11; the chain's bytes bound it since, below), and read back on fresh connections, after a checkpoint and after recovery. A
model test there (random inserts, changes and deletes with savepoints rolled back amid spills, 8
seeds, both formats) found that a balance renumbers its siblings' page objects, so one could carry
another page's logged version: every page of a record shares its frame, so the check that the
base is the page's latest frame passed, and a delta was made against the wrong page. A renumbered
page now holds no logged version (`PageInner::set_id`); the test fails without it.

Crash injection (`tests/crash.rs`, 10,000 power cuts, 2026-10-20): none lost an acknowledged batch
with deltas. Mutants: the checksum's bytes not carried whole, 1,553 of 10,000 failed (checksum
mismatches on rebuilt pages); chains never ending in an image, 29 (a chain past what a read looks
back through); deltas applied newest first, 9,552. With the check that a base is the page's latest
frame removed, no test fails: no path found leaves a page holding a version that isn't its latest
(a savepoint's rollback restores pages as new objects, and a spilled page is journaled first), so
the check stays as a guard. The log's bytes a row and what reads pay for chains: to be measured
beside decision 16's.

**Where a chain ends (2026-11-11).** Ada: "We don't tolerate arbitrary constants, learned and stat
derived values only." The chain had ended at a picked 8 deltas, each delta at a picked half page.
It now ends where its own bytes say. Each further delta is a choice between renting and buying, the
ski-rental problem (Karlin, Manasse, Rudolph and Sleator, "Competitive snoopy caching",
Algorithmica 3, 1988). A delta costs the log its runs, and each later read of the page applies them
again. An image costs a page once and resets the chain. Buying when the rented bytes would reach the
purchase is the deterministic rule whose cost is within twice the best in hindsight, with no
constant in it. The reader then applies under a page of deltas to an image, so a rebuild decodes
less than twice an image's bytes. The writer keeps a chain's bytes beside its depth (`Logged::chain`,
packed above the generation in `PageInner::logged_as`). The engine's test
(`packed_log.rs`, `pages_changed_again_and_again_*`) makes the same rows change a byte thirty times
and checks the log holds chains deeper than 8 (red at the old bound: "chains of small deltas end at
8") whose deltas' bytes, page by page, stay under a page.

Chains past 8 come with new header format values (Amphora agent, 2026-11-12; CLAUDE.md §3, formats
change append-only): a generation begun now is `packed::PACKED_CHAINS_FORMAT` (0x414d_5004) or
`MIXED_CHAINS_FORMAT` (0x414d_5005; since decision 39 a mixed generation begins
`MIXED_FRAMES_FORMAT`, 0x414d_5007, whose chains are ended by their bytes too). A generation begun under 0x414d_5002 or 0x414d_5003 keeps its
format, and its chains keep that format's bound of 8 (`packed::most_deltas`), so a binary from
before still reads it. A binary from before refuses a log of the new values as a format it doesn't
know, naming the value ("the log's format 0x414d5004 isn't known"), never as corrupt. The remedy for
that refusal, a downgrade, is a checkpoint that empties the log on the newer binary first.
`a_generation_keeps_the_chain_bound_of_the_format_it_began_with`: a generation begun now takes
0x414d_5004 (red: 0x414d_5002), and one restamped 0x414d_5002 keeps its format, with chains of at most
8.

What the bytes rule doesn't price is the number of records a cold read decodes: a chain of small
deltas may reach 255 records back, each read once and kept decoded (within `packed::decoded_budget`,
decision 39). If a
cold read's latency on such a chain beats the image's bytes as the cost, the rule must count
records too.

**Falsified by.** A page read back other than as written; a chain whose deltas take a page or more,
or deeper than 255; an acknowledged batch lost or torn by a power cut; writes or reads slower with
deltas than with compressed whole pages at any writer count; a cold read of a page at the end of a
long chain of small deltas slower than its read with chains cut at 8 (the record count, not the
bytes, then being the cost).

## 18. A reader forgets only what was written since it last read

**Decision.** A connection starting a read transaction after others committed forgets only the
pages those commits wrote (`Pager::begin_read_tx`, `Wal::begin_read_tx` answering a `ReadChange`),
not its whole page cache. The log lists the pages its recent frames hold, in frame order, beside
its frame index and under its lock (`crates/engine/storage/changes.rs`: at most 65,536 entries, 1
MiB, the oldest half dropped past that). The list vouches only for frames it saw appended one after
another in the current generation: a reader whose last read was in another generation (the log
restarted), or before what the list holds (dropped for the bound, or appended past a gap, as frames
recovered or restored whole are), or behind more changed pages than its cache holds, forgets
everything, as before. A checkpoint's backfill alone changes no page's contents and forgets
nothing. A page a cursor still pins, or one being read, can't be dropped alone, so then the whole
cache is cleared. Under another process's authority (the shared-memory index) no list is kept, and
readers forget everything.

**Why.** SQLite's pager, and Turso's, cleared a connection's page cache whenever the database had
changed since its last read (the comment it came with: "we can probably do better with more
granular invalidation"). Under writes every read began cold, and a larger cache only cost more to
clear and fill: that is why decision 5's readers keep 128 pages. The comprehensive suite
(`bench --suite`) shows what a small cache costs reads. A reader caching 20,000 pages reads the
suite's `order_by_index` at 139,710 a second, against 45,822 at 2,000 pages and 43,783 at 128;
`point_secondary` 378,174, against 306,489 and 229,422 (fedora, 1 client, load 8: a check, not a
recorded run). With this decision a reader's cache outlives commits, so decision 5's budget can be
chosen for what reads need.

**Sources.** SQLite's pager (`src/pager.c`): `pagerBeginReadTransaction` resets the cache when the
log's read reports a change (`sqlite3WalBeginReadTransaction`), which this replaces. PostgreSQL's
shared buffers (its documentation, "Resource Consumption", `shared_buffers`): pages stay cached
across transactions, and a page changed is replaced alone.

**Evidence.** `crates/engine/storage/pager.rs`,
`a_reader_forgets_only_the_pages_written_since_its_last_read`: a reader caching a 400-row table
keeps every page but the one a one-row update wrote and reads the new row; it forgets everything,
another table's pages too, when a commit writes more pages than it caches, and after a checkpoint
that restarted the log.
`crates/engine/tests/reader_cache.rs`: 400 random steps of transactions (one row to hundreds, rows
growing into overflow pages and shrinking, inserts, deletes and rollbacks), checkpoints of every
mode and reads, 12 seeds, page-a-frame and packed logs, readers caching 200 pages (the least a cache
keeps) and 2,000: every
read is the model's. With a listed change forgotten not at all (a mutant) both fail, the model at
seed 0, step 4; with a range's last frame left out (a mutant), the model fails at seed 0, step 11.

**Falsified by.** A read returning a row as it was before a commit its read transaction started
after; reads under writes slower than when every commit cleared the cache.

## 19. The page cache evicts the oldest page not looked at again

**Decision.** A page new to a connection's cache joins at the head of its queue, and the clock
hand sweeps from the tail toward the head (`crates/engine/storage/page_cache.rs`), as SIEVE has
it: the hand evicts the first page it finds unmarked, taking a mark off each marked page it
passes, so a page new to the cache is examined after every older one. Turso's cache inserted a new
page just after the hand, where the sweep looks next.

**Why.** Inserted there, a page read once was the next the sweep examined: with the cache full,
each miss evicted the page the previous miss brought in, the pages cached before stayed, and a new
working set never settled unless each of its pages was looked at again before the next miss. The
suite found it: the same reader running `point_secondary` and then `order_by_index`, whose working
set is about 600 pages, read `order_by_index` at 49,044 a second with 2,000 pages cached, against
151,719 from an empty cache (fedora, 1 client, load 6: checks, not recorded runs). Readers live
across statements in a store, where Turso's bench target opens connections afresh.

**Sources.** Zhang, Yang, Yue, Vigfusson and Rashmi, "SIEVE is Simpler than LRU: an Efficient
Turn-Key Eviction Algorithm for Web Caches" (NSDI 2024): new objects are inserted at the head, and
the hand moves from the tail toward the head, evicting the first object not visited since it last
passed.

**Evidence.** `page_cache.rs`, `a_new_working_set_settles_in_a_full_cache`: a cache of 100 pages
full of pages read once, then a working set of 40 read round after round: every read after the
first round a hit; with the insertion after the hand, 360 misses in the 9 rounds after the first,
every read. Two of Turso's tests asserted the old order (the newest page evicted first) and now
assert SIEVE's. With it, the same reader reads `order_by_index` after `point_secondary` at 147,133
a second with 2,000 pages, and at 72,402 with 128 (45,128 before).

**Falsified by.** A working set that settles in the cache yet misses; a hot page evicted before a
page read once.

## 20. Pages one connection read in, every connection reads from memory

**Decision.** A database can share the pages its connections read (`Database::share_pages`,
`crates/engine/storage/shared_pages.rs`). A page a connection misses in its own cache is looked up
there before it is read: under the version it is (a frame of the log in the log's generation, or
the file's), its image as committed, kept by whichever connection read it in first. The images
never change: only a connection in a read transaction and holding no write lock shares, so no frame
of a write in progress is kept, and a file version is dropped before the file's page is written or
the file cut. A connection about to write a shared buffer copies it first (`Pager::add_dirty`, which
every write goes through before it touches a page: it keeps the page's contents for savepoints).
At most the pages it was made for are kept, in 64 shards by page number with a lock each, evicted
by CLOCK. Refused under another process's authority (its writes are unseen), with MVCC and with a
packed log. One pool can serve a process's databases, a budget for them all
(`Database::share_pages_in`, `Db::share_pages_in`): each database file that joins it gets a number
of its own, part of every key its pages take (`SharedFile`), so two databases' pages never meet, and
forgetting one's file versions leaves the others'. Not on by default (`Db::share_pages`, the bench's
`AMPHORA_SHARED_PAGES`, one pool for all of K0's groups).

**Why.** Each connection's cache is its own: 32 readers each missing a working set larger than its
cache read the same pages from the file 32 times, each a `pread`, a 4 KiB copy and a checksum, and
evicted for them. A profile of the suite's `order_by_sort` at 32 clients (2,000 pages a reader):
reading pages in 27% of the time and evicting 19%, the CPU a statement took 4.4 times what it took
at 1 client.

**Sources.** PostgreSQL's shared buffers (its documentation, "Resource Consumption",
`shared_buffers`) and InnoDB's buffer pool (MySQL's reference manual, "Buffer Pool"): one cache all
sessions read, a page read once for all. CLOCK (Corbató, "A Paging Experiment with the Multics
System", MIT Project MAC, 1968): a reference bit a page, cleared as the hand passes, the first page
found unreferenced evicted.

**Evidence.** `crates/engine/tests/shared_pages.rs`: a second reader reads every page a first read
in from memory, from the log and from the file; a writer that holds a shared page updates it, and
the other readers read the old row while the write is open and after it is rolled back; a reader
after a checkpoint that wrote the file reads every new row; a writer whose spilled frames a rolled
back write used again reads, and leaves, only what committed; 300 random steps for 16 seeds, four
readers, a writer of up to hundreds of rows a transaction, rollbacks, checkpoints of every mode:
every read the model's. With no copy before a write (a mutant) the writer's test and the model fail
(a reader saw the write in progress); with a file write forgetting nothing, the checkpoint's test
and the model fail (a row read from a version written over); with a writer sharing its own frames,
the rolled-back write's test fails (it read the rolled-back value). The suite, fedora, one client
and 32, 200,000 rows, 2,000 pages a reader, sharing 65,536 pages (256 MiB) against none (checks at
loads 6–30, not recorded runs): at 32 clients `order_by_sort` 7,664 a second against 3,183,
`group_by_having` 7,137 against 2,907, `count_distinct` 7,277 against 2,844, `join_group` 1,366
against 687, `exists_subquery` 10,562 against 5,185, `table_group_by` 79 against 36, the peak
resident set 410 MB against 414; at one client the same gains, 2.2–2.4 times, the resident set 249
MB against 51 (the shared pages filled). Sharing 16,384 pages (64 MiB), smaller than the working
set, gains half as much (`order_by_sort` 5,314 at 32 clients) for more memory at 32 clients (611
MB: buffers the shared pages evicted live on in readers' caches beside their next copies). K0's
load (32 writers, 4 readers, two runs each): without sharing 163,682 and 163,620 writes (p99 6.3
ms), reads 1.36 and 1.32 million (p50 0.054 ms, p99 0.12 ms), 82–84 MB; sharing 65,536 pages,
153,178 and 157,216 writes (p99 7.0 ms), reads 1.94 and 1.92 million (p50 0.037 ms, p99 0.082–0.086
ms), 339–341 MB; sharing 16,384, writes 154,464 and 154,744 (p99 6.8–8.2 ms), reads unchanged,
150 MB. The recorded suite at 39549dc, without sharing and with 256 MiB shared
(`bench/records/2026-10-21-fedora-suite-amphora-39549dc.jsonl`, `-shared-256mib.jsonl`, loads
1.7 and 3.4): the analytic reads gain as above, but reads whose pages each reader's cache mostly
holds ran slower shared, `point_pk` 263,000 a second against 426,000 at one client,
`point_pk_one_column` 561,000 against 1,064,000 and `join_three` 10,500 against 23,400 at 32. Run
alone for 5 s after a second's warmup, `point_pk` at one client is faster shared (567,847 and
665,306 a second against 421,239 and 458,198, no read from the file left in its profile): the
suite's slowdown is likely its first statements' 2.5 s right after the load, while the
checkpointer drains the log and restarts it (every frame version kept then is a key no reader asks
for again), to be confirmed. Confirmed: at f2105d2, the statements that read slower shared run
after a 3 s warmup, sharing 65,536 pages against none, in turns, best of 2 (fedora, load 2.8–2.9,
checks): `point_pk` 578,820 a second against 426,838 at one client (1,118,528 against 1,080,926 at
32), `point_pk_one_column` 667,829 against 449,580 (1,132,324 against 1,078,408),
`point_secondary` 429,388 against 322,838 (1,123,615 against 1,065,232), `join_three` 2,109
against 1,729 (31,325 against 24,631), `order_by_sort` 766 against 384 (8,704 against 3,798),
`exists_subquery` 947 against 484 (11,450 against 5,932), `table_count` at 32 clients 255,748
against 76,860 (at one, 20,837 and 29,699 against 4,732 and 38,656: it varies eightfold unshared);
the peak resident set 265–297 MB against 333 (32 readers' caches hold one copy of a page between
them, not one each). `crates/engine/tests/shared_pages.rs`: two databases whose tables have the
same pages, frame for frame, share one pool and each reads its own rows alone, from the log and
after a checkpoint of one, the second time from memory; with every file given the same number (a
mutant) one reads the other's rows. So every read measured is faster shared, at 32 readers for less
memory; what is left before it is on by default is the default budget, and readers' own caches
smaller while sharing, for the process with few readers.

**Falsified by.** A read of a version the reader's snapshot doesn't see; reads slower shared than
not; more memory shared than the budget and the readers' own caches together.

## 21. A page read records itself only when it must wait for the cache

**Decision.** A page read from the file goes straight into the connection's cache; only a read
whose page the cache can't take yet (full of pages it can't evict, a spill in flight) is recorded in
`pending_reads`, so the call that resumes it reuses its page and its read rather than reading again
(`Pager::read_page_into`). Every read used to record itself before the cache took it and remove the
record after: two write locks, a hash map insert and remove, and two reference counts, for each page
read from the file.

**Why.** A profile of the suite's `filter_no_index` (a whole scan of 200,000 rows, 1 client,
2,000 pages cached): reading pages from the file 60% of its time, and removing the read's record
alone 11.6%, nearly all of it the record's last reference counts, on lines cold by then.

**Sources.** SQLite's pager reads a page it misses into its cache and keeps no record of the read
(`getPageNormal`, `src/pager.c`): its reads are synchronous, so nothing resumes them. Amphora's
reads stay resumable, as they are when the cache must spill first; only that case needs the record.

**Evidence.** `storage/pager.rs`, `a_read_waiting_on_a_spill_is_recorded_and_reused`: a read whose
insert into the cache must wait (a test hook makes it) is recorded, and the next call reuses its
page and its read and clears the record; with the record left out on that path (a mutant) it fails.
`read_page_nonblock_reentry_reuses_pending_entry` still holds. A probe of the engine on fedora (a
200,000-row copy of the suite's table, release, 2,000 pages cached, best of 3 in turns against the
binary before, load 2.6–2.7; checks, not recorded runs): a whole scan counted 33.64 → 31.35 ms, a
scan to row 90,000 30.21 → 28.58 ms. With 65,536 pages shared they take 16.88 and 14.24 ms: a page
found shared costs about a fifth of one read from the file.

**Falsified by.** A read resumed after a spill that reads its page twice, or another's.

## 22. A shared page found in one lookup, and counted where it was found

**Decision.** A shard of the shared pages keeps each image in its map's entry, beside the bit CLOCK
reads, and its ring holds only the keys, in the order the hand passes them
(`storage/shared_pages.rs`): a page found is one lookup, where it was the slot's number from the map
and then the slot. The pool's counts of lookups found, lookups missed and images added are kept in
each shard, written under the lock the lookup holds anyway, and summed when asked; they were one
count for the pool, written by every reader's every lookup.

**Why.** A profile of the suite's `group_by_having` sharing pages (1 client): finding pages in the
pool 21% of its time, half of it the slot reached after the map's entry, a second line from memory.
And a line every reader writes at every lookup moves between their cores each time: point reads
gained 1.36 times shared at one client, 1.03–1.05 times at 32 (decision 20).

**Sources.** CLOCK as in decision 20. Intel's optimization reference manual on false sharing: a line
written by many cores moves between their caches at each write. Counters split by CPU or by shard
and summed when read are the usual remedy (the Linux kernel's `percpu_counter`).

**Evidence.** `storage/shared_pages.rs` and `crates/engine/tests/shared_pages.rs` pass unchanged:
each version found under its own key and the counts summed, eviction by CLOCK under the bound, two
files' pages kept apart, readers never reading a version their snapshot doesn't see. Release
binaries on fedora sharing 65,536 pages (200,000 items, best of 2 in turns, each run started at
load under 2; checks, not recorded runs): at 32 clients `order_by_sort` 8,566 → 11,660 statements a
second, `exists_subquery` 11,330 → 15,483, `group_by_having` 8,169 → 9,851, `table_count` 266,452
→ 274,682; point reads unchanged at the bench's own ceiling (about 1.13 million a second); at one
client within the runs' spread (each statement varied 10–25% between rounds of one binary, on a
machine with heavy I/O pressure from other work).

**Falsified by.** Counts that differ from one count for the pool; reads slower at 32 clients than
before, or at one client beyond the spread of runs of one binary.

## 23. A commit ends before its pages outgrow the writer's cache

**Decision.** The writer commits what waits for it in as many commits as its cache holds the pages
of: a commit takes whole batches, in order, and ends before the next once the pages written so far,
and as many again as the last batch wrote, would pass the point where the cache spills written pages
into the log (90% of it); the rest go into the next commit at once (`Engine::apply_batches`,
`Db::transaction_while_room`). The first batch always goes in, however large. Decision 10's row
bound stays.

**Why.** A profile of the suite's `insert_batch_100` at 32 clients (a hundred rows of `items`, which
has four secondary indexes, to a statement): about 24 statements a commit, each commit writing about
3,400 pages through a writer that keeps 1,024; 13% of the writer's time went to spilling written
pages into the log mid-commit (to make room for a page a split allocates) and 21% to reading pages
back. Decision 10's bound counts rows, which for an engine of SQL statements are statements of any
size: it was meant to keep a commit's pages in the writer's cache, and with statements of a hundred
rows it doesn't.

**Sources.** SQLite's pager spills dirty pages into the log mid-transaction when its cache is full
(`pagerStress`, `src/pager.c`; `PRAGMA cache_spill`), and its pages are read back when the
transaction needs them again; Amphora's engine spills at 90% of its cache
(`storage/page_cache.rs`). Group commit as in decision 1: what a commit takes is a trade between
its fixed cost, paid fewer times by larger commits, and the work it causes itself.

**Evidence.** `crates/amphora/tests/store.rs`: ten batches of about 45 pages each handed to a held
writer that keeps 200 pages are committed in more than one commit and fewer than ten, each batch
durable and whole; with Amphora's 1,024 pages, in one;
`a_transaction_over_batches_ends_before_one_its_cache_might_not_hold`: a transaction over such
batches takes more than one and fewer than ten and commits exactly those, and a first batch larger
than the cache goes in whole, alone. With the bound left out (a mutant), both fail. Before the
bound, release binaries of 4a60ada on fedora, `insert_batch_100`, 200,000 items, one round each,
each run started at load under 2 (checks, not recorded runs): at 32 clients 351 statements a second
(p99 184 ms, 100 MB resident at most); at two statements to a commit (`AMPHORA_COMMIT_ROWS=2`, a
stand-in for the bound) 526 (p99 100 ms, 57 MB); keeping 4,096 pages instead, 512 (p99 139 ms,
114 MB); 16,384 pages, 685 (p99 131 ms, 209 MB), the larger caches slower at one client (196, 159
and 125 a second). With the bound, against the binary before it, the suite's writes in turns
(one round each, the binary before twice): at 32 clients `insert_batch_100` 333 and 325 → 456
statements a second (p99 203 and 197 → 131 ms), `delete_range_insert` 2,841 and 2,764 → 3,136
(p99 32 and 36 → 19 ms), `update_indexed` 7,145 and 7,170 → 8,486, `insert_one` and
`txn_read_write` within the runs' spread; peak resident 102 MB → 59 MB, the heap's peak 61 → 21
MB. With the bound and 4,096 pages kept, `insert_batch_100` 528 at 97 MB resident; the writer
keeps decision 5's 1,024 pages.

**Falsified by.** A batch committed in parts, or lost; fewer statements a second or longer tails
with the bound than without on the suite's writes.

## 24. The checkpointer lists the pages of the frames it copies

**Decision.** A checkpoint finds the pages it copies, each at its latest frame in the checkpoint's
range, in the frames' own list (decision 18's, in frame order: `storage/changes.rs`), copied out
under the list's lock and sorted by page after it (`InProcessWalCoordination::iter_latest_frames`,
`changes::latest_by_page`). Only when the list doesn't hold the whole range (past its bound, after
a gap a recovery or a restore left) does the checkpoint scan every page the log holds, as before. A
record cut into by a rewind leaves the list as it leaves the frame cache (`cache_record`).

**Why.** The scan walked every page with a frame in the log, at every checkpoint, holding the frame
cache's spin lock: the lock every page read from the log takes (`find_frame`). Past its bound the
checkpointer copies without pausing (decision 4), and a log of 131,072 frames holds tens of
thousands of pages. In a profile of the suite's `insert_batch_100` at 32 clients the writer spent
5.4% of its time spinning on that lock in `find_frame`.

**Sources.** SQLite's checkpoint iterates only the frames past the last backfill, sorted by page
from its WAL index (`walIteratorInit`, `src/wal.c`): its work is the frames it copies, not the pages
the log holds.

**Evidence.** `storage/wal.rs`, `the_checkpointers_list_is_what_a_scan_of_every_page_finds`: 20
seeds of 400 random appends of frames and packed records, rewinds by frames and by records, rollbacks
and restarts, each followed by three ranges: every range's list is the scan's, more than 10,000 of
them answered by the frames' list; with the cut record left in the list (a mutant) it fails at the
first seed. `storage/changes.rs`, `lists_each_pages_latest_frame_in_a_range`. A checkpoint with
frames to copy now searches no page's frames
(`in_process_checkpoint_with_no_frames_to_backfill_compares_no_frames`).

**Falsified by.** A checkpoint that copies a page at other than its latest frame in its range, or
misses one; checkpoints that list their pages slower than the scan did.

## 25. A checkpointer that copied everything durable waits for a flush

**Decision.** Past its bound the checkpointer passes without pausing only while its passes copy
something. A pass that copies nothing new (it copies only what a flush made durable, decision 15)
makes the next wait until a flush makes more durable, or a checkpoint period passes, or the store
stops (`Sends::flushed_past`, on the condition variable the flusher already signals at every
flush).

**Why.** With decisions 23 and 24, a profile of the suite's `insert_batch_100` at 32 clients: the
checkpointer passed about 10,000 times a second (784,505 times in a 70-second run of five
statements), nearly every pass copying nothing, each taking the gate, the log's coordination locks
and its file's reference count, lines the writer takes at every commit and page read. The
checkpointer's samples rose to 923, a third of the writer's, 18% of them on the log file's reference
count alone; and with decision 24's spinning gone (0 samples, from 5.4% of the writer's time), the
writer wrote no faster (`insert_batch_100` 448 and 399 statements a second against 456–463).

**Sources.** Decision 15: nothing that isn't durable is copied, so until a flush ends a pass has
nothing new to copy. Decision 4: past its bound, passes without pause keep what a restart must
copy small; a pass that copies something is still followed by the next at once.

**Evidence.** `crates/amphora/tests/store.rs`,
`a_checkpointer_with_nothing_durable_to_copy_waits_for_a_flush`: with flushes held and the log
past its bound and short of twice it, at most ten passes in 500 ms (the test's checkpoint period
100 ms), every row durable and read back once the flushes go; with the wait left out (a mutant),
135,526 passes in those 500 ms.

**Falsified by.** A log that stays longer past its bound (its restarts later or fewer) or writes
slower than with passes without pause.

## 26. One pool of pages a process's databases can share, a quarter of its memory

**Decision.** A database opened with `Sharing::Process` (`Db::open_sharing`) joins the process's
pool of shared pages (decision 20): one pool for every database the process opens that way, made
when the first opens, holding at most a quarter of the memory the process may use, in 4 KiB pages
(`db::process_pages`, `db::default_pages`, `memory::limit`): on Linux the
smallest memory limit of the control groups it runs in (cgroup v2, else v1, each group up to the
root) and the machine's memory; where the system doesn't say (macOS, Windows: their interfaces need
`unsafe`, which the crate denies), 65,536 pages (256 MiB, decision 20's measured size); never under
4,096 pages. `Sharing::Pool` gives a database a pool of the caller's, `Sharing::Off` none (a packed
log, decision 16, needs `Off`). `Db::open` still shares nothing: whether it should share the
process's pool by default, and that budget, are the person's call (STATUS.md). A storage that refuses to
share (another process's authority over the file, MVCC, a packed log: one this process packs or,
new with this decision, one whose generation on disk is packed, which recovery reads as written
without anything asking for packing again) leaves each connection its own cache and says so
(`amphora.store` event `pages_unshared`). The pool only holds pages connections have read:
a cap, not a reservation. A database that closes takes its pages out of the pool
(`SharedFile`'s membership, `SharedPages::forget_all`), so a pool that outlives its databases keeps
none of theirs.

**Why.** At ten million rows (6.7 GB of pages) each reader's 2,000 pages hold neither the table's
leaves nor even its inner pages (about 5,500): a lookup by rowid misses twice, each miss a log
lookup, a `pread`, a 4 KiB copy, a checksum and an eviction, about 20 µs a lookup against SQLite's
~1 µs from a warm OS cache. One pool for the process, sized from its memory, is the budget a
process has to give it, where a pool a database each multiplies with the databases (K0 runs one a
group).

**Sources.** PostgreSQL's documentation ("Resource Consumption", `shared_buffers`): on a dedicated
server with 1 GB or more, "a reasonable starting value for `shared_buffers` is 25% of the memory in
your system" (its shipped default is 128 MB). CedarDB's configuration reference: its buffer pool
(`buffersize`) defaults to 45% of available memory. DuckDB's configuration reference:
`memory_limit` defaults to 80% of RAM. A quarter is the cautious end of what databases that size
their caches from memory take. Control groups v2 (the kernel's `Documentation/admin-guide/
cgroup-v2.rst`, `memory.max`): a group's limit bounds every group under it, so the smallest along
the path is the process's.

**Evidence.** `crates/amphora/src/memory.rs`' tests: a process finds its memory group in cgroup
v2, v1 and hybrid hierarchies; limits read as bytes, "max" and v1's no-limit value as none; every
group above a group bounds it; this process's limit is no larger than its machine.
`db.rs`, `the_process_pool_is_a_quarter_of_its_memory`: 32 GiB gives 2,097,152 pages, 16 MiB the
floor, none said the fallback, no overflow. `storage/shared_pages.rs`,
`a_file_that_leaves_the_pool_takes_its_pages_with_it`: a file's pages stay while a handle is left
and all go with the last, the other file's untouched; with the membership's drop left empty (a
mutant) it fails. `crates/engine/tests/packed_log.rs`,
`a_database_opened_on_a_packed_log_doesnt_share_its_pages`: reopened on a packed generation, the
database refuses to share and reads every row; with the check of the log's format on disk left
out (a mutant) it fails. `crates/amphora/tests/crash.rs`: the power-cut seeds that don't pack their
log now share the process's pages through every crash and recovery, and lose nothing acknowledged.
The suite at ten million rows (adacachy, 2026-10-23, loads 65 to 117, checks rather than recorded
runs; `ab-v4-10m.jsonl`), the process's pool (8 GiB in the bench's 32 GiB unit; 98.8% of 628
million lookups found their page) against none: `insert_one` 3,023 and 1,302 a second at 32
clients, `update_indexed` 3,182 and 1,921, `txn_read_write` 2,469 and 1,521, `exists_subquery`
8.6 and 0.6 at one client, `join_pk` 430,752 and 272,128 at 32 clients; slower where many readers
scan the same pages at once (`table_group_by` at 32 clients 50 s a query against 15 s,
`window_row_number` 7,300 a second against 13,977), and for the statements right after the load
(`point_pk` at one client 8,968 against 172,322: the pool and the OS's cache of 14 GB of database
and log no longer both fit the unit). With decisions 27 to 29 and planner decision 7 in, and the
suite's settle cutting the log so the OS's cache of the database stays warm (`ab-v6-10m.jsonl`):
no pool reads faster still (`case_aggregate` at 32 clients 43,339 a second against 21,751,
`join_pk` 547,680 against 165,541, `union_all` at one client 206,733 against 17,936 while the pool
fills), the pool ahead on writes at 32 clients (`update_indexed` 4,696 against 3,265,
`txn_read_write` 2,497 against 2,025). A pool beside a warm OS cache holds every page twice and pays
a lookup and an insert on every miss; it pays where the OS can't keep the pages.

**Falsified by.** A process sharing the pool whose reads run slower than with none once the pool
is warm (the scans above, until the pool's lookups stop contending); a process whose other memory
the pool crowds out (its memory pressure rising with the pool); a closed database's pages found in
the pool.

## 27. A writer reads committed pages through the pool, and keeps none of what it reads

**Decision.** A connection holding the write lock looks pages up in the shared pool like a reader,
but only versions committed before its write transaction began: the log's committed end is
recorded when the transaction begins (`WalFile::write_base`, after any restart it makes), and a
page whose latest frame for the writer lies past it (a frame the transaction itself wrote, spilled
from its cache) isn't looked up. What the writer reads on a miss it doesn't keep in the pool: it
reads the pages it is about to change, whose committed images its commit supersedes. Pages it
changes it copies first, as decision 20's writers did (`Pager::add_dirty`).

**Why.** Decision 20 kept the writer out of the pool while it held the write lock. At ten million
rows the writer's 1,024 pages (decision 5) hold none of six B-trees' leaves and few of their inner
pages, so each commit read pages from the file that readers had already brought into memory. The
first version of this decision let the writer keep what it read too: the suite's 10M load then
filled the 8 GiB pool with log versions that all died when the settle restarted the log, and with
the pool on the suite's reads ran slower than with none (pool off against on at 32 clients:
`date_bucket` 9.1x, `string_functions` 8x, `case_aggregate` 5.6x; at one client `string_functions`
3.4x; `ab-v3-10m.jsonl`). A pool hit itself costs less than a read from the OS cache (one query of
340 pages through a 200-page reader cache, one client: 0.34 ms from the pool, 0.6 ms without, with
reads up to 2.9 ms), so the loss was the pool's contents, not its lookups.

**Sources.** Decision 20's invariant: only committed images are kept. SQLite's WAL (its
documentation, "Write-Ahead Logging", and `wal.c`): a writer's snapshot is the latest commit plus
its own uncommitted frames, appended past the committed end; frames up to that end are committed.

**Evidence.** `crates/engine/tests/shared_pages.rs`:
`a_writer_in_a_transaction_reads_committed_pages_from_memory` (a writer caching 200 pages that
updates 200 rows on 100 leaves in a transaction finds at least 50 of its pages in the pool, from
the log and after a checkpoint from the file, while a reader reads the old rows until it commits);
`a_writer_keeps_nothing_it_reads_in_the_shared_pages` (with nothing read in, the writer's
transaction adds at most its schema read to the pool, a reader after it the table's pages); the
model (300 random steps for 16 seeds) and `a_write_rolled_back_leaves_no_shared_page` still pass.
Mutants: a writer keeping what it reads fails the second; one keeping even its own spilled frames
fails `a_write_rolled_back_leaves_no_shared_page` (a reader read a rolled-back value); one whose
transaction's base is 0 fails the first. With the process's pool at ten million rows, writes run as
decision 26's evidence gives (`insert_one` at 32 clients 3,023 a second against 1,302 with no pool,
`update_indexed` 3,182 against 1,921).

**Falsified by.** A reader or a writer reading a version its snapshot doesn't see (the model's
check), or a write workload running slower with the writer reading the pool.

## 28. A reader's snapshot takes the log's generation without a lock; spin locks test first

**Decision.** The log's header keeps a copy of its generation (`checkpoint_seq`) that reads take
without the header's lock (`wal::HeaderCell`): every guard of the header refreshes the copy as it
is dropped, so no write to the header leaves it stale, and every write to the header holds the
shared log's write lock, so a reader under its read lock sees the copy and the other snapshot fields
together, as it saw them under the header's lock. A reader's snapshot (`snapshot_of`, taken at
every transaction a reader begins, and again to check it) reads the copy. And the engine's spin
lock (`fast_lock::SpinLock`) tests before it takes: a waiter spins on a plain load and swaps only
once the lock looks free.

**Why.** A profile of the suite's `point_pk` at 32 clients, ten million rows (v3, 2026-10-23):
19.5% of the time beginning read transactions, 18% loading the snapshot, 17.6% in the header
lock's swap alone. Every point read is a read transaction, so 32 readers took one lock line from
each other at every query; the spin lock swapped in its loop, so each waiter wrote the line at
every turn.

**Sources.** Anderson, "The Performance of Spin Lock Alternatives for Shared-Memory
Multiprocessors" (IEEE TPDS 1, 1990): test-and-test-and-set spins in the cache and goes to the
interconnect only when the lock is released. SQLite's WAL index header (`wal.c`,
`walIndexTryHdr`): readers copy the header without a lock and check the copy.

**Evidence.** `storage/wal.rs`, `a_header_cells_generation_follows_every_write`: the copy is the
header's after a field write and after the whole header is replaced. `fast_lock.rs`'s test: 1,000
threads incrementing under the lock lose no increment. The suite at ten million rows with no pool
(adacachy, checks rather than recorded runs, loads near 100 and 80): `point_pk` at 32 clients
701,613 a second before (`ab-v3-10m.jsonl`) and 1,572,331 after (`ab-v4-10m.jsonl`),
`point_pk_one_column` 589,372 and 1,713,766.

**Falsified by.** A reader's snapshot naming a generation the log isn't in (the WAL tests, the
model, the crash injection), or point reads at 32 clients no faster.

## 29. A pool lookup reads under its shard's read lock and copies the page out

**Decision.** A lookup in the shared pages (`SharedPages::copy_into`) takes its shard's read lock,
copies the page's image into a buffer of the reader's own, and lets the lock go: readers of the
same shard proceed together, and no reader holds a count on a buffer every other reader of that
page shares. The mark CLOCK reads (read since the hand last passed) is an atomic each entry, set by
lookups only when not already set; the shard's hits and misses are atomics beside its lock.
Additions, evictions and forgetting take the shard's write lock. A page read on a miss still keeps
the buffer it publishes (one reader, one count).

**Why.** With decision 27, the process's pool at ten million rows hit 98.8% of 628 million lookups
and still lost to no pool where many readers scanned the same pages at once (`table_group_by` at 32
clients 50 s a query against 15 s, `window_row_number` 7,300 a second against 13,977): each hit took
its shard's mutex and a count on the page's shared buffer, and released both, so 32 readers in step
took the same two lines from each other at every page.

**Sources.** The kernel's page cache serves the same reads without a lock a page (`filemap_read`,
the page cache's xarray read under RCU): what the pool's lookups lose to. Corbató's CLOCK (decision
20): only the reference bit is written on a hit.

**Evidence.** `storage/shared_pages.rs`, `a_lookup_copies_the_image_out_and_holds_nothing`: a hit
copies the image's bytes and adds no holder of the pool's buffer; a buffer of another size gets
nothing. The suite at ten million rows with the process's pool (adacachy, 2026-10-23, checks
rather than recorded runs; `ab-v4-10m.jsonl` before, `ab-v5-10m.jsonl` after, 98.8% of lookups
found both times), 32 clients: `window_row_number` 7,300 → 15,146 a second, `string_functions`
134,293 → 214,902, `group_by_range` 1,545 → 2,340, `date_bucket` 1,530 → 2,470, `aggregates_range`
21,591 → 24,428; against no pool in the same run, the pool now ahead or level on those, and
behind on `case_aggregate` (26,274 against 36,065) and on `union_all` at one client (53,790
against 75,565).

**Falsified by.** Scans many readers make of the same pages at once still slower with the pool than
without, or single readers slower (a 4 KiB copy a hit costing more than the count it saves).

## Virtual filesystem registry ownership (AC-T1.1.1)

A synthetic file identity belongs to its virtual filesystem. Independent MemoryIO and MemoryYieldIO
instances can contain different bytes under the same path; their registry keys therefore combine
FileId with an opaque, retained Arc identity. Reopening through the same backend or a transparent
wrapper retains that scope and shares the existing Database. Native files keep the unscoped OS
identity, including hard-link sharing. FileId's public dev/ino representation is unchanged.
Within each memory backend, file creation assigns a monotonically increasing checked file ID
under the existing files mutex. Removing and recreating a path gets a fresh ID while old handles
retain the old bytes. Exhausting the counter returns a typed refusal; IDs never wrap or repeat.
A missing path has no file identity and returns NotFound.

A scoped registration is owned by its Database and removed in one key lookup when its final alias
closes. The cleanup compares the exact weak Database identity, so an old destructor cannot remove a
successor or an Opening reservation. Failed and cancelled opens keep the existing state-owned
reservation cleanup. Registry locks are released before validating or dropping an upgraded Database.
This prevents both an unbounded accumulation of dead virtual scopes and destructor lock reentrancy.

Evidence: the engine's implicit_rowid_seek suite opens independent MemoryIO backends at the same
path; the previous path-only keys reused one Database and three parallel cases returned Busy. The
parameterized ownership regression is a sequential witness: independent backends keep different
rows, aliases share writes, unlink/recreate keeps old readers separate from new bytes, and the
final alias releases its scoped entry. It covers both memory backends and their mixed use when
io_memory_yield is enabled. These behaviors, and the unchanged implicit-rowid suite, would falsify
this ownership model if they fail.

Sources: Rust's [`Arc::ptr_eq`](https://doc.rust-lang.org/std/sync/struct.Arc.html#method.ptr_eq)
and [`Weak::ptr_eq`](https://doc.rust-lang.org/std/sync/struct.Weak.html#method.ptr_eq) specify
allocation identity. Unlike value equality, these preserve separate scopes with equal payloads;
the retained Arc and exact Weak owner keep those allocations alive until cleanup.

## 30. A transaction's spills write over the frames it already wrote

**Decision.** A page a write transaction already wrote into the log past its committed end is
written back over that frame when it spills again, or a cache flush writes it again, not appended
(`WalFile::append_frames_vectored`, `write_over`; frames next to each other in one write). A frame
written over holds no checksum until its commit (`prepare_unchained_frame`): the commit computes
every frame's from the first one written over, so one computed at the spill would be thrown away,
and a transaction that spills heavily writes its pages over again many times. The
frame keeps its page, so the frame cache, the list of changed pages (decision 18) and readers, who
see frames only up to the committed end, are untouched. The first frame written over is noted: its
checksum, and every later frame's, no longer chain. Before a commit prepares its frames,
`Wal::rechain` (the commit's `Rechain` step) reads the transaction's frames back from that frame to
its last, 1,024 at a time (as many as a spill writes at once, 4 MiB: what it holds in memory),
chains their checksums again from the frame before it (the log's header for frame 1, the last
commit's checksum, or the checksum that frame holds, never written over) and writes them back; the
commit's frames then chain on from them, as they always did. A savepoint rolled back past the first
frame written over forgets it. A raw frame (`write_frame_raw`, the sync engine's) after frames
written over is refused (`TxError`): it could commit them before they chain. A packed log (decision
16, opt-in) keeps appending its records, whose lengths follow their contents.

So the log a transaction writes holds at most a frame for each page it dirtied before its commit,
and at most two after it, whatever its statement count: the engine keeps a spilled page dirty, and
its commit writes every dirty page again, where SQLite's pager marks a spilled page clean and its
commit writes only what changed since (left as it was here: one frame a page is that change).

**Why.** The unify audit (kiln-unify, finding data-1, Amphora 542f5a9): one 8 MiB log-search
transaction (for each trigram a row into a table with two indexes and an upsert of its term's
count, in nested savepoints) left a 7,068.5 MiB log beside a 119.9 MiB file, and a 12 MiB one,
rolled back, 9,010.7 MiB. Each page changed again after it spilled was appended again: 1.8 million
4 KiB frames for 1,790,521 postings. A log that grows with a transaction's statements instead of its
pages costs every byte written, the disk it keeps (decision 13) and the checkpoints that copy it.

**Sources.** SQLite 3.54.0's `walFrames` (`src/wal.c`): "Check if this page has already been
written into the wal file by the current transaction. If so, overwrite the existing frame";
`iReCksum` ("On commit, recalculate checksums from here") and `walRewriteChecksums`, which reads the
frames back from the earliest written over and writes their headers again;
`sqlite3WalSavepointUndo` forgets `iReCksum` past the frame it rolls back to. Its pager's spill,
`pagerStress` (`src/pager.c`): a single frame for the page, the page then clean.
sqlite.org/fileformat2.html, "The Write-Ahead
Log": a frame is valid only if its checksum matches the one "computed consecutively on the first 24
bytes of the WAL header and the first 8 bytes and the content of all frames up to and including the
current frame", and a read takes "the last valid instance of page P that is followed by a commit
frame or is a commit frame itself"; sqlite.org/walformat.html: mxFrame "is set to the index of the
last valid commit frame in WAL". So frames past the last commit stand for nothing until a commit
frame follows them: a crash before the commit's frames, written over or chained again or not, keeps
the last commit.

**Evidence.** `crates/engine/tests/spilled_log.rs`,
`a_transactions_log_is_bounded_by_the_pages_it_dirtied_not_its_statements`: 60,000 statements of
kiln's log search on a 200-page cache, the log cut first, dirty 473 pages; on c87e332 the log held
1,892 frames past its last commit before the commit and 2,365 after (the test failed); with this
decision 462 and 935, every row read back on a fresh connection and after opening the database
again (its recovery reads the commit's chain whole, from the log's header), the integrity check
whole. The audit's legs, its corpus and SQL on a connection as Amphora's writer keeps one (1,024
pages; `the_audits_log_search_legs_log_within_their_pages`, release builds without LTO as the audit
built them, adacachy 2026-10-29 at load 46-64, sizes, not times): on c87e332 the 1, 2, 4 and 8 MiB
transactions dirtied 3,405, 7,390, 15,162 and 30,689 pages and left 22,554, 207,360, 688,238 and
1,792,166 frames past the last commit (88.6, 814.7, 2,704.2 and 7,041.7 MiB: the audit's 7,068.5
MiB at 8 MiB, growing faster than the pages); with this decision 6,780, 14,758, 30,307 and 61,359
(26.6, 58.0, 119.1 and 241.1 MiB), about two frames a page at every size. Crash injection
(`crates/amphora/tests/crash.rs`,
`a_power_cut_in_a_transaction_that_wrote_its_frames_over_keeps_the_last_commit`): 1,200 edits of 600
one-page documents on a 200-page cache, a posting and an upsert each, in nested savepoints, the
commit flushed, the power cut at a change drawn from the 319 the transaction makes: in every gate
100 seeds, 79 cut midway, 78 of them after writes over the log's own frames, 21 restarted with the
whole transaction, the rest with what was there before; in 10,000 seeds (release) 7,964 cut midway,
7,913 of them after writes over the log's own frames, 2,044 restarted with the whole transaction
(2,036 of them acknowledged), never one with a part of it. The engine's suites pass unchanged: the
log's and pager's unit tests (116 in `storage::wal` and `storage::pager`, among them the contract
that a commit chains after its transaction's spilled frames), the model that rolls savepoints back
amid spills in both log formats (`packed_log.rs`), and the reader-cache and shared-pages models.
What it costs, the four legs in turn with c87e332's (release binaries, adacachy 2026-10-29 at load
17-90, best of 2; checks rather than recorded runs): on files written in place, as decision 12
makes Amphora's, 50.3 s against c87e332's 76.9 s (frames written one at a time, not together: 64.4
s); in a copy-on-write directory with compression, 59.6 s against 39.5 s, every frame written over
there a new extent.

On a quiet Mac (adas-mac-studio-3, M4 Max, APFS; `bench/records/2026-11-06-studio3-wal-regression.md`)
the audit's legs, one call each, took the same time as on 542f5a9, before this decision, while the
disks wrote 13-16 times less. With checksums computed at each write over (521e4bc), 4 MiB took
6.49 s against 6.39 s, 1.6% longer. A spill that writes over now computes none
(`a_frame_a_spill_writes_over_holds_no_checksum_until_its_commit`, red before), and best of 2
against 542f5a9: 1 MiB 1.19 s against 1.18 s, 4 MiB 6.39 s against 6.45 s, 8 MiB 14.16 s against
15.11 s, the disks writing 658-786 MiB against 10.1 GB.

**Falsified by.** A transaction whose log holds more frames past the last commit than the pages it
dirtied before its commit, or more than twice them after (`spilled_log.rs`); a row read back other
than as committed, on a fresh connection or after opening again (the models); a power cut that
leaves part of a transaction whose frames were written over, or loses one whose commit was flushed
(`tests/crash.rs`); on files written in place, a transaction that spills taking longer than it did
when its spills appended.

## 31. A restart cuts a log past twice its bound to nothing

**Decision.** `Db::restart_log` starts the log again with its file kept (decision 13) while the
file holds room for no more frames than `Db::keep_log` keeps: by default twice the store's default
bound (decision 4), 262,144 frames, 1.0 GiB of 4 KiB pages, the most a log takes between restarts. A
file past that is cut to nothing in the same step (the engine's TRUNCATE checkpoint). When: only at
a restart, which the store already makes between commits with its readers and checkpointer held
back, and only once every frame is in the file and the file flushed (the checkpoint copies what is
left and flushes the file first, then cuts the log and flushes it). Not cut to the bound: until the
next commit writes a new header the file holds the old generation's header and frames, all copied,
and a file cut short among them would keep a valid prefix of old frames that a recovery would read
over the newer pages the checkpoint wrote, which is why SQLite cuts a log to its limit only after
the new header is synced, and to zero bytes on close. A log within the bound pays nothing new.

**Why.** The unify audit (finding data-1): the restart kept the log's file at its high water for
good: the 8 MiB transaction's 7 GiB log stayed, and the rolled-back 12 MiB one left 9 GiB. Decision
30 bounds what one transaction writes by its pages, not by the store's bound: a transaction of a
million pages still grows the log past it, and nothing gave the space back.

**Sources.** sqlite.org/pragma.html, `journal_size_limit`: "in WAL mode, the write-ahead log file is
not truncated following a checkpoint. Instead, SQLite reuses the existing file for subsequent WAL
entries since overwriting is faster than appending", and "Each time a transaction is committed or a
WAL file resets, SQLite compares the size of the rollback journal file or WAL file left in the
file-system to the size limit set by this pragma and if the journal or WAL file is larger it is
truncated to the limit"; its default is no limit. sqlite.org/c3ref/wal_checkpoint_v2.html:
`SQLITE_CHECKPOINT_TRUNCATE` "works the same way as SQLITE_CHECKPOINT_RESTART with the addition that
it also truncates the log file to zero bytes just prior to a successful return". SQLite 3.54.0's
`src/wal.c`: the limit is applied at the first commit after a reset, once its header is written
(`truncateOnCommit`), and `sqlite3WalClose` cuts "to zero bytes as truncating to the
journal_size_limit might leave a corrupt WAL file on disk". sqlite.org/fileformat2.html:
"Performance is usually a little better if the WAL is not truncated, since filesystems generally
will overwrite an existing file faster than they will grow a file."

**Evidence.** `crates/amphora/tests/files.rs`,
`a_restart_cuts_a_log_past_its_bound_to_nothing_and_keeps_one_within_it`: with 64 frames kept, 500
rows of a page grew the log to 2,076,512 bytes; on c87e332 (with the setter alone) the restart left
the file at that length (the test failed); with this decision it cut it to 0 bytes, a row's commit
grew it to a few frames, a restart within the bound kept that length, and all 501 rows read back
after opening again. Crash injection (`tests/crash.rs`): every third seed keeps 16 frames, so each
of its restarts cuts the log; in 10,000 seeds 4,365 cuts came before a power cut (40 in the gate's
200), and nothing acknowledged was lost, nothing refused found, nothing torn. Its cost to writers
(`what_cutting_the_log_costs_its_writer`, release without LTO, adacachy 2026-10-29 at load 66-100,
files in place as decision 12 makes them; a check rather than a recorded run): on a 1 GiB log a
restart keeping the file held the writer 42.4 ms (best of 6) and one cutting it 66.0 ms (best of 2),
and the commit of 4 MiB and the flush after it took 36.0 ms after a restart that kept the file and
79.7 ms after one that cut it (its writes extend the file again). A log is cut only once it has
outgrown twice its bound, so that is paid once for each such log, not at every restart.

**Falsified by.** A log's file longer than its bound after a restart that copied every frame
(`files.rs`); an acknowledged write lost, or a database refused, after a power cut around a restart
that cut the log (`tests/crash.rs`); a restart that cuts the log holding the writer much longer than
one that keeps it, or the commits after it slower than after a restart that keeps the file.

## 32. A batch that wrote nothing is durable as the last flush left it

**Decision.** A batch whose commit wrote nothing, a session that only read, waits for no flush of
its own. Its ticket waits for a flush, like every batch's (decision 15). That flush finds the log
where the last flush left it, so it returns without syncing, and the batch is told durable at once.
If an earlier commit's flush is still to come, the batch waits for it: every commit the batch could
have seen is durable before it is answered, so its answer rests only on durable writes. Writes keep
their rule: one is acknowledged only once a flush that began after its commit ends (AC-T1.4.4).

**Why.** kiln's W24-AC10 lookups on 29be700 took about 92 ms each on adacachy (load 40-90), and
that was read as read-only calls each waiting for a fresh fsync (kiln goal f4b650c4). They don't.
A read-only `Database::call` writes no frames and waits for no fsync (docs/research/read-only-calls.md).
Those lookups write a hit count on every hit, and a write waits for its flush.

**Sources.** A read-only transaction that depends on commits not yet durable must wait for them,
and only for them. Kimura, Graefe and Kuno ("Efficient Locking Techniques for Databases on Modern
Hardware", ADMS 2012, §5.3): a read-only transaction "immediately exits if the maximum tag is
already durable. Otherwise, it wakes up the log flusher and waits until the LSN becomes durable."
Aether (Johnson et al., PVLDB 3(1), 2010, §3.1): a transaction that read a pre-committed
transaction's data "must not be allowed to return results to the user until both their own and
their predecessor's log records have reached the disk". Engines skip the flush for a transaction
that wrote nothing. PostgreSQL's `RecordTransactionCommit` (`xact.c`, REL_18_0) writes no commit
record without a transaction ID, and SQLite's `sqlite3PagerCommitPhaseOne` (3.54.1) returns early
"If no database changes have been made".

**Evidence.** `crates/amphora/tests/crash.rs`,
`a_batch_that_wrote_nothing_is_durable_with_every_flush_held`: on the simulated disk with every
flush held, a batch that only reads is told durable at once, with no flush held, and a write's
flush stays held until let go. It passes at 29be700, and a mutant whose flusher syncs even with
nothing new in the log fails it (its 30 s hang guard; adas-mac-studio-3, 2026-11-01). The probe in
docs/research/read-only-calls.md: 1,500 read-only calls left the log at 791,072 bytes, p50 about
78 µs.

**Falsified by.** A batch that wrote nothing waiting for a flush that syncs (the test above), or
answered before an earlier commit it could have seen is durable.

## 33. A query runs beside the writer, on a reader, and writes nothing

**Decision.** `Database::query` runs a read-only SQL session on one of the store's readers, on its
caller's thread, the readers beside the writer this store already keeps. It runs on the writer
only when the store has no reader to lend (while its file is replaced), between commits. It sees
what was committed when it began, one snapshot (a deferred `BEGIN`), and waits for neither the
writer's queue nor a flush. It writes nothing: the engine's `query_only` refuses every statement
that writes, and the session ends rolled back. A remote or replicated database runs a query as a
call.

**Why.** Commands go to the one writer and queries to its readers (CQRS; Ada, 2026-11-01, kiln goal
f4b650c4). A read-only session through `Database::call` waits behind the writer: behind every batch
handed over before it, and, while a session holds the writer, until that session ends.

**What it sees.** What is committed, as the store's record reads see it (`Store::read_committed`).
So a query can see a commit that is not yet durable, which a power cut could take back, though no
writer was acknowledged for it (decision 15). A session whose answer must rest only on durable
writes goes through `Database::call` (decision 32).

**Sources.** SQLite's write-ahead log (sqlite.org/wal.html): "A COMMIT occurs when a special record
indicating a commit is appended to the WAL", and "When a read operation begins on a WAL-mode
database, it first remembers the location of the last valid commit record in the WAL": a reader
reads committed frames and never waits on the writer. The engine refuses writing statements under
`query_only` (`crates/engine/translate/mod.rs`: "Cannot execute write statement in query_only mode").

**Evidence.** `crates/amphora/tests/database.rs`,
`a_query_answers_beside_a_held_writer_and_writes_nothing`. A session held the writer with its insert
uncommitted, and a query answered with the committed count (1) in 0.07 s. With `query` taking
`call`'s path, as before this decision, the query waited behind the writer until its 30 s deadline
and failed with Timeout. Once the writer went, a query saw the insert (2). A query that wrote was
refused, and the table stayed at 2 (adas-mac-studio-3, 2026-11-01).

**Falsified by.** A query that waits behind the writer or a flush (the test above), one that writes,
or one that sees a commit made after it began.

## 34. A prepared statement is validated once and held between executes

**Decision.** The public SQL API's `Statement` (`crates/api/src/sql.rs`) holds what
`Session::prepare` gave it, and each execute runs that. The local session (`crates/amphora/src/sql.rs`,
`Db`) validates the text once there and holds its engine statement, taken out of Db's statement
cache or prepared (`Db::cached`, `Db::prepare_held`): an execute neither parses the text nor looks it up. Validation
reads the text alone (the whole statement parsed, its tokens walked for reserved names, transaction
and connection control refused), so its answer can't change between executes. A held statement is
prepared again only when the schema changed since it was (`Db::run_held`, by the cache's own test of
the schema), and dropped, it goes back to the cache, kept while that schema stands
(`Db::give_back`), reset with its bindings cleared. While it's held, the same text is prepared anew
for anyone else: a statement in use is never handed out twice. A call given text
(`Connection::execute`, `query_row`) validates unless its cache entry carries decision 36's mark,
`execute_batch` every time, and a session that says nothing (`Session::prepare`'s default, the remote client's, whose owner validates
what it receives) keeps the text and validates it on every execute; no wire format changes.

The cache (`db/cache.rs`) keeps at most `TX_MAX` (1,000) statements and `SQL_MAX` (1 MiB) of their
text, and evicts the least recently used until both hold. Its recency order is a list linked
through its map by the statements' text, so taking, returning and evicting a statement each cost a
few map operations; the order was a queue searched end to end on every take and every return.

**Why.** The unify audit's finding 4: every execute of a prepared statement parsed its SQL again to
validate it, 1,001 parses for 1,000 executes, though Db's statement cache spared the engine's own
prepare. On kiln's hot statements at e2e67247 that validation was 27% of a REAPI digest's CPU time
(2,891 of 10,729 ns: memo.rs `missing`'s EXISTS, cas.rs `object`'s SELECT and UPDATE), 35% of a log
search page's (15,269 of 43,175 ns) and 17% of a posting INSERT's (649 of 3,756 ns), by
`amphora-bench --sql-api --shapes parse` beside each shape, the writer thread's CPU time, best of 3,
on adacachy at load 15 to 22: an estimate beside production, not a recorded run.

**Sources.** sqlite.org/c3ref/stmt.html: "The original SQL text is source code. A prepared statement
object is the compiled object code", and its life-cycle prepares once, then binds, steps and resets
it again and again ("Reset the prepared statement using sqlite3_reset() then go back to step 2. Do
this zero or more times"). The plan's research `01a18d12143072d5992fe36a5f3b2ccb` keeps the notes.

**Evidence.** `crates/amphora/src/sql.rs`, `a_prepared_statement_is_parsed_once_however_often_it_runs`:
1,000 executes of one `prepare_cached` INSERT and 1,000 runs of log search's next page (kiln's
`ci/logsearch.rs` at r80, three correlated subqueries) through one `prepare_cached` statement. At
e2e67247 the INSERT parsed 1,001 times (the test failed there, its pages already right); with this
decision each text parsed once, every page's rows the same as e2e67247's (blocks 2 and 1 of leg l1,
then 7 of l0), and the same INSERT given as text parsed again.
`a_held_statement_follows_the_schema_and_is_never_handed_out_twice`: a held `SELECT * FROM held`
read one column, then two after `ALTER TABLE held ADD COLUMN c`, as a new one did; one INSERT
prepared twice gave two statements, which wrote ten rows in turn. `db/cache.rs`,
`a_full_cache_evicts_its_least_recently_used_statement`: a cache at its own bounds given `TX_MAX` + 1
statements, the first taken and returned before the last, evicted the second-oldest, kept the first
and held `TX_MAX`.

**Falsified by.** A prepared statement that runs what validation refuses (its text can't change, so
this would be a fault in `prepare`); a text call that runs without validation; or, at plan step 3,
the parser above 1% of a prepared statement's hot loop.

## 35. Small commits: the checkpoint copies a page once for many commits

**Decision.** While the log is within its bound, the checkpointer looks at it every period
(`Config::checkpoint_every`, 25 ms) and leaves it uncopied; it copies once the log has passed its
bound (decision 4's drain, unchanged). From 2026-11-06 to 2026-11-11 it copied once
`Config::copy_after` (8,192) of its frames weren't in the file yet; that count was picked and is
gone (below, "No count of frames"). It reads the log through `Engine::uncopied`, `Db::uncopied`, the engine's `Database::wal_backfill`. A page that
many small commits change is copied into the file once for all of them, not once for each period's
commits. What the log records for a commit is unchanged in this step: each page the commit touched,
whole (SQLite's frame, a 24-byte header and the 4 KiB page; decision 30 for spills).

**Why.** The unify audit's batched leg (kiln's log index, 128 grams a durable call, kiln goal
60805ec4) wrote 616-626 times its 1 MiB of log text to a quiet Mac's disks
(`bench/records/2026-11-06-studio3-wal-regression.md`). Counted where Amphora writes
(`crates/amphora/tests/amplification.rs`: the leg's statements and shape on a disk that counts each
write and takes 3 ms a flush; fedora, 2026-11-06), its 1,884 calls committed 13.97 MiB of rows and
wrote 231.8 MiB of log (16.6 bytes a byte of rows: about 31 page frames a call) and 145.5 MiB into
the file (10.4): the checkpointer, every 25 ms, copied the pages two or three commits had touched,
and the next pass copied most of them again, with a flush of the file each time (359). The file the
leg builds is about 1,000 pages; a page dirtied k times between copies costs one copy, not k.

**The plan, by what each step cuts** (the same leg, bytes written a byte of rows, log and file;
adas-mac-studio-3, 2026-11-06, `docs/research/small-commits.md`):

| step | log | file | all |
|---|---|---|---|
| before (a copy every period, page frames) | 16.6 | 9.9 | 26.4 |
| 1. this decision | 16.6 | 0.7 | 17.3 |
| 2. commits as changes: the packed log's deltas (decision 17) | 3.0 | 0.2 | 3.3 |
| 3. a row's change logged once (CP-T1.3, engine.md decision 6) | the rows | the pages | |

Step 1 is built here: it needs no change on disk and cuts the file's share 15-fold. Step 2 cuts the
log's fivefold, but the packed log can't be made the default as it is. Its spills append (decision
30 covers the page-a-frame log only), which would grow a big transaction's log back toward
decision 30's gigabytes. Its pages aren't shared (decisions 20 and 26 refuse a packed log). And it
cost the comprehensive bench's bulk statements a wait not yet found (decision 16). The follow-up is
a log of both kinds of slot, under a new header format value (an append-only change: generations
written before read as ever): a spill writes whole page frames over the transaction's own frames, as
decision 30 does, and a commit writes packed records with deltas, their first slot's page field
marked apart from a page number; with the records' pages shared, and the bulk statements measured
against page-a-frame. Step 3 is CP-T1.3's storage.

**The bound.** The file is written at most once for each generation of the log (the bound's frames,
`copy_after` until 2026-11-11), and each time with at most the pages those frames hold: the bytes the
checkpoint writes are never more than the log's (a page is in the log before it is copied), and at
most the log's times W / the bound for commits that touch a working set of W pages. For the batched leg (W about
1,000) the checkpoint writes at most a quarter of the log's bytes, where it wrote 60%
(`small_commits_copy_their_pages_once_for_many_commits`), and the leg's bytes a byte of rows fall
from 26.4 to at most 16.6 x 1.25 = 20.8. Commits that touch pages nothing touches again (random
writes over a database many times the window) gain nothing: as in any B-tree updated in place, each
such page costs its frame and its copy.

On a Mac's disks a durable flush (`F_FULLFSYNC`, decision 13) costs about 107 KiB beyond what it
makes durable (measured on adas-mac-studio-3's APFS: 4 KiB written and flushed 1,000 times wrote
113.4 KiB a write; 128 KiB written 1,000 times and flushed once, 126.5 KiB). A caller that waits for
each call, as kiln's indexer does, pays a flush a call, and no format of the log removes that: only
more rows a call do. So the bound for the audit's leg, a MiB of text on a quiet Mac: its log (232
MiB), at most a quarter of that copied (58 MiB), and 107 KiB for each of its 1,822 calls' flushes
and the checkpoints' few (191 MiB): at most 481 MiB, where 616-626 were written.

**Sources.** sqlite.org/wal.html, "Automatic Checkpoint": "By default, SQLite does a checkpoint
automatically when the WAL file reaches a threshold size of 1000 pages"; checkpoints follow the
log's growth, not a clock. PostgreSQL's documentation, "WAL Configuration"
(`wal-configuration.html`): checkpoints come every `checkpoint_timeout` or `max_wal_size` of log;
"Reducing checkpoint_timeout and/or max_wal_size causes checkpoints to occur more often. This allows
faster after-crash recovery, since less work will need to be redone. However, one must balance this
against the increased cost of flushing dirty data pages more often". Here recovery reads the whole
log whatever was copied (Turso's recovery rebuilds the frame index from every frame of the
generation, and a reopened log is copied again from its first frame), so copying more often buys no
faster recovery. ARIES (Mohan et al., TODS 17(1), 1992): pages are written to their place
independently of the log, which needs only that a page's log records be durable before it is
(decision 15's mark, unchanged). The 8,192 (to 2026-11-11, now removed): what a 25 ms pass copied at 32 writers (decision 4: about
73 frames a commit, about 5,000 commits a second, about 9,000 frames a period), so no pass copies
more pages, or flushes the file over more, than passes did under that load, and that load's
checkpoints are as before.

**Evidence.** `crates/amphora/tests/amplification.rs`,
`small_commits_copy_their_pages_once_for_many_commits`: the batched leg's statements, as kiln's
indexer calls them one durable call at a time, each document read back whole after; the checkpoint
must write at most a quarter of the log's bytes. Red at 13e385d. What `copy_after` buys
(adas-mac-studio-3, bytes into the file a byte of rows, and its flushes): every period 9.9 (270),
1,024 frames 3.7 (53), 2,048 2.2 (28), 8,192 0.7 (7), 32,768 0.2 (1). On a quiet Mac (adas-mac-studio-3,
2026-11-07, `bench/records/2026-11-07-studio3-small-commits.md`) the audit's batched leg wrote
427-442 MiB to the disks for its MiB of text, within the bound of 481, where 13e385d wrote 599-624;
its 1, 4 and 8 MiB legs took 1.20, 6.40 and 14.61 s, best of 2, against 13e385d's 1.24, 6.99 and
15.20 s.

**No count of frames (2026-11-11).** Ada: "We don't tolerate arbitrary constants, learned and stat
derived values only." The 8,192 was one load's pass size (32 writers on fedora), a 16th of the
bound. No measured quantity calls for it. Copying a frame within the bound buys nothing this store
can use: recovery reads the whole log whatever was copied (Sources, above), and a restart, the only
thing that needs the log copied, comes past the bound, where the drain copies it anyway. Waiting
only makes the copy smaller: a page changed by k commits within a generation is copied once, not
up to k times. The table above already shows the trend: 32,768 frames copied 0.2 bytes a byte of
rows with one flush. So the derived threshold is the bound itself, and `copy_after` is removed.
What a pass then copies is the generation's distinct pages, at most the smaller of the database and
the bound. The open question is decision 4's, not this one's: whether one pass of that size, with
its flush of the file, lengthens the writers' tails at 32 writers and more against passes of 8,192
frames. The falsifier below names that run. `crates/amphora/tests/store.rs`,
`a_log_within_its_bound_is_not_copied`: a log of bound 12,288 holding 10,240 frames saw no pass (red
before, adas-mac-studio.local-2: "passes with 10242 frames in a log of bound 12288"), then passes
once past its bound, every row read back. The crash seeds (`tests/crash.rs`) copied after 0 to 7
frames; they now take bounds of 8 to 23 frames, so cuts still land with frames left uncopied across
commits.

**Falsified by.** The checkpoint writing more than a quarter of the log's bytes on the batched leg
(the test above); a pass within the log's bound; a reader reading a page other than as committed while copies wait, or an
acknowledged batch lost by a power cut with frames left uncopied across commits (`tests/crash.rs`,
whose seeds copy after 0 to 7 frames); at 32 writers and more, commits slower or their tails longer
than with a copy at every period (decision 4's bench).

## 36. A validated text is marked on its statement-cache entry

**Decision.** Db's statement-cache entry (`db/cache.rs`) carries whether the session's
one-statement validation, `validate(sql, false)` in `crates/amphora/src/sql.rs`, accepted its text.
Every Session path that runs or prepares one statement (`query`, a text call's and a remote
client's Query; `column_count`; `prepare`, which `prepare_cached` and the owner's answer to a remote
Columns use) takes the entry with its mark (`Db::cached`): a marked hit runs without a parse, a
miss or an unmarked hit is validated before it is prepared or stepped, and only that validation's
success marks it (`Held::mark`). A refused text is never marked, and an unmarked hit it refused goes
back as it was. Db's private SQL (`query_owned`, `column_count`) keeps an entry's mark and never
sets one, so an entry the product cached for itself is validated on its first Session use. The mark
lives in the entry and goes with it: an eviction, a replacement (`put` gives the new statement the
putter's mark), a schema change and a restore (`Cache::schema`, `Cache::clear`) drop it. A statement
held by a `Statement` is out of the cache, so the same text given meanwhile is prepared and
validated anew. `execute_batch` (`validate(sql, true)`) and the database's `execute`/`migrate`
validators are unchanged and keep no mark. Nothing crosses the wire, and there is no second set
of texts.

**Why.** Decision 34 made a held statement's executes parse nothing, but each `prepare_cached`
validated its text again, and so did every text call: kiln prepares per request (log search's page,
memo.rs `missing`) and runs text calls in its hot paths, so each still parsed once per request. The
mark is sound for the reason decision 34's hold is: validation reads the text alone, and the
entry's key is that text.

**Sources.** Decision 34's: SQLite compiles a statement once from its text (sqlite.org/c3ref/stmt.html).

**Evidence.** `crates/amphora/src/sql.rs`, `a_validated_text_keeps_its_mark_until_its_cache_entry_goes`:
an INSERT run as text three times and a SELECT prepared anew three times parsed once each, and no
more in a second `Database::call`; `BEGIN` and a reserved name, each run twice, were refused and
parsed both times; after `CREATE TABLE`, the SELECT parsed once more.
`a_privately_cached_or_evicted_text_is_validated_again`: a text Db's private path cached was not
parsed there, then parsed once on its first prepare and not on its second; a text evicted by
`TX_MAX` newer ones parsed again. `a_session_mark_never_widens_database_execute`: `SAVEPOINT s` and `RELEASE s`, run
twice each as Session text calls, parsed once each, marked; `Database::execute` still refused both
("connection and transaction control belongs to Amphora"): its validator reads no mark. Decision
34's tests pass unchanged.

**Falsified by.** A Session path that runs a text no validation accepted while its entry stood: a
private-SQL entry run unvalidated, a refused text run on its second call, or a mark that survives
its entry's eviction, replacement, schema change or restore; or `Database::execute` or
`migrate` accepting a text because a session marked it.

## 37. A commit logs its pages' changes: a log of both kinds of slot

**Decision.** A new generation of the log is mixed (its header's file format
`packed::MIXED_FORMAT`, `0x414d5003`; Amphora's default, `Db::open`): a commit writes its pages as
packed records (decision 16), each page a delta against its version the log already holds where one
saves half the page (decision 17), else its image, a record compressed only when it carries a delta
(a record of images alone, a big transaction's pages new to the generation, is stored as it is); a spill or a cache flush writes whole page frames,
over the transaction's own frames as decision 30 has it, and the commit chains them again before its
records. A slot is told by its page field: a page number below `0x8000_0000` is a page frame, a
record's first slot carries `0x8000_0000` with its slot count, and its other slots the continuation
mark (`u32::MAX`). The frame index maps each page of a record to the record's first slot, so read
marks, backfill, the durable mark (decision 15), the checkpointer's list (decision 24) and restarts
keep their frames; whether a frame is a record is the record registry's to say (`Records::slots_of`).
A read, a checkpoint's copy and recovery go by it, a page frame read as ever and a record rebuilt
through its chain (decision 17). A chain never reaches past a page frame: a delta is made only
against the page's latest frame in the generation (`Wal::delta`), and a spill's frame is published
before the commit prepares its records, so a page spilled in a transaction is logged whole at its
commit. A mixed log's pages are shared (decisions 20 and 26): a page read from a record publishes its
image under its page and the record's frame, as a page frame's does. Generations keep the format they
began with: page-a-frame and packed logs written before read as ever, and recovery refuses a header
whose format it doesn't know rather than read it as page frames. Page numbers at or past `0x8000_0000`
(8 TiB of 4 KiB pages) are refused in a mixed generation. The raw frame interface (Turso's sync's,
and the pages a replica installs, `Db::apply_pages`) reads and writes a mixed log's page frames as
ever and refuses a record's slots; the multi-process authority refuses a mixed log, as it refuses a
packed one. A mixed generation begun now is laid out as decision 39 has it (0x414d5007): pages new
to a chain as page frames, the chains' entries in records of one slot; one begun as this decision
had it keeps 0x414d5003 and its layout.

**Why.** Decision 35's plan: on the unify audit's batched leg the log takes 16.6 bytes a byte of the
rows the calls commit, a 4 KiB page for each page a commit touches; a packed log with deltas took
3.0-3.2 (`docs/research/small-commits.md`). The packed log couldn't be the default: its spills append
records, which would grow a big transaction's log back toward decision 30's gigabytes, its pages
weren't shared, and it cost the comprehensive bench's bulk statements a wait not found (decision 16).
Spills stay page frames written over, so a transaction's log stays bounded by the pages it dirtied;
commits, which are what small writes are made of, become the records.

**The bound.** On the batched leg the log takes at most 4 bytes a byte of rows
(`small_commits_log_their_changes_not_their_pages`), against 16.6; with decision 35's copies (at
most a quarter of the log), at most 5 bytes a byte of rows written in all, against 26.4 before
either. On a quiet Mac's disks the per-flush cost (decision 35: about 107 KiB a `F_FULLFSYNC`) is
then most of what a durable call writes.

**Sources.** ARIES (Mohan, Haderle, Lindsay, Pirahesh and Schwarz, ACM TODS 17(1), 1992): log
records describe the change made to a page, physiologically, not the page. PostgreSQL's
`full_page_writes` (its documentation, `runtime-config-wal`): a page's first change after a
checkpoint is logged whole, later ones as the change, the split decision 17 makes at the page's first
version in a generation. SQLite's WAL format (sqlite.org/fileformat2.html, "The WAL File Format"):
frame headers, the checksum chain from the header's salts, recovery stopping at the first frame that
doesn't hold, and a commit valid only once a frame with its database size follows; and
sqlite.org/wal.html: frames past the last commit stand for nothing, which is what lets a spill's
frames be written over (decision 30). The LZ4 block format (decision 16).

**Evidence.** On Mac Studios, with the load gate off (Ada, 2026-11-11):
- The audit's batched leg had the disks write 217-218 MiB for its MiB of text (adas-mac-studio-3,
  `bench/records/2026-11-12-studio3-small-commits-step2.md`). That is within the 261 MiB that
  decision 35's figures give for at most 4 bytes of log a byte of rows. 13e385d wrote 595-633 MiB,
  and the log ended at 24 MiB against 208.
- Interleaved with 13e385d, best of 3 (adas-mac-studio.local,
  `bench/records/2026-11-12-studio-local-small-commits-step2-interleaved.md`):
  - The 8 MiB leg is no slower: 14.25 s against 14.28 s.
  - The comprehensive bench's bulk statements are slower: `insert_batch_100` 80.0 against 125.4 a
    second at 1 client and 92.8 against 544.2 at 32, and `delete_range_insert` 974.4 against 3,082.6
    at 32. The falsifier below holds for them.
  - The cost is the writer's, not the flush's. Step 2's writer spent 12.6-22.4 ms a commit on
    `insert_batch_100` against 2.7-3.7, while it flushed less.
  - In its profile, 72-83% of the writer's samples are a page read past the page cache from a
    record (`begin_read_record`). Nearly all of that is `pread`: in a mixed log such a read reads the
    whole record that holds the page (`RecordRead::issue`), where a page frame's read reads its one
    frame. Another 13-25% is the commit packing its records (`prepare_records`), against 0.2-1.6% for
    page frames.

**Falsified by.** The batched leg's log above 4 bytes a byte of rows (the test above); a row read back
other than as committed, on a fresh connection, after a checkpoint, after recovery or through the
shared pages, in a mixed log (`crates/engine/tests/packed_log.rs`, `shared_pages.rs`,
`reader_cache.rs` on mixed logs); an acknowledged batch lost or torn by a power cut in a mixed log,
spills written over beside records included (`crates/amphora/tests/crash.rs`); a transaction's log
holding more frames past its last commit than the pages it dirtied (`spilled_log.rs` on a mixed
log); the audit's 1, 4 and 8 MiB legs or the comprehensive bench's bulk statements slower than
page-a-frame at 13e385d.

## 38. A page is searched by key heads once its searches have paid for them (prototype, behind a switch)

G3 step 4 (PR.4, kiln plan 01a16b0f464172dc80c7a3b9a14acbce), read-path.md fix 6: B-tree search is
11.9-23.0% of a point read's user cycles at one client (`point_text` 23.0%), 43.7% of
`count_range`'s (bench/records/2026-10-23-adacachy-read-path.md).

- **What.** A directory beside a cached page, never in the file (`crates/engine/storage/key_heads.rs`):
  for each cell, 4 bytes of its first key's normalized form after the prefix every cell on the page
  shares, as LeanStore's nodes keep heads and truncate prefixes (`BTreeNode.hpp:204`, `:357`, at
  fcb64bf3; research point-reads.md §3). The cursor's binary search is today's, probe for probe; a
  head replaces a probe's decode and compare only when it orders the cell strictly, so positions and
  equal keys seen are today's. Table pages (rowids) and index pages (BINARY, NOCASE, RTRIM, either
  direction) get one; other collations and explicit NULLS FIRST/LAST don't.
- **When it is built.** Not on entry to the cache, as the plan said: a point read of a table larger
  than the cache would decode a whole leaf once to save a dozen probes. A deterministic ski rental
  (Karlin, Manasse, Rudolph, Sleator, Algorithmica 3, 1988: at most twice the offline cost): each
  search of a page without one is charged the decodes it would have saved (the bit length of the
  cell count, times the share of probes the page's last directory decided, learned as it runs);
  built when the charges reach the cell count, the decodes a build makes. Rent and price are
  counted in the same operation, so the rule holds on any machine.
- **When it is dropped.** Whenever the page's bytes may change: made dirty, buffer replaced or
  taken, renumbered, a frame written over it in place. Never built or used while dirty.
- **Its memory.** At most 4 * (page size - 8) / 6 bytes a page (SQLite's `MX_CELL`: a cell takes at
  least 6 bytes with its pointer), plus a fixed header, for each page the connection caches; the
  prefix is read from the page, not copied.
- **Hints.** LeanStore's hint array narrows a search over slots that interleave heads with offsets
  and lengths. Here the heads are a dense array of their own, 16 to a cache line, and the probe
  sequence must stay today's for identical positions; hints are not built.
- **Switch.** `AMPHORA_KEY_HEADS=1`; off, every search is today's.
- **Falsified if** cycles per in-page search on the corpus's index lookups fall by less than 30%
  against the cell-pointer search (AC-PR.4.1), or B-tree search's share of `point_text`'s user
  cycles falls by less than 30% against step 1 (AC-PR.4.2); or a build costs more than twice the
  decodes it is charged.
- **Tested.** T-PR.4.1 (`key_heads::tests`): random keys of every class, splits and deletes,
  committed rounds, seeks of every kind through a cursor with heads and one without on the same
  pages: the same results and positions always, with probes decided.

## 39. A page read past the cache reads its own frames: pages new to a chain are page frames, a chain's entries records of one slot

**Decision.** A mixed generation begun now (its header's file format
`packed::MIXED_FRAMES_FORMAT`, `0x414d5007`; what `Db::open` begins) lays a commit's pages out by what
a read of them will need (`WalFile::lay_out`). A page the log can't chain from (new to the
generation, read back past the cache, renumbered, or whose latest frame isn't the version its buffer
holds: `Logging::New`) is written as a page frame, whole and as it is, transformed straight into its
frame as page-a-frame writes it. A
page whose latest frame in the generation is the version its buffer holds is a delta (decision 17)
or, where the chain's bound ends the chain, its image (`Logging::Ends`); these entries of chains are
packed into records of one slot each (`packed::one_slot_records`): largest first, each into the first
record with room for it, an image sized by what LZ4 makes of it alone, each record LZ4'd where that
makes it smaller. An image that LZ4 can't fit in a slot is a page frame; a delta too large for a slot
takes a record of the slots it needs. The page frames come first, then the records, and the last
frame carries the commit's size. A chain may begin at a page frame: a read past the cache
(`RecordRead`) reads one frame for each entry of its page's chain, a page frame's page or a
record's slot, where decision 37's read the whole record that held each entry. The records kept
decoded take at most what a connection's page cache holds by default (`packed::decoded_budget`:
Turso's 2,000 pages, the budget Amphora's readers keep, times the page size), where 4 MiB was
picked. A generation begun before keeps its format and is written and read as it began (0x414d5002
and 0x414d5003 as decisions 16 and 37 have them, 0x414d5004 and 0x414d5005 as decision 17's chains
ended by their bytes have them); a chain that begins at a page frame is a fault there. A binary that
knows only 0x414d5003 or 0x414d5005 refuses a log of 0x414d5007 by name. A chain of 0x414d5007 ends
as decision 17 has it: a delta while the chain's deltas, this one included, take less than the page,
the depth field's width (255) its format bound.

**Why.** Decision 37's interleaved rerun found the bulk statements slower than page-a-frame, the
writer's time in reads past the cache from a record and in packing. Counted on adas-mac-studio.local-2
(an Apple M3 Ultra, 2026-11-12; the batch head 930ac68 with counters in the read and write paths,
`~/.cache/claude-scratch/probe-carrier/wal-read-measure.patch`, sha256 0daaf73a1566380b; the
suite's `insert_batch_100` and `delete_range_insert`, 200,000 items, 1 s of warm-up and 5 s, each
statement its own call; checks, not records):
- A miss needs one page and reads 40 of them. At 32 clients `insert_batch_100` read 94,517 pages
  past the cache from records. Their chains held 1.19 entries each (the deepest 9), 4,176 bytes of
  entries a miss, while each miss read 171 KB: 16.19 GB in 89,304 reads of whole records, 42 slots
  a record on average. 99.6% of the records read were compressed. The decoded records served 20% of
  the entries the chains wanted (40% at 1 client), each record read kept and soon dropped (99,954
  of 100,074 kept went again within the run). The misses took 4.78 s of the 6 s run, 50.6 µs each.
  `delete_range_insert` at 32 clients: 86,769 misses read 11.87 GB for 357 MB of entries.
- Packing is LZ4 on images. `prepare_records` took 857 ms at 32 clients, 689 ms of it in
  `packed::encode`, which compressed 589 MB of raw entries: 75% of the pages a commit wrote were
  images, and 133,724 of 185,412 were pages with no version to chain from (read back past the
  cache after the writer's cache let them go). 95% of the records carried a delta, so all of them
  were compressed.
- The misses are of pages new to chains. Of the misses on records, 86-87% were of pages whose last
  write had no version to make a delta against, 13-14% of pages in chains
  (`wal-read-whatif2.patch`, ba853ce3a319d902: at 32 clients 71,843 and 11,134; at 1 client 29,220
  and 4,919).

So a page new to a chain, written and read as page-a-frame does it, costs what page-a-frame costs;
a chain's entries, few and small, read one slot each. LZ4 then compresses only the chain's
entries: a reset image once a chain, beside the deltas that chain saved.

**Sources.** RocksDB's block-based table format (the RocksDB wiki, "Rocksdb BlockBasedTable
Format") and LevelDB's table format (`doc/table_format.md`): data blocks of about 4 KiB, each
compressed on its own, so a point read reads and decompresses one block; the unit of compression is
the unit of reading. The LZ4 block format (`lz4_Block_format.md`): a match copies from up to 65,535
bytes back in what was decoded, so an entry inside a compressed record can't be decoded without
what came before it. SQLite's WAL (sqlite.org/wal.html, fileformat2.html): a reader reads the latest
frame of a page, one frame. Levandoski, Lomet and Sengupta, "The Bw-Tree: A B-tree for New Hardware
Platforms" (ICDE 2013): a page's delta chain is consolidated into a new image because a read pays
for every delta in its chain. First-fit decreasing packing: Dósa, "The Tight Bound of First Fit
Decreasing Bin-Packing Algorithm Is FFD(I) ≤ 11/9 OPT(I) + 6/9" (ESCAPE 2007, LNCS 4614).

**The bound.** Decision 37's, kept: on the batched leg the log takes at most 4 bytes a byte of rows
(`small_commits_log_their_changes_not_their_pages`). What the layouts would have taken there, from
the same commits at 930ac68 (`wal-read-whatif.patch`, acaf450aae3fcd25): decision 37's 10,830 slots
(3.05 bytes a byte of rows); every image a page frame 16,290 (4.58, over the bound); pages new to a
chain as page frames and the chains' entries packed together 12,853 (3.62); the same, the chains'
entries in records of one slot packed greedily in the order given 14,958 (4.21, over). The batched
leg's images were 4,346 of pages new to a chain and 8,286 that ended chains: compressing the second
keeps the bound, and packing them largest first leaves less of each slot unused than packing them
in order. A read past the cache reads at most one frame for each entry of its chain, and a chain
holds its image and deltas whose bytes take less than the page (decision 17), at most
`packed::MOST_DELTAS` of them.

**Evidence.** On the batched leg the log takes 3.9 bytes a byte of rows (14,644,960 bytes of rows,
56,967,240 of log; 3.0 at 930ac68), within the bound. The suite's bulk statements, a quick check on
adas-mac-studio.local-2 (an M3 Ultra, 2026-11-12, at 443b174 against 13e385d, interleaved, best of 3,
loads 1.6 to 7.9; not a record): `insert_batch_100` 92.0 against 83.6 a second at 1 client and
411.6 against 446.0 at 32 (slower in every round); `delete_range_insert` 124.2 against 114.4 and
2,191.6 against 2,288.2. At 930ac68 on the same host the same statements ran 65.2 and 76.6, 113.2
and 822.4. A profile of `insert_batch_100` at 32 clients (AMPHORA_PROFILE, the writer's thread,
samples a commit): 3.05 against base's 2.54. The reads past the cache took 1.94 against 1.81 (a
page frame's 1.63, a chain's records 0.23), the commit 0.90 against 0.59: its preparing 0.32 against
0.12, of which the deltas' diff 0.13 and the records' packing 0.07. What is left is what deltas
cost the writer, making them and rebuilding pages from them, and no longer the reads of records.

`crates/engine/tests/packed_log.rs`: `a_page_new_to_the_log_is_read_in_one_frame` (300
rows of a page each committed at once, the database opened again, rows looked up: no read of the
log takes more than a frame, and the lookups read it) and `a_pages_chain_is_read_one_frame_an_entry`
(the same rows changed in 12 transactions, so chains run to their bound and begin again, read back
whole and by id: no read of the log more than a frame); both red at 930ac68. Also there:
`a_mixed_generation_begun_before_keeps_its_records` (a generation stamped 0x414d5003 keeps its
records and gets no page frame past its first commit, and the next generation begins 0x414d5007);
the mixed log's existing tests with the new layout's expectations (pages new to the log as page
frames, records of one slot, spills written over, rows read back through every path and after
recovery). `storage::packed`'s unit tests: a chain's entries pack into records of one slot within
first-fit decreasing's bound; an image that fits no slot is left for a page frame while a delta too
large for one takes a record of its own; and a delta found a word at a time rebuilds 400 pages
changed at random, word edges and the carried tail among the changes.

**Falsified by.** A read past the cache reading more than one frame for an entry of its chain (the
tests above); the batched leg's log above 4 bytes a byte of rows; a row read back other than as
committed, on a fresh connection, after a checkpoint, after recovery or through the shared pages
(`packed_log.rs`, `shared_pages.rs`, `reader_cache.rs`), or an acknowledged batch lost by a power
cut (`crates/amphora/tests/crash.rs`); the comprehensive bench's bulk statements slower than
page-a-frame at 13e385d, interleaved, best of 3.

## Decision (lane wal-read): a commit's records published at a lookup's cost, its deltas found a word at a time

**Decision.** Three costs decision 39 left on the writer of a mixed log, each where it begins, with
the log's layout and size unchanged:
- The record registry (`packed::Records`) publishes a record by a lookup. It kept its decoded records
  in one map keyed by generation and first slot, so publishing a record, which drops whatever was
  decoded at its first slot, passed over every record kept and over the order they were kept in. It
  now keys them by first slot, with the generation beside each (a read of another generation finds
  none), and the order of keeps passes over stale entries, swept once they outnumber the records
  kept. Slot counts are kept in frame order, so the record that covers a frame is found by a search,
  not a pass over every record of the generation.
- A commit's records are given their pages in one pass over its pages ordered by frame
  (`WalFile::publish_records`), not a pass over all of them for each record.
- A delta is found a word of 8 bytes at a time (`packed::diff`), each word compared as one integer
  and its first and last changed bytes read from the XOR's zero bytes, where a byte at a time took up
  to 16 comparisons a word. Decision 39's `SAME_BLOCK` (64 bytes, a cache line on x86-64, half the M3
  Ultra's) is gone: the integer's width is the unit, and no size is picked for a machine. The runs
  are the same bytes as before.

The bench can run the page-a-frame log in the same binary (`AMPHORA_PAGE_LOG=1`), so the bulk
statements are compared log against log, everything else the same.

Merged with decision 17's chains ended by their bytes (main's 0x414d_5004 and 0x414d_5005): a mixed
generation begun now is decision 39's 0x414d_5007, and its layout (`WalFile::lay_out`) ends a chain
by decision 17's rule as `prepare_records` does, the writer carrying each chain's bytes
(`Logged::chain`) through the publishing above; the half page decision 39's layout still took as a
delta's most is gone with it.

**Why.** Decision 39's profile of `insert_batch_100` at 32 clients put the commit at 0.90 samples a
commit against page-a-frame's 0.59: preparing 0.32 against 0.12 (the diff 0.13, packing 0.07), and
0.58 against 0.47 outside it. Decision 39 writes a record for each slot of a commit's chains' entries,
where decision 37 wrote a few records of about 42 slots, so what costs a record grew with it:
- each record published passed over the decoded records kept, up to the budget of 2,000 pages'
  bytes (`packed::decoded_budget`), hundreds of one-slot records, and over their order;
- each record's pages were found by a pass over all of the commit's pages, up to 1,024
  (`IOV_MAX`).

Both grow with the records a commit writes times what is kept or written. The diff's share is its
inner loop: a word that changed was searched byte by byte from each end.

**Sources.** Decision 39's profile and counters (adas-mac-studio.local-2, 2026-11-12). Amortized
analysis of a sweep paid for by what it removes: Tarjan, "Amortized Computational Complexity" (SIAM
J. Algebraic Discrete Methods 6(2), 1985). The XOR of two words is zero exactly where they agree, and
`u64::trailing_zeros` and `leading_zeros` count from the lowest and highest bit (the Rust standard
library's documentation); with the words read little-endian the lowest byte is the first.

**Evidence.** `storage::packed`'s unit tests: `records_published_over_kept_ones_leave_a_bounded_order`
(5,000 records published over kept ones: the order of keeps never past twice the records kept and
one more, the bytes always the kept records', another generation's record never given out, a
rollback leaving no stale keep) and `a_delta_found_word_by_word_rebuilds_every_change` (400 pages
changed at random, word edges and the carried tail among the changes, each rebuilt from its runs).
The mixed log's tests (`packed_log.rs`, `shared_pages.rs`, `spilled_log.rs`, `reader_cache.rs`)
pass unchanged. The bulk statements' interleaved bench, mixed against page-a-frame in one binary:
the coordinator's, not yet run.

**Falsified by.** The comprehensive bench's `insert_batch_100` or `delete_range_insert` slower on the
mixed log than on the page-a-frame log of the same binary (`AMPHORA_PAGE_LOG=1`), interleaved, best
of 3; a profile of the writer still showing the registry's publishing or the commit's pages found
per record; a row read back other than as committed (the tests above).

## 40. A page read finds its frame and the log's header without the shared locks (prototype, behind a switch)

G3 step 2 (kiln plan "G3 Point reads", step 2; docs/research/read-path.md fix 1(b)). Every page read
took the read side of the `RwLock` over the log's shared state, then the frame index's lock
(`find_frame`) or the header's (`wal_header`). At 32 point readers those were `point_pk`'s three
hottest leaves: `find_frame` 26.9%, `wal_header` 23.4%, `RwLock::lock_shared_slow` 13.2%
(bench/records/2026-10-23-adacachy-read-path.md).

The prototype is `crates/engine/storage/log_index.rs`. Its switch is `LogReadIndex`:
`AMPHORA_LOG_READ_INDEX` = `locked` (today's path, the default), `filtered`, `published` or
`optimistic`, or `LogReadIndex::set_process_default`. Each connection holds the log's `ReadIndex`.
The `ReadIndex` is never replaced, so reaching it takes no lock.

- **The header** is copied, all eight words, under a sequence number (a seqlock) each time its
  guard lets the lock go. A reader reads the copy and checks that the sequence didn't move. Only
  when a writer is mid-change does it read the header under its lock.
- **A page the log doesn't hold** is answered from a bitmap with one bit per page number. The
  bitmap's size is the highest page the log holds, doubled as it grows. It is exact: it never
  reports a page as held when no frame holds it, except frames cut since the bitmap was built. It
  has no false-positive rate to choose, unlike a Bloom filter. The empty log (`max_frame <
  min_frame`) already skipped the index.
- **Way (a), `published`**: the whole index is copied and published through an `ArcSwap`. A reader
  whose snapshot reaches past the copy publishes a new one. Each new copy costs O(pages in the log).
  While another reader publishes, it reads under the lock. A read-only workload publishes once.
- **Way (b), `optimistic`**: a table of each page's latest frame, in shards. Each shard has its own
  version, after LeanStore's optimistic latches (Leis et al., ICDE 2018): read the version, read
  the slot, check the version. There is one shard for each thread that can run at once
  (`available_parallelism`, the reader count where the process runs). There are twice as many slots
  as the log has pages, so each shard is at most half full. Knuth (TAOCP vol. 3 §6.4) gives linear
  probing 2.5 expected probes for a miss and 1.5 for a hit at that load. A shard that would pass
  half full doubles the table. A read whose version moved, or that finds a frame newer than its
  snapshot, is answered under the lock: one try, never retried. An append costs O(1).
- **Changes other than an append** (a rollback, slots written again, a log restart, recovery,
  restore) move the index's generation on. The next reader rebuilds from the index under its lock.
  A reader that loaded a structure just before the generation moved still answers correctly:
  frames that are cut or written again lie past every committed snapshot, and a restart happens
  only when no reader reads the log.
- Not taken: the multi-process backend (`ShmWalCoordination`). Its local index is filled from other
  processes' frames, so its fallback reads under the locks.

Tests: `storage::wal::tests::g3_2_*` (the model, and readers beside a writer) and
`crates/engine/tests/log_read_index.rs` (a crash at every write of a checkpoint and of a log
restart). The measurements are the coordinator's: the 32-client profiles, the suite at 10M rows
and `bench/prof_parts.py`'s shares, with each way switched on. One way is kept and the others are
removed, as the numbers favour. Falsified, as read-path.md has it, if with the log empty after the
settle `point_pk`'s instructions per statement at one client fall by less than 15%, or if
`table_count` at 32 clients rises less than 3x. The step's own bars: the log's and locks' share of
point reads at 32 clients is at most 5%, and `point_pk` at 32 clients is at least 4x one client.

## 41. Skipping the descent: an adaptive hash against a learned leaf locator (prototypes, behind a switch)

G3 step 5 (G3.5, kiln plan 01a16b0f464172dc80c7a3b9a14acbce): the descent is within B-tree search's
11.9-23.0% of a point read's user cycles (bench/records/2026-10-23-adacachy-read-path.md), so with it
gone a point read gains at most 1.14-1.30x (Amdahl's bound).

- **What.** Two cores that name a leaf without descending (`crates/engine/storage/descent_skip.rs`):
  prototype D, InnoDB's adaptive hash index (`btr0sea.cc`; MySQL 8.0 reference manual §15.5.3), a
  hash from a key to its leaf page built a leaf at a time; prototype E, a learned, error-bounded
  locator from a rowid to its leaf for rowid tables (Kraska et al., SIGMOD 2018, RMI; Ferragina and
  Vinciguerra, PVLDB 13(8) 2020, PGM): lines through the leaves' first rowids.
- **Never trusted.** The caller reads the page named and takes its answer only if it is a leaf whose
  first key is at most the key and last at least, and no page has left its tree since the prototype
  learned it (a stamp the caller raises on every freed page). Leaves of one tree hold disjoint
  ranges, so that leaf is the only place the key can be. Anything else is refuted, forgotten, and
  the descent runs as today and teaches the prototype the leaf it found.
- **When D builds.** A ski rental (Karlin, Manasse, Rudolph, Sleator, Algorithmica 3, 1988), as
  decision 38: a descent ending at a leaf without a hash is charged the probes it made above the
  leaf (what a hit saves), counted as it makes them; the hash is built when the charges reach the
  leaf's cell count (a key hashed per cell). The one assumption, that a hash insert costs what a
  probe does, is measured by `descent_skip_bench`.
- **E's error bound.** Each segment is the line through its first and last fence; its bound is the
  largest distance between where the line puts a fence and where it is, the fit's own. The worst
  segment is split at its worst fence until all are exact, and the number of splits with the fewest
  expected probes over all fences (a search over the segments plus one over the window, in
  integers) is kept: one split at a time stalls where a bit more search buys a bit less window. Learning a leaf refits only the
  segments its fences touched.
- **Upkeep.** Lazy: paid where a write moved a leaf and a lookup noticed (a refute, then a learn).
  Every freed page drops everything either holds: coarse, and right for a prototype; a per-page
  owner would keep what a free didn't touch.
- **Memory.** D holds at most its budget of entries (keys plus leaves tracked), evicting the least
  recently used leaves and refusing a leaf larger than the budget; E at most its budget of fences,
  refusing past it. The caller derives both from the memory it grants.
- **The hook.** `AMPHORA_DESCENT_SKIP` = `off` | `hash` | `locator`, default `off`. Each
  connection's pager holds the hook: hash and locators learned by its read cursors' descents, and
  the stamp they are valid under: the connection's own count of pages that left their tree (every
  free, every rollback to a savepoint or of a transaction) with its snapshot's identity (its WAL
  position). A pager-wide count isn't sound: another connection's free happens at a snapshot this
  one doesn't see, and a page learned under one snapshot proves nothing under another. When the
  stamp moves, everything learned is dropped. Only a cursor opened to read consults it, and only
  for a point seek; the named page is used only if it is loaded, clean and unlocked in the
  connection's own cache (no I/O, nothing a write is changing), and only after `table_leaf_span`
  proves the rowid in range; anything else descends. The cursor then stands on that leaf alone:
  SQLite's leaves have no sibling links and the stack has no ancestors, so every other move (next,
  prev, insert, delete, the current-leaf fast path) first descends again, to the row it stands on
  or, with none, to the rowid its seek sought, which is where the plain descent would have put it;
  a move to the root rebuilds the stack anyway. Bounds come from the connection's page cache, as
  only cached pages are used: the locators together hold at most a fence a cached page, the hash
  at most as many keys as the cached pages hold cells (SQLite's `MX_CELL`, btreeInt.h). Tested
  through the real cursor by T-G3.5.4; AC-G3.5.2's measurement is `bench/descent_skip.sh`.
- **Falsified if** point_pk at one client with the winner rises less than 20% against steps 2-4
  (AC-G3.5.2), or the winner's upkeep on the corpus's writes costs more than its lookups save.
- **Tested.** T-G3.5.1: two tables sharing pages under random inserts, deletes, splits and merges,
  every lookup through each prototype against a plain descent: the same row or absence always, with
  hits, refutes and stamp drops counted. T-G3.5.2: over complete fences the locator names the
  descent's leaf. T-G3.5.3: on the engine's own pages, each named page proven by its bytes
  (`table_leaf_span`) gives the leaf and cell a cursor's descent stands on. Measured by
  `descent_skip_bench` (the model: lookups, builds, upkeep a write) and `descent_skip_engine_bench`
  (the engine's pages: lookups and E's rebuild).

## 42. The page path SQLite takes: plain pins, the pool's buffer counted once, misses read in line, checksums once (prototype, behind a switch)

G3 step 3 (kiln plan 01a16b0f464172dc80c7a3b9a14acbce), read-path.md fix 2: the page cache and pager
are 15.6-21.4% of a point read's user cycles at one client, checksums 2.9-4.1%, atomics 1.6-2.3% and
I/O completion 2.7-3.6%, together 23.0-30.3%; SQLite's page cache and pager are 1.8-4.6% of its own
(bench/records/2026-10-23-adacachy-read-path.md).

- **What.** `PagePath::Plain` beside today's path (`crates/engine/storage/pager.rs`):
  - *Plain pins.* A page a connection makes on this path is pinned with a load and a store, no
    read-modify-write: SQLite's `nRef` (`pcache.c`, `sqlite3PcacheRef`, `sqlite3PcacheRelease`).
    Only the connection that made a page pins it, one thread at a time; the flag is set when the
    page is made, never after, so a page's pins are of one kind all its life.
  - *Shared pages counted once.* A page the pool keeps is taken as the pool's own buffer, its
    count taken once as the page enters the connection's cache, its later pins the connection's
    own: PostgreSQL's private refcounts (`bufmgr.c`, `PrivateRefCountEntry`: a backend's first pin
    touches the shared buffer header, the rest its own array). Today's path copies the image out
    (decision 29) and the plain path is measured against it. A write copies a shared buffer first
    (`Pager::add_dirty`, decision 20), so a counted image never changes.
  - *Misses read in line.* A page the log doesn't hold, from a backend that completes reads in line
    (`File::pread_inline`: one `pread(2)` on Unix), with no completion, closure or wrapper
    completion for the checksum; other backends (io_uring, Windows, a codec) answer `None` and read
    as today. It reads into the page the cache's last eviction left, with its buffer, when no one
    else holds them: `pcache1` recycles the page a fetch evicts (`pcache1FetchStage2`). The cache
    keeps at most that one spare, the page an eviction frees, taken by the read that follows it.
    The log's frames are read as today: the log's read path is G3 step 2's (decision 40).
  - *Checksums once.* Checked as the page enters the process from the file (`read_page_inline`,
    PostgreSQL's rule: a page is verified as it is read into shared buffers, never while it stays
    there), then trusted while the pool or the connection's cache keeps it; a page that fails is
    neither cached nor shared.
- **Switch.** `AMPHORA_PAGE_PATH=plain` in the environment for every pager the process opens;
  `Pager::set_page_path` for one connection's.
- **Not yet.** A miss with no spare (a cache not yet full) still allocates the page and its
  buffer's `Arc`; every miss still allocates the cache's entry (`Box<PageCacheEntry>`); a cache hit
  still clones the page's `Arc` (one read-modify-write). The plan's bars (at most 2 atomic
  read-modify-writes and 1 allocation per `point_pk` lookup) are measured on what this builds first.
- **Falsified if** the page cache, pager, checksums, atomics and I/O completion together stay above
  15% of a point read's user cycles with the switch on (read-path.md), or the plain path's point
  reads are not faster than today's at one client, or counting the pool's buffer once costs more at
  32 clients than decision 29's copy (perf c2c on the pool's buffers).
- **Tested.** T-G3.3.1-3 (`crates/engine/tests/shared_pages.rs`, `g3s3_*`): the shared-pages model
  on the plain path (16 seeds, page and mixed logs); each path reads the rows today's reads, from
  the log, the file and memory, with and without shared pages; a byte flipped in the file is
  refused with a checksum error through a fresh connection on each path. T-G3.3.4
  (`storage::pager::tests::g3s3_plain_pins_count_as_atomic_pins_do`). T-G3.3.5
  (`g3s3_plain_misses_read_into_the_pages_evictions_leave`): a table four times a 200-page cache
  read twice on each path: the same rows, the plain path's misses in recycled pages. T-G3.3.6
  (`crates/engine/tests/page_path_allocs.rs`, a counting allocator): allocations per point lookup
  by a prepared statement, one small run on adacachy (debug build, 2026-11-21, f046bc1, one run, a
  smoke check, not a benchmark): misses without shared pages 15.98 today against 6.00 plain, with
  shared pages 7.99 against 7.00; hits 5.00 on both (the statement's own: its row vectors).

## 43. Point readers write no line another reader writes: the reader taken from a slot of its own, shared pages read under a version check (prototype, behind a switch)

G3 step 6 (kiln plan "G3 Point reads", step 6, prototype F; docs/research/read-path.md; engine.md
decision 3). At 32 clients `point_pk` scaled 3.8x over one client (1,315,252 against 343,467 a
second in the v6 matrix), the log 66.0% and locks 14.6% of its user cycles. Step 2 (decision 40)
takes the log's locks out. Two shared writes per read are left that this step removes, each behind
its own switch:

- **The reader checkout** (`crates/amphora/src/store/checkout.rs`, `ReadCheckout`;
  `AMPHORA_READ_CHECKOUT` = `pool`, the default, or `slots`). The pool took its mutex to take a
  reader and again to give it back, woke the waiters, and held the read side of one `RwLock` (the
  gate) through the read: four writes to lines every reader writes. With `slots`, each reader
  lives in a slot of its own, on a 128-byte line pair of its own, and a thread takes the slot its
  number names (a number taken once per thread). With no more threads than slots, a read writes only
  its own slot's lock. A thread whose slot is busy tries the others, then waits on a condition
  variable; a read that ends wakes waiters only when its read of the waiters' count says there are
  some. Holding a slot stands for holding the gate: the log's restart and the readers' release take
  every slot, and giving them back wakes the reads that came meanwhile (no read ends to wake them
  while every slot is held).
- **Shared page lookups** (`crates/engine/storage/optimistic_pages.rs`, `SharedPageReads`;
  `AMPHORA_SHARED_PAGE_READS` = `locked`, the default, or `optimistic`). A lookup took its shard's
  read lock, a write to the lock's word. With `optimistic`, frames sit in front of the shards, each
  image held as atomic 64-bit words under a sequence that is odd while a writer changes it
  (LeanStore's optimistic latch: Leis et al., ICDE 2018; Leis et al., DaMoN 2016). A reader reads
  the sequence, the key and the words, then the sequence again; equal and even, it has the whole
  image of the key it asked for, else it asks the shard under its lock, once, never retrying. Safe
  Rust: the words are atomics read relaxed, ordered by an acquire fence before the second read of
  the sequence (Boehm, "Can Seqlocks Get Along with Programming Language Memory Models?", MSPC
  2012). A key's bytes never change (a frame's image is committed; a file version is forgotten
  before the file's page is written), so a whole image read here is what the shard would give; a
  forget clears the frame before it returns. An image found under its shard's lock is copied into
  its frame (direct-mapped by the key's hash) by a writer that takes the frame by turning its
  sequence odd with a compare-and-swap (LeanStore's exclusive latch, on the word readers check), so
  writers of different frames share no line and no lock spans the frames; an offer that finds its
  frame taken leaves the image out, and a forget waits for the frame's writer (one image's stores). Lookups answered here are counted on a stripe per
  thread, so the count writes no shared line.
- **The frames' budget.** The pool gives the frames one shard's share of its budget (`per_shard`
  frames of a page each) and charges it, so the shards keep that many fewer pages: the memory stays
  the pool's. The share is derived from the pool, not picked, but whether one shard's share is the
  right split is for the 32-client runs to say: c2c's HITM records and the frames' hit share
  against the shards' decide it (and a frame count learned from the lookups' spread, if they show
  one share is wrong).
- Not taken: the plain page path's `take_count` (decision 42's, lane g3s3's) still looks up under
  the shard's lock; a lock-free form of it needs a counted buffer the frames don't hold.

Tests: `store::checkout::tests::g3_s6_*` (slot readers never share a reader, a waiter is woken,
release lets every reader go) and `storage::optimistic_pages::tests::g3_s6_*`, among them the
step's model: 32 readers and a writer, the writer committing frames into a pool too small to keep
them, with a checkpoint's nemesis of file versions forgotten and written again; every image a
reader gets is whole and the version its snapshot asked for. Its short instance runs in the unit
tests; `AMPHORA_G3S6_SECONDS=60` runs it for 60 s. The measurements are the coordinator's: the
60 s run, `perf c2c` at 32 clients with both switches on, and the suite's `point_pk` at 1 and 32
clients. Falsified if `point_pk` at 32 clients stays under 12x one client with both on, or c2c
finds more than one contended line per million lookups on the read path.

## Decision (lane constants): one process memory budget (2026-11-21)

Ada's rule: no picked numbers. The process's memory bounds (an image's 256 MiB, the owners' queues' 512 MiB each)
were each picked, and nothing knew what the process may hold where it runs: decision 26 read the limit on Linux
only, and a caller fell back to a number of its own elsewhere.

**The decision (MB.1).** `memory::limit` is the smallest limit the platform states: on Linux the limits of the
control groups the process runs in (v2, else v1) and `/proc/meminfo`'s `MemTotal`; on macOS `hw.memsize` and on
Windows `GlobalMemoryStatusEx`'s physical memory (the engine makes those two calls, `io::system_memory`, since
Amphora's crate denies `unsafe`); and on Unix the process's own soft `RLIMIT_AS`, `RLIMIT_DATA` and `RLIMIT_RSS`.
`memory::resident` is the bytes the process holds resident (memory-stats: `/proc/self/statm`, `task_info`,
`GetProcessMemoryInfo`). `memory::Budget` shares the limit out by measured use, no share picked: a use (the owners'
queues, images, caches) charges what it holds as it takes it and gives it back when the charge drops; its room is the
limit less what the rest of the process holds, the larger of the resident bytes and every charge counted, its own
given back. Both readings are taken again at every ask, so a limit moved while the process runs is followed. Where
the limit can't be read the budget refuses, `Unavailable`, naming the platform: there is no fallback.

**What is left.** A Windows job object's memory limit isn't read (`QueryInformationJobObject`): the machine's memory
bounds such a process from above. The owners' queues don't charge the budget yet (their piece, next in the goal's
list: `QUEUED_BYTES`, `OUTGOING_BYTES`, `OUTGOING_MESSAGES`).

**Tests.** In `memory::tests`: `mb1_a_uses_room_is_what_the_rest_of_the_process_leaves` (T-MB.1.1),
`mb1_no_limit_read_is_a_typed_refusal_and_the_limit_is_read_again` (T-MB.1.2),
`mb1_unread_resident_bytes_fall_to_the_charges_counted` (T-MB.1.3), `mb1_this_process_reads_its_limit_and_resident_bytes`
(T-MB.1.4, on Linux, macOS and Windows); the engine's `io::system_memory::tests::the_platform_says_its_physical_memory`.

## Decision (lane perf-A9): a call is charged the answer bound its caller declares (2026-11-23)

A request admitted to a database's writer is charged what it and its answer can hold (MB.1, `Admission::bytes_for`).
A call's answer is whatever its closure returns, which no API bound holds, so `Database::call` was charged the full
`REQUEST_MAX` (32 MiB): about 25 calls filled a 1 GiB scope.

**The decision.** `Database::call`, `call_session` and `call_nowait` take the caller's bound on its answer,
`AnswerMax<R>`: charged at admission, and checked inside the call's transaction before it commits (`AnswerBytes`, its
bytes inline and on the heap), so an answer past it rolls the call back, refused `TooLarge`. A caller derives the bound:
`AnswerMax::fixed()` for an answer that holds nothing on the heap (its type's size, checked by the type,
`FixedAnswer`), a count of rows for a bounded read, or the API's own `RESULT_MAX` for an answer only it bounds (an SQL
write's frames over the wire, as an SQL batch is charged, lane perf-A5).

**Evidence.** Lane perf-A8's A/B (bench/records/2026-11-23-adacachy2-perf-cross.md, r949): 1k and 10k calls in
flight, unconstrained and in a `MemoryMax=1G` scope, against the flat `REQUEST_MAX` and the answer's measured bytes
charged on the writer. The flat charge refused 86-98k calls per 3 s in the scope; the declared bound beat the measured
one on p50 (13-62%) and calls a second (10-36%) in all four cells and on the tails in three. Ada's decision: the
declared bound, the only call charge.

**What is left.** The cross-group steps' calls on the owner's own session (`call_internal`) and a replicated owner's
command queue (replication.md, "A call is charged its worst case") still charge a call `REQUEST_MAX`.

**Falsified if** a caller's derived bound refuses an answer its type or its read allows, or the declared bound loses
p50, calls a second or the tails to another charge at 1k-10k calls in flight on the same machine.

## 44. A commit lays its chains' pages out in the buffer pool's frames, and finds its deltas in one buffer it reuses

**Decision.** A mixed log's commit (`WalFile::lay_out`, decision 39) takes no buffer of its own for
the images of the pages it chains. Each such page is transformed into a frame from the log's buffer
pool, as a page frame is (`prepare_wal_frame_header`); its delta is found against that frame's
body; and an image the packing leaves a page frame (`OneSlot::frames`) is written from that frame as
it is, where it was copied from the commit's buffer into a new frame. The images that become
records' entries are read from their frames while the records are packed, and the frames go back to
the pool when the commit is laid out. A delta's runs are written into one buffer the commit reuses
from page to page (`packed::diff_into`), and each delta keeps a copy of its own length, where each
took a buffer of the most a delta may hold, up to a page, and used a few bytes of it. The log's
format, layout and size are unchanged: the same frames and records, byte for byte. No size is
picked: a frame is the page size and its header (the format's), and the reused buffer grows to the
largest delta the commit finds.

**Why.** A profile of the suite's `insert_batch_100` at 32 clients on adas-mac-studio-3 (an Apple
M-series Mac Studio, 2026-11-22, AMPHORA_PROFILE, 20,000 items, 4 s; checks, not records) put 21%
of the process's samples in `lay_out`, and most of that in the allocator rather than the packing:
- growing the commit's buffer of images a page at a time (`Vec::resize`) and freeing it: 409 of
  `lay_out`'s 723 samples, a buffer of up to `IOV_MAX` pages made and dropped every commit;
- each delta's buffer of up to a page (`packed::diff`): 78;
- each record's encoding: 109 (left as it is, below).

The page-a-frame log takes its frames from the pool and writes each page into its frame once, so
the mixed log's writer paid for allocation that page-a-frame's never did. After this decision the
same profile at 1 client shows no sample of `lay_out` in the images' buffer or the deltas'.

**What is left.** The same profile at 1 client puts 227 of `lay_out`'s 283 samples in freeing a
page's log base (`pager::LogBase`, the page's bytes as the log holds them, kept from its first
change in a transaction so its delta can be found), an allocation of a page made for each page a
transaction changes and dropped at its commit; and each record's encoding still allocates its
payload. Both are the next costs to take to the pool.

**Sources.** Decision 39's profile and this one's. SQLite's WAL writer (`walWriteOneFrame` in
`wal.c`) writes each page from the pager's own buffer into the log, with no copy of the commit's
pages of its own. The page-a-frame path here (`WalFile::prepare_frames`) takes its frames from the
buffer pool.

**Evidence.** The mixed log's tests pass unchanged (`packed_log.rs` 16, `shared_pages.rs` 6,
`spilled_log.rs` 14, `reader_cache.rs` 2), as do `storage::packed`'s 19 and `storage::wal`'s 103
unit tests, among them the delta that rebuilds 400 pages changed at random (through `diff`, which
now writes through `diff_into`). A tiny interleaved check on adas-mac-studio-3 (`bench/log_ab.sh 2
insert_batch_100,delete_range_insert --items 20000 --clients 1,32 --seconds 2 --warmup 1`, best of
2, with three CI jobs on the host; not a record), mixed over page-a-frame, before and after:
`insert_batch_100` 0.858 and 1.076 at 1 and 32 clients before, 0.983 and 0.930 after;
`delete_range_insert` 1.060 and 1.392 before, 0.971 and 0.965 after. At this size the rounds
differ by more than the logs do: the run of record is the coordinator's.

**Falsified by.** The comprehensive bench's `insert_batch_100` or `delete_range_insert` slower on the
mixed log than on the page-a-frame log of the same binary (`bench/log_ab.sh`, interleaved, best of
3); a profile of the mixed log's writer still showing the commit's images' buffer or a delta's
buffer of a page; a row read back other than as committed (the tests above).

## Decision (lane constants-22): a busy statement wakes on the lock's release (2026-11-22, not yet landed)

- **What was there.** A statement that found the database busy (another connection holding the write lock, a read
  mark or the checkpoint lock) retried on SQLite's fixed schedule (`crates/engine/busy.rs`: 1, 2, 5 ... 100 ms), and
  `Statement` woke its waker at once with `StepResult::Sleep`, so an async caller spun until the delay passed.
  Amphora's connections set no busy handler, so `db::step` did its own retry: 1 ms (`BUSY_WAIT`) up to 10,000 times
  (`BUSY_TRIES`). Both are picked numbers, and both wake on time where an event says when to try again (found by
  lane colcopy-8; goal dfabffcc).
- **The rule.** Every release of the log's locks in this process (the write lock, a read mark, its downgrade and the
  transient releases inside a read's start, the checkpoint lock, the vacuum lock) and every published commit fires
  one event, `busy::LockRelease`, held in the log's shared runtime. A statement reads the event's count before it
  tries its locks; when it finds them busy it hands its waker to the event for the next release after that count,
  or, if one came already, tries again at once, so a release between a failed try and the wait is never missed. The
  busy timeout is its deadline only (`BusyHandlerState::until_released`). The wakers held are those of the statements
  busy now, each once.
- **Amphora's connections** wait with no deadline of their own (`busy_timeout` = `Duration::MAX`): every holder is a
  connection of this process, and each holder's transaction ends, by its end or by its own request's deadline, which
  releases the lock. Setting the busy timeout per request was weighed and left: the engine's setter makes every
  cached statement prepare again. `db::step` steps with a waker that unparks its thread.
- **The control, behind a switch.** `Connection::set_busy_wake(BusyWake::Schedule)` keeps SQLite's schedule. A
  statement stepped without a waker, or on a log shared across processes (whose holders elsewhere release unseen),
  keeps it too.
- **What decides it.** `db::tests::constants_22_a_busy_write_wakes_on_the_write_locks_release` prints, under each wake,
  the time from the holder's commit to the waiter's insert done. The run of record (the coordinator's): that test
  repeated over the concurrency ladder of waiters (1 to 4096 writers on one database), wake latency p50/p99/p99.9 and
  CPU per wait under each switch.
- **Still open.** `WalFile::begin_read_tx` has its own retry for a read mark that moved under it (5 at once, then
  yields, then `(n - 9)^2 * 39` microseconds, 100 at most: SQLite's `walTryBeginRead`); it is the same kind of wait and
  could wait on the same event.
- **Falsified by.** A busy statement that never wakes though the lock was released (a missed release); wake latency
  under `Release` above `Schedule`'s at any rung of the ladder.

## 45. A restart switches the log to its second segment: no read holds a write back (decided 2026-11-23, not yet built)

**What was there.** Decision 4 starts the log again (`Readers::restart_if_asked`, `crates/amphora/src/store.rs`) with
the readers' gate taken for writing and every slot taken: the writer waits for every read under way, and a read that
arrives meanwhile waits behind it. A read holds its snapshot, and the gate, until its answer is out; a streamed answer
until its last frame is written to its client, which can take the request's whole deadline (`serve.rs`,
`Answering::send`: each frame's write gets the request's time left and is refused past it). Lane P4 found the result:
one slow streaming client holds every write, and every new read behind the waiting writer, for up to the deadline (5 s
by default) at each restart. The writer waits because a restart writes the log again from its first frame: a read
whose snapshot ends inside the log would read a later generation's frames in place of its own.

**Decision (the coordinator, 2026-11-23).** The log is two segments, as in SQLite's wal2: the file it has today
(`<db>-wal`) and a second (`<db>-wal2`). The writer appends to one of them, the current one. Where decision 4 starts
the log again, the writer switches instead. It writes the other segment's header (new salts, the checkpoint sequence
one past the current one's), syncs it before any frame follows (decision 13's rule), and appends there from its first
frame. It takes no gate and waits for no read. The segment it left is now closing. Every frame in it was committed
before any frame of the current one.

- **The shape.** Each segment is `Current`, `Closing { reads, copied, shipped }` or `Free`. A switch turns the current
  segment into `Closing` and a `Free` one into `Current`. A reset turns `Closing { reads: 0, copied: true, shipped:
  true }` into `Free`. The writer switches only into a `Free` segment. A read begun while the closing segment is not
  yet copied whole into a flushed file takes that segment whole in its snapshot, plus the current one up to its last
  commit, and counts itself on both. A read begun once it is copied takes the current segment alone and reads the
  rest from the file, where every closing frame now is, and counts itself on the current one only. Without that
  rule, reads that overlap with no gap between them would keep the closing segment's count above zero for ever, and
  the writer could never switch again (revised 2026-11-23, lane B6). A page is looked up in the current segment,
  then the closing one if the read counts on it, then the file. A read takes read mark 0 (the file alone) only
  when it counts on no closing segment (lane B10). Right after a switch the current segment is empty, so its
  frames are all copied (0 of 0), and the one-log test would send the read to the file alone, which lacks the
  closing segment's frames not yet copied. Each segment keeps its own frame index: the frame cache and decision 40's
  lock-free index are per segment, because frame numbers start again in each.
- **Checkpoints.** The checkpointer copies the closing segment first and the current one's frames only after it. The
  other order is wrong: a frame copied from the current segment would be overwritten by an older one copied after it.
  It copies the current segment only once the closing one is reset (the coordinator, 2026-11-23, of lane B10's two
  ways). A read begun before the switch searches the closing segment up to its mark and then the file, never the
  current segment. When its mark is the closing segment's last frame, the closing segment's copy goes whole while it
  reads, and a page of the current segment copied then would reach the file, where the read would find a version
  newer than its snapshot. One log has no such case, because a read mark bounds every copy. Waiting for the reset
  needs no new state, delays that copy by at most the bound below (2D plus one copy), and keeps the order above.
  Counting the reads begun before the switch would allow it sooner, at the cost of one more counter.
  Its passive copy of the closing segment stops at the oldest snapshot inside it, and only reads begun before the
  switch hold one there. Reads begun after the switch see the whole closing segment and hold nothing back. The copy
  and the file's flush that decision 4's restart made on the writer move to the checkpointer. A switch costs the
  writer one header write and its sync, against today's copy of what is left, the file's flush and the header's sync
  (10-14 ms on fedora, decision 4).
- **The reset, on an event.** The closing segment is reset (`Free`) once it is copied whole, the file is flushed, a
  backup has shipped it (decision 31's backup rule moves from the restart to the reset), and no read can look into
  it. Each read counts itself on the segments its snapshot reaches (The shape). The reset is made by whichever of
  its events comes last: the give-back of the last read counted on the segment (its count reaching zero), the
  checkpointer's pass that copies its last frame and flushes the file, or, for a backed-up database, the backup's
  word that it shipped the segment. Each already happens, so the reset needs no timer and no poll. It writes nothing.
  The segment's frames stay until the next switch writes a new header over them (decision 13). Decision 31's cut of
  a log past twice its bound applies to a segment at its reset.
- **Bounded.** A read holds a segment for at most its request's deadline D. Reads begun before the switch end by D
  after it. Reads begun before the closing segment's copy finished end by D after that, and no read begun after
  it counts on the segment (The shape). So a closing segment is free
  within 2D plus one copy of it after the switch. If the current segment reaches its bound first, the writer keeps
  appending to it past the bound and switches at its first turn after the reset. It waits for nothing either way.
  Each segment then holds at most its bound plus what the writer appends in 2D plus one copy. The A/B below measures
  that against today's twice-the-bound.

**On disk (revised 2026-11-23, Ada: "There are no 'older databases' besides any ones you've created for test.
Amphora is not even released. This is not a concern.").** The two segments replace the one log outright. There is no
upgrade step, no reading of a one-segment format and no switch between the two, and the one-segment restart is deleted.
Each segment's header is the log header the engine writes today (format, page size, checkpoint sequence, salts,
checksums), so a log whose `-wal2` is absent, or holds no valid header, is one of the two-segment log's own states (one
segment `Current`, the other `Free`), read by the same recovery.

**What plain SQLite sees (the coordinator, 2026-11-23, on lane B6's question).** Three rules.
- *Amphora opens the WAL-mode files SQLite writes.* Such a file is the one-segment state above, read at no cost.
- *Plain SQLite refuses an open Amphora database, and one a crash left.* Bytes 18 and 19 of the database header carry
  3, the value SQLite's wal2 writes. SQLite refuses a read version above 2 ("If a database file with a read version
  greater than 2 is encountered, then that database cannot be read or written", sqlite.org/fileformat2.html, 1.3.3;
  `btree.c` `lockBtree` returns `SQLITE_NOTADB` for `page1[19]>2`; wal2.md: "Attempting to use any other version of
  SQLite results in an SQLITE_NOTADB error"), so it never reads `-wal` without `-wal2`'s commits. The open writes 3 where it rewrites a rollback file's 1 to 2 today
  (`crates/engine/database.rs`, 2272-2287), for a 1 and a 2 alike, in the database's first commit, before any other
  frame. The header in memory then carries 3, so every later frame of page 1 does too. Whenever `-wal` exists and the
  file is not empty, SQLite opens `-wal` before it reads page 1 (`pager.c` `pagerOpenWalIfPresent`, called from
  `sqlite3PagerSharedLock`) and takes page 1 from its frame there if it has one, so the mark must be in the log's
  frames of page 1 as well as in the file. From that first commit on it is in both. Any other value stays `Corrupt("Invalid read_version: …")` at open (2184-2190).
- *A cleanly closed Amphora database is a plain SQLite file.* The last connection's close (`Connection::close`,
  `Pager::checkpoint_shutdown`) folds both segments into the file. It copies the closing segment, then the current
  one, syncs the file, truncates both segments to nothing and syncs them. Only then does it write 2 into bytes 18 and
  19 of the file's first page, in place and not through the log, and sync the file again. sqlite3 and other tools
  then read a WAL-mode file with an empty log. The order is the rule. A 2 that is durable while either segment still
  holds frames, or while a truncation is not, lets SQLite read `-wal` alone: it misses `-wal2`'s commits, or reads the
  closing segment's older frames over the newer pages in the file. A 2 written through the log is such a frame. A
  crash before the 2 is written leaves 3: SQLite refuses the file, and Amphora recovers it as it recovers any crash.
  A crash while it is written leaves 3 or 2, and with 2 every commit is already in the file and both segments are
  empty. A close that cannot fold
  (the shutdown checkpoint gives up after three `Busy` answers, as it does today) also leaves 3: that database was not
  closed cleanly, and SQLite refuses it.

**Crash recovery across a switch.** Recovery reads both segments' headers. A segment whose header checksum fails is
empty. Of two valid segments, the one whose checkpoint sequence is one past the other's is current and the other
closing: a switch writes the other's sequence plus one, and the sequence is a u32 that wraps (`wal.rs`
`restart_wal_header` handles its wrap), so "the higher" is wrong at the wrap. A pair neither of which follows the
other is no switch's, and is refused as corrupt (`storage::log_segments`, `Segments::recover`, on branch
`perf/pagepath-d45`). Each segment's frames are recovered as one log's are today (salts and checksums, up to the last valid commit frame), the
closing one's first. A recovered database has copied nothing of either segment, as one log's recovery does today, so
the checkpointer copies the closing segment and then the current one. A crash at each step of a switch leaves:

1. Before the other segment's header is synced: that segment holds its old header (two sequences back) or a torn
   one. Nothing was appended after the switch began, so every commit is in the current segment.
2. After the header's sync, before any frame: two valid segments, the new one empty. Every commit is in the closing
   one.
3. Frames in the new segment: its valid prefix, as one log's recovery finds today.
4. After a reset: the reset wrote nothing, so recovery finds the closing segment valid and recovers its frames again.
   They are already in the file, and copying them again writes the same pages before the current segment's newer
   frames are copied.

**Where it lands in the engine.** Today there is one log in every structure that keys on a frame number. These are
the places a segment is added (`crates/engine/storage/`):
- `wal.rs` `restart_wal_header` (8142-8183) becomes the switch. `begin_restart` and `end_restart` (1731-1757) take
  every read mark exclusively and refuse a restart while any is held: that is the rule the switch drops, and both are
  deleted with the rest of the one-segment restart (the rewrite of the log from its first frame, `Readers::
  restart_if_asked`'s gate taken for writing and `Slots::take_all` in `store.rs` 1164-1183). No path starts a log
  again from its first frame while a read can look into it, so none is kept beside the switch.
- `WalSharedMetadata` and `WalSharedRuntime` (3703-3787) hold one header, one file, one frame cache, one `nbackfills`
  and five read marks. Each becomes one per segment.
- `try_begin_read_tx`'s read marks (1528-1604) and `WalSnapshot` (196-211): a snapshot names its segments.
- `find_frame` (5343-5422, 1471-1498, 1324-1340), `log_index.rs` `ReadIndex` (387-627, which assumes no reader at a
  restart, 23-25), and `frame_offset` and `read_frame` (6875-6879, 5426-5498): a frame is a segment's.
- The pager's shared pages key a version by its generation and frame (`pager.rs` 4787-4790, `Version::Frame`), and
  take the generation from the connection's position (`connection_wal_pos`), which is the current segment's. Frame n
  of the closing segment and frame n of the current one would then share a key, and a read could be handed the other
  segment's page. So `find_frame` answers with the segment its frame is in, and the key takes that segment's checkpoint
  sequence (checked by lane B8, 2026-11-23). The page cache's forget rule needs no change (decision 18, `read_change`,
  `wal.rs` 5114-5136): a switch changes the current segment's checkpoint sequence, so a connection whose last read was
  in the other generation forgets every page, as after a restart today. A packed delta chains only from a version of
  the same generation (`chains_from`, `wal.rs` 4430-4436, on both of the commit's paths), so none chains across a
  switch.
- The checkpoint (6977-7372, its safe frame 1701-1725) and the pager's phases (`pager.rs` 5906-6340): the closing
  segment is copied first, and is reset only after the file's sync. Today a Restart's restart comes before that
  sync, with the writer's guard held through it (7306-7317).
- Recovery (`sqlite3_ondisk.rs` 1576-2188) reads two files and orders them by checkpoint sequence; `database.rs`
  (1659-1663, 2366-2412) opens the second path beside `-wal`.
- `crates/amphora/src/db.rs` `restart_log` and `hold_log` (505-566), the store's gate (`store.rs` 1129-1148), and the
  backup, which reads frames out of `-wal` by offset for each generation (`backup/ship.rs` 754-872): it reads the
  segment the generation is in.

**Sources.** SQLite's wal2 branch, `doc/wal2.md` (github.com/sqlite/sqlite, branch `wal2`): the files are
"<database>-wal" and "<database>-wal2"; "Once the first wal file has grown large enough, writers switch to appending
data to the second wal file"; "At this point the first wal file can be checkpointed (after which it can be
overwritten)"; "once the second wal file has grown large enough and the first wal file has been checkpointed,
writers switch back to the first wal file". Checkpoint starvation under overlapping readers, which the writer's wait
prevents today: sqlite.org/wal.html, "Avoiding Excessively Large WAL Files". Decisions 4, 13, 15 and 31 here.

**Tests (behaviour, none on elapsed time).**
- `d45_a_write_commits_while_a_read_holds_the_closing_segment`: a read holds its snapshot across a switch (its closure
  blocked on a channel). A write handed over then is acknowledged durable while the read still holds, and the read
  then answers its snapshot's rows, without the write's.
- `d45_the_closing_segment_is_reset_when_its_last_read_leaves`: copied whole and held by one read, the closing segment
  is not reset. The read gives back, and it is reset with no other event between.
- `d45_a_crash_at_each_step_of_a_switch_loses_no_commit`: `tests/crash.rs`'s power cut at each I/O of a switch, a
  reset and a clean close (steps 1-4, and each step of the close). After reopening, every acknowledged batch is
  present, nothing refused is found, and nothing is torn.
- `d45_sqlite3_reads_a_cleanly_closed_database_and_refuses_an_open_one`: SQLite itself, not Amphora's reader, opens
  a copy of an open database's files with commits in both segments (the files a crash leaves) and is refused; after a
  clean close it opens the file and reads every committed row. The engine's workspace has no SQLite today: the bench
  workspace's `rusqlite` (bundled) is one source of it.
- `d45_the_log_stays_bounded_while_reads_overlap`: reads handed one to the next, so one is always under way, each
  held across at most k of the writer's batches (counted, not timed), through many switches. No segment ever holds
  more than its bound plus k batches' frames plus one copy, and the writer never finds the other segment not `Free`
  at a switch it asks for once that segment's last read has left and its copy is done.

**The A/B (main against the two segments, one binary each).** Writers at 32 and 128 clients with one slow reader
present: a streamed read to a client that reads nothing (P4's test in `serve.rs`), held for its deadline and started
again each time it ends. Commit p50, p99 and p99.9; read p50, p99 and p99.9; the log's largest size on disk; switches
or restarts counted. 5 alternated rounds, cpus=16, load and pressure recorded beside each run.

**Falsified by.** A write that waits for a read (the first test failing, or the A/B's commit p99.9 still at the
deadline); SQLite reading an open or crashed Amphora database, or refusing a cleanly closed one; a closing segment
never reset while reads come and go (the log growing without bound in the A/B); an
acknowledged commit lost, or a database refused, after a crash in a switch.
