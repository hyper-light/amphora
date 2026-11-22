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

A key's position is its FNV-1a hash (store.md, decision 10) finished by MurmurHash3's
64-bit finalizer (`fmix64`, SMHasher): spans cut by high bits, and FNV-1a alone leaves a change
in a key's last byte out of its top bits, so `k0` to `k19` all fell in one quarter of the space
(found by `tests/groups.rs` once `Groups` routed by spans). Kiln's ids are time-ordered, so ordering groups by key would send every
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
   directory's changes; it keeps nothing durable of its own but the last image it installed, and
   is replayed from the log after it, and every entry is acted on at start. Its leader runs the driver with its
   term as the driver's epoch (`DirectoryMachine::lead`). `Groups` routes by the directory
   (`Groups::route` takes a newer generation), and `Groups::start(n)` makes its `n` groups by
   splits of the one group every database starts as. Each group checks every write and read
   against its holding (`Groups::hold` carries out an action on it, under the route's write lock,
   so every batch let in before a fence is queued before it); a refused key is
   `store::Refused::Away`.

   Built (lane split-merge-5): each group's holding is kept by the group's own engine with its rows
   (`Engine::holding`, `Engine::keep_holding`). A change of it is kept after every batch let in
   before it, and durable with them, before the group acts on it (`Groups::hold`); if it can't be
   kept, the group holds what it held and the driver asks again. A group started again answers from
   what it kept, whatever directory it is started by (T-T1.9.R1, T-T1.9.R3). A directory member
   keeps the image it installed in a file of its own, replaced whole and flushed before `install`
   returns (hyper-durable's I8), and opens at it (`DirectoryMachine::open`), so a restart needs no
   catch-up (T-T1.9.R2). Amphora's own engine (`database::Local`) doesn't keep a holding yet: it
   becomes a group's with the shell below.

   Built (lane split-merge-6): the copy path. An engine that can be a group's keeps a log of its
   commits: each commit of rows, and each kept holding (a fence's entry), is one entry numbered in
   order (`Engine::head`). It reads a span's keys in position order (`Engine::export`, rows indexed
   by position), and the keys a run of its entries changed, as they stand now, stamped with its head
   (`Engine::changed`, the change feed by log position, or none once the log no longer reaches back);
   the new group writes a key only over an older stamp (`Engine::import`, `holding::replays`). The old
   group's log is held from the first copy's read (`Engine::keep_log`), on the same turn of its
   writer as that read, and let go when it releases or unfences. A key read as it stands at head `H`
   is the key as of every entry up to `H`, so the feed needs only the keys, not their values.
   `Groups::carry` is the shell: it carries each of the driver's actions to this node's groups
   (create, copy, catch-up, fence, activate, release with its sweep, unfence, discard, retire) and
   answers it. A fence's entry is numbered on the group's writer, after every batch let in before
   it. A batch is as many positions or entries as hold each writer it runs on
   about as long as a commit there does, learned from the batches run before (decision "Batches
   learned where they run", below). A split or merge carried
   through it moves the range's rows while writes go on (T-T1.9.R4), and the harnesses for the
   cutover pause (T-T1.9.P1, AC-T1.9.2) and the operations' lab (T-T1.9.L1, AC-T1.9.3) run on it.

   Built (lane split-merge-7): batches learned where they run, and the groups retired on a node
   kept durable (the two decisions below, T-T1.9.R6). Amphora's own engine (`database::Local`)
   keeps a group's holding through a command of its own, `Mutation::Holding`, appended to the
   wire, applied by the same `apply` a local writer and a replicated owner's log entry run
   (decision "A local database keeps its holding through its command", below; T-T1.9.R7).

   Built (lane split-merge-8): every copy-path batch is bounded by the process's memory as well
   as by time (decision "A batch bounded by the process's memory", T-T1.9.R8). `database::Local`
   is a group's engine in full: a row routes by its namespace, family and key, and from its first
   holding it keeps a log of its commits and its Records rows indexed by position, and carries the
   copy path over them (decision "A row routes by its namespace, family and key", T-T1.9.R9).

   Built (lane split-merge-9): moves to another node. A copy or catch-up batch reads its old group
   through a pull (`groups::Pull`), answered by `Groups::serve` at the node the old group is on, on
   its writer, through `groups::Peers` when that node is another; a node starts only the groups the
   directory places on it (decision "Moves across nodes", T-T1.9.R10).

   Built (lane split-merge-10): a pull over the owner transport, its appended request and answer,
   between a node's endpoint and its peers (decision "Pulls over the owner transport", T-T1.9.R12).

   Built (lane split-merge-11): the routing key is bytes, and a Records transaction routes by
   every key it names, so it stays within one group (decision "The routing key is bytes",
   T-T1.9.R13); local databases as groups split and merge under Records writes.

   Still left of this item: a `Database` over `Groups<Local>` (a command routed by its row's key,
   which needs the store's routing key to be bytes, not a `&str`, and a Records transaction kept to
   one group), with a split and merge of it under writes through `carry`. Built (lane split-merge-13):
   the transport between machines (decision "The transport between machines", T-T1.9.R18). Built (lane split-merge-12): a retired group's files, named to
   the shell's caller, who deletes them (decision "A retired group's files", T-T1.9.R16).
2. **Load measured from the store's meter.** The writer's busy time per window, a bounded sample
   of written positions for the load cut, and the size cut from the B-tree's middle key. The
   window is the time between planner runs, and shares don't depend on its length.

   Built (lane split-merge-12): load from the store's meter (decision "Load from the store's
   meter", T-T1.9.R14, T-T1.9.R15). `Groups::measure` gives each group's load (its writer's commit
   time over the window) and its load cut (where the keys written to it fall), and `groups::room`
   this machine's cores; the operations' lab plans from them. A group's size is what its engine says
   its rows hold (`Engine::bytes`; a local database's pages in use, T-T1.9.R17). Built (lane
   split-merge-15): the size cut from the B-tree's middle key (decision "The size cut from the
   B-tree's middle key", T-T1.9.R19).
3. **`bench reshard`.** The cutover pause on a 10 GB partition under load (AC-T1.9.2), and the
   three-machine lab's 1,000 operations (AC-T1.9.3).
4. **Metrics.** Operations by kind and outcome, and the fence lag. Their names still need
   agreement with kiln's Observe owner (CLAUDE.md §10).

## Decision (lane split-merge-7): batches learned where they run

A copy, catch-up or sweep batch runs on a group's writer, and every write to that group waits
behind it. So a batch should hold the writer about as long as one of the writer's own commits
does: then a write waits for a batch no longer than it already waits for a commit. Both sides of
that are measured where they run, never picked:
- the target is the writer's mean commit time (its meter's commit time over its commits); a group
  that hasn't committed yet uses the mean over this node's groups (one machine, one disk);
- the cost of a unit (a position read or swept, a log entry replayed, a moved key written) is the
  time every batch of that kind held that writer, over its units, so it moves as the load and the
  rows move.

A batch is the target over a unit's cost, at least one unit. A copy or catch-up runs on two
writers (the read on the old group's, the write on the new one's), so it takes the smaller of the
two. The first batch of a kind, before any cost is measured, is one unit. On a node where nothing
has committed, no write is waiting, and a batch doubles from the last, as TCP's slow start probes
for room (RFC 5681, section 3.1). This replaces the store's `commit_rows` as the batch size: that
counted rows, not time, and a copied position costs nothing like a committed row.

## Decision (lane split-merge-7): retired groups kept

A merge or a move retires a group. A stale driver can ask again for the Create that made it.
Before this, a node kept the groups it retired only in memory, so after a restart such a Create
made an empty group with the retired id. Now each node keeps the groups it retired in one file of
its own (`Groups::on`'s `retired`), replaced whole and flushed before the retire is answered. A
restart reads it back: a retired group is neither started nor made again, and a Create of it is
refused as of an ended operation (T-T1.9.R6). The file keeps the directory the node routed by when
it was written, with the retired groups that directory doesn't show retired (its id given out, the
group placed nowhere). A restart routes by the newer of that directory and the one it is started
by, so a router never goes back past what it routed by, and every group retired here shows retired
by the one or the other. The file stays bounded: an id leaves it once a directory kept with it
shows the group retired. So it holds one directory and no more ids than this node's directory lags
behind its own retires.

## Decision (lane split-merge-7): a local database keeps its holding through its command

A group's holding must be kept with its rows, in its log, never written beside it. Amphora's own
engine keeps it as one more command of its database, `Mutation::Holding`, appended after every
other variant of the wire (postcard by variant index, so the wire only grows). It is applied by
the same `apply` that a local writer's commit and a replicated owner's log entry run: one row of a
table of its own, replaced whole. So a replicated database keeps a holding through its log like
any write, and a local one as one commit of its writer, flushed with every commit before it
before the shell acts on it (`Engine::keep_holding`). A remote client can't send it: a holding is
the group's own engine's.

## Decision (lane split-merge-8): a batch bounded by the process's memory

A copy, catch-up or sweep batch is held in memory whole: its keys and rows are read out of one
group and written into another. A batch learned from time is bounded by the time a commit takes,
but on a node where nothing has committed a batch doubles from the last with no end, so a long
copy on an idle node would grow its batch until the process runs out of memory: a resource with no
bound (CLAUDE.md §2). Now every batch also holds no more units than the process's memory budget
takes at the most bytes a unit of that work has held here.

- The budget is the one the process already sets aside for what its databases hold in memory at
  once: its pool of pages, a quarter of the memory the process may use, from its control groups'
  limits and its machine's memory, with that decision's fallback where the system doesn't say
  (docs/design/store.md, decision 26; `crate::memory::limit`). It is measured where the process
  runs, at start, never picked.
- A unit's bytes are learned from the batches run: each moved key's size and its rows' bytes, the
  most a unit held in any batch of that work on that group, so a batch of larger rows is smaller.
  A sweep's position holds the keys the group's own copies moved, so it is bounded by the most a
  unit of any work there held.
- The first batch of a work is one unit, as before, so its bytes are measured before the bound
  applies; a unit larger than the whole budget is still taken one at a time.

**Evidence.** `groups::tests`: with no commit measured, sixty-four batches of 4 KiB units under a
budget of 1 MiB double 1, 2, 4, 8 and stop at 256 units, never past it; a batch learned from time
whose units are too large is cut to what the memory holds (T-T1.9.R8).

**Falsified by.** A copy, catch-up or sweep batch whose keys and rows hold more bytes than the
budget; or, at equal load, a copy slower with the bound than without on a node whose memory the
batches don't approach (the bound should never bind there).

## Decision (lane split-merge-8): a row routes by its namespace, family and key

A group's rows must be indexed by position, so a span can be copied and swept in position order.
Amphora's own engine (`database::Local`) routed every command by one key (`"database"`), so a
database was one position and could never be split. Its Records rows now each have a position:
the position (`reshard::position_of`) of the row's namespace, family and key, written as one
string of bytes (`reshard::row_key`: the namespace and the family each preceded by its length as
a little-endian u32, a format value that makes the three parts unambiguous, then the key).

**Why this key.** It is how Records already addresses a row: a row is a key in a family of a
namespace (`amphora-api`'s `Records::get(family, key)` on a namespace; `__amphora_versions`'s
primary key is (namespace, family, key, revision); docs/design/records.md). Every version of a
key then lands in one group, so a read at any revision, a condition and the feed's order within a
key are each one group's. Spreading by key, not by namespace or family, spreads one busy
namespace or family over groups, which a prefix of the key would not (a hash's spans are what the
planner splits by measured load; decision "split, merge and move as one fenced hand-off").

**Built.** A local database becomes a group's when it first keeps a holding: in that commit its
log starts and every Records key it holds is indexed by position (`__amphora_positions`); from
then on each commit is one entry of its log, each Records key a commit writes (through a
transaction or a bulk import) is indexed, and while the log is held, noted with its entry in the
change feed (`__amphora_group_feed`). The copy path moves a key's every version; the namespace's
head is raised to the newest revision moved in, and its live keys are counted again, so the new
group's next revision is newer than every row it took (`database/group.rs`; T-T1.9.R9). A
database that is no group's pays one lookup a write of a log row it doesn't have.

**Reversible.** The position is derived, never stored as a key: the index and feed tables are a
group's alone, rebuilt from `__amphora_versions` when a database becomes a group's. Another
routing key means another `row_key` and the index built again, with no change to the rows.

**Needs Ada's word.** This is the reversible option the evidence favours, taken by lane
split-merge-8 without her. What it leaves open:
- A namespace's revisions are one counter today. Once its keys are in more than one group, each
  group counts its own: a revision orders the writes of one key, and of one group, but not of the
  whole namespace, so `changes` and `head` over a namespace split across groups need the
  cross-group order of CP-T1.10.
- A Records transaction whose keys fall in more than one group is refused, as any batch across
  groups is (`store::Refused::Groups`), until CP-T1.10's transactions across groups.
- SQL tables (`Mutation::Sql`, `Migrate`, sessions) have no row key: they stay in the group the
  database's first group holds, and don't move.

**Falsified by.** A Records key whose versions are found in two groups, or in none, after a
split, merge or move; a read at a revision answered differently after a move; a new group's next
revision of a namespace not newer than one it took.

## Decision (lane split-merge-9): moves across nodes

A move's new group is on another node than the old one, so its copy and catch-up can't read the old
group's engine in the same process. Every copy and catch-up batch now has two halves:
- The old group's half is a pull (`groups::Pull`): a copy's next batch of positions, or the keys a
  run of the old group's log changed. `Groups::serve` answers it at the node the old group is on, on
  that group's writer, as the path within one node did: the operation's first read holds the old
  group's log from the entry it reads at, on the same turn, and the batch is the fewer of the units
  asked and those the old group's writer takes in about a commit's time (decision "Batches learned
  where they run"), so the old group's writes wait no longer whichever node asks.
- The new group's half stays in `Groups::carry`, at the new group's node: it asks for at most the
  units its own writer takes, writes the batch with the stamps it was read at, and keeps its progress
  in its holding, as before.

`carry` answers a pull itself when the old group is on its node (a split or a merge) and otherwise
sends it to the node the directory it routes by places the old group on, through `groups::Peers`.
So a split, a merge and a move all read through `serve`: one path. A node starts only the groups the
directory places on it (`Groups::on` takes its node) and refuses the keys of the others
(`Away::Elsewhere`). A pull and its answer are serde types, encoded by postcard by variant index,
variants only appended.

Nothing else changes. The fence, flip, activate, release and retire are each carried at their own
group's node, where the driver addresses them (`Do::group`). Correctness never rests on the channel:
a pull or answer lost is an error, and the driver asks the action again; a stale one is refused by
the new group's holding.

**Evidence.** `tests/groups.rs`, T-T1.9.R10: two nodes, two groups on the first, the second moved to
the second node by the driver while a row is written before every action, each pull and its answer
carried as postcard bytes. The move ends flipped; the first node keeps one group and has retired the
other; the second holds the moved span; every key's acknowledged rows read back at its group's node,
and every key is in exactly one group's file. The tests within one node (T-T1.9.R4 to R6) pass
unchanged through the same path. T-T1.9.R11: the same move with every third pull's answer lost on the way
back, after the old group's node acted on it, each loss asked again by the driver: the same outcome.

**Left.** `Peers` over a socket between machines: the owner transport (`crate::transport`) speaks to
one database owner, and its authenticated frames are where the pull goes, as appended request
variants (local-owner-transport.md, "Versions"), with loss and partition tests. That is the next
piece of this item.

**Falsified by.** A move under writes that loses an acknowledged write or leaves a key in two
groups; or a pull's batch larger than the old group's writer's learned batch.

## Decision (lane split-merge-10): pulls over the owner transport

A move's pull now crosses processes on the owner transport (local-owner-transport.md), with its
listener, its protected record and its authentication unchanged. Two variants are appended, after
every existing one: `Request::Pull` and `Answer::Pulled`, and the protocol's version goes to 7.
- A node's endpoint (`transport::Node`) is an owner listener with no database: it answers a pull with
  `Groups::serve_within` at its node's groups, and any other request finds no database. A database's
  owner answers a pull `Pulled::NoGroup`.
- A node's peers over it (`transport::Nodes`) know each node's endpoint by its path, and implement
  `groups::Peers`: one authenticated connection and one request a pull. A pull refused, cut off or
  unanswered is an error, and the driver asks the action again, as before.
- An answer must fit in one frame. So the endpoint serves a batch held to the protocol's bound on a
  result (`amphora_api::RESULT_MAX`, a format value) at the most bytes a unit of that work has held
  there, as the memory bound does (decision "A batch bounded by the process's memory"); the frame is
  a mebibyte larger, for the answer's own fields. A batch the channel can't carry is never read.

**Evidence.** `tests/groups.rs`, T-T1.9.R12: the move of T-T1.9.R10 with each pull over the
transport between two endpoints, every fourth pull asked while the other node's endpoint is down
(a partition: the pull never reaches it; it is started again after) and every third answer dropped
after it arrived (a loss after the node acted). Both faults are met; the move ends flipped, every
acknowledged write reads back at its group's node, and every key is in exactly one group's file.

**Left.** The owner transport listens on loopback, its record in a protected directory beside the
database: it reaches another process on the same machine. A node on another machine needs that
listener on a network address with an authentication that doesn't rest on a shared file (the
replicated transport's, hyper-transport); `Nodes` is the seam it plugs into. Built since (lane
split-merge-13): decision "The transport between machines".

**Falsified by.** A pull answered through a partition; a move over the transport that loses an
acknowledged write or leaves a key in two groups; a pull's answer larger than a frame.

## Decision (lane split-merge-11): the routing key is bytes

A store in groups routed a row by one key, a string (`Engine::key`), the same key a read waits on
for its writes. A Records row's routing key is bytes (`reshard::row_key`: namespace, family, key),
and a Records transaction names many keys, each of which may fall in a different group. Now an
engine also says the keys a row routes by, as bytes (`Engine::routes`, one call a key); a store in
groups routes a batch by every one of them, and refuses the batch whole if they are not all in one
group (`store::Refused::Groups`), as it refused a batch of rows across groups before. Its key, unless
the engine says otherwise, so every engine whose key is its route is unchanged.

`Engine::key` stays what a read waits on: Amphora's own engine (`database::Local`) keeps one key
for every command (`"database"`), so a read still waits for every write handed to its group before
it, while it routes:
- a Records transaction by the row key of every key its conditions and writes name, so a
  condition on a key in another group refuses the transaction as a write there would;
- a get by its key's row key;
- any other command (SQL, a scan, changes, a namespace's head) by none: it goes to the group
  holding position 0. SQL tables have no row key and don't move (decision "A row routes by its
  namespace, family and key"); a scan, changes and a head over a namespace whose keys are in more
  than one group need CP-T1.10's order across groups.

`Groups::read_routed` reads in the group of a key in bytes, waiting on the key the engine's reads
wait on, and `Groups::group_of` names the group of a key in bytes.

**Evidence.** `database::group_holding`, T-T1.9.R13: local databases as groups (`Groups<Local>`),
one group split by the driver through the shell while a Records transaction is written before every
action, then merged back. Each operation is done; after each, every acknowledged write reads back
through its group; while split, a transaction of two keys in one group commits, and one whose
writes, or whose condition, name a key of the other group is refused whole.

**Left.** A `Database` (the public type) over `Groups<Local>`: its writer is one store today, and
its scans, changes, heads and SQL need the answers above across groups before it can be.

**Falsified by.** A Records transaction committed with keys in two groups; a write acknowledged
through `Groups<Local>` that doesn't read back after a split or merge.

## Decision (lane split-merge-12): load from the store's meter

The planner (`plan::plan`) compares each group's share of the load with one core's share, but the
shell didn't measure the load: the lab counted its own writes. Now the shell does
(`Groups::measure`, one call a planner run):
- a group's **load** is the time its writer spent committing since the last call (the store's
  meter, `Counters::apply_us`). Time adds up across groups and weighs a costly write more than a
  cheap one, which a count of writes doesn't;
- its **load cut** is where the keys handed to it since the last call fall half each side
  (`plan::Weights`, one weight a key at its position, cut by `plan::cut`);
- its **size** is the bytes its engine says its rows hold (`Engine::bytes`, asked on its writer):
  a local database's pages in use (its pages less its free ones, at its page size), not its file's
  length, which doesn't shrink as a swept range's rows leave;
- this machine's **cores** are what the operating system lets the process run on
  (`groups::room`, `std::thread::available_parallelism`).

Each call starts the next window, so the window is the time between planner runs; the plan
compares shares, which don't depend on its length. None of this is a picked number: the split,
merge and move rules stay those of "When: from what is measured", now fed what the store measured.

The positions are kept bounded. A group's weights hold at most as many entries as the process's
memory for batches holds (decision "A batch bounded by the process's memory"), shared among this
node's groups. Past that, each pair of neighbouring entries folds into one at the first's position,
so the total stays exact and a cut still falls at a position written, at the resolution the bound
leaves. A fold halves the entries, so it runs no more than once every half-bound adds.

**Evidence.** `plan::tests`, T-T1.9.R14: a thousand weights kept to eight entries stay within eight,
their total exact, and cut with weight on each side; four single weights cut exactly in the middle.
`tests/groups.rs`, T-T1.9.R15: a group measured with nothing written has no load cut; after
sixty-four keys are written its load is above zero, its cut counts every key on the side its
position falls, and the plan on two cores splits it there; the next window starts empty. The
operations' lab (T-T1.9.L1) plans from `Groups::measure` and `groups::room`.
`database::group_holding`, T-T1.9.R17: after a split of local databases, each group's measured size
is above zero.

**Left.** A write takes the group's window lock once a batch; if that shows in a profile, the
weights can be kept per writer thread and gathered at `measure`.

**Falsified by.** A plan from `Groups::measure` that splits a group with no load or misses one with
more than a core's share; a group's weights growing past their bound.

## Decision (lane split-merge-12): a retired group's files

A merge or a move retires a group, and its files stayed on disk for good. `Groups` doesn't know
where a group's files are (its caller opens each group's engine), so it names them and the caller
deletes them: `Groups::retired_of(found)` takes the groups whose files the caller found and gives
back those retired here or shown retired by the directory routed by, and not held here. Such a
group is never opened again by this node: a restart doesn't start it and a create asked of it again
is refused (decision "Retired groups kept"). A group held here, one being made, and an id never
given out are never named, so no live file is named. The list is as long as what the caller found,
so it stays bounded. A file a store still has open (a read in flight as the group retired) may
refuse deletion on Windows; the caller asks again at its next pass, or at start, when nothing of the
group is open.

**Evidence.** `tests/groups.rs`, T-T1.9.R16 (with T-T1.9.R6): after a split and a merge back, of
groups 0, 1 and 2 only the retired group 1 is named, before and after a restart; its files deleted,
every key still reads back from its group.

**Falsified by.** A group named whose files are still read or written by this node; a retired
group's files left after the caller has deleted those named.

## Decision (lane split-merge-13): the transport between machines

A move's new group pulls from the old group's node, and a transaction across groups steps each
group's owner, through `transport::Nodes`. Until now every endpoint listened on loopback,
authenticated by a capability in a protected file beside the database: it reached another process
on the same machine, never another machine.

Now a node's endpoint (`Node::listen`) and a group's database owner (`Database::listen`) also listen
at a network address, and `Nodes::network` dials them:
- Authentication is the replicated transport's: mutual TLS 1.3 as hyper-transport's `Tls::mutual`
  builds it. Each side presents a certificate chain from the deployment's authority and verifies the
  other's against the authority's roots, and the dialer checks that the server's certificate carries
  the name it was given for that node. Key exchange is by the hybrid post-quantum groups only and
  the suites are the 256-bit ones only, as hyper-quic's `node_provider` names them; no early data (a
  first message may be a write). The TLS is hyper-tls, the rustls the replicated transport already
  vendors. Nothing rests on a shared file.
- The owner protocol runs inside the TLS stream unchanged. Its greeting's capability is exported
  from the TLS session (RFC 8446 §7.5), so the proofs bind each connection to its session and one
  greeting serves both endpoints. Frames, deadlines, the descriptor budget and the accept loop are
  the loopback endpoint's (`Owner::accept`, `endpoint::listener`).
- A network endpoint answers only what a node asks: a pull, a step or a probe. Sessions, Records and
  SQL requests stay on the loopback endpoint, and a network one refuses them.
- Each connection builds its TLS configuration from the credentials (hyper-tls drives a connection
  with its configuration borrowed mutably), checked once when the endpoint or the peers are made.
  So no connection resumes a TLS session: every pull and step is a full handshake.

Values introduced: the protocol name `amphora-owner` and the exporter label, format values.

**Evidence.** `transport::network`'s tests (T-T1.9.R18): pulls over a network address under mutual
TLS; a client from another authority, a server from another authority and a server whose
certificate names another node, each refused with no pull served; a database request refused at a
network endpoint. `xgroup::lan`'s harness over two databases' network endpoints commits with no
unknown outcome. The LAN harness (`examples/xgroup_lan.rs`) runs it between machines: `node
--listen`, and `bench` or `elle` with `--cert`, `--key` and `--roots`.

**Falsified by.** A request served to a peer whose certificate isn't under the roots, or by a node
other than the one the dialer named; a network endpoint serving a database session.

**Left.** One connection a request, each a full handshake: if the LAN runs show the handshake
dominating a step's or a pull's time, connections are kept and reused. The coordinator's numbers
decide it.

## Decision (lane split-merge-15): the size cut from the B-tree's middle key

The planner splits a group whose size is more than one core's share of the whole at its size cut
(`GroupLoad::by_size`), but the shell gave none, so a group too big for its share and with no load
was never split. Now `Groups::measure` asks each group's engine for one (`Engine::size_cut`, on its
writer, with its size). A local database reads it from its position index (`__amphora_positions`)
alone: the root page of that index's B-tree, read through the engine's pager in a transaction of its own, and its middle cell's key.
- A B-tree keeps every leaf at one depth and splits a full page into two halves, so the root's
  children hold about as many keys each. The middle cell's position cuts the keys about in half,
  and the weight on each side is the root's children before and from it (a root that is a leaf:
  its cells). The planner needs weight on both sides; the numbers are counts, not bytes.
- It reads one page whatever the rows, on the group's writer, so a measure holds the writer about as
  long as a point read does, never as long as a scan.
- The page's layout is SQLite's file format (§1.6, "B-tree Pages"; §2.1, "Record Format"), which the
  engine keeps: format values, none picked. A page it doesn't know, or a root with too few keys to
  cut, gives no cut, and the group splits by its load alone as before.

**Evidence.** `database::group_holding`, T-T1.9.R19: a local database made a group's has no size cut
with one key; with eight keys, and again with more keys than one page of the index holds, its cut
has weight on each side and keys on each side.

**Left.** The cut counts keys, not their bytes: a group whose large rows sit on one side is cut by
count. If a lab shows cuts far from the middle of the bytes, the root's children can be read too
(one level, at most the root's fan-out of pages), weighing each by its cells.

**Falsified by.** A size cut with no key on one side; a measure that reads more than the root page
of the position index for it.

## Decision (lane split-merge-18): reads across groups by commit stamp

Each group counts its own revisions, so once a namespace's keys are in more than one group a
revision orders one group's writes only, and a scan, the changes and the head went to one group
(decision "The routing key is bytes"). Ada's decision (2026-11-22, 07:17Z) is that a revision
across groups is the cross-group timestamp, the one the session tokens use. Now a database handed
the groups' coordinator (`Database::across`, the optional `Cluster` agreed with lane xgroup-sql-6)
reads its namespaces across every group, ordered by each revision's commit stamp:
- The stamp is the backup's hybrid logical clock (`backup::Marks`; Kulkarni et al., "Logical
  Physical Clocks", OPODIS 2014), which the cross-group steps already carry from group to group
  (T-T1.12.X2), so a commit made after another, by any path of steps, is stamped after it, whatever
  the two machines' clocks read. A backed-up writer takes a stamp for each revision as its commit
  writes it (`Marks::next`) and keeps it beside the revision (`__amphora_rev_stamps`).
- A read takes a stamp from every group, and its cut is the latest; every group then observes the
  cut, so each commit it stamps from then on is later; then one call on each group's writer reads as
  of the cut, after every commit that took its stamp before. The cut is closed in every group: no
  revision is ever made at or before it later, so a cursor at the head never passes a change, and
  the cut holds every write answered before the read began.
- `head` is the cut. `changes` are every group's after the cursor through the cut, in stamp order;
  a group with more than the limit bounds the page at its last stamp returned, and no page cuts a
  stamp. `scan` reads each group as of its newest revision at or before the cut (or a stamp asked
  as a snapshot), merged by key. The revision answered is the stamp.
- A group must stamp its commits (opened backed up) and be in this process; otherwise the read is
  refused, typed, naming the group.

**Evidence.** `database::across`, T-T1.9.R20: two backed-up groups, the second's clock 500
seconds behind the first's, rows written in turn through their coordinator; the changes across
them come in the order written, in pages of one alike, a scan merges them by key, and a row written
after the head is stamped past it.

**Left.**
- One candidate is built (now both, behind a switch: decision "Decision (lane split-merge-19):
  the read point, A/B"). Its rival is the earliest of the groups' stamps, with no observe: no
  group's clock is pushed forward, but the head trails by the widest skew between the groups'
  clocks, and a write a group with a clock ahead just answered is missing from a read begun after
  it. They want a switch and the lab's numbers (head latency p50/p99/p99.9, how far the head
  trails, writes missing from a read begun after them, at 1 to 4096 groups) before either is kept.
- A revision made before its group stamped, or moved in from another group, has no stamp: a scan
  answers it at 0, and the changes refuse it. The copy path doesn't move the stamps with the rows
  yet. Moved rows done: decision "Decision (lane split-merge-19): a moved row carries its commit
  stamp".
- Compaction keeps every stamp, one row a revision: it must trim the stamps of the revisions it
  took, within its bounded steps, before this is bounded. Done: decision "Decision (lane
  split-merge-19): compaction trims the commit stamps".
- `get`, `commit`, `changed` and `wait` still answer from this database alone; groups on a node are
  read through their own endpoint once it answers Records.

**Falsified by.** A change across groups answered after one that was written after it; a change
made at or before a head answered earlier; a page that cuts a stamp.

## Decision (lane split-merge-19): compaction trims the commit stamps

Every resource is bounded, and `__amphora_rev_stamps` held one row a revision for good. Now
compaction trims it in the same page-fenced steps as the versions (`across::trim`, called from
`compaction::step`): a cutoff is complete only once its versions are and the stamps they leave are.
- A stamp is kept while any version of its revision is, in any family: a scan across groups answers
  a row's revision as its stamp, and a row compaction keeps can be old.
- The newest stamp at or before the feed is kept as well: it is the horizon a refused read of the
  changes is told, and it keeps `through` exact. A cursor at or past it finds the revisions as
  before; one older finds a revision below the feed, which is refused as compacted, as before.
- The sweep goes in rounds, each through the stamp kept when it began, its cursor in
  `__amphora_stamp_trim` (made lazily in the step, as compaction's own state is), so it never
  holds more than one stamp's work and resumes after any step. A round walks the revisions kept
  below the feed, as the version pass walks the versions below its cutoff, and finds those whose
  versions moved out to another group as well as those compaction dropped.
- Whether a revision is still kept is one seek of each of its namespace's families in the feed
  index, so commits pay nothing more (no index on revision alone).

So the table holds at most one stamp a revision with a kept version, plus one, plus the revisions
not yet compacted, the same bound as the versions.

**Evidence.** T-T1.9.R21: one backed-up database, 600 commits each overwriting the next of 8 keys,
compacted to the head after every 50; after each compaction every revision with a kept version is
stamped, and the stamps number at most the kept revisions plus one (8 or 9, not the commits so
far).

**Falsified by.** A stamps table that grows with the commits under compaction; a kept row whose
stamp is gone; a read of the changes after a trimmed stamp that is answered rather than refused.

## Decision (lane split-merge-19): the read point, A/B

Two ways to take a read's cut across groups are built, behind a switch (`ReadPoint`, chosen by
`Database::across_at`; `Database::across` takes the control). Each takes a fresh stamp from every
group, and each cut is closed: a group stamps all it makes from then on after its own fresh stamp,
so after either cut, and a cursor at either head never passes a change.
- `Latest`, the control: the latest of the stamps, which every group then observes. A read holds
  every write answered before it began. Its cost is the push: a second lock of every group's
  clock a read, and a group whose clock lags has its stamps held at the pushed clock (counted in
  the marks' `held_back`) until its own clock passes.
- `Earliest`, the rival: the earliest of the stamps, observed by none, so no clock is pushed. A
  write answered by a group whose clock runs ahead of the slowest is missing from reads begun
  after it, until the slowest clock passes its stamp: the head trails by the widest skew between
  the groups' clocks, not by a quiet group (each group's fresh stamp is its clock's now).

Neither is kept until the lab's numbers decide. The lab, `database::across::tests::lab_read_point_a_b`,
forces each failure mode: many groups for the push, and one group whose clock lags the rest for
the staleness. Each round writes a row in every group, then reads the changes from the last cursor;
it prints the read's and the cut's latency (p50, p99, p99.9), the rows answered before the read
that it missed, and how far its head trailed the newest stamp. The run of record (the
coordinator's, on one machine for both candidates):

    AMPHORA_AB_GROUPS=2,16,128 AMPHORA_AB_ROUNDS=2000 AMPHORA_AB_LAG_NS=1000000000 \
      cargo test -p amphora --locked --release --lib -- --ignored --nocapture lab_read_point_a_b

The lag is the scenario's input, not a tuning: it stands for a machine whose clock is a second
behind, and the staleness the rival shows scales with it.

**Evidence.** T-T1.9.R22: two groups, the second's clock 500 seconds behind; a row written in the
first is missing from a read at the earliest point begun after it, whose head is behind the row,
and held by one at the latest point; both points' heads are closed.

**Falsified by.** A read at the latest point missing a write answered before it began; a change
answered at or before a head either point answered; the lab showing the push's latency growing
faster than the reads' own one call a group.

## Decision (lane split-merge-19): a moved row carries its commit stamp

A version moved in from another group keeps the revision it had there (so a read at any revision
answers the same in the new group), and that number says nothing here: it can be the number of one
of this group's own revisions. So a moved row's place in the order across groups must come with it.
- The copy path sends each version's commit stamp after its versions, in the same `rows` bytes
  (`group::moved`): the versions first, as before, then one stamp a version (none for one its
  sender had no stamp for). A reader of the versions alone stops after them, so the format grows
  without a new field.
- The importer notes each version it takes with its stamp (`__amphora_moved_stamps`, by namespace,
  family, key and revision, indexed by stamp) and observes the newest, so every commit it makes
  after is stamped after the rows it took.
- A scan answers a moved row's revision as its own stamp. The changes read a group's own versions
  by revision, as before, leaving out the moved ones, and the moved ones by stamp, and merge the
  two by stamp; a side that had more bounds the page at its last stamp, as the groups do.
- A version moved in without a stamp (from a sender that carried none) has no place in the order:
  the changes are refused, typed, saying so, rather than answered without it.
- The notes go with their versions: compaction deletes a version's note with it, and a key moved
  out or swept deletes its notes.

**Evidence.** T-T1.9.R23: two groups with revisions 1 to 3 each, stamped 10, 30, 50 and 20, 40,
60; the first's keys moved into the second. Each moved row answers its own stamp; the changes
interleave by stamp, after a stamp the rows after it, two at a time the first two with more; the
same rows moved without stamps into a third group make its changes refused.

**Left.** A scan as of an older stamp still picks each group's versions by its own revision, so a
moved row's version is chosen by its old group's number there; the scan at the cut is exact.

**Falsified by.** A moved row answered with the stamp of this group's own revision of the same
number; a moved row missing from the changes, or answered twice; changes answered without a moved
row that has no stamp.

## Decision (lane split-merge-19): one key and what changed, across groups

`get` and `changed` on a database handed the groups' coordinator now read across groups, as
`scan`, `changes` and `head` do, each revision its commit stamp.
- `get` reads only the group the key's row is in (the cluster's route of its row key). As of a
  fresh stamp of that group (Linearizable): the group's writer runs the read after every commit
  that took its stamp before, so it holds every write of the key answered before it began. As of a
  stamp asked (Snapshot), which that group first observes, so none of its later commits is stamped
  at or before it, and the answer at that stamp never changes. No other group is read and no other
  clock is pushed, at either read point: one key lives in one group.
- `changed` takes the cut at the read point, as `changes` does, and answers the newest stamp of a
  change to the family after the cursor through the cut in any group, its own versions or moved
  in; none while there is none. A compacted cursor, or a version moved in without its stamp, is
  refused as `changes` refuses it.

**Evidence.** T-T1.9.R24: two groups, the second's clock 500 seconds behind; each key read across
them answers its row with its stamp as its revision, a key never written none, a key as of just
before its stamp none and as of its stamp the row; what changed after nothing is the newest stamp,
after the head none, and after a later write its stamp, past the head.

**Left.** `commit` and `wait` still answer from this database alone. A commit across groups goes
through the coordinator (`Cluster::commit`) but its conditions name revisions, which across groups
are stamps; `wait` wakes on one database's commits, and across groups it wants one event a commit
of any group, not a poll.

**Falsified by.** A get across groups missing a write of the key answered before it began; a get
as of a stamp whose answer changes later; `changed` answering none while `changes` after the same
cursor answers a change.
