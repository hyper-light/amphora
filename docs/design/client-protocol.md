# The client protocol (CP-T1.2, AC-T1.2.3)

How a program reaches Amphora's records. The comparison with PostgreSQL's wire protocol, MySQL's,
SQLite's embedded API and libSQL's Hrana is in `docs/design.md` ("The client protocol, researched");
the sources' own words are in `docs/research/client-protocol.md`, cited here as `[pN]`.

## 1. Embedded first, one framed protocol between processes, no network listener yet (proposed 2026-11-21)

**The decision.** A program in the database's own process calls the `amphora` crate directly, as a
program calls SQLite's library [p13]: one atomic batch of statements a call [p28], or a closure run
once inside the writer [p26]. A second process on the same machine reaches the one process that owns
the engine through Amphora's own local protocol, never by opening the engine again [p22]: frames of a
big-endian length and a postcard envelope whose variants only grow [p23], an owner that proves
itself before the client does [p24], and a link that is local by design [p25]. Amphora has no network
listener yet: neither PostgreSQL's wire protocol, MySQL's, nor Hrana is served. Kiln, today's only
client, reaches Amphora in its own process through kiln-seams' `Records`.

**Why, from the evidence.**
- A commit whose answer is lost. PostgreSQL's pages say how a message's statements commit or roll
  back [p6], and Hrana's how a batch's statements depend on each other [p19]; in the pages read,
  neither they nor MySQL's name the case where the answer to a commit never arrives (our reading,
  an absence no quote can show). Amphora's own protocol names it and refuses it typed, never
  replaying the closure [p27]; serving another product's protocol would drop that answer or add it
  outside the protocol.
- What the network protocols spend round trips on, an embedded caller doesn't pay: SQLite's prepare
  and step are calls, not messages [p14] [p15]. The leaders' answer to round trips, pipelining with
  skip-to-Sync [p4] [p5] and Hrana's conditional batches [p19], is what Amphora's one batch a call
  gives a caller in one step [p28].
- Records (compare-and-set, waits, the change feed) are not SQL. A SQL wire protocol would carry
  them only as SQL text [p2] [p3]; Amphora's frames carry them as typed requests [p25].

**What would falsify it.** A client Amphora must serve from another machine before replication's
groups take that role (CP-T1.8, CP-T1.9); a measured class where the local protocol's framing, not
the engine, holds a caller back; or a client that can only speak PostgreSQL's or MySQL's protocol
(a driver or tool the corpus needs). Any of these opens the network-listener choice, which this
record leaves open: PostgreSQL's extended query protocol with pipelining [p4] or Hrana's streams over
WebSocket or HTTP [p17] [p18], each judged first on whether it can carry an unknown outcome typed.

**Needs Ada's word.** This record states what Amphora is today and keeps the network choice open;
it is proposed, not approved. Reversible: nothing outside this file depends on it.

## 2. SQL with bound parameters and typed rows on serve's connections (proposed 2026-11-23)

**The decision.** `amphora serve` answers SQL from other machines on its own listener, taking up
the network choice the first decision left open. A connection whose first byte is `Q` carries frames, each
a little-endian length and a postcard value, under the same proof of the cluster's secret, the same
mutual TLS and the same descriptor budget as its `C` and `R` connections, and is kept between
requests. A request is `Write` (statements with bound parameters, run in order as one transaction
on the group's writer: all of them or none) or `Read` (one statement on the member's own state,
asking no leader, as `count` reads). Its answer is each statement's table: the columns' names, rows
of SQLite's own types (NULL, INTEGER, REAL, TEXT and BLOB, as `amphora_api::Value`), the rows the
last change touched and the last rowid; or a typed `Refusal`, a statement the engine refuses named
by its place in the request, the statements before it rolled back with it. The first request is
`Hello`, the oldest and newest versions the client speaks, answered with the newest both speak;
variants are only appended, each raising the version, as the local protocol's do [p23]. The client
is `serve::SqlClient`.

**Bounds, derived, never picked.** A request frame is at most the API's request bound
(`amphora_api::REQUEST_MAX`) and an answer at most its result bound (`amphora_api::RESULT_MAX`):
the bounds an embedded caller of Amphora's API already has, so a caller on another machine gets
the same. A request's bytes are charged to the process's memory budget (`Use::Queues`) before they
are read; an answer's rows are charged by their real size in memory as the engine steps them (each
value and what it points to), and its frame by its length, each given back once written. Past a
bound or the budget's room the answer is a typed `TooLarge`, a write rolled back whole, and the
connection goes on; a frame refused is read past through a fixed buffer, never held. The result
bound fits one frame, so an answer never spans frames: nothing below the bound needs streaming,
and nothing past it is answered.

**Why, from the evidence.**
- The benchmark drives every server from one program through that server's own client (MySQL's
  protocol for MySQL, ClickHouse's for ClickHouse), so Amphora's own protocol is as fair as
  theirs. A PostgreSQL listener would map SQLite's types onto PostgreSQL's and still leave
  PostgreSQL's tools broken on what Amphora lacks: its catalog and its dialect.
- The owner transport's network endpoint already carries typed statements and rows, but opens a
  connection and a TLS handshake for each request and has no session client across machines;
  serve's connections are kept, so a benchmark measures the database, not handshakes.
- A transaction is one request, as Hrana's batches make one a single round trip [p19] and
  Amphora's own call is one batch [p28]: no client holds a member's writer open across round
  trips, so no write waits on a client's network.

**What would falsify it.** A client whose transaction needs its own logic between statements
across round trips; an answer past the result bound a caller must have whole; or a measured class
where serve's thread for each connection, not the engine, holds clients back.

**Needs Ada's word.** Proposed, not approved. Left out, each a variant appended later: a
transaction held open across round trips, and an answer past the result bound streamed in frames.
