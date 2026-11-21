//! Amphora: row versions in the versioned storage (Amphora's `docs/design/engine.md`, decision 4).
//!
//! A row's newest version is its cell in the B-link tree, changed in place by its writer under the
//! leaf's exclusive latch. While a snapshot may still need an older one, the cell is flagged
//! versioned and the row's key maps, beside the pages, to a head: who wrote the cell's version (a
//! transaction while it is open, then its commit timestamp) and the versions before it, newest
//! first, each with the timestamp of the commit that wrote it (0: before any snapshot now open).
//! A removed row keeps its cell, flagged removed, until no snapshot can see the row.
//!
//! Commits take timestamps from one clock and are published in order under one short lock, so a
//! snapshot that begins at timestamp `t` sees exactly the commits at or before `t`. A reader sees,
//! for each row, the cell's version if its writer committed at or before its snapshot (or is its
//! own transaction), else the newest older version committed at or before it. A writer that finds a
//! row's newest version written by another open transaction, or committed after its own snapshot
//! began, is refused (`WriteWriteConflict`): the first writer wins. Versions no snapshot can see go:
//! the low watermark is the oldest open snapshot's timestamp ([`Clock::low_watermark`]).
//!
//! A serializable transaction reads through [`VTree::read`] and [`VTree::read_range`], which keep
//! what it read; its commit is validated by the database's [`Validation`] under the publish lock,
//! and refused with `SerializationFailure` where serializability asks (decision 5, and the
//! prototypes G8 step 1 measures against each other).
//!
//! Locks are always taken the publish lock, then the recent commits', then a page latch, then a
//! head's shard, then a transaction's state, so readers, writers, rollbacks, cleanups, collections
//! and validations never wait on each other in a cycle.

use super::blink::{Cell, Change, Descender, Tree};
use super::page::{self, Flags};
use super::pool::Pool;
use super::redo::Op;
use crate::error::LimboError;
use crate::sync::atomic::{AtomicU64, Ordering};
use crate::sync::Mutex;
use crate::Result;
use rustc_hash::FxHashMap;
use std::cmp::Ordering as Order;
use std::collections::{BTreeMap, VecDeque};

/// Ids at or above this name a transaction; below it, a commit timestamp.
const TX: u64 = 1 << 63;

/// Shards of a tree's heads and of the clock's states: a lock each.
const SHARDS: usize = 64;
const SHARDS_U64: u64 = 64;

/// A commit timestamp.
pub type Ts = u64;

fn shard_of(bytes: &[u8]) -> usize {
    // FNV-1a: cheap, and spreads keys that differ in their last bytes.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    usize::try_from(h % SHARDS_U64).unwrap_or(0)
}

/// A transaction's state, by id, while any version names it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Open,
    Committed(Ts),
    Aborted,
}

/// What a version's writer is, resolved.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Writer {
    Committed(Ts),
    Open(u64),
    Aborted,
}

/// The commit clock, the transactions in flight and the snapshots open.
pub struct Clock {
    /// Every commit at or below it is published: snapshots begin here.
    visible: AtomicU64,
    /// Held to publish a commit: the last timestamp handed out.
    publish: Mutex<Ts>,
    next_tx: AtomicU64,
    states: Vec<Mutex<FxHashMap<u64, State>>>,
    /// Open snapshots' timestamps, each with how many are open at it.
    open: Mutex<BTreeMap<Ts, u64>>,
}

/// A read's view: the commits at or before `start`, and its own transaction's changes.
#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub start: Ts,
    own: Option<u64>,
}

/// A transaction: its id, its snapshot, and the rows it changed, each once, in order. It ends by
/// [`Versioned::commit`] or [`Versioned::abort`]; after any error of a change, by `abort`.
pub struct Txn {
    pub id: u64,
    pub start: Ts,
    changed: Vec<(u64, Vec<u8>)>,
    /// Its changes in the order made: its redo record (decision 6).
    ops: Vec<Op>,
    /// What it read through [`VTree::read`] and [`VTree::read_range`]: its commit's validation.
    reads: Reads,
}

/// How a database validates a commit (decision 5). Each one is the floor, snapshot isolation with
/// the first writer winning, and what it adds; G8 step 1 measures the three that make it
/// serializable against each other on the same commit path, and keeps one.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Validation {
    /// Nothing: snapshot isolation, which allows write skew. The Elle checks' control.
    Snapshot,
    /// Precision locking as HyPer adapts it (Neumann, Mühlbauer, Kemper, SIGMOD 2015): a
    /// transaction that wrote checks the rows and spans it read against the rows written by the
    /// commits since its snapshot; any match refuses it. One that only read commits as of its
    /// snapshot, unchecked.
    #[default]
    Precision,
    /// Serializable snapshot isolation as PostgreSQL makes it (Cahill, Röhm, Fekete, SIGMOD 2008;
    /// Ports, Grittner, PVLDB 5(12), 2012): read-write antidependencies among concurrent
    /// transactions, refusing one only where it would complete a dangerous structure
    /// `in -> pivot -> out` whose `out` committed first. Every commit's reads are kept while a
    /// concurrent transaction is open.
    Ssi,
    /// Optimistic validation as Silo makes it (Tu, Zheng, Kohler, Liskov, Madden, SOSP 2013): each
    /// row read is checked in its tree, by the version its head names, for a commit since the
    /// snapshot; each span is read again for one (Silo's node set, at row grain: an open writer's
    /// in-place change moves a leaf's version, which would refuse its own readers). Nothing is
    /// kept between commits.
    Occ,
}

/// What a transaction read: rows by key, and spans of keys, each in its tree.
#[derive(Default)]
struct Reads {
    points: Vec<(u64, Vec<u8>)>,
    spans: Vec<Span>,
}

/// The keys of a tree from `from` to `to`, both included (none: from the least, to the greatest).
struct Span {
    tree: u64,
    from: Option<Vec<u8>>,
    to: Option<Vec<u8>>,
}

impl Reads {
    fn is_empty(&self) -> bool {
        self.points.is_empty() && self.spans.is_empty()
    }

    /// Whether these reads saw `key` of tree `tree`: a row read under it, or a span holding it.
    fn covers(&self, v: &Versioned, tree: u64, key: &[u8]) -> Result<bool> {
        let mut points = self.points.iter().filter(|(t, _)| *t == tree).peekable();
        let mut spans = self.spans.iter().filter(|s| s.tree == tree).peekable();
        if points.peek().is_none() && spans.peek().is_none() {
            return Ok(false);
        }
        let cmp = v.tree(tree)?.tree.compare();
        if points.any(|(_, k)| cmp(k, key) == Order::Equal) {
            return Ok(true);
        }
        Ok(spans.any(|s| {
            s.from
                .as_deref()
                .is_none_or(|f| cmp(f, key) != Order::Greater)
                && s.to
                    .as_deref()
                    .is_none_or(|t| cmp(key, t) != Order::Greater)
        }))
    }
}

/// A commit precision locking or SSI keeps while a transaction concurrent with it is open: its
/// timestamp, the rows it wrote, and for SSI what it read and the earliest commit it read past
/// that came before its own (its `out` edge to a transaction that committed first).
struct Committed {
    ts: Ts,
    writes: Vec<(u64, Vec<u8>)>,
    reads: Reads,
    out_first: Option<Ts>,
}

impl Txn {
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            start: self.start,
            own: Some(self.id),
        }
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock {
    pub fn new() -> Self {
        Self {
            visible: AtomicU64::new(0),
            publish: Mutex::new(0),
            next_tx: AtomicU64::new(0),
            states: (0..SHARDS)
                .map(|_| Mutex::new(FxHashMap::default()))
                .collect(),
            open: Mutex::new(BTreeMap::new()),
        }
    }

    fn states(&self, id: u64) -> Result<&Mutex<FxHashMap<u64, State>>> {
        let at = usize::try_from(id % SHARDS_U64).map_err(|_| LimboError::IntegerOverflow)?;
        self.states.get(at).ok_or_else(|| {
            LimboError::InternalError("versioned storage: a missing state shard".into())
        })
    }

    /// A snapshot of every commit published now, open: read under the open snapshots' lock, so
    /// a low watermark taken at once either counts it or is at most its timestamp (read before
    /// the lock, a commit and a collection between the read and the count could drop versions it
    /// sees, and the commits a validation needs for it).
    fn open_now(&self) -> Result<Ts> {
        let mut open = self.open.lock();
        let start = self.visible.load(Ordering::Acquire);
        let n = open.entry(start).or_insert(0);
        *n = n.checked_add(1).ok_or(LimboError::IntegerOverflow)?;
        Ok(start)
    }

    /// Ends a snapshot that began at `start` ([`Clock::snapshot`], [`Clock::begin`]).
    pub fn close(&self, start: Ts) {
        let mut open = self.open.lock();
        if let Some(n) = open.get_mut(&start) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                open.remove(&start);
            }
        }
    }

    /// A read-only snapshot of every commit published now; [`Clock::close`] it.
    pub fn snapshot(&self) -> Result<Snapshot> {
        let start = self.open_now()?;
        Ok(Snapshot { start, own: None })
    }

    /// A transaction on a snapshot of every commit published now.
    pub fn begin(&self) -> Result<Txn> {
        let n = self.next_tx.fetch_add(1, Ordering::AcqRel);
        if n >= TX {
            return Err(LimboError::IntegerOverflow);
        }
        let id = TX | n;
        let start = self.open_now()?;
        self.states(id)?.lock().insert(id, State::Open);
        Ok(Txn {
            id,
            start,
            changed: Vec::new(),
            ops: Vec::new(),
            reads: Reads::default(),
        })
    }

    fn resolve(&self, writer: u64) -> Result<Writer> {
        if writer < TX {
            return Ok(Writer::Committed(writer));
        }
        match self.states(writer)?.lock().get(&writer) {
            Some(State::Open) => Ok(Writer::Open(writer)),
            Some(State::Committed(ts)) => Ok(Writer::Committed(*ts)),
            Some(State::Aborted) => Ok(Writer::Aborted),
            None => Err(LimboError::InternalError(format!(
                "versioned storage: a version written by transaction {} the clock no longer knows",
                writer ^ TX
            ))),
        }
    }

    /// Publishes `txn`'s commit: its timestamp, after every commit before it; `log` given the
    /// timestamp first, under the same lock (so commits are logged in timestamp order), and a
    /// failure of it leaves the transaction open, for its caller to roll back.
    fn commit(&self, txn: &Txn, log: impl FnOnce(Ts, &[Op]) -> Result<()>) -> Result<Ts> {
        let mut last = self.publish.lock();
        let ts = last.checked_add(1).ok_or(LimboError::IntegerOverflow)?;
        if ts >= TX {
            return Err(LimboError::IntegerOverflow);
        }
        log(ts, &txn.ops)?;
        self.states(txn.id)?
            .lock()
            .insert(txn.id, State::Committed(ts));
        *last = ts;
        self.visible.store(ts, Ordering::Release);
        Ok(ts)
    }

    /// `f` with the last commit's timestamp, while no commit can be published: a checkpoint's
    /// timestamp and the redo log's switch at it (decisions 6 and 7).
    pub fn at_publish<R>(&self, f: impl FnOnce(Ts) -> R) -> R {
        let last = self.publish.lock();
        f(*last)
    }

    /// After recovery: commits up to `ts` are in, published, and the next is after it.
    pub fn recovered(&self, ts: Ts) {
        let mut last = self.publish.lock();
        *last = (*last).max(ts);
        self.visible.store(*last, Ordering::Release);
    }

    fn set_aborted(&self, txn: &Txn) -> Result<()> {
        self.states(txn.id)?.lock().insert(txn.id, State::Aborted);
        Ok(())
    }

    fn forget(&self, txn: &Txn) -> Result<()> {
        self.states(txn.id)?.lock().remove(&txn.id);
        self.close(txn.start);
        Ok(())
    }

    /// The oldest open snapshot's timestamp, or the last commit's with none open: versions older
    /// than the newest one committed at or before it no snapshot can see.
    pub fn low_watermark(&self) -> Ts {
        let open = self.open.lock();
        let visible = self.visible.load(Ordering::Acquire);
        open.keys()
            .next()
            .copied()
            .map_or(visible, |oldest| oldest.min(visible))
    }
}

/// A row's versions before its cell's: who wrote the cell's version, and the ones before it.
struct Head {
    writer: u64,
    older: Vec<Older>,
}

/// A version a snapshot may still need: the commit that wrote it, and the row (None: absent).
struct Older {
    written: Ts,
    value: Option<Vec<u8>>,
}

/// A B-link tree whose rows have versions.
pub struct VTree {
    pub id: u64,
    tree: Tree,
    heads: Vec<Mutex<FxHashMap<Vec<u8>, Head>>>,
}

/// The row a cell holds (None: removed, or no cell).
fn row_of(cell: Option<Cell<'_>>) -> Option<Vec<u8>> {
    cell.filter(|c| !c.flags.removed).map(|c| c.value.to_vec())
}

impl VTree {
    pub fn new(id: u64, tree: Tree) -> Self {
        Self {
            id,
            tree,
            heads: (0..SHARDS)
                .map(|_| Mutex::new(FxHashMap::default()))
                .collect(),
        }
    }

    fn heads(&self, key: &[u8]) -> Result<&Mutex<FxHashMap<Vec<u8>, Head>>> {
        self.heads.get(shard_of(key)).ok_or_else(|| {
            LimboError::InternalError("versioned storage: a missing head shard".into())
        })
    }

    /// The version of a row `snap` sees, its cell and head given (the leaf latched).
    fn visible(
        clock: &Clock,
        snap: &Snapshot,
        cell: Option<Cell<'_>>,
        head: Option<&Head>,
    ) -> Result<Option<Vec<u8>>> {
        let Some(c) = cell else {
            return Ok(None);
        };
        if !c.flags.versioned {
            return Ok(row_of(Some(c)));
        }
        // Heads come and go under the leaf's exclusive latch with their cells' flags, and
        // recovery clears the flags it reads back: a versioned cell has a head.
        let Some(head) = head else {
            return Err(LimboError::InternalError(
                "versioned storage: a versioned cell with no versions beside it".into(),
            ));
        };
        let cell_seen = match clock.resolve(head.writer)? {
            Writer::Open(id) => snap.own == Some(id),
            Writer::Committed(ts) => ts <= snap.start,
            Writer::Aborted => false,
        };
        if cell_seen {
            return Ok(row_of(Some(c)));
        }
        Ok(head
            .older
            .iter()
            .find(|o| o.written <= snap.start)
            .and_then(|o| o.value.clone()))
    }

    /// The row under `key` as `snap` sees it.
    pub fn get(
        &self,
        pool: &Pool,
        d: &mut Descender,
        clock: &Clock,
        snap: &Snapshot,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        self.tree.read_with(pool, d, key, |cell| {
            let heads = self.heads(key)?.lock();
            Self::visible(clock, snap, cell, heads.get(key))
        })
    }

    /// Up to `most` rows from `from` on as `snap` sees them, in key order.
    pub fn scan(
        &self,
        pool: &Pool,
        d: &mut Descender,
        clock: &Clock,
        snap: &Snapshot,
        from: Option<&[u8]>,
        most: usize,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut out = Vec::new();
        if most == 0 {
            return Ok(out);
        }
        self.tree.scan_with(pool, d, from, |key, cell| {
            let row = if cell.flags.versioned {
                let heads = self.heads(key)?.lock();
                Self::visible(clock, snap, Some(cell), heads.get(key))?
            } else {
                row_of(Some(cell))
            };
            if let Some(row) = row {
                out.push((key.to_vec(), row));
            }
            Ok(out.len() < most)
        })?;
        Ok(out)
    }

    /// The row under `key` as `txn` sees it, the read kept for its commit's validation: a
    /// serializable read (decision 5). `get` with `txn`'s snapshot reads without keeping it.
    pub fn read(
        &self,
        pool: &Pool,
        d: &mut Descender,
        clock: &Clock,
        txn: &mut Txn,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        let row = self.get(pool, d, clock, &txn.snapshot(), key)?;
        txn.reads.points.push((self.id, key.to_vec()));
        Ok(row)
    }

    /// Up to `most` rows from `from` on as `txn` sees them, in key order, the span read kept for
    /// its commit's validation: up to the last row given when there are `most`, else to the end.
    pub fn read_range(
        &self,
        pool: &Pool,
        d: &mut Descender,
        clock: &Clock,
        txn: &mut Txn,
        from: Option<&[u8]>,
        most: usize,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if most == 0 {
            return Ok(Vec::new());
        }
        let rows = self.scan(pool, d, clock, &txn.snapshot(), from, most)?;
        let to = if rows.len() < most {
            None
        } else {
            rows.last().map(|(k, _)| k.clone())
        };
        txn.reads.spans.push(Span {
            tree: self.id,
            from: from.map(<[u8]>::to_vec),
            to,
        });
        Ok(rows)
    }

    /// `txn` takes the row under `key` (its leaf latched, `cell` its cell): refused if another
    /// transaction's change to it is open or committed after `txn`'s snapshot; the version `txn`
    /// replaces kept for the snapshots that see it. The row as `txn` saw it before.
    fn take(
        &self,
        clock: &Clock,
        txn: &mut Txn,
        key: &[u8],
        cell: Option<Cell<'_>>,
        heads: &mut FxHashMap<Vec<u8>, Head>,
    ) -> Result<Option<Vec<u8>>> {
        let before = row_of(cell);
        match heads.get_mut(key) {
            Some(head) => match clock.resolve(head.writer)? {
                Writer::Open(id) if id == txn.id => {}
                Writer::Open(_) => return Err(LimboError::WriteWriteConflict),
                Writer::Committed(ts) if ts > txn.start => {
                    return Err(LimboError::WriteWriteConflict)
                }
                Writer::Committed(ts) => {
                    head.older.insert(
                        0,
                        Older {
                            written: ts,
                            value: before.clone(),
                        },
                    );
                    head.writer = txn.id;
                    txn.changed.push((self.id, key.to_vec()));
                }
                Writer::Aborted => {
                    return Err(LimboError::InternalError(
                        "versioned storage: an aborted transaction's version still in place".into(),
                    ))
                }
            },
            None => {
                heads.insert(
                    key.to_vec(),
                    Head {
                        writer: txn.id,
                        older: vec![Older {
                            written: 0,
                            value: before.clone(),
                        }],
                    },
                );
                txn.changed.push((self.id, key.to_vec()));
            }
        }
        Ok(before)
    }

    /// `txn` puts `value` under `key`: the row it saw there before, if any.
    pub fn put(
        &self,
        pool: &Pool,
        d: &mut Descender,
        clock: &Clock,
        txn: &mut Txn,
        key: &[u8],
        value: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        Tree::check_row(key, value)?;
        self.tree.modify(pool, d, key, |cell| {
            let mut heads = self.heads(key)?.lock();
            let before = self.take(clock, txn, key, cell, &mut heads)?;
            let flags = Flags {
                versioned: true,
                removed: false,
            };
            txn.ops.push(Op::Put {
                tree: self.id,
                key: key.to_vec(),
                value: value.to_vec(),
            });
            Ok((Change::Put(value.to_vec(), flags), before))
        })
    }

    /// `txn` removes the row under `key`: the row it saw there, if any (none: nothing removed).
    pub fn remove(
        &self,
        pool: &Pool,
        d: &mut Descender,
        clock: &Clock,
        txn: &mut Txn,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        self.tree.modify(pool, d, key, |cell| {
            let mut heads = self.heads(key)?.lock();
            let seen = Self::visible(clock, &txn.snapshot(), cell, heads.get(key))?;
            if seen.is_none() {
                // Nothing to remove, unless the cell is another's change, which conflicts.
                if let Some(head) = heads.get(key) {
                    match clock.resolve(head.writer)? {
                        Writer::Open(id) if id != txn.id => {
                            return Err(LimboError::WriteWriteConflict)
                        }
                        Writer::Committed(ts) if ts > txn.start => {
                            return Err(LimboError::WriteWriteConflict)
                        }
                        _ => {}
                    }
                }
                return Ok((Change::Keep, None));
            }
            let before = self.take(clock, txn, key, cell, &mut heads)?;
            let flags = Flags {
                versioned: true,
                removed: true,
            };
            txn.ops.push(Op::Remove {
                tree: self.id,
                key: key.to_vec(),
            });
            Ok((Change::Mark(flags), before))
        })
    }

    /// `op` made directly, with no versions: recovery's corrections and replay (decision 8).
    pub fn apply_plain(&self, pool: &Pool, d: &mut Descender, op: &Op) -> Result<()> {
        match op {
            Op::Put { key, value, .. } => {
                Tree::check_row(key, value)?;
                self.tree.modify(pool, d, key, |_| {
                    Ok((Change::Put(value.clone(), Flags::default()), ()))
                })
            }
            Op::Remove { key, .. } => self.tree.modify(pool, d, key, |_| Ok((Change::Remove, ()))),
        }
    }

    /// The rows whose cell isn't their version as of `ts` (a change of a transaction still open,
    /// or committed after `ts`), each with that version: a checkpoint's list (decision 7). Asked
    /// while no page can change.
    fn corrections(&self, clock: &Clock, ts: Ts, out: &mut Vec<Op>) -> Result<()> {
        for shard in &self.heads {
            let heads = shard.lock();
            for (key, head) in heads.iter() {
                if matches!(clock.resolve(head.writer)?, Writer::Committed(w) if w <= ts) {
                    continue;
                }
                let then = head
                    .older
                    .iter()
                    .find(|o| o.written <= ts)
                    .and_then(|o| o.value.clone());
                out.push(match then {
                    Some(value) => Op::Put {
                        tree: self.id,
                        key: key.clone(),
                        value,
                    },
                    None => Op::Remove {
                        tree: self.id,
                        key: key.clone(),
                    },
                });
            }
        }
        Ok(())
    }

    /// The tree's root page.
    pub fn root(&self) -> u64 {
        self.tree.root()
    }

    /// The B-link tree under the versions: a cursor walks its leaves ([`super::cursor`]).
    pub(super) fn tree(&self) -> &Tree {
        &self.tree
    }

    /// `f` on each of cells `cells` of `leaf` (a page under its latch) whose row `snap` sees, in
    /// key order: its key and that row.
    pub(super) fn leaf_rows(
        &self,
        clock: &Clock,
        snap: &Snapshot,
        leaf: &[u8],
        cells: std::ops::Range<usize>,
        mut f: impl FnMut(&[u8], &[u8]) -> Result<()>,
    ) -> Result<()> {
        for i in cells {
            let key = page::key(leaf, i)?;
            let cell = Cell {
                flags: page::flags(leaf, i)?,
                value: page::value(leaf, i)?,
            };
            if cell.flags.versioned {
                let row = {
                    let heads = self.heads(key)?.lock();
                    Self::visible(clock, snap, Some(cell), heads.get(key))?
                };
                if let Some(row) = row {
                    f(key, &row)?;
                }
            } else if !cell.flags.removed {
                f(key, cell.value)?;
            }
        }
        Ok(())
    }

    /// Puts back the version `txn` replaced under `key` (its rollback).
    fn restore(&self, pool: &Pool, d: &mut Descender, txn: &Txn, key: &[u8]) -> Result<()> {
        self.tree.modify(pool, d, key, |cell| {
            let mut heads = self.heads(key)?.lock();
            let Some(head) = heads.get_mut(key) else {
                return Ok((Change::Keep, ()));
            };
            if head.writer != txn.id || head.older.is_empty() {
                return Ok((Change::Keep, ()));
            }
            let prior = head.older.remove(0);
            head.writer = prior.written;
            let change = if head.older.is_empty() && prior.written == 0 {
                // No snapshot needs anything older: the row is as it was before any.
                heads.remove(key);
                match (prior.value, cell) {
                    (Some(v), _) => Change::Put(v, Flags::default()),
                    (None, Some(_)) => Change::Remove,
                    (None, None) => Change::Keep,
                }
            } else {
                match prior.value {
                    Some(v) => Change::Put(
                        v,
                        Flags {
                            versioned: true,
                            removed: false,
                        },
                    ),
                    None => Change::Mark(Flags {
                        versioned: true,
                        removed: true,
                    }),
                }
            };
            Ok((change, ()))
        })
    }
}

/// A database's versioned trees, their pool, their clock, and how its commits are validated.
pub struct Versioned {
    pub pool: Pool,
    pub clock: Clock,
    trees: Vec<VTree>,
    validation: Validation,
    /// The commits a validation may still check against, oldest first: each later than the low
    /// watermark, so bounded as the versions are (every row one of them wrote keeps its head
    /// until its newest version is at or below the watermark). Locked only under the publish
    /// lock.
    recent: Mutex<VecDeque<Committed>>,
}

impl Versioned {
    /// `trees` empty trees in a pool of `frames` pages.
    pub fn new(
        frames: usize,
        trees: usize,
        cmp: impl Fn() -> super::blink::Comparator,
    ) -> Result<Self> {
        let pool = Pool::new(frames);
        let mut made = Vec::with_capacity(trees);
        for id in 0..trees {
            let id = u64::try_from(id).map_err(|_| LimboError::IntegerOverflow)?;
            made.push(VTree::new(id, Tree::create(&pool, cmp())?));
        }
        Ok(Self {
            pool,
            clock: Clock::new(),
            trees: made,
            validation: Validation::default(),
            recent: Mutex::new(VecDeque::new()),
        })
    }

    /// The same database, its commits validated by `validation`.
    pub fn with_validation(mut self, validation: Validation) -> Self {
        self.validation = validation;
        self
    }

    pub fn validation(&self) -> Validation {
        self.validation
    }

    /// The trees whose roots are `roots`, in `pool` (recovery).
    pub fn open(
        pool: Pool,
        roots: &[u64],
        cmp: impl Fn() -> super::blink::Comparator,
    ) -> Result<Self> {
        let mut trees = Vec::with_capacity(roots.len());
        for (id, &root) in roots.iter().enumerate() {
            let id = u64::try_from(id).map_err(|_| LimboError::IntegerOverflow)?;
            trees.push(VTree::new(id, Tree::open(root, cmp())));
        }
        Ok(Self {
            pool,
            clock: Clock::new(),
            trees,
            validation: Validation::default(),
            recent: Mutex::new(VecDeque::new()),
        })
    }

    /// Each tree's root page, by tree.
    pub fn roots(&self) -> Vec<u64> {
        self.trees.iter().map(VTree::root).collect()
    }

    /// Clears the versions on every tree's cells, dropping removed rows' (recovery, which starts
    /// with no heads: each cell is every snapshot's).
    pub fn clear_versions(&self, d: &mut Descender) -> Result<()> {
        for t in &self.trees {
            t.tree.clear_versions(&self.pool, d)?;
        }
        Ok(())
    }

    /// Every tree's rows whose cell isn't their version as of `ts`, with that version (decision 7).
    pub fn corrections(&self, ts: Ts) -> Result<Vec<Op>> {
        let mut out = Vec::new();
        for t in &self.trees {
            t.corrections(&self.clock, ts, &mut out)?;
        }
        Ok(out)
    }

    pub fn tree(&self, id: u64) -> Result<&VTree> {
        usize::try_from(id)
            .ok()
            .and_then(|i| self.trees.get(i))
            .ok_or_else(|| LimboError::InternalError(format!("versioned storage: no tree {id}")))
    }

    /// Commits `txn`: its changes visible to every snapshot from its timestamp on, which it
    /// returns; its versions then name the timestamp, not the transaction. A commit that fails is
    /// rolled back.
    pub fn commit(&self, txn: Txn, d: &mut Descender) -> Result<Ts> {
        self.commit_logged(txn, d, |_, _| Ok(()))
    }

    /// [`Versioned::commit`], `log` given the timestamp and the changes under the publish lock
    /// (the redo log's append, in timestamp order); a failure of `log` rolls `txn` back. Validated
    /// first, under the same lock, so no commit lands between its check and its timestamp: a
    /// refusal (`SerializationFailure`) rolls `txn` back.
    pub fn commit_logged(
        &self,
        mut txn: Txn,
        d: &mut Descender,
        log: impl FnOnce(Ts, &[Op]) -> Result<()>,
    ) -> Result<Ts> {
        let reads = std::mem::take(&mut txn.reads);
        let published = self.clock.commit(&txn, |ts, ops| {
            let mut recent = self.recent.lock();
            let out_first = self.validate(&txn, &reads, &recent, d)?;
            log(ts, ops)?;
            self.remember(&mut recent, &txn, reads, ts, out_first);
            Ok(())
        });
        let ts = match published {
            Ok(ts) => ts,
            Err(e) => {
                self.abort(txn, d)?;
                return Err(e);
            }
        };
        for (tree, key) in &txn.changed {
            let t = self.tree(*tree)?;
            let mut heads = t.heads(key)?.lock();
            if let Some(head) = heads.get_mut(key) {
                if head.writer == txn.id {
                    head.writer = ts;
                }
            }
        }
        self.clock.forget(&txn)?;
        Ok(ts)
    }

    /// Whether `txn`'s commit may go ahead (decision 5), for SSI with the earliest commit it read
    /// past; refused with `SerializationFailure`. Under the publish lock, `recent` locked.
    fn validate(
        &self,
        txn: &Txn,
        reads: &Reads,
        recent: &VecDeque<Committed>,
        d: &mut Descender,
    ) -> Result<Option<Ts>> {
        // Since the snapshot, newest first.
        let since = || recent.iter().rev().take_while(|c| c.ts > txn.start);
        match self.validation {
            Validation::Snapshot => Ok(None),
            // One that only read is serialized at its snapshot, a prefix of the commit order.
            Validation::Precision | Validation::Occ if txn.changed.is_empty() => Ok(None),
            Validation::Precision => {
                if reads.is_empty() {
                    return Ok(None);
                }
                for c in since() {
                    for (tree, key) in &c.writes {
                        if reads.covers(self, *tree, key)? {
                            return Err(LimboError::SerializationFailure);
                        }
                    }
                }
                Ok(None)
            }
            Validation::Occ => {
                self.occ(txn, reads, d)?;
                Ok(None)
            }
            Validation::Ssi => {
                // `txn -> c` where it read past a row `c` wrote; `c -> txn` where `c` read past
                // one `txn` wrote. Each pair is seen at the later commit of the two, so only
                // committed transactions need checking (Fekete et al.'s theorem: every cycle
                // has a dangerous structure whose `out` commits first).
                let mut out_first: Option<Ts> = None;
                let mut in_last: Option<Ts> = None;
                for c in since() {
                    let mut out = false;
                    for (tree, key) in &c.writes {
                        if reads.covers(self, *tree, key)? {
                            out = true;
                            break;
                        }
                    }
                    if out {
                        // `txn -> c -> o` with `o` committed before `c`, and so before `txn`.
                        if c.out_first.is_some() {
                            return Err(LimboError::SerializationFailure);
                        }
                        out_first = Some(out_first.map_or(c.ts, |o| o.min(c.ts)));
                    }
                    for (tree, key) in &txn.changed {
                        if c.reads.covers(self, *tree, key)? {
                            in_last = Some(in_last.map_or(c.ts, |i| i.max(c.ts)));
                            break;
                        }
                    }
                }
                // `c -> txn -> o` with `o` committed no later than `c` (the same, a two-cycle).
                if let (Some(o), Some(i)) = (out_first, in_last) {
                    if i >= o {
                        return Err(LimboError::SerializationFailure);
                    }
                }
                Ok(out_first)
            }
        }
    }

    /// Silo's read-set check at row grain: refused if a row `txn` read, or one in a span it read
    /// (read again), has a version committed after its snapshot. A head no longer kept was last
    /// written at or below the low watermark, at or before the snapshot. The newest committed
    /// version is the head's own when its writer committed, else the newest older one: an open
    /// writer's change (another's, or `txn`'s own) stands over a commit made since the snapshot
    /// that `txn` read past, and must not hide it.
    fn occ(&self, txn: &Txn, reads: &Reads, d: &mut Descender) -> Result<()> {
        let moved = |head: &Head| -> Result<bool> {
            let newest = match self.clock.resolve(head.writer)? {
                Writer::Committed(ts) => ts,
                Writer::Open(_) | Writer::Aborted => head.older.first().map_or(0, |o| o.written),
            };
            Ok(newest > txn.start)
        };
        for (tree, key) in &reads.points {
            let t = self.tree(*tree)?;
            let heads = t.heads(key)?.lock();
            if let Some(head) = heads.get(key.as_slice()) {
                if moved(head)? {
                    return Err(LimboError::SerializationFailure);
                }
            }
        }
        for s in &reads.spans {
            let t = self.tree(s.tree)?;
            let cmp = t.tree.compare();
            let mut refused = false;
            t.tree
                .scan_with(&self.pool, d, s.from.as_deref(), |key, cell| {
                    if s.to
                        .as_deref()
                        .is_some_and(|to| cmp(key, to) == Order::Greater)
                    {
                        return Ok(false);
                    }
                    if cell.flags.versioned {
                        let heads = t.heads(key)?.lock();
                        if let Some(head) = heads.get(key) {
                            refused = moved(head)?;
                        }
                    }
                    Ok(!refused)
                })?;
            if refused {
                return Err(LimboError::SerializationFailure);
            }
        }
        Ok(())
    }

    /// Keeps `txn`'s commit at `ts` for the validations of transactions concurrent with it, and
    /// lets go of those no open transaction began before.
    fn remember(
        &self,
        recent: &mut VecDeque<Committed>,
        txn: &Txn,
        reads: Reads,
        ts: Ts,
        out_first: Option<Ts>,
    ) {
        let lwm = self.clock.low_watermark();
        while recent.front().is_some_and(|c| c.ts <= lwm) {
            recent.pop_front();
        }
        let (keep, reads) = match self.validation {
            Validation::Precision => (!txn.changed.is_empty(), Reads::default()),
            Validation::Ssi => (!txn.changed.is_empty() || !reads.is_empty(), reads),
            Validation::Snapshot | Validation::Occ => (false, Reads::default()),
        };
        if keep {
            recent.push_back(Committed {
                ts,
                writes: txn.changed.clone(),
                reads,
                out_first,
            });
        }
    }

    /// Commits kept for validations: a measure of [`Versioned`]'s `recent`.
    pub fn recent_commits(&self) -> usize {
        self.recent.lock().len()
    }

    /// Rolls `txn` back: each row it changed as it was before, last change first.
    pub fn abort(&self, txn: Txn, d: &mut Descender) -> Result<()> {
        for (tree, key) in txn.changed.iter().rev() {
            self.tree(*tree)?.restore(&self.pool, d, &txn, key)?;
        }
        self.clock.set_aborted(&txn)?;
        self.clock.forget(&txn)
    }

    /// Drops the versions no open snapshot can see: rows whose cell's version every snapshot sees
    /// lose their heads (and their cells, if removed); others keep only the older versions still
    /// visible to some snapshot. How many rows lost their heads.
    pub fn collect(&self, d: &mut Descender) -> Result<usize> {
        let lwm = self.clock.low_watermark();
        let mut collected = 0usize;
        for t in &self.trees {
            let mut ripe = Vec::new();
            for shard in &t.heads {
                let mut heads = shard.lock();
                for (key, head) in heads.iter_mut() {
                    match self.clock.resolve(head.writer)? {
                        Writer::Committed(ts) if ts <= lwm => ripe.push(key.clone()),
                        _ => {
                            // The newest older version committed at or before the watermark is
                            // the last any snapshot needs.
                            if let Some(keep) = head.older.iter().position(|o| o.written <= lwm) {
                                head.older.truncate(keep.saturating_add(1));
                            }
                        }
                    }
                }
            }
            for key in ripe {
                let gone = t.tree.modify(&self.pool, d, &key, |cell| {
                    let mut heads = t.heads(&key)?.lock();
                    let ripe = match heads.get(&key) {
                        Some(head) => {
                            matches!(self.clock.resolve(head.writer)?, Writer::Committed(ts) if ts <= lwm)
                        }
                        None => false,
                    };
                    if !ripe {
                        return Ok((Change::Keep, false));
                    }
                    heads.remove(&key);
                    let change = match cell {
                        Some(c) if c.flags.removed => Change::Remove,
                        Some(_) => Change::Mark(Flags::default()),
                        None => Change::Keep,
                    };
                    Ok((change, true))
                })?;
                if gone {
                    collected = collected.saturating_add(1);
                }
            }
        }
        Ok(collected)
    }

    /// Rows with versions kept beside the pages, in every tree.
    pub fn versioned_rows(&self) -> usize {
        self.trees
            .iter()
            .map(|t| t.heads.iter().map(|s| s.lock().len()).sum::<usize>())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::Arc;

    fn db() -> Versioned {
        Versioned::new(20_000, 1, || Box::new(|a: &[u8], b: &[u8]| a.cmp(b))).unwrap()
    }

    /// Do: a transaction changes a row and adds one while another, begun before, reads; then it
    /// commits. Expect: the earlier reader sees the rows as they were before and after the commit;
    /// a reader begun after sees the changes; the writer sees its own changes before it commits.
    #[test]
    fn a_snapshot_sees_the_commits_before_it_and_its_own_changes() {
        let v = db();
        let t = v.tree(0).unwrap();
        let mut d = Descender::new();
        let mut setup = v.clock.begin().unwrap();
        t.put(&v.pool, &mut d, &v.clock, &mut setup, b"a", b"1")
            .unwrap();
        v.commit(setup, &mut d).unwrap();

        let reader = v.clock.snapshot().unwrap();
        let mut w = v.clock.begin().unwrap();
        t.put(&v.pool, &mut d, &v.clock, &mut w, b"a", b"2")
            .unwrap();
        t.put(&v.pool, &mut d, &v.clock, &mut w, b"b", b"9")
            .unwrap();
        assert_eq!(
            t.get(&v.pool, &mut d, &v.clock, &w.snapshot(), b"a")
                .unwrap(),
            Some(b"2".to_vec())
        );
        assert_eq!(
            t.get(&v.pool, &mut d, &v.clock, &reader, b"a").unwrap(),
            Some(b"1".to_vec())
        );
        assert_eq!(
            t.get(&v.pool, &mut d, &v.clock, &reader, b"b").unwrap(),
            None
        );
        v.commit(w, &mut d).unwrap();
        assert_eq!(
            t.get(&v.pool, &mut d, &v.clock, &reader, b"a").unwrap(),
            Some(b"1".to_vec())
        );
        assert_eq!(
            t.get(&v.pool, &mut d, &v.clock, &reader, b"b").unwrap(),
            None
        );
        let later = v.clock.snapshot().unwrap();
        assert_eq!(
            t.get(&v.pool, &mut d, &v.clock, &later, b"a").unwrap(),
            Some(b"2".to_vec())
        );
        assert_eq!(
            t.scan(&v.pool, &mut d, &v.clock, &later, None, 10).unwrap(),
            vec![
                (b"a".to_vec(), b"2".to_vec()),
                (b"b".to_vec(), b"9".to_vec())
            ]
        );
        assert_eq!(
            t.scan(&v.pool, &mut d, &v.clock, &reader, None, 10)
                .unwrap(),
            vec![(b"a".to_vec(), b"1".to_vec())]
        );
        v.clock.close(reader.start);
        v.clock.close(later.start);
    }

    /// Do: two open transactions change the same row; then one commits and a transaction begun
    /// before that commit changes the row. Expect: the second writer refused each time
    /// (`WriteWriteConflict`); a transaction begun after the commit may change it.
    #[test]
    fn the_first_writer_wins() {
        let v = db();
        let t = v.tree(0).unwrap();
        let mut d = Descender::new();
        let mut a = v.clock.begin().unwrap();
        let mut b = v.clock.begin().unwrap();
        t.put(&v.pool, &mut d, &v.clock, &mut a, b"k", b"a")
            .unwrap();
        assert!(matches!(
            t.put(&v.pool, &mut d, &v.clock, &mut b, b"k", b"b"),
            Err(LimboError::WriteWriteConflict)
        ));
        assert!(matches!(
            t.remove(&v.pool, &mut d, &v.clock, &mut b, b"k"),
            Err(LimboError::WriteWriteConflict)
        ));
        v.commit(a, &mut d).unwrap();
        assert!(matches!(
            t.put(&v.pool, &mut d, &v.clock, &mut b, b"k", b"b"),
            Err(LimboError::WriteWriteConflict)
        ));
        v.abort(b, &mut d).unwrap();
        let mut c = v.clock.begin().unwrap();
        assert_eq!(
            t.put(&v.pool, &mut d, &v.clock, &mut c, b"k", b"c")
                .unwrap(),
            Some(b"a".to_vec())
        );
        v.commit(c, &mut d).unwrap();
    }

    /// Do: a transaction changes, adds and removes rows, then rolls back. Expect: every row as it
    /// was, to readers before and after, and no versions left once collected.
    #[test]
    fn a_rollback_puts_every_row_back() {
        let v = db();
        let t = v.tree(0).unwrap();
        let mut d = Descender::new();
        let mut setup = v.clock.begin().unwrap();
        for k in [b"a", b"b", b"c"] {
            t.put(&v.pool, &mut d, &v.clock, &mut setup, k, b"0")
                .unwrap();
        }
        v.commit(setup, &mut d).unwrap();
        let mut w = v.clock.begin().unwrap();
        t.put(&v.pool, &mut d, &v.clock, &mut w, b"a", b"1")
            .unwrap();
        t.put(&v.pool, &mut d, &v.clock, &mut w, b"a", b"2")
            .unwrap();
        t.remove(&v.pool, &mut d, &v.clock, &mut w, b"b").unwrap();
        t.put(&v.pool, &mut d, &v.clock, &mut w, b"z", b"1")
            .unwrap();
        v.abort(w, &mut d).unwrap();
        let snap = v.clock.snapshot().unwrap();
        let want = vec![
            (b"a".to_vec(), b"0".to_vec()),
            (b"b".to_vec(), b"0".to_vec()),
            (b"c".to_vec(), b"0".to_vec()),
        ];
        assert_eq!(
            t.scan(&v.pool, &mut d, &v.clock, &snap, None, 10).unwrap(),
            want
        );
        v.clock.close(snap.start);
        v.collect(&mut d).unwrap();
        assert_eq!(v.versioned_rows(), 0);
        let snap = v.clock.snapshot().unwrap();
        assert_eq!(
            t.scan(&v.pool, &mut d, &v.clock, &snap, None, 10).unwrap(),
            want
        );
        v.clock.close(snap.start);
    }

    /// Do: change and remove rows many times with a reader holding an early snapshot, then collect,
    /// close the reader and collect again. Expect: while the reader is open it still sees its
    /// rows, and versions the oldest snapshot needs stay; after, no versions remain, removed rows'
    /// cells are gone, and every row reads as last committed.
    #[test]
    fn versions_no_snapshot_sees_are_collected() {
        let v = db();
        let t = v.tree(0).unwrap();
        let mut d = Descender::new();
        let mut setup = v.clock.begin().unwrap();
        for k in 0..100u8 {
            t.put(&v.pool, &mut d, &v.clock, &mut setup, &[k], &[0])
                .unwrap();
        }
        v.commit(setup, &mut d).unwrap();
        let early = v.clock.snapshot().unwrap();
        for round in 1..5u8 {
            let mut w = v.clock.begin().unwrap();
            for k in 0..100u8 {
                if k.is_multiple_of(10) {
                    t.remove(&v.pool, &mut d, &v.clock, &mut w, &[k]).unwrap();
                } else {
                    t.put(&v.pool, &mut d, &v.clock, &mut w, &[k], &[round])
                        .unwrap();
                }
            }
            v.commit(w, &mut d).unwrap();
        }
        v.collect(&mut d).unwrap();
        assert_eq!(v.versioned_rows(), 100, "the early reader's versions stay");
        assert_eq!(
            t.scan(&v.pool, &mut d, &v.clock, &early, None, 1000)
                .unwrap()
                .len(),
            100
        );
        assert_eq!(
            t.get(&v.pool, &mut d, &v.clock, &early, &[3]).unwrap(),
            Some(vec![0])
        );
        v.clock.close(early.start);
        v.collect(&mut d).unwrap();
        assert_eq!(v.versioned_rows(), 0);
        let now = v.clock.snapshot().unwrap();
        let rows = t.scan(&v.pool, &mut d, &v.clock, &now, None, 1000).unwrap();
        assert_eq!(rows.len(), 90);
        assert!(rows.iter().all(|(_, val)| val == &vec![4]));
        let cells = t.tree.scan(&v.pool, &mut d, None, 1000).unwrap();
        assert_eq!(cells.len(), 90, "removed rows' cells are gone");
        v.clock.close(now.start);
    }

    /// Do: 16 threads move amounts between 50 accounts in transactions (read two, write both),
    /// retrying when refused, while 4 threads sum every account in a snapshot; collect at the end.
    /// Expect: every reader's sum is the total (a snapshot is consistent), and after all
    /// transfers the total and every account's existence hold.
    #[test]
    fn concurrent_transfers_keep_the_total_in_every_snapshot() {
        let v = Arc::new(db());
        let accounts = 50u8;
        let start = 1_000u64;
        {
            let t = v.tree(0).unwrap();
            let mut d = Descender::new();
            let mut setup = v.clock.begin().unwrap();
            for a in 0..accounts {
                t.put(
                    &v.pool,
                    &mut d,
                    &v.clock,
                    &mut setup,
                    &[a],
                    &start.to_be_bytes(),
                )
                .unwrap();
            }
            v.commit(setup, &mut d).unwrap();
        }
        let total = start * u64::from(accounts);
        let amount = |bytes: &[u8]| u64::from_be_bytes(bytes.try_into().unwrap());
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let readers: Vec<_> = (0..4)
            .map(|_| {
                let (v, done) = (v.clone(), done.clone());
                std::thread::spawn(move || {
                    let t = v.tree(0).unwrap();
                    let mut d = Descender::new();
                    let mut sums = 0;
                    while !done.load(std::sync::atomic::Ordering::Relaxed) {
                        let snap = v.clock.snapshot().unwrap();
                        let rows = t
                            .scan(&v.pool, &mut d, &v.clock, &snap, None, 1000)
                            .unwrap();
                        v.clock.close(snap.start);
                        assert_eq!(rows.len(), usize::from(accounts));
                        assert_eq!(rows.iter().map(|(_, b)| amount(b)).sum::<u64>(), total);
                        sums += 1;
                    }
                    sums
                })
            })
            .collect();
        let writers: Vec<_> = (0..16u64)
            .map(|w| {
                let v = v.clone();
                std::thread::spawn(move || {
                    let t = v.tree(0).unwrap();
                    let mut d = Descender::new();
                    let mut seed = w + 1;
                    let mut moved = 0;
                    while moved < 300 {
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                        let from = u8::try_from((seed >> 33) % u64::from(accounts)).unwrap();
                        let to = u8::try_from((seed >> 40) % u64::from(accounts)).unwrap();
                        if from == to {
                            continue;
                        }
                        let mut txn = v.clock.begin().unwrap();
                        let snap = txn.snapshot();
                        let attempt = (|| -> Result<()> {
                            let a =
                                amount(&t.get(&v.pool, &mut d, &v.clock, &snap, &[from])?.unwrap());
                            let b =
                                amount(&t.get(&v.pool, &mut d, &v.clock, &snap, &[to])?.unwrap());
                            let x = a.min(7);
                            t.put(
                                &v.pool,
                                &mut d,
                                &v.clock,
                                &mut txn,
                                &[from],
                                &(a - x).to_be_bytes(),
                            )?;
                            t.put(
                                &v.pool,
                                &mut d,
                                &v.clock,
                                &mut txn,
                                &[to],
                                &(b + x).to_be_bytes(),
                            )?;
                            Ok(())
                        })();
                        match attempt {
                            Ok(()) => {
                                v.commit(txn, &mut d).unwrap();
                                moved += 1;
                            }
                            Err(LimboError::WriteWriteConflict) => v.abort(txn, &mut d).unwrap(),
                            Err(e) => panic!("{e}"),
                        }
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let sums: usize = readers.into_iter().map(|r| r.join().unwrap()).sum();
        assert!(sums > 0, "the readers summed");
        let mut d = Descender::new();
        v.collect(&mut d).unwrap();
        assert_eq!(v.versioned_rows(), 0);
        let t = v.tree(0).unwrap();
        let snap = v.clock.snapshot().unwrap();
        let rows = t
            .scan(&v.pool, &mut d, &v.clock, &snap, None, 1000)
            .unwrap();
        assert_eq!(rows.iter().map(|(_, b)| amount(b)).sum::<u64>(), total);
        v.clock.close(snap.start);
    }

    const SERIALIZABLE: [Validation; 3] = [Validation::Precision, Validation::Ssi, Validation::Occ];
    const EVERY: [Validation; 4] = [
        Validation::Snapshot,
        Validation::Precision,
        Validation::Ssi,
        Validation::Occ,
    ];

    /// A database validating by `validation`, with rows `x` and `y` at 1.
    fn xy(validation: Validation) -> Versioned {
        let v = db().with_validation(validation);
        let t = v.tree(0).unwrap();
        let mut d = Descender::new();
        let mut setup = v.clock.begin().unwrap();
        for k in [b"x", b"y"] {
            t.put(&v.pool, &mut d, &v.clock, &mut setup, k, b"1")
                .unwrap();
        }
        v.commit(setup, &mut d).unwrap();
        v
    }

    fn refused(r: Result<Ts>) -> bool {
        matches!(r, Err(LimboError::SerializationFailure))
    }

    /// G8 step 1 (T-G8.1.2): do two transactions each read `x` and `y` and write the one the
    /// other didn't, both from the same snapshot. Expect: snapshot isolation commits both (write
    /// skew, G2); each serializable validation refuses the second, its write rolled back.
    #[test]
    fn g8_1_write_skew_is_refused_by_each_serializable_validation() {
        for validation in EVERY {
            let v = xy(validation);
            let t = v.tree(0).unwrap();
            let mut d = Descender::new();
            let mut a = v.clock.begin().unwrap();
            let mut b = v.clock.begin().unwrap();
            for txn in [&mut a, &mut b] {
                for k in [b"x", b"y"] {
                    t.read(&v.pool, &mut d, &v.clock, txn, k).unwrap();
                }
            }
            t.put(&v.pool, &mut d, &v.clock, &mut a, b"x", b"0")
                .unwrap();
            t.put(&v.pool, &mut d, &v.clock, &mut b, b"y", b"0")
                .unwrap();
            v.commit(a, &mut d).unwrap();
            let second = v.commit(b, &mut d);
            if validation == Validation::Snapshot {
                assert!(second.is_ok(), "{validation:?}: {second:?}");
            } else {
                assert!(refused(second), "{validation:?}");
                let snap = v.clock.snapshot().unwrap();
                assert_eq!(
                    t.get(&v.pool, &mut d, &v.clock, &snap, b"y").unwrap(),
                    Some(b"1".to_vec()),
                    "{validation:?}: the refused write rolled back"
                );
                v.clock.close(snap.start);
            }
        }
    }

    /// T-G8.1.2: do a transaction read every row from `a` on (fewer than it asked for), another
    /// read `x`, insert `m` into that span and commit, then the first write `x` and commit.
    /// Expect: refused by each serializable validation (each read past the other's write, a
    /// cycle through a phantom), committed under snapshot isolation.
    #[test]
    fn g8_1_a_phantom_in_a_span_read_is_refused() {
        for validation in EVERY {
            let v = xy(validation);
            let t = v.tree(0).unwrap();
            let mut d = Descender::new();
            let mut a = v.clock.begin().unwrap();
            let rows = t
                .read_range(&v.pool, &mut d, &v.clock, &mut a, Some(b"a"), 10)
                .unwrap();
            assert_eq!(rows.len(), 2);
            let mut b = v.clock.begin().unwrap();
            t.read(&v.pool, &mut d, &v.clock, &mut b, b"x").unwrap();
            t.put(&v.pool, &mut d, &v.clock, &mut b, b"m", b"1")
                .unwrap();
            v.commit(b, &mut d).unwrap();
            t.put(&v.pool, &mut d, &v.clock, &mut a, b"x", b"2")
                .unwrap();
            let r = v.commit(a, &mut d);
            assert_eq!(
                refused(r),
                validation != Validation::Snapshot,
                "{validation:?}"
            );
        }
    }

    /// T-G8.1.2: do a transaction read `x`, another write `x` and commit, then the first write
    /// `y` and commit. Expect: precision locking and OCC refuse it (a row it read changed since
    /// its snapshot); SSI commits it (one antidependency, no dangerous structure): what makes
    /// SSI's aborts fewer.
    #[test]
    fn g8_1_a_lone_antidependency_commits_under_ssi_alone() {
        for validation in SERIALIZABLE {
            let v = xy(validation);
            let t = v.tree(0).unwrap();
            let mut d = Descender::new();
            let mut a = v.clock.begin().unwrap();
            t.read(&v.pool, &mut d, &v.clock, &mut a, b"x").unwrap();
            let mut b = v.clock.begin().unwrap();
            t.put(&v.pool, &mut d, &v.clock, &mut b, b"x", b"2")
                .unwrap();
            v.commit(b, &mut d).unwrap();
            t.put(&v.pool, &mut d, &v.clock, &mut a, b"y", b"2")
                .unwrap();
            let r = v.commit(a, &mut d);
            assert_eq!(refused(r), validation != Validation::Ssi, "{validation:?}");
        }
    }

    /// T-G8.1.2: Fekete, O'Neil and O'Neil's read-only anomaly ("A Read-Only Transaction Anomaly
    /// Under Snapshot Isolation", SIGMOD Record 33(3), 2004). Do: `w` reads `x` and `y`; `d`
    /// writes `y` and commits; a reader begins after, reads both and commits; `w` writes `x` and
    /// commits. Expect: the reader commits under every validation; `w` is refused by each
    /// serializable one (under SSI as the pivot of `reader -> w -> d`, `d` committed first).
    #[test]
    fn g8_1_the_read_only_anomaly_is_refused() {
        for validation in EVERY {
            let v = xy(validation);
            let t = v.tree(0).unwrap();
            let mut desc = Descender::new();
            let d = &mut desc;
            let mut w = v.clock.begin().unwrap();
            t.read(&v.pool, d, &v.clock, &mut w, b"x").unwrap();
            t.read(&v.pool, d, &v.clock, &mut w, b"y").unwrap();
            let mut deposit = v.clock.begin().unwrap();
            t.read(&v.pool, d, &v.clock, &mut deposit, b"y").unwrap();
            t.put(&v.pool, d, &v.clock, &mut deposit, b"y", b"21")
                .unwrap();
            v.commit(deposit, d).unwrap();
            let mut reader = v.clock.begin().unwrap();
            t.read(&v.pool, d, &v.clock, &mut reader, b"x").unwrap();
            t.read(&v.pool, d, &v.clock, &mut reader, b"y").unwrap();
            v.commit(reader, d).unwrap();
            t.put(&v.pool, d, &v.clock, &mut w, b"x", b"-10").unwrap();
            let r = v.commit(w, d);
            assert_eq!(
                refused(r),
                validation != Validation::Snapshot,
                "{validation:?}"
            );
        }
    }

    /// T-G8.1.2: do a transaction read `x` while another changes `x` and stays open, then the
    /// first write `y` and commit. Expect: committed under each validation: the open writer
    /// commits after it, so it reads before that writer in the serial order (Silo's refusal of a
    /// read row locked by a writer is for its validation outside a publish lock, which this
    /// commit path doesn't have).
    #[test]
    fn g8_1_an_open_writer_of_a_row_read_refuses_nothing() {
        for validation in EVERY {
            let v = xy(validation);
            let t = v.tree(0).unwrap();
            let mut d = Descender::new();
            let mut a = v.clock.begin().unwrap();
            t.read(&v.pool, &mut d, &v.clock, &mut a, b"x").unwrap();
            let mut b = v.clock.begin().unwrap();
            t.put(&v.pool, &mut d, &v.clock, &mut b, b"x", b"2")
                .unwrap();
            t.put(&v.pool, &mut d, &v.clock, &mut a, b"y", b"2")
                .unwrap();
            v.commit(a, &mut d).unwrap();
            v.commit(b, &mut d).unwrap();
        }
    }

    /// T-G8.1.2: do a transaction read `x`, another write `x` and commit, a third begin after and
    /// write `x` and stay open, then the first write `y` and commit. Expect: precision locking and
    /// OCC refuse it as they do with no third (the open writer's change stands over the commit it
    /// read past, and OCC finds that commit under it); SSI commits it (one antidependency).
    #[test]
    fn g8_1_an_open_writer_does_not_hide_a_commit_read_past() {
        for validation in SERIALIZABLE {
            let v = xy(validation);
            let t = v.tree(0).unwrap();
            let mut d = Descender::new();
            let mut a = v.clock.begin().unwrap();
            t.read(&v.pool, &mut d, &v.clock, &mut a, b"x").unwrap();
            let mut c = v.clock.begin().unwrap();
            t.put(&v.pool, &mut d, &v.clock, &mut c, b"x", b"2")
                .unwrap();
            v.commit(c, &mut d).unwrap();
            let mut b = v.clock.begin().unwrap();
            t.put(&v.pool, &mut d, &v.clock, &mut b, b"x", b"3")
                .unwrap();
            t.put(&v.pool, &mut d, &v.clock, &mut a, b"y", b"2")
                .unwrap();
            let r = v.commit(a, &mut d);
            assert_eq!(refused(r), validation != Validation::Ssi, "{validation:?}");
            v.commit(b, &mut d).unwrap();
        }
    }

    /// T-G8.1.2: do 100 commits one after another, then 100 with a snapshot held open from the
    /// first, then one after it closes. Expect: the commits kept for validation are the last one
    /// alone with nothing open, every one since the held snapshot while it is, and the last alone
    /// again once it closes (the bound of `recent`: the low watermark's).
    #[test]
    fn g8_1_commits_are_kept_only_while_a_snapshot_before_them_is_open() {
        for validation in [Validation::Precision, Validation::Ssi] {
            let v = db().with_validation(validation);
            let t = v.tree(0).unwrap();
            let mut d = Descender::new();
            let mut write = |n: u8| {
                let mut txn = v.clock.begin().unwrap();
                t.read(&v.pool, &mut d, &v.clock, &mut txn, &[n]).unwrap();
                t.put(&v.pool, &mut d, &v.clock, &mut txn, &[n], &[n])
                    .unwrap();
                v.commit(txn, &mut d).unwrap();
            };
            for n in 0..100 {
                write(n);
            }
            assert_eq!(v.recent_commits(), 1, "{validation:?}");
            let held = v.clock.snapshot().unwrap();
            for n in 0..100 {
                write(n);
            }
            assert_eq!(v.recent_commits(), 100, "{validation:?}");
            v.clock.close(held.start);
            write(0);
            assert_eq!(v.recent_commits(), 1, "{validation:?}");
        }
    }
}
