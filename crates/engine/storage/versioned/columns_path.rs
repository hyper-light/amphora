//! Amphora: a table's statement read by the path the planner chooses, the column copy or the rows
//! (G11 step 5, CS.5; the choice is `translate::optimizer::column_path`'s, by cost).
//!
//! [`read`] asks the copy for its view at the statement's snapshot ([`Columns::plan_view`]), lets
//! the planner choose, and reads by the path chosen. The copy's own scan keeps its guarantee
//! whatever the choice: it reads a generation only where one answers the snapshot exactly, and
//! reads rows otherwise, so a copy behind its snapshot is never read past it, even if it fell
//! behind between the view and the scan. A copy scan's time is charged back to the copy
//! ([`Columns::measured`]), which is how its cost per row is learned.
//!
//! Behind the planner's switch (`column_path::asked`) until G11 step 4's vectorized scan is on
//! main; it then replaces [`Columns::scan`] here and passes the rows its summaries keep as the
//! copy's reads.
#![cfg_attr(not(test), allow(dead_code))]

use super::columns::{Block, Columns, Rows, Rowset, ScanCounts};
use super::version::Ts;
use crate::translate::optimizer::column_path::{self, Choice, CopyView, Path, RowRates, Shape};
use crate::Result;

/// What a statement asks of the table.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Ask<'a> {
    /// The row with this key.
    Point(&'a [u8]),
    /// Every row, in key order (a whole-table aggregate's input).
    Whole,
}

/// A read's answer and where it came from.
#[derive(Debug)]
pub(crate) struct Read {
    pub(crate) rows: Rowset,
    pub(crate) choice: Choice,
    /// The copy's scan counts: all zero when the rows were chosen.
    pub(crate) copy: ScanCounts,
    /// Rows read from the row B-tree: by the row path, or by the copy's path for queued keys and
    /// fallbacks.
    pub(crate) row_reads: u64,
}

fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// Reads `ask` at snapshot `s`, `rows` being the row B-tree at `s`, by the path the planner chooses
/// with the row path's learned `rates`; `on` is the planner's switch, `clock` the caller's
/// nanoseconds, which time the copy's scan.
pub(crate) fn read<B: Block>(
    copy: &Columns<B>,
    rows: &dyn Rows,
    s: Ts,
    ask: Ask<'_>,
    rates: RowRates,
    on: bool,
    clock: &dyn Fn() -> u128,
) -> Result<Read> {
    let view = copy.plan_view(s);
    let table_rows = view.rows.saturating_add(view.queued) as f64;
    let shape = match ask {
        Ask::Point(_) => Shape::Point,
        Ask::Whole => Shape::Whole,
    };
    let cv = CopyView {
        exact: view.exact,
        reads: view.rows as f64,
        queued: view.queued as f64,
        nanos_per_row: view.nanos_per_row,
    };
    let choice = column_path::choose(on, shape, table_rows, rates, cv);
    match choice.path {
        Path::Rows => {
            let out = match ask {
                Ask::Point(key) => rows
                    .get(key)?
                    .map(|v| vec![(key.to_vec(), v)])
                    .unwrap_or_default(),
                Ask::Whole => rows.all()?,
            };
            let row_reads = match ask {
                Ask::Point(_) => 1,
                Ask::Whole => count(out.len()),
            };
            Ok(Read {
                rows: out,
                choice,
                copy: ScanCounts::default(),
                row_reads,
            })
        }
        Path::Copy => {
            let start = clock();
            let (all, counts) = copy.scan(s, rows)?;
            copy.measured(clock().saturating_sub(start), &counts);
            let row_reads = if counts.fallbacks > 0 {
                count(all.len())
            } else {
                counts.queued
            };
            let out = match ask {
                Ask::Point(key) => all.into_iter().filter(|(k, _)| k == key).collect(),
                Ask::Whole => all,
            };
            Ok(Read {
                rows: out,
                choice,
                copy: counts,
                row_reads,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::versioned::blink::{Comparator, Descender};
    use crate::storage::versioned::columns::{Feed, Packed, Plain};
    use crate::storage::versioned::disk::Simulated;
    use crate::storage::versioned::durable::Database;
    use crate::storage::versioned::version::Snapshot;
    use crate::sync::Arc;
    use std::cell::Cell;

    const N: u16 = 600;

    fn cmp() -> Comparator {
        Box::new(|a: &[u8], b: &[u8]| a.cmp(b))
    }

    struct At<'a> {
        db: &'a Database,
        snap: &'a Snapshot,
    }

    impl Rows for At<'_> {
        fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
            self.db.get(&mut Descender::new(), self.snap, 0, key)
        }
        fn all(&self) -> Result<Rowset> {
            self.db
                .scan(&mut Descender::new(), self.snap, 0, None, usize::MAX)
        }
    }

    fn open(seed: u64, bound: usize) -> (Database, Arc<Feed>) {
        let db = Database::open(Arc::new(Simulated::new(seed, 1).unwrap()), 4_000, 1, cmp).unwrap();
        let feed = Arc::new(Feed::new(0, bound, Box::new(|| 0)));
        db.keep_columns(feed.clone()).unwrap();
        (db, feed)
    }

    fn commit(db: &Database, keys: impl Iterator<Item = u16>, tag: u8, remove_every: u16) {
        let mut d = Descender::new();
        let mut t = db.begin().unwrap();
        for k in keys {
            if remove_every > 0 && k % remove_every == 0 {
                db.remove(&mut d, &mut t, 0, &k.to_be_bytes()).unwrap();
            } else {
                db.put(&mut d, &mut t, 0, &k.to_be_bytes(), &[tag; 8])
                    .unwrap();
            }
        }
        db.commit(t, &mut d, db.lane(), false).unwrap();
    }

    fn rebuild<B: Block>(db: &Database, copy: &Columns<B>) {
        let snap = db.snapshot().unwrap();
        let rows = At { db, snap: &snap }.all().unwrap();
        copy.rebuild(snap.start, &rows, 0).unwrap();
        db.close_snapshot(snap);
    }

    /// A clock that moves `step` nanoseconds a reading: a copy scan (two readings) is measured at
    /// `step` for the rows it read, so the test sets the copy's learned cost per row.
    fn clock(step: u128) -> impl Fn() -> u128 {
        let t = Cell::new(0u128);
        move || {
            let now = t.get().saturating_add(step);
            t.set(now);
            now
        }
    }

    // The row path's rates, as a database would have learned them.
    const RATES: RowRates = RowRates {
        row: 50.0,
        seek: 1_000.0,
    };

    /// A copy with rows in blocks, applied deltas, and queued keys not applied yet.
    fn kept<B: Block>(seed: u64) -> (Database, Columns<B>) {
        let (db, feed) = open(seed, usize::MAX);
        let copy = Columns::<B>::new(feed);
        commit(&db, 0..N, 1, 0);
        rebuild(&db, &copy);
        commit(&db, (0..N).step_by(7), 2, 3);
        copy.catch_up(0).unwrap();
        commit(&db, (0..N + 40).step_by(11), 3, 4);
        (db, copy)
    }

    fn answers_identical<B: Block>(seed: u64) {
        let (db, copy) = kept::<B>(seed);
        let snap = db.snapshot().unwrap();
        let at = At {
            db: &db,
            snap: &snap,
        };
        let truth = at.all().unwrap();
        // A seek dearer than every row's scan makes the copy win even a point read.
        let seek_dear = RowRates {
            row: RATES.row,
            seek: 1e12,
        };
        let tick = clock(1);
        for rates in [RATES, seek_dear] {
            for on in [false, true] {
                let whole = read(&copy, &at, snap.start, Ask::Whole, rates, on, &tick).unwrap();
                assert_eq!(whole.rows, truth, "{:?}", whole.choice);
                for k in [0u16, 1, 7, 11, 44, N - 1, N + 5, N + 39] {
                    let key = k.to_be_bytes();
                    let want: Rowset = truth.iter().filter(|(kk, _)| kk == &key).cloned().collect();
                    let p = read(&copy, &at, snap.start, Ask::Point(&key), rates, on, &tick).unwrap();
                    assert_eq!(p.rows, want, "key {k} {:?}", p.choice);
                }
            }
        }
        // Both paths were taken, for both shapes.
        let whole_copy = read(&copy, &at, snap.start, Ask::Whole, RATES, true, &tick).unwrap();
        assert_eq!(whole_copy.choice.path, Path::Copy);
        let key = 7u16.to_be_bytes();
        let p = read(&copy, &at, snap.start, Ask::Point(&key), seek_dear, true, &tick).unwrap();
        assert_eq!(p.choice.path, Path::Copy);
        let p = read(&copy, &at, snap.start, Ask::Point(&key), RATES, false, &tick).unwrap();
        assert_eq!(p.choice.path, Path::Rows);
        db.close_snapshot(snap);
    }

    /// Do: read every row and single rows, by each path, from a copy with deltas and queued keys.
    /// Expect: every answer equals the rows at the snapshot (CS.5, answers identical).
    #[test]
    fn cs5_answers_identical_whichever_path_plain() {
        answers_identical::<Plain>(11);
    }

    #[test]
    fn cs5_answers_identical_whichever_path_packed() {
        answers_identical::<Packed>(12);
    }

    /// Do: with the copy measured cheaper per row than a row's visit, read the whole table, then
    /// one row. Expect, by counts: the whole table from the copy (every row from its blocks and
    /// deltas, only the queued keys from the rows), the one row from the rows (one row read, the
    /// copy untouched) (CS.5).
    #[test]
    fn cs5_wide_aggregate_reads_the_copy_point_read_the_rows() {
        let (db, copy) = kept::<Plain>(13);
        let snap = db.snapshot().unwrap();
        let at = At {
            db: &db,
            snap: &snap,
        };
        let truth = at.all().unwrap();
        // The copy's first scan at its block size is tried, and measured: one reading step for
        // its rows, a tenth of a row's visit each.
        let step = (truth.len() as u128).saturating_mul(5);
        let tick = clock(step);
        let first = read(&copy, &at, snap.start, Ask::Whole, RATES, true, &tick).unwrap();
        assert_eq!(first.choice.path, Path::Copy);
        assert!(copy.plan_view(snap.start).nanos_per_row.is_some());
        let whole = read(&copy, &at, snap.start, Ask::Whole, RATES, true, &tick).unwrap();
        assert_eq!(whole.choice.path, Path::Copy, "{:?}", whole.choice);
        assert_eq!(whole.copy.fallbacks, 0);
        let queued = copy.plan_view(snap.start).queued;
        assert!(queued > 0);
        assert_eq!(whole.row_reads, whole.copy.queued);
        assert!(whole.copy.queued <= count(queued));
        assert_eq!(whole.copy.copy.saturating_add(whole.copy.queued), count(truth.len()));
        assert_eq!(whole.rows, truth);

        let key = 44u16.to_be_bytes();
        let point = read(&copy, &at, snap.start, Ask::Point(&key), RATES, true, &tick).unwrap();
        assert_eq!(point.choice.path, Path::Rows, "{:?}", point.choice);
        assert_eq!(point.copy, ScanCounts::default());
        assert_eq!(point.row_reads, 1);
        db.close_snapshot(snap);
    }

    /// Do: read at snapshots no generation answers exactly: after commits a full queue dropped, and
    /// before the copy's only generation was made. Expect: the rows are chosen, the copy is not
    /// read, and the answers equal the rows at each snapshot (CS.5, never read past its snapshot).
    #[test]
    fn cs5_stale_copy_never_read_past_its_snapshot() {
        // A queue that holds nothing: every commit after the rebuild is dropped, the copy behind.
        let (db, feed) = open(14, 0);
        let copy = Columns::<Plain>::new(feed);
        commit(&db, 0..N, 1, 0);
        let early = db.snapshot().unwrap();
        commit(&db, (0..N).step_by(3), 2, 5);
        rebuild(&db, &copy);
        commit(&db, (0..N).step_by(2), 3, 7);
        copy.catch_up(0).unwrap();
        let late = db.snapshot().unwrap();
        let tick = clock(1);
        for snap in [&early, &late] {
            let at = At { db: &db, snap };
            assert!(!copy.plan_view(snap.start).exact);
            let truth = at.all().unwrap();
            let r = read(&copy, &at, snap.start, Ask::Whole, RATES, true, &tick).unwrap();
            assert_eq!((r.choice.path, r.choice.copy_cost), (Path::Rows, None));
            assert_eq!(r.copy, ScanCounts::default());
            assert_eq!(r.rows, truth);
            assert_eq!(r.row_reads, count(truth.len()));
        }
        db.close_snapshot(late);
        db.close_snapshot(early);
    }
}
