# Columns for analytics, beside the rows (G11)

G11 step 1 (kiln plan `01a16b0f477676d5a7ce168b80aa29f5`; requirement CS.1 in
`docs/requirements.toml`): six designs that keep columns for analytics beside rows for
transactions, compared on Amphora's terms (how fresh the columns are, the memory they take, what
they cost a write, how they fit Amphora's row B-trees and its log), and the decision that follows,
with what would falsify it. Every claim cites a line of `docs/research/columnar.md` as `[cN]`,
which holds the source's words, where they are, and how far the source can be trusted;
`crates/amphora/tests/columnar_decision.rs` checks the citations and every number against them
(T-CS.1.1). The decision is decision 1 below.

On whole-table statements Amphora's rows are two to four orders of magnitude behind the column
stores: `table_count` at 1 client 36.8 a second against CedarDB's 2,197, `filter_no_index` 0.6
against 2,608, `table_count` at 32 clients 140 against ClickHouse's 14,127 [c12][c13][c14], on a
machine whose load ran from 3.5 to 123.6, so the sizes are rough and the orders of magnitude are
not [c15].

## What a design must fit

- **Rows in versioned B-trees.** A reader sees each row's newest version committed at or before
  its start timestamp [c3], and readers never wait behind writers [c4].
- **A record of each commit's row changes.** Every commit writes one redo record: each row change
  with its tree, its key and its whole new row, in commit-timestamp order [c1], appended under the
  lock that orders commits [c2], made durable by a group commit with no timer [c97]. A checkpoint
  cuts the redo it no longer needs [c96].
- **Not yet under SQL.** That storage, CP-T1.3's, doesn't run under SQL yet [c7]. The engine SQL
  runs on today logs whole pages [c5], and the replicated path ships page images, not rows [c6].
- **Kiln's tables keep their keys.** Kiln's Records are tables clustered by composite primary keys,
  whose storage semantics are kept [c8].
- **One code path, safe Rust, no recursion.** A laptop and a fleet run the same code [c9]; no
  function calls itself [c10]; `unsafe` is denied [c11].
- **The plan's bar.** With the columns kept, write throughput at 32 clients falls by no more than
  5% [c16]; a scan at each snapshot under concurrent writes equals the rows at that snapshot [c17];
  each analytic class at least matches the best of CedarDB and ClickHouse [c19]. Columns serve
  whole-table and wide-range statements, rows serve point reads [c95].

## The six designs

**SQLite3/HE.** An in-memory Arrow copy beside SQLite's B-trees [c21], fed by a memlog of each
write's kind, row ID and data [c22], in the order SQLite's single writer gives it [c23], applied
during the transaction, at the next read, or by a background thread [c24], rebuilt from the
database after a crash [c25], and used only when every table a query reads is in memory [c26].
Analytics 100x-1000x faster at no cost to transactions, by its abstract [c20]; the evidence for
the second half is a read-and-write mix that ran up to 20% faster [c27], not a write's own cost,
and the copy's memory was not measured.

**Oracle's dual format.** Rows logged and persisted; columns in memory with no extra logging [c28],
in units each a snapshot as of an SCN [c29]. At commit the changed rows are journaled [c30], by
row and SCN, never by value [c34]; a scan reads journaled rows from the row store [c35]; as the
journal grows scans slow, and a background rebuild makes the unit anew [c31] beside the old one
[c36]. The column store is an area of a size the user gives [c32], compressed 2x on their test
[c33]. A restart empties it and refills it with I/O- and CPU-heavy work [c37], which Oracle answered
with copies on disk that lag the live ones [c38].

**SQL Server's columnstore indexes.** Declared on an in-memory table as on a disk table [c39] and
kept in the writing transaction. On an in-memory table new rows and versions go to a tail [c103]
that a background task compresses once a million rows have stayed unchanged for an hour [c42],
without logging the moved rows' locators [c43]; the compressed groups are files that can be evicted
[c41], 10% to 20% over the table's own memory [c40]. Kept this way, the columns made inserts 11.9%
slower, updates 46.7% and deletes 25.6% [c45][c46][c47], and scans 55X faster than the rows [c44],
slowing by 47.3% after a batch of updates until the tail was compressed [c48]. On a disk B-tree, a
secondary columnstore can't find a deleted row in its compressed groups without a scan [c49], so it
buffers the deletes and every scan joins against them, and its disk tables' delta groups are
compressed once they hold 1 million rows [c54]: 1000 small updates took 1.400 s against the
row store's 0.893 [c50], and a scan went from 4.7 s to 16.8 s after 2% of the rows were updated
[c51][c52]. Microsoft's guide says plainly that transactions keeping such an index pay some
additional overhead [c53].

**TiDB's TiFlash.** Learner replicas that never vote [c55] decode rows from the Raft log into
columns [c56], declared per table [c98], buffering rows up to a size or time limit [c57] and keeping
them in a DeltaTree in memory and on disk [c58] at a write amplification of 16.11 [c59]. A read at a
timestamp waits until its learner has applied that far [c60]: within 300 ms on 10 warehouses and
about one second at most on larger ones [c61][c62]. Analytics on the learners cost the transactions
at most 10% [c63], on separate machines.

**Data Blocks, with positional deltas.** Instead of merging a write-optimized part into the whole
relation, which grows with the relation and recompresses all of it [c64], HyPer freezes cold rows into immutable compressed blocks
as their only home [c65]: a frozen row can only be flagged deleted, an update is a delete and an
insert into the hot part [c66], encodings stay byte-addressable for point reads [c67], and min/max
summaries are not kept on hot data because they would slow transactions [c68]. With only cold rows
in blocks TPC-C ran 88,699 transactions a second against 89,229 [c71]; a read by primary key into a
block cost about 60% more [c70]; scans on compressed blocks gained a 1.27 geometric mean over
HyPer's uncompressed columns [c69]. Positional delta trees keep a column store's changes by
position instead of by key [c72], find an insert's place by a search on the sort key helped by its
order [c74][c75], and must be merged back to bound their memory [c73].

**Colibri.** One relation in two formats: a B+-tree on a row ID given in insertion order [c76]
whose leaves point to hot pages or to immutable compressed block files written before the commit
and not logged [c78]; a cold row updated moves to the hot end [c77]; leaves turn cold when
unchanged for a time the authors measured on their hardware and visible to every transaction
[c79]; keys reach rows through logged secondary B-trees [c81]. It ran TPC-C at 300,000 Tx/s, 8 %
below its own row store [c82]; from disk its scans were 4x faster than rows that didn't fit [c84],
2.5x slower than from memory [c85]; with the data in memory, rows held their own [c83]. It chose
moving rows out of blocks over positional deltas, which complicate reads [c80].

## The comparison

| design | freshness | memory | write cost | fit with Amphora's rows and log |
|---|---|---|---|---|
| SQLite3/HE | exact if applied in the transaction or at the read; else behind by the apply [c24] | all of each copied table, in memory, never measured [c26][c27] | a memlog entry a write [c22]; its own cost never isolated [c27] | close: the memlog is the redo record [c22][c1]; order from one writer where Amphora has the publish lock [c23][c2] |
| Oracle dual format | exact at every scan's snapshot: journaled rows read from the row store [c29][c34][c35] | a user-sized area [c32]; rebuilt at every restart [c37] | a journal entry a changed row at commit, no values [c30][c34] | close: an SCN is a commit timestamp, rows at any snapshot come from the versioned B-tree [c3][c35] |
| SQL Server columnstore | exact: kept in the transaction; scans pay for the tail until it is compressed [c48][c52] | 10% to 20% over an in-memory table; evictable files [c40][c41] | 11.9% to 46.7% on in-memory tables; 1.400 s against 0.893 on a disk table [c45][c46][c50] | the shape fits (a secondary index on a B-tree) but its cost breaks the 5% bar [c16][c46] |
| TiFlash | a read waits for its learner: within 300 ms to about one second [c60][c61][c62] | the learner's own, memory and disk [c58] | none on the leader beyond replication [c55]; at most 10% from analytics elsewhere [c63] | needs rows in the log, where Amphora's replicated log holds pages [c56][c6]; a laptop has no learner [c9] |
| Data Blocks | none to keep: the blocks are the rows [c65] | compressed cold rows only [c66] | small with only cold rows in blocks [c71]; summaries too costly on hot data [c68] | a primary format for HyPer's columnar rows [c65]; Amphora's rows are records in B-trees [c1] |
| Colibri | none to keep: one relation [c76] | the buffer pool; blocks from disk at 2.5x the memory time [c85] | 8 % on TPC-C [c82] | rows in arrival order with keys through secondary trees [c76][c81], where kiln's tables are clustered by key [c8] |

## 1. Columns as a copy derived from the rows, kept from the commit stream, exact at every snapshot, every size learned where it runs

**Decision.**

1. **The rows stay the record.** Each table's rows stay in their B-tree, which alone serves writes,
   transactions and point reads [c3][c95]. A table's columns are a copy derived from its rows,
   declared on the table, as SQL Server's and TiDB's are [c39][c98]. Nothing about the copy is
   ever needed to answer correctly: without it, a scan reads the rows. In the SQLite dialect the
   declaration is a table option, `COLUMNAR`, after the column list:
   `CREATE TABLE t (...) COLUMNAR`, or `STRICT, COLUMNAR`. Why that form: SQLite says what a table
   is by its table options, `STRICT` and `WITHOUT ROWID`, written after the closing parenthesis
   and separated by commas, and it keeps each CREATE statement's text in `sqlite_schema`, so the
   declaration is read back at every open with no record of its own. A declared table keeps a
   copy on the versioned storage, bought by its measured use (`columns_sql`: rent before buying);
   a table not declared never keeps one, and the translator hands the copies no statement of its.
   The pager's storage keeps no copies, so there the option changes no answer. No process-wide
   switch turns copies on; changing the declaration means making the table anew, since no ALTER
   form is built.
2. **Fed by the commit stream, after the commit.** The copy takes the redo record each commit
   already writes [c1]: the commit hands it over, as built, to the copy's queue at the point it
   appends it to the redo log, under the lock that orders commits [c2], so the copy is offered
   every commit a snapshot can see. The writer does nothing else for the copy; the copy's own
   work applies the records to the columns in timestamp order, as many as are queued at a time,
   with no timer [c97]. The queue's bytes are charged to the copy's memory (CLAUDE.md §2); a commit
   that finds it full marks that table's copy behind, counted and reported, and the table's scans
   read rows until a rebuild from a snapshot catches up. A commit is never refused or slowed for
   the copy.
3. **Exact at every snapshot, without waiting.** The copy is made of blocks, each current as of a
   commit timestamp [c29], with a journal of the keys whose rows changed after it and when, never
   their values [c34]. A scan at snapshot S reads a block's rows as the block holds them, except the
   rows the journal or the queue shows changed at or before S, which it reads from the row B-tree
   at S [c35][c3]. A block made after S is not used for S; a block's previous version is kept while
   a snapshot may need it [c36], within the copy's memory, and a scan that finds neither reads rows.
   A scan never waits for the copy to catch up [c4].
4. **Derived, never logged.** Nothing of the copy goes into the redo log [c28][c43]: it is rebuilt
   from the rows at a snapshot, or found still current, never replayed. Whether it lives in memory
   only and is rebuilt at open [c25][c37], or is also kept as files reused at open where still
   current [c38][c78], is step 2's choice, by its numbers.
5. **Immutable blocks, a tail for changes.** Rows enter compressed blocks once cold; a change to a
   row in a block flags it deleted there, and its new version lives in an uncompressed tail until
   it cools [c66][c77]. Blocks are cut in the row tree's key order, so a changed key finds its
   block by the blocks' key bounds and its place by a search of the block's key column [c74][c75]:
   the locator SQL Server's secondary columnstore lacked [c49].
6. **Every size and time learned where it runs.** None of the copy's sizes, times or shares is a
   number in the code, from the literature or from a run [c91][c93][c101]. By Ada's word of
   2026-11-21, a value is learned where it runs and adapts as conditions move; one measured once on
   one machine and frozen, or derived so that it comes out the same whatever the machine or the
   data, is a picked constant. So:
   - *Block size*, per table: the copy measures, on every scan, the cost per row it scanned, how
     many blocks the summaries let it skip, and the bytes per row each encoding left, by the size of
     the block; new and rebuilt blocks are cut at the size whose measured cost per scanned row is
     least, so the size follows the data and the machine as they change. Too small a block wastes
     its bytes on metadata and too large skips too little [c102][c68].
   - *When a row is cold*, per table: as it applies commits, the copy records how long rows go
     between changes, and a row moves into a block when the chance, so measured, that it changes
     again before its block's next rebuild costs less than keeping it in the tail, both costs as
     measured on this machine. It is the rule SQL Server leaves to its users [c99], made automatic;
     Colibri saw the best fixed threshold move with the memory the system had [c100][c101].
   - *When a block is rebuilt*: when the extra cost its deletes and journal add to the scans that
     read it, as measured, exceeds its rebuild's measured cost spread over the scans expected before
     it changes again. Not at a share of deleted rows, and not on a timer [c31].
   - *The copy's memory*: within the database's memory budget (CLAUDE.md §2), the copy and the
     rows' buffer pool share it by measured return: the scan time a byte of copy saves against the
     reads a byte of pool saves. Not a size the user gives once [c32].
   - *Which path a statement takes*, per table: the copy or the rows, whichever costs less as
     measured on that table, a tie to the rows. Both sides are measured the same way, by the work
     itself, in nanoseconds per row of the table: the copy's from the time its scans took, the
     rows' from the time the engine's own row-by-row loop took for the statements the copies handed
     back to it, while the statement runs and not while its caller holds a row. Neither side is
     priced by a calibration loop that times other work. A table's first analytic statement pays
     the rent on the rows and so measures them; until the copy's first scan is measured, a
     whole-table statement tries the copy at the rows' measured cost. A side never timed on a
     table is not guessed: the rows answer, and their statement measures them.
   These estimators are part of the copy's core; each starts from what it measures at open and
   keeps measuring. Their state and their inputs are exported as metrics (names agreed at step 2).
7. **One core, local first.** The copy's core is sans-IO: it takes commit records in order and
   answers scans at a snapshot, the same on a laptop and in a fleet [c9]. Locally it is fed by the
   versioned storage's redo records [c1]; a replica can run the same core once the entries it
   applies carry rows, which today they don't [c6]. Whether copies live on replicas that don't vote,
   as TiFlash's do [c55][c56], is placement's choice (CP-T1.9).

**Why.**
- *Write cost.* Kept in the writer's transaction, columns cost SQL Server's writes 11.9% to 46.7%
  [c45][c46][c47] against the plan's 5% [c16]. Fed from what the commit already logs [c1], the
  writer's only added work is a hand-over under a lock it already holds [c2]; the designs that keep
  columns from a log or journal after the commit are the ones whose writers pay least [c30][c55].
- *Fit.* Amphora's rows already sit in versioned B-trees that answer any snapshot [c3], and each
  commit already writes its row changes in order [c1]: a copy needs no new log and changes nothing
  under the rows. A columnar primary format would replace that storage while it is still being
  built [c7]; Colibri's would order rows by arrival where kiln's are clustered by key [c76][c8] and
  read every row by key through a secondary tree [c81], the path on which Data Blocks measured about
  60% more per read [c70]; Colibri itself gave up 8 % of TPC-C [c82].
- *Freshness.* Oracle shows a copy answering exactly at every snapshot without the writer copying
  values or the reader waiting [c34][c35]; Amphora's readers must not wait [c4], which rules out
  TiFlash's read-index wait for local scans [c60].
- *Memory.* A copy costs memory beside the rows [c40][c33]. The sources split on keeping it all in
  memory [c28][c21] or paging it from files [c41][c58][c78], so step 2 measures both.

**Not taken.**
- *Colibri's one relation in two formats*: it gives up a second copy and freshness to keep, at the
  cost of a second storage rewrite and kiln's key order (Why, above).
- *Data Blocks as the primary format*: they rest on HyPer's rows already being columns [c65].
- *SQL Server's columns kept in the transaction*: the write cost [c45][c46][c47].
- *SQLite3/HE's eager and lazy modes*: one pays in the writer, the other in the first reader [c24].
- *TiFlash's wait for the learner*, for local scans [c60][c4]; and learners on the replicated log
  until its entries carry rows [c56][c6].
- *Positional delta trees in place of the tail*: not first, since they complicate reads [c80]; step
  2 builds them only if the tail's merge cost is what limits scans.

**Falsified by.**
1. Write throughput at 32 clients with the copy kept falls by more than 5% [c16] in both of step
   2's prototypes. A copy fed after the commit then costs writers too much, and the next candidates
   are a copy on a replica that doesn't vote [c55] or Colibri's single store [c82].
2. A scan whose answer differs from the rows at its snapshot [c17], under concurrent writes, a crash
   and reopen, or a full queue.
3. The copy's apply falls behind the commit stream at 32 clients without bound while writers run,
   so scans read ever more rows from the B-tree [c16].
4. At step 5 the analytic classes stay below the best of CedarDB and ClickHouse [c19] while the
   profile shows the time in the journal and the tail, not in decoding and scanning blocks: then
   merging is the bottleneck, and a single store with nothing to merge is next [c76].
5. A learned value that settles at the same number on different machines or data sets where the
   costs it measures differ, or that loses to some fixed value from the sources' range on any of
   them [c91][c93]: then it is a constant in disguise, or its estimator measures the wrong thing.

## 2. Column copies granted their memory before they are built, a refused copy bought again by a switch

**Decision.**

1. **Asked before built.** Each block of a table's copy asks the storage's memory source for its
   bytes before it is made, sized from the block's format alone (`Block::bytes`, equal to the made
   block's resident bytes), and each change asks for its bytes before it is applied. A build refused
   part way gives back what it took with the blocks it made. So a copy never holds a byte its source
   did not grant, and a capped source is never passed (T-CB.1.1, T-CB.1.2, T-CB.1.3).
2. **Bought again by one of two ways, behind `AMPHORA_COLUMN_REBUY`.** A, the control and today's
   behaviour: a refused copy is let go, its table's next analytic statement pays the rent again and
   the one after buys again, reading the whole table. B (`room`): a refused copy waits until the
   source reports room (`Source::room`) for its expected size, what it needed when refused and what
   its table's commits put since; no whole-table read is spent before (T-CB.2.1, T-CB.2.2).
3. **Instruments.** Counters (`CopyCounts`: rebuys, the rows and bytes builds and rebuys read, the
   statements and rows the row loop answered against the rows the copy served, the time copies were
   absent), a metered source (`Meter`: takes asked, granted and refused, bytes held and at most),
   the copies waiting and for how long, the bytes resident against the bytes granted, and trace
   events with their reasons under the target `amphora::columns`.
4. **Which way is the default is open.** The A/B harness (`columns_sql::tests::ab`, T-CB.4.1) forces
   each way's failure mode: a laptop's cap with one big table, many tables under a small cap, steady
   pressure, bursty pressure that frees memory in small pieces, a table that grows while its copy
   waits, and a huge cap with many tables; and at fleet scale, 64 tables in half a copy's room, and
   64 growing tables given room for every copy at once and then none, so copies are bought and let
   go together. The scenarios are defined once (`storage::versioned::columns_ab`, AC-CB.4.3) for the
   test and the DuckDB driver; each record says what its scenario forces, counts the copies' trace
   events by reason, and gives the last copy let go with its refusal, the copies still waiting, and
   the writes the cap refused. It writes one JSONL record per way and scenario. The
   choice waits for the coordinator's run of record; until then A stays the default. Against DuckDB,
   one driver (`amphora-bench --colcopy`, AC-CB.4.2) runs the same scenarios on Amphora under both
   ways and on DuckDB, both in process and in memory, each statement timed the same way. DuckDB's
   cap is `SET memory_limit` to the bytes it holds before the statement plus the scenario's free
   bytes: its table is its columns, so it gets the same room for the analytic work as Amphora's copies.
   The scenario's room is the analytic work's alone (lane colcopy-8, AC-CB.4.4): while a table's rows
   grow, each engine's cap is lifted (Amphora's other use gives its bytes back, DuckDB's limit is
   reset) and set again after, so a copy that must grow with its table meets the cap, not the rows;
   the A/B test checks that no growing table's write is refused and that, under waves, copies a wave
   bought are let go in a round after it (T-CB.4.3). The same driver
   runs the concurrency ladder (`--ladder`, AC-CB.4.5): 1, 8, 32, 128, 512, 2048 and 4096 clients at
   once on each engine, each its own connection and thread, one record a step with p50, p99 and
   p99.9, and the client's threads, connections, peak resident bytes and own share of the time, to
   show whether the client bounds the top step. Only statements a copy could answer are weighed
   (lane colcopy-9, AC-CB.4.6): a statement the scan hook hands over that no copy can answer (its
   rows, a filter only its loop states, a join, a transaction) reads the rows under either way, so
   it is counted with its reason and traced, and left out. The A/B test tells statement by
   statement from the copies' counts; the driver runs each query once on a probe table first, and
   leaves the same ones out on DuckDB and in the ladder. Each record's percentiles count only the
   statements that reached the copies, every statement's beside them, and it names the queries
   left out and why. A transaction that only reads reads the copy at its own snapshot (lane
   colcopy-10, AC-CB.4.7): a generation made at or before the snapshot and not behind there answers
   it, the current one or the one it replaced, which is kept while an older snapshot is open; a
   rebuild is made at a snapshot of every commit now, never at an older transaction's, which would
   lack the commits between them. A snapshot no generation answers is refused, counted `unserved`
   and traced, and the rows answer it. What the copy answered is kept as read, the whole table, for
   the transaction's validation, as the rows' cursor walking every row would keep it. Only a
   transaction that changed rows still skips the copy.

## What the next steps prototype and measure

**Step 2, in-memory columns against persistent column blocks.** Both prototypes implement the whole
decision and differ only in its part 4.
- *A*: blocks in memory only, unlogged; rebuilt at open from a snapshot scan of the rows [c25][c37].
- *B*: compressed blocks also kept as files outside the redo log, with their deletes, reused at
  open where still current [c38][c78]. B must show how it learns at open which of its rows changed
  after its blocks were made, when a checkpoint has cut the redo that held those changes [c96]: by
  keeping each block's key journal with it, or by holding the cut for the copy; it measures both.
- Each is measured on the same machines, corpus and commit, best of three, on at least two machines
  with different cores, memory and disks, so the learned values of part 6 are seen to move (CLAUDE.md
  §7): write throughput and commit p99 at 1 and 32 clients with the copy kept and without [c16]; the
  lag from a commit to its application in the copy, p50 to p99.9, and the rows scans read from the
  B-tree because of it; the copy's resident bytes per row against the rows' own; the analytic
  classes' speed with the copy current and with a tail [c12][c13][c14]; the time to open, by table
  size; the values part 6's estimators settle at on each machine and how fast they follow a change
  of workload; and the plan's test, a scan at each snapshot under concurrent writes equal to the
  rows at that snapshot [c17], with a crash and reopen and a full queue as the nemesis.
- The choice, in order: the prototype whose writes stay within the bar [c16], then the faster
  analytic classes, then the fewer bytes per row. The loser is recorded with its numbers.

**Step 3, BtrBlocks against FastLanes.** BtrBlocks's scans ran 2.2× faster than the formats it
was compared with [c86]; FastLanes decodes >40 values per CPU cycle [c88], in vectors of 1024
[c89]. Both in safe Rust [c11] with no recursion [c10]:
BtrBlocks's cascade, recursive to a depth it takes as a parameter [c87], becomes a loop whose depth
is chosen per block by the same sampled estimate that chooses its schemes, so no depth is fixed;
FastLanes is scalar code the compiler vectorizes [c90], checked for vector instructions on x86_64
and aarch64. Measured on the corpus's columns: values decoded per cycle, at least 8 with the winner
[c18], and bytes per value. Since the copy never serves point reads [c95], it may use the sub-byte
encodings Data Blocks gave up to keep point reads fast [c67]; but part 5's search of a block's key
column for a changed key is measured with each.

**The steps after.** The plan names starting block sizes from ClickHouse and DuckDB [c91][c92].
With Colibri's and SQL Server's, the sources' sizes are 8,192, 122,880, 262,144 and about a million
rows [c91][c92][c94][c93]; under part 6 they are neither starting points nor defaults, only the
range a learned size is checked against (falsifier 5). The planner sends whole-table and wide-range statements to the copy and
point reads to the rows [c95], by measured cost.

## How the claims are checked

`crates/amphora/tests/columnar_decision.rs` (T-CS.1.1) reads this file and `docs/research/columnar.md`:
- every `[cN]` here is a claim line there, with a source, a locator and a quote, and every claim
  line there is cited here;
- every source a claim names is in the note's table with a locator and a grade, A to F;
- every paragraph, list item and table row here that states a number cites a claim, and each number
  it states is in the quote of a claim it cites (section, step and decision numbers, IDs, dates and
  code excepted);
- every paragraph of "What a design must fit" and "The six designs" and every cell of the comparison
  after its first column cites a claim.

That every quote is the source's own words, on the page given, was checked against the sources
themselves when the note was written (its "How to read this"); the test can't fetch them.
