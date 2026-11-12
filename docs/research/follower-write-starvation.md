# A follower's write starves under steady leader writes (2026-11-07)

Source notes for kiln goal 9c63f7d4 (Duplicate Systems, kiln step 10), plan 01a1b8d174dc72d18b9d2289b6e8fe11, research
01a1b8d174e17549916bd3fc88081ff9. Each finding carries kiln's grade there (A to F, out of 100) as of 2026-11-07, its
sources and the words it rests on. The candidates it leads to are A/B'd in bench/records and decided in
docs/design/replication.md once Ada has chosen.

## Why it starves

**f1 (A 85).** A replicated write applies only on its exact base. `on_base` (crates/amphora/src/replication.rs:99-108):
`if base_index == point.index && base_term == point.term {`, else `"the database advanced since preparation"`. The
design says so (docs/design/replication.md:11): "An entry with a stale base is refused;". Any entry committed between
a follower's preparation and its proposal's append refuses it, whatever that entry touched.

**f2 (A 89).** The entry is whole-page images of the dirty pages and nothing of what was read. `Db::prepare_pages`
(crates/amphora/src/db.rs:764-781) runs the session under `BEGIN IMMEDIATE`, takes `Pager::transaction_image`
(crates/engine/storage/pager.rs:4140-4175, `let dirty = self.dirty_pages.read();`) and rolls back. With no read set,
nothing can tell a harmless intervening entry from a conflicting one, so the exact-base fence is the only safe rule
today.

**f3 (B 81).** kiln re-runs a refused closure until its deadline, with no backoff and no priority
(kiln p2p/stall 3aa1cb35097, crates/kiln/src/store.rs:126:
`Err(refusal) if stale(&refusal) && Instant::now() < until => continue,`). A follower's write takes a ReadIndex round
trip, its apply, its preparation and half a round trip to reach the leader's log, while the leader's own writes take
none of that, so under a steady leader the follower loses every round. Kiln's real-process test saw box-a's
`write_again` reach 64 in about 37 s, then "commit outcome is unknown" (the goal's brief).

## Validate what was read (optimistic)

**f4 (C 64), f5 (C 64).** Kung and Robinson, "On Optimistic Methods for Concurrency Control", ACM TODS 6(2), 1981
([pdf](https://www.eecs.harvard.edu/~htk/publication/1981-tods-kung-robinson.pdf)). Backward validation (p. 221):
"if (write set of transaction with transaction number t intersects read set)" then invalid. The history is bounded
(p. 220): "We solve this problem by only requiring the concurrency control to maintain some finite number of the most
recent write sets", and a transaction older than the window fails. Section 6 applies it to B-trees with pages as the
objects (p. 223): "For B-trees, we naturally choose the objects of the read and write sets to be the pages of the
B-tree." Conflicts come from splits (p. 224): "Splitting occurs only when an insertion is attempted on an already full
page". This is candidate A1, with CLAUDE.md §2's bound built in.

**f8 (A 85).** SQLite's BEGIN CONCURRENT is the same rule on Amphora's file format
([begin_concurrent.md](https://www.sqlite.org/src/doc/begin-concurrent/doc/begin_concurrent.md)): "When a BEGIN
CONCURRENT transaction is committed, the system checks whether or not any of the database pages that the transaction
has read have been modified since the BEGIN CONCURRENT was opened." It warns about rowid appends: "This is terrible for
concurrency, as it all but ensures that all new rows are added to the same database page." So page validation cannot
fix two writers appending to one table: the rightmost leaf is a real page conflict, even though no row is.

**f9 (B 72, inferred), f10 (A 90).** Page 1 ([file format](https://www.sqlite.org/fileformat2.html) §1.3, §1.6) holds
the "Size of the database file in pages", the "Page number of the first freelist trunk page", the freelist count and
the schema cookie, and "The 100-byte database file header is found only on page 1, which is always a table b-tree
page" (the root of sqlite_schema). Every transaction reads it. In WAL mode "the change counter might not be
incremented on each transaction" (§1.3.6), so page 1 is written only by allocation, the freelist and the schema, but
every allocating writer writes it. Merging page 1's allocation fields is not enough on its own: two writers that each
allocate page N+1 from the same size N collide on the page number, so a merge must also renumber (or partition)
allocations. Candidate A1m measures how often that happens before anyone builds it.

**f11 (A 85).** FoundationDB validates by key range
([developer guide](https://apple.github.io/foundationdb/developer-guide.html)): "Two concurrent transactions conflict if
the first to commit writes a value that the second reads.", refused as `not_committed`, by resolvers holding recent
commits' written ranges. Snapshot reads and atomic operations add no read conflict. Key ranges remove the rightmost-leaf
false conflict that pages keep, but a page image cannot be applied over a leaf another entry changed, so key-range
validation needs a row-level rebase beside it (candidate E).

**f12 (A 85).** SSI as PostgreSQL implements Cahill, Röhm and Fekete
([README-SSI](https://raw.githubusercontent.com/postgres/postgres/master/src/backend/storage/lmgr/README-SSI)): it
tracks rw-conflicts only, and "there is a risk of false positives, because not every dangerous structure is embedded in
an actual cycle"; to bound memory "multiple fine-grained locks may be promoted to a single coarser-grained lock as
needed". Pages are the coarse end of that trade.

**f13 (A 85).** Silo (Tu et al., SOSP 2013, [MIT](https://dspace.mit.edu/handle/1721.1/90269)) validates read sets at
commit by per-record version; TicToc (Yu et al., SIGMOD 2016, [MIT](https://dspace.mit.edu/handle/1721.1/115329))
"commits transactions that would be aborted by conventional T/O schemes" by computing commit timestamps from
per-item timestamps. The lesson for Amphora: validate per item read, not by one global log position, which is what
`on_base` does.

## Make the starving writer pessimistic

**f6 (C 64).** Kung and Robinson's own remedy for starvation (p. 220): "If the concurrency control detects a
"starving" transaction (this could be detected by keeping track of the number of times validation for a given
transaction fails), the transaction can be restarted, but without releasing the critical section semaphore. This is
equivalent to write-locking the entire database, and the "starving" transaction will run to completion." In Amphora
the critical section is the leader's write slot: the owner already serializes its preparations until each resolves.
Candidate C gives a follower's write that slot (a bounded lease) for its preparation; D takes it only after k
refusals.

**f7 (C 64).** MOCC (Wang and Kimura, PVLDB 10(2), 2016, [pdf](https://www.vldb.org/pvldb/vol10/p49-wang.pdf)):
"Optimistic CC (OCC) scales the best for workloads with few conflicts, but suffers from clobbered reads for high
conflict workloads", and "For low conflict workloads, MOCC maintains OCC's high performance without taking read locks."
The hybrid of optimistic validation and a pessimistic fallback for what is hot is established practice (candidate D).

## Run the write where it is ordered

**f17 (A 98).** SQLite's replicated cousins write at one node. LiteFS ([primary](https://docs.fly.io/litefs/primary)):
a replica's application "should forward the request to the primary". dqlite
([replication](https://canonical.com/dqlite/docs/explanation/replication)): "Dqlite uses the Raft algorithm to
replicate SQLite write transactions across application nodes", the leader encoding each write's changes into an entry.

**f18 (A 85).** CockroachDB ([replication layer](https://docs.cockroachlabs.com/docs/stable/architecture/replication-layer)):
"A single node in the Raft group acts as the leaseholder, which is the only node that can serve reads or propose writes
to the Raft group leader". Candidate F runs a follower's statements at the leader as a remote session.

**f15 (A 85).** VoltDB ([stored procedures](https://docs.voltdb.com/UsingVoltDB/DesignProc.php)): "In VoltDB, a stored
procedure and a transaction are one and the same." and "To ensure data consistency and durability, VoltDB procedures
must be deterministic." Candidate B (named commands re-run at the leader) is this model: a write is registered code
plus its arguments. Kiln's writes are in-process closures (f3), so B costs kiln a registry of every write it makes,
and both nodes must run the same registry; Amphora's application floor already attests which application each member
runs.

**f14 (A 87).** Deterministic databases replicate inputs. Calvin (Thomson et al., SIGMOD 2012,
[pdf](https://cs.yale.edu/homes/thomson/publications/calvin-sigmod12.pdf)): "By replicating transaction inputs rather
than effects, Calvin is also able to support multiple consistency levels". Aria (Lu et al., PVLDB 13(11), 2020,
[pdf](https://www.vldb.org/pvldb/vol13/p2047-lu.pdf)) notes that "existing state-of-the-art deterministic databases
require that transaction read/write sets are known before execution", and proposes "a novel deterministic reordering
mechanism that allows Aria to order transactions in a way that reduces the number of conflicts." Amphora replicates
effects (resolved pages) on purpose (f16), so the usable idea is Aria's ordering: the leader decides admission
deterministically and can favour the older, starving write (candidate G).

**f16 (A 93).** Statement replication is out. rqlite ([non-deterministic](https://rqlite.io/docs/api/non-deterministic/)):
"rqlite only rewrites functions that are evaluated when the statement itself runs.", so defaults, triggers and CHECK
constraints diverge; replication.md:7-8: "Replaying its SQL on followers would repeat those decisions and can produce
different rows."

## Rebase the effect at row level

**f19 (A 85).** The SQLite session extension ([intro](https://www.sqlite.org/sessionintro.html)) records row changes by
primary key with their old values: "An UPDATE change represents the modification of one or more non-PRIMARY KEY fields
of a single row within a database table, identified by its PRIMARY KEY fields." and detects when "the current values of
the fields that will be modified by the change may not match the original values stored within the changeset."
Candidate E sends a follower's resolved row changes and its read key ranges; the leader validates the ranges and
replays the changes on its newer state, capturing the pages it proposes. No SQL runs again, so defaults and random
values stay resolved.

**f20 (C 68).** The engine's own MVCC is not that rebase: crates/engine/mvcc/database/mod.rs:3293, "TODO: For full
serializability (beyond snapshot isolation), we still need:" read-set and phantom validation. It detects write-write
conflicts only.

## Checking a candidate

**f21 (C 64).** Elle (Kingsbury and Alvaro, PVLDB 14(3), 2021, [pdf](https://www.vldb.org/pvldb/vol14/p268-alvaro.pdf)):
"We present Elle: a novel checker which infers an Adya-style dependency graph between client-observed transactions."
Every candidate runs list-append histories checked for cycles, beside a serial-replay oracle of the committed order.

## The candidates, and what each costs

| | candidate | rests on | what changes | what kiln changes |
|---|---|---|---|---|
| A0 | exact base (today, the control) | f1 | nothing | nothing |
| A1 | page read-set validation, bounded window | f4, f5, f8 | pager records reads; entry carries them; machine keeps recent write sets | nothing |
| A1m | A1, page 1's allocation fields merged | f9, f10 | A1 plus allocation renumbering or partitioning | nothing |
| B | named commands re-run at the leader | f15, f17 | a command registry and its wire | every write registered |
| C | write-slot lease for a follower's write | f6 | ReadIndex carries intent; leader holds its own preparations | nothing |
| D | A1m, then C after k refusals | f6, f7 | A1m and C | nothing |
| E | row-level rebase at the leader | f11, f19 | row change capture, key-range reads, leader replay | nothing |
| F | remote session at the leader | f18 | statements cross the network | nothing (latency) |
| G | age-priority admission at the leader | f14 | leader defers its own preparation for an older forwarded write | nothing |
