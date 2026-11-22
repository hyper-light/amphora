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
//! loop over the rows. A statement in a transaction that changed rows, or outside autocommit,
//! reads the rows: the copy holds commits alone.
//!
//! A copy's queue, decoded blocks and blocks take their bytes from the storage's memory source as
//! they grow and give them back as they go, as its pages and row versions do (AC-CS.5.5): no share
//! is set aside. A copy whose blocks the source refuses is let go, the refusal kept as its reason
//! ([`Dropped`]), and the rows answer; its table's next analytic statement pays the rent again.
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
use super::memory::Source;
use super::switch::{tree_of, Switch};
use super::version::{Snapshot, Ts};
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

/// What a copy's blocks take their bytes from the memory source as.
pub const BLOCKS: &str = "a column copy's blocks";

/// One table's copy: its columns, its decoded blocks kept between scans, and the lock its rebuilds
/// take, so two statements don't rebuild it at once.
pub(crate) struct TableCopy {
    columns: Columns<Plain>,
    cache: Cache,
    rebuilding: Mutex<()>,
    /// Where its blocks' bytes are taken from, and the bytes taken now.
    room: Arc<dyn Source>,
    held: Mutex<u64>,
}

impl TableCopy {
    /// Charges its generations (blocks and deltas) to its memory source as they stand now: what
    /// they grew by taken, what they shrank by given back; the source's typed refusal
    /// ([`LimboError::MemoryBudget`]) when it has no room for the growth.
    fn charge(&self) -> Result<()> {
        let mut held = self.held.lock();
        let now = as_u64(self.columns.blocks_resident());
        if now > *held {
            self.room.take(BLOCKS, now.saturating_sub(*held))?;
        } else if now < *held {
            self.room.give(held.saturating_sub(now));
        }
        *held = now;
        Ok(())
    }
}

impl Drop for TableCopy {
    fn drop(&mut self) {
        let held = *self.held.lock();
        if held > 0 {
            self.room.give(held);
        }
    }
}

/// A storage's SQL tables' copies.
pub(crate) struct Copies {
    on: AtomicBool,
    /// The storage's memory source: what each copy's queue, decoded blocks and blocks take.
    room: Arc<dyn Source>,
    /// The clock the copies' freshness lags and scans are timed by.
    base: Instant,
    tables: Mutex<HashMap<u64, Arc<TableCopy>>>,
    /// Trees an analytic statement has read by the rows, no copy kept yet: the rent paid.
    rented: Mutex<HashSet<u64>>,
    counts: Mutex<CopyCounts>,
    /// The last copy let go, and why.
    dropped: Mutex<Option<Dropped>>,
}

fn nanos(base: Instant) -> u128 {
    base.elapsed().as_nanos()
}

fn tick(base: Instant) -> u64 {
    u64::try_from(nanos(base)).unwrap_or(u64::MAX)
}

impl Copies {
    pub(crate) fn new(on: bool, room: Arc<dyn Source>) -> Self {
        Self {
            on: AtomicBool::new(on),
            room,
            base: Instant::now(),
            tables: Mutex::new(HashMap::new()),
            rented: Mutex::new(HashSet::new()),
            counts: Mutex::new(CopyCounts::default()),
            dropped: Mutex::new(None),
        }
    }

    /// Whether the planner's switch is on: the translator then hands analytic statements to the
    /// scan hook.
    pub(crate) fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    #[cfg(test)]
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
    /// counted; any other error is the caller's. The table's next analytic statement pays the
    /// rent again, and the one after buys again, asking the source anew.
    fn let_go(&self, db: &VDatabase, tree: u64, refused: LimboError) -> Result<()> {
        let LimboError::MemoryBudget {
            asked, held, limit, ..
        } = refused
        else {
            return Err(refused);
        };
        self.forget(db, tree);
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
        if self.rented.lock().insert(tree) {
            return Ok(None);
        }
        let base = self.base;
        let feed = Arc::new(Feed::new(
            tree,
            self.room.clone(),
            Box::new(move || tick(base)),
        ));
        db.keep_columns(feed.clone())?;
        let copy = Arc::new(TableCopy {
            columns: Columns::new(feed),
            cache: Cache::new(self.room.clone()),
            rebuilding: Mutex::new(()),
            room: self.room.clone(),
            held: Mutex::new(0),
        });
        tables.insert(tree, copy.clone());
        self.rented.lock().remove(&tree);
        self.counts.lock().kept += 1;
        Ok(Some(copy))
    }

    /// Forgets `tree`'s copy and its rent: its table was dropped.
    pub(crate) fn forget(&self, db: &VDatabase, tree: u64) {
        self.tables.lock().remove(&tree);
        self.rented.lock().remove(&tree);
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
    let aggs = data
        .aggregates
        .iter()
        .map(|a| match a {
            Aggregate::CountStar => Agg::Count,
            Aggregate::Count(c) => Agg::CountOf(*c),
            Aggregate::Sum(c) => Agg::Sum(*c),
            Aggregate::Min(c) => Agg::Min(*c),
            Aggregate::Max(c) => Agg::Max(*c),
        })
        .collect();
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
    let storage = switch.storage();
    let copies = &storage.copies;
    if !copies.on() || data.join.is_some() || !connection.get_auto_commit() || switch.wrote() {
        return Ok(None);
    }
    let db = storage.db();
    let tree = tree_of(data.root_page)?;
    let Some(copy) = copies.for_statement(db, tree)? else {
        return Ok(None);
    };
    let q = query(data, statement_memory)?;
    let snap = db.snapshot()?;
    let answered = read(copies, db, &copy, &snap, tree, data, &q, connection);
    db.close_snapshot(snap);
    answered
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
    copy.columns.catch_up(tick(base))?;
    // The copy's blocks as they stand, charged to the memory source; refused, the copy is let go
    // and the rows answer.
    if let Err(refused) = copy.charge() {
        copies.let_go(db, tree, refused)?;
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
            let start = clock();
            let all = rows.all()?;
            copy.columns.rebuild(snap.start, &all, tick(base))?;
            copy.columns.rebuilt(clock().saturating_sub(start));
            copies.counts.lock().rebuilds += 1;
        }
    }
    if let Err(refused) = copy.charge() {
        copies.let_go(db, tree, refused)?;
    }
    Ok(answer)
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

    const QUERIES: [&str; 7] = [
        "SELECT count(*), sum(v), min(v), max(v) FROM t",
        "SELECT count(s), min(s), max(s) FROM t WHERE v >= 500",
        "SELECT g, count(*), sum(v) FROM t GROUP BY g",
        "SELECT s, count(*), max(id) FROM t WHERE g <> 3 GROUP BY s",
        "SELECT g, s, sum(v) FROM t WHERE s IS NOT NULL GROUP BY g, s",
        "SELECT count(*) FROM t WHERE s IS NULL",
        "SELECT min(id), max(id), sum(id) FROM t WHERE v < 100",
    ];

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
