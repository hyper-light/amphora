# PostgreSQL clients on Amphora (lane PG1, 2026-11-23)

Ada, 2026-11-23: "I'm not opposed to also supporting Postgres wire protocol. That just means we have
to do a lot of work to support Postgres as well (latest version). If you can manage that, then do
it." And her bar for it: "I expect order of magnitude better Postgres performance from us at all
scales."

Amphora stays SQLite underneath: SQLite's five value types, its storage, its engine. Supporting
PostgreSQL means a PostgreSQL client and its tools (psql, pgbench, libpq, JDBC, psycopg, pgx,
node-postgres, ORMs) work against Amphora as they would against the latest PostgreSQL: its wire
protocol, its SQL dialect, types and semantics, its catalog (`pg_catalog`, `information_schema`),
its error codes and its transaction behaviour. Never by trading correctness for speed: where
Amphora can't yet do what PostgreSQL does, it refuses with PostgreSQL's own SQLSTATE (`0A000`,
feature_not_supported) and says what is missing.

This record opens the network-listener choice that [client-protocol.md](client-protocol.md) left
open, for PostgreSQL's protocol only, and meets the condition that record set for it (section 2).
Lane P adds a SQLite-rows request to `amphora serve`'s own protocol for the bench; both front ends
share one admission and execution path (section 2) and touch different files.

The sources' words are in [docs/research/postgres.md](../research/postgres.md). PostgreSQL's source
is cited as `path:line` at tag `REL_19_0` (commit `5c839a2a0`, a clone at `~/Projects/postgres`);
pganalyze's libpg_query as `libpg_query/path:line` at branch `18-latest` (`7663e07`, at
`~/Projects/libpg_query`); Amphora's code as `crates/...:line` at `78fa9fdb`.

## 1. The target is PostgreSQL 19.0 and protocol 3.2, and it moves with each major release

**The decision.** The target is PostgreSQL 19.0, released 2026-11-19 (postgresql.org's versioning
page: "19 / 19.0 / Yes / November 19, 2026"). Its protocol is 3.2: `PG_PROTOCOL_LATEST` is
`PG_PROTOCOL(3,2)` (`src/include/libpq/pqcomm.h:95`), the earliest it serves is 3.0 (`:94`), and
3.2 arrived in PostgreSQL 18 with a cancel key "enlarged from 4 bytes to a variable length field"
(`doc/src/sgml/protocol.sgml`, table "Protocol versions"). libpq still asks for 3.0 unless told
otherwise (`src/interfaces/libpq/fe-connect.c:2145-2153`), so Amphora serves both, as PostgreSQL 19
does.

What 19 changed that a reimplementation must match: protocol greasing (version 3.9999 and the
option `_pq_.test_protocol_negotiation` are reserved, and a server must answer both with
NegotiateProtocolVersion and carry on, `pqcomm.h:115`, `protocol.sgml:274`, `:370`);
`standard_conforming_strings` always on (`doc/src/sgml/release-19.sgml:161-168`); new syntax
(`INSERT ... ON CONFLICT DO SELECT`, `REPACK`, `WAIT FOR LSN`, `IGNORE NULLS` on window functions,
`COPY ... TO` as JSON); new types and functions (`oid8`, `regdatabase`, casts between `bytea` and
`uuid`, `random(min, max)` over dates and timestamps, `base64url` and `base32hex` encodings); no
change to the `pg_catalog` tables, only to statistics views.

"Latest" moves: when PostgreSQL ships a new major release, the target becomes that release on the
day it ships. Its grammar and catalog data are regenerated from its tag (section 3), its regression
suite becomes the measure (section 10), and the difference between the two releases' suites is the
backlog.

**Falsified by.** A PostgreSQL release later than 19.0 on postgresql.org's versioning page, or a
protocol version above 3.2 in its `pqcomm.h`.

## 2. A PostgreSQL listener on Amphora's execution path, event-driven, one session a state machine

**The decision.** A PostgreSQL listener in the `amphora` binary, beside `amphora serve`'s, speaking
protocol 3.0 and 3.2 as PostgreSQL 19 does:

- **Startup.** SSLRequest (80877103) answered `S` or `N`, GSSENCRequest (80877104) answered `N`;
  direct TLS (libpq's `sslnegotiation=direct`) only with ALPN `postgresql` (`pqcomm.h:189`),
  refused without it as PostgreSQL refuses it (`src/backend/tcop/backend_startup.c:472-476`).
  StartupMessage at 3.0 or 3.2; a newer minor or any `_pq_.` option answered with
  NegotiateProtocolVersion listing them (`backend_startup.c:834`, `:883`).
- **Authentication.** SCRAM-SHA-256 and SCRAM-SHA-256-PLUS (RFC 5802, RFC 7677), with channel
  binding `tls-server-end-point` (RFC 5929), the only type PostgreSQL uses
  (`src/backend/libpq/auth-scram.c:20-21`); the user name inside SCRAM ignored as PostgreSQL ignores
  it. Verifiers are kept as PostgreSQL keeps them, with its default 4096 iterations
  (`src/include/common/scram-common.h:50`, the floor RFC 7677 sets). OAUTHBEARER and MD5 are out of
  this record (Choices for Ada).
- **Session.** The 15 ParameterStatus values PostgreSQL 19 reports (`application_name`,
  `client_encoding`, `DateStyle`, `default_transaction_read_only`, `in_hot_standby`,
  `integer_datetimes`, `IntervalStyle`, `is_superuser`, `scram_iterations`, `search_path`,
  `server_encoding`, `server_version`, `session_authorization`, `standard_conforming_strings`,
  `TimeZone`; flagged `GUC_REPORT` in `src/backend/utils/misc/guc_parameters.dat`). BackendKeyData
  with a 4-byte key at 3.0 and PostgreSQL's 32 bytes at 3.2 (`MAX_CANCEL_KEY_LENGTH`,
  `src/include/storage/procsignal.h:63`, `src/backend/tcop/postgres.c:4364`). ReadyForQuery with
  `I`, `T` or `E`.
- **Queries.** The simple protocol (several statements in one Query run as one implicit
  transaction) and the extended protocol (Parse, Bind, Describe, Execute, Close, Flush, Sync; named
  and unnamed statements and portals; Execute with a row limit and PortalSuspended; after an error,
  every message skipped until Sync; pipelining). Text and binary formats, binary by each type's
  `typsend`/`typreceive` as `src/include/catalog/pg_type.dat` names them.
- **COPY.** CopyIn and CopyOut, text, CSV and binary, Flush and Sync ignored during copy-in as
  PostgreSQL ignores them. CopyBoth is replication's and out of this record.
- **CancelRequest** on a new connection, keys of 4 to 256 bytes under 3.2, no reply.
- **LISTEN/NOTIFY.** NotificationResponse only outside a transaction, delivered in commit order from
  Amphora's commit stream, never before the notifying transaction is durable.
- **Errors.** ErrorResponse and NoticeResponse with PostgreSQL's fields (`S`, `V`, `C`, `M`, `D`,
  `H`, `P`, ...), every refusal a SQLSTATE from `src/backend/utils/errcodes.txt`.

The protocol is a pure core (CLAUDE.md §9): bytes in, messages out, no socket and no clock. The
shell is event-driven on hyper-rt, as Amphora's rules require of new product code: a socket's
readiness, a commit, a lock's release or a notification wakes a session, and time enters only as
PostgreSQL's own timeouts (`statement_timeout`, `lock_timeout`, `idle_in_transaction_session_timeout`,
off unless set, as in PostgreSQL). A session is a state machine with its memory charged to the
process budget; a session refused for room is refused with `53300` (too_many_connections) or
`53200` (out_of_memory), typed, never dropped.

Execution is shared with lane P's SQLite-rows request and with `amphora serve`: requests are
admitted by `Admission` (`crates/amphora/src/database.rs:79`), which charges queues to the memory
budget and refuses `Full`; reads run on readers (`Database::query`, `database.rs:1624`), and a
session's transaction is a transaction of the versioned storage, committed through the group commit
(engine.md §4 and §5, store.md §1). `Database::call` (`database.rs:1544`) runs one closure once on
the writer; a PostgreSQL session holds its transaction across messages, so it needs a session-held
transaction API on `Database`. Whichever of lanes P and PG1 adds that API first owns it, and the
other uses it; neither edits the other's protocol files.

**The client-protocol record's condition.** client-protocol.md §1 left the network listener open,
to be "judged first on whether it can carry an unknown outcome typed". PostgreSQL carries it in its
own vocabulary: a client reads `pg_current_xact_id()` before COMMIT and, after a lost answer,
`pg_xact_status(xid)` reports `committed`, `aborted` or `in progress`; PostgreSQL's documentation
names exactly this use, "to determine whether their transaction committed or aborted after the
application and database server become disconnected while a COMMIT is in progress"
(`doc/src/sgml/func/func-info.sgml:2877`). Amphora answers both from its commit records, so a
PostgreSQL client can learn an unknown outcome without Amphora replaying anything.

**Against the 10x bar.** PostgreSQL runs "a process per user" (`doc/src/sgml/arch-dev.sgml:118`): a
connection costs a fork and a cold backend. An Amphora session is a state machine on an event loop
with one thread a core, so connection setup and 10,000 or more clients pay neither a process nor a
thread each. `amphora serve` today runs a thread a connection (`crates/amphora/src/serve.rs:557`);
the PostgreSQL listener doesn't copy that.

**Falsified by.** A PostgreSQL 19 client that connects to PostgreSQL 19 and fails to connect to
Amphora with the same options; or a measured case where a session waits on a timer rather than an
event.

## 3. The parser is PostgreSQL 19's own grammar and scanner tables, driven by flat Rust loops

**The decision.** Ada's direction: start from PostgreSQL 19's own parser and optimize from there.
Amphora runs PostgreSQL's own build tools on PostgreSQL's own files, as PostgreSQL builds itself:
bison on `src/backend/parser/gram.y` (20,077 lines; `%expect 0` at `gram.y:216`, so the grammar has
no conflicts) and flex on `scan.l` (1,421 lines) with PostgreSQL's flags `-CF -p -p` and no backing
up (`src/backend/parser/Makefile:60-61`, `meson.build:33`). A generator script lifts the numeric
tables bison and flex produce into checked-in Rust constants; flat Rust loops drive them (an LR
parse loop with explicit state and value stacks under a counted budget, and a DFA scan loop); each
grammar rule's action is ported to Rust and builds Amphora's arena parse tree bottom-up on reduce.
`parser.c`'s one-token lookahead filter (`NOT_LA`, `NULLS_LA`, `WITH_LA`, `WITHOUT_LA`, `FORMAT_LA`,
`UIDENT`/`USCONST`; `src/backend/parser/parser.c:93-254`, 527 lines) is ported as written. Bison
3.8.2 on 19's grammar reports 3,477 productions, 744 nonterminals, 546 terminals and 6,574 states.

**The options compared, on exact parity with 19 and on staying current.**

| | Exact parity with 19 | Staying current | Amphora's rules | Cost |
|---|---|---|---|---|
| A. libpg_query vendored (the base case) | Exact for the release it wraps; it wraps 18.6 (`libpg_query/Makefile:10`), and has no 19 branch | pganalyze's schedule: 47, 99, 1 and 237 days after PostgreSQL 15, 16, 17 and 18 (its `CHANGELOG.md`) | C behind FFI (`unsafe`); recursive output walks (`src/pg_query.c:269`); a mocked `proc_exit` that calls `exit(1)` (`scripts/extract_source.rb:540`) | Least to start |
| B. sqlparser-rs's PostgreSQL dialect | Not exact by its own account: it "accepts queries that specific databases would reject" (its README) | Community-driven, no parity target | Hand-written recursive descent | Low, but never exact |
| C. A Rust LR generator (grmtools, lalrpop) over gram.y | Presumed only: another table construction can resolve precedence and default reductions differently from bison | Regenerate, port changed actions | Fits | All 3,477 actions ported |
| D. PostgreSQL's own bison and flex tables, Rust drivers (chosen) | By construction for acceptance and for where an error is reported: the same tables from the same files with the same tools | Bump the pin, regenerate the tables, port only the rules that changed | Fits: no `unsafe`, no C, no recursion, every table read by `.get()` | All 3,477 actions ported once, then each release's diff |

libpg_query is how PostgreSQL's parser is usually taken out of PostgreSQL: libclang keeps the code
`raw_parser` reaches (`scripts/extract_source.rb:599`), 44 backend functions are mocked
(`:577`), and the Makefile applies 15 patches to the release tarball before generating the grammar
(`Makefile:124-146`). A parse through it creates and deletes a memory context
(`src/pg_query.c:112-133`), redirects stderr with `pipe`, `dup` and `dup2` in release builds
(`src/pg_query_parse.c:24-40`), enters `PG_TRY` (setjmp), runs `raw_parser`, walks the tree again to
JSON or protobuf (`pg_query_outfuncs_protobuf.c:252`), copies the result, and the Rust binding then
decodes the protobuf into Rust values (pg_query.rs `src/query.rs:51`). D keeps the one part of that
which is PostgreSQL's (the scan and the table-driven parse) and drops the rest: no context, no
syscall, no setjmp, no second walk, no encode and decode. Measured on 19's regression statements
(51,739 that 18.6's grammar parses), on a loaded machine and so inconclusive in absolute terms: the
raw parse took 1.2 to 2.4 µs a statement and parse with protobuf 6.5 to 11 µs, and in every run the
protobuf step cost 3 to 5 times the parse
([bench/records/2026-11-23-mbp2-perf-postgres.md](../../bench/records/2026-11-23-mbp2-perf-postgres.md)).
The Rust parser's bar is the raw parse, rerun idle beside it on the same statements.

**Kept exact, release after release.** The generator numbers every production and records the hash
of its C action text beside it; each Rust action names the hash it was ported from. A new rule, or a
rule whose action changed, fails the build (an exhaustive match over the rule enum, and a hash
mismatch), so nothing from a new release slips through unported. Until a statement kind's actions
are ported, the parser accepts exactly what 19 accepts and refuses the unported statement with
`0A000` naming it. The `ereport(ERROR` calls inside `gram.y`'s actions (89 of them) are ported with
their SQLSTATEs. The oracle, used only in tests, is PostgreSQL 19's own `raw_parser`, unpatched,
taken out of `REL_19_0` with libpg_query's extraction machinery (its libclang walk and its output
functions) but not its patches: those don't apply to 19 (patch 04 fails at `scan.l`), and several
change what is accepted (patches 01, 09 and 16 allow `$n` parameters in more places and remove 19's
"trailing junk after parameter" rule, so a patched oracle accepts `NOTIFY ch, $1` and `SELECT $1abc`,
which 19 refuses; patch 04 makes comments tokens; patch 15 moves node locations). Building that
oracle is a unit of its own (section 11). Every statement of 19's regression SQL, and a
grammar-driven fuzzer's output, must then give the oracle's verdict, error position and parse tree.

**Vendoring.** One entry in `vendor/MANIFEST.toml`: `postgres` at `REL_19_0` (`5c839a2a0`), the
files `src/backend/parser/{gram.y,scan.l,parser.c,scansup.c}`, `src/include/parser/kwlist.h`,
`src/include/nodes/{parsenodes.h,primnodes.h}`, `src/include/catalog/*.dat` (section 4),
`src/backend/catalog/information_schema.sql` (section 9), `src/backend/utils/errcodes.txt` and
`src/timezone/data/tzdata.zi` (section 5), taken by `scripts/vendor.py` with local changes recorded
in `vendor/patches/postgres/` (none planned). PostgreSQL's licence is permissive
(`COPYRIGHT:8`, "Permission to use, copy, modify, and distribute this software"); bison's output may
be used "in nonfree programs" (the Bison manual's Conditions page). The generator pins bison 3.8.2 and
flex and writes the tables to checked-in `.rs` files; `scripts/vendor.py check` proves the vendored
files are upstream's, and a doc-truth test proves the tables are the generator's output for them.

**Against the 10x bar.** A statement's parse is one scan and one table loop writing into an arena
reused across statements. Parsing is skipped entirely where the text was seen before: Amphora
already keeps prepared statements validated between executes (store.md §34, §36), and the extended
protocol parses a named statement once.

**Falsified by.** The differential corpus finding a parse tree that differs from the oracle's and
traces to the tables rather than a ported action; bison's tables differing across bison versions in
what they accept; or PostgreSQL moving off bison.

## 4. PostgreSQL's semantics in a front end that lowers into Amphora's planner

**The decision.** The front end is PostgreSQL's own pipeline up to the plan: parse (section 3), then
analysis against the catalog (section 9): names resolved through `search_path` (section 7), every
operator, function and cast resolved by PostgreSQL's rules (exact match; binary-coercible; unknown
literals take their partner's type; most exact matches; the preferred type of the category,
`typcategory` and `typispreferred`; implicit casts, `castcontext 'i'`; assignment casts on INSERT
and UPDATE), ambiguity refused with `42725`. Analysis gives a typed tree with every column's OID and
typmod, which lowers into the engine's existing planner and bytecode, so Amphora's join search,
statistics, learned weights, morsel scans and columnar copies all serve PostgreSQL's statements.

The engine already has the seam: a `Dialect` trait (`crates/engine/dialect/mod.rs:28`) that owns
parsing, stored table text (`POSTGRES_TABLE_SQL_PREFIX`, `:19`), catalog registration and function
resolution, with only `SqliteDialect` implemented (`dialect/sqlite.rs:35`). The PostgreSQL dialect
implements it. Two changes to the seam: `Dialect::parse` gains the session (search path and catalog
version), because re-preparing after a schema change (`crates/engine/vdbe/execute.rs:5016`) must
resolve names as the session would; and `Func` (`crates/engine/function.rs:1587`) gains
`Func::Pg(ProcOid)`, a dense table index, because `Func::Dialect(String)` (`:1603`) matches a string
on every call.

Casts, operators and functions come from PostgreSQL's own catalog data, vendored: `pg_type.dat`,
`pg_proc.dat`, `pg_operator.dat`, `pg_cast.dat`, `pg_amop.dat`, `pg_opclass.dat`, `pg_aggregate.dat`,
turned into static tables at build. Each `prosrc` (`int4pl`, `numeric_div`, ...) maps to a pure
Rust function written from PostgreSQL's C and raising its SQLSTATEs (`int4pl` overflow `22003`, never
a REAL; division by zero `22012`, never NULL; bad input `22P02`). A function with no Rust body yet
still appears in `pg_proc`; resolving to it refuses at analysis with `0A000` naming it, never at run
time. A doc-truth test counts the bodies against the rows.

**Against the 10x bar.** Arithmetic and comparison on the hot path lower to the engine's own
instructions (integer add with an overflow check that raises `22003`; comparisons of stored keys,
section 5), not function calls; the rest dispatch through `Func::Pg`'s table. Analysis and planning
run once per prepared statement, with Amphora's custom-then-generic plan rule (planner.md, lane
pl12), where PostgreSQL's simple protocol parses, analyzes and plans every statement anew.

**Falsified by.** A statement whose resolved operator, function, cast or result type differs from
PostgreSQL 19's on the same catalog (the regression suite shows it as a diff), or a hot statement
whose profile shows per-row string dispatch.

## 5. Every PostgreSQL value stored on SQLite's five types under a key the engine already orders

**The decision.** Each PostgreSQL type is stored on one of SQLite's five storage classes in a key
whose SQLite order (INTEGER numeric order, TEXT under BINARY, BLOB by memcmp) is exactly the type's
PostgreSQL btree order and equality. So B-tree indexes, ORDER BY, DISTINCT, GROUP BY, joins and
UNIQUE need no new comparator: the engine's record comparison (`crates/engine/types.rs:2703`) stays
as it is. What PostgreSQL shows but does not compare (a numeric's display scale, an interval's three
fields, a jsonb's text) is kept beside the key, from the column's typmod where that fixes it, else
in a hidden companion column (`ColDefFlags::Hidden`, `crates/engine/schema.rs:5671`) that analysis
never lets into a comparison. The table's PostgreSQL definition is the one copy, stored behind
`POSTGRES_TABLE_SQL_PREFIX`; the engine's table and `pg_attribute` both derive from it. PostgreSQL
columns carry no SQLite affinity, so SQLite never converts a value.

| Type (OID) | Stored as | The key, and why its order is PostgreSQL's |
|---|---|---|
| bool (16), int2 (21), int4 (23), int8 (20), oid (26), regclass (2205) | INTEGER | Native order; ranges checked on write (`22003`); reg* types print names through `search_path` |
| float4 (700), float8 (701) | REAL; NaN as one fixed BLOB | REAL compares -0 = 0 as PostgreSQL does; every NaN is one BLOB, and SQLite orders numbers before BLOBs, so NaN = NaN and NaN sorts above everything, as `float8_cmp_internal` does. The engine stores NaN as NULL (`types.rs:678`), so NaN never takes the REAL path |
| numeric (1700) | BLOB | An ascending decimal key with trailing zeros folded into the exponent (CockroachDB's layout), so 1.0 and 1.00 are one key; -Infinity, finite, +Infinity, NaN in PostgreSQL's order. Display scale from `numeric(p,s)`'s typmod, else a hidden INTEGER |
| text (25), varchar (1043), name (19) | TEXT | BINARY, which is PostgreSQL's order under the `C` and builtin `C.UTF-8` collations; lengths checked on write (`22001`) |
| bpchar (1042) | TEXT | Trailing spaces stripped, as bpchar compares; `char(n)` padded back to n on output, and a `bpchar` without a length keeps the text as written in a hidden column, since PostgreSQL shows its spaces |
| bytea (17), uuid (2950) | BLOB | memcmp, PostgreSQL's order for both |
| date (1082), time (1083), timestamp (1114), timestamptz (1184) | INTEGER | Days or microseconds from 2000-01-01, PostgreSQL's own epoch; plus and minus infinity as the integer extremes, as PostgreSQL stores them. timestamptz is UTC; the session's `TimeZone` applies on input and output, from the vendored tz database |
| timetz (1266) | BLOB | `time + zone` in microseconds (the zone in seconds west of UTC, not wrapped to a day), then the zone, exactly as `timetz_cmp_internal` computes them (`src/backend/utils/adt/date.c:2570`, `:2582-2585`) |
| interval (1186) | BLOB | The 128-bit span PostgreSQL compares (`interval_cmp_value`: a month is 30 days, a day 24 hours), so '1 mon' = '30 days'; the three fields in a hidden BLOB |
| json (114) | TEXT | The text as written; PostgreSQL gives json no btree order either |
| jsonb (3802) | BLOB | PostgreSQL's jsonb order (object > array > boolean > number > string > null, counts before members), with its one exception kept: a top-level empty array sorts below every top-level scalar (`src/backend/utils/adt/jsonb_util.c:248-262`, `doc/src/sgml/json.sgml:593`); strings escaped and terminated, numbers as numeric keys; the text in a hidden column |
| arrays (`_int4` 1007, `_text` 1009, ...) | BLOB | Element keys in order, NULL after every value as `array_cmp` orders it, then dimensions and lower bounds |
| enum | BLOB | A fractional key between its neighbours, so `ALTER TYPE ... ADD VALUE BEFORE` never renumbers; `pg_enum.enumsortorder` derived from it |

Every encoder and decoder is a flat loop with an explicit stack bounded by the input's length
(CLAUDE.md §11). Hostile input to every decoder is a test (CLAUDE.md §6).

**Why not a new comparator (the other candidate).** The other design kept exact bytes and taught
the record comparator PostgreSQL's operator classes, a `Pg(PgOpClass)` beside `CollationSeq`
(`crates/engine/translate/collate.rs:27`). It must then reach every place the engine orders or equates
values, and those are scattered (`types.rs:2703`, `vdbe/execute.rs:404`, `vdbe/hash_table.rs:85`,
min and max, IN lists, window partitions, statistics, columnar copies): one missed site is a silent
wrong answer. Keys the engine already orders can't be missed, and compare as integers or one memcmp,
which is also the faster path for the 10x bar. What the key design can't express is refused: a
nondeterministic (case-insensitive) collation and a user `CREATE OPERATOR CLASS` are `0A000` until a
key for them is designed; ICU collations are Ada's to bring in (Choices for Ada).

**Falsified by.** Any value pair whose order or equality in Amphora differs from PostgreSQL 19's
(a differential test over -0, NaN, 1.5 and 1.50, '1 mon' and '30 days', trailing spaces, enum values
added BEFORE, jsonb, arrays and NULLs, through ORDER BY, DISTINCT, GROUP BY, UNIQUE, joins, min and
max, and windows); or -0 lost in a record's round trip.

## 6. Sequences, serial and identity outside the caller's transaction, as PostgreSQL keeps them

**The decision.** PostgreSQL's `nextval` is not transactional: a rollback never returns a value,
another session sees the advance at once, and a crash may skip values. Amphora's sequences today are
transactional (`crates/engine/translate/sequence.rs`), which is wrong for PostgreSQL. A PostgreSQL
sequence is a row of its own tree, advanced in memory under the sequence's latch and made durable
ahead of use by its own commit through the group commit, `SEQ_LOG_VALS` (32) values at a time as
PostgreSQL does (`src/backend/commands/sequence.c:58`), so a crash skips at most what PostgreSQL's
would. `nextval` records no read and so never causes `40001`. `serial` is int4 plus an owned
sequence plus `DEFAULT nextval('t_c_seq'::regclass)`; identity columns keep `attidentity` `a` or `d`
and refuse an explicit value into `GENERATED ALWAYS` with `428C9` unless `OVERRIDING SYSTEM VALUE`.

**Against the 10x bar.** One durable write per 32 values, shared with other commits by the group
commit, as PostgreSQL's.

**Falsified by.** A rolled-back `nextval` whose value comes back, or a crash that skips more values
than PostgreSQL 19's would.

## 7. Schemas and search_path resolved in analysis, statements lowered fully qualified

**The decision.** Namespaces are PostgreSQL's: `pg_catalog` (OID 11), `public` (2200),
`information_schema`, `pg_toast`, `pg_temp` and user schemas. Resolution happens only in analysis, in
PostgreSQL's order (`src/backend/catalog/namespace.c:4308-4320`): `pg_catalog` first unless
`search_path` names it; `pg_temp` first for relations unless `search_path` names it, then at its
listed place, and never for functions or operators; then `search_path`'s entries, `"$user"` as the
role's schema. Statements lower fully qualified, so the engine never guesses: its own resolution
checks temp first unconditionally (`crates/engine/translate/emitter/mod.rs:1020-1076`), which is
not PostgreSQL's rule. The engine resolves unqualified names through a search path
(`crates/engine/connection.rs:178`), with `public` mapped to `main` (`emitter/mod.rs:1072-1074`); PostgreSQL schemas
are engine schemas, and `pg_temp` is the connection's temp schema, so temporary tables end with the
session as in PostgreSQL. User object OIDs start at `FirstNormalObjectId` (16384, PostgreSQL's
`src/include/access/transam.h`) from a durable counter outside the caller's transaction. A prepared
statement's cache key holds the search path and the catalog version.

**Falsified by.** A name that resolves to a different object than PostgreSQL 19 resolves it to under
the same `search_path`.

## 8. Isolation: PostgreSQL's three levels exactly, with the engine work each needs

**The decision.** Each PostgreSQL level is given exactly; where the engine can't yet, the gap is
named and counted, never hidden.

SQL's storage runs one validation for every transaction today: `Precision`, HyPer's precision
locking, the `#[default]` of `Validation` (`crates/engine/storage/versioned/version.rs:330-336`,
taken at `:1504`); the other variants (`Snapshot`, `Ssi`, `Occ`) serve only the Elle checks.
Choosing the validation per transaction, from the session's isolation level, is the first engine
work every level below needs.

- **SERIALIZABLE** refuses with `40001`. Precision refuses different transactions than
  PostgreSQL's SSI does, and the versioned storage's `Ssi` validation is not yet PostgreSQL's
  algorithm as PostgreSQL builds it; which transactions abort is Ada's choice below (Choices for
  Ada), measured by the isolation suite's serializable specs.
- **REPEATABLE READ** is snapshot isolation, which needs the `Snapshot` validation chosen per
  transaction: under Precision a writing transaction is refused when something it read changed,
  which PostgreSQL's REPEATABLE READ never does. A second difference to close: Amphora's first writer
  wins at once (`WriteWriteConflict`, `crates/engine/error.rs:109`), where PostgreSQL makes the
  second writer wait for the first and then fails it with `40001` only if the first committed.
  Engine work: a row-lock wait queue woken by the holder's commit or abort (an event, no timer), a
  wait-for graph refusing deadlocks with `40P01`, and `lock_timeout` refusing with `55P03`. Until
  then, every `40001` PostgreSQL wouldn't raise is counted as a failure, as READ COMMITTED's are.
- **READ COMMITTED** (PostgreSQL's default, and pgbench's) does not exist in the engine: a
  transaction's snapshot is fixed at its start (`version.rs:631-650`), and the engine's own
  Hermitage tests mark the difference (`crates/engine/mvcc/database/hermitage_tests.rs:101`). Engine
  work: a new snapshot for each statement under one transaction id, the same row-lock waits, and
  PostgreSQL's recheck (EvalPlanQual): after the waited-on writer commits, UPDATE, DELETE and
  `SELECT ... FOR UPDATE` re-read the newest version, re-test the statement's WHERE on it, and apply
  or skip. `FOR UPDATE`, `FOR SHARE`, `LOCK TABLE` and advisory locks share that lock manager.
  Until it lands, `BEGIN ISOLATION LEVEL READ COMMITTED` and the default run as REPEATABLE READ and
  say so in `SHOW transaction_isolation`; any `40001` this causes that PostgreSQL wouldn't is counted
  as a failure of section 10's pgbench measure.

An error inside a transaction block puts the session in PostgreSQL's aborted state (`25P02` until
ROLLBACK); SAVEPOINT and ROLLBACK TO map to the engine's savepoints (`crates/engine/translate/rollback.rs`).

**Against the 10x bar.** Contended rows (TPC-B's branch row, TPC-C's district row) are where
PostgreSQL's per-row locks and process wake-ups cost it most; Amphora's wait queue wakes the next
waiter on the commit event itself.

**Falsified by.** pgbench's TPC-B script at its defaults, at any client count, reporting a failed
transaction against Amphora that it doesn't report against PostgreSQL 19; or the isolation suite's
READ COMMITTED and REPEATABLE READ specs differing.

## 9. The catalog served from Amphora's schema, one source of truth

**The decision.** `pg_catalog`'s relations are internal virtual tables the PostgreSQL dialect
registers on every schema build (`Dialect::register_catalog`, `Schema::register_internal_vtab`,
`crates/engine/schema.rs:992`; `turso_types_vtab.rs:23` is a working example), derived each time from
the engine's schema and the tables' stored PostgreSQL definitions, never copied:

- **First, what pgbench and psql read** (`src/bin/pgbench/pgbench.c:5458-5468`, `:868`;
  `src/bin/psql/describe.c:1512-1516`, `:1651-1656`, `:1952-2045`, `:4165-4289`): `pg_class`,
  `pg_namespace`, `pg_attribute`, `pg_type`, `pg_attrdef`, `pg_index`, `pg_constraint`,
  `pg_inherits` and `pg_partitioned_table` (empty until partitioning), `pg_am`, `pg_collation`,
  `pg_description`, `pg_depend`, `pg_trigger`, `pg_policy`, `pg_statistic_ext`, `pg_rewrite`,
  `pg_publication`, `pg_publication_rel` and `pg_publication_namespace` (present, empty, with every
  column), `pg_roles`, `pg_database`, `pg_settings`; the casts `regclass`, `regnamespace` and
  `regtype`; and the functions those queries call: `format_type`, `pg_get_expr`,
  `pg_get_indexdef`, `pg_get_constraintdef`, `pg_get_statisticsobjdef_columns`, `pg_get_viewdef`,
  `pg_get_userbyid`, `pg_table_is_visible`, `pg_relation_is_publishable`, `col_description`,
  `obj_description`, `current_schemas`, `array_position`, `= any(array)`, and the `~` operator,
  which is PostgreSQL's regular expressions (Spencer's ARE engine, ported as a flat, bounded loop:
  Rust's `regex` crate is a different language). psql sends every describe query for a plain table
  to a 19 server (foreign keys, extended statistics and publications among them,
  `src/bin/psql/describe.c:2640`, `:2826`, `:3091`) and stops printing at the first that fails, so
  `\d` needs all of them.
- **Static rows** for `pg_proc`, `pg_operator`, `pg_cast`, `pg_opclass`, `pg_amop` from the vendored
  `.dat` files, plus user rows.
- **`information_schema`** is PostgreSQL's own `information_schema.sql`, vendored and run through the
  front end: its views are SQL over `pg_catalog`, so running it is also a coverage test.

**Against the 10x bar.** psql's describe queries and pgbench's start-up query are joins over the
catalog; served from the in-memory schema, they cost what reading memory costs.

**Falsified by.** A describe or start-up query whose rows differ from PostgreSQL 19's for the same
objects.

## 10. Done when: PostgreSQL's own suites pass, the drivers' suites pass, and 10x PostgreSQL at every scale

Ada's goal form: a plain requirement, when it is done, the benchmark against competitors, and where
to stop.

**Compatibility, the requirement.** A PostgreSQL 19 client can't tell Amphora from PostgreSQL 19 by
what it answers.

- **PostgreSQL's regression suite.** Upstream `pg_regress --use-existing` against Amphora, each of
  the 243 files of 19's `src/test/regress/parallel_schedule` (29 groups) compared byte for byte with
  19's expected output or one of its `_0` to `_9` variants (`pg_regress.c:1349`, `:1469`). The
  database is made as `pg_regress --no-locale` makes it, `LOCALE='C' LOCALE_PROVIDER='builtin'`
  (`pg_regress.c:2043-2047`). Reported per file, pass or fail, with the diff's size. Done when 243
  of 243 pass, and the isolation suite's 139 specs (`src/test/isolation/isolation_schedule`) pass.
  The suite assumes what a PostgreSQL server has: C functions from `regress.so` (`test_setup.sql`
  `:250-253`; 18 files load it), COPY from files on the server's disk (`:137-138`; 13 files name
  `abs_srcdir`, 15 with `abs_builddir`), an in-place tablespace (`:27-28`) and a superuser (Choices
  for Ada asks how to count these).
- **The drivers' own suites,** at pinned versions: libpq (PostgreSQL's `libpq_pipeline` binary,
  called directly with a conninfo and `max_protocol_version=latest`, its usage at
  `src/test/modules/libpq_pipeline/libpq_pipeline.c:2156`, its traces compared by our harness, since
  its TAP script starts a PostgreSQL of its own, `t/001_libpq_pipeline.pl:24-26`), pgjdbc,
  psycopg 3, pgx v5, node-postgres, and after them lib/pq, asyncpg and Npgsql. Done when each
  suite's pass and fail sets against Amphora equal its sets against PostgreSQL 19 on the same machine
  (CockroachDB runs the same suites with lists of expected failures, its `orm_helpers.go`; ours is
  empty: the reference is PostgreSQL itself).
- **pgbench is correct:** `pgbench -i` with its default steps `dtgvp` (`pgbench.c:163`) and with
  server-side generation `G`, then the TPC-B-like script (`pgbench.c:793-805`) in all three `-M`
  modes; done when no transaction fails at any client count and the four balances agree (accounts,
  tellers and branches each sum to history's deltas).
- **The competitors** for compatibility, as published: DoltgreSQL 39.38% of 25,080 regression tests
  (dolthub.com, 2025-07-08); pgrust "all 46,066 tests" of PostgreSQL 18.6's 231 files (its README at
  `79ad992e`), at 728 TPC-B transactions a second against PostgreSQL 18.4's 6,365 (datapace.ai);
  CockroachDB runs 189 files at `REL_16_0` with expected diffs and publishes no rate. The bar is
  pgrust's 100%, at 10x PostgreSQL's speed rather than a ninth of it.

**Performance, the requirement (Ada: "order of magnitude better Postgres performance from us at all
scales").** Amphora over PostgreSQL's protocol is about 10x PostgreSQL 19.0, on the same machine,
through the same client, at every scale.

- **Workloads:** pgbench's TPC-B-like and select-only scripts (`pgbench.c:821-824`) in `-M simple`,
  `extended` and `prepared`; a TPC-C load by BenchBase, whose transactions are client-side SQL sent
  through pgjdbc's prepared statements (its `NewOrder.java`), where HammerDB's PostgreSQL TPROC-C
  runs as server-side PL/pgSQL functions ("by default the 5 TPROC-C transactions are implemented
  using PostgreSQL functions", hammerdb.com docs ch04s09) and so would compare procedural languages,
  not servers (HammerDB joins once PL/pgSQL is in scope); TPC-H and JOB (the IMDB data of "How Good
  Are Query Optimizers, Really?") through the same driver Amphora's bench already uses against
  PostgreSQL (`bench/src/pg.rs`, the `postgres` crate); connection setup per second; and
  prepared-statement throughput (Parse once, Bind and Execute many).
- **Scales:** 1 client to 10,000 and more; data from what fits in a CPU cache to larger than the
  machine's memory (pgbench scale, TPC-H scale factor and TPC-C warehouses chosen from the machine's
  measured memory, recorded with each run).
- **Reported:** throughput and latency at p50, p99 and p99.9 per workload and scale (pgbench's
  per-transaction log, BenchBase's latency histogram), with the load average, both binaries'
  commits and the configuration beside each run (CLAUDE.md §7).
- **PostgreSQL tuned as it runs in production** (benchmark checklist question 2): memory settings
  sized to the machine, and at 1,000 clients and more both PostgreSQL alone and PostgreSQL behind
  PgBouncer in transaction pooling, the ratio taken against the faster. Durability equal: both make
  each commit durable before answering by the platform's full flush (PostgreSQL with `fsync`,
  `synchronous_commit` and `full_page_writes` on; on macOS `wal_sync_method=fsync_writethrough`,
  matching Amphora's `F_FULLFSYNC`).
- **The floors physics sets, measured and shown with every ratio:** a stub server that speaks the
  protocol and answers each message at once gives the most any server can reach through that client
  on that machine (the client's own work, the kernel and the loopback round trips); the platform's
  full-flush latency bounds one client's durable commits; the client's SCRAM key derivation (4096
  rounds of PBKDF2, done by the client) and the TLS handshake bound connection setup; the device's
  random-read latency bounds reads larger than memory. A workload whose floor already leaves less
  than 10x room says so, with the numbers, rather than hiding it.
- **Where the 10x must come from,** by workload:

| Workload | PostgreSQL's cost Amphora removes | Floor shown beside it |
|---|---|---|
| Connection setup | A forked backend per connection, warmed from cold (`arch-dev.sgml:118`) | Client SCRAM rounds, TCP and TLS handshakes |
| Select-only, simple | Parse, analysis and planning of every statement; buffer pins and visibility checks | One loopback round trip per statement; the client's CPU |
| Select-only, prepared; prepared throughput | Executor start-up per execute; snapshot taking across all backends | As above |
| TPC-B | Heap tuple versions and WAL insertion locks; row-lock waits that wake processes | One full flush per commit at one client, shared by group commit above it |
| TPC-C | The lock manager under contended district and warehouse rows | Full flush per commit group |
| TPC-H, JOB | A planner without learned weights or bushy search; tuple-at-a-time execution with few workers | Memory bandwidth; device reads past memory |
| 10,000 clients | A process and its memory per client | The kernel's per-socket work |

**Done when** every workload at every scale reaches 10x PostgreSQL 19.0 on throughput and on p50, p99
and p99.9, measured on the part either server can change: (PostgreSQL's time less the floor) over
(Amphora's time less the floor) is at least 10, with the raw ratio printed beside it.

**The diminishing-returns stop.** A workload stops when it is done, or when Amphora's time less the
floor is within the floor's own run-to-run noise (nothing is left for a server to remove), with the
numbers recorded. A failing regression file stops when what it still needs is outside the scope Ada
sets (Choices for Ada); it is reported as failing, never removed from the count.

## 11. The order of delivery: one-hour units, each ending in its check

The first useful milestone is psql and pgbench connecting and pgbench's TPC-B-like script running
correctly. A unit that outgrows its hour splits at its check.

| Unit | What | The check that ends it |
|---|---|---|
| 1 | The wire core, sans-IO: startup at 3.0 and 3.2, NegotiateProtocolVersion (grease included), SSLRequest and GSSENCRequest answered `N`, SCRAM-SHA-256, the 15 ParameterStatus values, BackendKeyData, simple Query framing with each statement refused `0A000`, Terminate | A protocol-level test client written in the test: RFC 7677's SCRAM vectors, a 3.9999 start-up negotiated down, a wrong password refused `28P01`, a Query answered ErrorResponse then ReadyForQuery `I`; hostile input to every message parser |
| 2 | The listener shell on hyper-rt, sessions charged to the memory budget, admission shared with lane P | psql and pgbench connect, authenticate, and get `0A000` for a statement; a refused session is `53300` |
| 3 | The oracle: 19's own `raw_parser` taken out of `REL_19_0` by libpg_query's extraction, without its patches, with tree output, in a test-only crate | It refuses `SELECT $1abc` and `NOTIFY ch, $1` as 19 does, and gives a verdict for every statement of 19's regression SQL |
| 4 | The scanner: flex tables from 19's `scan.l` (11 exclusive start conditions, `yyless`), the Rust scan loop, its actions, `parser.c`'s filter | Every statement of 19's regression SQL scans to the oracle's tokens; fuzzed input never panics |
| 5 | The grammar tables from 19's `gram.y`, the Rust LR loop, every rule refusing `0A000` | Every statement of 19's regression SQL accepted or rejected as the oracle does, at the same position |
| 6 | Actions for the statements pgbench and psql send (SELECT, INSERT, UPDATE, DELETE, BEGIN, END, COPY, CREATE and DROP TABLE, TRUNCATE, VACUUM, ALTER TABLE ADD PRIMARY KEY, SET, SHOW) | Their parse trees equal the oracle's |
| 7 | The catalog core: the vendored `.dat` tables, `pg_namespace`, `pg_class`, `pg_attribute`, `pg_type`, OIDs, `search_path` | Built-in `pg_type` rows equal 19's; name resolution tests |
| 8 | Analysis for int4, int8, text, char(n), `"char"`, timestamp, timestamptz (and the assignment cast between them under the session's `TimeZone`), `regclass`, and their operators and casts, with SQLSTATEs | `SELECT 1` is int4; `2147483647 + 1` is `22003`; `$1::pg_catalog.regclass` resolves a table; results typed as 19 types them |
| 9 | Lowering into the planner and the keys of section 5 for those types; RowDescription and DataRow in text | TPC-B's statements on a small table give 19's rows |
| 10 | The extended protocol and binary formats for those types | pgbench `-M extended` and `-M prepared` statement flows, and step `g`'s `PQexecParams` relkind query (`pgbench.c:868`), through the test client |
| 11 | COPY FROM STDIN with `FREEZE` (sent to servers reporting 14 or later, `pgbench.c:5050-5052`), TRUNCATE, VACUUM, `ALTER TABLE ADD PRIMARY KEY`, `CREATE TABLE ... WITH (fillfactor)`, `generate_series` in FROM | `pgbench -i` (steps `dtgvp`, and `G`) leaves the tables 19 leaves |
| 12 | The start-up query's catalog: `pg_partitioned_table`, `pg_inherits`, LATERAL, `current_schemas`, `array_position`, text[] | pgbench's partition query returns 19's row; TPC-B at one client runs with no failure and the balances agree (milestone one at one client) |
| 13 | psql's describe catalog, casts and functions (section 9's list), and PostgreSQL's regular expressions | `\dt` and `\d pgbench_accounts` print what 19 prints |
| 14 | The validation chosen per transaction, statement snapshots and the row-lock wait queue (section 8) | Two sessions updating one row: the second waits and then sees the first's value |
| 15 | EvalPlanQual recheck, deadlock refusal, `lock_timeout` | TPC-B at 32 clients: no failed transaction, balances agree (milestone one) |
| 16 | The bench harness: PostgreSQL 19 and Amphora on one machine, the stub server, the flush floor, percentiles | The first 10x table for TPC-B and select-only at 1 to 10,000 clients, floors beside each ratio |
| 17 | The regression harness: upstream `pg_regress --use-existing`, per-file results kept in `bench/records/` | The first per-file pass table for 19's 243 files |

After milestone one, units follow the regression suite's failing files and the benchmark's largest
server-attributable gaps, alternating, with TLS (SCRAM-PLUS, direct TLS), CancelRequest and
LISTEN/NOTIFY each a unit with a driver-suite check.

## Choices for Ada

Each is a plain choice; the default holds until she says otherwise.

1. **The regression suite's C functions.** `test_setup.sql` loads four C functions from PostgreSQL's
   `regress.so`, and 18 files use it. Amphora can't load PostgreSQL's C. Either write those functions
   natively in Amphora, present only in a database made for the suite (most files can then run), or
   count every file that needs them as failing. Default: write them natively.
2. **Scope.** What may be declared out of scope, counted as failing rather than built: PL/pgSQL
   (the regression suite and HammerDB need it), C-language functions and extensions, ICU and libc
   locales (the default keeps `C` and builtin `C.UTF-8`, whose order is SQLite's BINARY), XML,
   full-text search, geometric and range types, large objects, logical replication
   (`CREATE PUBLICATION`, `WAIT FOR LSN`), tablespaces beyond a catalog row. Default: everything in
   scope, ordered by what the suite and the benchmarks hit first.
3. **SERIALIZABLE's aborts.** PostgreSQL guarantees serializability, not which transaction aborts.
   Either match PostgreSQL's abort choices exactly (port its predicate-lock granularity) so the
   isolation suite's serializable specs pass byte for byte, or keep Amphora's `Ssi` validation and
   report those specs' differences. Default: match exactly, since done is 139 of 139.
4. **OAUTHBEARER and MD5.** PostgreSQL 19 offers OAuth and still allows MD5 with a warning. Default:
   SCRAM only, MD5 refused, OAuth later if a client needs it.
