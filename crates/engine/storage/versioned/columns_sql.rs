//! Amphora: SQL tables' column copies, and SQL's aggregates read from them (G11 step 5, CS.5;
//! `docs/design/columnar.md`, "The steps after").
//!
//! Which tables keep a copy is decided by their measured use, by the rule of renting before buying
//! (Karlin, Manasse, Rudolph and Sleator 1988, "Competitive snoopy caching": rent until the rent
//! paid equals the price, then buy; never more than twice the best choice made knowing the future).
//! A copy's price is one read of every row of its table (its first generation is built from a
//! snapshot scan); an analytic statement on the rows (an aggregate the executor's scan hook takes,
//! `translate::parallel`) reads every row of the table too. So the first such statement on a table
//! is read by the rows, and pays as much as a copy would cost; the second makes the copy. A table
//! no analytic statement reads never keeps one.
//!
//! A copy is fed from the commit stream ([`super::durable::Database::keep_columns`], under the
//! publish lock), applies what is queued as a statement reaches it ([`Columns::catch_up`]), and
//! is rebuilt when it says a rebuild pays ([`Columns::wants_rebuild`]): after the statement's own
//! choice, so a copy behind its snapshot is never read, it is made current for the next one.
//!
//! An aggregate goes by the planner's choice ([`columns_path::aggregate_choice`], by cost, with
//! the database's learned rates): the copy, read by the vectorized scan on as many cores as the
//! morsel executor finds pay ([`OnCores`], `Plan::run_on_cores`); or the statement's own serial
//! loop over the rows. A statement in a transaction that changed rows reads the rows: the copy
//! holds commits alone. One in a transaction that only reads reads the copy at the transaction's
//! own snapshot, which a generation of the copy answers exactly if it was made at or before it
//! and the copy isn't behind there (the current one, or the one it replaced, kept while an older
//! snapshot is open); if none does, the rows answer, counted as `unserved` with a trace event.
//! What the copy answered is kept as read, the whole table, for the transaction's validation, as
//! the rows' cursor keeps its walk (lane colcopy-10, AC-CB.4.7).
//!
//! A copy's queue, decoded blocks and blocks take their bytes from the storage's memory source as
//! they grow and give them back as they go, as its pages and row versions do (AC-CS.5.5): no share
//! is set aside. Each block's bytes are asked for before it is built, sized from its format, and
//! each change's before it is applied, so a copy never holds a byte it wasn't granted; a build
//! refused part way gives back what it took (lane colcopy-bound, AC-CB.1). A copy the source
//! refuses is let go, the refusal kept as its reason ([`Dropped`]), and the rows answer. How it is
//! bought again is a switch ([`Rebuy`], `AMPHORA_COLUMN_REBUY`), both ways measured by the A/B
//! harness in this file's tests (AC-CB.2, AC-CB.4): A pays the rent again and buys again; B waits
//! until the source reports room ([`Source::room`]) for the copy's expected size, re-estimated as
//! its table grows. Counters and trace events (target `amphora::columns`) say what each copy did
//! and why (AC-CB.3).
//!
//! Behind the planner's switch (`AMPHORA_COLUMN_PATH=on`, `column_path::asked`), read once when
//! the storage opens.

use super::blink::Descender;
use super::columns::scan::{
    Agg, Cache, Cell, Counts, Data, Decode, Filter, KeyRange, Op, Placed, Plan, Query, Records,
    Sources,
};
use super::columns::{as_u64, Block, Columns, Feed, Plain, Rows, Rowset};
use super::columns_path::{self, CopyScan, OnCopy, Scanned};
use super::cursor::key_rowid;
use super::durable::Database as VDatabase;
use super::memory::{self, Source};
use super::switch::{tree_of, Switch};
use super::version::{Snapshot, Ts, Txn};
use crate::error::LimboError;
use crate::numeric::Numeric;
use crate::sync::{Arc, Mutex};
use crate::translate::optimizer::column_path::{Path, RowRates};
use crate::types::Value;
use crate::vdbe::insn::MorselAggregateData;
use crate::vdbe::morsel::{
    Aggregate, CmpOp, MachineCosts, MemoryBudget, MorselError, ParallelMode, Predicate,
};
use crate::Result;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// What the copies have counted: the evidence that SQL reads them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CopyCounts {
    /// Copies made (a table's second analytic statement).
    pub kept: u64,
    /// Analytic statements answered by a copy.
    pub copy: u64,
    /// Analytic statements on a table with a copy that the planner sent to the rows.
    pub rows: u64,
    /// Granules the copy's scans read, and rows in them.
    pub granules: u64,
    pub granule_rows: u64,
    /// Rebuilds made.
    pub rebuilds: u64,
    /// Copies let go because the memory source refused their blocks.
    pub dropped: u64,
    /// Copies bought again after one was let go: each costs a whole-table read (AC-CB.3.1).
    pub rebuys: u64,
    /// Copies let go that waited for room ([`Rebuy::Room`]) and found it.
    pub resumed: u64,
    /// Rows the copies' builds read from their tables, and of those the rows rebuys read.
    pub build_rows: u64,
    pub rebuy_rows: u64,
    /// The keys' and rows' bytes those builds read, and of those the bytes rebuys read.
    pub build_bytes: u64,
    pub rebuy_bytes: u64,
    /// Analytic statements the rows answered with no copy kept (the rent paid, or the copy let go
    /// or waiting).
    pub absent: u64,
    /// Rows the row loop answered: each statement the rows answered counts its table's rows as
    /// its copy's last build read them (a table never built counts none).
    pub loop_rows: u64,
    /// Nanoseconds copies were absent, from being let go to being built again: absences ended
    /// ([`Copies::absent_nanos`] adds those still open).
    pub absent_nanos: u64,
    /// Analytic statements the scan hook handed the copies with the copies on (lane colcopy-9,
    /// AC-CB.4.6): those a copy could answer, and those [`Skip`] says none could.
    pub handed: u64,
    /// Of those, the statements no copy could answer, by [`Skip`]'s reason: they read the rows
    /// under either rebuy policy, so an A/B leaves them out.
    pub skipped_rows: u64,
    pub skipped_body: u64,
    pub skipped_join: u64,
    pub skipped_transaction: u64,
    /// Statements on a table with a copy, at a snapshot no generation of it answers exactly (none
    /// made yet, made after it, or behind there): the rows answered (lane colcopy-10, AC-CB.4.7).
    pub unserved: u64,
}

impl CopyCounts {
    /// The statements handed over that no copy could answer, every reason.
    pub fn skipped(&self) -> u64 {
        self.skipped_rows
            .saturating_add(self.skipped_body)
            .saturating_add(self.skipped_join)
            .saturating_add(self.skipped_transaction)
    }

    /// Whether the statements between `before` and `self` were all handed over and none was
    /// skipped: a copy could answer them (lane colcopy-9, AC-CB.4.6).
    pub fn reached_since(&self, before: &CopyCounts, statements: u64) -> bool {
        self.handed.saturating_sub(before.handed) == statements
            && self.skipped() == before.skipped()
    }
}

/// Why an analytic statement the scan hook handed over can't be answered by a copy (lane
/// colcopy-9, AC-CB.4.6): the rows answer it under either rebuy policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// It returns the table's rows, not aggregates.
    Rows,
    /// A filter or expression only the statement's loop body states.
    Body,
    /// A join.
    Join,
    /// In a transaction that changed rows: the copy holds commits alone. (A transaction that
    /// only reads reads the copy at its own snapshot, lane colcopy-10.)
    Transaction,
}

impl Skip {
    /// The reason in the records' and trace events' words.
    pub fn name(self) -> &'static str {
        match self {
            Skip::Rows => "rows",
            Skip::Body => "body",
            Skip::Join => "join",
            Skip::Transaction => "transaction",
        }
    }
}

/// What the copies asked their memory source for (AC-CB.3.1): takes asked, granted and refused,
/// their bytes, and the bytes held now and at most.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Meter {
    pub asked: u64,
    pub granted: u64,
    pub refused: u64,
    pub asked_bytes: u64,
    pub granted_bytes: u64,
    pub refused_bytes: u64,
    pub held: u64,
    pub peak: u64,
}

/// The storage's memory source as the copies use it, every take and give counted.
struct Metered {
    inner: Arc<dyn Source>,
    meter: Mutex<Meter>,
}

impl Source for Metered {
    fn take(&self, what: &'static str, bytes: u64) -> Result<()> {
        let taken = self.inner.take(what, bytes);
        let mut m = self.meter.lock();
        m.asked = m.asked.saturating_add(1);
        m.asked_bytes = m.asked_bytes.saturating_add(bytes);
        if taken.is_ok() {
            m.granted = m.granted.saturating_add(1);
            m.granted_bytes = m.granted_bytes.saturating_add(bytes);
            m.held = m.held.saturating_add(bytes);
            m.peak = m.peak.max(m.held);
        } else {
            m.refused = m.refused.saturating_add(1);
            m.refused_bytes = m.refused_bytes.saturating_add(bytes);
        }
        drop(m);
        if taken.is_err() {
            tracing::debug!(target: "amphora::columns", what, bytes, "column copy: memory refused");
        }
        taken
    }

    fn give(&self, bytes: u64) {
        self.inner.give(bytes);
        let mut m = self.meter.lock();
        m.held = m.held.saturating_sub(bytes);
    }

    fn room(&self) -> Result<u64> {
        self.inner.room()
    }
}

/// How a copy the memory source refused is bought again (lane colcopy-bound, AC-CB.2.1). Both go
/// through the same charge-before-build path; which serves best is the A/B harness's to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rebuy {
    /// A: let go and forgotten; its table's next analytic statement pays the rent again and the
    /// one after buys again, asking the source anew.
    Rent,
    /// B: let go and waiting until the source reports room ([`Source::room`]) for the copy's
    /// expected size, re-estimated as its table grows (AC-CB.2.2); bought at the table's first
    /// analytic statement that finds it.
    Room,
}

/// The switch between the two: `room` for [`Rebuy::Room`], anything else [`Rebuy::Rent`]. Read
/// once when the storage opens.
pub const REBUY_ENV: &str = "AMPHORA_COLUMN_REBUY";

impl Rebuy {
    pub fn asked() -> Self {
        match std::env::var(REBUY_ENV) {
            Ok(v) if v == "room" => Self::Room,
            _ => Self::Rent,
        }
    }
}

/// A copy let go under [`Rebuy::Room`]: the bytes it was expected to need, when it was let go (the
/// copies' ticks), and a feed that keeps nothing (its source grants none) and counts the bytes its
/// table grows by.
struct Waiting {
    bytes: u64,
    since: u64,
    feed: Arc<Feed>,
}

impl Waiting {
    /// The bytes it waits for now: what it needed when let go, and what its table grew by since.
    fn bytes(&self) -> u64 {
        self.bytes.saturating_add(self.feed.counts().put_bytes)
    }
}

/// A copy waiting for room (AC-CB.3.2): its table's tree, the bytes it waits for and how long it
/// has waited, in nanoseconds, so a copy that never comes back shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wait {
    pub tree: u64,
    pub bytes: u64,
    pub nanos: u64,
}

fn asked(e: &LimboError) -> u64 {
    match e {
        LimboError::MemoryBudget { asked, .. } => *asked,
        _ => 0,
    }
}

/// Why a copy was let go: the storage's memory source refused its blocks, `asked` more bytes with
/// `held` held of `limit` (the source's own [`LimboError::MemoryBudget`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    Memory {
        tree: u64,
        asked: u64,
        held: u64,
        limit: u64,
    },
}

pub use super::columns::BLOCKS;

/// One table's copy: its columns, its decoded blocks kept between scans, and the lock its rebuilds
/// take, so two statements don't rebuild it at once.
pub(crate) struct TableCopy {
    columns: Columns<Plain>,
    cache: Cache,
    rebuilding: Mutex<()>,
}

/// A storage's SQL tables' copies.
pub(crate) struct Copies {
    on: AtomicBool,
    /// How a copy let go is bought again.
    rebuy: Mutex<Rebuy>,
    /// The storage's memory source, counted ([`Metered`]): what each copy's queue, decoded blocks
    /// and blocks take.
    room: Arc<dyn Source>,
    meter: Arc<Metered>,
    /// The clock the copies' freshness lags and scans are timed by.
    base: Instant,
    tables: Mutex<HashMap<u64, Arc<TableCopy>>>,
    /// Trees an analytic statement has read by the rows, no copy kept yet: the rent paid.
    rented: Mutex<HashSet<u64>>,
    counts: Mutex<CopyCounts>,
    /// The last copy let go, and why.
    dropped: Mutex<Option<Dropped>>,
    /// Copies let go and waiting for room ([`Rebuy::Room`]).
    waiting: Mutex<HashMap<u64, Waiting>>,
    /// Trees whose copy was let go and isn't built again yet, and since when (ticks).
    absent: Mutex<HashMap<u64, u64>>,
    /// Each tree's rows as its copy's last build read them.
    rows_known: Mutex<HashMap<u64, u64>>,
}

fn nanos(base: Instant) -> u128 {
    base.elapsed().as_nanos()
}

fn tick(base: Instant) -> u64 {
    u64::try_from(nanos(base)).unwrap_or(u64::MAX)
}

impl Copies {
    pub(crate) fn new(on: bool, room: Arc<dyn Source>) -> Self {
        let meter = Arc::new(Metered {
            inner: room,
            meter: Mutex::new(Meter::default()),
        });
        let room: Arc<dyn Source> = meter.clone();
        Self {
            on: AtomicBool::new(on),
            rebuy: Mutex::new(Rebuy::asked()),
            room,
            meter,
            base: Instant::now(),
            tables: Mutex::new(HashMap::new()),
            rented: Mutex::new(HashSet::new()),
            counts: Mutex::new(CopyCounts::default()),
            dropped: Mutex::new(None),
            waiting: Mutex::new(HashMap::new()),
            absent: Mutex::new(HashMap::new()),
            rows_known: Mutex::new(HashMap::new()),
        }
    }

    /// How a copy let go is bought again.
    pub(crate) fn rebuy(&self) -> Rebuy {
        *self.rebuy.lock()
    }

    pub(crate) fn set_rebuy(&self, rebuy: Rebuy) {
        *self.rebuy.lock() = rebuy;
    }

    /// What the copies asked their memory source for.
    pub(crate) fn meter(&self) -> Meter {
        *self.meter.meter.lock()
    }

    /// The copies waiting for room, with the bytes each waits for and how long it has.
    pub(crate) fn waiting(&self) -> Vec<Wait> {
        let now = tick(self.base);
        self.waiting
            .lock()
            .iter()
            .map(|(tree, w)| Wait {
                tree: *tree,
                bytes: w.bytes(),
                nanos: now.saturating_sub(w.since),
            })
            .collect()
    }

    /// Nanoseconds copies have been absent, from being let go to being built again, those still
    /// absent counted to now (the A/B's records).
    #[cfg(test)]
    pub(crate) fn absent_nanos(&self) -> u64 {
        let now = tick(self.base);
        let open = self.absent.lock().values().fold(0u64, |n, since| {
            n.saturating_add(now.saturating_sub(*since))
        });
        self.counts.lock().absent_nanos.saturating_add(open)
    }

    /// The copies' generations' resident bytes and the bytes granted them: the first never more
    /// than the second (AC-CB.1.1).
    pub(crate) fn held(&self) -> (u64, u64) {
        self.tables
            .lock()
            .values()
            .fold((0u64, 0u64), |(resident, granted), c| {
                (
                    resident.saturating_add(as_u64(c.columns.blocks_resident())),
                    granted.saturating_add(as_u64(c.columns.granted())),
                )
            })
    }

    /// A statement on `tree` the rows answered: `absent` when no copy was kept.
    fn by_rows(&self, tree: u64, absent: bool) {
        let rows = self.rows_known.lock().get(&tree).copied().unwrap_or(0);
        let mut c = self.counts.lock();
        c.loop_rows = c.loop_rows.saturating_add(rows);
        if absent {
            c.absent = c.absent.saturating_add(1);
        }
    }

    /// An analytic statement the scan hook handed over, and why no copy can answer it if none
    /// can (lane colcopy-9, AC-CB.4.6).
    fn handed(&self, skip: Option<Skip>) {
        {
            let mut c = self.counts.lock();
            c.handed = c.handed.saturating_add(1);
            let n = match skip {
                None => return,
                Some(Skip::Rows) => &mut c.skipped_rows,
                Some(Skip::Body) => &mut c.skipped_body,
                Some(Skip::Join) => &mut c.skipped_join,
                Some(Skip::Transaction) => &mut c.skipped_transaction,
            };
            *n = n.saturating_add(1);
        }
        if let Some(skip) = skip {
            tracing::debug!(target: "amphora::columns", reason = skip.name(), "column copy skipped: no copy can answer the statement");
        }
    }

    /// A build of `tree`'s copy read `rows` rows of its table: a rebuy's when its copy was let go.
    fn read_for_build(&self, tree: u64, rows: u64, bytes: u64) {
        let rebuy = self.absent.lock().contains_key(&tree);
        let mut c = self.counts.lock();
        c.build_rows = c.build_rows.saturating_add(rows);
        c.build_bytes = c.build_bytes.saturating_add(bytes);
        if rebuy {
            c.rebuy_rows = c.rebuy_rows.saturating_add(rows);
            c.rebuy_bytes = c.rebuy_bytes.saturating_add(bytes);
        }
    }

    /// `tree`'s copy built from `rows` rows: present again if it was let go.
    fn built(&self, tree: u64, rows: u64) {
        self.rows_known.lock().insert(tree, rows);
        let since = self.absent.lock().remove(&tree);
        let mut c = self.counts.lock();
        c.rebuilds = c.rebuilds.saturating_add(1);
        if let Some(since) = since {
            c.absent_nanos = c
                .absent_nanos
                .saturating_add(tick(self.base).saturating_sub(since));
            tracing::debug!(target: "amphora::columns", tree, rows, "column copy built again");
        }
    }

    fn has(&self, tree: u64) -> bool {
        self.tables.lock().contains_key(&tree)
    }

    /// Whether the planner's switch is on: the translator then hands analytic statements to the
    /// scan hook.
    pub(crate) fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    pub(crate) fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }

    pub(crate) fn counts(&self) -> CopyCounts {
        *self.counts.lock()
    }

    /// The last copy let go and why (None: none was).
    pub(crate) fn last_dropped(&self) -> Option<Dropped> {
        *self.dropped.lock()
    }

    /// Lets `tree`'s copy go when `refused` is the memory source's refusal, kept as its reason and
    /// counted; any other error is the caller's. Then by [`Rebuy`]: A, the table's next analytic
    /// statement pays the rent again and the one after buys again; B, it waits for room for
    /// `expected` bytes and what its table grows by.
    fn let_go(&self, db: &VDatabase, tree: u64, refused: LimboError, expected: u64) -> Result<()> {
        let LimboError::MemoryBudget {
            asked, held, limit, ..
        } = refused
        else {
            return Err(refused);
        };
        let now = tick(self.base);
        let rebuy = self.rebuy();
        self.tables.lock().remove(&tree);
        db.drop_columns(tree);
        self.absent.lock().entry(tree).or_insert(now);
        match rebuy {
            Rebuy::Rent => {
                self.rented.lock().remove(&tree);
            }
            Rebuy::Room => {
                let base = self.base;
                let feed = Arc::new(Feed::new(
                    tree,
                    memory::capped(0),
                    Box::new(move || tick(base)),
                ));
                db.keep_columns(feed.clone())?;
                self.waiting.lock().insert(
                    tree,
                    Waiting {
                        bytes: expected,
                        since: now,
                        feed,
                    },
                );
            }
        }
        tracing::debug!(target: "amphora::columns", tree, asked, held, limit, expected, ?rebuy, "column copy let go: its memory source refused");
        *self.dropped.lock() = Some(Dropped::Memory {
            tree,
            asked,
            held,
            limit,
        });
        self.counts.lock().dropped += 1;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn kept(&self, tree: u64) -> Option<Arc<TableCopy>> {
        self.tables.lock().get(&tree).cloned()
    }

    /// The copy of `tree` an analytic statement reads: kept, made now (the rent paid), or None
    /// (this statement pays it).
    fn for_statement(&self, db: &VDatabase, tree: u64) -> Result<Option<Arc<TableCopy>>> {
        let mut tables = self.tables.lock();
        if let Some(copy) = tables.get(&tree) {
            return Ok(Some(copy.clone()));
        }
        let waited = self.waiting.lock().get(&tree).map(Waiting::bytes);
        match waited {
            // B: bought again only once the source reports room for it, its own signal asked at
            // the table's statements; no whole-table read is spent before.
            Some(bytes) => {
                let room = self.room.room()?;
                if room < bytes {
                    return Ok(None);
                }
                self.waiting.lock().remove(&tree);
                db.drop_columns(tree);
                self.counts.lock().resumed += 1;
                tracing::debug!(target: "amphora::columns", tree, bytes, room, "column copy: room for it again");
            }
            None => {
                if self.rented.lock().insert(tree) {
                    return Ok(None);
                }
            }
        }
        let base = self.base;
        let feed = Arc::new(Feed::new(
            tree,
            self.room.clone(),
            Box::new(move || tick(base)),
        ));
        db.keep_columns(feed.clone())?;
        let copy = Arc::new(TableCopy {
            columns: Columns::new(feed, self.room.clone()),
            cache: Cache::new(self.room.clone()),
            rebuilding: Mutex::new(()),
        });
        tables.insert(tree, copy.clone());
        self.rented.lock().remove(&tree);
        let rebuy = self.absent.lock().contains_key(&tree);
        {
            let mut c = self.counts.lock();
            c.kept += 1;
            if rebuy {
                c.rebuys += 1;
            }
        }
        tracing::debug!(target: "amphora::columns", tree, rebuy, "column copy bought");
        Ok(Some(copy))
    }

    /// Forgets `tree`'s copy, its rent and its wait: its table was dropped.
    pub(crate) fn forget(&self, db: &VDatabase, tree: u64) {
        self.tables.lock().remove(&tree);
        self.rented.lock().remove(&tree);
        self.waiting.lock().remove(&tree);
        self.absent.lock().remove(&tree);
        self.rows_known.lock().remove(&tree);
        db.drop_columns(tree);
    }
}

/// The rows of `tree` as `snap` sees them.
struct At<'a> {
    db: &'a VDatabase,
    snap: &'a Snapshot,
    tree: u64,
}

impl Rows for At<'_> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.db
            .get(&mut Descender::new(), self.snap, self.tree, key)
    }
    fn all(&self) -> Result<Rowset> {
        self.db.scan(
            &mut Descender::new(),
            self.snap,
            self.tree,
            None,
            usize::MAX,
        )
    }
}

/// A SQL table's rows: record payloads, an INTEGER PRIMARY KEY's column (stored as NULL) read
/// from the row's key, as the executor's scan hook reads it.
struct SqlRows {
    rowid_alias: Option<usize>,
}

impl Decode for SqlRows {
    fn decode(&self, key: &[u8], row: &[u8], out: &mut Vec<Cell>) -> Result<()> {
        Records.decode(key, row, out)?;
        if let Some(at) = self.rowid_alias {
            if let Some(cell) = out.get_mut(at) {
                *cell = Cell::Int(key_rowid(key)?);
            }
        }
        Ok(())
    }
}

/// The vectorized scan run on as many cores as the morsel executor finds pay (G11 step 5's third
/// part): `Plan::run_on_cores` with the connection's parallel mode and machine costs. A value it
/// doesn't take, or groups past the statement's memory, is a decline: the serial loop answers.
struct OnCores<'a> {
    cache: &'a Cache,
    mode: ParallelMode,
    costs: &'a Mutex<Option<MachineCosts>>,
    budget: &'a MemoryBudget,
    declined: std::cell::Cell<bool>,
}

fn morsel(e: MorselError) -> LimboError {
    LimboError::InternalError(format!("column copy's aggregate: {e}"))
}

impl<B: Block> CopyScan<B> for OnCores<'_> {
    fn scan(
        &self,
        copy: &Columns<B>,
        s: Ts,
        rows: &dyn Rows,
        dec: &dyn Decode,
        q: &Query,
    ) -> Result<Scanned> {
        let (granules, sources) = copy.granules(s, rows, dec, self.cache, &q.keys)?;
        // A SUM over text or blobs: SQLite sums text by its numeric prefix and makes the sum a
        // real, which the copy's scan doesn't; it declines, and the serial loop answers.
        if let Some(kind) = sums_text(q, &granules) {
            self.declined.set(true);
            return Err(morsel(MorselError::Unsupported { kind }));
        }
        let built = granules
            .iter()
            .fold(0usize, |n, p| n.saturating_add(p.granule.rows));
        let plan = Plan { granules, query: q };
        // The costs are taken for the run and put back after, so a statement learns from the last.
        let taken = self.costs.lock().take();
        let mut costs = match taken {
            Some(costs) => costs,
            None => MachineCosts::measure().map_err(morsel)?,
        };
        let ran = plan.run_on_cores(self.mode, &mut costs, self.budget);
        *self.costs.lock() = Some(costs);
        match ran {
            Ok((answer, counts, _)) => Ok(Scanned {
                answer,
                counts,
                sources,
                built,
            }),
            Err(e @ (MorselError::Unsupported { .. } | MorselError::MemoryBudget { .. })) => {
                self.declined.set(true);
                Err(morsel(e))
            }
            Err(e) => Err(morsel(e)),
        }
    }
}

/// The kind of the first value that isn't a number in a column `q` sums (None: every one is a
/// number or null).
fn sums_text(q: &Query, granules: &[Placed]) -> Option<&'static str> {
    q.aggs
        .iter()
        .filter_map(|a| match a {
            Agg::Sum(c) => Some(*c),
            _ => None,
        })
        .find_map(|c| {
            granules
                .iter()
                .find_map(|p| match &p.granule.columns.get(c)?.data {
                    Data::Int(_) | Data::Real(_) => None,
                    Data::Text { .. } => Some("TEXT"),
                    Data::Mixed(cells) => cells.iter().find_map(|v| match v {
                        Cell::Text(_) => Some("TEXT"),
                        Cell::Blob(_) => Some("BLOB"),
                        _ => None,
                    }),
                })
        })
}

fn cell(v: &Value) -> Result<Cell> {
    Ok(match v {
        Value::Null => Cell::Null,
        Value::Numeric(Numeric::Integer(i)) => Cell::Int(*i),
        Value::Numeric(Numeric::Float(f)) => Cell::Real(f64::from(*f)),
        Value::Text(t) => Cell::Text(t.as_str().as_bytes().to_vec()),
        Value::Blob(b) => Cell::Blob(b.to_vec()),
    })
}

fn value(c: Cell) -> Result<Value> {
    Ok(match c {
        Cell::Null => Value::Null,
        Cell::Int(i) => Value::from_i64(i),
        Cell::Real(f) => Value::from_f64(f),
        Cell::Text(t) => Value::build_text(String::from_utf8(t).map_err(|_| {
            LimboError::InternalError("column copy: a text value that isn't UTF-8".into())
        })?),
        Cell::Blob(b) => Value::from_blob(b),
    })
}

/// The scan hook's statement as the copy's query: the same filter, groups and aggregates, over
/// the same columns (the table's stored columns, the rowid in its alias's place).
fn query(data: &MorselAggregateData, budget: usize) -> Result<Query> {
    let mut filters = Vec::with_capacity(data.filter.len());
    for p in &data.filter {
        filters.push(match p {
            Predicate::Compare { column, op, value } => Filter::Cmp {
                col: *column,
                op: match op {
                    CmpOp::Lt => Op::Lt,
                    CmpOp::Le => Op::Le,
                    CmpOp::Eq => Op::Eq,
                    CmpOp::Ne => Op::Ne,
                    CmpOp::Gt => Op::Gt,
                    CmpOp::Ge => Op::Ge,
                },
                lit: cell(value)?,
            },
            Predicate::IsNull { column } => Filter::IsNull(*column),
            Predicate::NotNull { column } => Filter::NotNull(*column),
        });
    }
    let mut aggs = Vec::with_capacity(data.aggregates.len());
    for a in &data.aggregates {
        aggs.push(match a {
            Aggregate::CountStar => Agg::Count,
            Aggregate::Count(c) => Agg::CountOf(*c),
            Aggregate::Sum(c) => Agg::Sum(*c),
            Aggregate::Min(c) => Agg::Min(*c),
            Aggregate::Max(c) => Agg::Max(*c),
            // Only the loop body's copy takes them, which `aggregate` hands to the rows.
            Aggregate::Avg(_) | Aggregate::Total(_) => {
                return Err(LimboError::InternalError(
                    "the column copy has no AVG or TOTAL".into(),
                ))
            }
        });
    }
    Ok(Query {
        filters,
        group: data.group_by.clone(),
        aggs,
        budget,
        keys: KeyRange::default(),
    })
}

/// Answers the scan hook's statement `data` from its table's column copy when the planner chooses
/// the copy: its rows (the GROUP BY values, then the aggregates, groups in key order). None: the
/// statement's serial loop reads the rows.
pub(crate) fn aggregate(
    switch: &Switch,
    data: &MorselAggregateData,
    connection: &crate::Connection,
    statement_memory: usize,
) -> Result<Option<Vec<Vec<Value>>>> {
    // A column with a default a short row reads: the copy decodes the records as written, so the
    // serial loop reads the rows (lane qs6-17; the copy's own padding is open, executor.md 8).
    if !data.defaults.is_empty() {
        return Ok(None);
    }
    let storage = switch.storage();
    let copies = &storage.copies;
    if !copies.on() {
        return Ok(None);
    }
    // A statement no copy can answer reads the rows, counted with its reason (lane colcopy-9,
    // AC-CB.4.6) so an A/B can leave it out: it reads the rows under either rebuy policy.
    let skip = match &data.body {
        Some(body) if !body.simple && body.rows => Some(Skip::Rows),
        Some(body) if !body.simple => Some(Skip::Body),
        _ if !data.joins.is_empty() => Some(Skip::Join),
        _ if switch.wrote() => Some(Skip::Transaction),
        _ => None,
    };
    copies.handed(skip);
    if skip.is_some() {
        return Ok(None);
    }
    let db = storage.db();
    let tree = tree_of(data.root_page)?;
    let Some(copy) = copies.for_statement(db, tree)? else {
        copies.by_rows(tree, true);
        return Ok(None);
    };
    let q = query(data, statement_memory)?;
    // At the transaction's own snapshot (lane colcopy-10, AC-CB.4.7); in autocommit with none
    // open, at one of the statement's own.
    let answered = match switch.reading(connection.get_auto_commit())? {
        Some(shared) => {
            let snap = shared
                .lock()
                .as_ref()
                .map(Txn::snapshot)
                .ok_or_else(ended)?;
            let answered = read(copies, db, &copy, &snap, tree, data, &q, connection);
            // What the copy answered is kept as read, the whole table, for the transaction's
            // validation: a superset of the rows the aggregate depends on refuses more, never
            // less, as the rows' cursor walking every row would keep.
            if matches!(answered, Ok(Some(_))) {
                let mut guard = shared.lock();
                let txn = guard.as_mut().ok_or_else(ended)?;
                db.v.keep_read(txn, tree, None, None)?;
            }
            answered
        }
        None => {
            let snap = db.snapshot()?;
            let answered = read(copies, db, &copy, &snap, tree, data, &q, connection);
            db.close_snapshot(snap);
            answered
        }
    };
    if matches!(answered, Ok(None)) {
        copies.by_rows(tree, !copies.has(tree));
    }
    answered
}

fn ended() -> LimboError {
    LimboError::TxError("versioned storage: the transaction has ended".into())
}

#[allow(clippy::too_many_arguments)]
fn read(
    copies: &Copies,
    db: &VDatabase,
    copy: &TableCopy,
    snap: &Snapshot,
    tree: u64,
    data: &MorselAggregateData,
    q: &Query,
    connection: &crate::Connection,
) -> Result<Option<Vec<Vec<Value>>>> {
    let base = copies.base;
    let clock = move || nanos(base);
    let rows = At { db, snap, tree };
    // The queued commits applied, each change's bytes granted first (AC-CB.1.1); refused, the copy
    // is let go, expecting what it held and the bytes asked, and the rows answer.
    if let Err(refused) = copy.columns.catch_up(tick(base)) {
        let expected = as_u64(copy.columns.granted()).saturating_add(asked(&refused));
        copies.let_go(db, tree, refused, expected)?;
        return Ok(None);
    }
    // Unknown rates price nothing: the rows.
    let rates = RowRates::learned(&connection.db.learned_costs);
    let chosen =
        rates.map(|r| columns_path::aggregate_choice(&copy.columns, snap.start, q, r, true));
    let answer = match chosen {
        Some((choice, view)) if choice.path == Path::Copy => {
            let budget = MemoryBudget::new(q.budget);
            let scan = OnCores {
                cache: &copy.cache,
                mode: connection.parallel_mode(),
                costs: &connection.morsel_costs,
                budget: &budget,
                declined: std::cell::Cell::new(false),
            };
            let dec = SqlRows {
                rowid_alias: data.rowid_alias,
            };
            match columns_path::aggregate_on_copy(
                &copy.columns,
                &rows,
                snap.start,
                q,
                &dec,
                &scan,
                view,
                &clock,
            ) {
                Ok(OnCopy {
                    answer,
                    counts,
                    sources,
                    ..
                }) => {
                    count_copy(copies, &counts, &sources);
                    let mut out = Vec::with_capacity(answer.len());
                    for row in answer {
                        out.push(row.into_iter().map(value).collect::<Result<Vec<_>>>()?);
                    }
                    Some(out)
                }
                Err(_) if scan.declined.get() => None,
                Err(e) => return Err(e),
            }
        }
        Some((_, view)) if !view.exact => {
            // No generation answers this snapshot exactly: the copy refuses it, and the rows
            // answer (lane colcopy-10, AC-CB.4.7).
            {
                let mut c = copies.counts.lock();
                c.rows += 1;
                c.unserved = c.unserved.saturating_add(1);
            }
            tracing::debug!(target: "amphora::columns", tree, snapshot = snap.start, "column copy refused: no generation answers the statement's snapshot");
            None
        }
        _ => {
            copies.counts.lock().rows += 1;
            None
        }
    };
    // The copy's own upkeep, after the statement's choice: a copy behind its snapshot was not
    // read, and is made current for the next statement.
    copy.columns.release(db.v.clock.low_watermark());
    if copy.columns.wants_rebuild() {
        let _one = copy.rebuilding.lock();
        if copy.columns.wants_rebuild() {
            // Made at a snapshot of every commit now, not the statement's: a generation made at
            // an older transaction's snapshot would lack the commits between it and the one it
            // replaces (lane colcopy-10).
            let now = db.snapshot()?;
            let built = rebuild(copies, db, copy, &now, tree, &clock);
            db.close_snapshot(now);
            built?;
        }
    }
    Ok(answer)
}

/// Rebuilds `tree`'s copy from its rows at `snap`, under the copy's rebuild lock; let go if its
/// memory source refuses a block.
fn rebuild(
    copies: &Copies,
    db: &VDatabase,
    copy: &TableCopy,
    snap: &Snapshot,
    tree: u64,
    clock: &dyn Fn() -> u128,
) -> Result<()> {
    let base = copies.base;
    let rows = At { db, snap, tree };
    let start = clock();
    let all = rows.all()?;
    let n = as_u64(all.len());
    let read = all.iter().fold(0u64, |b, (k, v)| {
        b.saturating_add(as_u64(k.len()))
            .saturating_add(as_u64(v.len()))
    });
    copies.read_for_build(tree, n, read);
    // Each block's bytes granted before it is built (AC-CB.1.1); refused, the copy is let go,
    // expecting the generation these rows make.
    if let Err(refused) = copy.columns.rebuild(snap.start, &all, tick(base)) {
        return copies.let_go(db, tree, refused, as_u64(Plain::bytes(&all)));
    }
    copy.columns.rebuilt(clock().saturating_sub(start));
    copies.built(tree, n);
    Ok(())
}

fn count_copy(copies: &Copies, counts: &Counts, _sources: &Sources) {
    let mut c = copies.counts.lock();
    c.copy += 1;
    c.granules = c.granules.saturating_add(counts.granules);
    c.granule_rows = c.granule_rows.saturating_add(counts.rows);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{MemoryIO, IO};
    use crate::{ConnectOpts, Database, SqliteDialect};

    fn connect() -> (Arc<Database>, Arc<crate::Connection>) {
        connect_in(None)
    }

    /// A connection on a versioned storage whose memory source is `room` (None: the machine's).
    fn connect_in(room: Option<Arc<dyn Source>>) -> (Arc<Database>, Arc<crate::Connection>) {
        let io: Arc<dyn IO> = Arc::new(MemoryIO::new());
        let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
        let opts = ConnectOpts::new().with_versioned_storage();
        let opts = match room {
            Some(room) => opts.with_memory(room),
            None => opts,
        };
        let conn = db.connect(opts).unwrap();
        (db, conn)
    }

    /// A memory source that refuses every take for one use, `what`, and grants the rest.
    struct Refuses {
        what: &'static str,
    }

    impl Source for Refuses {
        fn take(&self, what: &'static str, bytes: u64) -> Result<()> {
            if what == self.what {
                return Err(LimboError::MemoryBudget {
                    what,
                    asked: bytes,
                    held: 0,
                    limit: 0,
                });
            }
            Ok(())
        }

        fn give(&self, _bytes: u64) {}

        fn room(&self) -> Result<u64> {
            Ok(u64::MAX)
        }
    }

    fn refusing(what: &'static str) -> Option<Arc<dyn Source>> {
        let r: Arc<Refuses> = Arc::new(Refuses { what });
        Some(r)
    }

    fn copies(conn: &Arc<crate::Connection>) -> &Copies {
        &conn.versioned.as_ref().unwrap().storage().copies
    }

    fn rows(conn: &Arc<crate::Connection>, sql: &str) -> Vec<Vec<Value>> {
        let mut stmt = conn.prepare(sql).unwrap();
        let mut out = Vec::new();
        stmt.run_with_row_callback(|row| {
            out.push(row.get_values().cloned().collect());
            Ok(())
        })
        .unwrap();
        out
    }

    const SCHEMA: &str = "CREATE TABLE t (id INTEGER PRIMARY KEY, g INTEGER, v INTEGER, s TEXT)";

    fn fill(conn: &Arc<crate::Connection>, from: i64, to: i64) {
        conn.execute(format!(
            "INSERT INTO t SELECT value, value % 7, (value * 37) % 1000, \
             CASE WHEN value % 5 = 0 THEN NULL ELSE 'k' || (value % 11) END \
             FROM generate_series({from}, {to})"
        ))
        .unwrap();
    }

    use crate::storage::versioned::columns_ab::QUERIES;

    /// Do: the same table on two versioned storages, one with the column copies' switch on; the
    /// analytic statements (filters, aggregates, GROUP BY) on both, with inserts, updates and
    /// deletes between rounds. Expect: the same rows each round, and the copies' counts show SQL
    /// read a copy (made at a table's second analytic statement) on the switched one only.
    #[test]
    fn cs5_sql_aggregates_answer_alike_on_copy_and_rows() {
        let (_a, on) = connect();
        let (_b, off) = connect();
        copies(&on).set_on(true);
        copies(&off).set_on(false);
        for c in [&on, &off] {
            c.execute(SCHEMA).unwrap();
            fill(c, 1, 3000);
        }
        let writes = [
            "UPDATE t SET v = v + 1 WHERE id % 13 = 0",
            "DELETE FROM t WHERE id % 17 = 0",
            "INSERT INTO t VALUES (5000, 2, 999, 'new'), (-4, NULL, NULL, NULL)",
            "UPDATE t SET s = 'zz', g = 9 WHERE id BETWEEN 100 AND 140",
        ];
        for round in 0..=writes.len() {
            for q in QUERIES {
                let want = rows(&off, q);
                assert!(!want.is_empty(), "{q}");
                assert_eq!(rows(&on, q), want, "round {round}: {q}");
            }
            if let Some(w) = writes.get(round) {
                on.execute(w).unwrap();
                off.execute(w).unwrap();
            }
        }
        let seen = copies(&on).counts();
        assert_eq!(seen.kept, 1, "{seen:?}");
        assert!(seen.copy > 0, "no statement read the copy: {seen:?}");
        assert!(seen.granule_rows > 0, "{seen:?}");
        assert_eq!(copies(&off).counts(), CopyCounts::default());
    }

    /// Do: a table no analytic statement reads, and one read once. Expect: neither keeps a copy
    /// (the first statement pays the rent; only a second buys).
    #[test]
    fn cs5_a_copy_is_kept_only_for_tables_analytic_statements_read() {
        let (_a, conn) = connect();
        copies(&conn).set_on(true);
        conn.execute(SCHEMA).unwrap();
        conn.execute("CREATE TABLE u (a INTEGER, b INTEGER)")
            .unwrap();
        fill(&conn, 1, 200);
        conn.execute("INSERT INTO u VALUES (1, 2), (3, 4)").unwrap();
        rows(&conn, "SELECT a, b FROM u WHERE a = 1");
        rows(&conn, "SELECT sum(g) FROM t");
        assert_eq!(copies(&conn).counts().kept, 0);
        rows(&conn, "SELECT sum(v) FROM t");
        assert_eq!(copies(&conn).counts().kept, 1);
    }

    /// T-CB.4.4 (lane colcopy-9, AC-CB.4.6): an analytic statement in autocommit, then the same
    /// statement in a transaction that wrote and in one that only reads, then each of the A/B's
    /// queries in autocommit. Expect: the first is handed over and reaches the copies; the one in
    /// the transaction that wrote is counted skipped for it, and the one in the transaction that
    /// only reads reaches the copies (lane colcopy-10, AC-CB.4.7), the rows answering alike; every
    /// statement is handed over once, and each query's reach is printed (the A/B's records name
    /// those that don't).
    #[test]
    fn cb4_statements_no_copy_can_answer_are_counted_with_their_reason() {
        let (_a, on) = connect();
        let (_b, off) = connect();
        copies(&on).set_on(true);
        for c in [&on, &off] {
            c.execute(SCHEMA).unwrap();
            fill(c, 1, 500);
        }
        let q = "SELECT count(*), sum(v) FROM t WHERE v < 700";
        let before = copies(&on).counts();
        assert_eq!(rows(&on, q), rows(&off, q));
        let after = copies(&on).counts();
        assert!(after.reached_since(&before, 1), "{after:?}");
        for c in [&on, &off] {
            c.execute("BEGIN").unwrap();
            c.execute("INSERT INTO t VALUES (100000, 1, 1, 'w')")
                .unwrap();
        }
        assert_eq!(rows(&on, q), rows(&off, q));
        let wrote = copies(&on).counts();
        assert_eq!(wrote.handed, after.handed + 1, "{wrote:?}");
        assert_eq!(
            wrote.skipped_transaction,
            after.skipped_transaction + 1,
            "{wrote:?}"
        );
        for c in [&on, &off] {
            c.execute("COMMIT").unwrap();
            c.execute("BEGIN").unwrap();
        }
        assert_eq!(rows(&on, q), rows(&off, q));
        for c in [&on, &off] {
            c.execute("COMMIT").unwrap();
        }
        let read = copies(&on).counts();
        assert!(read.reached_since(&wrote, 1), "{read:?}");
        for (i, q) in QUERIES.iter().enumerate() {
            let before = copies(&on).counts();
            assert_eq!(rows(&on, q), rows(&off, q), "{q}");
            let after = copies(&on).counts();
            assert!(after.handed <= before.handed + 1, "{q}: {after:?}");
            eprintln!(
                "query {i} reaches the copies: {} ({q})",
                after.reached_since(&before, 1)
            );
        }
    }

    /// The statement the read-only transaction tests read by: groups, and every aggregate kind.
    const SNAPSHOT_Q: &str = "SELECT g, count(*), sum(v), min(v), max(s) FROM t GROUP BY g";

    /// A storage's reading connection with the copies on, its table filled, its copy bought and
    /// read once; and a second connection on the same storage that writes.
    fn reader_and_writer(
        room: Option<Arc<dyn Source>>,
    ) -> (
        Arc<Database>,
        Arc<crate::Connection>,
        Arc<crate::Connection>,
    ) {
        let (db, reader) = connect_in(room);
        let writer = db
            .connect(ConnectOpts::new().with_versioned_storage())
            .unwrap();
        copies(&reader).set_on(true);
        reader.execute(SCHEMA).unwrap();
        fill(&reader, 1, 2000);
        // Rent, buy (built after the statement's choice), then read.
        for _ in 0..3 {
            rows(&reader, SNAPSHOT_Q);
        }
        assert!(
            copies(&reader).counts().copy >= 1,
            "{:?}",
            copies(&reader).counts()
        );
        (db, reader, writer)
    }

    /// T-CB.4.5 (lane colcopy-10, AC-CB.4.7): a transaction that only reads, its first statement
    /// before another connection's update, insert and delete, then the same statement three times
    /// after them. Expect: each statement in the transaction is handed over and none is skipped,
    /// the copy answers them (its counts grow, none refused), and each answers as the
    /// transaction's snapshot sees the table: what it answered before the writes, which the writer
    /// no longer sees; after COMMIT the reader sees the writes.
    #[test]
    fn cb4_a_read_only_transaction_reads_the_copy_at_its_snapshot() {
        let (_db, reader, writer) = reader_and_writer(None);
        reader.execute("BEGIN").unwrap();
        let first = copies(&reader).counts();
        let then = rows(&reader, SNAPSHOT_Q);
        for w in [
            "UPDATE t SET v = v + 5000, s = 'zz' WHERE g = 3",
            "INSERT INTO t VALUES (9000, 3, 1, 'zzz')",
            "DELETE FROM t WHERE g = 4",
        ] {
            writer.execute(w).unwrap();
        }
        let before = copies(&reader).counts();
        assert!(before.reached_since(&first, 1), "{before:?}");
        for _ in 0..3 {
            assert_eq!(rows(&reader, SNAPSHOT_Q), then);
        }
        let after = copies(&reader).counts();
        assert!(after.reached_since(&before, 3), "{after:?}");
        // The copy's view and the rows' learned rates, which the planner chose by.
        let tree = tree_of(root(&reader)).unwrap();
        let view = copies(&reader)
            .kept(tree)
            .map(|c| c.columns.plan_view(Ts::MAX));
        let rates = RowRates::learned(&reader.db.learned_costs);
        assert!(
            after.copy >= before.copy + 3,
            "the copy didn't answer: {after:?}, its view {view:?}, the rows' rates {rates:?}"
        );
        assert_eq!(after.unserved, first.unserved, "{after:?}");
        let now = rows(&writer, SNAPSHOT_Q);
        assert_ne!(now, then);
        reader.execute("COMMIT").unwrap();
        assert_eq!(rows(&reader, SNAPSHOT_Q), now);
    }

    /// T-CB.4.6 (lane colcopy-10, AC-CB.4.7): a transaction that only reads, its first statement
    /// before two of another connection's writes the copy can't queue (its memory source refuses
    /// the queue), each followed by a statement that rebuilds the copy, so neither generation it
    /// keeps was made at or before the transaction's snapshot. Expect: the copy refuses the
    /// transaction's next statement, counted `unserved` and not skipped, and the rows answer it as
    /// the snapshot sees the table; after COMMIT the reader sees the writes.
    #[test]
    fn cb4_a_snapshot_no_generation_answers_is_refused_and_the_rows_answer() {
        let (_db, reader, writer) = reader_and_writer(refusing(super::super::columns::QUEUE));
        reader.execute("BEGIN").unwrap();
        let then = rows(&reader, SNAPSHOT_Q);
        for w in [
            "UPDATE t SET v = v + 5000, s = 'zz' WHERE g = 3",
            "DELETE FROM t WHERE g = 4",
        ] {
            let built = copies(&reader).counts().rebuilds;
            writer.execute(w).unwrap();
            rows(&writer, SNAPSHOT_Q);
            assert_eq!(copies(&reader).counts().rebuilds, built + 1);
        }
        let before = copies(&reader).counts();
        assert_eq!(rows(&reader, SNAPSHOT_Q), then);
        let after = copies(&reader).counts();
        assert!(after.reached_since(&before, 1), "{after:?}");
        assert_eq!(after.unserved, before.unserved + 1, "{after:?}");
        assert_eq!(after.copy, before.copy, "{after:?}");
        reader.execute("COMMIT").unwrap();
        assert_eq!(rows(&reader, SNAPSHOT_Q), rows(&writer, SNAPSHOT_Q));
    }

    /// Do: a copy whose queue holds no commit (its memory source refuses the queue), read, then a
    /// write it can't queue, then the statement again. Expect: the copy behind its snapshot is not read (the
    /// rows answer, with the write), then rebuilt and read again, every answer the rows'.
    #[test]
    fn cs5_a_copy_behind_its_snapshot_is_never_read() {
        let (_a, on) = connect_in(refusing(super::super::columns::QUEUE));
        let (_b, off) = connect();
        copies(&on).set_on(true);
        copies(&off).set_on(false);
        let q = "SELECT g, count(*), sum(v), max(s) FROM t GROUP BY g";
        for c in [&on, &off] {
            c.execute(SCHEMA).unwrap();
            fill(c, 1, 2000);
        }
        // Rent, buy (built after the statement's choice), then read.
        for _ in 0..3 {
            assert_eq!(rows(&on, q), rows(&off, q));
        }
        let before = copies(&on).counts();
        assert!(before.copy >= 1, "{before:?}");
        for c in [&on, &off] {
            c.execute("UPDATE t SET v = v + 1000, s = 'zzz' WHERE g = 2")
                .unwrap();
        }
        let tree = tree_of(root(&on)).unwrap();
        let copy = copies(&on).kept(tree).unwrap();
        assert_eq!(copy.columns.feed().counts().lost, 1);
        assert_eq!(rows(&on, q), rows(&off, q));
        let behind = copies(&on).counts();
        assert_eq!(
            behind.copy, before.copy,
            "a copy behind was read: {behind:?}"
        );
        assert_eq!(behind.rows, before.rows + 1, "{behind:?}");
        assert_eq!(behind.rebuilds, before.rebuilds + 1, "{behind:?}");
        // Current again: read.
        assert_eq!(rows(&on, q), rows(&off, q));
        assert!(copies(&on).counts().copy > before.copy);
    }

    /// T-CS.5.10: a storage whose memory source refuses a copy's blocks (and grants the rest), the
    /// analytic statement run four times. Expect: each copy bought (the second and fourth
    /// statements) is let go with the source's typed refusal kept and counted, none is read, and
    /// every answer is the rows'.
    #[test]
    fn cs5_a_copy_its_memory_refuses_is_let_go_and_the_rows_answer() {
        let (_a, on) = connect_in(refusing(BLOCKS));
        let (_b, off) = connect();
        copies(&on).set_on(true);
        copies(&on).set_rebuy(Rebuy::Rent);
        copies(&off).set_on(false);
        let q = "SELECT g, count(*), sum(v) FROM t GROUP BY g";
        for c in [&on, &off] {
            c.execute(SCHEMA).unwrap();
            fill(c, 1, 2000);
        }
        for _ in 0..4 {
            assert_eq!(rows(&on, q), rows(&off, q));
        }
        let seen = copies(&on).counts();
        assert_eq!(seen.copy, 0, "{seen:?}");
        assert_eq!(seen.kept, 2, "{seen:?}");
        assert_eq!(seen.dropped, 2, "{seen:?}");
        let tree = tree_of(root(&on)).unwrap();
        assert!(copies(&on).kept(tree).is_none());
        assert!(
            matches!(copies(&on).last_dropped(), Some(Dropped::Memory { tree: t, asked, .. }) if t == tree && asked > 0),
            "{:?}",
            copies(&on).last_dropped()
        );
    }

    /// T-CS.5.11: an INTEGER column holding text and a blob beside its integers, summed with the
    /// copies' switch on and off, a copy kept. Expect: the same answer each time, a real (SQLite
    /// sums text by its numeric prefix, 'abc' as 0, and the sum is then a real); the copy declines
    /// the column, so no SUM of it is read from the copy, while a SUM of a column of numbers is.
    #[test]
    fn cs5_sum_over_text_in_an_integer_column_answers_as_sqlite() {
        let (_a, on) = connect();
        let (_b, off) = connect();
        copies(&on).set_on(true);
        copies(&off).set_on(false);
        for c in [&on, &off] {
            c.execute(SCHEMA).unwrap();
            fill(c, 1, 3000);
            c.execute("INSERT INTO t VALUES (4001, 1, 'abc', 'x'), (4002, 2, x'01', 'y')")
                .unwrap();
        }
        let numbers = "SELECT g, count(*), sum(g) FROM t GROUP BY g";
        for _ in 0..3 {
            assert_eq!(rows(&on, numbers), rows(&off, numbers));
        }
        let before = copies(&on).counts();
        assert_eq!(before.kept, 1, "{before:?}");
        for q in ["SELECT sum(v) FROM t", "SELECT g, sum(v) FROM t GROUP BY g"] {
            for _ in 0..2 {
                assert_eq!(rows(&on, q), rows(&off, q), "{q}");
            }
        }
        let total = rows(&on, "SELECT sum(v) FROM t");
        assert!(
            matches!(
                total.first().and_then(|r| r.first()),
                Some(Value::Numeric(Numeric::Float(_)))
            ),
            "{total:?}"
        );
        let seen = copies(&on).counts();
        assert_eq!(
            seen.copy, before.copy,
            "a SUM over text was read from the copy: {seen:?}"
        );
    }

    use crate::storage::versioned::memory::Capped;

    /// The bytes granted the generation of a copy of `t` filled with rows 1 to `n`, with room to
    /// spare: what the tests size their room by.
    fn need(n: i64) -> u64 {
        let (_a, c) = connect();
        copies(&c).set_on(true);
        c.execute(SCHEMA).unwrap();
        fill(&c, 1, n);
        for _ in 0..3 {
            rows(&c, QUERIES[2]);
        }
        copies(&c).held().1
    }

    /// A storage whose memory source is a [`Capped`] another use shares, `t` filled with rows 1
    /// to `n`.
    fn pressed(rebuy: Rebuy, n: i64) -> (Arc<Database>, Arc<crate::Connection>, Arc<Capped>) {
        let room = Arc::new(Capped::new(u64::MAX / 2));
        let shared: Arc<dyn Source> = room.clone();
        let (db, on) = connect_in(Some(shared));
        copies(&on).set_on(true);
        copies(&on).set_rebuy(rebuy);
        on.execute(SCHEMA).unwrap();
        fill(&on, 1, n);
        (db, on, room)
    }

    /// The other use takes or gives back so that the copies have `free` bytes in all, what they
    /// hold included, whatever the storage's pages and versions took or gave back since: `other`
    /// the bytes it holds.
    fn press(room: &Capped, other: &mut u64, free: u64, on: &Arc<crate::Connection>) {
        let want = free.saturating_sub(copies(on).meter().held);
        let now = room.room().unwrap();
        if now > want {
            room.take("a test's other use", now - want).unwrap();
            *other = other.saturating_add(now - want);
        } else {
            let back = (want - now).min(*other);
            room.give(back);
            *other = other.saturating_sub(back);
        }
    }

    fn plain() -> (Arc<Database>, Arc<crate::Connection>) {
        let (db, off) = connect();
        copies(&off).set_on(false);
        (db, off)
    }

    /// T-CB.1.3: a copy with room for it, then one with half the room it needs, through writes,
    /// rebuilds and refusals, under both policies. Expect: after every statement and write the
    /// copies' generations hold no more bytes than were granted them, the copies never hold more
    /// than the room left them, and every answer equals the rows'.
    #[test]
    fn cb1_a_copy_never_holds_bytes_it_was_not_granted() {
        let need = need(2000);
        for rebuy in [Rebuy::Rent, Rebuy::Room] {
            for free in [need.saturating_mul(2), need / 2] {
                let (_a, on, room) = pressed(rebuy, 2000);
                let mut other = 0u64;
                let (_b, off) = plain();
                off.execute(SCHEMA).unwrap();
                fill(&off, 1, 2000);
                let step = |label: &str| {
                    let (resident, granted) = copies(&on).held();
                    assert!(
                        resident <= granted,
                        "{rebuy:?} {free} {label}: {resident} held, {granted} granted"
                    );
                };
                let writes = [
                    "UPDATE t SET v = v + 1 WHERE id % 13 = 0",
                    "DELETE FROM t WHERE id % 17 = 0",
                    "INSERT INTO t SELECT value, 1, 2, 'x' FROM generate_series(3001, 3400)",
                ];
                for round in 0..=writes.len() {
                    for q in QUERIES {
                        press(&room, &mut other, free, &on);
                        assert_eq!(
                            rows(&on, q),
                            rows(&off, q),
                            "{rebuy:?} {free} round {round}: {q}"
                        );
                        step(q);
                    }
                    if let Some(w) = writes.get(round) {
                        on.execute(w).unwrap();
                        off.execute(w).unwrap();
                        step(w);
                    }
                }
                let seen = copies(&on).counts();
                if free > need {
                    assert!(seen.copy > 0, "{rebuy:?}: {seen:?}");
                } else {
                    assert!(seen.dropped > 0, "{rebuy:?}: {seen:?}");
                    assert!(copies(&on).meter().refused > 0);
                }
            }
        }
    }

    /// T-CB.2.1: under policy B, a copy refused with half the room it needs, its table growing
    /// while it waits, then room given back. Expect: no rebuy and no whole-table read while the
    /// source has less room than the copy's expected size; the expected size grows with the
    /// table; the copy is bought at the first statement after room appears and read after; every
    /// answer equals the rows'.
    #[test]
    fn cb2_a_refused_copy_waits_for_room_then_is_bought() {
        let need = need(2000);
        let (_a, on, room) = pressed(Rebuy::Room, 2000);
        let mut other = 0u64;
        let (_b, off) = plain();
        off.execute(SCHEMA).unwrap();
        fill(&off, 1, 2000);
        let q = QUERIES[2];
        // Rent, then buy: refused.
        for _ in 0..2 {
            press(&room, &mut other, need / 2, &on);
            assert_eq!(rows(&on, q), rows(&off, q));
        }
        let refused = copies(&on).counts();
        assert_eq!(
            (refused.kept, refused.dropped, refused.rebuys),
            (1, 1, 0),
            "{refused:?}"
        );
        let first = copies(&on).waiting();
        let waited = first.first().map_or(0, |w| w.bytes);
        assert!(first.len() == 1 && waited > need / 2, "{first:?}");
        for _ in 0..5 {
            press(&room, &mut other, need / 2, &on);
            assert_eq!(rows(&on, q), rows(&off, q));
        }
        let still = copies(&on).counts();
        assert_eq!(
            still.kept, refused.kept,
            "bought again with no room: {still:?}"
        );
        assert_eq!(
            still.build_rows, refused.build_rows,
            "a whole-table read with no room: {still:?}"
        );
        assert_eq!(still.absent, refused.absent + 5, "{still:?}");
        // The table grows while the copy waits: so does what it waits for.
        for c in [&on, &off] {
            fill(c, 2001, 2300);
        }
        let grown = copies(&on).waiting().first().map_or(0, |w| w.bytes);
        assert!(grown > waited, "{grown} after {waited}");
        press(&room, &mut other, need / 2, &on);
        assert_eq!(rows(&on, q), rows(&off, q));
        assert_eq!(copies(&on).counts().kept, refused.kept);
        // Room appears: bought at the next statement, then read.
        room.give(other);
        assert_eq!(rows(&on, q), rows(&off, q));
        let back = copies(&on).counts();
        assert_eq!(
            (back.kept, back.resumed, back.rebuys),
            (2, 1, 1),
            "{back:?}"
        );
        assert!(back.rebuy_rows >= 2300, "{back:?}");
        assert!(copies(&on).waiting().is_empty());
        for q in QUERIES {
            assert_eq!(rows(&on, q), rows(&off, q), "{q}");
        }
        assert!(copies(&on).counts().copy > 0, "{:?}", copies(&on).counts());
    }

    /// T-CB.2.2: the same pressure under policy A. Expect: the table rents and buys again and
    /// again, each buy a whole-table read the source refuses; every answer equals the rows'.
    #[test]
    fn cb2_policy_a_rebuys_under_steady_pressure() {
        let need = need(2000);
        let (_a, on, room) = pressed(Rebuy::Rent, 2000);
        let mut other = 0u64;
        let (_b, off) = plain();
        off.execute(SCHEMA).unwrap();
        fill(&off, 1, 2000);
        let q = QUERIES[2];
        for _ in 0..8 {
            press(&room, &mut other, need / 2, &on);
            assert_eq!(rows(&on, q), rows(&off, q));
        }
        let seen = copies(&on).counts();
        assert_eq!(
            (seen.kept, seen.dropped, seen.rebuys),
            (4, 4, 3),
            "{seen:?}"
        );
        assert_eq!(seen.rebuy_rows, 3 * 2000, "{seen:?}");
        assert!(copies(&on).waiting().is_empty());
    }

    /// The A/B harness for the two rebuy policies (lane colcopy-bound, AC-CB.4.1; scenarios shared
    /// with the bench's DuckDB comparison in [`columns_ab`], lane colcopy-7): each scenario forces
    /// a failure mode at its extreme, laptop-small and many tables under heavy pressure, under
    /// both, and reports per policy and scenario the statements' latency percentiles, throughput,
    /// the copies' peak and average bytes held, the rows their builds read, rebuys, the time
    /// copies were absent, the copies still waiting at the end, the last copy let go and why, the
    /// copies' trace events by reason, the writes the memory source refused, and whether every
    /// answer equalled the row loop's. Sizes scale by `AMPHORA_AB_SCALE` (1 by default: the tiny
    /// instance the test suite runs); `AMPHORA_AB_OUT` names the JSONL file the records go to. The
    /// run of record is the coordinator's:
    ///
    /// `AMPHORA_AB_SCALE=200 AMPHORA_AB_OUT=bench/records/<date>-<host>-colcopy-ab.jsonl cargo test -p amphora-engine --release --lib storage::versioned::columns_sql::tests::ab -- --nocapture`
    mod ab {
        use super::*;
        use crate::storage::versioned::columns_ab::{scenarios, schema, Press, Scenario, Traces};
        use std::time::Instant;

        fn scale() -> i64 {
            std::env::var("AMPHORA_AB_SCALE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1)
                .max(1)
        }

        fn fill_sql(i: usize, from: i64, to: i64) -> String {
            format!(
                "INSERT INTO t{i} SELECT value, value % 7, (value * 37) % 1000, \
                 CASE WHEN value % 5 = 0 THEN NULL ELSE 'k' || (value % 11) END \
                 FROM generate_series({from}, {to})"
            )
        }

        fn at(sorted: &[u64], p: f64) -> u64 {
            let n = sorted.len();
            let i = ((p * n as f64).ceil() as usize).clamp(1, n.max(1)) - 1;
            sorted.get(i).copied().unwrap_or(0)
        }

        fn run(s: &Scenario, rebuy: Rebuy, need: u64) -> serde_json::Value {
            let room = Arc::new(Capped::new(u64::MAX / 2));
            let shared: Arc<dyn Source> = room.clone();
            let (_a, on) = connect_in(Some(shared));
            let (_b, off) = plain();
            copies(&on).set_on(true);
            copies(&on).set_rebuy(rebuy);
            for c in [&on, &off] {
                for i in 0..s.tables {
                    c.execute(schema(i)).unwrap();
                    c.execute(fill_sql(i, 1, s.rows)).unwrap();
                }
            }
            let mut other = 0u64;
            // Every statement's latency, and those of the statements a copy could answer (lane
            // colcopy-9, AC-CB.4.6): the A/B's percentiles count only those, as the rest read
            // the rows under either policy. The queries (`QUERIES`' places) any statement of
            // skipped are named in the record.
            let mut latencies = Vec::with_capacity(s.statements);
            let mut reached = Vec::with_capacity(s.statements);
            let mut skipped_queries = std::collections::BTreeSet::new();
            let (mut held_sum, mut held_peak, mut equal, mut waited_most) =
                (0u64, 0u64, true, 0u64);
            let (mut writes_refused, mut write_refusal) = (0u64, None::<String>);
            let mut next = vec![s.rows + 1; s.tables];
            let mut rounds = Vec::new();
            let (mut resident_peak, mut granted_peak) = (0u64, 0u64);
            let began = Instant::now();
            for k in 0..s.statements {
                let i = s.table(k);
                if s.grow > 0 {
                    let from = next.get(i).copied().unwrap_or(s.rows + 1);
                    let w = fill_sql(i, from, from + s.grow - 1);
                    // The scenario's room is the copies' (lane colcopy-8): the other use gives its
                    // bytes back while the rows grow, so the table's growth is never what the cap
                    // refuses, and the press after it puts the copies back at their room. A copy
                    // that must grow with its table then meets the cap first, as the scenario
                    // says. A write still refused is counted, and left out of the row loop's
                    // table too, so the answers still compare.
                    room.give(other);
                    other = 0;
                    match on.execute(&w) {
                        Ok(()) => {
                            off.execute(&w).unwrap();
                            if let Some(n) = next.get_mut(i) {
                                *n = from + s.grow;
                            }
                        }
                        Err(e) => {
                            writes_refused += 1;
                            write_refusal.get_or_insert_with(|| e.to_string());
                        }
                    }
                }
                // The room the copies have at this statement, by the scenario.
                if let Some(f) = s.free_at(k, need) {
                    press(&room, &mut other, f, &on);
                }
                let q = s.query(k);
                let before = copies(&on).counts();
                let t = Instant::now();
                let got = rows(&on, &q);
                let took = u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX);
                latencies.push(took);
                if copies(&on).counts().reached_since(&before, 1) {
                    reached.push(took);
                } else {
                    skipped_queries.insert(k % QUERIES.len());
                }
                equal &= got == rows(&off, &q);
                let held = copies(&on).meter().held;
                held_sum = held_sum.saturating_add(held);
                held_peak = held_peak.max(held);
                let (resident, granted) = copies(&on).held();
                assert!(
                    resident <= granted,
                    "{}: {resident} resident, {granted} granted",
                    s.name
                );
                resident_peak = resident_peak.max(resident);
                granted_peak = granted_peak.max(granted);
                waited_most = copies(&on)
                    .waiting()
                    .iter()
                    .fold(waited_most, |m, w| m.max(w.nanos));
                // Each round's end (one statement on each table): what the copies did in it, so a
                // record shows in which round the room met them.
                if (k + 1) % s.tables.max(1) == 0 {
                    let (c, m) = (copies(&on).counts(), copies(&on).meter());
                    rounds.push(serde_json::json!({
                        "kept": c.kept, "dropped": c.dropped, "rebuilds": c.rebuilds,
                        "copy": c.copy, "rows": c.rows, "unserved": c.unserved,
                        "asked": m.asked, "refused": m.refused, "held": m.held,
                    }));
                }
            }
            let elapsed = began.elapsed().as_secs_f64();
            room.give(other);
            latencies.sort_unstable();
            reached.sort_unstable();
            let reached_ns = reached.iter().fold(0u64, |n, t| n.saturating_add(*t));
            let c = copies(&on).counts();
            let m = copies(&on).meter();
            let waiting = copies(&on).waiting();
            let n = s.statements.max(1) as u64;
            let last_dropped = copies(&on).last_dropped().map(
                |Dropped::Memory {
                     tree,
                     asked,
                     held,
                     limit,
                 }| {
                    serde_json::json!({"why": "memory source refused", "tree": tree, "asked": asked, "held": held, "limit": limit})
                },
            );
            serde_json::json!({
                "harness": "colcopy-ab",
                "policy": match rebuy { Rebuy::Rent => "A_rent", Rebuy::Room => "B_room" },
                "scenario": s.name,
                "forces": s.forces,
                "tables": s.tables,
                "rows": s.rows,
                "statements": s.statements,
                "need_bytes": need,
                "free_bytes": s.free(need),
                "p50_ns": at(&reached, 0.5),
                "p99_ns": at(&reached, 0.99),
                "p999_ns": at(&reached, 0.999),
                "reached_statements": reached.len(),
                "reached_statements_per_s": reached.len() as f64 / (reached_ns as f64 / 1e9).max(f64::MIN_POSITIVE),
                "skipped_queries": skipped_queries,
                "skipped": {"rows": c.skipped_rows, "body": c.skipped_body, "join": c.skipped_join, "transaction": c.skipped_transaction},
                "unserved": c.unserved,
                "handed": c.handed,
                "all_p50_ns": at(&latencies, 0.5),
                "all_p99_ns": at(&latencies, 0.99),
                "all_p999_ns": at(&latencies, 0.999),
                "statements_per_s": s.statements as f64 / elapsed.max(f64::MIN_POSITIVE),
                "peak_held": held_peak,
                "avg_held": held_sum / n,
                "build_rows": c.build_rows,
                "rebuy_rows": c.rebuy_rows,
                "build_bytes": c.build_bytes,
                "rebuy_bytes": c.rebuy_bytes,
                "resident_peak": resident_peak,
                "granted_peak": granted_peak,
                "rebuys": c.rebuys,
                "kept": c.kept,
                "dropped": c.dropped,
                "resumed": c.resumed,
                "absent_ns": copies(&on).absent_nanos(),
                "longest_wait_ns": waited_most,
                "waiting_end": waiting.len(),
                "waiting_bytes_end": waiting.iter().fold(0u64, |b, w| b.saturating_add(w.bytes)),
                "last_dropped": last_dropped,
                "copy_statements": c.copy,
                "absent_statements": c.absent,
                "copy_rows": c.granule_rows,
                "loop_rows": c.loop_rows,
                "asked": m.asked,
                "granted": m.granted,
                "refused": m.refused,
                "refused_bytes": m.refused_bytes,
                "writes_refused": writes_refused,
                "write_refusal": write_refusal,
                "equal": equal,
                "rounds": rounds,
            })
        }

        /// T-CB.4.1: every scenario once under each policy, the copies' trace events counted.
        /// Expect: every answer equal to the row loop's; in a steady room under one copy on more
        /// than one statement a table, each policy's failure mode shows (A buys again, B's copies
        /// still wait at the end and never buy again); a growing table's writes never refused
        /// (its copy meets the cap, not its rows); under waves copies a wave bought let go in a
        /// round after it (T-CB.4.3); one record per policy and
        /// scenario (written to `AMPHORA_AB_OUT` when given).
        #[test]
        fn cb4_ab_both_rebuy_policies_at_the_extremes() {
            let k = scale();
            let mut out = Vec::new();
            for s in scenarios(k) {
                let need = need_of(s.rows);
                for rebuy in [Rebuy::Rent, Rebuy::Room] {
                    let traces = Traces::default();
                    let mut line = traces.during(|| run(&s, rebuy, need));
                    if let Some(o) = line.as_object_mut() {
                        o.insert("traces".into(), serde_json::json!(traces.counts()));
                        o.insert("traces_compiled".into(), Traces::compiled().into());
                    }
                    assert_eq!(line["equal"], true, "{line}");
                    // Lane colcopy-9 (T-CB.4.4): the A/B counts only statements a copy could
                    // answer, so there must be some, and each statement handed over is either
                    // one of them or counted with its reason.
                    let skipped = line["skipped"]
                        .as_object()
                        .map_or(0, |o| o.values().filter_map(|v| v.as_u64()).sum::<u64>());
                    assert!(line["reached_statements"].as_u64() > Some(0), "{line}");
                    assert_eq!(
                        line["reached_statements"].as_u64().map(|r| r + skipped),
                        line["handed"].as_u64(),
                        "{line}"
                    );
                    let under_one = matches!(s.press, Press::Steady { copies } if copies < 1.0);
                    if under_one && s.statements >= 4 * s.tables {
                        assert!(line["dropped"].as_u64() > Some(0), "{line}");
                        match rebuy {
                            Rebuy::Rent => assert!(line["rebuys"].as_u64() > Some(0), "{line}"),
                            Rebuy::Room => {
                                assert_eq!(line["rebuys"], 0, "{line}");
                                assert!(line["waiting_end"].as_u64() > Some(0), "{line}");
                            }
                        }
                    }
                    // Lane colcopy-8: a growing table's rows have room, so its copy, not its
                    // rows, meets the cap; under waves no copy is bought before the first wave
                    // (the rent comes first) nor refused in it (room for every copy twice over),
                    // so a copy let go is one a wave bought, refused in a round after it.
                    if s.grow > 0 {
                        assert_eq!(line["writes_refused"], 0, "{line}");
                    }
                    if matches!(s.press, Press::Waves) && s.statements >= 4 * s.tables {
                        assert!(line["dropped"].as_u64() > Some(0), "{line}");
                    }
                    let line = line.to_string();
                    eprintln!("{line}");
                    out.push(line);
                }
            }
            if let Ok(path) = std::env::var("AMPHORA_AB_OUT") {
                let mut text = out.join("\n");
                text.push('\n');
                std::fs::write(&path, text).unwrap();
            }
        }

        /// One table of `rows` rows' copy's need, as [`need`] measures it.
        fn need_of(rows: i64) -> u64 {
            super::need(rows)
        }
    }

    fn root(conn: &Arc<crate::Connection>) -> i64 {
        match rows(conn, "SELECT rootpage FROM sqlite_schema WHERE name = 't'")
            .first()
            .and_then(|r| r.first())
        {
            Some(Value::Numeric(Numeric::Integer(n))) => *n,
            other => panic!("no root page: {other:?}"),
        }
    }
}
