# Decision (lane pitr): backups and point-in-time recovery to Mantle (CP-T1.12, G14 step 4)

**Decision.** A database's log is shipped to an object store, and a restore replays it, as
PostgreSQL's continuous archiving does: a base backup, then the log after it, up to a chosen stopping
point ("Continuous Archiving and Point-in-Time Recovery", postgresql.org/docs/current/continuous-archiving.html:
"If you want to recover to some previous point in time ..., just specify the required stopping point").

- **The seam.** Amphora reaches Mantle only through `amphora_api::blobs::Blobs` (CLAUDE.md §9):
  create-once, get, list by prefix, delete; `SimBlobs` is its simulator and `blobs::conformance` its
  suite (AMPHORA-BLOBS-1 to 7). kiln implements it over `kiln_seams::blobs::Blobs`, with Mantle
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
  The engine's own restarts (at the writer's next write once every frame is in the file, and on
  the last connection's close, as SQLite's) are off for a backed-up writer: they skipped the wait.
  The fence follows restarts one by one (each raises the header's checkpoint sequence by one), so a
  log started again twice with no write between, its middle generation empty, stays linked whole.
- **Through marks.** A round with nothing new to ship, every commit stamped so far shipped, writes
  a small sealed object (`p/through/...`) saying the archive is whole through now, once that is in
  a later second than it was: a database that stops committing, or closes, restores up to the
  second it stopped in. A later mark or segment lets the earlier mark go.
- **Bases.** A copy of the file, chunk by chunk, with no reader held (a reader held for a 100 GB copy
  would hold the log's restarts and the store's readers back), made consistent by replaying the log
  from the first frame of the generation it began in through where the log stood when it ended, as
  PostgreSQL's base backups are made consistent by the WAL from their start. A torn page in the copy
  is a page a checkpoint was writing, from a frame the replay writes again.
- **No picked numbers.** Shipping has no timer: each round sends what became durable while the last
  put ran, so a segment's size and the cadence follow the observed log rate and the store's put time
  (Ada, 2026-11-21: learned as they run). A segment is split only by the store's own object size. A
  base's chunk is as large as the store and the memory given allow: a restore's fetch rate rises with
  chunk size for any request latency and bandwidth (`size / (latency + size / bandwidth)` grows with
  size), so the bound is the best size whatever is measured. A new base is due when the log since the
  last outweighs it, after a break, or once restore rates measured by drills say a restore would pass
  AC-T1.12.3's 30 minutes (`plan::base_due`). Marks are kept one a second (the restore target's
  granularity, AC-T1.12.1) and at most as many as the log holds frames between restarts.
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

**Evidence.** `backup::folder` tests: the seam's conformance suite (AMPHORA-BLOBS-1 to 7) on a folder,
at two depths, leaving the folder empty; names that differ only in case, `.`, `..`, device names and
a key under a key each kept apart (AMPHORA-BLOBS-8); and 4 seeds of 400 random creates, deletes,
gets and listings (random prefixes, cursors and limits) answered exactly as `SimBlobs` answers them.
T-T1.12.C1 backs a database up to a folder through its own shipping thread and restores it.

**Falsified by.** A listing on a folder that differs from the simulator's; two creates of one key
both succeeding over a mount; an object read torn after a crash.

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
§1.3). A base's manifest names, in runs, the base each chunk is stored under (manifest format 2;
format 1 manifests still read, every chunk their own).

- **Which chunks changed.** A restore replays the log from the first frame of the generation its
  base began in, and a log only starts again once every frame is in the file. So a chunk of the
  latest base whose pages no frame wrote in the generations from that base's beginning up to the
  new base's is the same bytes in the file now; frames of the new base's own generation are
  replayed over it in any case. The shipper reads each frame's page number as it ships it (the
  frame header's first field, sqlite.org/fileformat2.html) into a bitmap a bit a page, bounded by
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
  bytes hold the restore (the source's file and log as they stand) and still leave the source's
  log room to grow to the most it keeps. No number is picked: each side is measured where the
  database runs.
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
