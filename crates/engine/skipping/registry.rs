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

use super::{Candidates, RowWrite, SkipError, SkipMethod, Skipping};
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
    /// Builds kept stale because rows were missing from them (G7 step 2, `AC-NS.2.1`): another
    /// open transaction had changed the table, or a commit passed the build's snapshot.
    pub built_stale: u64,
}

#[derive(Debug)]
struct Pending {
    writer: usize,
    base: WalPos,
    /// Its end is told, not read from the positions: on the versioned storage another writer may
    /// commit while it is open, so a newer commit doesn't mean it committed.
    told: bool,
}

#[derive(Debug)]
struct Entry {
    root_page: i64,
    column: usize,
    skipping: Skipping,
    pending: Option<Pending>,
    stale: bool,
    counts: SkipCounts,
}

/// The summaries one database keeps, by table root page.
pub struct SkipRegistry {
    entries: Mutex<Vec<Entry>>,
    /// How many summaries were installed, moved under the entries' lock (G7 step 2,
    /// `AC-NS.2.1`): a write that found none to feed reads it before and after its row changes.
    installs: AtomicU64,
}

impl Default for SkipRegistry {
    fn default() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
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
    /// Keeps `skipping` for column `column` (its index in the table's records) of the table at
    /// `root_page`, replacing any summary it had; `Skipping::Off` drops it. The caller built it at
    /// the newest commit with no writer open (it held the write lock).
    pub fn set(&self, root_page: i64, column: usize, skipping: Skipping) {
        self.set_checked(root_page, column, skipping, || None);
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
        });
        why
    }

    /// How many summaries were installed so far (G7 step 2, `AC-NS.2.1`). A write that found no
    /// summary for its table reads it before its row changes and again after: if it moved, a
    /// build may have missed the row, and the write looks for the table's summary again.
    pub fn installs(&self) -> u64 {
        self.installs.load(Ordering::SeqCst)
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
        match entry.skipping.candidates_by(lo, hi) {
            Ok(candidates) => {
                entry.counts.consulted = entry.counts.consulted.saturating_add(1);
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

    /// What the table at `root_page`'s summary has done, if one is kept.
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

    /// Builds a dropped zone map again from the cracker (in memory, under the lock).
    pub fn rebuild_zones(&self, root_page: i64, now: &mut dyn FnMut() -> u64) {
        let mut entries = self.entries.lock();
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.root_page == root_page)
        {
            if let Skipping::Measured(chooser) = &mut entry.skipping {
                if chooser.rebuild_zones(now).is_err() {
                    entry.stale = true;
                }
            }
        }
    }

    /// Installs a cracker the caller built again from the table, read when `writes` writes had
    /// come in: refused (`false`) when another came in since, as the copy would miss it.
    pub fn install_cracker(
        &self,
        root_page: i64,
        writes: u64,
        cracker: super::CrackerColumn,
        cost: u64,
    ) -> bool {
        let mut entries = self.entries.lock();
        let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.root_page == root_page)
        else {
            return false;
        };
        match &mut entry.skipping {
            Skipping::Measured(chooser) if entry.counts.writes == writes && !entry.stale => {
                chooser.install_cracker(cracker, cost);
                true
            }
            _ => false,
        }
    }

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
