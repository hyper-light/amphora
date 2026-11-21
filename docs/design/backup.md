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
  no longer say them, which counts as `source_moved`, never as a pass. Open: the shipping thread
  taking the source's digest while it writes a through mark (the source quiet, its rows those of
  that second), so a running database checks itself; not built.

**Evidence.** T-T1.12.C1: a database backed up to a folder, written a row a second and closed;
restore-and-check passes at its newest second against the model's rows and fails naming the rows
against rows that differ; the counters say one of each. T-T1.12.C2: the cadence rule's cases.

**Falsified by.** A check that passes on an archive whose restore differs from the source at that
second; a check that never comes due while bases land.

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

- **The groups' clocks.** A cut is one second in every group, so every group's commits must be
  stamped by one clock. Groups on one node share it. Across nodes, a participant's intent stamped
  later than its anchor's decision by clock skew would be cut away while the decision stays: open,
  not built (the stamps would need a bound on skew, as Spanner's commit wait gives, or a hybrid
  logical clock carried in each step).

**Evidence.** T-T1.12.X1: two groups backed up to one folder under a clock that moves a second at
every reading, so every commit in either group has its own second; two transactions across both
groups; then both restored to every second from their first bases to the end, under two-phase
commit and under Parallel Commits. At every cut the two keys are both absent, both the first
transaction's or both the second's, never moving back, the last cut holding the second's, and some
cut between the two (non-vacuous).

**Falsified by.** A cut at which one group holds a transaction's write and another doesn't after
`restore_cut`; a cut whose restore is refused while each group's archive holds the second whole.
