# Regions

## Decision (lane regions): region-local groups under a global root, placed by declared survival

CP-T1.11, G14 step 1 (kiln plan `01a16b0f47d5716eb35fedaf4f8c8410`, research
`01a16b0f47d5716eb35fedb0ee9c79b4`). Code: `crates/amphora/src/regions.rs` and
`crates/amphora/src/regions/lab.rs`; tests: `crates/amphora/tests/regions.rs`; the lab:
`crates/amphora/examples/regions_lab.rs`.

### What is decided

- **A region label on every member.** The operator names a member's region when it joins
  (`Members::join`). Nothing reads a region from a host name.
- **Placement rules, from declared failures.** A group's voters follow from how many failures it
  is declared to survive: a quorum of `2f + 1` survives `f` lost (Raft's majority).
  - A *regional* group (`Rule::Home`) has all `2f + 1` voters and its lead in its home region.
    It commits at that region's latency. CockroachDB does the same when it pins a partition to a
    region ([crdb]: "tables can be partitioned by access location with each partition (set of
    Ranges) pinned to a specific region.").
  - The *global root* (`Rule::Global`) has `2f + 1` voters in each region that can hold them,
    over at least `2r + 1` regions, so losing any `r` regions still leaves a majority. Its lead is
    in the primary region when one is named. This is CockroachDB's region survival goal. The
    root holds the directories and global rows: data that is rarely written or not tied to one
    region, which is what CockroachDB's global tables are for ([crdb]: "useful for data that is
    infrequently updated or cannot be tied to specific geographies.").
- **Placement under loss moves only what was lost.** `place` keeps every prior voter that still
  fits and fills the gaps from the same region, in node order. A regional group never borrows a
  voter from another region: with too few members left it refuses (`Refused::TooFewMembers`) and
  keeps its placement, which still has a quorum if a majority is live.
- **Routing by home region.** Each database region has its own directory, a `Directory` like any
  other, so each region reshards on its own with CP-T1.9's operations and `reshard/` is
  unchanged. A regional row (CockroachDB's `REGIONAL BY ROW`) is written to its home's group: by
  default the writer's own region. A read that doesn't name a home tries the reader's region
  first, then the others in region order, as CockroachDB's locality-optimized search does
  (`Atlas::read`). Global rows go to the root's directory.
- **One code path at every scale.** The database's regions are the same on one node, on three
  nodes and on three regions. A database region with no members of its own label is placed in
  the primary region (the first one given). That is the only choice a smaller topology adds
  (CLAUDE.md §8), so a corpus's answers and rows are the same at every scale.

### The lab

`regions::lab` is deterministic: no clock and no randomness. Round-trip times are its input
(`Latency { within, between }`), so the code picks no latency.

- A write commits after the client's round trip to the lead plus the lead's round trip to the
  voter that completes its majority.
- A read is answered by the lead.

The coordinator owns the 60 ms run and its p99 (AC-T1.11.2):
`cargo run --release -p amphora --example regions_lab -- <within µs> 60000 <ops> <keys> <groups> <seed>`.
The within-region round trip comes from the lab's network.

### Evidence

`crates/amphora/tests/regions.rs`:

- The same 400-operation corpus, over 20 seeds, gives equal answers and rows at one node, three
  nodes and three regions (AC-T1.11.4, on the lab).
- Placements stay inside their rules when members are lost, and when a region is lost the root
  stays quorate.
- In a tiny lab run, regional writes commit within their region and global writes pay the
  between-region round trip.

### Falsified by

- A regional write whose quorum waits on a voter outside its home region.
- A corpus whose answers differ between scales.
- A root that loses its quorum with fewer regions lost than it was declared to survive.
- A regional commit p99 above 10 ms in the 60 ms lab, measured end to end once the replicated
  shell carries groups across members.

### What is left

- The SQL corpus (`bench/corpus`) at three regions needs groups replicated across members by the
  replicated shell. Today the lab drives this routing and placement core with a record corpus.
- The region-loss drill with real logs (G14 step 2, AC-T1.11.3).
- The root keeping each region's directory as its records. Today `Atlas` takes the directories
  as given.

[crdb]: Taft et al., "CockroachDB: The Resilient Geo-Distributed SQL Database", SIGMOD 2020,
https://www.cockroachlabs.com/pdf/cockroachdb-the-resilient-geo-distributed-sql-database-sigmod-2020.pdf
