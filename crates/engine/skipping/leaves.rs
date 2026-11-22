//! Amphora (G5 step 4, AC-G5.4.1): a zone map built from a table's own leaves. The cursor walks
//! the table in rowid order and a zone ends where the B-tree's leaf ends (the cursor stands on a
//! leaf's first cell), so each zone holds exactly what one leaf holds: the run sizes are the
//! table's measured ones, set by its page size and its rows' encoded bytes, never a picked number.
//!
//! This is the one place the skipping module reads storage; the summaries themselves stay pure.

use super::{
    BuildPolicy, Candidates, Chooser, CrackerColumn, SkipCounts, SkipError, SkipRefusal, Skipping,
    Upkeep, ZoneMap, ZoneMapBuilder,
};
use crate::numeric::Numeric;
use crate::storage::btree::{BTreeCursor, CursorTrait};
use crate::storage::versioned::cursor::VersionedCursor;
use crate::sync::Arc;
use crate::util::IOExt;
use crate::{Connection, IOResult, ValueRef};

/// What building from the leaves refuses, with why.
#[derive(Debug, thiserror::Error)]
pub enum LeafBuildError {
    /// Reading the table failed.
    #[error("skipping summary: reading the table's leaves failed: {0}")]
    Read(#[from] crate::LimboError),
    #[error(transparent)]
    Summary(#[from] SkipError),
    /// The cursor can't say which leaf its row is on: it shares its B-tree with another cursor or
    /// must restore its position. Open a cursor of its own for the build.
    #[error("skipping summary: the cursor can't say which leaf row {rowid} is on; build with a cursor of its own")]
    LeafUnknown { rowid: i64 },
    /// No such table with rows in a B-tree of its own (a view, a virtual table, or a table whose
    /// rows aren't checkpointed into its B-tree yet).
    #[error("skipping summary: no table {table} with a B-tree of its rows")]
    NoTable { table: String },
    /// No such column, or one the record doesn't hold at its place: the rowid's alias (a scan
    /// already reads rowid ranges) or a table with generated columns (its records skip them).
    #[error("skipping summary: table {table} has no stored column {column} to summarize")]
    NoColumn { table: String, column: String },
    /// A summary is built holding the write lock: the connection has a transaction open.
    #[error("skipping summary: end the open transaction first; a summary is built holding the write lock")]
    InTransaction,
    /// The database can't keep a summary.
    #[error(transparent)]
    Refused(#[from] SkipRefusal),
}

/// The table cursor a summary is built with: on the pager's B-tree, or on the versioned storage's
/// tree when the connection's B-trees are there (G7 step 2, `AC-NS.2.1`), whose root page is a
/// tree's id, not a page of the pager's.
pub(crate) enum LeafCursor {
    Pager(Box<BTreeCursor>),
    Versioned(Box<VersionedCursor>),
}

impl LeafCursor {
    fn open(conn: &Arc<Connection>, root: i64, columns: usize) -> Result<Self, LeafBuildError> {
        Ok(match conn.versioned_for(crate::MAIN_DB_ID) {
            Some(switch) => {
                Self::Versioned(Box::new(switch.cursor(root, None, conn.get_pager())?))
            }
            None => Self::Pager(Box::new(BTreeCursor::new_table(
                conn.get_pager(),
                root,
                columns,
            ))),
        })
    }

    fn get(&mut self) -> &mut dyn CursorTrait {
        match self {
            Self::Pager(cursor) => cursor.as_mut(),
            Self::Versioned(cursor) => cursor.as_mut(),
        }
    }

    /// Whether the cursor stands on its leaf's first row; None when it stands on no leaf's row.
    /// A versioned cursor walking forward copies each leaf from its first row, so its copy's
    /// first row is the leaf's.
    fn first_of_leaf(&self) -> Option<bool> {
        match self {
            Self::Pager(cursor) => cursor.table_leaf_under_cursor().map(|leaf| leaf.cell == 0),
            Self::Versioned(cursor) => cursor.table_leaf_under_cursor().map(|leaf| leaf.cell == 0),
        }
    }
}

/// The zone map of `column` of `table` on `conn`'s main database, one zone per leaf of its B-tree.
pub fn zone_map_of_table(
    conn: &Arc<Connection>,
    table: &str,
    column: &str,
) -> Result<ZoneMap, LeafBuildError> {
    let btree = conn
        .with_schema_mut(|schema| schema.get_btree_table(table))?
        .filter(|btree| btree.root_page > 0)
        .ok_or_else(|| LeafBuildError::NoTable {
            table: table.to_owned(),
        })?;
    let no_column = || LeafBuildError::NoColumn {
        table: table.to_owned(),
        column: column.to_owned(),
    };
    if btree.has_virtual_columns() {
        return Err(no_column());
    }
    let (index, _) = btree
        .get_column(column)
        .filter(|(_, c)| !c.is_rowid_alias())
        .ok_or_else(no_column)?;
    let mut cursor = LeafCursor::open(conn, btree.root_page, btree.columns().len())?;
    if !matches!(cursor, LeafCursor::Versioned(_)) || !conn.get_auto_commit() {
        return zone_map_from_leaves(&mut cursor, index);
    }
    // The versioned cursor reads in the connection's transaction: one begun for the build alone
    // is ended after it, so no snapshot outlives the read.
    drop(cursor);
    conn.execute("BEGIN")?;
    let built = LeafCursor::open(conn, btree.root_page, btree.columns().len())
        .and_then(|mut cursor| zone_map_from_leaves(&mut cursor, index));
    conn.execute("ROLLBACK")?;
    built
}

/// The zone map of column `column` (its index in the table's records) of the table `cursor`
/// walks, one zone per leaf. A value that isn't an integer, or a record shorter than `column`
/// (a column added after the row was written), is summarized as not an integer.
pub(crate) fn zone_map_from_leaves(
    cursor: &mut LeafCursor,
    column: usize,
) -> Result<ZoneMap, LeafBuildError> {
    let pager = cursor.get().get_pager();
    let mut builder = ZoneMapBuilder::new();
    pager.io.block(|| cursor.get().rewind())?;
    while let Some(rowid) = pager.io.block(|| cursor.get().rowid())? {
        let first_of_leaf = cursor
            .first_of_leaf()
            .ok_or(LeafBuildError::LeafUnknown { rowid })?;
        if first_of_leaf {
            builder.end_run();
        }
        let value = loop {
            match cursor.get().record().map_err(|e| *e)? {
                IOResult::Done(record) => {
                    break match record.and_then(|record| record.get_value_opt(column)) {
                        Some(ValueRef::Numeric(Numeric::Integer(value))) => Some(value),
                        _ => None,
                    };
                }
                IOResult::IO(io) => io.wait(&*pager.io)?,
            }
        };
        builder.row(rowid, value)?;
        pager.io.block(|| cursor.get().next())?;
    }
    Ok(builder.finish())
}

/// Which summary [`set_skipping`] keeps for a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipKind {
    ZoneMaps,
    Cracking,
    /// Both, each filter answered by whichever measures cheaper on this column (lane skip-3,
    /// T-SKU.2).
    Measured,
    /// [`SkipKind::Measured`], the method not leading dropped once its write upkeep passes its
    /// cost to build again (lane skip-5, T-SKU.5: [`super::Upkeep::Capped`]).
    MeasuredCapped,
}

/// The table's root page and the record index of its summarized column, refusing what can't be
/// summarized.
fn summarized_column(
    conn: &Arc<Connection>,
    table: &str,
    column: &str,
) -> Result<(i64, usize, usize), LeafBuildError> {
    let btree = conn
        .with_schema_mut(|schema| schema.get_btree_table(table))?
        .filter(|btree| btree.root_page > 0 && btree.has_rowid)
        .ok_or_else(|| LeafBuildError::NoTable {
            table: table.to_owned(),
        })?;
    let no_column = || LeafBuildError::NoColumn {
        table: table.to_owned(),
        column: column.to_owned(),
    };
    if btree.has_virtual_columns() {
        return Err(no_column());
    }
    let (index, _) = btree
        .get_column(column)
        .filter(|(_, c)| !c.is_rowid_alias())
        .ok_or_else(no_column)?;
    Ok((btree.root_page, index, btree.columns().len()))
}

/// Nanoseconds since `started`, saturating.
pub(crate) fn nanos_since(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Lane skip-5 (T-SKU.5): a dropped cracker built again from the table's B-tree (rooted at
/// `root`, `columns` wide) for its trial, at the caller's snapshot: on the versioned storage, in
/// the connection's transaction (G7 step 2, `AC-NS.2.1`).
pub(crate) fn cracker_of_table(
    conn: &Arc<Connection>,
    root: i64,
    column: usize,
    columns: usize,
) -> Result<CrackerColumn, LeafBuildError> {
    let mut cursor = LeafCursor::open(conn, root, columns)?;
    rows_of(&mut cursor, column).map(CrackerColumn::new)
}

/// Every row's rowid and its column `column`'s integer value, in rowid order.
fn rows_of(
    cursor: &mut LeafCursor,
    column: usize,
) -> Result<Vec<(i64, Option<i64>)>, LeafBuildError> {
    let pager = cursor.get().get_pager();
    let mut rows = Vec::new();
    pager.io.block(|| cursor.get().rewind())?;
    while let Some(rowid) = pager.io.block(|| cursor.get().rowid())? {
        let value = loop {
            match cursor.get().record().map_err(|e| *e)? {
                IOResult::Done(record) => {
                    break match record.and_then(|record| record.get_value_opt(column)) {
                        Some(ValueRef::Numeric(Numeric::Integer(value))) => Some(value),
                        _ => None,
                    };
                }
                IOResult::IO(io) => io.wait(&*pager.io)?,
            }
        };
        rows.push((rowid, value));
        pager.io.block(|| cursor.get().next())?;
    }
    Ok(rows)
}

/// Amphora (G5 step 4): keeps a summary of `column` of `table` (`None`: drops it), shared by every
/// connection of the database and kept current by the Insert and Delete opcodes. It is built
/// holding the write lock, so it holds exactly the newest commit; `conn` must have no
/// transaction open. On the versioned storage no lock holds writers off: see [`BuildPolicy`]. A
/// build that waits gives up at the connection's query timeout, as a statement's would.
pub fn set_skipping(
    conn: &Arc<Connection>,
    table: &str,
    column: &str,
    kind: Option<SkipKind>,
) -> Result<(), LeafBuildError> {
    let deadline = match conn.get_query_timeout_ms() {
        0 => None,
        millis => Some(
            conn.get_pager().io.current_time_monotonic() + std::time::Duration::from_millis(millis),
        ),
    };
    set_skipping_with_until(
        conn,
        table,
        column,
        kind,
        super::BuildMode::default(),
        deadline,
    )
}

/// Lane skip-9 (T-SKU.11): [`set_skipping`], a measured summary's dropped method built again by
/// `build` (inline, void-on-write or replay).
pub fn set_skipping_with(
    conn: &Arc<Connection>,
    table: &str,
    column: &str,
    kind: Option<SkipKind>,
    build: super::BuildMode,
) -> Result<(), LeafBuildError> {
    let deadline = match conn.get_query_timeout_ms() {
        0 => None,
        millis => Some(
            conn.get_pager().io.current_time_monotonic() + std::time::Duration::from_millis(millis),
        ),
    };
    set_skipping_with_until(conn, table, column, kind, build, deadline)
}

/// [`set_skipping`] with the deadline of the statement that asks for it (`PRAGMA skipping`'s
/// opcode): a build that waits for its table's writers gives up there (lane g7s2-17).
pub fn set_skipping_until(
    conn: &Arc<Connection>,
    table: &str,
    column: &str,
    kind: Option<SkipKind>,
    deadline: Option<crate::MonotonicInstant>,
) -> Result<(), LeafBuildError> {
    set_skipping_with_until(
        conn,
        table,
        column,
        kind,
        super::BuildMode::default(),
        deadline,
    )
}

/// [`set_skipping_with`]'s build mode and [`set_skipping_until`]'s deadline together: what
/// `PRAGMA skipping`'s opcode asks for.
pub fn set_skipping_with_until(
    conn: &Arc<Connection>,
    table: &str,
    column: &str,
    kind: Option<SkipKind>,
    build: super::BuildMode,
    deadline: Option<crate::MonotonicInstant>,
) -> Result<(), LeafBuildError> {
    if conn.mvcc_enabled() {
        return Err(LeafBuildError::Refused(super::SkipRefusal::Mvcc));
    }
    if !conn.get_auto_commit() {
        return Err(LeafBuildError::InTransaction);
    }
    let (root, index, columns) = summarized_column(conn, table, column)?;
    let Some(kind) = kind else {
        conn.db.skipping.set(root, index, Skipping::Off);
        return Ok(());
    };
    // G7 step 2 (`AC-NS.2.1`): on the versioned storage, under `BuildPolicy::Wait`, a build
    // waits for the table's open writers to end first, and is done again, after another wait,
    // if one slipped in beside it; under `Stale` (the control) it goes in stale. A wait refused
    // (lane g7s2-17: the writer is this thread's own, or the deadline passed) builds as stale
    // does, once.
    let switch = conn.versioned_for(crate::MAIN_DB_ID);
    let mut wait = switch.is_some() && conn.db.skipping.policy() == BuildPolicy::Wait;
    let mut again = false;
    loop {
        if let Some(switch) = switch.as_ref().filter(|_| wait) {
            let give_up = deadline.map(|deadline| {
                deadline.duration_since(conn.get_pager().io.current_time_monotonic())
            });
            let waited = switch.wait_unwritten(root, give_up)?;
            conn.db.skipping.count_wait(waited == Ok(true), again);
            if let Err(refused) = waited {
                conn.db.skipping.count_refused(refused);
                tracing::debug!(target: "amphora.skipping", table, column, %refused, "a waiting build was refused: built as stale builds");
                wait = false;
            }
        }
        conn.execute("BEGIN IMMEDIATE")?;
        let built = build_from_leaves(conn, root, columns, index, kind);
        // A test's pause point: the rows are read, the check for writers not yet made.
        #[cfg(test)]
        {
            if let Some(switch) = switch.as_ref() {
                switch.built(root);
            }
        }
        match built {
            Ok(skipping) => {
                let missed = match switch.as_ref() {
                    None => {
                        conn.db
                            .skipping
                            .set_with_build(root, index, skipping, build);
                        None
                    }
                    // The build there held no lock against writers. One that changed the table
                    // and is still open, or one that committed since the build's snapshot, left
                    // rows out of it: it is kept stale until built again.
                    Some(switch) => {
                        let missed = conn.db.skipping.set_checked_with_build(root, index, skipping, build, || {
                            switch.skipping_build_missed(root).unwrap_or_else(|e| {
                                tracing::warn!(target: "amphora.skipping", error = %e, "a build's check of open writers failed");
                                Some("the check of open writers failed")
                            })
                        });
                        if let Some(why) = missed {
                            tracing::debug!(target: "amphora.skipping", table, column, why, wait, "summary built stale");
                        }
                        missed
                    }
                };
                conn.execute("COMMIT")?;
                if wait && missed.is_some() {
                    again = true;
                    continue;
                }
                return Ok(());
            }
            Err(e) => {
                // The build's error is the one to report; the rollback ends a read-only
                // transaction.
                if let Err(rollback) = conn.execute("ROLLBACK") {
                    tracing::warn!(target: "amphora.skipping", error = %rollback, "rollback after a failed build");
                }
                return Err(e);
            }
        }
    }
}

/// A summary of `kind` of column `index`, read from the leaves of the table at `root` in the
/// connection's open transaction.
fn build_from_leaves(
    conn: &Arc<Connection>,
    root: i64,
    columns: usize,
    index: usize,
    kind: SkipKind,
) -> Result<Skipping, LeafBuildError> {
    LeafCursor::open(conn, root, columns).and_then(|mut cursor| match kind {
        SkipKind::ZoneMaps => zone_map_from_leaves(&mut cursor, index).map(Skipping::ZoneMaps),
        SkipKind::Cracking => {
            rows_of(&mut cursor, index).map(|rows| Skipping::Cracking(CrackerColumn::new(rows)))
        }
        SkipKind::Measured | SkipKind::MeasuredCapped => {
            // Lane skip-5 (T-SKU.5): each build timed, the cost to build it again if dropped.
            let started = std::time::Instant::now();
            zone_map_from_leaves(&mut cursor, index).and_then(|zones| {
                let zones_cost = nanos_since(started);
                let started = std::time::Instant::now();
                rows_of(&mut cursor, index).map(|rows| {
                    let cracker = CrackerColumn::new(rows);
                    let upkeep = if kind == SkipKind::MeasuredCapped {
                        Upkeep::Capped
                    } else {
                        Upkeep::KeepBoth
                    };
                    let cost = [Some(zones_cost), Some(nanos_since(started))];
                    Skipping::Measured(Box::new(Chooser::with_upkeep(zones, cracker, upkeep, cost)))
                })
            })
        }
    })
}

/// Amphora (G5 step 4): the rows a filter `lo <= column <= hi` on `table` may need, as its
/// summary names them for `conn` at its current snapshot; refused (scan every row) unless `conn`
/// sees exactly what the summary holds.
pub fn skipping_candidates(
    conn: &Arc<Connection>,
    table: &str,
    lo: i64,
    hi: i64,
) -> Result<Candidates, SkipRefusal> {
    if conn.mvcc_enabled() {
        return Err(SkipRefusal::Mvcc);
    }
    let root = conn
        .with_schema_mut(|schema| schema.get_btree_table(table))
        .ok()
        .flatten()
        .map(|btree| btree.root_page)
        .ok_or(SkipRefusal::Off)?;
    let (reader_pos, committed) = conn.skipping_positions().ok_or(SkipRefusal::Off)?;
    conn.db
        .skipping
        .candidates(root, conn.skipping_id(), reader_pos, committed, lo, hi)
}

/// Lane skip-11 (T-SKU.14): how scans of `table` ask its summary for an IN list's rows (span or
/// value by value); `false` when no summary is kept.
pub fn set_list_probe(conn: &Arc<Connection>, table: &str, probe: super::ListProbe) -> bool {
    conn.with_schema_mut(|schema| schema.get_btree_table(table))
        .ok()
        .flatten()
        .is_some_and(|btree| conn.db.skipping.set_list_probe(btree.root_page, probe))
}

/// Amphora (G5 step 4): what `table`'s summary has done, if one is kept.
pub fn skipping_counts(conn: &Arc<Connection>, table: &str) -> Option<SkipCounts> {
    let root = conn
        .with_schema_mut(|schema| schema.get_btree_table(table))
        .ok()
        .flatten()?
        .root_page;
    conn.db.skipping.counts(root)
}
