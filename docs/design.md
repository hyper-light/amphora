# Amphora's design: every T1 decision

Track T1's decisions (AC-T1.2.3), in one index. The target they serve, and the areas AC-T1.2.3
names with their state, are in [docs/design/README.md](design/README.md). Each decision is recorded
in `docs/design/<area>.md` with its sources, its evidence and what would falsify it (CLAUDE.md §5);
the sources' notes are in `docs/research/`.

`crates/amphora/tests/design_index.rs` holds this file to the records: every level-two section of a
record is a decision with exactly one line in the index below, or is named under "Sections that are
not decisions"; a record without level-two sections is one decision, indexed by its file; and every
line here points at a record and a section that exist, under the area and number they carry.

## Adding a decision

Write it in `docs/design/<area>.md` as a level-two section (`## N. What it decides` where the
record numbers its decisions, `## Decision (lane L): what it decides` where it doesn't), then add
its one line to the index: area, number (`–` where the record doesn't number it), the decision in
one line, and a link to the section, `design/<area>.md#<anchor>`. The anchor is the heading as
GitHub makes it: lower case, punctuation other than `-` and `_` dropped, spaces as `-`, and `-1`,
`-2` after a repeated heading. `cargo test -p amphora --test design_index` names whatever is
missing or points nowhere. A section of a record that decides nothing (its evidence, its
comparison) is one line under "Sections that are not decisions" instead.

## The client protocol, researched

How a program reaches each system's records, from their specifications (cited as `[pN]`, each a
quote in [docs/research/client-protocol.md](research/client-protocol.md)). Amphora's decision, embedded first as
SQLite is [p13] with one framed protocol between processes [p22] [p23], is
[docs/design/client-protocol.md](design/client-protocol.md).

| | PostgreSQL | MySQL | SQLite | Hrana (libSQL) | Amphora |
|---|---|---|---|---|---|
| Where the engine runs | A server process of its own, as most engines are [p12] | A server process of its own [p12] | In the program's own process, reading and writing the files itself [p13] | A server reached over the network [p16] | In the program's own process; a second process asks the one that owns it [p22] |
| The frame | A type byte, then the length of the rest [p1] | Packets of at most 16 MByte, each with a sequence id [p8] [p9] | None: calls [p14] [p15] | JSON or Protobuf messages over WebSocket or HTTP [p18] [p21] | A big-endian length, then a postcard envelope whose variants only grow [p23] |
| A statement | A query string, or parse, bind and execute as separate steps [p2] [p3] | A text query, or a prepared statement the server names by an id [p10]; binary rows mark NULLs in a bitmap [p11] | Compiled to byte-code, then stepped row by row [p14] [p15] | SQL text on a stream, each stream one SQLite connection [p17] | One atomic batch a call, or a closure run once inside the writer [p28] [p26] |
| Many statements in one round trip | Pipelining: after an error, the server skips to the next Sync [p4] [p5] | Not in the pages read | No round trips [p13] | Batches whose steps run on conditions: a transaction in one round trip [p19] | The batch is one call [p28] |
| A session | A connection; a cancel comes on a new one [p7] | A connection; its sequence ids restart with each command [p9] | The program's own connection object [p13] | Over HTTP, a baton like a session cookie [p20] | An authenticated local connection; the owner proves itself first [p24] |
| A commit whose answer is lost | A message's statements commit or roll back together; a lost answer isn't named [p6] | Not named in the pages read | Can't happen: no link [p13] | Not named in the pages read | An unknown outcome, typed, never replayed [p27] |
| Records: compare-and-set, waits, the feed | As SQL statements [p2] | As SQL statements [p10] | As SQL statements [p14] | As SQL statements [p17] | Typed frames beside SQL [p25] |

## The decisions

| Area | No. | Decision | Record |
|---|---|---|---|
| audit | – | Decision (lane audit-log): the audit log is RFC 6962's Merkle tree over Records, verified on every read | [audit.md](design/audit.md#decision-lane-audit-log-the-audit-log-is-rfc-6962s-merkle-tree-over-records-verified-on-every-read) |
| client-protocol | 1 | Embedded first, one framed protocol between processes, no network listener yet (proposed 2026-11-21) | [client-protocol.md](design/client-protocol.md#1-embedded-first-one-framed-protocol-between-processes-no-network-listener-yet-proposed-2026-11-21) |
| columnar | 1 | Columns as a copy derived from the rows, kept from the commit stream, exact at every snapshot, every size learned where it runs | [columnar.md](design/columnar.md#1-columns-as-a-copy-derived-from-the-rows-kept-from-the-commit-stream-exact-at-every-snapshot-every-size-learned-where-it-runs) |
| engine | 1 | In-place versions in one shared buffer pool | [engine.md](design/engine.md#1-in-place-versions-in-one-shared-buffer-pool) |
| engine | 2 | The buffer pool: fixed frames, a latch each, CLOCK, a bound | [engine.md](design/engine.md#2-the-buffer-pool-fixed-frames-a-latch-each-clock-a-bound) |
| engine | 3 | The B-tree: right links, leaf latches, inner pages read without writing | [engine.md](design/engine.md#3-the-b-tree-right-links-leaf-latches-inner-pages-read-without-writing) |
| engine | 4 | Versions: undo records, commit timestamps, a low watermark | [engine.md](design/engine.md#4-versions-undo-records-commit-timestamps-a-low-watermark) |
| engine | 5 | Serializable by precision locking | [engine.md](design/engine.md#5-serializable-by-precision-locking) |
| engine | 6 | Durability: a row's change logged once, at commit | [engine.md](design/engine.md#6-durability-a-rows-change-logged-once-at-commit) |
| engine | 7 | Checkpoints: dirty pages copied in a pause, and the rows to correct | [engine.md](design/engine.md#7-checkpoints-dirty-pages-copied-in-a-pause-and-the-rows-to-correct) |
| engine | 8 | Recovery: the file's pages, the last journal, its list, then the redo after T | [engine.md](design/engine.md#8-recovery-the-files-pages-the-last-journal-its-list-then-the-redo-after-t) |
| engine | 9 | Beside the engine it replaces, behind the cursor the SQL layer already uses | [engine.md](design/engine.md#9-beside-the-engine-it-replaces-behind-the-cursor-the-sql-layer-already-uses) |
| engine | 10 | Memory: versions pruned between snapshots, one budget, typed refusals | [engine.md](design/engine.md#10-memory-versions-pruned-between-snapshots-one-budget-typed-refusals) |
| engine | 11 | The log in lanes: one group-committed log, or a log a writer | [engine.md](design/engine.md#11-the-log-in-lanes-one-group-committed-log-or-a-log-a-writer) |
| engine | 12 | Large rows as chunk rows of an overflow tree; trees made and dropped as records of their own | [engine.md](design/engine.md#12-large-rows-as-chunk-rows-of-an-overflow-tree-trees-made-and-dropped-as-records-of-their-own) |
| engine | 13 | The switch: a connection's B-trees on the versioned storage, the pager beside it | [engine.md](design/engine.md#13-the-switch-a-connections-b-trees-on-the-versioned-storage-the-pager-beside-it) |
| engine | 14 | Eviction: CLOCK to a spill file, slots by page number, reads verified once, a typed bound | [engine.md](design/engine.md#14-eviction-clock-to-a-spill-file-slots-by-page-number-reads-verified-once-a-typed-bound) |
| executor | 1 | A statement keeps its small ephemeral tables from one run to the next | [executor.md](design/executor.md#1-a-statement-keeps-its-small-ephemeral-tables-from-one-run-to-the-next) |
| executor | 2 | `IN (SELECT ...)` answers a row with one search of its list | [executor.md](design/executor.md#2-in-select--answers-a-row-with-one-search-of-its-list) |
| executor | 3 | LIKE jumps to where the literal after a `%` can match | [executor.md](design/executor.md#3-like-jumps-to-where-the-literal-after-a--can-match) |
| executor | 4 | GROUP BY aggregates each row into its group in a hash table | [executor.md](design/executor.md#4-group-by-aggregates-each-row-into-its-group-in-a-hash-table) |
| executor | 5 | An ORDER BY with a LIMIT keeps only the rows its reader takes | [executor.md](design/executor.md#5-an-order-by-with-a-limit-keeps-only-the-rows-its-reader-takes) |
| executor | 6 | A subquery runs where it is read | [executor.md](design/executor.md#6-a-subquery-runs-where-it-is-read) |
| executor | 7 | A row's header is parsed once, a cache hit takes no exclusive lock, and the index's terms run before the row is fetched | [executor.md](design/executor.md#7-a-rows-header-is-parsed-once-a-cache-hit-takes-no-exclusive-lock-and-the-indexs-terms-run-before-the-row-is-fetched) |
| executor | 8 | A scan, its aggregates and a hash join's probe run on every core, in morsels | [executor.md](design/executor.md#8-a-scan-its-aggregates-and-a-hash-joins-probe-run-on-every-core-in-morsels) |
| executor | 9 | A table scan decodes each leaf once into column batches, behind a switch | [executor.md](design/executor.md#9-a-table-scan-decodes-each-leaf-once-into-column-batches-behind-a-switch) |
| executor | 10 | A scan loop's filters and aggregates run over a whole batch by selection | [executor.md](design/executor.md#10-a-scan-loops-filters-and-aggregates-run-over-a-whole-batch-by-selection) |
| local-owner-transport | – | Versions (2026-11-06) | [local-owner-transport.md](design/local-owner-transport.md#versions-2026-11-06) |
| local-owner-transport | – | The tests' budgets (2026-10-31) | [local-owner-transport.md](design/local-owner-transport.md#the-tests-budgets-2026-10-31) |
| local-owner-transport | – | Owner guards (2026-11-06) | [local-owner-transport.md](design/local-owner-transport.md#owner-guards-2026-11-06) |
| local-owner-transport | – | Decision (lane constants): one descriptor budget (2026-11-21) | [local-owner-transport.md](design/local-owner-transport.md#decision-lane-constants-one-descriptor-budget-2026-11-21) |
| online-schema | – | Decision (lane online-schema): an index is built absent, delete-only, write-only, public | [online-schema.md](design/online-schema.md#decision-lane-online-schema-an-index-is-built-absent-delete-only-write-only-public) |
| online-schema | – | Decision (lane online-schema-5): a column is added with its states run together | [online-schema.md](design/online-schema.md#decision-lane-online-schema-5-a-column-is-added-with-its-states-run-together) |
| online-schema | – | Decision (lane online-schema-6): a constraint is added write-only, validated, then public | [online-schema.md](design/online-schema.md#decision-lane-online-schema-6-a-constraint-is-added-write-only-validated-then-public) |
| online-schema | – | Decision (lane online-schema-7): a column is dropped as a hidden slot, cleared in batches | [online-schema.md](design/online-schema.md#decision-lane-online-schema-7-a-column-is-dropped-as-a-hidden-slot-cleared-in-batches) |
| planner | 1 | A closed range is half a percent of its table, whether it seeks or filters | [planner.md](design/planner.md#1-a-closed-range-is-half-a-percent-of-its-table-whether-it-seeks-or-filters) |
| planner | 2 | The sort is charged to the candidate that needs it, on the rows it sorts | [planner.md](design/planner.md#2-the-sort-is-charged-to-the-candidate-that-needs-it-on-the-rows-it-sorts) |
| planner | 3 | A GLOB's literal prefix is a range of the index; a BLOB matches no GLOB or LIKE | [planner.md](design/planner.md#3-a-globs-literal-prefix-is-a-range-of-the-index-a-blob-matches-no-glob-or-like) |
| planner | 4 | A constraint on an index's second column skip-scans it | [planner.md](design/planner.md#4-a-constraint-on-an-indexs-second-column-skip-scans-it) |
| planner | 5 | A table lookup reads a page unless the connection's cache holds it | [planner.md](design/planner.md#5-a-table-lookup-reads-a-page-unless-the-connections-cache-holds-it) |
| planner | 6 | Many searches of an index the cache holds read its pages once | [planner.md](design/planner.md#6-many-searches-of-an-index-the-cache-holds-read-its-pages-once) |
| planner | 7 | A GROUP BY that groups in a hash table costs a probe a row, not a sort | [planner.md](design/planner.md#7-a-group-by-that-groups-in-a-hash-table-costs-a-probe-a-row-not-a-sort) |
| planner | 8 | Without statistics, a primary key's column is unique only as the whole key | [planner.md](design/planner.md#8-without-statistics-a-primary-keys-column-is-unique-only-as-the-whole-key) |
| planner | 9 | A correlated subquery that runs in the output is priced at the output's rows | [planner.md](design/planner.md#9-a-correlated-subquery-that-runs-in-the-output-is-priced-at-the-outputs-rows) |
| planner | 10 | A loop that gives at most one row orders its table and keeps the order before it | [planner.md](design/planner.md#10-a-loop-that-gives-at-most-one-row-orders-its-table-and-keeps-the-order-before-it) |
| planner | 11 | Without statistics, a unique key is searched rather than hashed | [planner.md](design/planner.md#11-without-statistics-a-unique-key-is-searched-rather-than-hashed) |
| planner | 12 | A statement counts its rows at each table access and join when asked, beside the estimates | [planner.md](design/planner.md#12-a-statement-counts-its-rows-at-each-table-access-and-join-when-asked-beside-the-estimates) |
| planner | 13 | One estimate per joined set | [planner.md](design/planner.md#13-one-estimate-per-joined-set) |
| planner | 14 | A row-value range seeks the index its columns lead | [planner.md](design/planner.md#14-a-row-value-range-seeks-the-index-its-columns-lead) |
| planner | 15 | A WITHOUT ROWID table's primary key is the table: ordered, covering, read through its cursor | [planner.md](design/planner.md#15-a-without-rowid-tables-primary-key-is-the-table-ordered-covering-read-through-its-cursor) |
| planner | 16 | An ordered read under a literal LIMIT stops at its rows | [planner.md](design/planner.md#16-an-ordered-read-under-a-literal-limit-stops-at-its-rows) |
| planner | 17 | An ordered read under a LIMIT known only as it runs stops at the q-middle of its rows | [planner.md](design/planner.md#17-an-ordered-read-under-a-limit-known-only-as-it-runs-stops-at-the-q-middle-of-its-rows) |
| planner | 18 | A join's rows divide once for each column its equivalence classes join | [planner.md](design/planner.md#18-a-joins-rows-divide-once-for-each-column-its-equivalence-classes-join) |
| planner | 19 | Joins searched over connected subsets within a counted budget; past it, the fewest rows first | [planner.md](design/planner.md#19-joins-searched-over-connected-subsets-within-a-counted-budget-past-it-the-fewest-rows-first) |
| planner | 20 | ANALYZE keeps each column's distinct count and a sample of each table | [planner.md](design/planner.md#20-analyze-keeps-each-columns-distinct-count-and-a-sample-of-each-table) |
| planner | 21 | A table's filters are estimated together on its sample, and otherwise combine by backoff | [planner.md](design/planner.md#21-a-tables-filters-are-estimated-together-on-its-sample-and-otherwise-combine-by-backoff) |
| planner | 22 | Correlated subqueries unnested by dependent joins taken apart, from a worklist (PL.10) | [planner.md](design/planner.md#22-correlated-subqueries-unnested-by-dependent-joins-taken-apart-from-a-worklist-pl10) |
| planner | – | Decision (lane pl6): No table read whole beneath loops that can give more than one row | [planner.md](design/planner.md#decision-lane-pl6-no-table-read-whole-beneath-loops-that-can-give-more-than-one-row) |
| planner | – | Decision (lane pl8): a RIGHT JOIN led by one table is planned as its mirrored LEFT JOIN | [planner.md](design/planner.md#decision-lane-pl8-a-right-join-led-by-one-table-is-planned-as-its-mirrored-left-join) |
| planner | – | Decision (lane pl8-3): a LEFT JOIN's right table follows the tables its ON terms read | [planner.md](design/planner.md#decision-lane-pl8-3-a-left-joins-right-table-follows-the-tables-its-on-terms-read) |
| planner | – | Decision (lane pl8-4): an outer join's ON terms on an earlier table count given its filters | [planner.md](design/planner.md#decision-lane-pl8-4-an-outer-joins-on-terms-on-an-earlier-table-count-given-its-filters) |
| planner | – | Decision (lane pl12): custom plans, then a generic one, for prepared statements | [planner.md](design/planner.md#decision-lane-pl12-custom-plans-then-a-generic-one-for-prepared-statements) |
| planner | – | Decision (lane pl9): row counts kept as rows change, statements planned again past their statistics' error | [planner.md](design/planner.md#decision-lane-pl9-row-counts-kept-as-rows-change-statements-planned-again-past-their-statistics-error) |
| planner | – | Decision (lane pl13): a nested loop that turns into a hash join at the learned costs' break-even, and a plan made again past its estimate's error | [planner.md](design/planner.md#decision-lane-pl13-a-nested-loop-that-turns-into-a-hash-join-at-the-learned-costs-break-even-and-a-plan-made-again-past-its-estimates-error) |
| planner | – | Decision (lane pl11): degree-sequence upper bounds guard each joined set's estimate | [planner.md](design/planner.md#decision-lane-pl11-degree-sequence-upper-bounds-guard-each-joined-sets-estimate) |
| planner | – | Decision (lane pl7): cost weights fitted to measured statement times, behind a switch | [planner.md](design/planner.md#decision-lane-pl7-cost-weights-fitted-to-measured-statement-times-behind-a-switch) |
| records-compaction | – | Durable progress and retention | [records-compaction.md](design/records-compaction.md#durable-progress-and-retention) |
| records-compaction | – | Dirty-page and serialized bounds | [records-compaction.md](design/records-compaction.md#dirty-page-and-serialized-bounds) |
| records-compaction | – | Embedded owner compatibility | [records-compaction.md](design/records-compaction.md#embedded-owner-compatibility) |
| records | – | Decision (lane records-api): the feed is read from the versions the commits wrote | [records.md](design/records.md#decision-lane-records-api-the-feed-is-read-from-the-versions-the-commits-wrote) |
| records | – | Decision (lane records-api): the simulator keeps Amphora's layout | [records.md](design/records.md#decision-lane-records-api-the-simulator-keeps-amphoras-layout) |
| records | – | Decision (lane records-api): one suite, ported into amphora-api | [records.md](design/records.md#decision-lane-records-api-one-suite-ported-into-amphora-api) |
| records | – | Decision (lane records-api): index families and counters are rows of ordinary families | [records.md](design/records.md#decision-lane-records-api-index-families-and-counters-are-rows-of-ordinary-families) |
| replicated-baseline-import | – | Importing a local baseline into the product group (U15 / CP-T1.8) | [replicated-baseline-import.md](design/replicated-baseline-import.md) |
| replication | – | The owner's time (2026-10-31) | [replication.md](design/replication.md#the-owners-time-2026-10-31) |
| replication | – | Membership (2026-11-03) | [replication.md](design/replication.md#membership-2026-11-03) |
| replication | – | A forward that meets an election (2026-11-06) | [replication.md](design/replication.md#a-forward-that-meets-an-election-2026-11-06) |
| replication | – | Fast acknowledgement: the replicated class (approved 2026-11-01, being built) | [replication.md](design/replication.md#fast-acknowledgement-the-replicated-class-approved-2026-11-01-being-built) |
| replication | – | The leader's quorum patience (2026-11-06) | [replication.md](design/replication.md#the-leaders-quorum-patience-2026-11-06) |
| replication | – | The application floor, replicated (proposed 2026-11-01; a to d decided and agreed 2026-11-06) | [replication.md](design/replication.md#the-application-floor-replicated-proposed-2026-11-01-a-to-d-decided-and-agreed-2026-11-06) |
| replication | – | A local read on a replica (2026-11-06) | [replication.md](design/replication.md#a-local-read-on-a-replica-2026-11-06) |
| replication | – | The class stated once per group (proposed 2026-11-09, Amphora goal 11209e8d) | [replication.md](design/replication.md#the-class-stated-once-per-group-proposed-2026-11-09-amphora-goal-11209e8d) |
| replication | – | A replica's local readers bounded by derivation (proposed 2026-11-09, Amphora goal 314d704a) | [replication.md](design/replication.md#a-replicas-local-readers-bounded-by-derivation-proposed-2026-11-09-amphora-goal-314d704a) |
| replication | – | A replica's database log: copied, bounded, and flushed off the owner (2026-11-09) | [replication.md](design/replication.md#a-replicas-database-log-copied-bounded-and-flushed-off-the-owner-2026-11-09) |
| replication | – | A replica's database log bounded by what it measures (2026-11-11) | [replication.md](design/replication.md#a-replicas-database-log-bounded-by-what-it-measures-2026-11-11) |
| replication | – | A follower's write under steady leader writes: the write slot (decided 2026-11-12, Amphora goal 9c63f7d4) | [replication.md](design/replication.md#a-followers-write-under-steady-leader-writes-the-write-slot-decided-2026-11-12-amphora-goal-9c63f7d4) |
| replication | – | Decision (lane follower-reads): Follower reads at a closed time (2026-11-21, G13 step 4, kiln goal 80d4cf5e) | [replication.md](design/replication.md#decision-lane-follower-reads-follower-reads-at-a-closed-time-2026-11-21-g13-step-4-kiln-goal-80d4cf5e) |
| replication | – | Decision (lane follower-reads): Search over partitions (2026-11-21, G13 step 4) | [replication.md](design/replication.md#decision-lane-follower-reads-search-over-partitions-2026-11-21-g13-step-4) |
| replication | – | Decision (lane constants): one drive a step (2026-11-21) | [replication.md](design/replication.md#decision-lane-constants-one-drive-a-step-2026-11-21) |
| replication | – | Decision (lane constants): the owner's wakes (2026-11-21) | [replication.md](design/replication.md#decision-lane-constants-the-owners-wakes-2026-11-21) |
| replication | – | Decision (lane constants): generations learned where they run (2026-11-21) | [replication.md](design/replication.md#decision-lane-constants-generations-learned-where-they-run-2026-11-21) |
| replication | – | Decision (lane constants): a generation of restarts ends once it covers its spend (2026-11-21) | [replication.md](design/replication.md#decision-lane-constants-a-generation-of-restarts-ends-once-it-covers-its-spend-2026-11-21) |
| replication | – | A follower's write that read nothing written since its base: read sets (built 2026-11-21, Amphora goal ceea4e0a) | [replication.md](design/replication.md#a-followers-write-that-read-nothing-written-since-its-base-read-sets-built-2026-11-21-amphora-goal-ceea4e0a) |
| replication | – | Decision (lane g12s2): the raft log as the WAL (2026-11-21, G12 step 2) | [replication.md](design/replication.md#decision-lane-g12s2-the-raft-log-as-the-wal-2026-11-21-g12-step-2) |
| replication | – | Decision (lane constants): the owner's tick pace (2026-11-21) | [replication.md](design/replication.md#decision-lane-constants-the-owners-tick-pace-2026-11-21) |
| replication | – | Decision (lane constants): the compaction's expansion learned where it runs (2026-11-21) | [replication.md](design/replication.md#decision-lane-constants-the-compactions-expansion-learned-where-it-runs-2026-11-21) |
| replication | – | Decision (lane constants): an envelope is what its encoding adds (2026-11-21) | [replication.md](design/replication.md#decision-lane-constants-an-envelope-is-what-its-encoding-adds-2026-11-21) |
| replication | – | Decision (lane constants): an image is bounded by the memory budget's room for one (2026-11-21) | [replication.md](design/replication.md#decision-lane-constants-an-image-is-bounded-by-the-memory-budgets-room-for-one-2026-11-21) |
| regions | – | Decision (lane regions): region-local groups under a global root, placed by declared survival | [regions.md](design/regions.md#decision-lane-regions-region-local-groups-under-a-global-root-placed-by-declared-survival) |
| reshard | – | Decision (lane split-merge): split, merge and move as one fenced hand-off, decided by measured share | [reshard.md](design/reshard.md#decision-lane-split-merge-split-merge-and-move-as-one-fenced-hand-off-decided-by-measured-share) |
| reshard | – | Decision (lane split-merge-7): batches learned where they run | [reshard.md](design/reshard.md#decision-lane-split-merge-7-batches-learned-where-they-run) |
| reshard | – | Decision (lane split-merge-7): retired groups kept | [reshard.md](design/reshard.md#decision-lane-split-merge-7-retired-groups-kept) |
| reshard | – | Decision (lane split-merge-7): a local database keeps its holding through its command | [reshard.md](design/reshard.md#decision-lane-split-merge-7-a-local-database-keeps-its-holding-through-its-command) |
| reshard | – | Decision (lane split-merge-8): a batch bounded by the process's memory | [reshard.md](design/reshard.md#decision-lane-split-merge-8-a-batch-bounded-by-the-processs-memory) |
| reshard | – | Decision (lane split-merge-8): a row routes by its namespace, family and key | [reshard.md](design/reshard.md#decision-lane-split-merge-8-a-row-routes-by-its-namespace-family-and-key) |
| reshard | – | Decision (lane split-merge-9): moves across nodes | [reshard.md](design/reshard.md#decision-lane-split-merge-9-moves-across-nodes) |
| semantics | 1 | TEXT is well-formed UTF-8 | [semantics.md](design/semantics.md#1-text-is-well-formed-utf-8) |
| semantics | 2 | atanh's last digit is its platform's C library's | [semantics.md](design/semantics.md#2-atanhs-last-digit-is-its-platforms-c-librarys) |
| store | 1 | A group commit with no timer | [store.md](design/store.md#1-a-group-commit-with-no-timer) |
| store | 2 | Durable beside the writer: a flusher | [store.md](design/store.md#2-durable-beside-the-writer-a-flusher) |
| store | 3 | Readers of their own, on the caller's thread, waiting only for their key's writes | [store.md](design/store.md#3-readers-of-their-own-on-the-callers-thread-waiting-only-for-their-keys-writes) |
| store | 4 | No checkpoint in a commit's way | [store.md](design/store.md#4-no-checkpoint-in-a-commits-way) |
| store | 5 | Page budgets by what a connection does | [store.md](design/store.md#5-page-budgets-by-what-a-connection-does) |
| store | 6 | One flusher | [store.md](design/store.md#6-one-flusher) |
| store | 7 | The writer keeps no statement journal; a commit that fails is retried batch by batch | [store.md](design/store.md#7-the-writer-keeps-no-statement-journal-a-commit-that-fails-is-retried-batch-by-batch) |
| store | 8 | Durable by ticket | [store.md](design/store.md#8-durable-by-ticket) |
| store | 9 | A failed flush is the last | [store.md](design/store.md#9-a-failed-flush-is-the-last) |
| store | 10 | Groups: writes on as many cores | [store.md](design/store.md#10-groups-writes-on-as-many-cores) |
| store | 11 | Told when durable | [store.md](design/store.md#11-told-when-durable) |
| store | 12 | Written in place, every page checksummed | [store.md](design/store.md#12-written-in-place-every-page-checksummed) |
| store | 13 | The log flushed by its data, and kept at its length | [store.md](design/store.md#13-the-log-flushed-by-its-data-and-kept-at-its-length) |
| store | 14 | Gathering before a flush at low load | [store.md](design/store.md#14-gathering-before-a-flush-at-low-load) |
| store | 15 | The checkpoint copies what the flusher made durable | [store.md](design/store.md#15-the-checkpoint-copies-what-the-flusher-made-durable) |
| store | 16 | The log packed: a commit's pages compressed into records | [store.md](design/store.md#16-the-log-packed-a-commits-pages-compressed-into-records) |
| store | 17 | Only the bytes that changed: deltas in the packed log | [store.md](design/store.md#17-only-the-bytes-that-changed-deltas-in-the-packed-log) |
| store | 18 | A reader forgets only what was written since it last read | [store.md](design/store.md#18-a-reader-forgets-only-what-was-written-since-it-last-read) |
| store | 19 | The page cache evicts the oldest page not looked at again | [store.md](design/store.md#19-the-page-cache-evicts-the-oldest-page-not-looked-at-again) |
| store | 20 | Pages one connection read in, every connection reads from memory | [store.md](design/store.md#20-pages-one-connection-read-in-every-connection-reads-from-memory) |
| store | 21 | A page read records itself only when it must wait for the cache | [store.md](design/store.md#21-a-page-read-records-itself-only-when-it-must-wait-for-the-cache) |
| store | 22 | A shared page found in one lookup, and counted where it was found | [store.md](design/store.md#22-a-shared-page-found-in-one-lookup-and-counted-where-it-was-found) |
| store | 23 | A commit ends before its pages outgrow the writer's cache | [store.md](design/store.md#23-a-commit-ends-before-its-pages-outgrow-the-writers-cache) |
| store | 24 | The checkpointer lists the pages of the frames it copies | [store.md](design/store.md#24-the-checkpointer-lists-the-pages-of-the-frames-it-copies) |
| store | 25 | A checkpointer that copied everything durable waits for a flush | [store.md](design/store.md#25-a-checkpointer-that-copied-everything-durable-waits-for-a-flush) |
| store | 26 | One pool of pages a process's databases can share, a quarter of its memory | [store.md](design/store.md#26-one-pool-of-pages-a-processs-databases-can-share-a-quarter-of-its-memory) |
| store | 27 | A writer reads committed pages through the pool, and keeps none of what it reads | [store.md](design/store.md#27-a-writer-reads-committed-pages-through-the-pool-and-keeps-none-of-what-it-reads) |
| store | 28 | A reader's snapshot takes the log's generation without a lock; spin locks test first | [store.md](design/store.md#28-a-readers-snapshot-takes-the-logs-generation-without-a-lock-spin-locks-test-first) |
| store | 29 | A pool lookup reads under its shard's read lock and copies the page out | [store.md](design/store.md#29-a-pool-lookup-reads-under-its-shards-read-lock-and-copies-the-page-out) |
| store | – | Virtual filesystem registry ownership (AC-T1.1.1) | [store.md](design/store.md#virtual-filesystem-registry-ownership-ac-t111) |
| store | 30 | A transaction's spills write over the frames it already wrote | [store.md](design/store.md#30-a-transactions-spills-write-over-the-frames-it-already-wrote) |
| store | 31 | A restart cuts a log past twice its bound to nothing | [store.md](design/store.md#31-a-restart-cuts-a-log-past-twice-its-bound-to-nothing) |
| store | 32 | A batch that wrote nothing is durable as the last flush left it | [store.md](design/store.md#32-a-batch-that-wrote-nothing-is-durable-as-the-last-flush-left-it) |
| store | 33 | A query runs beside the writer, on a reader, and writes nothing | [store.md](design/store.md#33-a-query-runs-beside-the-writer-on-a-reader-and-writes-nothing) |
| store | 34 | A prepared statement is validated once and held between executes | [store.md](design/store.md#34-a-prepared-statement-is-validated-once-and-held-between-executes) |
| store | 35 | Small commits: the checkpoint copies a page once for many commits | [store.md](design/store.md#35-small-commits-the-checkpoint-copies-a-page-once-for-many-commits) |
| store | 36 | A validated text is marked on its statement-cache entry | [store.md](design/store.md#36-a-validated-text-is-marked-on-its-statement-cache-entry) |
| store | 37 | A commit logs its pages' changes: a log of both kinds of slot | [store.md](design/store.md#37-a-commit-logs-its-pages-changes-a-log-of-both-kinds-of-slot) |
| store | 38 | A page is searched by key heads once its searches have paid for them (prototype, behind a switch) | [store.md](design/store.md#38-a-page-is-searched-by-key-heads-once-its-searches-have-paid-for-them-prototype-behind-a-switch) |
| store | 39 | A page read past the cache reads its own frames: pages new to a chain are page frames, a chain's entries records of one slot | [store.md](design/store.md#39-a-page-read-past-the-cache-reads-its-own-frames-pages-new-to-a-chain-are-page-frames-a-chains-entries-records-of-one-slot) |
| store | 40 | A page read finds its frame and the log's header without the shared locks (prototype, behind a switch) | [store.md](design/store.md#40-a-page-read-finds-its-frame-and-the-logs-header-without-the-shared-locks-prototype-behind-a-switch) |
| store | 41 | Skipping the descent: an adaptive hash against a learned leaf locator (prototypes, behind a switch) | [store.md](design/store.md#41-skipping-the-descent-an-adaptive-hash-against-a-learned-leaf-locator-prototypes-behind-a-switch) |
| store | – | Decision (lane wal-read): a commit's records published at a lookup's cost, its deltas found a word at a time | [store.md](design/store.md#decision-lane-wal-read-a-commits-records-published-at-a-lookups-cost-its-deltas-found-a-word-at-a-time) |
| store | 42 | The page path SQLite takes: plain pins, the pool's buffer counted once, misses read in line, checksums once (prototype, behind a switch) | [store.md](design/store.md#42-the-page-path-sqlite-takes-plain-pins-the-pools-buffer-counted-once-misses-read-in-line-checksums-once-prototype-behind-a-switch) |
| store | 43 | Point readers write no line another reader writes: the reader taken from a slot of its own, shared pages read under a version check (prototype, behind a switch) | [store.md](design/store.md#43-point-readers-write-no-line-another-reader-writes-the-reader-taken-from-a-slot-of-its-own-shared-pages-read-under-a-version-check-prototype-behind-a-switch) |
| store | – | Decision (lane constants): one process memory budget (2026-11-21) | [store.md](design/store.md#decision-lane-constants-one-process-memory-budget-2026-11-21) |
| without-rowid | – | Native primary-root query access | [without-rowid.md](design/without-rowid.md#native-primary-root-query-access) |
| xgroup | – | The two protocols: two-phase commit and Parallel Commits behind a switch, two-phase locking without waiting | [xgroup.md](design/xgroup.md#the-two-protocols) |
| xgroup | – | Crashes: every decision lives in a group, a new incarnation fences the old and finishes its transactions | [xgroup.md](design/xgroup.md#crashes) |
| xgroup | – | On a group's database: each step one atomic commit in its group, resolution timeouts learned as TCP's | [xgroup.md](design/xgroup.md#on-a-groups-database) |
| local-owner-transport | – | Embedded local owner transport: one owner process, an authenticated local client for the rest | [local-owner-transport.md](design/local-owner-transport.md) |
| records-compaction | – | Bounded Records compaction on the namespace writer, step by recoverable step | [records-compaction.md](design/records-compaction.md) |
| without-rowid | – | WITHOUT ROWID tables keep their primary-key index as the table | [without-rowid.md](design/without-rowid.md) |
| backup | – | Decision (lane pitr): backups and point-in-time recovery to Mantle (CP-T1.12, G14 step 4) | [backup.md](design/backup.md) |
| backup | – | The folder store: keys as paths, create once by link(2), listing in key order | [backup.md](design/backup.md#the-folder-store) |
| backup | – | Restore-and-check: the newest whole second restored and checked, on the archive's cadence | [backup.md](design/backup.md#restore-and-check) |
| backup | – | The source's own rows at a quiet second: the shipping thread checks itself against a snapshot lined up with a second's stamps | [backup.md](design/backup.md#the-sources-own-rows-at-a-quiet-second) |
| backup | – | A base begins just after a restart, so the next base copies least | [backup.md](design/backup.md#a-base-begins-just-after-a-restart) |
| backup | – | One cut across groups: each group restored to the second, half-done transactions finished by their records | [backup.md](design/backup.md#one-cut-across-groups) |
| backup | – | Incremental bases: a new base keeps the latest's chunks the log left unchanged | [backup.md](design/backup.md#incremental-bases) |

## Sections that are not decisions

- columnar: What a design must fit ([columnar.md](design/columnar.md#what-a-design-must-fit))
- columnar: The six designs ([columnar.md](design/columnar.md#the-six-designs))
- columnar: The comparison ([columnar.md](design/columnar.md#the-comparison))
- columnar: What the next steps prototype and measure ([columnar.md](design/columnar.md#what-the-next-steps-prototype-and-measure))
- columnar: How the claims are checked ([columnar.md](design/columnar.md#how-the-claims-are-checked))
- engine: The evidence that asks for it ([engine.md](design/engine.md#the-evidence-that-asks-for-it))
- engine: Stages, each measured ([engine.md](design/engine.md#stages-each-measured))
- store: Crash injection (T-T1.1.1) ([store.md](design/store.md#crash-injection-t-t111))
- xgroup: Evidence so far ([xgroup.md](design/xgroup.md#evidence-so-far))
- xgroup: Metrics (proposed; names to agree with kiln's Observe owner) ([xgroup.md](design/xgroup.md#metrics-proposed-names-to-agree-with-kilns-observe-owner))
- xgroup: Falsified by ([xgroup.md](design/xgroup.md#falsified-by))
