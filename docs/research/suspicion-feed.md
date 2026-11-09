# The suspicion feed: which detector, and how its verdicts reach the held count (2026-11-06)

Source notes for docs/design/replication.md, "The suspicion feed". Kiln goal dcdd872a0ed78f37902b06dd95dd3bf1.

## What hyper-raft asks of its owner

- R-8 (hyper-raft docs/raft.md §3.7 at 4d72e19b, lines 963-965): "A member its owner says is suspected
  (`RawNode::suspect_held`) is not counted until its owner says it is trusted (`RawNode::trust_held`); the owner's
  node-level detector decides (hyper-liveness), never the group's own heartbeats."
- The core does this whatever its elections: `RawNode::suspect_held` and `trust_held` keep their own set
  (`held_commit.suspected`, bounded by the members limit), and the core's test
  `a_suspected_members_held_answers_count_only_once_it_is_trusted` (crates/hyper-raft/src/tests.rs:4869) shows a
  suspected member's held answer not counted, then counted once trusted.
- A verdict of a member the configuration doesn't name, or of the member itself, keeps nothing
  (`Raft::suspect_held`: "Of a member the configuration does not name, or this one, it keeps nothing").

## What hyper-durable gives an owner that elects on ticks

- Amphora's replicated owner elects on ticks (crates/amphora/src/database/replicated.rs, `settings`:
  `elections: hyper_raft::Elections::Ticks`).
- `Replica::suspect`, `trust`, `restarted` and `stopped` each take the held verdict and then the elections' verdict
  (`r.node.suspect(member)`), which on ticks is refused (`Error::Settings("elections run on ticks")`, not fatal, so
  `ReplicaError::Refused`). hyper-durable's own test says so: "On ticks the replica elects by the owner's ticks and
  takes no detector's word" (crates/hyper-durable/tests/shell.rs:1550), asserting `Refused` for all three.
- So on ticks those calls change the held count and then answer `Refused`: an owner would rely on a refusal's side
  effect. `Replica` has no held verdict alone; `Replica::core` is read-only.

## The detector

- hyper-liveness is vendored already (vendor/MANIFEST.toml, `take`: `crates/hyper-liveness`, at 4d72e19b), unused by
  Amphora. The design's "not vendored" was out of date.
- It is the node-level detector R-8 names: "one heartbeat stream per pair of nodes that share a consensus group,
  shared by every group they share, each heartbeat proving a recent durable flush of the sender's log", judged with
  NFD-E (Chen, Toueg and Aguilera 2002; IEEE Transactions on Computers 51(5), doi:10.1109/TC.2002.1004316) through
  hyper-timing's estimator (crates/hyper-liveness/src/lib.rs:1-20). The paper itself couldn't be read here (IEEE
  refused the fetch, the Cornell copy is gone); it is cited through hyper-liveness's docs.
- The flush proof matters for the held count: "A node whose disk stalls therefore stops heartbeating and is suspected
  as a crashed one is." A member whose disk stalls still answers held (memory), so a group's own heartbeats would keep
  counting it; the node-level detector doesn't.
- It is sans-IO: "The crate is fed `now`, messages with their kernel receive stamps (hyper-tokio's `PlaneSocket`), and
  durable completions, and returns heartbeats to queue on the datagram plane, flush requests, trust changes and the
  time to be polled next." Its heartbeats ride the datagram plane between nodes.
- Its verdicts (`Change`): `Suspected`, `Trusted`, `Restarted`, `Stopped`, `DiskSlow`, `DiskCaughtUp`, `ModeDiffers`,
  `ModeAgrees` (lib.rs:242-310).

## Who runs it

- Amphora carries no records between nodes: "Hosts carry authenticated wire records only" (replicated.rs:1);
  `Database::open_replicated`: "The host carries authenticated wire records through `replica()`". Its own transport is
  local (crates/amphora/src/transport.rs: "Narrow embedded transport for one canonical database owner").
- So the datagram plane, and the detector on it, are the host's: one per node, shared by every group the node has, as
  hyper-liveness is built to be.
- Focal's bus is the other hyper-durable owner kiln runs. It runs hyper-liveness itself and maps each `Change` to the
  replica (kiln vendor/focal-bus/crates/focal-bus/src/member.rs:1805-1840): `Suspected` to `Replica::suspect`,
  `Trusted` to `trust`, `Restarted` to `restarted`, `Stopped` to `stopped`, `DiskSlow` and `DiskCaughtUp` to the
  disk calls; it elects by suspicion (member.rs:1027).

## Why a suspected member's held answers shouldn't count

- SAUCR (Alagappan et al., OSDI 2018), which R-8 follows: "with many nodes up, SAUCR buffers updates in memory; when
  failures arise, SAUCR flushes updates to disk" (abstract,
  https://www.usenix.org/conference/osdi18/presentation/alagappan). Its leader learns failures from heartbeats and
  leaves the fast path at once: "a transition to slow mode is immediate: the first time the leader suspects that only a
  bare majority of nodes are alive" (§ "Failure Reaction"), returning only "after confirming a handful number of times
  that it promptly gets a response from more than a bare majority".
- Gray failure (Huang et al., HotOS 2017, doi:10.1145/3102980.3103005): "a key feature of gray failure is differential
  observability: that the system's failure detectors may not notice problems even when applications are afflicted by
  them" (abstract). The detector that decides must see what the held count relies on, the node and its disk, which
  hyper-liveness's flush proof does and a group's heartbeat doesn't.

## What follows

- Safety doesn't rest on the feed: every held quorum spans the fates asked, suspected or not, and the leader counts
  only its disk (raft.md §3.7). A reopened owner trusting every member until told again is safe; the feed narrows what
  a gray, failing member's held answers can commit.
