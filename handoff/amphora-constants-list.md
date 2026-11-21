# amphora-constants: the picked constants in the batch (2026-11-11)

Scope: `git diff db991a0..490ea83` (the batch) and `521e4bc..db991a0` (main's T1 work since 2026-11-06), production code only.
Worktree ~/Projects/amphora-t1-constants, branch t1/constants at 490ea83. Lines are at 490ea83.
Classes: **P** a picked value to replace; **M** measured once on one host and baked in (Ada's rule: learn it at runtime);
**D** already follows from another bound but is written as a literal (make it a const expression of that bound);
**F** format, protocol or algorithm constant (one line, no change).

## Area 1: planner (crates/engine, PL.2 to PL.4)
| # | file:line | value | class | derived from |
|---|---|---|---|---|
| 1 | translate/optimizer/join.rs:1604 `JOIN_SEARCH_BUDGET` | 10,000 subsets | P (cited from Neumann & Radke, still their pick) | The search's own cost against what it can save: the DP goes on while the planning work spent (subsets built, each charged at the cost model's measured per-subset cost) stays below the greedy plan's estimated cost. No subset count. |
| 2 | translate/optimizer/cardinality.rs:36 `MEMO_SETS` | 1 << 14 | D | The search's own subsets: whatever #1 yields plus the tables plus 63² greedy sets; a const/runtime expression, not a literal. |
| 3 | translate/optimizer/sample_filter.rs:25 `MAX_OPS` | 64 ops a term | P | Drop as a separate bound: a term's operations are charged against the statement's step budget (#4), which already bounds them. |
| 4 | translate/optimizer/sample_filter.rs:30 `SAMPLE_STEPS` | 64 × 1,024 × 16 | P (the ×16 and the 64) | The statement's own terms once over each sample row: ops counted from the parsed statement (bounded by the SQL length limit in force) × the sample's rows. Exactly the work, no factor. |
| 5 | translate/optimizer/sample_filter.rs:450 `backoff` exponents | 1, 1/2, 1/4, 1/8, first 4 terms | P (SQL Server's published pick) | The sample measures the conjunction: below the hit floor, the joint selectivity's posterior on the sample (prior: the independence product, Beta-binomial update on the hits observed), no exponents. |
| 6 | cardinality.rs:957, :967 `(hits).max(0.5)` | 0.5 hits | F-ish (Jeffreys/continuity half-count) | Fold into #5's posterior ((hits + ½)/(n + 1) is Jeffreys' estimator), so it is stated as the estimator, not a floor. |
| 7 | column_stats.rs:41 `SAMPLE_ROWS` | 1,024 rows | P (the 3.1-point error target is picked) | Per table at ANALYZE: the rows needed to resolve the cost model's index/scan crossover selectivity for that table (from its row and page counts) within half the gap at the sample's standard error. |
| 8 | column_stats.rs:47 `SAMPLE_HITS` | 10 hits | P ("a third" relative error) | The same error requirement as #7: the hits at which the share's standard error is within the crossover's half-gap. |
| 9 | column_stats.rs:37 `SAMPLE_BYTES` | 1 MiB | P | Charged to the schema's memory budget in force (the process's page Account), the sample's rows × their measured record bytes. |
| 10 | column_stats.rs:43 `LOADED_SAMPLE_BYTES` | 64 × SAMPLE_BYTES | P (its own comment says "to be measured") | The same memory budget as #9: loaded samples are charged to it, refused past it. |
| 11 | column_stats.rs:22 `PRECISION` | 11 (2,048 registers) | P (Umbra's 1.5 KB, 2.3% picked) | Per column: the precision whose HLL error 1.04/√m is within the sample's error (#7), capped by the rows counted (no more registers than values). The sketch header already carries its precision, so the format holds. |
| 12 | translate/optimizer/constraints.rs:2848 `2.0 × sqrt(variance) + 0.5` | z = 2, ½ continuity | P (z) / F (½) | z from the number of common values tested (Bonferroni over them at a family error the planner already states), the ½ is the standard continuity correction. |
| 13 | column_stats.rs HLL `0.7213/(1+1.079/m)`, `2.5m`; FNV/fmix64 constants; `SKETCH_VERSION`, `SKETCH_HEADER`, `REST`, `GUARD` | — | F | Flajolet et al.'s published estimator and the hash's defined constants; the sketch encoding. |

## Area 2: the WAL and the packed log (crates/engine/storage, crates/amphora/src/store.rs)
| # | file:line | value | class | derived from |
|---|---|---|---|---|
| 14 | crates/amphora/src/store.rs:171 `copy_after` | 8,192 frames | M + P ("a 16th of the log's bound") | Learned from the log's own counts: copy when the frames waiting stop collapsing onto fewer distinct pages (the marginal frames-per-distinct-page the uncopied range shows falls to 1, so waiting longer saves no copy), bounded by the log's bound (amphora-log-bound's). |
| 15 | storage/packed.rs:130 `MOST_DELTAS` | 8 deltas in a row | P (2026-10-20, a known candidate) | Per page: write the whole image when the chain's delta bytes since the last image reach the image's own encoded bytes (a read then costs no more than the space the deltas saved). The reader's bound is the same rule over what it reads. |
| 16 | storage/packed.rs `MIXED_FORMAT`, `RECORD_HEAD`; `PACKED_CHAINS_FORMAT` 0x414d_5004, `MIXED_CHAINS_FORMAT` 0x414d_5005 (new, 2026-11-12); `EIGHT_DELTAS` 8 | — | F | Log format magic, the record slot's page-id tag; the new values mark generations whose chains end by bytes (older binaries refuse them by name); 8 is the old formats' own chain bound. |

## Area 3: the replicated owner (replicated.rs, replication.rs, class.rs, transport: to amphora-seam-owner-2)
| # | file:line | value | class | derived from |
|---|---|---|---|---|
| 17 | database/replicated.rs:645 `CHECKPOINT_EVERY` | 25 ms | M (fedora 2026-10-20, the standalone store's) | No period: a pass follows the flush whose log passes #14's learned threshold (event-driven on the flush). |
| 18 | database/replicated.rs:649 `FRAME_BYTES` | 4096 + 24 | F, but its 4096 should name the page size `validate_pages` enforces and the WAL frame header constant. |
| 19 | database/replicated.rs:192 `READERS` = `transport::CONNECTION_MAX` | 32 | D, on a P: CONNECTION_MAX (wire.rs:10, 2026-10-29) is picked | CONNECTION_MAX from the process's descriptor limit (RLIMIT_NOFILE) and memory budget in force; pre-dates the window: flagged, not in scope unless the Amphora agent says. |
| 20 | replication/class.rs:19 `ENTRY_BYTES` | 64 KiB | D | `replicated::MEMBERS` × a fate's postcard max size + prefix and framing: a const expression. |
| 21 | database/replicated.rs:47 `CHANGE_BYTES` | 1,024 | P ("policy") | The change message's own encoded size (postcard max size of a member change / hand-off). |
| 22 | database/replicated.rs:52 `FLOOR_SESSION` | 60 s | P ("policy") | The same lease the seed's attempt holds: name that one; and it in turn from the measured session/commit time (learned). Needs the seam owner's word. |
| 23 | database/patience.rs:17 `QUORUM_CEILING` | 165 ticks | M (adacachy p99.9 1,642.788 ms) | Learned: the window's own high quantile of the voter's gaps over the last generation; a ceiling from the measured record is what the window learns at runtime. |
| 24 | database/patience.rs:22 `GENERATION` | 3,000 ticks | M | Learned: the measured mean interval between gaps over the window. |
| 25 | database/patience.rs:25 `PEERS` | 64 | D | `replicated::MEMBERS` (the same 64): name it. |
| 26 | replication.rs `LOG_FRAMES` | 131,072 | — | amphora-log-bound's: left. |

## Area 4: local owner and misc (crates/amphora, main since 2026-11-06)
| # | file:line | value | class | derived from |
|---|---|---|---|---|
| 27 | database.rs:127 `DRAINED_MAX` | 4,096 | D, on a P | The owner table's bound, written twice as a literal 4096 (database.rs:725, remote.rs:360): one named const; that bound itself from the descriptor limit in force (each owner holds a lease file). |
| 28 | database.rs:130 `FIRST_PAUSE` | 50 µs | M (fedora 62.8 µs, rounded down) | Learned: this process's own measured drain windows (the running minimum/median of observed release times). |
| 29 | database.rs:132 `LONGEST_PAUSE` | 1 ms | P | The learned window (#28) at its high quantile. |
| 30 | database/application.rs:22 `NAME_BYTES` | 128 | P ("admission policy") | The family identifier's own grammar bound in amphora-api, if it has one; else a question for Ada. |
| 31 | meter.rs `FLUSH_BUCKETS` 20, `FIRST_FLUSH_BOUND_NS` 62,500 | — | F | The metric's bucket layout agreed with the Kiln Metrics agent: a dashboard format. |
| 32 | transport/wire.rs `VERSION` 6; class.rs `PREFIX`; table SQL | — | F | Protocol and format. |
| 33 | transport.rs `HANG` 120 s | — | test only | Out of scope (cfg(test)). |

Older than the window and not in this goal (flagged only): replicated.rs TICK 10 ms, POLL 1 ms, DRIVE_QUANTUM 32,
OUTGOING_MESSAGES 4,096, MESSAGE_BYTES +2 MiB; wire.rs COMMAND_MAX, WAIT_MAX, WATCH_*; store.rs commit_rows 512, readers 4;
packed.rs RECORD_BYTES 256 KiB; watch/import/restore 4,096s.

Order of work: Area 2 (#14, #15) and Area 1 (#1 to #12) first (mine alone), then Area 4, then Area 3 with the seam owner's review.

## Added 2026-11-11 from amphora-log-bound (01a1ce73d696761cb37454c75d29910e)
| # | file:line | value | class | derived from |
|---|---|---|---|---|
| 34 | crates/amphora/src/store.rs Config::log_frames; Db::keep_log 2× | 131,072 frames; 2× | P (the better of two candidates, fedora 2026-10-20) | log-bound's rule for the replica (restarts spaced T = R/(1-q), R the measured restart time, q from AC-T1.3.4; the disk bound from the filesystem's free space) applied to the store. Older than the window. I take it only after log-bound's design is agreed, so both use one rule. |
| 35 | store.rs restart threshold `(bound / 64).max(1)` | 1/64 | P | The same rule as #34. |
| 36 | replication.rs `SNAPSHOT_BYTES` | 256 MiB | P ("until streaming snapshots") | The database's own size in force when the image is taken (its page count × page size), refused against the filesystem's free space. log-bound's disk reserve then takes that value. |
| 37 | database/replicated.rs:32 `MEMBERS` | 64 | P (seam owner, 2026-11-11) | Bounds ENTRY_BYTES, PEERS and hyper-raft's limits.members; with (d)'s budget, or from the measured cost of a member's progress tracking. Open. |
| 38 | crates/api/src/lib.rs `FAMILY_MAX` | 64 bytes | P (older; it is what bounds application names, NAME_BYTES 128 was dead behind it, removed) | Where a family is a key column: the largest index-cell payload local at the page size in force (sqlite.org/fileformat2.html §1.6, X = ((U-12)*64/255)-23, U = 4096 - 8 reserved = 999 bytes) less the key's other columns. The api crate has no page size, so it needs the seam's word. |
| 39 | crates/api/src/replication.rs / replication.rs `SNAPSHOT_BYTES` | 256 MiB "until streaming snapshots" | P | Seam owner's ruling (c): a memory bound, from the process memory budget's room for one image, once a learned budget exists (its own goal). Stated as picked until then. |

## Pre-window values (in scope since the Amphora agent's word, 2026-11-12: "Hold main for them too")
Lines at 6cb6168. P = picked, M = measured once, C = cited but not derived, F = format or protocol (one line, no change).

### Area 5: outside replicated.rs, replication.rs and the transport (now)
| file:line | value | class | derived from |
|---|---|---|---|
| db.rs:28 LOCK_WAIT | 15 s | P | The caller's own deadline, passed down; no wait of its own. |
| db.rs:31-32 BUSY_WAIT, BUSY_TRIES | 1 ms × 10,000 | P | Wait on the lock's release (the engine's busy handler told by the holder), or the backoff J1 now uses (probe cost, doubling, the deadline). |
| db.rs:37-38 WRITER_PAGES, READER_PAGES | 1,024 / 2,000 pages | P | Each connection's cache charged to the process page budget (Account), sized from it and the connections open. |
| db.rs:48 MEMORY_SHARE_DIVISOR | ¼ | C (PostgreSQL's advice) | Learned: the pool's measured hit rate against the memory the process may use. Or Ada accepts the citation. |
| db.rs:51,53 UNKNOWN_MEMORY_PAGES, MIN_PROCESS_PAGES | 65,536 / 4,096 | P | Where cgroup/OS memory can't be read, a typed refusal naming the platform, or the OS's physical memory (sysconf/sysctl/GlobalMemoryStatusEx); the floor is one connection's working set. |
| db.rs:177-178 LOG_HEADER_BYTES, FRAME_HEADER_BYTES | 32 / 24 | F | SQLite's WAL format. |
| database.rs:57 LIVE_MAX | 10,000,000 | P | Read its use first. |
| database.rs:58 QUEUED_BYTES | 512 MiB | P | The process memory budget. |
| database.rs:124 OWNERS_MAX | 4,096 | P | (d): the descriptor budget. |
| compaction.rs:7-9 ENVELOPE, PAGE_ENTRY, MIN_ROOM | 81 / 4,103 / 4,184 | F | Encoded sizes; written as const expressions of what they encode. |
| import.rs:18-19, restore.rs:7 CURSOR_MAX, SOURCE_MAX | 4,096 / 1,024 | P | Read their use first: the result bound (RESULT_MAX) over a cursor's row size. |
| watch.rs:13 WAIT_MAX | 4,096 | P | As above. |
| memory.rs:9 V1_UNLIMITED | 1 << 62 | F | cgroup v1's "no limit" (PAGE_COUNTER_MAX rounded), cite the kernel. |
| meter.rs WAIT_BUCKETS 16, LONGEST 5, FLUSH_BUCKETS | — | F | Metric layouts agreed with Kiln Metrics. |
| store.rs:322 SHARDS | 64 | P | The CPUs available (available_parallelism) rounded to a power of two. |
| store.rs:326 ROOM_LOOK | 5 ms | P | Wait on the room's fact (a condvar the writer signals). |
| store.rs Config queue 4,096; commit_rows 512 (M, fedora); readers 4; checkpoint_every 25 ms (M); idle_every 1 s; flushers 1 | — | P/M | queue: the memory budget over a batch's bytes; commit_rows: the writer's cache over a row's measured pages; readers: CONNECTION_MAX's budget; checkpoint_every: event-driven (with log-bound's rule); idle_every: the engine's own next upkeep; flushers: the disk's measured parallelism. |
| engine packed.rs:132 MOST_PAGES | 1,024 | C | IOV_MAX, read from the platform (sysconf(_SC_IOV_MAX); Windows has no writev: a typed refusal or one write a record). |
| engine packed.rs:138 RECORD_BYTES | 256 KiB | P (4 LZ4 windows) | LZ4's 64 KiB window is the cited part; the 4 isn't. |
| engine packed.rs:617 DECODED_BUDGET | 4 MiB | P ("not yet measured") | The process page budget. |
| engine changes.rs:15-18 MOST, DROP | 65,536 / half | P | Read its use first. |
| engine shared_pages.rs:26 SHARDS | 64 | P | available_parallelism. |
| engine join.rs:75 MAX_MATERIALIZED_BUILD_ROWS, cost.rs:604 SKIP_SCAN_SEEK_FUDGE | 200,000 / 1.375 | ? | Turso's or Amphora's? To check; Turso's inherited tunings are a question for you. |

### Area 3: replicated.rs, replication.rs, the transport (after amphora-g-slot and amphora-log-bound land)
replicated.rs COMMANDS 64, QUEUED_BYTES/OUTGOING_BYTES 512 MiB, OUTGOING_MESSAGES 4,096, MEMBERS 64, WIRE_BYTES/MESSAGE_BYTES (+2 MiB framing), TICK 10 ms, POLL 1 ms, DRIVE_QUANTUM 32; replication.rs ENVELOPE_BYTES 1 MiB, SNAPSHOT_BYTES (row 39); bootstrap.rs FRAMING 1,024, IMAGE_BYTES; transport endpoint.rs RECORD_BYTES 34 (F), watch.rs REGISTRATION_MAX/PROBE_MAX 4,096, CHECK_INTERVAL 10 ms, REQUEST_MAX 2 s; wire.rs FRAME_MAX, COMMAND_MAX 4,096, CONNECTION_MAX 32, WAIT_MAX 8, WATCH_BATCH_MAX 64, WATCH_REPLY_MAX 64 KiB, SESSION_MAX 30 s, GREETING_MAX 2 s; FLOOR_SESSION (done, ac1fe63). Each needs its derivation and amphora-seam-owner-2's word.

### The API's limits (a question for Ada: they're the public contract kiln builds on)
crates/api/src/lib.rs FAMILY_MAX 64, KEY_MAX 1,024, VALUE_MAX 1 MiB, TX_MAX 1,000, SCAN_MAX 1,000, SQL_MAX 1 MiB, RESULT_MAX 32 MiB, REQUEST_MAX 32 MiB. Recommendation: derive what has a structural source (FAMILY_MAX and KEY_MAX from SQLite's local index-cell payload; VALUE_MAX from the overflow chain a value may take within the memory budget), and state the rest as the API's contract, changed append-only.
