//! Amphora (G5 step 4, T-G5.4.3; executor.md decision 11): a SQL filter's table scan skipping the
//! rows its summary rules out.
//!
//! Two halves. At translation, [`integer_bounds`] reads a SELECT's WHERE terms for each column's
//! integer bounds (`a BETWEEN 10 AND 20`, `a >= 10`, `5 < a`, `a = 7`), and the scan's cursor is
//! given a [`SkipHint`] with them; this costs a scan nothing when no summary is kept. At run time
//! the Rewind asks the database's summary for the hinted column (if one is kept and the reader
//! sees exactly what it holds), and a [`SkipWalk`] then moves the cursor past each run of rows the
//! candidates leave out with one seek, instead of reading them. The filter is still evaluated on
//! every row read, as in a plain scan: the candidates only leave out rows it can't match.
//!
//! Only rows whose value is an integer are left out: a NULL, real, text or blob value makes its
//! row a candidate (the summaries' rule, `skipping/mod.rs`), so SQLite's affinity and mixed-type
//! comparisons never reach the summaries. A term that isn't a plain integer comparison (a real or
//! text constant, a parameter, a COLLATE, an IN list, NOT BETWEEN) gives no bound: loose, never
//! wrong.

use super::{Candidates, RowidRange};
use amphora_engine_parser::ast::{Expr, Literal, Operator, TableInternalId, UnaryOperator};

/// One column's integer bounds, from a filter's AND-ed terms: a row it can match has
/// `lo <= value <= hi` (or a value that isn't an integer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnBounds {
    pub column: usize,
    pub lo: i64,
    pub hi: i64,
}

/// What a scan's cursor may skip by, set at translation: each column's integer bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipHint {
    pub cursor_id: usize,
    pub bounds: Vec<ColumnBounds>,
}

/// An integer constant: a numeric literal that is an `i64`, with an optional sign.
fn integer_of(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Literal(Literal::Numeric(text)) => text.parse::<i64>().ok(),
        Expr::Unary(UnaryOperator::Negative, inner) => match inner.as_ref() {
            Expr::Literal(Literal::Numeric(text)) => format!("-{text}").parse::<i64>().ok(),
            _ => None,
        },
        Expr::Unary(UnaryOperator::Positive, inner) => integer_of(inner),
        _ => None,
    }
}

/// The column of `table` an expression names, when it is a plain column (not the rowid).
fn column_of(expr: &Expr, table: TableInternalId) -> Option<usize> {
    match expr {
        Expr::Column {
            table: t,
            column,
            is_rowid_alias: false,
            ..
        } if *t == table => Some(*column),
        _ => None,
    }
}

/// `column op value` as bounds; `None` when it gives none (or the bound passes `i64`).
fn bound(column: usize, op: Operator, value: i64) -> Option<ColumnBounds> {
    let (lo, hi) = match op {
        Operator::Equals => (value, value),
        Operator::Greater => (value.checked_add(1)?, i64::MAX),
        Operator::GreaterEquals => (value, i64::MAX),
        Operator::Less => (i64::MIN, value.checked_sub(1)?),
        Operator::LessEquals => (i64::MIN, value),
        _ => return None,
    };
    Some(ColumnBounds { column, lo, hi })
}

/// `value op column` read as `column op' value`.
fn flipped(op: Operator) -> Operator {
    match op {
        Operator::Greater => Operator::Less,
        Operator::GreaterEquals => Operator::LessEquals,
        Operator::Less => Operator::Greater,
        Operator::LessEquals => Operator::GreaterEquals,
        other => other,
    }
}

fn term_bounds(expr: &Expr, table: TableInternalId) -> Option<ColumnBounds> {
    match expr {
        Expr::Binary(lhs, op, rhs) => match (column_of(lhs, table), integer_of(rhs)) {
            (Some(column), Some(value)) => bound(column, *op, value),
            _ => bound(column_of(rhs, table)?, flipped(*op), integer_of(lhs)?),
        },
        Expr::Between {
            lhs,
            not: false,
            start,
            end,
        } => Some(ColumnBounds {
            column: column_of(lhs, table)?,
            lo: integer_of(start)?,
            hi: integer_of(end)?,
        }),
        _ => None,
    }
}

/// Each column's integer bounds that AND-ed `terms` put on `table`'s rows (one entry a column,
/// the terms' bounds intersected). The caller passes only terms every result row must satisfy.
pub fn integer_bounds<'a>(
    terms: impl IntoIterator<Item = &'a Expr>,
    table: TableInternalId,
) -> Vec<ColumnBounds> {
    let mut out: Vec<ColumnBounds> = Vec::new();
    for b in terms.into_iter().filter_map(|t| term_bounds(t, table)) {
        match out.iter_mut().find(|o| o.column == b.column) {
            Some(o) => {
                o.lo = o.lo.max(b.lo);
                o.hi = o.hi.min(b.hi);
            }
            None => out.push(b),
        }
    }
    out
}

/// Where a walk sends its cursor from the row it is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkTarget {
    /// The row is a candidate: read it.
    Here,
    /// No candidate is left: the scan ends.
    Past,
    /// Seek to this rowid (the next candidate run's first row).
    Seek(i64),
}

/// Where a walk is in moving its cursor, kept across an I/O wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkPhase {
    /// On a candidate row, or not yet moved.
    Idle,
    /// The cursor moved: check its row.
    Settle,
    /// Seeking to a rowid.
    Seeking(i64),
    /// The seek landed before its leaf's end: step to the next row.
    Advancing,
}

/// One scan cursor's walk over its candidates' rowid runs (ascending, disjoint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipWalk {
    pub cursor_id: usize,
    ranges: Vec<RowidRange>,
    at: usize,
    pub phase: WalkPhase,
}

impl SkipWalk {
    /// A walk over `candidates`; `None` for every row (nothing to skip).
    pub fn new(cursor_id: usize, candidates: Candidates) -> Option<Self> {
        let ranges = match candidates {
            Candidates::All => return None,
            Candidates::Ranges(ranges) => ranges,
            Candidates::Rowids(rowids) => {
                let mut ranges: Vec<RowidRange> = Vec::with_capacity(rowids.len());
                for rowid in rowids {
                    match ranges.last_mut() {
                        Some(last) if last.last.checked_add(1) == Some(rowid) => last.last = rowid,
                        _ => ranges.push(RowidRange {
                            first: rowid,
                            last: rowid,
                        }),
                    }
                }
                ranges
            }
        };
        Some(Self {
            cursor_id,
            ranges,
            at: 0,
            phase: WalkPhase::Settle,
        })
    }

    /// Where to go from the row at `rowid`; rowids only ascend, so runs passed are dropped.
    pub fn target(&mut self, rowid: i64) -> WalkTarget {
        while self.ranges.get(self.at).is_some_and(|r| r.last < rowid) {
            self.at += 1;
        }
        match self.ranges.get(self.at) {
            None => WalkTarget::Past,
            Some(r) if rowid < r.first => WalkTarget::Seek(r.first),
            Some(_) => WalkTarget::Here,
        }
    }
}

/// A program run's walks, one a scan cursor that skips; empty unless a summary is consulted.
#[derive(Debug, Default)]
pub struct SkipWalks {
    walks: Vec<SkipWalk>,
}

impl SkipWalks {
    pub fn is_empty(&self) -> bool {
        self.walks.is_empty()
    }

    /// Takes cursor `cursor_id`'s walk out (give it back with [`SkipWalks::put`]).
    pub fn take(&mut self, cursor_id: usize) -> Option<SkipWalk> {
        let at = self.walks.iter().position(|w| w.cursor_id == cursor_id)?;
        Some(self.walks.swap_remove(at))
    }

    pub fn put(&mut self, walk: SkipWalk) {
        self.walks.push(walk);
    }

    pub fn clear(&mut self) {
        self.walks.clear();
    }
}
