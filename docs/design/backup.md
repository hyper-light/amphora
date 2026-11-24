# Decision (lane pitr): backups and point-in-time recovery to Mantle (CP-T1.12, G14 step 4)

**Decision.** A database's log is shipped to an object store, and a restore replays it, as
PostgreSQL's continuous archiving does: a base backup, then the log after it, up to a chosen stopping
point ("Continuous Archiving and Point-in-Time Recovery", postgresql.org/docs/current/continuous-archiving.html:
"If you want to recover to some previous point in time ..., just specify the required stopping point").

- **The seam.** Amphora reaches Mantle only through `amphora_api::blobs::Blobs` (CLAUDE.md §9):
  create-once, get, list by prefix, delete; `SimBlobs` is its simulator and `blobs::conformance` its
  suite (AMPHORA-BLOBS-1 to 7 and 9). kiln implements it over `kiln_seams::blobs::Blobs`, with Mantle
  behind that.
- **What is shipped.** The log's own bytes: a segment is a run of frames of one log generation with
  its header (sqlite.org/fileformat2.html, "WAL File Format"), so the restore needs no decoder of its
  own: it writes the frames back as a log file and the engine's recovery replays them, records and
  deltas of the mixed log included (docs/design/store.md, decision 37). Only durable frames are
  shipped: an archive never holds what a crash took from the source.
- **When each commit was made.** The writer stamps its commits (`backup::Marks`), the last commit of
  each second kept, as PostgreSQL's recovery target is compared with each commit record's time. A
  restore to second `s` keeps every commit stamped at or before `s`. A stretch whose commits weren't
  stamped one by one (the log's commits from before the shipper opened, after a break in the log, or
  marks merged at their bound) is refused (`Unrestorable::Inexact`), never restored wrong.
- **The log's restarts wait for shipping.** `Db::restart_log` starts the log again only once the
  backup shipped all of it (`backup::Fence`), until the log is past what a restart keeps (decision
  31); then it starts again and the archive records a break: seconds across it need a base after it.
  A send under way then may ship the rest, from the segment it is in (below): once it has shipped the
  generation's last frame the archive holds it whole, and nothing broke there (lane B25).
  The engine's own restarts (at the writer's next write once every frame is in the file, and on
  the last connection's close, as SQLite's) are off for a backed-up writer: they skipped the wait.
  The fence follows restarts one by one (each raises the header's checkpoint sequence by one), so a
  log started again twice with no write between, its middle generation empty, stays linked whole.
  It is told how a generation ends before the switch that ends it is made (`Fence::restart`, lane
  B24): a round on another thread sees the generation after at once, and would find the fence not
  told of it, so its base under way was refused its end and started over. A shipper opened on an
  archive finds the generation it ends in in either segment (decision 45, lane B25): the current one, or
  the one the last switch closed, where the process ended between that switch and the next round. If
  the archive holds the closed one through its last frame, the fence is told so and the log runs on;
  if it doesn't, the log broke there.
- **A send reads its generation's own segment** (decision 45, lane B24). A log past what a restart
  keeps starts again with a send of its frames under way (the store's put may take long). They stay
  in the segment that closed, so a send reads the segment its generation's header was in, not the
  current one, and ends. A log started again a second time meanwhile writes over that segment: a
  generation's header is durable before any frame of it, so the segment's header, read after each
  part's frames and the generation's own still, says the frames are too; else the send is refused
  and the log is broken there. No sum catches frames of another generation: an object's sum says its
  bytes are those sent.
- **A restore proves what it replayed** (lane B25, AC-T1.12.4). The frames say whose they are: each one's
  salts are its generation's and its sum runs on from the frame before (sqlite.org/fileformat2.html, "WAL
  Format"), and the engine's recovery keeps those that do, up to their last commit, and drops the rest as
  a crash's tail. So a restore compares the frames the engine recovered of each generation with those the
  archive's keys gave it, up to the cut, before it folds them: fewer is refused, typed
  (`Unrestorable::Short`, naming the generation and the frames), never answered with the shorter
  database. Nothing new is written for it, so an archive made before proves its log the same: one that
  holds another generation's frames under a key (a shipper before lane B24 sent some so, its log started
  again under a send) restores every second before them and refuses every second after.
- **A base says what its file holds** (lane B26, AC-T1.12.4). No frame says it: a restore replays the log
  from the first frame of the generation its base began in, and the file must hold every frame before
  that one. So a base's manifest (format 3) records, as its copy begins, the generation it began in (its
  header's sequence and salts) and the closing segment (decision 45): its file (`-wal`, `-wal2`), its
  generation, the frames committed in it, and how many of them the file held. A restore compares that
  with the log it replays, before it makes a file where it can: the closing segment all in the file; its last
  frame the one the archive says the generation before ended at (a switch is the one way a generation
  begins whole on another, so the segment is then known); and the generation the archive holds as the
  base's own the one it began in. A mismatch, or a manifest of an earlier format, which says none of it,
  is refused, typed (`Unrestorable::Unproven`, naming the base and what is missing). A shipper refuses to
  begin a base a restore would refuse so, and one opened on an archive whose latest base is of an earlier
  format needs a base, and begins a whole one: bases made before this restore nothing until it ends
  (Amphora is unreleased; the lead's decision, 2026-11-24).
- **A failed restore leaves no database** (lane B26, AC-T1.12.4). A restore makes the database under a name
  no open takes (`<database>.restoring`, its log beside it: `db::Unrestored`), and puts it at its path
  once it is whole, its log folded, emptied and removed, its file flushed: linked there, as a link
  refuses a name that exists ("The folder store", below), or renamed where the file system has no links.
  So nothing stands at the path until the restore ends, whatever ends it: a refusal, an error, a full
  disk, or a kill between two writes. Its files are removed where it fails; a kill leaves them, refused by
  every open under their own name, and the path is refused too while they are there (a new database made
  at it would be in the restore's way). A restore to the path is refused then, naming them: they may be
  another's under way. A check removes those a check killed before it left. A cut across groups puts no
  group's database at its path before every group's is made.
- **A segment with no header yet** (decision 45, lane B22). The store switches the log's segments once
  a batch is committed, and a segment's header is written by its first commit. Until then, which is
  as long as the writer is idle, the current segment's file is empty (a switch made it, or cut it),
  or holds the header of the generation before; nothing of its generation is committed, so a round
  ships nothing, writes its through mark and may end a base, and a shipper opens on it.
- **Through marks.** A round with nothing new to ship, every commit stamped so far shipped, writes
  a small sealed object (`p/through/...`) saying the archive is whole through now, once that is in
  a later second than it was: a database that stops committing, or closes, restores up to the
  second it stopped in. A later mark or segment lets the earlier mark go.
- **A check that finds the archive short begins a base** (lane B26). A check whose restore is refused for
  what the archive lacks (`Unrestorable::lacks`: frames of a generation, or a base's proof) asks the
  shipping thread for a base and wakes it: its next round begins one, with no commit to wait for, where
  the latest base began no later than the generation that is short, or is the one not proven
  (`Shipper::mend`). The base is whole: nothing in the archive vouches for a chunk kept. A base begun in the
  short generation itself would restore nothing, as a restore replays it from its first frame, so where
  that is the generation shipped now the log, which holds its frames still, is shipped again from its
  first frame as a generation of the archive's own, begun on a gap: its commits so far are stamped
  by the time the archive was whole through, so a mark at that time says which a second since holds. The
  same log gives the same archive: one that is found short again gets another base once a commit moves
  the log's end, so a check can't keep a base copying for nothing.
- **A restore makes a new database** (lane B24). It is refused where the file or either of its log's
  segments is there already: the engine recovers a segment left at the path (`-wal2`) as the
  restored log's own, commits after the asked second with it.
- **Bases.** A copy of the file, chunk by chunk, with no reader held (a reader held for a 100 GB copy
  would hold the log's restarts and the store's readers back), made consistent by replaying the log
  from the first frame of the generation it began in through where the log stood when it ended, as
  PostgreSQL's base backups are made consistent by the WAL from their start. A torn page in the copy
  is a page a checkpoint was writing, from a frame the replay writes again. The file must hold every
  frame of the generations before that one as the copy begins: a switch waits for no read (decision
  45), so a read under way can keep the closing segment's later frames out of the file, and a base
  begun then would lose them. So a base begins only once a pass has copied the closing segment whole:
  the shipping thread asks for one, and where a read still holds it back the base waits for a later
  round. A base begun before the log broke restores no second after the break (a restore's chain starts
  at the break), so it mends nothing (lane B25): it is refused its end (`Shipper::end_base`), the
  shipping thread drops one under way as soon as the log broke and begins another, and a shipper opened
  on an archive holding one (one before lane B25 ended them) needs a base.
- **No picked numbers.** Shipping has no timer: each round sends what became durable while the last
  put ran, so a segment's size and the cadence follow the observed log rate and the store's put time
  (Ada, 2026-11-21: learned as they run). A segment is split only by the store's own object size. A
  base's chunk is as large as the store and the memory given allow: a restore's fetch rate rises with
  chunk size for any request latency and bandwidth (`size / (latency + size / bandwidth)` grows with
  size), so the bound is the best size whatever is measured. A new base is due when the log since the
  last outweighs it, after a break, or once restore rates measured by drills say a restore would pass
  AC-T1.12.3's 30 minutes (`plan::base_due`). Marks are kept one a second (the restore target's
  granularity, AC-T1.12.1) and at most as many as the log holds frames between restarts.
- **A chunk's memory** (T-T1.12.M1, M2). The memory given is the process's budget
  (`memory::Use::Chunks`), not a size taken once: a base copy and a restore each open a stream of
  chunks, and a chunk takes at most its stream's share of the room for chunks now (the room over
  the streams open), read and sealed at once (twice its bytes and the sum), under the store's
  objects and the caller's cap (`Archive::memory`). Each chunk is charged while it's held and given
  back once it's put, so a file far larger than memory goes in chunks that fit beside every other
  use, and a chunk taken while another use holds the room is refused, typed, and tried again at the
  next commit. A new base keeps the latest base's chunk size while it still fits, so an incremental
  base can keep its chunks. A restore needs no room the backup had: it fetches each chunk in ranged
  gets (`Blobs::get_range`, AMPHORA-BLOBS-9; S3's ranged GET, as kiln's seam's `get` takes a
  `Range`) of its own stream's share, no larger than the manifest's largest sealed chunk, one part
  held and charged at a time, the chunk's sum checked over its parts. With no room for a part it's
  refused, typed (`Unrestorable::PastRoom`), before it makes the file.
- **A segment's memory** (T-T1.12.M3, M4). Shipping opens a stream of chunks for each send and
  bounds a segment's part by half its share (its frames read, then encoded, then sealed: two copies
  at most), charged before any part is put, so a send with no room is refused, typed, before it
  writes, and its commits stay pending for the next round. A send carries only as many of the
  marks ready as leave its head (the marks among it) room for a frame in that half, one at least,
  and the round sends again at once for the rest, so a backlog of marks after the store refused
  rounds for long ships over sends instead of never. A restore, and a shipper opening, read each
  segment in ranged parts of half their stream's share: the segment's head (its fields before the
  frames, as postcard lays them out) kept and charged until it decodes, then the frames written as
  their parts come, the sum checked over the parts (no room: `Unrestorable::SegmentPastRoom`). A
  create that finds its object there already (an earlier answer lost) compares it in ranged parts
  that its caller's charge covers, never reading it back whole.
- **A retried send** (T-T1.12.M6). Each segment is named by its generation and its first and last
  frame. A send the store refused partway (an error, or an answer lost after it stored the object)
  is sent again as it was: the same marks, the same through and the same parts, whatever was
  committed or freed since, so each object it stored before is made again under the same key and
  found the same, and no segment of the retry lies over another's frames; the commits since go in
  the next send. A shipper that opens carries on from the end of the log the archive covers. An
  archive whose sends were made again otherwise (an older shipper's, with more commits or other
  parts) can hold segments over the same frames: a restore reads the log once over
  (`plan::cover`): of the segments a generation has from one frame, the one reaching furthest,
  one ending at marks before a part of the same span, and none ending within what it kept. A kept
  segment may begin inside the one before it: those frames are compared, a frame at a time, with
  the log the restore wrote, and a segment that differs there is refused even when its sum holds.
  The plan walks back across such an overlap within a generation; across generations a segment
  still ends where the link says. Retention plans on the cover and lets go of every segment of a
  generation it lets go.
- **A base's end and the cut.** A restore takes the latest base that ended at or before the cut,
  never one ending after it, however close: a checkpoint may have folded frames past the cut into
  the file while the base was copied, and no replay takes those out. A base copied after the
  cut's own commit with nothing committed since ends at the cut's frame, so it is the one taken
  (T-T1.12.P7, M1, M2, M5).
- **Growing chunks** (T-T1.12.M5, an A/B; `ChunkGrowth`, Keep the control). Keeping the latest
  base's chunk size keeps every unchanged chunk, but a backup begun with little room keeps small
  chunks, and a restore's many requests, for good. Growing doubles the size as often as the room
  holds, never shrinking while it fits (so a wavering room makes the chunks grow at most
  log2(largest / first) times), and copies the whole file again each time it grows. The test forces
  both costs; the A/B of record decides.
- **The database's shipping thread.** `Database::open_backed_up` stamps the writer's commits as it
  opens and starts a thread woken by each commit the store makes (and once at its start), never by
  a timer: a round ships, then moves a due base on by a chunk, chunks following one another without
  waiting while a base is under way. It ships through a reader on the writer's engine, so the
  writer never waits on the store; that reader keeps the log on its close too, as it may close
  last. A failed round is logged (`backup.round_failed`) and counted, and tried again at the next
  commit; a base under way then starts over. When the database ends the thread ships once more and
  ends. How long one request to the store may take is the caller's (`Archive::patience`): no
  measure of the store's answer time stands yet.
- **Retention.** `plan::expired` lets go of what precedes the base the oldest kept second needs. How
  long to keep is not stated anywhere yet: the caller passes it (Needs Ada's word).

**Evidence.** `crates/amphora/tests/backup.rs` (T-T1.12.D1): three seeds of 600 random steps
(commits, flushes, checkpoints, log restarts, shipping rounds, base chunks interleaved), then restores
to 12 random seconds each: every row equal. The planner's cases are unit tests (T-T1.12.P1 to P10).

**Falsified by.** A restore to a second whose rows differ from the source's at that second; a second
refused that the archive holds whole; a restore that succeeds across a break in the log without a base
after it; the 100 GB restore drill (AC-T1.12.3) over 30 minutes from Mantle with bases taken by
`base_due`.

## The folder store

**Decision.** The archive's default store is a folder Amphora owns (`backup::FolderBlobs`), with no
cloud required; a peer machine's folder is the same store over a mount (Ada, 2026-11-21: own the
storage, proven from a laptop up). PostgreSQL's archive contract allows any destination ("copy the
segment files to an NFS-mounted directory on another machine", postgresql.org/docs/current/continuous-archiving.html
§25.3.1), and pgBackRest's default repository is `posix` (pgbackrest.org/configuration.html;
research/backup-single-node.md §1.3).

- **Keys as paths.** Each segment of a key is a directory, its last a file. A name is one letter
  (`d` directory, `f` object, `t` a create under way) and the segment's bytes, `[a-z0-9_-]` as they
  are and the rest `%xx`, so names agree under case folding (APFS and NTFS fold case), are never `.`,
  `..` or a Windows device name, and `a` and `a/b` live side by side. A segment whose name would pass
  255 bytes (NAME_MAX on ext4, XFS, btrfs and APFS; NTFS's 255 UTF-16 units) is refused `Invalid`.
- **Create once, atomically.** A create writes a temporary file beside the object, flushes it,
  then link(2)s it into place: link refuses a name that exists, atomically, on local filesystems and
  NFS alike (open(2), O_EXCL: "create a unique file on the same filesystem ... and use link(2) to make
  a link to the lockfile"), so two writers never both create a key and a reader sees an object whole
  or not at all. The directory is flushed after (fsync(2): "an explicit fsync() on a file descriptor
  for the directory is also needed"). Windows' std can't open a directory to flush; NTFS journals
  its directory changes itself.
- **Listing in key order without reading everything.** The walk is a stack, not recursion
  (CLAUDE.md §11): a directory sorts among its siblings as its key and a `/`, where its keys fall in
  byte order; it starts at the prefix's directory, skips directories wholly before the cursor or
  outside the prefix, and stops one past the limit. Names the folder didn't write (`desktop.ini`,
  `lost+found`, a crashed create's temporary file) are not listed.
- **Deletes** remove the file and then each directory left empty; a create that loses its directory
  to such a delete makes it again, until its deadline.

**Evidence.** `backup::folder` tests: the seam's conformance suite (AMPHORA-BLOBS-1 to 7 and 9) on a folder,
at two depths, leaving the folder empty; names that differ only in case, `.`, `..`, device names and
a key under a key each kept apart (AMPHORA-BLOBS-8); and 4 seeds of 400 random creates, deletes,
gets and listings (random prefixes, cursors and limits) answered exactly as `SimBlobs` answers them.
T-T1.12.C1 backs a database up to a folder through its own shipping thread and restores it.

**Falsified by.** A listing on a folder that differs from the simulator's; two creates of one key
both succeeding over a mount; an object read torn after a crash.

## The peer store

**Decision (lane backups-6).** A backup archive may live in another process's folder, reached over
Amphora's own owner transport rather than a mount: `transport::PeerStore` serves an object store
(a `backup::FolderBlobs` it owns) at an endpoint, and `transport::PeerBlobs` is the seam's `Blobs`
on the database's side. The archive then lives apart from the database it backs up with no cloud
and no shared mount, under the transport's authentication (the endpoint's private record), as the
folder store's decision asks of "a peer machine's folder".

- **One call, one request.** Each `Blobs` call is one authenticated connection and one request
  (`Request::Blob`, answered `Answer::Blob`), as a node's pull is; the store's refusals cross typed
  as the seam says them. An answer lost after a create went out is `Timeout`, the seam's "a create
  may still land", so the writer reads the object back.
- **Bounded by the protocol.** An object holds at most the protocol's bound on a result
  (`amphora_api::RESULT_MAX`, a format value), so its frame carries it with its key; the client's
  `object_max()` is the smaller of that and the store's own. A listing's answer holds as many
  entries as a result carries (a key of 1,024 bytes at most and two varints each), and says more
  follow past them, as the seam lets any store's. The protocol's version moved to 9.

**Evidence.** The seam's conformance suite through a peer store over the simulator, in one
process; and T-T1.12.N1: a child process serves a folder store, and from the parent the suite
passes through the client, a database backed up to it restores every row through the client and
straight from the child's folder, and the child ends cleanly.

**Falsified by.** A create acknowledged that the peer's folder doesn't hold; a refusal that crosses
as another; an object or listing larger than a frame carries.

## Restore-and-check

**Decision.** A routine restores the newest second the archive holds whole into a scratch file and
checks it: `PRAGMA integrity_check`, then every table's rows (count and a SHA-256 over them in rowid
order, `backup::digest`) against the source's at that second. Meta, GitLab (after 2017) and Uber
restore their backups continuously and check the result, and GitLab's untested dumps had failed
silently for a long time (research/backup-single-node.md §0.3). The outcome is typed
(`backup::Check`: passed, failed with why: restore, integrity or rows; the source moved on; an empty
archive) and counted (`amphora_backup_checks_total` by result; name to agree with kiln's Observe
owner).

- **Its schedule is the archive's cadence, never a timer** (`backup::check_due`): a check is due
  when a base landed since the last one (each base is checked once), or when the log shipped since
  the last check outweighs the log that check replayed, so the log a restore replays is checked as
  fast as it grows.
- **The source's rows at the second** are the caller's to give (`source(second)`), none when it can
  no longer say them, which counts as `source_moved`, never as a pass. The shipping thread gives
  its own: the next section.

**Evidence.** T-T1.12.C1: a database backed up to a folder, written a row a second and closed;
restore-and-check passes at its newest second against the model's rows and fails naming the rows
against rows that differ; the counters say one of each. T-T1.12.C2: the cadence rule's cases.

**Falsified by.** A check that passes on an archive whose restore differs from the source at that
second; a check that never comes due while bases land.

## The source's own rows at a quiet second

**Decision.** Given a scratch path (`Archive::check`), the shipping thread checks its own archive on
the cadence above, against the database's own rows: no caller and no model supplies them. It takes
them from a reader of its own (apart from the shipping reader, which never waits on a digest) whose
snapshot holds exactly the commits stamped by one second's end, so the check compares the restore
with the source at that very second.

- **Lining the snapshot up with a stamp** (`Marks::quiet_second`). Under the lock commits are
  stamped by, the reader begins its snapshot (a deferred `BEGIN` takes none until its first read,
  which runs there) and reads where the log ends. If that is the newest commit stamped, no commit
  lies between the snapshot and the stamps: a commit not yet stamped would have moved the log's end
  past it, and none can be stamped while the lock is held. The snapshot then holds every commit
  stamped at or before that commit's stamp and none after; it is not quiet otherwise (a commit
  between its write and its stamp, a log just started again) and the next round tries again.
- **Settling the second** (`Marks::settled`). The snapshot is the rows of the newest commit's second
  only if no later commit is stamped in that second. The marks watch it: a commit stamped in it
  says no, and the clock past the second's end with none says yes (stamps never go back, so every
  later commit is in a later second). The digest waits, a few dozen bytes a table, until it
  settles and the archive holds the second whole, then the thread restores that second and checks
  it (`restore_and_check_at`).
- **Where the result goes.** `Status::last_check` (the typed `Check`) and `Shipping::checks()`, the
  counters behind `amphora_backup_checks_total`. A check that can't run (the store, the scratch
  disk) is a failed round, logged and counted, leaving a base under way as it was. A passed check's
  scratch file is removed; a failed one's stays to look into until the next check.
- **What it costs.** One full read of the database a check, the snapshot held meanwhile, so the log
  can't start again until the digest ends (`Db::restart_log` waits, as for any reader); checks come
  as often as bases land or the log doubles, never on a timer. The scratch path is the caller's:
  the restore takes as much disk as the database. A database whose commits never leave a second
  quiet (one commit stamped in each second, every second, with the next always unstamped at the
  round) is never checked; each round tries.

**Evidence.** T-T1.12.C3 (unit): a snapshot lines up only at the newest stamp; its second settles
past its end, and never once a commit is stamped in it. T-T1.12.C3 (`tests/backup.rs`): a database
backed up to a folder with a scratch path, written a row a second, checks itself and passes with no
failure counted; the second it passed restores equal to the model's rows at that second.

**Falsified by.** A passed check at a second whose restore differs from the model's rows then; a
check counted `source_moved` or failed on an archive that restores the source exactly.

## A base begins just after a restart

**Decision.** A base due waits for the log to start again before it begins (`plan::base_begins_now`):
a base's own generation counts as changed for the next base, frames from before the base began
included ("Incremental bases"), so a base begun as its generation starts leaves the next the least
to copy. It begins at once when no base restores the log (none yet, or the log broke since the
latest), in the round the log started again, or once its generation holds the frames a restart
keeps (decision 31): past that the log starts again whether shipped or not, so the wait is one
generation at most, and no number is picked for it.

**Evidence.** T-T1.12.I4: the rule's cases. T-T1.12.C1, C3, D2 and X1 still take their first bases
through the thread.

**Falsified by.** A base that waits past a generation the size a restart keeps; a later base copying
more chunks after a base begun just after a restart than after one begun late in a generation.

## One cut across groups

**Decision.** Several groups restore to one second by restoring each group's archive to that second
and then finishing every cross-group transaction the cut left half-done exactly as a crashed
coordinator's are finished (docs/design/xgroup.md, "Crashes"): `backup::restore_cut` opens the
coordinator's next incarnation over the restored groups, which scans every group and commits or
aborts each transaction by its anchor's decision record (no record: aborted; under Parallel
Commits, a staging record commits only if every intent is there). The cut needs no protocol of its
own: a restore to a second is a crash at that second, and the protocol already survives a crash at
every step. This is the consistent cut Chandy and Lamport define ("Distributed Snapshots", ACM TOCS
3(1), 1985): no group holds the effect of a decision another group's cut has not yet reached once the
records finish it.

- **The groups' clocks.** A cut is one second in every group, so a commit made after another must
  be stamped after it in whichever group each is. Groups on one node share a clock; across nodes
  the next section carries one in each cross-group step.

**Evidence.** T-T1.12.X1: two groups backed up to one folder under a clock that moves a second at
every reading, so every commit in either group has its own second; two transactions across both
groups; then both restored to every second from their first bases to the end, under two-phase
commit and under Parallel Commits. At every cut the two keys are both absent, both the first
transaction's or both the second's, never moving back, the last cut holding the second's, and some
cut between the two (non-vacuous).

**Falsified by.** A cut at which one group holds a transaction's write and another doesn't after
`restore_cut`; a cut whose restore is refused while each group's archive holds the second whole.

## Cuts across machines

**Decision.** Groups on different nodes stamp commits by different clocks, and a participant whose
clock runs ahead stamps its prepared write later than the anchor's decision that followed it: a cut
between the two would keep the decision and lose the write. Each cross-group step therefore carries
a hybrid logical clock (Kulkarni, Demirbas et al., "Logical Physical Clocks", OPODIS 2014): a group
takes in the latest stamp the coordinator heard of before the step (`Marks::observe`), so its
commit is stamped later than that, and answers with its own latest (`Marks::latest`), which the
coordinator keeps as the most it has heard of. A stamp stays the clock's reading, or one nanosecond
past the latest stamp made or seen when that is later, as `Marks` already held a clock stepping
back. Every decision is sent after the replies it rests on, so it is stamped after every write it
decides on, whatever the clocks read, and a cut keeping it keeps them.

- **Why not a bound on skew.** Spanner's commit wait sleeps out a measured uncertainty (TrueTime),
  which needs clocks with a bound measured and kept (GPS and atomic references); Amphora's nodes are
  any machines, with no such bound to measure. The carried clock needs no bound, and costs no wait.
- **What a stamp then means.** Stamps stay close to real time: a stamp runs ahead of a group's
  clock only by as much as another group's clock ran ahead of it, the HLC's bound. A restore target
  is a second in the stamps, so a second asked for on a group whose clock lags is that much later in
  its own clock; each group's archive still restores exactly the commits stamped at or before it.
- **Where it is carried.** In the coordinator's process (`xgroup::sql`'s step), for groups backed
  up there; a step over the wire would carry the same number in its envelope (not built: groups are
  in-process `Database`s today).

**Evidence.** T-T1.12.X2: as T-T1.12.X1, the participant's clock 5 seconds ahead of the anchor's,
under both protocols: at every cut both keys or neither, never moving back. With the carried clock
taken out (the step observing 0), it fails: "second 1037: one group without the other". The unit
case: an observed stamp orders every later commit after it.

**Falsified by.** A cut that keeps a decision and loses a write it decided on, with any clocks; a
stamp further ahead of its group's clock than the furthest-ahead clock of a group it heard from.

## Incremental bases

**Decision.** A new base keeps the latest base's chunks whose pages the log didn't change, and
copies only the rest, so bases can come often and a restore replays a short log (restore time is the
usual failure: GitLab 2017, GitHub 2018, Atlassian 2022; research/backup-single-node.md §0.2), as
PostgreSQL 17's incremental `pg_basebackup` copies only blocks its WAL summaries say changed
(postgresql.org/docs/17/release-17.html) and pgBackRest's block incremental does (research §1.2,
§1.3). A base's manifest names, in runs, the base each chunk is stored under.

- **Which chunks changed.** A restore replays the log from the first frame of the generation its
  base began in, and a base begins only once the file holds every frame of the generations before
  its own ("Bases", above). So a chunk of the latest base whose pages no frame wrote in the
  generations from that base's beginning up to the new base's is the same bytes in the file now;
  frames of the new base's own generation are replayed over it in any case. The shipper reads each
  frame's page number as it ships it (the frame header's first field,
  sqlite.org/fileformat2.html) into a bitmap a bit a page, bounded by
  the database's pages: one for the generation shipped now and one for those before it since the
  latest base began.
- **When it may.** Only with every frame since the latest base began seen (not after the shipper
  opened on an archive, nor after a gap in the log), a generation since it, and the same chunk
  size; otherwise the base copies the whole file. A chunk is kept only if the latest base holds the
  same byte range of the file.
  A base's own generation counts as changed for the next, frames before the base began included,
  so a base begun just after the log starts again leaves the least for the next to copy.
- **When one is due.** `plan::base_due` compares the log shipped since the latest base with what a
  new base would copy (the changed chunks' bytes, measured from the bitmap as the log runs), no
  longer with the whole file, so a base comes as soon as making it costs less than the log it
  spares every later restore; the restore-rate rule stands.
- **Retention.** A chunk a kept base reuses stays when the base that stored it expires.

**Evidence.** T-T1.12.I1 (the shipper opened again midway): 400 rows, the log started again, and a base; 5 rows changed over two generations of the log; the
second base keeps most of the first's chunks and copies the changed ones; seconds across both
restore with every row equal, from both bases; after the first base expires, the newest and the
oldest kept seconds still restore equal. T-T1.12.D1's random drills (log restarts between bases)
now take incremental bases. T-T1.12.I2: manifest formats; T-T1.12.I3: the due rule.

**Falsified by.** A restore from an incremental base whose rows differ from the source's; a kept
chunk whose pages a frame in the traced generations wrote; a chunk deleted while a kept base
reuses it.

**After an open.** The shipper reads the archived segments since the latest base began back
and marks their pages, so the next base is incremental after a restart of the process too (the log
since a base is bounded by `base_due`); a gap since leaves it whole.

## Continuous restore drills

**Decision (lane backups-3).** The shipping thread checks its own archive by default, not only
when given a scratch path. `Archive::check` is a `backup::CheckPolicy`, a switch with three
settings: `Off`; `Scratch(path)`, lane pitr-8's behaviour kept as the control (a check whenever
one is due, at the newest quiet second, no budget); and `Continuous`, the default. Continuous
checks restore into `<database>.check` beside the database (the same disk), and:

- **Within a budget derived from the machine** (`backup::admit`, T-T1.12.C5). A due check runs only
  once the thread has waited for commits, since the last check ended, at least as long as that
  check took, so checks spend only time the foreground leaves idle and none while it keeps the
  thread busy; the first runs at once, nothing being measured yet. It runs only if the disk's free
  bytes hold the restore (the source's pages, those only its log holds yet among them, and its log's
  two segments as they stand: the replay's log is beside the file it folds into) and still leave the
  source's log room to grow to the most it keeps in each segment (decision 45, lane B24). No number
  is picked: each side is measured where the database runs.
- **At random seconds** (`backup::Drills`, T-T1.12.C6). Each quiet snapshot's second and rows join
  a uniform sample of every snapshot taken (Vitter's Algorithm R, "Random sampling with a
  reservoir", ACM TOMS 11(1), 1985), as many as `Archive::memory` holds; each check restores one
  drawn uniformly and compares every row (`backup::restore_and_check_at`). Draws come from
  SplitMix64 (Steele, Lea and Flood, OOPSLA 2014) seeded once from the system's entropy. A drilled
  second the archive's owner has since expired is `Check::Expired`, dropped from the sample, never
  a failure.
- **Instruments.** `amphora_backup_checks_deferred_total{reason="busy"|"disk"}`,
  `amphora_backup_check_seconds` (each check's cost, digest included, in power-of-two buckets for
  p50/p99/p99.9), drills of a second older than the newest, the thread's idle time
  (`Shipping::drills`), each deferral a trace event with its reason and inputs
  (`amphora.backup.check`, `event="deferred"`).

**Evidence.** T-T1.12.C4: on by default with no path given, three checks pass beside the database
and leave no file behind. T-T1.12.C5: one writer per core committing back to back, under the
control and under the default; no check fails under either, checks pass once the load ends, and
under the budget the checks took no more time than the thread was idle (the last check aside).
One tiny run (adacachy, 32 writers, 50 commits each; one run, not a measurement): control commit
p50 17.7 ms, p99 184 ms; continuous p50 11.8 ms, p99 351 ms, 181 checks deferred busy; 848 writes
refused `Full` by the database's own bound under both, the same count, so the refusals are the
load's, not the checks'. The live test of record is the coordinator's
(`AMPHORA_DRILL_LOAD_COMMITS=<n> cargo test -p amphora --release --test backup t_t1_12_c5 -- --nocapture`).

**Falsified by.** A continuous check that runs while the thread has been idle less than the last
check took; a restore admitted that leaves the source's log no room; a second held by the sample
more or less often than uniform; a drill that passes on rows that differ.

Paced checks (lane backups-4, T-T1.12.C7). A tiny C5 run showed continuous checks raising commit p99
from 184 ms to 351 ms, as they ran on the shipping thread with one fsync at the end of a whole
restore. Checks now run on a thread of their own (`amphora-backup-check`), told of each round,
so shipping never waits on them. Continuous, every step (a base chunk, a log segment, a replay,
an object's worth of rows digested) is paced (`backup::Pacer`): it begins only once the store
has made no commit for as long as the last step took, so a commit pauses the steps after it; a
writing step is made durable as it ends, so its cost is the device's; and after each step it
rests `took x (1 - room) / room`, the room being the fastest a step of its kind went per byte
over how fast this one went (the way LEDBAT, RFC 6817, and TCP Vegas read a queue from their own
delay). The default is back to unpaced checks into a scratch file (`CheckPolicy::Scratch(None)`,
beside the database), Ada's earlier word, because the data doesn't yet show paced checks within
the control's spread: in three interleaved tiny runs on adacachy (32 writers, run beside other
tests and production), commit p99 was 24 to 32 ms with no checks, 4.8 to 100 ms unpaced and 4.7
to 94 ms paced, a spread wider than any difference; and under one writer committing back to back
(T-T1.12.C4) paced checks found no quiet stretch and only the first passed. The coordinator's
run of record: `AMPHORA_DRILL_LOAD_RUNS=20 AMPHORA_DRILL_LOAD_COMMITS=500 cargo test -p amphora
--release --test backup t_t1_12_c5 -- --nocapture --test-threads=1` on an otherwise idle machine.

Starvation, event-driven (lane backups-6, T-T1.12.C7, C8). Under one writer per core the
foreground always has a call in flight, so a check that yields to it fully never runs: the
failure mode at scale. `CheckPolicy` gains a fourth setting for the A/B, the control
(`Scratch`, unpaced) unchanged beside it:

- `Continuous` yields fully (`backup::Yield::Fully`): a step begins only while the database runs
  no call. The database counts each call from its submission to its answer
  (`backup::Foreground::call`, reads on the local writer too), not only finished commits, and
  the call that ends last wakes the check. No timer: the quiet wait and the rest after each step
  are gone, as every wait now ends on an event (a call's end, a commit, the database's end).
- `Shared(retention)` gives each check a minimum share (`backup::Yield::Share`): its work over the
  retention period its owner expires the archive by, so a check finishes within the time its
  second is kept. A check behind its share (`backup::behind`: its work so far under the last whole
  check's work times the share of the period gone; before any whole check, once the whole period
  is gone) takes its step with calls in flight, counted as `forced`; it looks again at each commit
  (the store's one thread tells it) and at the last call's end, so callers never meet on the
  check's lock while the foreground stays busy.
- A check's work is counted in the device's time (`backup::device_ns`): a step's bytes at the
  fastest a step of its kind went per byte, never more than it took, so a step the foreground
  slowed counts what it would take alone. That reference decays: it is the fastest of the check
  under way and the last whole check (LEDBAT's base delay, kept as minima over recent intervals,
  RFC 6817's BASE_HISTORY; the interval here a check), so a device that slowed for good is read as it is after one
  check.
- The replay and the integrity check are steps that can pause. The replay folds the log in parts
  of an object's worth of frames (the engine's passive checkpoint, bounded), each made durable as
  it ends, the last fold emptying the log; unpaced, it folds in one step as before. The integrity
  check ends a step at each of its waits on the device, inside its one statement.

Evidence: unit tests of the share, the device cost, the decay and both yields, and a restore whose
paced replay and integrity check end steps inside them (T-T1.12.C8). The A/B of record is the
coordinator's: `AMPHORA_DRILL_LOAD_RUNS=20 AMPHORA_DRILL_LOAD_COMMITS=500
AMPHORA_DRILL_LOAD_WRITERS=<1, 8, 32, 128, 512, 2048, 4096>
AMPHORA_DRILL_RETENTION_MS=<the retention under test> cargo test -p amphora --release --test backup
t_t1_12_c5 -- --nocapture --test-threads=1`, reading `under_load`, `forced` and commit p99 per
policy. A single call running long with no other event holds a waiting check until it ends: one
call's length, not a starvation.
