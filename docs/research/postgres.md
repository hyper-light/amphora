# Source notes: PostgreSQL clients on Amphora (lane PG1, 2026-11-23)

The sources `docs/design/postgres.md` rests on, each with where it was read and its own words.
PostgreSQL's source is the clone at `~/Projects/postgres`, tag `REL_19_0` (commit `5c839a2a0`,
"Stamp 19.0."); libpg_query's is `~/Projects/libpg_query`, branch `18-latest` (`7663e07`).
Web pages were fetched on 2026-11-23.

## The release and the protocol

| Source | Locator | Its words |
|---|---|---|
| postgresql.org versioning | https://www.postgresql.org/support/versioning/ | "19 / 19.0 / Yes / November 19, 2026 / November 19, 2031" |
| PostgreSQL 19 announcement | https://www.postgresql.org/about/news/postgresql-19-released-3449/ | "November 19, 2026 - The PostgreSQL Global Development Group today announced the release of PostgreSQL 19" |
| pqcomm.h | `src/include/libpq/pqcomm.h:94-95` | `#define PG_PROTOCOL_EARLIEST PG_PROTOCOL(3,0)`, `#define PG_PROTOCOL_LATEST PG_PROTOCOL(3,2)` |
| pqcomm.h | `src/include/libpq/pqcomm.h:115`, `:189` | `#define PG_PROTOCOL_GREASE PG_PROTOCOL(3,9999)`; `#define PG_ALPN_PROTOCOL "postgresql"` |
| Protocol overview (19) | https://www.postgresql.org/docs/19/protocol-overview.html | "The current, latest version of the protocol is version 3.2. However … libpq still uses protocol version 3.0 by default." |
| Protocol overview (19) | same page, the versions table | "The secret key used in query cancellation was enlarged from 4 bytes to a variable length field." |
| Protocol greasing | `doc/src/sgml/protocol.sgml:370` | "Servers must not add special-case logic for this parameter; they should simply send the list of all unsupported options (including this one) via a NegotiateProtocolVersion message." |
| libpq's default | `src/interfaces/libpq/fe-connect.c:2145-2153` | "To not break connecting to older servers/poolers … default to the 3.0 protocol" |
| Release notes 19 | `doc/src/sgml/release-19.sgml:161-168` | "Force standard_conforming_strings to always be on in the database server" |
| Startup | `src/backend/tcop/backend_startup.c:472-476` | "received direct SSL connection request without ALPN protocol negotiation extension" |
| Message formats (19) | https://www.postgresql.org/docs/19/protocol-message-formats.html | BackendKeyData: "The minimum and maximum key length are 4 and 256 bytes"; "Before protocol version 3.2, the secret key was always 4 bytes long." |
| Cancel key length | `src/include/storage/procsignal.h:63` | `#define MAX_CANCEL_KEY_LENGTH 32` |
| SASL (19) | https://www.postgresql.org/docs/19/sasl-authentication.html | "The channel binding type used by PostgreSQL is tls-server-end-point." |
| SCRAM | `src/backend/libpq/auth-scram.c:20-21` | "the channel binding type is always "tls-server-end-point". The spec says the default is "tls-unique"" |
| SCRAM iterations | `src/include/common/scram-common.h:50` | `#define SCRAM_SHA_256_DEFAULT_ITERATIONS 4096` |
| RFC 7677 | https://www.rfc-editor.org/rfc/rfc7677.html | "iteration-count … SHOULD be at least 4096" |
| RFC 5929 | https://www.rfc-editor.org/rfc/rfc5929.html | tls-server-end-point is "The hash of the TLS server's certificate" |
| Protocol flow (19) | https://www.postgresql.org/docs/19/protocol-flow.html | "NotificationResponse can only be sent outside a transaction"; after an error the backend "reads and discards messages until a Sync is reached" |
| ParameterStatus | `doc/src/sgml/protocol.sgml:1527`, `:1548` | "a hard-wired set of parameters for which ParameterStatus will be generated"; "search_path was not reported by releases before 18" |
| An unknown COMMIT outcome | `doc/src/sgml/func/func-info.sgml:2877` | "to determine whether their transaction committed or aborted after the application and database server become disconnected while a COMMIT is in progress" |
| Process per user | `doc/src/sgml/arch-dev.sgml:118` | "process per user client/server model" |

## The parser

| Source | Locator | Its words |
|---|---|---|
| gram.y | `src/backend/parser/gram.y:216` (20,077 lines) | `%expect 0` |
| Scanner flags | `src/backend/parser/Makefile:60-61`; `meson.build:33` | `scan.c: FLEXFLAGS = -CF -p -p`, `FLEX_NO_BACKUP=yes`; `'--no-backup', '--', '-CF', '-p', '-p'` |
| Lookahead filter | `src/backend/parser/parser.c:93-94` | "requires more than one token lookahead. We reduce these cases to one-token lookahead by replacing tokens here" |
| Bison 3.8.2 on 19's gram.y | run by lane PG1's parser research | 3,477 productions, 744 nonterminals, 546 terminals, 6,574 states |
| libpg_query's version | `libpg_query/Makefile:10` | `PG_VERSION = 18.6` |
| libpg_query's extraction | `libpg_query/scripts/extract_source.rb:599`, `:577`, `:540` | `runner.deep_resolve('raw_parser')`; `runner.mock('transformExpr', 'return expr;')`; a mocked `proc_exit` calling `exit(1)` |
| libpg_query's recursion | `libpg_query/src/pg_query.c:269` | "Recursive code (e.g. the tree walkers) measures the current stack depth" |
| libpg_query's lag | `libpg_query/CHANGELOG.md:289`, `:227`, `:194`, `:87` | 15-4.0.0 (2022-11-29), 16-5.0.0 (2023-12-22), 17-6.0.0 (2024-09-27), 18.0.0 (2026-05-20) |
| sqlparser-rs | https://raw.githubusercontent.com/apache/datafusion-sqlparser-rs/main/README.md | "accepts queries that specific databases would reject, even when using that Database's specific Dialect" |
| Bison's terms | https://www.gnu.org/software/bison/manual/html_node/Conditions.html | "The distribution terms for Bison-generated parsers permit using the parsers in nonfree programs." |
| PostgreSQL's licence | `COPYRIGHT:8` | "Permission to use, copy, modify, and distribute this software and its documentation for any purpose" |
| libpg_query's cost | [bench/records/2026-11-23-mbp2-perf-postgres.md](../../bench/records/2026-11-23-mbp2-perf-postgres.md) | Measured under load: raw parse 1.2 to 2.4 µs a statement, protobuf 3 to 5 times that |

## The measure of done

| Source | Locator | Its words |
|---|---|---|
| Regression schedule | `src/test/regress/parallel_schedule` | 243 test names in 29 groups; `:7` "we put no more than twenty tests in any one parallel group" |
| Variant outputs | `src/test/regress/pg_regress.c:1349` | "Return: filename_i.ext, where 0 <= i <= 9" |
| No-locale database | `src/test/regress/pg_regress.c:2043-2047` | the test database made with `LOCALE='C' LOCALE_PROVIDER='builtin'` under `--no-locale` |
| C functions | `src/test/regress/sql/test_setup.sql:250-253` | `CREATE FUNCTION binary_coercible(oid, oid) … AS :'regresslib', 'binary_coercible' LANGUAGE C STRICT STABLE PARALLEL SAFE;` |
| Server-side COPY | `src/test/regress/sql/test_setup.sql:137-138` | `\set filename :abs_srcdir '/data/onek.data'` then `COPY onek FROM :'filename';` |
| Isolation suite | `src/test/isolation/isolation_schedule`; `src/test/isolation/README:6-12` | 139 tests; "the original motivation was to test the serializable isolation level" |
| pgbench scripts | `src/bin/pgbench/pgbench.c:793-805`, `:821-824` | the TPC-B-like script (`BEGIN;` … `END;`) and select-only `SELECT abalance FROM pgbench_accounts WHERE aid = :aid;` |
| pgbench init | `src/bin/pgbench/pgbench.c:163-164` | `DEFAULT_INIT_STEPS "dtgvp"`, `ALL_INIT_STEPS "dtgGvpf"` |
| pgbench start-up | `src/bin/pgbench/pgbench.c:5458-5468` | the partition query over `pg_class`, `pg_namespace`, `current_schemas`, `pg_partitioned_table`, `pg_inherits` |
| pgbench retries | `doc/src/sgml/ref/pgbench.sgml:539-541` | "By default, the option --max-tries is set to 1 and transactions with serialization/deadlock errors are not retried." |
| psql `\dt` | `src/bin/psql/describe.c:4165-4289` | `pg_class` joined to `pg_namespace`, `pg_get_userbyid`, `pg_table_is_visible(c.oid)` |
| DoltgreSQL | http://www.dolthub.com/blog/2025-07-08-doltgres-correctness-testing/ | 39.38% of 25,080 tests |
| pgrust | https://raw.githubusercontent.com/malisper/pgrust/79ad992ede22bcf6ae0c4fdead6fb01eeac5a990/README.md | "It passes all 46,066 tests in Postgres' regression suite" |
| pgrust's speed | https://datapace.ai/blog/pgrust-tests-are-not-the-spec | "about 728 transactions per second against 6,365 for vanilla Postgres 18.4" |
| CockroachDB's driver suites | https://raw.githubusercontent.com/cockroachdb/cockroach/master/pkg/cmd/roachtest/tests/orm_helpers.go | fails on `r.failUnexpectedCount > 0 \|\| r.passUnexpectedCount > 0` against a list of expected failures |
| HammerDB on PostgreSQL | https://www.hammerdb.com/docs/ch04s09.html | "With PostgreSQL by default the 5 TPROC-C transactions are implemented using PostgreSQL functions." |
| BenchBase TPC-C | https://github.com/cmu-db/benchbase/blob/main/src/main/java/com/oltpbenchmark/benchmarks/tpcc/procedures/NewOrder.java | `try (PreparedStatement stmtGetCust = this.getPreparedStatement(conn, stmtGetCustSQL)) {` |
| JOB | https://github.com/gregrahn/join-order-benchmark | "the Join Order Benchmark (JOB) queries from: "How Good Are Query Optimizers, Really?"" |
| Sequences | `src/backend/commands/sequence.c:58` | `#define SEQ_LOG_VALS 32` |
