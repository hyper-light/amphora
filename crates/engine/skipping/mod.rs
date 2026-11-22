//! Amphora (G5 step 4, AC-G5.4.1, T-G5.4.1): skipping rows a filter can't match, two prototypes
//! behind one switch ([`Skipping`]), each kept from the write path through one narrow hook
//! ([`Skipping::on_write`]):
//!
//! - [`ZoneMap`]: a min/max summary over each run of rows a table leaf holds (ClickHouse's minmax
//!   skipping index over granules, DuckDB's zone maps over row groups). A scan reads only the rowid
//!   ranges whose summary can match.
//! - [`CrackerColumn`]: database cracking (Idreos, Kersten and Manegold, CIDR 2007; updates by
//!   rippling, SIGMOD 2007): a copy of one unindexed column that each filter partitions further, so
//!   a filter repeated on it reads only the piece its bounds cut out.
//!
//! Both are pure cores (sans-IO, no clock): the caller hands them the rows the B-tree holds and every
//! write as it finishes, and asks which rows a filter may need. Neither decides a row matches: they
//! name the candidates, and the caller evaluates the filter on each as it does in a plain scan, so a
//! summary can be loose (it widens and never narrows on a delete) but never misses a row.
//!
//! A value is summarized only when it is an integer (`Some(i64)`); any other value (NULL, a real, a
//! text, a blob) is `None` and makes its zone, or its row in the cracker, a candidate for every
//! filter. That keeps SQLite's comparison rules (affinity, mixed types) out of the summaries: they
//! skip only rows whose integer value the filter's integer bounds exclude.
//!
//! Sizes: a zone's rows are a leaf's rows, as the builder is told where each leaf ends (no picked
//! run size; [`zone_map_of_table`] walks the table's own leaves); rows appended past the last
//! zone start a new zone once the tail holds as many rows as the built runs held on average (the
//! leaves' measured fill). The cracker's pieces are cut where the filters asked, no smaller or
//! larger by any picked rule.
//!
//! The switch is per table and per column: a connection that turns neither on pays nothing.

mod choose;
mod cracking;
mod leaves;
mod registry;
mod scan;
mod zone_map;

pub use choose::{Chooser, ChooserStats, MethodStats, SkipMethod, Upkeep};
pub use cracking::{CrackerColumn, CrackerStats};
pub(crate) use leaves::{cracker_of_table, nanos_since};
pub use leaves::{
    set_skipping, set_skipping_until, skipping_candidates, skipping_counts, zone_map_of_table,
    LeafBuildError, SkipKind,
};
pub use registry::{BuildPolicy, SkipCounts, SkipRefusal, SkipRegistry, WalPos};
pub use scan::{
    bound_terms, BoundOp, BoundTerm, BoundValue, ColumnBounds, SkipHint, SkipWalk, SkipWalks,
    WalkMeasure, WalkPhase, WalkTarget,
};
pub use zone_map::{RowidRange, ZoneMap, ZoneMapBuilder, ZoneStats};

/// One finished write to the summarized table, as the write path hands it to the hook: the row's
/// rowid and its summarized column's value before and after (`None`: not an integer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowWrite {
    Insert {
        rowid: i64,
        value: Option<i64>,
    },
    Delete {
        rowid: i64,
        old: Option<i64>,
    },
    Update {
        rowid: i64,
        old: Option<i64>,
        new: Option<i64>,
    },
}

/// What a summary refuses, with why.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkipError {
    /// The builder was handed rowids out of the B-tree's order: a leaf run must ascend.
    #[error(
        "skipping summary: rowid {rowid} came after {previous}; a table's rows ascend by rowid"
    )]
    RowidOrder { previous: i64, rowid: i64 },
    /// A delete or update named a row the summary never held: the write path and the summary
    /// disagree, so the summary can't be trusted and must be rebuilt.
    #[error(
        "skipping summary: no row {rowid} with value {value:?} to remove; rebuild the summary"
    )]
    MissingRow { rowid: i64, value: Option<i64> },
    /// The summary's positions disagree with its entries: rebuild it.
    #[error("skipping summary: its positions disagree with its entries; rebuild the summary")]
    Inconsistent,
    /// A count would pass what its type holds.
    #[error("skipping summary: a count overflowed")]
    Overflow,
}

/// The switch (G5 step 4's prototypes): which summary, if any, the hook keeps for a column.
#[derive(Debug, Default)]
pub enum Skipping {
    #[default]
    Off,
    ZoneMaps(ZoneMap),
    Cracking(CrackerColumn),
    /// Both, each filter answered by whichever measures cheaper (lane skip-3, T-SKU.2;
    /// `choose.rs`).
    Measured(Box<Chooser>),
}

/// The rows a filter `lo <= value <= hi` may need, as a summary names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Candidates {
    /// Every row: no summary is kept.
    All,
    /// Only rows whose rowid lies in one of these ranges (ascending, disjoint).
    Ranges(Vec<RowidRange>),
    /// Only these rowids (ascending).
    Rowids(Vec<i64>),
}

impl Skipping {
    /// The write path's hook: one call per row written, after the write. With the switch off it
    /// does nothing.
    pub fn on_write(&mut self, write: RowWrite) -> Result<(), SkipError> {
        match self {
            Skipping::Off => Ok(()),
            Skipping::ZoneMaps(zones) => zones.on_write(write),
            Skipping::Cracking(cracker) => cracker.on_write(write),
            Skipping::Measured(chooser) => chooser.on_write(write),
        }
    }

    /// The rows a filter `lo <= value <= hi` on the summarized column may need.
    pub fn candidates(&mut self, lo: i64, hi: i64) -> Result<Candidates, SkipError> {
        self.candidates_by(lo, hi).map(|(candidates, _)| candidates)
    }

    /// [`Skipping::on_write`], a measured summary's upkeep timed by `now` (lane skip-5, T-SKU.5).
    pub fn on_write_timed(
        &mut self,
        write: RowWrite,
        now: &mut dyn FnMut() -> u64,
    ) -> Result<(), SkipError> {
        match self {
            Skipping::Measured(chooser) => chooser.on_write_timed(write, now),
            other => other.on_write(write),
        }
    }

    /// [`Skipping::candidates`], with the method a [`Skipping::Measured`] answered by (its scan's
    /// cost goes back to [`Chooser::record`]).
    pub fn candidates_by(
        &mut self,
        lo: i64,
        hi: i64,
    ) -> Result<(Candidates, Option<SkipMethod>), SkipError> {
        Ok(match self {
            Skipping::Off => (Candidates::All, None),
            Skipping::ZoneMaps(zones) => (Candidates::Ranges(zones.candidate_ranges(lo, hi)), None),
            Skipping::Cracking(cracker) => {
                (Candidates::Rowids(cracker.candidate_rowids(lo, hi)?), None)
            }
            Skipping::Measured(chooser) => {
                let (candidates, method) = chooser.candidates(lo, hi)?;
                (candidates, Some(method))
            }
        })
    }
}

#[cfg(test)]
mod tests;
