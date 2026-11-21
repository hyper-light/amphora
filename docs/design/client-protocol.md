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
