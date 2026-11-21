# Columns beside rows: the HTAP designs at their sources

Source notes for the columnar decision (`docs/design/columnar.md`; G11 step 1, kiln plan
`01a16b0f477676d5a7ce168b80aa29f5`, research `01a16b0f477676d5a7ce168c89606fd2`). They compare six
ways of keeping columns for analytics beside rows for transactions: SQLite3/HE, Oracle's dual
format, SQL Server's columnstore indexes, TiDB's TiFlash, HyPer's Data Blocks (with the positional
delta trees they build on) and CedarDB's Colibri. Each is read for four things: how fresh its
columns are, what memory they take, what they cost a write, and how well they would fit Amphora's
row B-trees and its log. The compression papers the next steps start from (BtrBlocks, FastLanes)
and the block sizes others use are here too.

## How to read this

Each claim the decision rests on is a line below: its ID (`c1` onwards, which the decision cites
as `[c1]`), its source, where in the source, and the source's own words, quoted. The decision may
state a number only if a quote it cites holds that number; `crates/amphora/tests/columnar_decision.rs`
checks this, and that every ID resolves to a quote, a locator and a graded source (T-CS.1.1).

Where the quotes come from, and how they were checked (2026-11-21):
- **Papers**: each PDF fetched from the locator in the sources table, turned into text with
  `pdftotext` (poppler), and each quote searched for on the page its locator names, after taking
  out white space and hyphens (line breaks split words in PDF text). Every quote below was found
  on the page given. Page numbers are the journal's where the PDF prints them (PVLDB), else the
  PDF's own page count from its first page (CIDR, SIGMOD author copies). The PDFs are kept under
  `~/.cache/columnar-src/` on the machine that checked them; they are not in this repository.
- **Vendor documentation**: the pages fetched at the locator, tags stripped, each quote found in
  the text under the section it names.
- **Amphora's own**: this repository's files at the commit this note lands in, by line.
- **The G11 plan**: kiln's `plans_get` of the plan, version 2.

Kiln's research for this plan graded its findings f2 to f8 C or B with their quotes unchecked,
because kiln does not read a PDF's words. Every quote those findings rest on was found on its page
by the check above; the ones this note uses are `c20`, `c28`, `c39`, `c56` and `c86`. Kiln's research names the
Colibri paper "Colibri: A Cloud-Native Hybrid Column-Row Store"; the paper at that locator is
titled "Two Birds With One Stone: Designing a Hybrid Cloud Storage Engine for HTAP" (PVLDB
17(11): 3290-3303); Colibri is the architecture it proposes.

### Grades

A grade says how far a source can be trusted for the kind of claim cited from it, not how good
the technique is.
- **A**: the thing itself: Amphora's code, design or plan, read at a commit; a specification.
- **B**: a full peer-reviewed paper by the people who built the system, describing what they
  built and measured on their own hardware; or the vendor's reference documentation for how its
  product behaves. Design descriptions from these are reliable; their numbers hold for their
  workload and hardware, were not reproduced by anyone else, and come from parties with an
  interest in the result.
- **C**: a short paper with partial evaluation, or a measurement taken under conditions that
  weaken it. Good for what a design is; weak for how much it costs.
- **D** to **F**: secondary sources, blogs, unsourced claims. None is cited.

## Sources

| key | source | locator | kind | grade | why this grade |
|---|---|---|---|---|---|
| `sqlite3he` | Prammer, Sahadevan Rajesh, Chen, Patel, "Introducing a Query Acceleration Path for Analytics in SQLite3", CIDR 2022 | https://par.nsf.gov/servlets/purl/10390275 (PDF pages) | paper | C | Six pages; one machine (two Xeon Silver 4114); memory never measured; "no cost" rests on TATP throughput where faster reads offset the extra write, not on the write's own cost; the consistency design is left to future work by the authors (p. 6). Code at github.com/UWHustle/hustle, not pinned or read here. |
| `oracledbim` | Mukherjee et al., "Distributed Architecture of Oracle Database In-memory", PVLDB 8(12), 2015 | https://www.vldb.org/pvldb/vol8/p1630-mukherjee.pdf (pp. 1630-1641) | paper | B | The builders' description of a shipped product; its numbers are for RAC scale-out, not for the cost of DML. |
| `oracle-repop` | Oracle Database 19c In-Memory Guide, ch. 10 "Optimizing Repopulation of the IM Column Store" | https://docs.oracle.com/en/database/oracle/oracle-database/19/inmem/repopulation-and-dml.html | vendor docs | B | Reference documentation of current behavior; no numbers. It names the SMU "Snapshot Metadata Unit" where the 2015 paper says "Snapshot Management Unit". |
| `oracle-faststart` | Oracle Database 19c In-Memory Guide, ch. 11 "Managing IM FastStart for the IM Column Store" | https://docs.oracle.com/en/database/oracle/oracle-database/19/inmem/managing-im-faststart-for-im-column-store.html | vendor docs | B | As above. |
| `sqlservercs` | Larson, Birka, Hanson, Huang, Nowakiewicz, Papadimos, "Real-Time Analytical Processing with SQL Server", PVLDB 8(12), 2015 | https://www.vldb.org/pvldb/vol8/p1740-Larson.pdf (pp. 1740-1751) | paper | B | The builders' description and micro-benchmarks (one table, single sessions, an early build for section 4); the only source here that isolates what keeping columns costs each kind of write. |
| `mssql-rtoa` | Microsoft Learn, "Get started with columnstore for real-time operational analytics" (SQL Server 2017 onwards; updated 2026-10-19) | https://learn.microsoft.com/en-us/sql/relational-databases/indexes/get-started-with-columnstore-for-real-time-operational-analytics | vendor docs | B | Reference documentation; no measured numbers. |
| `tidb` | Huang et al., "TiDB: A Raft-based HTAP Database", PVLDB 13(12), 2020 | https://www.vldb.org/pvldb/vol13/p3072-huang.pdf (pp. 3072-3084) | paper | B | The builders' description; freshness and isolation measured on CH-benCHmark on their cluster; learners on their own nodes, so its interference numbers are for separate machines. |
| `datablocks` | Lang, Mühlbauer, Funke, Boncz, Neumann, Kemper, "Data Blocks: Hybrid OLTP and OLAP on Compressed Storage using both Vectorization and Compilation", SIGMOD 2016 | https://ir.cwi.nl/pub/24382/24382B.pdf (PDF pages) | paper | B | The builders (HyPer); measured on HyPer, a main-memory engine whose hot data is already columnar. |
| `pdt` | Héman, Zukowski, Nes, Sidirourgos, Boncz, "Positional Update Handling in Column Stores", SIGMOD 2010 | https://ir.cwi.nl/pub/16172/16172D.pdf (PDF pages) | paper | B | The builders (Vectorwise); measured inside Vectorwise against its own value-based deltas. |
| `colibri` | Schmidt, Durner, Leis, Neumann, "Two Birds With One Stone: Designing a Hybrid Cloud Storage Engine for HTAP", PVLDB 17(11), 2024 | https://www.vldb.org/pvldb/vol17/p3290-schmidt.pdf (pp. 3290-3303) | paper | B | The builders (Umbra, CedarDB's research parent); every comparison is against systems they configured, on their hardware. |
| `btrblocks` | Kuschewski, Sauerwein, Alhomssi, Leis, "BtrBlocks: Efficient Columnar Compression for Data Lakes", PACMMOD 1(2), SIGMOD 2023 | https://www.cs.cit.tum.de/fileadmin/w00cfj/dis/papers/btrblocks.pdf (PDF pages, author copy) | paper | B | The builders; compared with formats they did not build, on public data. |
| `fastlanes` | Afroozeh, Boncz, "The FastLanes Compression Layout: Decoding >100 Billion Integers per Second with Scalar Code", PVLDB 16(9), 2023 | https://www.vldb.org/pvldb/vol16/p2132-afroozeh.pdf (pp. 2132-2144) | paper | B | The builders; micro-benchmarks across four CPU families. |
| `chpaper` | Schulze et al., "ClickHouse - Lightning Fast Analytics for Everyone", PVLDB 17(12), 2024 | https://www.vldb.org/pvldb/vol17/p3731-schulze.pdf (pp. 3731-3744) | paper | B | The builders; cited for a format value only. |
| `duck_rg` | DuckDB, `src/include/duckdb/storage/storage_info.hpp` at 1641475ef1db08ec063631ff6a8f08d9d6736ae7 | https://raw.githubusercontent.com/duckdb/duckdb/1641475ef1db08ec063631ff6a8f08d9d6736ae7/src/include/duckdb/storage/storage_info.hpp | code | A | The constant itself, at a pinned commit. |
| `engine-md` | Amphora, `docs/design/engine.md` (CP-T1.3) | docs/design/engine.md (lines) | design | A | Amphora's own decision record. |
| `redo-rs` | Amphora, `crates/engine/storage/versioned/redo.rs` | crates/engine/storage/versioned/redo.rs (lines) | code | A | The code itself. |
| `replication-md` | Amphora, `docs/design/replication.md` (U15, CP-T1.8) | docs/design/replication.md (lines) | design | A | Amphora's own decision record. |
| `design-readme` | Amphora, `docs/design/README.md` | docs/design/README.md (lines) | design | A | Amphora's target. |
| `without-rowid-md` | Amphora, `docs/design/without-rowid.md` | docs/design/without-rowid.md (lines) | design | A | Amphora's own decision record. |
| `status-md` | Amphora, `STATUS.md` | STATUS.md (lines) | status | A | What is built, as recorded. |
| `claude-md` | Amphora's rules, `CLAUDE.md` | CLAUDE.md (lines) | rules | A | The rules themselves. |
| `bench-v6` | Amphora, `bench/records/2026-10-23-adacachy-matrix-v6.md` | bench/records/2026-10-23-adacachy-matrix-v6.md (lines) | measurement | C | Best of 3 passes per target, but taken at load averages of 3.5 to 123.6 where CLAUDE.md §7 asks under 2. The gaps cited are two to four orders of magnitude, far larger than load could make them; their exact sizes are not to be relied on. |
| `g11-plan` | Kiln plan "G11 Analytics at column-store speed", version 2 | kiln plan 01a16b0f477676d5a7ce168b80aa29f5 (`plans_get`), by step | plan | A | The targets themselves. |

## Amphora's terms

What any column design must fit: rows as SQLite records in versioned B-link trees, one redo record
a commit, page images on the replicated path today, and the plan's bar.

- **c1** · engine-md · lines 182-183 (decision 6) · "A commit appends one redo record: each row change of the transaction (the B-tree, the key, the new row or its removal), in the order made. Records are appended in commit-timestamp order"
- **c2** · redo-rs · line 227 · "Appends the record of commit `ts` (call in timestamp order: under the clock's publish lock)."
- **c3** · engine-md · lines 133-134 (decision 4) · "A reader sees, for each row, the newest version committed at or before its start timestamp"
- **c4** · design-readme · line 31 · "Readers on MVCC snapshots never wait behind writers"
- **c5** · engine-md · line 22 · "A commit logs every page it touched, 4 KiB each"
- **c6** · replication-md · lines 10-11 · "A Raft entry contains only the resolved pages and the index/term of the state the session observed."
- **c7** · status-md · line 217 · "the pool still holds every page, and nothing runs it under SQL yet (stage 3)."
- **c8** · without-rowid-md · lines 3-6 · "Kiln's Records and work migrations use composite primary keys and must retain their declared storage semantics. WITHOUT ROWID tables use their primary-key index as the table."
- **c9** · claude-md · line 69 (§8) · "A laptop, a fleet and global multi-region run the same code"
- **c10** · claude-md · line 86 (§11) · "No function calls itself, directly or through others"
- **c11** · claude-md · line 92 (§12) · "`unsafe` is denied"
- **c12** · bench-v6 · line 28 · "| table_count | 1 | 36.8 | 32.512 | 5/7 | cedardb | 2,197 | 0.017 | 0.36 |"
- **c13** · bench-v6 · line 17 · "| filter_no_index | 1 | 0.6 | 1589.248 | 7/8 | cedardb | 2,608 | 0.000 | 0.23 |"
- **c14** · bench-v6 · line 26 · "| table_count | 32 | 140 | 483.328 | 3/7 | clickhouse | 14,127 | 0.010 | 1.12 |"
- **c15** · bench-v6 · line 5 · "load average around the runs: 3.5 to 123.6 (not a quiet machine: CLAUDE.md §7 asks under 2)"
- **c16** · g11-plan · step 2, acceptance · "write throughput at 32 clients with the chosen copy kept falls by no more than 5% compared with the baseline"
- **c17** · g11-plan · step 2, test · "a scan at each snapshot under concurrent writes — passes when equal to the rows at that snapshot"
- **c18** · g11-plan · step 3, acceptance · "values decoded per cycle with the winner is at least 8"
- **c19** · g11-plan · step 5, acceptance · "each analytic class against the best of CedarDB and ClickHouse is at least 1.0x"
- **c95** · g11-plan · step 5, description · "Columns for whole-table and wide-range statements, rows for point reads"
- **c96** · engine-md · line 221 (decision 7) · "then the redo file switched from, which holds only commits up to T, is cut."
- **c97** · redo-rs · line 6 · "a group commit with no timer"

**Implies.** CP-T1.3's redo record is a feed of every committed row change, whole new rows, in
commit-timestamp order, handed out under the lock that orders commits (c1, c2): what the column
designs below build or borrow. It exists only on the versioned storage, which is not under SQL yet
(c7); the engine SQL runs on today logs pages (c5), and the replicated path ships page images
(c6). Kiln's own tables are clustered by their primary keys (c8), not by insertion order.

## SQLite3/HE

A second, in-memory columnar store (Apache Arrow) beside SQLite's B-trees; analytic SELECTs over
tables wholly in memory go to it, everything else to SQLite.

- **c20** · sqlite3he · p. 1, abstract · "SQLite3/HE improves the performance of analytical queries in the 100x-1000x speedup range, at no cost to the existing transactional query performance."
- **c21** · sqlite3he · p. 4, §4.1.2 · "SQLite3/HE utilizes Apache Arrow [3] as an in-memory columnar storage layer."
- **c22** · sqlite3he · p. 4, §4.2 · "Each write log includes information regarding the particular mode of the write (INSERT, UPDATE, or DELETE), the targeted row ID, and the relevant data if appropriate."
- **c23** · sqlite3he · p. 4, §4.2 · "Note that due to SQLite3’s database file-locking mechanisms, the memlog is presented with an ordered collection of changes"
- **c24** · sqlite3he · p. 4, §4.2 · "eager writes perform updates during the transaction, lazy writes only make changes when the table is next read by a query, and async background writes use a secondary thread to perform a lazy write"
- **c25** · sqlite3he · p. 4, §4.2 · "In the case of a crash or some other failure, the SQLite3/HE tables are regenerated from the SQLite3 database."
- **c26** · sqlite3he · p. 4, §4.1.1 · "(2) All source tables are present in memory."
- **c27** · sqlite3he · p. 5, §5.2 · "an improvement of up to 20% is possible. Most configuration and workload combinations will experience a 5%-10% improvement."

**Implies here.** The memlog is CP-T1.3's redo record in all but name: the row, its key and the
change, in commit order (c22, c23 against c1). SQLite3/HE gets the order from SQLite's single
writer (c23); Amphora gets it from the publish lock (c2). Its memory cost is never measured, its
write cost never isolated (c27 is the throughput of a read-and-write mix), so it shows the design
works, not what it costs. Its copy is all or nothing per table (c26) and rebuilt from scratch
after a crash (c25).

## Oracle Database In-Memory: the dual format

Rows logged and persisted as always; the same table in columns in memory, unlogged, as compressed
units (IMCUs), each a snapshot as of an SCN, with a journal of the rows changed since.

- **c28** · oracledbim · p. 1631, §1 · "to be simultaneously maintained in traditional row format logged and persisted in underlying storage, as well as in column format maintained purely in-memory without additional logging."
- **c29** · oracledbim · p. 1633, §3.4 · "Data in an IMCU is therefore a read snapshot as of its associated SCN."
- **c30** · oracledbim · p. 1633, §3.4 · "Once a transaction commits, the changes are broadcast to be journaled in the relevant set of SMUs across the cluster."
- **c31** · oracledbim · p. 1633, §3.4 · "As changes accumulate in the SMU, retrieving data from the transaction journal causes increased overhead for scans. Therefore, a background repopulate mechanism is periodically invoked to rebuild the local IMCU at a new SCN."
- **c32** · oracledbim · p. 1633, §3.3 · "The In-memory Column Store is carved out from the Oracle System Global Area (SGA) [12] per database instance based on a size provided by the user."
- **c33** · oracledbim · p. 1638, §4 · "A 2x compression ratio is achieved with the default compression level"
- **c34** · oracle-repop · §10.2.1 · "The transaction journal does not record the new value. Rather, it indicates the corresponding row as stale as of a specific SCN."
- **c35** · oracle-repop · §10.1.1 · "If a query accesses the data, and discovers modified rows, then it can obtain the corresponding rowids from the transaction journal, and then retrieve the modified rows from the buffer cache."
- **c36** · oracle-repop · §10.2.1.2 · "In double buffering, background processes create new IMCU versions by combining the original rows with the latest modified rows."
- **c37** · oracle-faststart · §11.1.1 · "The IM column store is populated whenever a database instance restarts, which can be a slow operation that is I/O-intensive and CPU-intensive."
- **c38** · oracle-faststart · §11.1.2.1 · "Depending on how much DML activity occurs for a CU, a lag can exist between the CUs in the FastStart area and the CUs in the IM column store."

**Implies here.** The design closest to Amphora's terms. An IMCU's SCN is a commit timestamp (c29
against c3); the journal keeps which rows changed and when, never their values (c34), and a scan
takes those rows from the row store (c35), which Amphora's versioned storage answers at any
snapshot (c3). Freshness is exact at every scan's snapshot, paid for by the scan as the journal
grows, until a rebuild (c31, c36). Memory is a fixed area the operator sizes (c32), with compressed
units (c33 is one table on their RAC test). The weakness is the restart: an empty store refilled
by a CPU- and I/O-heavy population (c37), which Oracle answered by also saving the units to disk,
lagging the live ones (c38).

## SQL Server: columnstore indexes on in-memory and on disk tables

A columnstore index kept in the writing transaction: on an in-memory (Hekaton) table, new rows sit
in the table's tail until a background task compresses the cool ones and hides the old copies
through a deleted-rows table; on a disk B-tree table, a secondary columnstore with B-tree delta
stores, a delete buffer and a delete bitmap.

- **c39** · sqlservercs · p. 1740, §1 · "Users will be able to create columnstore indexes on in-memory tables in the same way as they can now for disk-based tables."
- **c40** · sqlservercs · p. 1742, §3.1 · "The space overhead is data dependent but typically in the range 10% to 20%."
- **c41** · sqlservercs · p. 1742, §3.1 · "Each row group is stored in a separate file on stable storage; this permits the system to evict a compressed segment from memory and reload it as needed"
- **c103** · sqlservercs · p. 1742, §3.2 · "New rows and new versions of rows are inserted into the Hekaton table only, thus growing the tail."
- **c42** · sqlservercs · p. 1743, §3.3 · "data migration is not even started until there is a sufficient number of “cool” rows (at least a million). A row is considered “cool” if it has not been modified within some specified period of time (at least an hour)."
- **c43** · sqlservercs · p. 1743, §3.4 · "SQL Server addresses this problem by not logging changes to the RID column."
- **c44** · sqlservercs · p. 1744, §3.6.1 · "Scanning the CSI instead gives a 55X performance boost."
- **c45** · sqlservercs · p. 1744, §3.6.2 · "Having a CSI on the table increased the insert time by 11.9% because the Tail Index must be updated."
- **c46** · sqlservercs · p. 1744, §3.6.2 · "The cost of maintaining the CSI increased the elapsed time by 46.7% (from 28.9 to 42.4 sec)."
- **c47** · sqlservercs · p. 1744, §3.6.2 · "The CSI increased the cost of deletions by 25.6% (from 30.5 sec to 38.3 sec)."
- **c48** · sqlservercs · p. 1744, §3.6.2 · "Its elapsed time had now increased by 47.3% (to 1.181 sec from 0.802 sec)."
- **c49** · sqlservercs · p. 1745, §4.1 · "Determining the row’s location would require a complete scan of the compressed row groups, which would make deletes unbearably slow."
- **c50** · sqlservercs · p. 1746, Table 3 · "1000 updates 10,000 0.893 1.400 6.866"
- **c51** · sqlservercs · p. 1746, Table 4 · "Newly built 180 99.1 4.7 1.71"
- **c52** · sqlservercs · p. 1746, Table 4 · "After 2% updates 198 109.5 16.8 10.0"
- **c53** · mssql-rtoa · "Get started example" · "The OLTP workloads continue to perform, but incur some additional overhead to maintain the columnstore index."
- **c54** · mssql-rtoa · "Performance tip #3" · "The newly inserted rows will get compressed once 1 million rows have been inserted into a single delta rowgroup."
- **c99** · mssql-rtoa · "Performance tip #3" · "if for your workload, the newly inserted stays 'hot' (that is, gets updated multiple times) for say 60 minutes, you should choose COMPRESSION_DELAY to be 60."

**Implies here.** The only source that measures what columns cost each write when they are kept
in the writer's transaction: an insert 11.9% slower, an update 46.7%, a delete 25.6% (c45 to
c47), and on a disk table 1,000 small updates 0.893 s against 1.400 s (c50, columns: row store,
secondary columnstore, primary columnstore). Each is far past the plan's 5% (c16). Freshness is
perfect, since the index changes with the row, but scans pay for the tail and the deletes until
the background catches up: 0.802 s to 1.181 s after 400,000 updates (c48), 4.7 s to 16.8 s after
2% of the rows updated (c51, c52). Memory: 10% to 20% over the in-memory table (c40), segments
evictable to files (c41). The secondary index on a B-tree is Amphora's shape, and its hard part
is finding a deleted row in the compressed groups without a scan (c49). Its cold rule is picked,
not measured: a million rows, an hour (c42).

## TiDB: TiFlash learners

Columnar replicas on their own nodes, fed by the Raft log as learners that never vote; rows
decoded from the log, buffered, turned into columns, kept in a DeltaTree (appended deltas in
memory and on disk, merged into stable chunks).

- **c55** · tidb · p. 3073, §2 · "A learner does not participate in leader elections, nor is it part of a quorum for log replication."
- **c56** · tidb · p. 3076, §4.2 · "TiFlash is composed of learner nodes, which just receive Raft logs from Raft groups and transform row-format tuples into columnar data."
- **c57** · tidb · p. 3076, §4.2.1 · "If the data size in the row buffer exceeds a size limit or its time duration exceeds a time interval limit, these row-format tuples are transformed to columnar data"
- **c58** · tidb · p. 3077, §4.2.3 · "These deltas are cached into memory and materialized into disks."
- **c59** · tidb · p. 3077, §4.2.3 · "Although the write amplification of DeltaTree (16.11) is greater than the LSM tree (4.74), it is also acceptable."
- **c60** · tidb · p. 3077, §4.2.4 · "After receiving a read request, the learner sends a read index request to its leaders to get the newest data that covers the requested timestamp."
- **c61** · tidb · p. 3082, §6.5 · "the log replication delay is always less than 300 ms on 10 warehouses, and most delays are less than 100 ms."
- **c62** · tidb · p. 3082, §6.5 · "TiDB can guarantee data freshness of about one second on HTAP workloads."
- **c63** · tidb · p. 3082, §6.4 · "more analytical processing clients degrade the TP throughput at most 10% compared to no AP clients."
- **c98** · tidb · p. 3076, §4.2 · "A user can set up a column-format replica for a table using an SQL statement"

**Implies here.** The writer pays nothing beyond replicating its log (c55), and a read at a
timestamp waits until its learner has applied that far (c60): fresh to within about a second
(c61, c62). Memory and disk are the learner's own (c58), with a write amplification of 16.11 on
its disks (c59). The fit is the log: TiFlash decodes rows from log entries (c56), and Amphora's
replicated entries are page images (c6), so a learner here would first have to turn pages back
into rows. And a learner is a separate machine, which a laptop running Amphora doesn't have; the
same core must serve one node and a fleet (c9).

## Data Blocks, and positional deltas

HyPer's cold data frozen into immutable compressed blocks with small materialized aggregates (min,
max) and positional SMAs; hot data in uncompressed chunks; an update to a frozen row is a delete
flag and an insert into the hot part. Positional delta trees (Vectorwise) keep a column store's
updates by tuple position rather than by key.

- **c64** · datablocks · p. 2, §1 · "the merge process has O(n) time complexity (where n is the relation’s cardinality) and requires re-compressing of the entire relation."
- **c65** · datablocks · p. 3, §3 · "In HyPer, Data Blocks are used as a compressed in-memory storage format for cold data and for persistence."
- **c66** · datablocks · p. 3, §3 · "Once records have been packed into a Data Block, the contained data is immutable (frozen). Only delete operations are permitted where frozen records are marked with a flag. Updates are internally translated into a delete followed by an insert into a hot uncompressed chunk of the relation."
- **c67** · datablocks · p. 3, §3 · "(ii) Only byte-addressable compression formats are used and the use of sub-byte encodings such as BitWeaving [22] is rejected in order to allow for efficient point accesses"
- **c68** · datablocks · p. 4, §3.2 · "in HyPer we refrain from using these for hot uncompressed data as maintaining the minimum and maximum information as well as updating the PSMA indexes would have a very high negative impact on transaction processing performance."
- **c69** · datablocks · p. 8, §4.1 · "the geometric mean of the 22 query runtimes improves by 1.27"
- **c70** · datablocks · p. 11, §5.3 · "With a primary key index (traditional global index structure), we measured an overhead of around 60% when accessing records in Data Blocks."
- **c71** · datablocks · p. 11, §5.3 · "We measured a transaction throughput of 89,229 transactions per second on uncompressed storage and 88,699 transactions per second if the cold neworder records are stored in Data Blocks."
- **c72** · pdt · p. 1, abstract · "Rather than keeping differential structures organized by the table key values, the core proposition of this paper is that this can better be done by keeping track of the tuple position of the modifications."
- **c73** · pdt · p. 3, §2 · "Differential updates need to be eventually propagated to the stable storage to reduce the memory consumption and merging cost."
- **c74** · pdt · p. 7, §3.2 · "It can be found with a query that computes the minimum RID whose tuple has a larger SK"
- **c75** · pdt · p. 7, §3.2 · "This query could profit in the table-scan from the order on SK, exploiting e.g. a sparse index"

**Implies here.** Data Blocks are a primary format, not a copy: cold rows live only in blocks
(c65, c66), and the hot part is HyPer's columns, uncompressed. Their write cost is small when the
blocks hold only cold rows (c71) and their price is point reads (c70) and summaries that can't be
kept on hot data (c68). The 1.27 geometric mean (c69) is their scans on compressed blocks against
HyPer's own uncompressed columns, not against rows. For a copy beside rows they give the block:
immutable, summarized, deletes as flags (c66). Positional deltas are the alternative to rewriting
blocks; to place an insert they search the sort key, helped by its order (c74, c75), and they
must be merged back to bound their memory (c73).

## Colibri (CedarDB, Umbra)

One relation, two formats: a B+-tree on an insertion-order row ID whose leaves point to hot
uncompressed pages (PAX, logged) or cold compressed Data Blocks files (immutable, flushed before
commit, never logged); cold means unmodified and globally visible for a measured time; an update
to a cold row deletes it by a bit and reinserts it at the hot end; secondary B-trees map keys to
row IDs.

- **c76** · colibri · p. 3294, §4.1 · "Every tuple is assigned a unique and monotonic increasing row ID upon creation, i.e., the records are implicitly sorted by the insertion order."
- **c77** · colibri · p. 3294, §4.1 · "Note that once a cold record is updated, we decompress it and move it to the hot section."
- **c78** · colibri · p. 3294, §4.1 · "A compressed data block file is immutable after creation, and we persist it before committing the transaction."
- **c79** · colibri · p. 3295, §4.5 · "If this transaction has become globally visible, i.e., no older transaction exists, and the last modification happened more than 10 seconds ago, the leaf and its blocks are cold"
- **c80** · colibri · p. 3295, §4.4 · "Column stores can avoid out-of-place updates using differential update structures like the Positional Delta Tree [32]. However, this approach requires an additional tree structure to store the deltas and complicates data retrieval."
- **c81** · colibri · p. 3297, §5.1 · "The secondary indexes are stored on standard buffer pages, and all modifications are logged."
- **c82** · colibri · p. 3298, §6.2.1 · "Colibri, with its row store for the hot part of data processes 300 000 Tx/s, only 8 % less than the row store."
- **c83** · colibri · p. 3299, §6.2.2 · "Row stores provide competitive performance for in-memory analytics."
- **c84** · colibri · p. 3300, §6.2 · "Even as we load all the data from disk, the column-row store is 4x faster than the row store."
- **c85** · colibri · p. 3300, §6.3.1 · "Compared to the in-memory run, reading the data from disk is only 2.5x slower"
- **c100** · colibri · p. 3295, §4.5 · "The buffer size impacts the performance and the threshold to use."
- **c101** · colibri · p. 3295, §4.5 · "For small buffer sizes (8 to 32 GB), performance varies by 2x between a 0.1 s and a 300 s threshold."

**Implies here.** Colibri has no second copy and so no freshness to keep: a scan reads the one
relation at its snapshot. Its cost is borne by the writes it moves (c77) and the 8% it gave up
on TPC-C against its own row store (c82). Its memory is the buffer pool: cold columns load from
disk at 2.5x the in-memory time (c85) and still beat rows that don't fit by 4x (c84). The "row
store" it beats in memory is Umbra's PAX pages, already columns within a page (c83). The fit is
the order: its relation is sorted by when rows arrived (c76), and kiln's tables are clustered by
composite primary keys whose storage semantics are kept (c8). Taking it would mean a hidden row ID
under every table and each primary-key read through a secondary index (c81), the path on which
Data Blocks measured about 60% more per read (c70).

## Compression and block sizes, for the next steps

- **c86** · btrblocks · p. 2, §1 · "scans using BtrBlocks are 2.2× faster and 1.8× cheaper due to its superior decompression performance."
- **c87** · btrblocks · p. 4, §3.2 · "The maximum number of recursions is a parameter of the compression algorithm, with the default value set to 3."
- **c88** · fastlanes · p. 2132, abstract · "decoding >40 values per CPU cycle"
- **c89** · fastlanes · p. 2132, abstract · "amortizing function call overhead over 1024 tuples"
- **c90** · fastlanes · p. 2133, §1 · "modern compilers can auto-vectorize our scalar code"
- **c91** · chpaper · p. 3734 · "The rows of a part are further logically divided into groups of 8192 records, called granules."
- **c92** · duck_rg · line 23 · "#define DEFAULT_ROW_GROUP_SIZE 122880ULL"
- **c93** · sqlservercs · p. 1741, §2.1.1 · "of rows is divided into row groups of about one million rows each."
- **c94** · colibri · p. 3294, §4.1 · "One data block file contains up to 262 144 records"
- **c102** · datablocks · p. 10, §5 · "if the number of compressed records in a block becomes too small, the overhead of block metadata worsens the compression ratio"

**Implies here.** BtrBlocks picks a cascade of schemes by sampling, recursively to a depth it
takes as a parameter (c87); here that is a loop with a counted depth (c10). FastLanes is written
as scalar code the compiler vectorizes (c90), the only kind of SIMD Amphora can have without
`unsafe` (c11). The unit others group rows in runs from 8,192 to about a million (c91 to c94):
the literature doesn't agree. Too small a block spends its bytes on metadata (c102); the right
size depends on the data and the machine, so it is learned where the copy runs, not taken from any
of them. The same holds for when a row is cold: SQL Server's documentation has the user set the
delay to the time rows were seen to stay hot (c99), and Colibri found the best threshold moves with
the memory it has, by 2x in scan speed across the thresholds it tried (c100, c101).
