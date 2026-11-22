# The storage engine: many writers on one database (CP-T1.3)

Amphora's engine (`crates/engine`) is Turso's as of 3e09a82: SQLite's design, one writer at a time,
each commit a set of whole pages appended to a write-ahead log, each connection with a page cache of
its own. Its SQL layer (parser, planner, bytecode executor) stays. This document records the storage
under it that lets many writers change one database at once, the choices it is made of, their
sources, the evidence and what would falsify each (CLAUDE.md §5). Until it beats the engine it
replaces on the comprehensive suite (`bench --suite`), it is built beside that engine and the
current engine stays the default.

## The evidence that asks for it

Measured on fedora (32 cores, Samsung 9100 PRO), 2026-10-21 and 2026-10-22, 200,000 rows of the
suite's `items` with four secondary indexes; the 10-22 runs are checks at load 17–50 from other
work, not recorded runs.

- **One writer is the ceiling.** `insert_batch_100` (a hundred rows a statement) at 32 clients:
  Amphora's store writes 456–463 statements a second with its writer's thread busy throughout
  (store.md, decision 23), where OrioleDB writes 2,814 and PostgreSQL 1,854, from as many backends
  as clients. A profile of the writer: 66% inserting into indexes, 16% committing, 10% building log
  frames; nothing left that a thread could give back sixfold.
- **Whole pages logged.** A commit logs every page it touched, 4 KiB each: a hundred random index
  inserts log hundreds of pages. PostgreSQL logs a row's change (and a page's image only on its
  first change after a checkpoint), OrioleDB and InnoDB the row's change.
- **The MVCC inherited from Turso doesn't serve as the way out.** Turso's MVCC
  (`crates/engine/mvcc`, after Larson et al., below) keeps every row version a commit writes in
  in-memory skiplists beside the B-tree, merges the two on every read and checkpoints the skiplists
  into the B-tree. The whole suite on it, every write in `BEGIN CONCURRENT` from a connection a
  client (the bench's `mvcc` target, 2026-10-22), against Amphora's store at the same commit: reads
  0.64 times the store's throughput at the median, point reads at 32 clients 0.19–0.25 times,
  `table_count` 30,117 → 33 a second; writes 0.87 times at the median (single-row writes at 32
  clients 0.26–0.65 times); `insert_batch_100` 1.6–2.3 times; peak resident memory 4.5 GB against
  1.3 GB. With its checkpoints on the commit path a hundred-row batch at one client waited up to
  311 ms (p99); moved to a thread of their own, 20 ms. Its costs are in its design, not its tuning:
  a second copy of every recent row that each read probes, and checkpoints that walk it.

## 1. In-place versions in one shared buffer pool

**Decision.** One copy of each page, in one buffer pool every connection of the database shares;
writers change rows in place, in the pages, under page latches, concurrently; the version a row
had before an uncommitted or recent change is kept in the changing transaction's undo record, and
readers of older snapshots reach it from there. Reads go to the B-tree alone. This is the design of
InnoDB, Oracle, HyPer, Umbra, LeanStore and OrioleDB, against the design of the engines that keep
versions apart from the base data (Hekaton, Turso's MVCC) or append them beside it (PostgreSQL's
heap).

**Why.** It is the design whose reads cost what a single-writer engine's do (one structure, the
newest version where the B-tree has it), whose memory is one copy of the data plus the undo of
changes some snapshot can still see, and whose writers change different rows of the same table at
once. Turso's MVCC measured the other choice: a second structure costs every read (above).

**Sources.** Wu, Arulraj, Lin, Xian, Pavlo, "An Empirical Evaluation of In-Memory Multi-Version
Concurrency Control" (PVLDB 10(7), 2017): delta (undo) version storage, newest version in place and
deltas newest to oldest, beats append-only and time-travel storage for updates and keeps scans on
the base data. Neumann, Mühlbauer, Kemper, "Fast Serializable Multi-Version Concurrency Control for
Main-Memory Database Systems" (SIGMOD 2015): in-place updates with undo buffers. Leis, Haubenschild,
Kemper, Neumann, "LeanStore: In-Memory Data Management beyond Main Memory" (ICDE 2018), and
Alhomssi, Leis, "Scalable and Robust Snapshot Isolation for High-Performance Storage Engines"
(PVLDB 16(6), 2023): the same for a buffer-managed engine whose data outgrows memory. OrioleDB's
architecture notes (github.com/orioledb/orioledb, `doc/arch.md`): row-level undo, no buffer mapping
for hot pages, row-level WAL. Larson, Blanas, Diaconu, Freedman, Patel, Zwilling, "High-Performance
Concurrency Control Mechanisms for Main-Memory Databases" (PVLDB 5(4), 2011): the design Turso's
MVCC follows.

**Falsified by.** At 32 clients on the suite, writes no faster than the single-writer store, or
reads slower than it, on the same hardware and data.

## 2. The buffer pool: fixed frames, a latch each, CLOCK, a bound

**Decision.** The pool holds a fixed number of frames, sized by its budget (a typed refusal when a
transaction would need more pinned frames than there are). Each frame holds one page, a latch
(shared or exclusive), a version counter bumped by every exclusive release, a pin count and a
dirty mark; a page table maps page numbers to frames, sharded by page number. Eviction is CLOCK
over unpinned clean frames; dirty frames are written by the checkpointer (decision 7) and become
clean only then. One pool serves every connection of the database: the per-connection page caches
and decision 20's shared pages give way to it.

**Why.** Today each connection caches its own copy of the pages it reads (decision 5 of store.md:
32 readers at 2,000 pages each could hold 256 MB of mostly the same pages); concurrent writers
can't each keep a private copy of a page they all change. The suite's peak resident memory fell from
1,315 to 423 MB with one shared pool beside the private caches (store.md, decision 20, checked
2026-10-22); one pool instead of them is the step after.

**Sources.** PostgreSQL's shared buffers (`src/backend/storage/buffer/README`): a buffer table, a
pin count and a content lock per buffer, clock-sweep replacement. Corbato's CLOCK as in store.md,
decision 19.

**Falsified by.** Point reads at 32 clients slower than with the per-connection caches, or memory
beyond the pool's budget.

## 3. The B-tree: right links, leaf latches, inner pages read without writing

**Decision.** B-link trees (each page knows its right sibling and its upper bound, so a reader that
arrives at a page just split follows the link). A writer latches a leaf exclusively and, when it
must split, latches the parent only after the leaf's split is published, never holding two latches
on the way down. The root is the exception: its split makes the new root before the root's latch
goes, so no writer reaches the root's new page before the level above it exists (released first, a
writer that moved right into that page could split it and find no level to post to). Inner pages
change only on splits; readers descend through them without taking their latches: each thread keeps
copies of the inner pages it has read, keyed by page and the frame's version counter, and uses its
copy while the counter it reads (an atomic load, a read of a line no reader writes) still matches.
Leaves are latched shared for reads. Rows and index keys stay the executor's SQLite records, so
comparisons and the planner's statistics stay as they are, in cells of a page format of this
storage's own (`storage/versioned/page.rs`: slotted, each page bounded above and linked right).

**Why.** A shared latch on the root at every descent is a write to one line by every reader: at 32
clients it moves between their caches at each read (the decision 22 effect, store.md). Lock-free
reads of inner pages are what LeanStore's optimistic lock coupling gives; its unsynchronized reads of
page memory are not expressible without `unsafe` (CLAUDE.md §12), so readers here read their own
copies, validated by a counter, and the copy is made once per change of the page, which is once per
split.

**Sources.** Lehman, Yao, "Efficient Locking for Concurrent Operations on B-Trees" (ACM TODS 6(4),
1981): the inserter holds the node it split until it has the parent. PostgreSQL's nbtree README
(src/backend/access/nbtree/README): a split page stays locked until its downlink, or for the root
the new root, is in place, and a page split again before its downlink exists leaves no place to
post (notes: docs/research/blink-root-split.md). Graefe, "A Survey of B-Tree Locking Techniques"
(ACM TODS 35(3), 2010). Leis, Scheibner, Kemper, Neumann, "The ART of Practical Synchronization"
(DaMoN 2016): optimistic lock coupling, version validation.

**Falsified by.** A reader that misses a committed row its snapshot sees, or sees one twice, during
splits (model test against a sorted map under a nemesis of concurrent splits); a writer that splits
the root's new page while the root's splitter is held after releasing the root's latch, and finds
no level above it (blink.rs, `a_root_split_is_whole_before_another_writer_reaches_its_new_page`);
point reads at 32 clients that stop scaling with clients while the leaves they read are distinct.

## 4. Versions: undo records, commit timestamps, a low watermark

**Decision.** A transaction takes a start timestamp when it begins and a commit timestamp when it
commits, both from one counter. A change records, in the transaction's undo log, what the row was
(its old cell, or that it didn't exist), and the frame holding the row maps it to the head of its
version chain while any version of it is newer than the oldest active snapshot (in memory only:
frames whose map is empty, nearly all of them, cost a reader one emptiness check). A reader sees,
for each row, the newest version committed at or before its start timestamp, walking the chain from
the page's version when the frame maps the row. Versions no active snapshot can see are dropped:
the low watermark is the oldest active start timestamp, and a frame's map entry goes when the row's
newest version is older than it. Writes take a row by first-writer-wins: changing a row whose newest
version is another transaction's uncommitted change aborts the later writer with a typed
`WriteWriteConflict`. Secondary-index entries are versioned the same way.

**Why.** It is what readers need to see a consistent snapshot while writers change the same pages,
and what bounds memory by what some snapshot can still see.

**Sources.** Neumann et al. (SIGMOD 2015, above): undo buffers, version chains newest to oldest,
start and commit timestamps. Alhomssi, Leis (PVLDB 2023, above): garbage collection by an oldest
active snapshot without long-running scans holding memory. The MySQL reference manual, "InnoDB
Multi-Versioning": undo logs, purge.

**Evidence.** `storage/versioned/version.rs`'s tests: a reader begun before a commit sees the rows
as before it, one begun after sees its changes, a writer its own changes before it commits; a
second writer of a row refused while the first is open and after it commits past the second's
snapshot; a rollback of changes, an insert and a removal leaves every row as it was and no
versions once collected; collection keeps what the oldest open snapshot needs and, once it closes,
drops every version and the cells of removed rows; 16 threads moving amounts between 50 accounts
(retrying when refused) while 4 threads sum every account in a snapshot: every sum the total.

**Falsified by.** An Elle-style history (AC-T1.3) with an anomaly snapshot isolation forbids; undo
memory that grows without bound under a steady workload with no long-running reader.

## 5. Serializable by precision locking

**Decision.** Snapshot isolation is the floor; a serializable transaction (the default) also logs
the predicates it read (key ranges per B-tree, and point keys), and at commit checks them against
the writes of the transactions that committed since it began. A match aborts it with a typed
`SerializationFailure`, which the caller retries.

**Why.** The design's README asks for serializable within a group. Snapshot isolation alone allows
write skew; the store's callers (kiln's records) are read-modify-write transactions where that
matters.

**Sources.** Neumann et al. (SIGMOD 2015, above): precision locking for MVCC, validation against
recently committed writes, cheap for short transactions. Jordan, Banerjee, Ma, "Precision Locks"
(SIGMOD 1981). For contrast, Cahill, Röhm, Fekete, "Serializable Isolation for Snapshot Databases"
(SIGMOD 2008) and Ports, Grittner, "Serializable Snapshot Isolation in PostgreSQL" (PVLDB 5(12),
2012): rw-antidependency tracking, more state per read.

**Falsified by.** A serializable history with a cycle (Elle, AC-T1.3); validation costing more than
a fifth of a short transaction's time on the suite's `txn_read_write`.

**Prototypes (G8 step 1, AC-G8.1.1, AC-G8.1.2).** Three validations sit behind one switch on the
commit path (`storage/versioned/version.rs`, `Validation`, chosen per database by
`Versioned::with_validation`; precision locking the default as this decision says, until the
measurements below choose). Each validates inside the publish lock, so no commit lands between a
transaction's check and its timestamp, and each refuses with `SerializationFailure`, the
transaction rolled back. Reads that count are the ones made through `VTree::read` and
`VTree::read_range` (a row by key; a span from a key to the last row given, or to the end when
fewer came back than asked); a write's own read of its row needs no keeping, since the first
writer's win already refuses a row changed since the snapshot.

- *Precision locking* (HyPer): a transaction that wrote checks its rows and spans against the rows
  written by every commit since its snapshot; one that only read commits unchecked, serialized at
  its snapshot (a prefix of the commit order). Kept between commits: each writing commit's rows,
  while a snapshot older than it is open.
- *SSI* (PostgreSQL, after Cahill et al. and Ports and Grittner): read-write antidependencies
  between concurrent transactions, each pair found at the later commit of the two, so only
  committed transactions are checked; refused where the committing transaction would be the pivot
  of `in -> pivot -> out` with `out` committed no later than `in`, or the `in` of a pivot that
  committed with an `out` before it (Fekete et al.'s theorem: every cycle holds such a structure
  whose `out` commits first). Readers are validated too (the read-only anomaly). Kept: every
  commit's rows read and written, and its earliest `out`, while a snapshot older than it is open.
- *OCC* (Silo): each row read is checked in its tree, by the version its head names, for a commit
  since the snapshot; each span is read again for one. Silo's node set (a leaf's version) is
  replaced by the rows of the span, since an open writer's in-place change moves a leaf's version
  and would refuse the transactions reading beside it, its own included; Silo's refusal of a row
  locked by an open writer is left out, since that writer commits after the publish lock and so
  after the reader in the serial order. TicToc's lazily chosen commit timestamps don't apply: a
  snapshot here is a prefix of commit timestamps, and a commit placed in the past would change
  what open snapshots have already read. Kept: nothing.

Precision locking and OCC refuse the same transactions (a row or span read that a commit since
the snapshot wrote) by different work: a scan of the recent commits' rows against a transaction's
reads, against a look-up of each read row and a second read of each span. SSI refuses fewer (a lone
antidependency commits) for more kept state. The tests: `version.rs`'s `g8_1_*` (write skew, a
phantom closing a cycle, a lone antidependency, the read-only anomaly, an open writer of a row
read, and the kept commits bounded by the oldest snapshot); `storage/versioned/elle.rs`, Elle's
list-append and register histories from its own generator, 32 transactions open at once, checked
for G0, G1a, G1b, G1c, G-single and G2, with snapshot isolation as the control that shows G2. The
throughput, refusals and p99 of each on the suite's write classes and K0 come from
`amphora-bench --cc`, not yet recorded.

A race this work found and fixed: a transaction or snapshot read the last published timestamp
before counting itself open, so a commit and a collection between the two could drop versions it
needed (and, with validation, the commits it must be checked against). It now reads the timestamp
under the open snapshots' lock, which the low watermark takes too.

## 6. Durability: a row's change logged once, at commit

**Decision.** A commit appends one redo record: each row change of the transaction (the B-tree, the
key, the new row or its removal), in the order made. Records are appended in commit-timestamp order
by a leader that gathers the commits waiting for it (group commit) and flushes once for them all;
a commit is durable when a flush that began after its record was appended ends, as the store's
flusher works today (store.md, decisions 2 and 8). Uncommitted changes are never logged and never
written to the data file (decision 7), so recovery redoes and never undoes.

**Why.** Logging rows instead of pages cuts the log by what a page is to a row (4 KiB against the
~600 bytes of an `items` row, and far more for index entries), and logging at commit means aborted
work costs no I/O.

**Sources.** Johnson, Pandis, Stoica, Athanassoulis, Ailamaki, "Aether: A Scalable Approach to
Logging" (PVLDB 3(1), 2010): consolidated log buffer inserts, flush pipelining. Haubenschild, Sauer,
Neumann, Leis, "Rethinking Logging, Checkpoints, and Recovery for High-Performance Storage Engines"
(SIGMOD 2020): per-thread logs with group commit at many cores. Tu, Zheng, Kohler, Liskov, Madden,
"Speedy Transactions in Multicore In-Memory Databases" (Silo, SOSP 2013): redo-only value logging
at commit.

**Built** (`storage/versioned/redo.rs`): one record a commit (its changes, its timestamp, each
record's length and XXH3), appended under the clock's publish lock to the pending bytes of the
writer's lane (decision 11; one lane is the one log this decision describes); whoever next needs a
commit durable takes a lane's flush lock and writes and flushes everything pending in it at once, so
one flush serves every commit appended to the lane before it began, with no timer. A flush that
fails stops the log: nothing more is logged or made durable until the database is reopened
(store.md, decision 9). Recovery reads each file up to its first torn or damaged record and cuts the
file after the last record it replays, so an append past it can't leave a torn record's leftovers to
be read back behind it.

**Falsified by.** An acknowledged commit lost after a crash at any write (crash injection,
T-T1.3.1); log bytes per row on the suite not less than a quarter of page-a-frame's.

## 7. Checkpoints: dirty pages copied in a pause, and the rows to correct

**Decision.** A checkpoint holds a gate exclusive for a pause (each row change, commit, rollback
and collection holds it shared for as long as it changes pages or publishes): it takes the last
commit's timestamp T and switches the redo log's turn at it, ends the buffer pool's epoch of writes,
and lists the rows whose version in the pages isn't their version as of T (changes of transactions
still open), each with that version. Taken under the gate, T splits no commit, and no commit after
it exists until the gate is released. After the pause, while writers go on, it copies every page
written in the epoch it ended as the page was at the pause, by copy-on-update: a page no one has
written since is copied under its shared latch; a writer reaching such a page first keeps its bytes
before its change (at most one kept page a frame), and the checkpoint takes those. The pause is the
list's time, bounded by the rows with versions, not by the pages written. The copies, the list and T go into
a checkpoint journal, flushed; then the copies and a header naming the checkpoint are written over
the data file, flushed; then the redo file switched from, which holds only commits up to T, is cut.
Journals take turns by the checkpoint's sequence number, so a checkpoint never writes over the
journal recovery may need, and making a database is checkpoint 0 (its empty trees journaled first,
like any other). A checkpoint that fails leaves the database making nothing more durable until it
is reopened: the pages it took as copied may not have reached the file. Splits wait for the pause
like any change, so the copies are one consistent tree.

**Why.** Writing each page as of T (undoing newer changes in the copy) can need more than a page:
a row whose older version is larger than its newer one no longer fits the page it was split for.
Listed instead, the rows are put back to their versions as of T at recovery by ordinary tree
changes, which split as they must. The pause, not the I/O, is what writers wait for.

**Sources.** InnoDB's doublewrite buffer (MySQL reference manual, "Doublewrite Buffer"): pages
written whole to one place, flushed, then over their homes, so a torn write is repaired from the
first copy. Cao, Vaz Salles, Sowell, Yue, Gehrke, Demers, White, "Fast Checkpoint Recovery
Algorithms for Frequently Consistent Applications" (SIGMOD 2011): copy-on-update, a consistent
image taken while updates go on, each update copying what it changes first. SQLite's write-ahead log backfill (sqlite.org/wal.html) for the same order. Mohan et
al., ARIES (ACM TODS 17(1), 1992), for what the list replaces: undo of uncommitted changes from
records logged before each page write.

**Falsified by.** A file after recovery that holds a change of a transaction that never committed,
or misses one that did; a commit's p99.9 while checkpoints run more than 1 ms above its p99.9
without them (AC-T1.4.3, `amphora-bench --vlog`).

## 8. Recovery: the file's pages, the last journal, its list, then the redo after T

**Decision.** Opening a database takes the newest complete journal at or after the checkpoint its
header names (a damaged header, any complete journal) and writes its pages and header over the
data file again: a power cut may keep a checkpoint's header and not every page it wrote. It then
applies the journal's list (each row back to its version as of T), replays every redo record
committed after T, in timestamp order across every lane's files up to the first timestamp none holds
(decision 11), and clears the versions' flags (no snapshot outlives the process), removing the cells
of removed rows. The lanes' files are read side by side and the list and records replayed side by
side, each row's changes in their order on one thread of as many as the machine runs (SiloR: Zheng,
Tu, Liskov, Madden, "Fast Databases with Fast Durability and Recovery Through Multicore
Parallelism", OSDI 2014), which leaves each row as replaying in order would. A data file with no complete
journal is made anew only when it is empty and no commit is logged; otherwise it is refused as
corrupt, never replaced by an empty database.

**Falsified by.** Recovery that ends in a state no prefix of acknowledged commits explains
(T-T1.3.1).

## 9. Beside the engine it replaces, behind the cursor the SQL layer already uses

**Decision.** The new storage implements the cursor interface the executor drives
(`storage::btree::CursorTrait`, which Turso's MVCC cursor implements too) and the connection's
begin, commit and rollback, chosen per database when it is created; a database the current engine
created stays on it. The store runs as many writers as it has callers with the new storage, each
transaction on its caller's thread, and keeps its tickets, flusher semantics and refusals.

**Why.** The SQL layer, the planner's work and their tests stay, and the suite can run both
engines side by side until the new one is the default.

**Evidence.** Stage 1 (`storage/versioned`): `blink.rs`'s tests: one writer's puts, replacements
and removals of 20,000 random keys against a sorted map; 32 writers with 4 readers at once, five
times, no key lost or read with another's value; with the bound on a walk to the right fixed when
the walk began (a mutant: splits add pages under it), both fail. `bench --blink`, fedora,
2026-10-22, load 10–15 from other work (a check): one tree in memory, 2,000,000 random 8-byte keys
with 64-byte values, puts and gets a second at 1, 2, 4, 8, 16 and 32 threads: 660,371 and 847,238;
1.24 and 1.73 million; 2.33 and 3.29 million; 3.63 and 4.89 million; 6.38 and 8.42 million; 8.55
and 11.64 million.

Stage 2 (`storage/versioned/{disk,redo,durable}.rs`): the files behind a seam (`Disk`), as a
directory's files and as a simulator whose writes are durable only once flushed and which crashes
after any number of operations, keeping of each unflushed write all, none or a prefix of its sectors.
`durable.rs`'s tests: commits survive a reopen and a power cut; a checkpoint taken while one
transaction is open and another rolls back keeps only what commits; a checkpoint whose header
reached the file and whose page didn't is finished from its journal; a damaged header is recovered
from its journal, and with the journals gone the file is refused, not made anew; a checkpoint that
fails refuses the commits after it; a torn redo tail is cut before appends reach it; eight writers
committing while one checkpoints every 20 commits, and 57 crash points through a workload of
commits and checkpoints, recover a prefix of the commits at least as long as the last acknowledged
durable. `redo.rs`'s: records decode up to the first torn or damaged one; one flush makes every
commit appended before it durable. Five mutants each fail the test written for them: recovery
without rewriting the newest journal, making a database whenever the header is unreadable,
checkpoints that don't stop the log when they fail, redo tails left uncut, and versions left on
recovered cells (which fails five).

**The cursor** (stage 3's first step, NS.1, `storage/versioned/cursor.rs`). `VersionedCursor`
implements `CursorTrait` on one tree of a `durable::Database`, reading as a snapshot sees it and
writing in a transaction (`Access`), so the executor reaches it through the `Cursor::Dyn` slot the
MVCC cursor uses. Keys: a table's are its rowids, 8 bytes big-endian with the sign bit flipped (a
format value: the bytes order as the integers); an index's (and a WITHOUT ROWID table's) are its
records with an empty value, ordered by the index's columns and then by the record's bytes, so a
cell's key is exactly the record put. A record equal on the columns but not in its bytes (another
spelling under NOCASE, a WITHOUT ROWID row's other columns) replaces the one the executor's seek
placed the cursor on by removal and insertion, so each snapshot sees the record it saw: a B-link
cell's key never changes in place. The cursor holds no latch between calls. It copies the rows of
one leaf its snapshot sees, under the leaf's shared latch, with the frame's version, and while the
version stays the copy is the leaf; once it moves (any write to the leaf) the cursor finds its row
again by its key. That is how it sees its transaction's writes through another cursor, as SQLite's
cursors do after `saveAllCursors`. Moving on from a copy's last row follows the right link; moving
back from its first descends to the leaf just short of its lower bound, as the leaves link only to
the right. A seek is a point between keys: a table's just short of or just past its rowid's key, an
index's placed by the executor's record comparison with SQLite's tie-breakers
(`get_tie_breaker_from_seek_op`). An exact seek is placed as its inexact kind is, then asked whether
the record found equals the key. A delete leaves the cursor in the removed row's gap: the next row is
the first after it, the previous the last before it.

*Evidence.* `storage/versioned/conformance.rs`: random sequences of 2,000 operations each, in the
executor's grammar (an insert after the seek for its key, a delete of the row the cursor is on, steps
from a row or a write, every seek kind with `TryAdvance` followed by its step), on the current
engine's B-tree cursor and the versioned one through `CursorTrait`, compared after every step (the
seek's answer, whether the cursor is on a row, its rowid and record) and in full at the end. It runs
on a table, an ascending binary index of (a, b, rowid), and an index whose first column descends and
whose second compares under NOCASE, with records that differ only in case put over each other. Beside
it: a reading cursor walks and seeks its snapshot exactly while 4 writers commit rows into its leaves
and split them; a cursor sees writes made through another cursor of its transaction; a reading
cursor refuses writes (`ReadOnly`).

*Falsified by.* A sequence of the executor's operations after which the two cursors differ; a
snapshot's walk that misses or repeats a row while other transactions split its leaves.

*What step 2 needs from it.* A comparison per tree (`Database::open` takes one for all trees; an
index's is `KeyOrder::index`), trees made and dropped by the catalog (`btree_destroy` is refused
until then), overflow for rows past a cell's bound (refused now, `TooBig`), the connection's
transaction behind `Access::Write` (begin, commit, rollback, savepoints from decision 4's versions),
and the connection's pager for `get_pager`.

## 10. Memory: versions pruned between snapshots, one budget, typed refusals

**Decision.** Collection keeps, of a row's older versions, only those an open snapshot sees: for
each open snapshot the newest version at or before it unless it sees the row's newest, every
version after the last commit read with the snapshots and the newest at or before it (what a
snapshot opened meanwhile may see), and the version an open writer replaced (its rollback's). A
long reader so holds one version a row, not one an update. Every byte kept beside the pages (row
versions and their heads, each transaction's write and read records, the commits kept for
validation) is charged to one budget, reserved before anything changes and refused past it with a
typed `MemoryBudget`, the change refused whole. Commits are kept for validation only while a
transaction begun before them is open: a read-only snapshot never validates. A transaction's reads
that the budget can't take become its whole tree, one span. The budget is the shell's
(`Versioned::with_budget`, the process's memory as `crates/amphora/src/memory.rs` learns it where
it runs); until the shell opens this storage, the pool's own bytes.

**Why.** The low watermark alone bounds versions by the oldest snapshot: one reader held an hour
keeps every version written since, which no budget survives. Pruning between snapshots bounds them
by rows times open snapshots; the budget turns what is left into a refusal a caller can handle
instead of the process running out of memory. Escalation keeps a long serializable reader running:
a coarser record refuses more, never fewer.

**Sources.** Böttcher, Leis, Neumann, Kemper, "Scalable Garbage Collection for In-Memory MVCC
Systems", PVLDB 12(6), 2019 (Steam: eager pruning of versions no active snapshot sees). Lee et al.,
"Hybrid Garbage Collection for Multi-Version Concurrency Control in SAP HANA", SIGMOD 2016
(interval collection). Alhomssi, Leis, PVLDB 2023 (LeanStore: version records in the buffer
manager, bounded by it). Ports, Grittner, PVLDB 5(12), 2012 (PostgreSQL SSI: predicate locks
promoted to coarser grains when memory runs short). The MySQL reference manual, "InnoDB
Multi-Versioning" (undo in buffer-pool pages).

**Evidence.** `version.rs`'s T-G8.2.1 soak (one reader, 32 writers, a collector; precision locking
and SSI), T-G8.2.2 and T-G8.2.3.

**Falsified by.** The hour-long soak (`AMPHORA_G8_SOAK_SECONDS=3600`) finding the budget held past
its limit, an untyped failure, a scan that isn't its snapshot's, or bytes not given back; or a
workload whose refusals under the budget a version store out of memory (LeanStore's graveyard)
would have avoided at a cost the suite accepts (the step this leaves open).

## 11. The log in lanes: one group-committed log, or a log a writer

**Decision.** The redo log is in lanes, each its own pair of files; a writer takes a lane
(`Database::lane`) and appends its commits' records to it. One lane is one group-committed log (decision
6); a lane a writer is per-thread logging. Commit timestamps run with no gap, every commit logging
one record under the publish lock, so the lanes merge by timestamp alone: a commit is acknowledged
once every lane is durable through it (a lane holding nothing at or before it already is, with no
I/O; one holding some is flushed by whichever waiting thread takes its lock first), and recovery
replays from the checkpoint to the first timestamp no lane holds, then cuts every lane after the
last record it replayed, so a record past a gap (never acknowledged) can't be read back beside the
commit that takes its timestamp next. No timer and no group size: each flush carries what arrived
in its lane while the one before it ran, so groups follow the device's flush time as it is, flush
by flush (`Database::log_counters`: flushes and the records they carried). On Linux, with the
engine's `io_uring` feature, a flush is its write and a data flush linked in one submission, a ring
a file (`storage/versioned/uring.rs`); elsewhere a positional write and `sync_data`.

**Why.** One log's flush lock makes every commit wait for the flush before its own: up to two
flushes. Lanes flushing side by side put as many flushes in flight as there are writers, which NVMe
serves at once; whether the file system lets them overlap (btrfs commits its log one flush at a time,
store.md decision 4) is what the measurement decides, lane count 1 against one a core, on the same
binary.

**Sources.** Haubenschild, Sauer, Neumann, Leis, "Rethinking Logging, Checkpoints, and Recovery for
High-Performance Storage Engines" (SIGMOD 2020): a log per worker thread, merged at recovery; their
remote-flush avoidance acknowledges a commit before other logs are durable when it depends on none of
them, which recovers a set of commits closed under dependency, not a prefix: this storage keeps the
prefix (decision 8's falsifier), and pays for it only where another lane holds an earlier commit not
yet durable. Haas, Leis, "What Modern NVMe Storage Can Do, And How To Exploit It" (PVLDB 16(9),
2023): NVMe needs many requests in flight, and io_uring puts them there. Johnson et al., Aether
(above): flush pipelining.

**Evidence.** `redo.rs`'s tests: a commit on one lane flushes it and every lane holding an earlier
commit, and no idle lane; 8 threads on 4 lanes merge back in order; the merge stops at the first
missing commit and cuts each lane there; records out of order or twice are refused as damage.
`durable.rs`'s: commits racing checkpoints (pages copied as writers change them) recover a prefix on
one lane and on eight; a crash at many points on one lane and on three. T-T1.3.1
(`tests/versioned_crash.rs`): each seed's workload (one to four lanes, open transactions across
checkpoints) crashed at every disk operation it makes, the power cut keeping each unflushed write
whole, torn or lost; 200 seeds in every gate, 10,000 on their own: acknowledged commits lost 0.
Not yet measured: `amphora-bench --vlog` (AC-T1.4.2, AC-T1.4.3).

**Falsified by.** At 32 writers on NVMe, per-thread lanes' commit p99 not at most half one log's
(AC-T1.4.2), in which case one lane stays the default and the lanes go; an acknowledged commit lost
in T-T1.3.1.

## 12. Large rows as chunk rows of an overflow tree; trees made and dropped as records of their own

(G7 step 2, `AC-NS.2.4`, `AC-NS.2.5`; `crates/engine/storage/versioned/{overflow,durable}.rs`.)

**Decision.** A value with at least as many bytes as its cell has room for beside its key keeps
exactly that many in its cell; the rest goes, in chunks of the most a cell holds beside the chunk's
key, into the database's overflow tree under (tree, key, chunk number), written by the same
transaction. A cell's value of exactly the room is how a reader knows to look for chunks. The room
and the chunk size come from the page format (`page.rs`: `MAX_CELL`, a quarter page with its slot,
less the two `u16` lengths); the longest value is SQLite's `SQLITE_MAX_LENGTH`
(sqlite.org/limits.html), which bounds every walk of a row's chunks. A page splits by bytes, the new
cell counted, at the first cell past half the bytes: each half then holds at most half plus one cell
and one bound, which a quarter-page cell keeps within a page. Making or dropping a tree is a redo
record of its own, durable before it returns, taken with row changes and checkpoints held off; a
checkpoint lists a dropped tree's root as 0.

**Why.** Chunks as ordinary versioned rows get snapshots, first-writer-wins, rollback, savepoints,
the redo log, checkpoints' lists and recovery from the code that already does them for rows
(decisions 4, 6 to 8), where SQLite's overflow page chains (sqlite.org/fileformat2.html, 1.6) would
need versions, logging and recovery of their own. The other way, a header byte on every value naming
whether it continues, costs a byte and a branch on every row to serve the few that overflow; it is
the reversible alternative and Ada's to choose.

**Evidence.** `durable.rs`'s `ns2_4_*` tests (every snapshot, savepoints, recovery, a crash at each
of many writes) and `ns2_5_*` (reopens, power cuts, a crash at each write of trees made and
dropped); `page.rs`'s `ns2_4_splits_by_bytes_fit_cells_of_the_largest_size`.

**Falsified by.** The corpus's large-row classes measuring the chunk reads (one descent a chunk)
slower than the current engine's overflow chains by more than the plan's bar, or a split that finds
no room for its cell.

## 13. The switch: a connection's B-trees on the versioned storage, the pager beside it

(G7 step 2, `AC-NS.2.1`; `crates/engine/storage/versioned/switch.rs`, `crates/engine/vdbe/execute.rs`.)

**Decision.** A connection opened with `ConnectOpts::with_versioned_storage`, or any connection
while `AMPHORA_STORAGE=versioned` is set, puts the database's B-trees on the versioned storage: the
first such connection makes it (in memory for a memory database or one on the engine's memory IO,
else in `<path>-versioned/`) and every connection after it uses it. The executor's `OpenRead` and `OpenWrite` open a
`VersionedCursor` on the tree whose id is the root page, `CreateBtree` makes a tree, `Destroy`
drops it once the transaction commits (a tree it made is dropped if it rolls back or its commit
fails, or as the transaction ends if a failed statement or a savepoint rolled back to made it; a
DROP so undone is forgotten), and
`ClearBtree` removes its rows; the SQL transaction is one storage transaction, begun by
its first cursor and committed (durable before it returns) or rolled back where the pager's
transaction ends, and each write statement in an explicit transaction holds a savepoint of it
(SQLite's statement journal), rolled back to if the statement fails. An index's tree (or a WITHOUT
ROWID table's) is ordered by the key its first cursor brings (`IndexInfo` from the schema): the tree
is empty until then, so nothing is compared before; the order is kept for the tree and every later
cursor, and recorded in the redo log (and each checkpoint's list) before a row of the tree is
written, so a reopened database orders the tree before it replays a row of it; at open the schema
is read from tree 1. The planner prices a page read as resident: the pool keeps every page in a
frame, and the pager's miss ratio, from fetches it no longer makes, would price each as read from
the file. Tree 0 is never named (SQLite's pages
start at 1), tree 1 is `sqlite_schema`, tree 2 the overflow tree, and each tree made later takes the
next id, which `sqlite_schema` records as its root page. The pager still runs beside it for the
database header (schema cookie, user version). The storage sets no memory aside: it takes its
pages' frames and its row versions' bytes from a memory source as they grow, and gives them back as
it lets them go (`storage::versioned::memory::Source`, `ConnectOpts::with_memory`). Amphora's shell
gives its process memory budget (`amphora::memory::StorageMemory`, as caches; store.md, "Decision
(lane constants): one process memory budget", MB.1), which every database of the process shares and
which grants a use what the rest of the process leaves it, by measured use; without one, the
storages of the process share the machine's memory (`memory::machine`). No fraction is picked, and
none is split between pages and versions. The pool makes its frames as pages need them, in runs
that double, each run's bytes taken from the source; a run refused, the pool evicts among the
frames it has (decision 14). The versions take from the source as a dynamic table grows: doubled
when full, halved down to twice what is held once that falls under a quarter (Cormen et al.,
*Introduction to Algorithms*, section 17.4), so the source is asked once per doubling, not per row.
The redo log takes a lane per thread the machine runs (decision 11). Off by default: the pager's B-trees stay the engine's
storage until the suites pass on this one (the design's stage 5).

**Why.** The executor already drives every B-tree through `CursorTrait` (decision 9), so swapping
the cursor at the three opcodes that make one is the whole of the executor's change: every other
opcode, the planner and the schema code run unchanged on both storages, which is what lets the same
suites judge them. Tree ids as root pages keep `sqlite_schema`'s `rootpage` column meaning what it
means to every reader of the schema.

**Evidence.** `sql.rs`'s `t_ns_2_6_*` tests: the same CREATE TABLE, INSERT (a row past a page among
them), SELECT, explicit transactions, rollback, UPDATE and DELETE through both storages answer alike,
and the versioned run's table sits on tree 3.

**Falsified by.** An engine suite that passes on the pager and fails on the switch for a reason the
cursor interface can't carry; or the header kept by the pager disagreeing with the trees after a
crash between the two (the header moves into the catalog with `sqlite_schema`'s own tree, G7 step
2's next piece).

## 14. Eviction: CLOCK to a spill file, slots by page number, reads verified once, a typed bound

(G7 step 4, `NS.4`; `crates/engine/storage/versioned/pool.rs`, tests in `evict.rs`.)

**Decision.** A database's pool holds the frames its room grants (`pool::Room`): a fixed count, or
as many as its memory source grants, a run at a time (decision 13). Pages are read in as they are asked for and
evicted by CLOCK (decision 2's rule, the reach prototypes' shared eviction, G7 step 3): a frame whose
reference bit is clear and whose latch no one holds; a clean frame is reused at once, a dirty one is
first written to the spill file, page `n` at `n` pages in. The data file still changes only by
checkpoints (decision 7), so the spill file is emptied as the pool opens and never read by recovery:
eviction needs no ordering against the redo log, and recovery is decision 8's unchanged. A page's
newest bytes out of the pool are the spill file's once it was spilled, else the data file's; a page
read in is checked once, as it enters the pool, against the low 32 bits of XXH3 of those bytes,
taken when the pool wrote them out or, for the data file, as it opened. Page numbers reach frames
through a direct array, one 16-byte slot a page (frame, spilled mark, the epochs of decision 7's
copy-on-update, checksum), in buckets that double so it grows without moving: G7 step 3's design
(c). A page's version for decision 3's copies is its frame's, started past the highest version any
evicted frame reached each time a page is read in, so no copy from an earlier stay matches. Every
frame latched, or a page that left its frame on as many tries as there are frames, is refused with
`MemoryBudget`; a pool without a disk keeps every page resident and refuses past its frames with
`DatabaseFull`, as before. A checkpoint writes its journal a page at a time as `Pool::copy_owed`
hands the pages over, its seal last over the body's hash, and recovery checks the journal and
writes its pages home a page at a time: neither holds more than one page of it in memory, however
many pages were written since the last checkpoint (the journal's body: its stamp, its list's length
and list, then its pages to the end).

**Why.** In-place write-back between checkpoints would break decision 8: recovery starts from the
last checkpoint's pages and replays logical row changes, and a page written home early (a split's
half, a row of a transaction still open) leaves a file no replay can repair. Writing evicted pages
aside keeps the checkpoint's image whole, as shadow paging does (Lorie, "Physical Integrity in a
Large Segmented Database", TODS 1977), at the cost of a write per dirty eviction. CLOCK over
LeanStore's cooling stage: CLOCK is what the three reach prototypes share and the model test checked
under 32 threads; the cooling stage (Leis et al., ICDE 2018) pays off with swizzled pointers, which
the coordinator's choice among the prototypes hasn't made. The direct array because decision 7's
copy-on-update already needs state for every page, resident or not: the frame index rides in the
same slot (vmcache's translation, Leis et al., SIGMOD 2023, without the virtual memory). 32-bit
checksums: PostgreSQL keeps 16 bits a page (`src/include/storage/checksum_impl.h`).

**Evidence.** `evict.rs`: a database of more than ten times its 24 frames written, rewritten across
a checkpoint, read back and reopened; a crash at every disk operation of a workload that spills and
checkpoints recovers a prefix of its commits at least as long as the last acknowledged durable; the
pool's model test with eviction, 32 threads on 8 frames over 64 pages, every read whole and every
owner reading its last write. `pool.rs`: a spilled page damaged on disk refused as corrupt; every
frame latched refused typed. `durable.rs`: every crash test, through journals written and read a
page at a time.

**Falsified by.** The coordinator's 10M-row corpus run in a pool a tenth of the data with resident
memory more than 10% over the budget, or slower than the reach design chosen by the numbers. What
isn't bounded by the pool yet, and would show there: a page first written
during a checkpoint's copy keeps its old bytes in memory until the copy takes them; the slots are 16
bytes a page of the database (0.4% of it, 3.9% of a pool a tenth of it, up to twice that in a bucket
just made).

## 15. Read-ahead runs: one read for a sequential walk's next pages, sized by the measured cost

(Query speed step 2, `QT.2`; `crates/engine/storage/versioned/pool.rs`, `disk.rs`; behind
`AMPHORA_READ_AHEAD=runs`, or `Pool::set_read_ahead`, off by default.)

**Decision.** A miss on the page after the last page the pool read in is a sequential walk's: the
pool reads that page and the pages after it in one read (`Disk::read_run`: one `preadv` on Unix, the
kernel scattering the run straight into the frames, no staging copy; one operation in the
simulator), as many as the run the measured costs give. The pool fits a least-squares line through
every read it made with read-ahead on, its time against its pages: `fixed`, what an I/O costs
whatever it carries, and `each`, what a page more costs (the sequential bandwidth's inverse), each
read with the same weight, nothing picked. The run is `ceil(fixed / each)` pages: there a page costs
twice what an endless run's would, and past it each page more saves less than it costs to read, a
page the walk may never reach (the break-even between an access's fixed cost and its transfer: Gray
and Graefe, "The Five-Minute Rule Ten Years Later", SIGMOD Record 26(4), 1997). Until two sizes are
measured, or while noise hides the slope, the run is twice the largest read so far, so each run
measures a size not measured yet. The pool's budget bounds it by a stated rule: a page read ahead
takes only a frame never used, or one whose reference bit is clear and which no one latches (one
sweep at most, the bits left as they are), and a run holds at most the pool's frames less one; so a
run never evicts a page CLOCK hasn't found cold, never latches every frame, and holds no memory past
the frames but the list of its buffers. A full budget shrinks a run, down to the page asked for
alone. A page joins a run only while it isn't resident, comes from the same file as the page asked
for (the data file, or the spill file at the same offsets), has bytes there, and its loading stripe
can be had without waiting; the run stops at the first that can't. Each page is checked against its
checksum once as it comes in (decision 14's check): the page asked for failing is refused as a
plain miss's is, a page after it failing or past a short read is left out and its own read meets
it. Pages read ahead come in with their reference bits clear, CLOCK's first to go if the walk
doesn't reach them (the cold end PostgreSQL's ring gives a large scan, `src/backend/storage/buffer/README`).
A miss allocates nothing (the page is read into its frame), checks its checksum once, and ends
inline, with no completion to call back; a run allocates the list of its buffers, once a run.
Read-ahead runs only where the disk reads a run in one I/O (`Disk::reads_runs`): Unix's files and
io_uring's (through the same `preadv`) and the simulator; Windows reads a page at a time until a
scatter read there is built (`ReadFileScatter` needs unbuffered, page-aligned handles).

**Why.** Out of cache Amphora grew 19.4x from TPC-H SF 1 to SF 10 where PostgreSQL grew 11.3x, with
one synchronous 4 KiB read a miss and no read-ahead (the query-speed plan's research, f17, f18, f35).
PostgreSQL 17 combines up to 128 kB of adjacent blocks into one read by default (`io_combine_limit`,
f34); here the run comes from this machine's own costs instead of a stated size, since NVMe's fixed
cost against its transfer differs by device and by whether the page cache answers (Haas and Leis,
"What Modern NVMe Storage Can Do, And How To Exploit It", PVLDB 16(9), 2023). Detecting the walk in
the pool, from the page numbers it reads in, keeps the change to the pool and the I/O path while the
cursor's own walk is reworked beside it (G7 step 2 to 6).

**Evidence.** `pool.rs`: `qs2_answers_are_identical_with_read_ahead_on_and_off` (sequential,
backwards, strided and repeated walks over data-file, spilled and newly handed-out pages: the same
bytes, one read a page off, fewer reads than pages on); `qs2_read_ahead_never_reads_past_the_budget`
(runs asked for 1,000 pages in a pool of 6 frames, one latched: none longer than 5, no frame past 6,
a pool of one frame reads none); `qs2_a_page_read_ahead_that_fails_its_checksum_is_not_served`;
`qs2_runs_over_files_read_each_page_as_written` (`preadv` over a directory's files);
`qs2_a_run_is_the_measured_fixed_cost_over_the_cost_of_a_page`. `tests/versioned_crash.rs` passes
with the switch off and on. The run's length and the time saved are the coordinator's to measure.

**Falsified by.** TPC-H SF 10 on the versioned storage with `AMPHORA_READ_AHEAD=runs` no faster
than without, or JOB more than 5% slower (runs read for walks that don't go on). Known limits: the
walk is detected pool-wide from the last page read in, so two scans interleaving misses read no
runs (a cursor's own hint is the next step, in `cursor.rs`); runs are read synchronously, one in
flight at a time; the B-tree's leaves are sequential in the file only as far as they were written
in order.

## Stages, each measured

1. The buffer pool and the B-link tree, versioned rows, in memory: concurrent inserts and reads
   against a model (a sorted map), from 1 to 32 threads; a micro-bench of concurrent inserts
   against the current engine's single writer.
2. The redo log, group commit, checkpoints with their pause and list, and recovery: crash injection
   at every write; the pool still holds every page (eviction to the file follows). Built (above);
   measured with the rest of the storage through the suite once stage 3 puts it under SQL.
3. The executor on the new storage: the comprehensive suite, both engines.
4. Serializable validation, garbage collection, memory bounds; Elle histories (AC-T1.3).
5. The default, once the suite says so.
