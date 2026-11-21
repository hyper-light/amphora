# Records in amphora-api: the feed, the simulator, the suite (CP-T1.5)

Kiln goal a1f386fe (G9 step 1, kiln plan `01a16b0f4735713283725eb7cfd502d3`): Records in
`amphora-api`, typed and simulated: families and keys at revisions, conditions, index families,
counters; the change feed derived from the log with typed `Compacted` refusals; the simulator
beside it. Done when both implementations pass kiln-seams' conformance suite.

## Decision (lane records-api): the feed is read from the versions the commits wrote

**Decision.** A family's change feed (`Records::changes`, `wait`, `changed`) is read from the
version rows the commits themselves wrote (`__amphora_versions`), through their index by
(namespace, family, revision, place in the commit) (`__amphora_feed`). Nothing beside a commit
writes the feed, and no second log is kept. Its guarantees are etcd's for watches: ordered (by
revision, a commit's changes in the order of its writes), reliable (from the horizon on, nothing
skipped), atomic (a page ends between commits, never inside one; a first commit over the page's
limit is refused `Invalid`), and bounded below by the feed horizon (a compaction's revision less
one), under which `changes`, `wait` and `changed` are refused `Compacted` naming it.

`changed(family, after, until)` joins the trait: `wait`'s answer if it has one now, without
waiting. It is what Amphora's async `Watch` and the transport's `TryWait` already did per
implementation; on the trait, the goal driver (G9 step 3) and the audit log build on one typed
call, with any implementation, the simulator's included.

**Sources.** PostgreSQL derives its change stream by decoding the write-ahead log, and a slot
replays it "in the order they were made on the origin server" (PostgreSQL, Logical Decoding
Concepts; research `01a16b0f473676fe9d98087817eb1057` f5, graded A). etcd's watches are
"Ordered - events are ordered by revision" and "Reliable - a sequence of events will never drop any
subsequence of events within the available history window" (etcd API guarantees; f4, graded A).

**Evidence.** The property logical decoding gives is that the stream is the commit's own record:
it can't disagree with the data and can't be lost apart from it. The version rows have that
property already: they are written in the commit's own transaction and are what snapshot reads
read. Decoding Amphora's physical log (page frames, or the replicated log's prepared pages) back
into rows would rebuild what the version rows hold, as a second path. The conformance cases
RECORDS-9, RECORDS-10, RECORDS-14 and AMPHORA-RECORDS-FEED-1 hold the guarantees on both
implementations.

**Falsified by.** A committed change that a reader of `changes` from the horizon on doesn't see,
or sees twice, or sees out of revision order; a page ending inside a commit; a read below the
horizon answered from a partial history.

## Decision (lane records-api): the simulator keeps Amphora's layout

**Decision.** `amphora_api::sim::SimRecords` keeps one namespace in maps shaped as Amphora's tables:
versions by (family, key, revision) with their place in the commit; the feed's index over the same
versions by (family, revision, place); each family's newest change; the snapshot and feed
horizons; one compaction's target, completed revision and cursor. Its commit, reads, change pages
and compaction steps follow Amphora's (`crates/amphora/src/database.rs`,
`crates/amphora/src/database/compaction.rs`) step for step, so the two give equal answers,
refusals and compaction progress included. Its bounds are the caller's (`SimLimits`: live rows,
versions kept); past either a commit is refused `Full` and writes nothing, and a compaction frees
versions. It has no defaults: a test states what it holds.

**Evidence.** `the_simulator_answers_as_amphora_does` (crates/amphora/tests/records_conformance.rs)
sends seeded histories of commits (conditions met and failed, puts, deletes, keys written twice),
reads at snapshots and at the head, scans, change pages, readiness checks and compaction steps to
both, and requires every answer equal.

**Falsified by.** Any request on which the two answer differently.

**What was given up.** kiln-seams' simulator bounds its history by count and drops the oldest on
its own (its horizon moving under the caller). This one never compacts on its own, as Amphora
doesn't: a caller that never compacts is refused `Full`, as a full disk would refuse it.

## Decision (lane records-api): one suite, ported into amphora-api

**Decision.** `amphora_api::conformance` holds the Records suite: kiln-seams' RECORDS-1 to 14
(kiln `crates/kiln-seams/src/records.rs` at 492af26f9, unchanged since 02d5f6837), each id and
`says` text as kiln has them and each check the same requests and expectations, ported to the
synchronous trait with deadlines; and Amphora's own cases after them. Amphora's `Namespace` (in
memory, on a file, and through the local owner's transport) and `SimRecords` run it.

**Why a port and not a dev-dependency on kiln-seams.** Three facts rule the dependency out:
- `scripts/vendor.py`'s pin check, run by kiln when it vendors Amphora, refuses a product that
  vendors a hyper-light product kiln doesn't hold itself; kiln can't hold a copy of itself, so a
  vendored kiln-seams in Amphora would stop kiln's next re-vendor.
- kiln-seams links aws-lc-rs and rcgen (C, built by a build script), which Amphora's cross-target
  clippy gates (Windows and macOS from Linux) would have to build.
- Its trait is kiln's async one; running it needs an adapter from this trait, which kiln already
  owns (`crates/kiln/src/records.rs`, `u06_amphora_passes_the_records_suite`).

So the port is the copy Amphora owns, and kiln-seams' is kiln's. The way to one copy is for kiln's
suite to become this one run through kiln's adapter; that is kiln's change to make.

**Falsified by.** A kiln-seams Records case added or changed that the port lacks: the port names
kiln's revision so the difference is one `git diff` away.

## Decision (lane records-api): index families and counters are rows of ordinary families

**Decision.** An index family is an ordinary family whose keys are an indexed term and then the
indexed row's key, written in that row's commit (`amphora_api::index::Index`). A counter is a row
holding the last number taken, big-endian in eight bytes, taken under compare-and-set in the commit
that uses the number (`amphora_api::counter::Counter`). Both are the layout kiln's deploy records
use (kiln's docs/deploy-api.md §14.1, agreed with Amphora's owner on 2026-10-23; research f6), made
typed so every caller encodes them one way. The store learns nothing new: a value is bytes to it,
so it couldn't derive an index's terms itself.

**Index keys.** The term is escaped as FoundationDB's tuple layer escapes a byte string (each 0x00
as 0x00 0xFF; FoundationDB `design/tuple.md`, "Byte String"), then ends with 0x00 0x01, then the
indexed key follows as it is. Inside an escaped term a 0x00 is always followed by 0xFF, so 0x00
0x01 occurs only at the term's end: keys sort as (term, indexed key) do, a term's prefix starts
that term's keys only (no other term's, however they share bytes), and a key splits one way.
`index_keys_keep_term_order_and_prefixes_reach_one_term` checks both over every term of up to three
bytes drawn from {0x00, 0x01, 0xFF, a}.

**Falsified by.** Two (term, key) pairs whose index keys sort otherwise than the pairs; a term's
scan that returns another term's row; two commits that take one counter number.
