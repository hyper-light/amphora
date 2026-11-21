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
- **Retention.** `plan::expired` lets go of what precedes the base the oldest kept second needs. How
  long to keep is not stated anywhere yet: the caller passes it (Needs Ada's word).

**Evidence.** `crates/amphora/tests/backup.rs` (T-T1.12.D1): three seeds of 600 random steps
(commits, flushes, checkpoints, log restarts, shipping rounds, base chunks interleaved), then restores
to 12 random seconds each: every row equal. The planner's cases are unit tests (T-T1.12.P1 to P10).

**Falsified by.** A restore to a second whose rows differ from the source's at that second; a second
refused that the archive holds whole; a restore that succeeds across a break in the log without a base
after it; the 100 GB restore drill (AC-T1.12.3) over 30 minutes from Mantle with bases taken by
`base_due`.
