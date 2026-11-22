//! Amphora (G5 step 4, T-G5.4.3; executor.md decision 11): a SQL filter's table scan skipping the
//! rows its summary rules out.
//!
//! Two halves. At translation, [`SkipHint::of`] reads a SELECT's WHERE terms that bound a column
//! by an integer constant or a parameter (`a BETWEEN 10 AND 20`, `a >= ?`, `5 < a`, `a = :v`, and
//! lane skip-10's `a IN (?, ?)`), and the scan's cursor is given the hint; this costs a scan
//! nothing when no summary is kept. At run time the Rewind reads the parameters' bound values ([`SkipHint::bounds`]: a
//! prepared statement skips by whatever it is bound to on each run) and asks the database's summary for the hinted column (if one is kept and the reader
//! sees exactly what it holds), and a [`SkipWalk`] then moves the cursor past each run of rows the
//! candidates leave out with one seek, instead of reading them. The filter is still evaluated on
//! every row read, as in a plain scan: the candidates only leave out rows it can't match.
//!
//! Only rows whose value is an integer are left out: a NULL, real, text or blob value makes its
//! row a candidate (the summaries' rule, `skipping/mod.rs`), so SQLite's affinity and mixed-type
//! comparisons never reach the summaries. A term that isn't a plain integer comparison (a real or
//! text constant, a parameter bound to anything but an integer, a COLLATE, NOT IN, NOT BETWEEN)
//! gives no bound: loose, never wrong. An IN list bounds its column by its smallest and largest
//! value (lane skip-10, T-SKU.13): a row it matches equals one of them, so lies between the two;
//! one value that isn't an integer (as bound on this run) leaves the whole list unbounded, as a
//! row could match that value.

use super::{Candidates, RowidRange, SkipMethod};
use amphora_engine_parser::ast::{Expr, Literal, Operator, TableInternalId, UnaryOperator};
use std::num::NonZeroUsize;

/// One column's integer bounds, resolved when the scan starts: a row its filter can match has
/// `lo <= value <= hi` (or a value that isn't an integer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnBounds {
    pub column: usize,
    pub lo: i64,
    pub hi: i64,
}

/// What a term compares its column with: an integer constant, or a statement parameter whose
/// bound value is read when the scan starts (lane skip-usable, T-SKU.1: prepared statements skip).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundValue {
    Integer(i64),
    Parameter(NonZeroUsize),
}

/// How a term bounds its column: `column op value`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundOp {
    Equals,
    Greater,
    GreaterEquals,
    Less,
    LessEquals,
}

/// One AND-ed WHERE term's bound on a column, as translation reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundTerm {
    pub column: usize,
    pub op: BoundOp,
    pub value: BoundValue,
}

/// One AND-ed `column IN (value, ...)` term (lane skip-10, T-SKU.13): a row it matches has one of
/// the values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListTerm {
    pub column: usize,
    pub values: Vec<BoundValue>,
}

/// What a scan's cursor may skip by, set at translation: its filter's bounding terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipHint {
    pub cursor_id: usize,
    pub terms: Vec<BoundTerm>,
    /// Lane skip-10 (T-SKU.13): its IN lists.
    pub lists: Vec<ListTerm>,
}

impl SkipHint {
    /// The bounding terms AND-ed `terms` put on `table`'s rows, for cursor `cursor_id`'s scan, a
    /// parameter's value left to be read when the scan starts; `None` when no term bounds a
    /// column. The caller passes only terms every result row must satisfy.
    pub fn of<'a>(
        cursor_id: usize,
        terms: impl IntoIterator<Item = &'a Expr>,
        table: TableInternalId,
    ) -> Option<Self> {
        let mut hint = Self {
            cursor_id,
            terms: Vec::new(),
            lists: Vec::new(),
        };
        for term in terms {
            push_terms(term, table, &mut hint);
        }
        (!hint.terms.is_empty() || !hint.lists.is_empty()).then_some(hint)
    }

    /// `column`'s bounds from the hint's terms, intersected, each parameter's value read by
    /// `parameter` (`None`: the value isn't an integer, so its term gives no bound: loose, never
    /// wrong); `None` when no term bounds the column.
    pub fn bounds(
        &self,
        column: usize,
        parameter: impl Fn(NonZeroUsize) -> Option<i64>,
    ) -> Option<ColumnBounds> {
        let read = |value: BoundValue| match value {
            BoundValue::Integer(value) => Some(value),
            BoundValue::Parameter(index) => parameter(index),
        };
        let terms = self
            .terms
            .iter()
            .filter(|t| t.column == column)
            .filter_map(|term| read(term.value).and_then(|value| bound(column, term.op, value)));
        // Lane skip-10 (T-SKU.13): an IN list's hull, when every value it holds is an integer.
        let lists = self
            .lists
            .iter()
            .filter(|list| list.column == column)
            .filter_map(|list| {
                let mut values = list.values.iter().map(|value| read(*value));
                let first = values.next()??;
                values.try_fold(
                    ColumnBounds {
                        column,
                        lo: first,
                        hi: first,
                    },
                    |hull, value| {
                        value.map(|value| ColumnBounds {
                            column,
                            lo: hull.lo.min(value),
                            hi: hull.hi.max(value),
                        })
                    },
                )
            });
        terms.chain(lists).reduce(|a, b| ColumnBounds {
            column,
            lo: a.lo.max(b.lo),
            hi: a.hi.min(b.hi),
        })
    }
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

/// What a term's other side compares with: an integer constant or a parameter.
fn value_of(expr: &Expr) -> Option<BoundValue> {
    match expr {
        Expr::Variable(variable) => usize::try_from(variable.index.get())
            .ok()
            .and_then(NonZeroUsize::new)
            .map(BoundValue::Parameter),
        _ => integer_of(expr).map(BoundValue::Integer),
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

/// `column op value` as bounds; `None` when it gives none (the bound passes `i64`).
fn bound(column: usize, op: BoundOp, value: i64) -> Option<ColumnBounds> {
    let (lo, hi) = match op {
        BoundOp::Equals => (value, value),
        BoundOp::Greater => (value.checked_add(1)?, i64::MAX),
        BoundOp::GreaterEquals => (value, i64::MAX),
        BoundOp::Less => (i64::MIN, value.checked_sub(1)?),
        BoundOp::LessEquals => (i64::MIN, value),
    };
    Some(ColumnBounds { column, lo, hi })
}

/// The bounding operator `op` is, if any.
fn op_of(op: Operator) -> Option<BoundOp> {
    match op {
        Operator::Equals => Some(BoundOp::Equals),
        Operator::Greater => Some(BoundOp::Greater),
        Operator::GreaterEquals => Some(BoundOp::GreaterEquals),
        Operator::Less => Some(BoundOp::Less),
        Operator::LessEquals => Some(BoundOp::LessEquals),
        _ => None,
    }
}

/// `value op column` read as `column op' value`.
fn flipped(op: BoundOp) -> BoundOp {
    match op {
        BoundOp::Greater => BoundOp::Less,
        BoundOp::GreaterEquals => BoundOp::LessEquals,
        BoundOp::Less => BoundOp::Greater,
        BoundOp::LessEquals => BoundOp::GreaterEquals,
        BoundOp::Equals => BoundOp::Equals,
    }
}

fn push_terms(expr: &Expr, table: TableInternalId, hint: &mut SkipHint) {
    let out = &mut hint.terms;
    match expr {
        Expr::Binary(lhs, op, rhs) => {
            let Some(op) = op_of(*op) else {
                return;
            };
            let term = match (column_of(lhs, table), value_of(rhs)) {
                (Some(column), Some(value)) => Some(BoundTerm { column, op, value }),
                _ => column_of(rhs, table)
                    .zip(value_of(lhs))
                    .map(|(column, value)| BoundTerm {
                        column,
                        op: flipped(op),
                        value,
                    }),
            };
            out.extend(term);
        }
        Expr::Between {
            lhs,
            not: false,
            start,
            end,
        } => {
            let Some(column) = column_of(lhs, table) else {
                return;
            };
            for (op, side) in [(BoundOp::GreaterEquals, start), (BoundOp::LessEquals, end)] {
                out.extend(value_of(side).map(|value| BoundTerm { column, op, value }));
            }
        }
        // Lane skip-10 (T-SKU.13): `a IN (1, ?, :v)`, every value an integer constant or a
        // parameter; any other value leaves the list unbounded.
        Expr::InList {
            lhs,
            not: false,
            rhs,
        } => {
            let Some(column) = column_of(lhs, table) else {
                return;
            };
            let values: Option<Vec<BoundValue>> = rhs.iter().map(|value| value_of(value)).collect();
            if let Some(values) = values.filter(|values| !values.is_empty()) {
                hint.lists.push(ListTerm { column, values });
            }
        }
        _ => {}
    }
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

/// A walk whose cost goes back to a measured summary's chooser when it ends (lane skip-3,
/// T-SKU.2): the table, the method that named its candidates, and when it asked for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalkMeasure {
    pub root: i64,
    pub method: SkipMethod,
    pub started: std::time::Instant,
}

/// One scan cursor's walk over its candidates' rowid runs (ascending, disjoint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipWalk {
    pub cursor_id: usize,
    ranges: Vec<RowidRange>,
    at: usize,
    pub phase: WalkPhase,
    /// Taken (once) when the walk ends; `None` unless a measured summary answered.
    pub measure: Option<WalkMeasure>,
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
            measure: None,
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

    /// The last rowid of the candidate run cursor `cursor_id`'s walk stands in: a fused loop runs
    /// its body over the batch up to it (lane skip-5, T-SKU.4).
    pub fn run_end(&self, cursor_id: usize) -> Option<i64> {
        let walk = self.walks.iter().find(|w| w.cursor_id == cursor_id)?;
        walk.ranges.get(walk.at).map(|range| range.last)
    }

    pub fn put(&mut self, walk: SkipWalk) {
        self.walks.push(walk);
    }

    pub fn clear(&mut self) {
        self.walks.clear();
    }
}
