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

use super::{Candidates, RowWrite, SkipError, SkipMethod, Skipping};
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
}

#[derive(Debug)]
struct Pending {
    writer: usize,
    base: WalPos,
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
#[derive(Debug, Default)]
pub struct SkipRegistry {
    entries: Mutex<Vec<Entry>>,
}

impl SkipRegistry {
    /// Keeps `skipping` for column `column` (its index in the table's records) of the table at
    /// `root_page`, replacing any summary it had; `Skipping::Off` drops it. The caller built it at
    /// the newest commit with no writer open (it held the write lock).
    pub fn set(&self, root_page: i64, column: usize, skipping: Skipping) {
        let mut entries = self.entries.lock();
        entries.retain(|entry| entry.root_page != root_page);
        if !matches!(skipping, Skipping::Off) {
            entries.push(Entry {
                root_page,
                column,
                skipping,
                pending: None,
                stale: false,
                counts: SkipCounts::default(),
            });
        }
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
    pub fn on_write(&self, root_page: i64, writer: usize, committed: WalPos, write: RowWrite) {
        let mut entries = self.entries.lock();
        let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.root_page == root_page)
        else {
            return;
        };
        entry.pending = Some(Pending {
            writer,
            base: committed,
        });
        entry.counts.writes = entry.counts.writes.saturating_add(1);
        if entry.stale {
            return;
        }
        if entry.skipping.on_write(write).is_err() {
            entry.stale = true;
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
            Some(p) if p.base != committed => {
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
