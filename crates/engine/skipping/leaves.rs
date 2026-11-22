//! Amphora (G5 step 4, AC-G5.4.1): a zone map built from a table's own leaves. The cursor walks
//! the table in rowid order and a zone ends where the B-tree's leaf ends (the cursor stands on a
//! leaf's first cell), so each zone holds exactly what one leaf holds: the run sizes are the
//! table's measured ones, set by its page size and its rows' encoded bytes, never a picked number.
//!
//! This is the one place the skipping module reads storage; the summaries themselves stay pure.

use super::{
    Candidates, CrackerColumn, SkipCounts, SkipError, SkipRefusal, Skipping, ZoneMap,
    ZoneMapBuilder,
};
use crate::numeric::Numeric;
use crate::storage::btree::{BTreeCursor, CursorTrait};
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
    let mut cursor =
        BTreeCursor::new_table(conn.get_pager(), btree.root_page, btree.columns().len());
    zone_map_from_leaves(&mut cursor, index)
}

/// The zone map of column `column` (its index in the table's records) of the table `cursor`
/// walks, one zone per leaf. A value that isn't an integer, or a record shorter than `column`
/// (a column added after the row was written), is summarized as not an integer.
pub(crate) fn zone_map_from_leaves(
    cursor: &mut BTreeCursor,
    column: usize,
) -> Result<ZoneMap, LeafBuildError> {
    let pager = cursor.get_pager();
    let mut builder = ZoneMapBuilder::new();
    pager.io.block(|| cursor.rewind())?;
    while let Some(rowid) = pager.io.block(|| cursor.rowid())? {
        let first_of_leaf = cursor
            .table_leaf_under_cursor()
            .map(|leaf| leaf.cell == 0)
            .ok_or(LeafBuildError::LeafUnknown { rowid })?;
        if first_of_leaf {
            builder.end_run();
        }
        let value = loop {
            match cursor.record().map_err(|e| *e)? {
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
        pager.io.block(|| cursor.next())?;
    }
    Ok(builder.finish())
}

/// Which summary [`set_skipping`] keeps for a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipKind {
    ZoneMaps,
    Cracking,
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

/// Every row's rowid and its column `column`'s integer value, in rowid order.
fn rows_of(
    cursor: &mut BTreeCursor,
    column: usize,
) -> Result<Vec<(i64, Option<i64>)>, LeafBuildError> {
    let pager = cursor.get_pager();
    let mut rows = Vec::new();
    pager.io.block(|| cursor.rewind())?;
    while let Some(rowid) = pager.io.block(|| cursor.rowid())? {
        let value = loop {
            match cursor.record().map_err(|e| *e)? {
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
        pager.io.block(|| cursor.next())?;
    }
    Ok(rows)
}

/// Amphora (G5 step 4): keeps a summary of `column` of `table` (`None`: drops it), shared by every
/// connection of the database and kept current by the Insert and Delete opcodes. It is built
/// holding the write lock, so it holds exactly the newest commit; `conn` must have no
/// transaction open.
pub fn set_skipping(
    conn: &Arc<Connection>,
    table: &str,
    column: &str,
    kind: Option<SkipKind>,
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
    conn.execute("BEGIN IMMEDIATE")?;
    let mut cursor = BTreeCursor::new_table(conn.get_pager(), root, columns);
    let built = match kind {
        SkipKind::ZoneMaps => zone_map_from_leaves(&mut cursor, index).map(Skipping::ZoneMaps),
        SkipKind::Cracking => {
            rows_of(&mut cursor, index).map(|rows| Skipping::Cracking(CrackerColumn::new(rows)))
        }
    };
    match built {
        Ok(skipping) => {
            conn.db.skipping.set(root, index, skipping);
            conn.execute("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            // The build's error is the one to report; the rollback ends a read-only transaction.
            if let Err(rollback) = conn.execute("ROLLBACK") {
                tracing::warn!(target: "amphora.skipping", error = %rollback, "rollback after a failed build");
            }
            Err(e)
        }
    }
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

/// Amphora (G5 step 4): what `table`'s summary has done, if one is kept.
pub fn skipping_counts(conn: &Arc<Connection>, table: &str) -> Option<SkipCounts> {
    let root = conn
        .with_schema_mut(|schema| schema.get_btree_table(table))
        .ok()
        .flatten()?
        .root_page;
    conn.db.skipping.counts(root)
}
