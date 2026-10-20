# Amphora's design

The target, from track T1 of kiln's critical path (kiln plan `01a157fdefba746a94a5fcd920a80d5a`). Each decision below is
made by the goal named beside it, recorded here with its sources, its evidence and what would
falsify it (CLAUDE.md §5).

## What it is

Amphora is the store for every record kiln keeps, and for the records of the products kiln is
built from. Records and bytes stay apart: Amphora keeps records, and Mantle keeps bytes (its
backups, too, go to Mantle).

It holds three things:
- `amphora`: the embedded database: the engine, typed records, transactions, change feeds,
  replication, placement and sharding;
- `amphora-api`: the traits others use, with a simulator implementation;
- the `amphora` binary: a server, speaking the client protocol CP-T1.2 decides.

Kiln vendors it at `vendor/amphora` and reaches it only through kiln-seams' `Records`.

## The target

- **One code path.** A laptop runs every group with one voter, a fleet with three, and a region
  set runs region-local groups under a global root. The same differential suite passes at one
  node, three nodes and three regions. Each step up in scale adds only the choice it can't avoid;
  nobody picks Raft terms, shard counts, split keys, WAL segments or replica placement to set it up.
- **The write-ahead log is mandatory.** The Raft log (hyper-log, through hyper-durable's
  `GroupStore` on a real device with the full flush) is the only write-ahead log, and Turso is each
  group's state machine, durable before the log compacts past it. Until CP-T1.8 the single node
  keeps Turso's WAL with `SyncMode::Full`. `RamStore` exists only under `cfg(test)`.
- **Many readers and many writers.** Readers on MVCC snapshots never wait behind writers; writers
  are concurrent across and within groups, with adaptive group commit and no single writer thread;
  serializable within a group and atomic across groups.
- **Groups.** One per session, one per deploy environment, one per CI repository; a root group for
  the fleet's records, global and replicated across regions; directory partitions for search.
- **Placement and sharding after Vitess and PlanetScale:** keyspaces with a VSchema, primary and
  lookup vindexes, automatic split and merge with fenced transfers, VReplication-style moves,
  cells to contain failures, and leaders that follow locality.
- **Turso, with carte blanche.** Amphora owns its copy (`vendor/turso`): each change has its row in
  `UPSTREAM.md`, keeps Turso's own suites passing, meets this repository's rules, and is offered
  upstream.

## Decisions

| Decision | Made by | Status |
|---|---|---|
| The commit pipeline: adaptive group commit, its batch budget | CP-T1.1, CP-T1.4 | open |
| The MVCC scheme: SSI or OCC with precision locking | CP-T1.2, CP-T1.3 | open |
| I/O per platform: io_uring and direct I/O, `F_BARRIERFSYNC`/`F_FULLFSYNC`, `FlushFileBuffers` | CP-T1.4 | open |
| The client protocol: libSQL's Hrana, the Postgres wire protocol, or both | CP-T1.2 | open |
| Replication: hyper-raft groups, the Raft log as the write-ahead log, commit classes | CP-T1.8 | open |
| Placement and sharding: the directory, split and merge thresholds, cells | CP-T1.9 | open |
| Transactions across groups and follower reads | CP-T1.10 | open |
| Regions: region-local groups, the global root, the multi-region class | CP-T1.11 | open |
