//! Amphora (G5 step 4, AC-G5.4.1; executor.md decision 11): the summaries a database keeps, one
//! per summarized table, shared by its connections, fed by the Insert and Delete opcodes and
//! consulted only by a reader that sees exactly what the summary holds.
//!
//! A summary changes at the write, before the writer commits, and a delete or update lowers a
//! zone's counts (a zone with no rows left is skipped) or moves a row in the cracker. So a summary
//! holds the newest committed rows plus the open writer's. A reader may consult it only when its
//! own snapshot is that state: it is the open writer itself, or no writer is open and its snapshot
//! is the newest commit. Any other reader (an older snapshot, or a reader beside an open writer) is
//! refused with [`SkipRefusal::OlderSnapshot`] and scans every row: never stale skipping. A write
//! undone by a rollback (of its transaction or its statement) leaves the summary ahead of the
//! table, so it is marked [`SkipRefusal::Stale`] until built again.
//!
//! The registry is pure: the caller passes each connection's identity and WAL positions.
//!
//! Lane skip-8 (T-SKU.10): a measured summary's dropped method is built again by one builder at
//! a time, off the scan's path. A builder claims the build ([`SkipRegistry::claim_build`]); while
//! it holds the claim, no other gets one, so connections that find the same build due at once
//! never duplicate it. A builder that reads the table pins its snapshot to the state the summary
//! holds ([`SkipRegistry::pin_build`], the reader's rule above). Each write the hook takes while a
//! build is claimed marks the build invalidated: the copy would miss it. The builder hands the
//! claim back with what it built or with nothing, and only a build no write invalidated is
//! installed ([`BuildEnd`]). Nothing waits on a clock: the write is the event that voids a build.
//!
//! Lane skip-9 (T-SKU.11): three ways to build behind one switch ([`BuildMode`]), per summary.
//! [`BuildMode::Inline`] is skip-7's: the scan that finds the trial due builds on its own path, at
//! its own snapshot (the control). [`BuildMode::VoidOnWrite`] is skip-8's above: under steady
//! writes every build is voided, so the dropped method never comes back. [`BuildMode::Replay`]:
//! the build records each write the hook takes after its pin and applies them, in order, to its
//! copy before installing it, so it installs however many writes arrive. Its record is bounded by
//! the rows the summary holds at the pin: past that, replaying costs more than reading the table
//! again and the record outweighs the copy it feeds, so the build drops its record and falls back
//! to void ([`SkipCounts::replay_overflows`]).
//!
//! Lane skip-11 (T-SKU.14): how a scan asks a summary for an IN list's rows, behind one switch
//! ([`ListProbe`]), per summary. [`ListProbe::Span`] is skip-10's: one probe over the list's span,
//! smallest value to largest (the control); a list whose values are far apart reads every row
//! between them. [`ListProbe::PerValue`] asks once per value and unions what each names; a long
//! list of clustered values pays a probe per value for rows one span would name. Either way the
//! scan is one filter to a measured summary's chooser: one method chosen, one cost handed back,
//! covering every probe it made and every row it read ([`SkipRegistry::candidates_spans`]).

//!
//! On the versioned storage (G7 step 2, `AC-NS.2.1`) writers don't take one write lock: another
//! may commit while one is open. So a write there is taken with [`SkipRegistry::on_write_told`]:
//! the positions are commit timestamps, the writer's end is told ([`SkipRegistry::committed`],
//! [`SkipRegistry::rolled_back`]) rather than read from them, and a second open writer's write
//! makes the summary stale.
//!
//! A build there takes no lock against writers either: a writer may change the table before the
//! build, beside it, and commit after, its rows missing from the summary. So a build is installed
//! with [`SkipRegistry::set_checked`], which asks the storage, under the registry's lock, whether
//! one did; if so the summary is kept stale until built again. A write that had found no summary
//! to feed before its row changed looks again after ([`SkipRegistry::installs`]).

use super::{Candidates, CrackerColumn, RowWrite, SkipError, SkipMethod, Skipping};
use crate::sync::atomic::{AtomicU64, Ordering};
use crate::sync::Mutex;

/// A WAL position: `(checkpoint_seq, max_frame)`, as the pager reports a connection's snapshot
/// and the newest commit.
pub type WalPos = (u32, u64);

/// Why a reader can't skip with a summary: it scans every row instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SkipRefusal {
    /// No summary is kept for this table.
    #[error("skipping: no summary is kept for this table")]
    Off,
    /// A write the summary took was rolled back, or the summary refused a write: build it again.
    #[error("skipping: the summary is stale (a write it took was rolled back); build it again")]
    Stale,
    /// The reader's snapshot isn't the state the summary holds (an older snapshot, or another
    /// connection's write is open).
    #[error("skipping: the reader's snapshot is older than the summary")]
    OlderSnapshot,
    /// The database runs MVCC, whose readers' snapshots the summary doesn't follow.
    #[error("skipping: not kept under MVCC")]
    Mvcc,
}

/// What a summarized table's entry has done, for tests and the harness.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SkipCounts {
    /// Rows written through the hook.
    pub writes: u64,
    /// Scans that skipped with the summary.
    pub consulted: u64,
    /// Scans refused because the reader's snapshot was older.
    pub refused_older: u64,
    /// Scans refused because the summary was stale.
    pub refused_stale: u64,
    /// Lane skip-9 (T-SKU.11): builds of a dropped method installed.
    pub builds_installed: u64,
    /// Lane skip-9: builds voided by a write that came in during them.
    pub builds_invalidated: u64,
    /// Lane skip-9: writes a replaying build applied to its copy before installing it.
    pub replayed_writes: u64,
    /// Lane skip-9: replaying builds whose record reached its bound and fell back to void.
    pub replay_overflows: u64,
    /// Lane skip-9: the most writes one replaying build's record held at once.
    pub replay_peak: u64,
    /// Lane skip-9: rows inline builds read on a scan's own path (the inline control's cost to
    /// the scan that finds the trial due).
    pub inline_rows_read: u64,
    /// Lane skip-11 (T-SKU.14): probes of the summary scans made (one per span asked for: one per
    /// filter under [`ListProbe::Span`], one per IN value under [`ListProbe::PerValue`]).
    pub probes: u64,
    /// Builds kept stale because rows were missing from them (G7 step 2, `AC-NS.2.1`): another
    /// open transaction had changed the table, or a commit passed the build's snapshot.
    pub built_stale: u64,
}

/// Lane skip-11 (T-SKU.14): how a scan asks the summary for an IN list's rows (see the module's
/// comment).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ListProbe {
    /// One probe over the list's span, smallest value to largest (lane skip-10's; the control).
    #[default]
    Span,
    /// One probe per value, the candidates each names unioned.
    PerValue,
}

/// Lane skip-9 (T-SKU.11): how a summary's dropped method is built again (see the module's
/// comment).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BuildMode {
    /// Lane skip-7's: on the scan's own path, at its snapshot; its time is left out of the scan's
    /// measured cost. The control.
    Inline,
    /// Lane skip-8's: off the scan's path; a write during the build voids it.
    VoidOnWrite,
    /// Off the scan's path; the writes during the build are applied to its copy before it is
    /// installed, up to the bound, past which it falls back to void.
    #[default]
    Replay,
}

#[derive(Debug)]
struct Pending {
    writer: usize,
    base: WalPos,
    /// Its end is told, not read from the positions: on the versioned storage another writer may
    /// commit while it is open, so a newer commit doesn't mean it committed.
    told: bool,
}

/// Lane skip-8 (T-SKU.10): a build of a dropped method one builder holds the claim to.
#[derive(Debug)]
struct Building {
    ticket: u64,
    /// A write came in since the build's snapshot: what it reads would miss the write.
    invalidated: bool,
    /// Lane skip-9 (T-SKU.11): under [`BuildMode::Replay`], from the pin on, the writes the
    /// hook took since, to apply to the copy; `None` before the pin, in the other modes, and
    /// once the record reached `bound` (the build is then void).
    record: Option<Vec<RowWrite>>,
    /// The rows the summary held at the pin: the most writes `record` holds.
    bound: u64,
}

impl Building {
    /// A write the hook took during the build: recorded when replaying within the bound,
    /// otherwise the build is void. `true`: the record just reached its bound.
    fn take_write(&mut self, write: RowWrite) -> bool {
        let Some(record) = self.record.as_mut() else {
            self.invalidated = true;
            return false;
        };
        if u64::try_from(record.len()).is_ok_and(|held| held < self.bound) {
            record.push(write);
            return false;
        }
        self.record = None;
        self.invalidated = true;
        true
    }
}

#[derive(Debug)]
struct Entry {
    root_page: i64,
    column: usize,
    skipping: Skipping,
    pending: Option<Pending>,
    stale: bool,
    counts: SkipCounts,
    building: Option<Building>,
    /// Lane skip-9 (T-SKU.11): how the summary's dropped method is built again.
    build_mode: BuildMode,
    /// Lane skip-11 (T-SKU.14): how a scan asks it for an IN list's rows.
    list_probe: ListProbe,
}

/// Lane skip-8 (T-SKU.10): the claim to build a summary's dropped `method` again, held by one
/// builder at a time; hand it back with [`SkipRegistry::rebuild_zones`],
/// [`SkipRegistry::install_cracker`] or [`SkipRegistry::abandon_build`].
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a claim not handed back keeps every other build of the summary out"]
pub struct BuildClaim {
    pub root_page: i64,
    pub method: SkipMethod,
    ticket: u64,
}

/// Lane skip-8 (T-SKU.10): how a claimed build ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildEnd {
    /// Built and installed: the method is kept again, its trial next.
    Installed,
    /// A write came in during the build: nothing installed, the method stays dropped and the
    /// next scan that finds its trial due claims a new build.
    Invalidated,
    /// The summary was replaced, dropped or went stale since the claim: nothing installed.
    Superseded,
    /// The summary refused the build (a cut out of order): it is stale until set again.
    Refused,
    /// The builder's snapshot isn't the state the summary holds (another connection's write is
    /// open): nothing read, the claim handed back.
    OlderSnapshot,
    /// The builder couldn't read the table (its error is logged): nothing installed.
    Failed,
    /// Lane skip-9 (T-SKU.11): the copy refused a write the build recorded (it disagrees with
    /// the summary): nothing installed, the summary kept.
    ReplayRefused,
}

/// How a skipping build on the versioned storage meets writers open on its table (G7 step 2,
/// `AC-NS.2.1`), chosen by `AMPHORA_SKIP_BUILD` (`stale`, the control, unless `wait`) for the A/B
/// of record: each has a failure mode its test forces (`skipping/tests.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildPolicy {
    /// Build at once; a build that missed an open writer's rows goes in stale, and every scan of
    /// the table reads all its rows until it is built again.
    Stale,
    /// Wait for the table's open writers to end (woken by the last one's end), then build; built
    /// again, after another wait, if a writer slipped in beside the build. Blocked as long as
    /// writers keep the table open.
    Wait,
}

impl BuildPolicy {
    /// The environment variable that chooses: `wait`, else stale.
    pub const ENV: &'static str = "AMPHORA_SKIP_BUILD";

    /// The policy the environment asks for.
    pub fn asked() -> Self {
        if std::env::var(Self::ENV).is_ok_and(|v| v == "wait") {
            Self::Wait
        } else {
            Self::Stale
        }
    }
}

/// The summaries one database keeps, by table root page.
pub struct SkipRegistry {
    entries: Mutex<Vec<Entry>>,
    /// Lane skip-8: the last build claim's ticket, unique across summaries set again; locked only
    /// under `entries`.
    tickets: Mutex<u64>,
    /// How a build on the versioned storage meets open writers (`BuildPolicy`).
    policy: Mutex<BuildPolicy>,
    /// Builds that waited for their table's writers to end, and builds done again because a
    /// writer slipped in beside them (`BuildPolicy::Wait`): the registry's, not a summary's, as
    /// a summary built again starts its own counts.
    build_waits: AtomicU64,
    rebuilds: AtomicU64,
    /// Waits refused, by why (lane g7s2-17): the writer was the build's thread's own, or the
    /// deadline passed. Each such build went in as `Stale` builds.
    refused_own: AtomicU64,
    refused_deadline: AtomicU64,
    /// How many summaries were installed, moved under the entries' lock (G7 step 2,
    /// `AC-NS.2.1`): a write that found none to feed reads it before and after its row changes.
    installs: AtomicU64,
}

impl Default for SkipRegistry {
    fn default() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            tickets: Mutex::new(0),
            policy: Mutex::new(BuildPolicy::asked()),
            build_waits: AtomicU64::new(0),
            rebuilds: AtomicU64::new(0),
            refused_own: AtomicU64::new(0),
            refused_deadline: AtomicU64::new(0),
            installs: AtomicU64::new(0),
        }
    }
}

impl std::fmt::Debug for SkipRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkipRegistry")
            .field("entries", &self.entries)
            .field("installs", &self.installs.load(Ordering::SeqCst))
            .finish()
    }
}

impl SkipRegistry {
    /// How builds on the versioned storage meet open writers.
    pub fn policy(&self) -> BuildPolicy {
        *self.policy.lock()
    }

    /// Sets it, as `AMPHORA_SKIP_BUILD` does at open (tests and the A/B's driver).
    pub fn set_policy(&self, policy: BuildPolicy) {
        *self.policy.lock() = policy;
    }

    /// A build waited for its table's writers (`BuildPolicy::Wait`); `again`: it is a rebuild,
    /// a writer having slipped in beside the one before.
    pub(crate) fn count_wait(&self, waited: bool, again: bool) {
        if waited {
            self.build_waits.fetch_add(1, Ordering::SeqCst);
        }
        if again {
            self.rebuilds.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A build's wait for its table's writers was refused (lane g7s2-17).
    pub(crate) fn count_refused(&self, why: crate::storage::versioned::version::UnwrittenRefusal) {
        use crate::storage::versioned::version::UnwrittenRefusal;
        let counter = match why {
            UnwrittenRefusal::OwnThread => &self.refused_own,
            UnwrittenRefusal::Deadline => &self.refused_deadline,
        };
        counter.fetch_add(1, Ordering::SeqCst);
    }

    /// Waits refused: the writer the build's own thread's, and the deadline passed
    /// (`BuildPolicy::Wait`, lane g7s2-17).
    pub fn wait_refusals(&self) -> (u64, u64) {
        (
            self.refused_own.load(Ordering::SeqCst),
            self.refused_deadline.load(Ordering::SeqCst),
        )
    }

    /// Builds that waited for writers, and builds done again (`BuildPolicy::Wait`).
    pub fn wait_counts(&self) -> (u64, u64) {
        (
            self.build_waits.load(Ordering::SeqCst),
            self.rebuilds.load(Ordering::SeqCst),
        )
    }

    /// Keeps `skipping` for column `column` (its index in the table's records) of the table at
    /// `root_page`, replacing any summary it had; `Skipping::Off` drops it. The caller built it at
    /// the newest commit with no writer open (it held the write lock).
    pub fn set(&self, root_page: i64, column: usize, skipping: Skipping) {
        self.set_with_build(root_page, column, skipping, BuildMode::default());
    }

    /// Lane skip-9 (T-SKU.11): [`SkipRegistry::set`], its dropped method built again by `build`.
    pub fn set_with_build(
        &self,
        root_page: i64,
        column: usize,
        skipping: Skipping,
        build: BuildMode,
    ) {
        self.set_checked_with_build(root_page, column, skipping, build, || None);
    }

    /// [`SkipRegistry::set`] for a build that held no lock against writers (the versioned
    /// storage, G7 step 2, `AC-NS.2.1`): `missed`, asked under the registry's lock as the summary
    /// goes in (no write can feed or miss it meanwhile), says why the build misses rows (`None`:
    /// it holds the newest commit), and then the summary is kept stale until built again. A write
    /// whose row changed before the ask is seen by it; one after finds the summary in place. Why
    /// it was kept stale.
    pub fn set_checked(
        &self,
        root_page: i64,
        column: usize,
        skipping: Skipping,
        missed: impl FnOnce() -> Option<&'static str>,
    ) -> Option<&'static str> {
        self.set_checked_with_build(root_page, column, skipping, BuildMode::default(), missed)
    }

    /// [`SkipRegistry::set_checked`] with its dropped method built again by `build` (lane skip-9's
    /// modes on the versioned storage's checked install).
    pub fn set_checked_with_build(
        &self,
        root_page: i64,
        column: usize,
        skipping: Skipping,
        build: BuildMode,
        missed: impl FnOnce() -> Option<&'static str>,
    ) -> Option<&'static str> {
        let mut entries = self.entries.lock();
        entries.retain(|entry| entry.root_page != root_page);
        if matches!(skipping, Skipping::Off) {
            return None;
        }
        self.installs.fetch_add(1, Ordering::SeqCst);
        let why = missed();
        entries.push(Entry {
            root_page,
            column,
            skipping,
            pending: None,
            stale: why.is_some(),
            counts: SkipCounts {
                built_stale: u64::from(why.is_some()),
                ..SkipCounts::default()
            },
            building: None,
            build_mode: build,
            list_probe: ListProbe::default(),
        });
        why
    }

    /// How many summaries were installed so far (G7 step 2, `AC-NS.2.1`). A write that found no
    /// summary for its table reads it before its row changes and again after: if it moved, a
    /// build may have missed the row, and the write looks for the table's summary again.
    pub fn installs(&self) -> u64 {
        self.installs.load(Ordering::SeqCst)
    }

    /// Lane skip-11 (T-SKU.14): how scans of the table at `root_page` ask its summary for an IN
    /// list's rows; `false` when no summary is kept.
    pub fn set_list_probe(&self, root_page: i64, probe: ListProbe) -> bool {
        let mut entries = self.entries.lock();
        let entry = entries
            .iter_mut()
            .find(|entry| entry.root_page == root_page);
        let kept = entry.is_some();
        if let Some(entry) = entry {
            entry.list_probe = probe;
        }
        kept
    }

    /// Lane skip-11 (T-SKU.14): how scans of the table at `root_page` ask its summary for an IN
    /// list's rows, if a summary is kept.
    pub fn list_probe(&self, root_page: i64) -> Option<ListProbe> {
        let entries = self.entries.lock();
        entries
            .iter()
            .find(|entry| entry.root_page == root_page)
            .map(|entry| entry.list_probe)
    }

    /// Marks the table at `root_page`'s summary stale: a write reached it that it couldn't take.
    pub fn mark_stale(&self, root_page: i64) {
        let mut entries = self.entries.lock();
        for entry in entries
            .iter_mut()
            .filter(|entry| entry.root_page == root_page)
        {
            entry.stale = true;
        }
    }

    /// The column index summarized for the table at `root_page`, if one is.
    pub fn column_of(&self, root_page: i64) -> Option<usize> {
        let entries = self.entries.lock();
        entries
            .iter()
            .find(|entry| entry.root_page == root_page)
            .map(|entry| entry.column)
    }

    /// The write hook: `writer` (the connection's identity, its snapshot the newest commit
    /// `committed`, as it holds the write lock) wrote one row of the table at `root_page`. A write
    /// the summary refuses makes it stale.
    pub fn on_write(
        &self,
        root_page: i64,
        writer: usize,
        committed: WalPos,
        write: RowWrite,
        now: &mut dyn FnMut() -> u64,
    ) {
        self.write_by(root_page, writer, committed, false, write, now)
    }

    /// The write hook on the versioned storage (G7 step 2, `AC-NS.2.1`): as
    /// [`SkipRegistry::on_write`], by `writer` whose snapshot is the newest commit `committed`,
    /// whose end the caller tells with [`SkipRegistry::committed`] or
    /// [`SkipRegistry::rolled_back`]. A write while another writer's is open makes the summary
    /// stale: it would hold two open transactions' rows.
    pub fn on_write_told(
        &self,
        root_page: i64,
        writer: usize,
        committed: WalPos,
        write: RowWrite,
        now: &mut dyn FnMut() -> u64,
    ) {
        self.write_by(root_page, writer, committed, true, write, now)
    }

    fn write_by(
        &self,
        root_page: i64,
        writer: usize,
        committed: WalPos,
        told: bool,
        write: RowWrite,
        now: &mut dyn FnMut() -> u64,
    ) {
        let mut entries = self.entries.lock();
        let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.root_page == root_page)
        else {
            return;
        };
        if entry
            .pending
            .as_ref()
            .is_some_and(|p| p.told && p.writer != writer)
        {
            entry.stale = true;
        }
        entry.pending = Some(Pending {
            writer,
            base: committed,
            told,
        });
        entry.counts.writes = entry.counts.writes.saturating_add(1);
        // Lane skip-8 (T-SKU.10): the write is the event that voids a build under way; lane
        // skip-9 (T-SKU.11): a replaying build records it instead, within its bound.
        if let Some(building) = entry.building.as_mut() {
            let overflowed = building.take_write(write);
            if overflowed {
                entry.counts.replay_overflows = entry.counts.replay_overflows.saturating_add(1);
                tracing::debug!(target: "amphora.skipping", root = root_page, bound = building.bound, "skipping build's replay record reached its bound (the rows the summary held); the build falls back to void");
            } else if let Some(record) = building.record.as_ref() {
                let held = u64::try_from(record.len()).unwrap_or(u64::MAX);
                entry.counts.replay_peak = entry.counts.replay_peak.max(held);
            }
        }
        if entry.stale {
            return;
        }
        if entry.skipping.on_write_timed(write, now).is_err() {
            entry.stale = true;
        }
    }

    /// `writer` committed its transaction on the versioned storage (G7 step 2, `AC-NS.2.1`): the
    /// newest commit holds the writes each summary took from it.
    pub fn committed(&self, writer: usize) {
        let mut entries = self.entries.lock();
        for entry in entries.iter_mut() {
            if entry.pending.as_ref().is_some_and(|p| p.writer == writer) {
                entry.pending = None;
            }
        }
    }

    /// `writer` rolled back a transaction or a statement: each summary that took its writes is
    /// stale.
    pub fn rolled_back(&self, writer: usize) {
        let mut entries = self.entries.lock();
        for entry in entries.iter_mut() {
            if entry.pending.as_ref().is_some_and(|p| p.writer == writer) {
                entry.stale = true;
            }
        }
    }

    /// The rows a filter `lo <= value <= hi` on the summarized column of the table at
    /// `root_page` may need, for `reader` at snapshot `reader_pos` when the newest commit is
    /// `committed`; refused (scan every row) unless the reader sees exactly what the summary holds.
    pub fn candidates(
        &self,
        root_page: i64,
        reader: usize,
        reader_pos: WalPos,
        committed: WalPos,
        lo: i64,
        hi: i64,
    ) -> Result<Candidates, SkipRefusal> {
        self.candidates_by(root_page, reader, reader_pos, committed, lo, hi)
            .map(|(candidates, _)| candidates)
    }

    /// [`SkipRegistry::candidates`], with the method a measured summary answered by: hand the
    /// scan's cost back with [`SkipRegistry::record_cost`] (lane skip-3, T-SKU.2).
    pub fn candidates_by(
        &self,
        root_page: i64,
        reader: usize,
        reader_pos: WalPos,
        committed: WalPos,
        lo: i64,
        hi: i64,
    ) -> Result<(Candidates, Option<SkipMethod>), SkipRefusal> {
        self.candidates_spans(root_page, reader, reader_pos, committed, &[(lo, hi)])
    }

    /// Lane skip-11 (T-SKU.14): [`SkipRegistry::candidates_by`] for a filter whose rows lie in
    /// any of `spans` (each `(lo, hi)`, inclusive): every span probed, the candidates unioned, and
    /// the whole scan one filter to a measured summary's chooser (one method, one cost back).
    pub fn candidates_spans(
        &self,
        root_page: i64,
        reader: usize,
        reader_pos: WalPos,
        committed: WalPos,
        spans: &[(i64, i64)],
    ) -> Result<(Candidates, Option<SkipMethod>), SkipRefusal> {
        let mut entries = self.entries.lock();
        let entry = entries
            .iter_mut()
            .find(|entry| entry.root_page == root_page)
            .ok_or(SkipRefusal::Off)?;
        if entry.stale {
            entry.counts.refused_stale = entry.counts.refused_stale.saturating_add(1);
            return Err(SkipRefusal::Stale);
        }
        let own_write = match &entry.pending {
            Some(p) if p.writer == reader => true,
            // The writer committed since its writes: the newest commit holds them.
            Some(p) if !p.told && p.base != committed => {
                entry.pending = None;
                false
            }
            Some(_) => {
                entry.counts.refused_older = entry.counts.refused_older.saturating_add(1);
                return Err(SkipRefusal::OlderSnapshot);
            }
            None => false,
        };
        if !own_write && reader_pos != committed {
            entry.counts.refused_older = entry.counts.refused_older.saturating_add(1);
            return Err(SkipRefusal::OlderSnapshot);
        }
        match entry.skipping.candidates_spans(spans) {
            Ok(candidates) => {
                entry.counts.consulted = entry.counts.consulted.saturating_add(1);
                let probes = u64::try_from(spans.len()).unwrap_or(u64::MAX);
                entry.counts.probes = entry.counts.probes.saturating_add(probes);
                Ok(candidates)
            }
            Err(SkipError::Overflow)
            | Err(SkipError::Inconsistent)
            | Err(SkipError::MissingRow { .. })
            | Err(SkipError::RowidOrder { .. }) => {
                entry.stale = true;
                entry.counts.refused_stale = entry.counts.refused_stale.saturating_add(1);
                Err(SkipRefusal::Stale)
            }
        }
    }

    /// A scan of the table at `root_page` answered by `method` cost `cost` (nanoseconds, from
    /// asking for its candidates to its end): the measured summary's chooser takes it. A summary
    /// replaced since (or not measured) ignores it.
    pub fn record_cost(&self, root_page: i64, method: SkipMethod, cost: u64) {
        let mut entries = self.entries.lock();
        if let Some(Skipping::Measured(chooser)) = entries
            .iter_mut()
            .find(|entry| entry.root_page == root_page)
            .map(|entry| &mut entry.skipping)
        {
            chooser.record(method, cost);
        }
    }

    /// Lane skip-5 (T-SKU.5): the dropped method a measured summary's trial waits on, if any.
    pub fn rebuild_due(&self, root_page: i64) -> Option<SkipMethod> {
        let entries = self.entries.lock();
        match entries.iter().find(|entry| entry.root_page == root_page) {
            Some(Entry {
                skipping: Skipping::Measured(chooser),
                stale: false,
                ..
            }) => chooser.rebuild_due(),
            _ => None,
        }
    }

    /// Lane skip-8 (T-SKU.10): claims the build of the table at `root_page`'s dropped method
    /// whose trial is due. `None` when none is due, the summary is stale, or another builder holds
    /// the claim: the caller goes on with the leader.
    pub fn claim_build(&self, root_page: i64) -> Option<BuildClaim> {
        let mut entries = self.entries.lock();
        let entry = entries
            .iter_mut()
            .find(|entry| entry.root_page == root_page)?;
        if entry.stale || entry.building.is_some() {
            return None;
        }
        let Skipping::Measured(chooser) = &entry.skipping else {
            return None;
        };
        let method = chooser.rebuild_due()?;
        let ticket = {
            let mut tickets = self.tickets.lock();
            *tickets = tickets.wrapping_add(1);
            *tickets
        };
        entry.building = Some(Building {
            ticket,
            invalidated: false,
            record: None,
            bound: 0,
        });
        Some(BuildClaim {
            root_page,
            method,
            ticket,
        })
    }

    /// Lane skip-8 (T-SKU.10): `builder`, at snapshot `builder_pos` when the newest commit is
    /// `committed`, reads the table for `claim`'s build: `true` when its snapshot is exactly the
    /// state the summary holds (by the reader's rule: no other writer open, the newest commit),
    /// and the build counts writes from here. `false`: hand the claim back.
    pub fn pin_build(
        &self,
        claim: &BuildClaim,
        builder: usize,
        builder_pos: WalPos,
        committed: WalPos,
    ) -> bool {
        let mut entries = self.entries.lock();
        let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.root_page == claim.root_page)
        else {
            return false;
        };
        if entry.stale
            || !entry
                .building
                .as_ref()
                .is_some_and(|building| building.ticket == claim.ticket && !building.invalidated)
        {
            return false;
        }
        match &entry.pending {
            Some(p) if p.writer == builder => {}
            // Only the single-writer WAL can infer a commit from the position.
            Some(p) if !p.told && p.base != committed => entry.pending = None,
            Some(_) => return false,
            None => {}
        }
        if builder_pos != committed {
            return false;
        }
        let replay = entry.build_mode == BuildMode::Replay;
        let bound = held_rows(&entry.skipping);
        if let Some(building) = entry.building.as_mut() {
            building.record = replay.then(Vec::new);
            building.bound = bound;
        }
        true
    }

    /// Lane skip-9 (T-SKU.11): how the table at `root_page`'s dropped method is built again, if
    /// a summary is kept.
    pub fn build_mode(&self, root_page: i64) -> Option<BuildMode> {
        let entries = self.entries.lock();
        entries
            .iter()
            .find(|entry| entry.root_page == root_page)
            .map(|entry| entry.build_mode)
    }

    /// Takes `claim`'s build off its entry: the entry and the build, when the claim is still the
    /// one under way and the summary isn't stale.
    fn end_build<'a>(
        entries: &'a mut [Entry],
        claim: &BuildClaim,
    ) -> Result<(&'a mut Entry, Building), BuildEnd> {
        let entry = entries
            .iter_mut()
            .find(|entry| entry.root_page == claim.root_page)
            .ok_or(BuildEnd::Superseded)?;
        match entry.building.take() {
            Some(building) if building.ticket == claim.ticket => {
                if entry.stale {
                    Err(BuildEnd::Superseded)
                } else {
                    Ok((entry, building))
                }
            }
            other => {
                entry.building = other;
                Err(BuildEnd::Superseded)
            }
        }
    }

    /// Lane skip-8 (T-SKU.10): hands `claim` back with nothing built.
    pub fn abandon_build(&self, claim: BuildClaim) {
        let mut entries = self.entries.lock();
        let current = Self::end_build(&mut entries, &claim).is_ok();
        tracing::debug!(target: "amphora.skipping", root = claim.root_page, method = ?claim.method, current, "skipping build handed back with nothing built");
    }

    /// Builds `claim`'s dropped zone map again from the cracker, timed by `now`. The cracker is
    /// the summary's own copy and took every write, and the build runs under the lock, so no
    /// write invalidates it.
    pub fn rebuild_zones(&self, claim: BuildClaim, now: &mut dyn FnMut() -> u64) -> BuildEnd {
        let mut entries = self.entries.lock();
        let entry = match Self::end_build(&mut entries, &claim) {
            Ok((entry, _)) => entry,
            Err(end) => return end,
        };
        match &mut entry.skipping {
            Skipping::Measured(chooser) if claim.method == SkipMethod::ZoneMaps => {
                if chooser.rebuild_zones(now).is_err() {
                    entry.stale = true;
                    return BuildEnd::Refused;
                }
                entry.counts.builds_installed = entry.counts.builds_installed.saturating_add(1);
                BuildEnd::Installed
            }
            _ => BuildEnd::Superseded,
        }
    }

    /// Installs the cracker `claim`'s builder read from the table at its pinned snapshot, its read
    /// costing `cost`; not when a write came in since the pin ([`BuildEnd::Invalidated`]), as the
    /// copy would miss it. Lane skip-7 (T-SKU.8): it is cut at the dropped cracker's cut points as
    /// it is installed, timed by `now`; a cut it refuses makes the summary stale. Lane skip-9
    /// (T-SKU.11): a replaying build's recorded writes are applied to the copy first, in the order
    /// the hook took them, their time added to the build's cost.
    pub fn install_cracker(
        &self,
        claim: BuildClaim,
        mut cracker: CrackerColumn,
        cost: u64,
        now: &mut dyn FnMut() -> u64,
    ) -> BuildEnd {
        let mut entries = self.entries.lock();
        let (entry, building) = match Self::end_build(&mut entries, &claim) {
            Ok(ended) => ended,
            Err(end) => return end,
        };
        if building.invalidated {
            entry.counts.builds_invalidated = entry.counts.builds_invalidated.saturating_add(1);
            return BuildEnd::Invalidated;
        }
        let Skipping::Measured(chooser) = &mut entry.skipping else {
            return BuildEnd::Superseded;
        };
        if claim.method != SkipMethod::Cracking {
            return BuildEnd::Superseded;
        }
        let replay_started = now();
        let record = building.record.unwrap_or_default();
        let replayed = u64::try_from(record.len()).unwrap_or(u64::MAX);
        for write in record {
            if let Err(error) = cracker.on_write(write) {
                // The copy disagrees with a write the summary took: the copy is dropped, the
                // summary (which took the write) is kept.
                tracing::debug!(target: "amphora.skipping", root = claim.root_page, %error, "skipping build's copy refused a recorded write; nothing installed");
                return BuildEnd::ReplayRefused;
            }
        }
        let cost = cost.saturating_add(now().saturating_sub(replay_started));
        if chooser.install_cracker(cracker, cost, now).is_err() {
            entry.stale = true;
            return BuildEnd::Refused;
        }
        entry.counts.builds_installed = entry.counts.builds_installed.saturating_add(1);
        entry.counts.replayed_writes = entry.counts.replayed_writes.saturating_add(replayed);
        BuildEnd::Installed
    }

    /// Lane skip-9 (T-SKU.11): an inline build read `rows` rows on a scan's path.
    pub fn count_inline_read(&self, root_page: i64, rows: u64) {
        let mut entries = self.entries.lock();
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.root_page == root_page)
        {
            entry.counts.inline_rows_read = entry.counts.inline_rows_read.saturating_add(rows);
        }
    }

    /// Lane skip-8: whether a build of the table at `root_page`'s summary is claimed now.
    pub fn building(&self, root_page: i64) -> bool {
        let entries = self.entries.lock();
        entries
            .iter()
            .any(|entry| entry.root_page == root_page && entry.building.is_some())
    }

    /// What the table at `root_page`'s summary has done, if one is kept.
    pub fn counts(&self, root_page: i64) -> Option<SkipCounts> {
        let entries = self.entries.lock();
        entries
            .iter()
            .find(|entry| entry.root_page == root_page)
            .map(|entry| entry.counts)
    }

    /// The table at `root_page`'s summary's own statistics: zones read and skipped, or cracks.
    pub fn with_summary<T>(&self, root_page: i64, f: impl FnOnce(&Skipping) -> T) -> Option<T> {
        let entries = self.entries.lock();
        entries
            .iter()
            .find(|entry| entry.root_page == root_page)
            .map(|entry| f(&entry.skipping))
    }
}

/// Lane skip-9 (T-SKU.11): the rows a summary holds, by the method it keeps (a replaying build's
/// bound). Zero when it holds none or can't say (a count past its type): such a build replays
/// nothing and falls back to void at its first write.
fn held_rows(skipping: &Skipping) -> u64 {
    let Skipping::Measured(chooser) = skipping else {
        return 0;
    };
    match (chooser.zone_map(), chooser.cracker()) {
        (Some(zones), _) => zones.rows().unwrap_or(0),
        (None, Some(cracker)) => cracker.row_count(),
        (None, None) => 0,
    }
}
