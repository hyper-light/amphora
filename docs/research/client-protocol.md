# The client protocol: PostgreSQL's, MySQL's, SQLite's API, libSQL's Hrana, and Amphora's

Source notes for the client-protocol decision (AC-T1.2.3, CP-T1.2; `docs/design/client-protocol.md`
and the comparison in `docs/design.md`). Each claim below is one source's own words: the line is
`- **pN** · source · locator · "quote"`, and the design cites it as `[pN]`. The doc-truth test
(`crates/amphora/tests/design_index.rs`) holds the design to these lines: every citation resolves,
every claim is cited, every source is graded, and every number the comparison states is in a quote
its own paragraph or cell cites.

## How to read this

The pages were read on 2026-11-21: PostgreSQL's chapter 53 for version 17, the documentation of
MySQL's source (its "Client/Server Protocol" pages, generated from the source of MySQL 26.10.0),
SQLite's own pages, and libSQL's Hrana 3 specification at commit
`a1d031cb602fedefbcf55c568e98e998a2274400`. Amphora's own claims quote its design record and code
at the commit this note landed with. HTML pages were read as text, so a quote's spacing is the
page's words with its markup removed. Grades: A is a specification or the system's own reference,
B its own design notes, down to F for an unsourced claim; every source here is A.

## Sources

| Key | What | Where | Kind | Grade | Used for |
|---|---|---|---|---|---|
| `pg-overview` | PostgreSQL 17, §53.1 Overview | https://www.postgresql.org/docs/17/protocol-overview.html | specification | A | Messages, simple and extended query. |
| `pg-flow` | PostgreSQL 17, §53.2 Message Flow | https://www.postgresql.org/docs/17/protocol-flow.html | specification | A | Implicit transactions, pipelining, cancellation. |
| `mysql-packets` | MySQL, Client/Server Protocol: MySQL Packets | https://dev.mysql.com/doc/dev/mysql-server/latest/page_protocol_basic_packets.html | specification | A | Framing and sequence ids. |
| `mysql-prepare` | MySQL, COM_STMT_PREPARE | https://dev.mysql.com/doc/dev/mysql-server/latest/page_protocol_com_stmt_prepare.html | specification | A | Prepared statements. |
| `mysql-binary` | MySQL, Binary Protocol Resultset | https://dev.mysql.com/doc/dev/mysql-server/latest/page_protocol_binary_resultset.html | specification | A | Binary rows. |
| `sqlite-serverless` | SQLite Is Serverless | https://www.sqlite.org/serverless.html | reference | A | No server process. |
| `sqlite-cintro` | An Introduction To The SQLite C/C++ Interface | https://www.sqlite.org/cintro.html | reference | A | Prepare, bind, step. |
| `hrana3` | libSQL, The Hrana protocol specification (version 3) | https://github.com/tursodatabase/libsql/blob/a1d031cb602fedefbcf55c568e98e998a2274400/docs/HRANA_3_SPEC.md | specification | A | The embedded-replica systems' protocol. |
| `amphora-transport` | Amphora, `docs/design/local-owner-transport.md` | docs/design/local-owner-transport.md (lines) | design | A | Amphora's wire between processes. |
| `amphora-database` | Amphora, `crates/amphora/src/database.rs` | crates/amphora/src/database.rs (lines) | code | A | Amphora's embedded API. |

## Claims

### PostgreSQL

- **p1** · pg-overview · §53.1 · "The first byte of a message identifies the message type, and the next four bytes specify the length of the rest of the message (this length count includes itself, but not the message-type byte)."
- **p2** · pg-overview · §53.1 · "In the “simple query” protocol, the frontend just sends a textual query string, which is parsed and immediately executed by the backend."
- **p3** · pg-overview · §53.1 · "In the “extended query” protocol, processing of queries is separated into multiple steps: parsing, binding of parameter values, and execution."
- **p4** · pg-flow · §53.2.4 Pipelining · "Use of the extended query protocol allows pipelining, which means sending a series of queries without waiting for earlier ones to complete."
- **p5** · pg-flow · §53.2.4 Pipelining · "Since, after an error, the backend will skip command messages until it finds Sync, this allows later commands in a pipeline to be skipped automatically when an earlier one fails, without the client having to manage that explicitly with BEGIN and COMMIT."
- **p6** · pg-flow · §53.2.2.1 Multiple Statements in a Simple Query · "an implicit block is closed automatically at the end of the Query message, either by an implicit commit if there was no error, or an implicit rollback if there was an error."
- **p7** · pg-flow · §53.2.8 Canceling Requests in Progress · "To issue a cancel request, the frontend opens a new connection to the server and sends a CancelRequest message, rather than the StartupMessage message that would ordinarily be sent across a new connection."

### MySQL

- **p8** · mysql-packets · Protocol::Packet · "Data between client and server is exchanged in packets of max 16MByte size."
- **p9** · mysql-packets · Sequence ID · "The sequence-id is incremented with each packet and may wrap around. It starts at 0 and is reset to 0 when a new command begins in the Command Phase."
- **p10** · mysql-prepare · COM_STMT_PREPARE · "Creates a prepared statement for the passed query string."
- **p11** · mysql-binary · NULL-Bitmap · "The binary protocol sends NULL values as bits inside a bitmap instead of a full byte as the Text Resultset Row does."

### SQLite

- **p12** · sqlite-serverless · §1 · "Most SQL database engines are implemented as a separate server process."
- **p13** · sqlite-serverless · §1 · "With SQLite, the process that wants to access the database reads and writes directly from the database files on disk."
- **p14** · sqlite-cintro · §1 Summary · "Compile SQL text into byte-code that will do the work of querying or updating the database."
- **p15** · sqlite-cintro · §1 Summary · "Advance an sqlite3_stmt to the next result row or to completion."

### libSQL's Hrana

- **p16** · hrana3 · lines 3-4 · "is a protocol for connecting to a SQLite database over the network."
- **p17** · hrana3 · lines 11-12 · "Each stream corresponds to a SQLite connection and executes a sequence of SQL statements."
- **p18** · hrana3 · lines 18-19 · "Multiple streams can be multiplexed over a single WebSocket."
- **p19** · hrana3 · lines 99-102 · "To reduce the number of roundtrips, the protocol supports batches of statements that are executed conditionally, based on success or failure of previous statements. Clients can use this mechanism to implement non-interactive transactions in a single roundtrip."
- **p20** · hrana3 · lines 593-597 · "This is accomplished by the use of a baton, which is similar to a session cookie."
- **p21** · hrana3 · lines 30-33 · "is the canonical encoding, backward compatible with Hrana 1 and 2."

### Amphora

- **p22** · amphora-transport · lines 3-4 · "A second process uses a product-owned local client; it never opens another engine."
- **p23** · amphora-transport · lines 39-40 · "The wire is a four-byte big-endian length followed by a postcard envelope. Variant order is append-only."
- **p24** · amphora-transport · line 32 · "The client authenticates the owner before revealing a client proof"
- **p25** · amphora-transport · lines 36-37 · "OS-local stream isolation protects subsequent SQL/Records frames; this is not a network transport."
- **p26** · amphora-transport · line 68 · "Adaptive SQL is executed inside existing Database.call. The client closure runs once."
- **p27** · amphora-transport · lines 71-72 · "Loss of the final response after Commit is an unknown outcome; callers must not replay FnOnce."
- **p28** · amphora-database · `Database::execute`'s comment · "One atomic batch, including SELECT or DML RETURNING results."
