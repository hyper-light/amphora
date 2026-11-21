# Groups that split, merge and move themselves (CP-T1.9)

## Decision (lane split-merge): split, merge and move as one fenced hand-off, decided by measured share

Kiln goal 49477403 (G13 step 2, kiln plan `01a16b0f47b771b683ed0b11157f72f5`): "By size and load,
fenced, copy then catch-up then a brief cutover. Done when: cutover pause p99 on a 10 GB partition
is at most 250 ms; writes lost over 1,000 automatic operations is exactly 0. Tests: simulator and
lab: passes when every key in exactly one group." Requirements AC-T1.9.1 to AC-T1.9.3, and this
lane's T-T1.9.S1 (`docs/requirements.toml`).

The code is a pure core (CLAUDE.md §9), `crates/amphora/src/reshard/`: the directory, a group's
holding, the planner and the driver. It has no clock, disk or network; a shell carries out its
actions. The simulator in `crates/amphora/tests/reshard.rs` is the first shell. The shell over
Amphora's groups and replicated owners is not built yet (see "What is left").

### The directory

A key's position is its FNV-1a hash: the same hash `crate::groups` spreads keys by today
(store.md, decision 10). Kiln's ids are time-ordered, so ordering groups by key would send every
new row to the last group, and no split could spread that load. Ordering by hash spreads them.
A span is a run of positions, `lo` to `hi` inclusive. The directory keeps the spans in position
order. Each span starts right after the one before it, the first at 0 and the last ending at
`u64::MAX`. Every group holds at most one span. A change that would break any of this is refused
whole (`Directory::check`), so every position belongs to exactly one group after every change.
A database starts as one group holding every position. Nobody chooses a shard count or a split key
(AC-T1.9.1).

Splits, merges and moves are all the same operation: span S goes from group A to group B.
- A split moves the upper part of A's span to a new group B on A's node.
- A merge moves all of A's span into B, the group next to it, and A is retired.
- A move sends all of A's span to a new group B on another node, and A is retired.

The directory is a deterministic state machine of changes: `Begin`, `Cutover`, `Flip`, `Abort` and
`End`. The shell makes each change durable before it carries out anything that follows it. The
operation's one commit point is `Flip`. Before the flip, the operation can be given up and nothing
has moved. After the flip, it only goes forward. A group is in at most one operation at a time.
Operation ids and group ids only grow and are never reused.

### A group's holding

Each group keeps its own record of what it holds, in its own state machine and durable with its
rows:
- the span it holds;
- the part of that span it has fenced for a cutover, with the position of the fence entry in its
  own log;
- the span it is taking in, with its copy and catch-up progress;
- the last operation that ended at this group.

A group answers a key only from this record. It never trusts a router's directory. So a stale
router, or a stale action from a driver that has since restarted, can never make two groups take
writes for one key.

A fenced span refuses reads as well as writes. After the flip, the new group may already have
newer writes, so a read at the old group could be stale.

The holding rejects stale actions using one number: the last operation that ended at this group.
That works because each group is in at most one operation at a time and operation ids only grow.
Rows are swept only if they are garbage: neither held nor being taken in. So a late or repeated
sweep can't delete a row the group holds again after a later merge.

### Copy, then catch-up, then a brief cutover

This is Vitess's VReplication model (copy, then catch up from the replication position;
https://vitess.io/docs/reference/vreplication/vreplication/; plan finding f6, graded A).
1. **Copy.** B copies S in batches. Each batch reads A's rows at A's current log position `P_i`
   and stamps every copied row with `P_i`. The first batch's position, `P_1`, is where catch-up
   starts.
2. **Catch-up.** B replays A's log from `P_1` forward, in order. It applies a change to a row only
   if the change is newer than the row's stamp (`holding::replays`). A replayed row is stamped with
   its log entry's position. Why this works: every copied row reflects every change up to its
   batch's position, and every row change after `P_1` is replayed in order. So once replay reaches
   entry E, B holds S exactly as A held it at E. Deletes need no tombstones, because replay only
   starts after the copy ends, so no copied batch can bring back a row that replay already deleted.
3. **Rounds.** Catch-up runs in rounds. Each round replays up to where A's log ended when the
   round began. This is Raft's rule for catching up a new server (Ongaro's dissertation, section
   4.2.1, which `replication.md`'s learner catch-up already follows). When a round no longer
   shrinks the lag, A fences: either the lag is zero, or the writes made during a round now take as
   long to replay as the round itself. More rounds would not make the final replay smaller. No
   count of rounds and no lag limit is picked.
4. **Cutover.** A writes a fence entry at position F in its own log. From then on, A refuses S.
   B replays up to F. The directory flips S to B. B takes S up, A lets S go and sweeps its rows,
   and a group left holding nothing is retired. The pause, while S takes no writes, lasts from the
   fence until B takes S up. Its work is the entries between where replay stood and F (the driver
   reports this as `fence_lag`), plus one directory commit and one entry at B.
5. **Abort.** If anything fails before the flip, the operation is given up. A's fence is lifted,
   B's copy is thrown away (its rows swept), and a group made for the operation is retired. One
   example: A's log no longer reaches back to where catch-up stands (`Failure::LogGone`).

After a restart, the driver takes every operation up again from its phase in the directory, and
asks each group where it stands. Every action is idempotent at its group. Every answer carries
the ask it answers (the driver's epoch and a count), and the driver ignores any answer except the
one to an operation's last ask.

### When: from what is measured

Each time it is asked, the shell measures and passes in each group's bytes and load over its last
window, and each node's cores. Load is anything that adds up across groups, such as writes or the
writer's busy time. With `C` as the measured cores of every node:
- **Split** a group whose load times `C` is more than the total load, or whose bytes times `C` is
  more than the total bytes. Such a group holds more than one core's share. A group has one writer,
  so past that share it holds back a core another group could use. Store.md's decision 10 measured
  write rate rising with groups up to the cores: 1 group 34,918 rows a second, 12 groups 254,430,
  on 32 cores. The cut falls where the group's measured load, or its rows, split half each side
  (`plan::cut`, which needs weight on both sides).
- **Merge** two neighbouring groups whose combined load and combined bytes are each at most half a
  core's share. The factor of two is the split's own (a split halves). It keeps a merge from
  undoing a split while the work holds still. This is the dynamic table's rule: grow when full,
  shrink at a quarter rather than a half, so no run of changes thrashes (Cormen, Leiserson, Rivest
  and Stein, Introduction to Algorithms, 3rd ed., section 17.4.2). When nothing is merged, every
  pair of neighbours holds more than `1/(2C)` of the load or of the bytes, so there are at most
  `8C + 1` groups.
- **Move** a group from the node with the most load per core to the node with the least, but only
  if after the move the second node is no busier per core than the first. A move never overshoots,
  so a group never bounces back and forth. A node with no cores (being removed) gives up its groups
  first, as far as the others will take them.

Every value is measured as the system runs (bytes, load, cores) or follows from the rule's
structure (the split's factor of two). None is measured once and frozen. A node with more cores
attracts more groups, and a busier group splits sooner, on any hardware.

### The simulator

`t_t1_9_s1_every_key_in_exactly_one_group_through_automatic_operations` runs the core in a
deterministic simulator:
- **Clients** write, delete and read keys through a router whose directory may be stale. Most
  writes hit a run of hot keys that moves now and then. The mix leans to inserts or to deletes.
- **The cluster** changes: nodes join, are removed (no cores) and come back, and their cores
  change.
- **The planner** runs on each window's measured load and sizes.
- **The nemesis** restarts the driver between any two of its steps, with its stale actions still
  on their way. It also restarts groups, losing what they were asked, and takes an old group's log
  away under a catch-up.
- **After every step,** it checks:
  - every position is in exactly one span of the directory;
  - no key is answered by two groups;
  - the group answering a key is the one the directory names, and it holds the last write
    acknowledged for that key.
- **At every quiet point,** it checks:
  - every key's row is in exactly one group, the directory's;
  - each row holds the last write acknowledged for it, and no deleted key is anywhere;
  - no row is left outside its group's span;
  - the directory's groups are exactly the groups that exist.

  Writes lost must be 0. As non-vacuity checks, every seed must complete at least one split, one
  merge and one move, and give up at least one operation.

### Falsified by

- Any of the following, in the simulator or the lab: a position in two spans or in none; a key
  answered by two groups; a write acknowledged and then missing.
- A merge undoing a split under steady load.
- A group moved back to the node it left under steady load.
- The cutover pause on a 10 GB partition above 250 ms at p99 (AC-T1.9.2), with the fence lag the
  driver reports showing the final replay to be the cause.

### What is left

1. **The shell over Amphora's groups.** Each group needs:
   - rows indexed by position, so a span can be copied and swept in position order;
   - a change feed by log position (the engine's row-level redo at commit, engine.md);
   - the holding kept in the group's state machine, so it is replicated with its log;
   - the old group's log held from `P_1` until the operation ends.

   Built (lane split-merge-4): the directory is a replicated group's state,
   `reshard::replicated::DirectoryMachine`, a hyper-durable state machine whose entries are the
   directory's changes; like focal's control groups it keeps nothing durable of its own and is
   replayed from the log, and every entry is acted on at start. Its leader runs the driver with its
   term as the driver's epoch (`DirectoryMachine::lead`). `Groups` routes by the directory
   (`Groups::route` takes a newer generation), and `Groups::start(n)` makes its `n` groups by
   splits of the one group every database starts as. Each group checks every write and read
   against its holding (`Groups::hold` carries out an action on it, under the route's write lock,
   so every batch let in before a fence is queued before it); a refused key is
   `store::Refused::Away`. Still left of this item: the holding is in the router's memory, not yet
   durable in the group's own state with its rows; rows by position; the change feed; the old
   group's log held from `P_1`; and the shell that carries the driver's actions to the groups. A
   member that installs a directory image holds it in memory only, so it must be caught up again
   from its leader after a restart: persisting images comes with the holding's durability.
2. **Load measured from the store's meter.** The writer's busy time per window, a bounded sample
   of written positions for the load cut, and the size cut from the B-tree's middle key. The
   window is the time between planner runs, and shares don't depend on its length.
3. **`bench reshard`.** The cutover pause on a 10 GB partition under load (AC-T1.9.2), and the
   three-machine lab's 1,000 operations (AC-T1.9.3).
4. **Metrics.** Operations by kind and outcome, and the fence lag. Their names still need
   agreement with kiln's Observe owner (CLAUDE.md §10).
