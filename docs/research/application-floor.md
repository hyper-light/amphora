# The application floor, replicated: source notes (2026-11-01)

Notes behind docs/design/replication.md, "The application floor, replicated". Kiln goal
d8362e1ea15980ce84da0b168a93bc8a; Codex (01a188136cb1758cac52149a6eea8c7b) asked Amphora to own the seam.

## What exists

**The standalone floor** is kiln's U05 patch to Amphora (Codex's), applied over 29be700 in kiln's vendor tree:
`vendor/patches/amphora/0001-u05-retained-application-writer-floor.patch` at kiln f77518103a.
- A singleton table, `__amphora_application_floor(application, version > 0)`.
- `Database::for_application(name, version)` binds a handle's claim. It installs nothing.
- `writer_floor(name, required, until)` raises the floor, monotonically, in the original local writer. It answers after
  the durable callback and returns a `WriterBoundary`. It refuses any owner but the original standalone one: "Replication
  and remote-owner takeover need their own application-floor proof and are deliberately not certified by this API."
- `application::check(db, claim, operation)` runs inside every application transaction. It refuses, `Refusal::Fenced`, a
  missing claim or one older than the floor. The open's empty-SQL barrier is excepted.
- `require_replication` refuses replicated mode while a floor is installed: "application writer floor requires a
  member-proven replication contract".

**Replication at 29be700** (docs/design/replication.md, crates/amphora/src/replication.rs, database/replicated.rs):
- An entry carries resolved pages and the index and term of the state its session observed.
- `Machine::apply` applies the pages only when that base equals the applied point. Otherwise it records the refusal "the
  database advanced since preparation", and the index still advances.
- `apply_change(at, &ConfChangeV2, &ConfState) -> Result<(), Fatal>` advances the point to the change's index and writes
  the configuration with it. The machine can't refuse a configuration change: an error there is fatal.
- `step(from, bytes)` takes `from` from the authenticated transport session. It refuses a record whose `message.from`
  differs ("authenticated sender or destination differs from record"), or whose sender isn't in the voters, learners,
  outgoing voters or next learners ("sender is not a current member").
- `context()` builds a request's context in the proposer itself: its replica id, term, incarnation and serial. Nothing
  checks a forwarded proposal's context against the authenticated sender.
- Public SQL can't name an `__amphora_*` table (`sql::validate_reserved_sql`: "__amphora_* names are reserved for product
  metadata"). Only Amphora's private Db SQL writes them.

## Raft

Ongaro and Ousterhout, "In Search of an Understandable Consensus Algorithm (Extended Version)",
https://raft.github.io/raft.pdf.
- Section 6, p. 11: "Raft introduces an additional phase before the configuration change, in which the new servers join
  the cluster as non-voting members (the leader replicates log entries to them, but they are not considered for
  majorities). Once the new servers have caught up with the rest of the cluster, the reconfiguration can proceed as
  described above."
- Section 6, p. 11: "Once a given server adds the new configuration entry to its log, it uses that configuration for all
  future decisions (a server always uses the latest configuration in its log, regardless of whether the entry is
  committed)."
- Section 7, p. 12: "the snapshot also includes the latest configuration in the log as of last included index."

## CockroachDB's cluster version

"Version migration for backwards incompatible functionality",
https://github.com/cockroachdb/cockroach/blob/master/docs/RFCS/20170815_version_migration.md:
- Summary: the bump "represents a promise that the old version is no longer running on any of the nodes in the cluster".
- Upgrade process: "Verify no node running the old version remains in the cluster (and no new one will accidentally be
  added)."
- Rationale: "relying on the operator to guarantee that a cluster is not running mixed versions when the explicit version
  bump is issued."
- Server configuration: "If a server starts up with a store that has a persisted version smaller than this" ... "it exits
  with an error." Storage: "unless the binary can't support it, in which case it commits suicide".
- The ClusterVersion proto: "This value must monotonically increase." Rationale: "no support for downgrades once the
  version setting has been bumped."

"Finalize cluster upgrade automatically" (Status: in-progress; Start Date: 2018 4/11),
https://github.com/cockroachdb/cockroach/blob/master/docs/RFCS/20180411_finalize_cluster_upgrade_automatically.md:
- It finalizes once all live nodes run the new version and, from node liveness, "all non-decommissioned nodes are alive
  (no missing nodes)".
- It leaves a race open, "Race condition [#24670] (future work)": "if right before the command get executed, the operator
  erroneously joined another node running 1.1 version" ... "the node running 1.1 version can not perform some of the 2.0
  version operations and it will cause trouble."

## etcd's cluster version

"Upgrade etcd from 3.0 to 3.1", https://etcd.io/docs/v3.6/upgrades/upgrade_3_1/, "Mixed versions":
- "an etcd cluster supports mixed versions of etcd members, and operates with the protocol of the lowest common version."
- "The cluster is only considered upgraded once all of its members are upgraded to version 3.1."
- "Internally, etcd members negotiate with each other to determine the overall cluster version."

## What they add up to

- Every system here raises a cluster-wide version only on evidence that every member runs it, never lowers it, and has a
  member that can't run the version refuse to serve.
- CockroachDB checks its evidence outside the log, so a member can join between the check and the bump. Its own RFC leaves
  that race open.
- In Amphora the check can be one transaction's: the proof is read in the session that raises the floor, and the stale-base
  rule applies that session only if nothing, configuration changes included, committed since it read.

## For questions a to d (2026-11-06)

Codex's agent ended before answering; Amphora's membership-floor agent decided a to d from these.

Ongaro and Ousterhout, raft.pdf, section 2 (Replicated state machines): consensus algorithms for practical systems
"ensure safety (never returning an incorrect result) under all non-Byzantine conditions, including network delays,
partitions, and packet loss, duplication, and reordering." Nothing in Raft authenticates a message; Amphora's `step`
binds the sender to the transport's authenticated identity.

Ongaro, "Consensus: Bridging Theory and Practice" (2014), https://github.com/ongardie/dissertation:
- Section 3.10, Leadership transfer extension (basicraft/consensus.tex): a leader "must step down" when "it may be
  removed from the cluster"; "The prior leader stops accepting new client requests", "fully updates the target
  server's log to match its own", and "sends a TimeoutNow request to the target server"; "If leadership transfer does
  not complete after about an election timeout, the prior leader aborts the transfer and resumes accepting client
  requests."
- Section 4.1, Safety (membership/safety.tex): "each server always uses the latest configuration found in its log";
  "a log entry for a configuration change can be removed (if leadership changes); in this case, a server must be
  prepared to fall back to the previous configuration in its log."

CockroachDB, RFC 20170815_version_migration:
- Rationale: "no support for downgrades once the version setting has been bumped."
- Server configuration: a server has a `MinimumSupportedVersion`; "If a server starts up with a store that has a
  persisted version smaller than this or larger than its `ServerVersion`," "it exits with an error." The lower end
  guards a binary against old data; Amphora's floor guards data against old writers, so it needs only the upper end.

Kiln's U05 patch (kiln f77518103a): `Application { name, version: u32 }`; `check` refuses a missing claim ("application
writer contract required") and a claim below the floor; `require_replication` refuses replicated mode with a floor
("application writer floor requires a member-proven replication contract"), and is called on the remote owner's paths
as well as replication's.

The membership code the floor guards, t1/membership-fixes at 49bf342 (crates/amphora/src/database/replicated.rs):
`member_change` refuses while `has_pending_conf` or a joint configuration holds, at a ReadIndex barrier;
`ReplicaHandle::transfer` hands the lead on so the leader can be changed; `Machine::admits_joiner` refuses a learner
past the snapshot ceiling; `a_removal_another_lead_wrote_over_gives_the_member_back` pins the fallback above.
