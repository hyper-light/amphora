//! Transactions across groups on real groups (docs/design/xgroup.md, "On a group's database"): a
//! [`Tablet`] over a group's own database, and [`Cluster`], the shell that runs a [`Coordinator`]
//! over [`Database`]s, each step one [`Database`] call: one atomic commit in its group, durable and,
//! on a replicated group, committed by its quorum before it answers.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use amphora_api::Refusal;
use amphora_engine::Value;

use super::coordinator::{Answer, Config, Coordinator, Event, Lost, Protocol, Txn, Why};
use super::lan::Latencies;
use super::{
    Envelope, GroupId, Guard, Intent, Lock, Mark, OpenPage, Outcome, Record, Reply, Tablet, TxnId,
    apply,
};
use crate::database::{Database, Token};
use crate::db::Db;

/// The tables a group keeps for transactions across groups. The data a transaction reads and
/// writes is `__amphora_xg_data`'s: a key-value space spread over the groups.
pub(crate) const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS __amphora_xg_data(key BLOB PRIMARY KEY, value BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS __amphora_xg_lock(key BLOB NOT NULL, coordinator INTEGER NOT NULL, epoch INTEGER NOT NULL, seq INTEGER NOT NULL, anchor INTEGER NOT NULL, kind INTEGER NOT NULL, value BLOB, PRIMARY KEY(key, coordinator, epoch, seq)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS __amphora_xg_lock_txn ON __amphora_xg_lock(coordinator, epoch, seq);
CREATE TABLE IF NOT EXISTS __amphora_xg_record(coordinator INTEGER NOT NULL, epoch INTEGER NOT NULL, seq INTEGER NOT NULL, status INTEGER NOT NULL, participants BLOB NOT NULL, PRIMARY KEY(coordinator, epoch, seq)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS __amphora_xg_done(coordinator INTEGER NOT NULL, epoch INTEGER NOT NULL, seq INTEGER NOT NULL, outcome INTEGER NOT NULL, PRIMARY KEY(coordinator, epoch, seq)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS __amphora_xg_guard(coordinator INTEGER PRIMARY KEY, fence INTEGER NOT NULL, floor_epoch INTEGER, floor_seq INTEGER);
CREATE TABLE IF NOT EXISTS __amphora_xg_epoch(coordinator INTEGER PRIMARY KEY, epoch INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS __amphora_xg_lock_at(coordinator INTEGER NOT NULL, epoch INTEGER NOT NULL, seq INTEGER NOT NULL, stamp INTEGER NOT NULL, PRIMARY KEY(coordinator, epoch, seq)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS __amphora_xg_commit_at(coordinator INTEGER NOT NULL, epoch INTEGER NOT NULL, seq INTEGER NOT NULL, stamp INTEGER NOT NULL, PRIMARY KEY(coordinator, epoch, seq)) WITHOUT ROWID;
";

/// A transaction's lock stamp in a group, and its commit stamp as kept there (lane split-merge-22).
const LOCK_STAMP: &str =
    "SELECT stamp FROM __amphora_xg_lock_at WHERE coordinator = ?1 AND epoch = ?2 AND seq = ?3";
const COMMIT_STAMP: &str =
    "SELECT stamp FROM __amphora_xg_commit_at WHERE coordinator = ?1 AND epoch = ?2 AND seq = ?3";

// The tables' codes: a format, written into every group's database; only ever appended to.
const READ: i64 = 0;
const PUT: i64 = 1;
const DELETE: i64 = 2;
const STAGING: i64 = 0;
const COMMITTED: i64 = 1;
const ABORTED: i64 = 2;

fn int(n: u64) -> anyhow::Result<Value> {
    Ok(Value::from_i64(i64::try_from(n)?))
}

fn blob(bytes: &[u8]) -> anyhow::Result<Value> {
    Value::from_slice(bytes).map_err(|e| anyhow::anyhow!("a value's bytes: {e}"))
}

fn txn_params(txn: TxnId) -> anyhow::Result<[Value; 3]> {
    Ok([int(txn.coordinator)?, int(txn.epoch)?, int(txn.seq)?])
}

fn outcome_code(outcome: Outcome) -> i64 {
    match outcome {
        Outcome::Committed => COMMITTED,
        Outcome::Aborted => ABORTED,
    }
}

fn outcome_of(code: i64) -> anyhow::Result<Outcome> {
    match code {
        COMMITTED => Ok(Outcome::Committed),
        ABORTED => Ok(Outcome::Aborted),
        other => anyhow::bail!("outcome code {other} isn't one"),
    }
}

/// Participants as a record keeps them: each group's number, four bytes big-endian.
fn participants_bytes(groups: &[GroupId]) -> Vec<u8> {
    groups.iter().flat_map(|g| g.to_be_bytes()).collect()
}

fn participants_of(bytes: &[u8]) -> anyhow::Result<Vec<GroupId>> {
    let (chunks, rest) = bytes.as_chunks::<4>();
    if !rest.is_empty() {
        anyhow::bail!(
            "a record's participants are {} bytes, not a multiple of 4",
            bytes.len()
        );
    }
    Ok(chunks.iter().map(|c| GroupId::from_be_bytes(*c)).collect())
}

fn u64_at(row: &amphora_engine::Row, i: usize) -> anyhow::Result<u64> {
    Ok(u64::try_from(row.get::<i64>(i)?)?)
}

fn optional_at(row: &amphora_engine::Row, i: usize) -> anyhow::Result<Option<i64>> {
    match row.get_value(i) {
        Value::Null => Ok(None),
        _ => Ok(Some(row.get::<i64>(i)?)),
    }
}

fn bytes_at(row: &amphora_engine::Row, i: usize) -> Option<Vec<u8>> {
    row.get_value(i).to_blob().map(<[u8]>::to_vec)
}

fn intent_of(kind: i64, value: Option<Vec<u8>>) -> anyhow::Result<Intent> {
    match (kind, value) {
        (READ, _) => Ok(Intent::Read),
        (PUT, Some(value)) => Ok(Intent::Put(value)),
        (DELETE, _) => Ok(Intent::Delete),
        (kind, _) => anyhow::bail!("lock kind {kind} without its value isn't one"),
    }
}

/// The prefix of a Records row's key among a transaction's keys (AC-T1.10.1, lane xgroup-sql-6).
/// A format, only ever appended to: [`RECORD`], then the namespace and the family, each as its
/// length (four bytes, big-endian) and its UTF-8 bytes, then the row's key. A key without the
/// prefix is a row of `__amphora_xg_data`'s key-value space.
pub const RECORD: &[u8] = b"\xffamphora-record\x00";

/// A Records row of `namespace`'s `family` as a transaction's key: what a cross-group commit
/// writes is read back through Records' get, scan and feed.
pub fn record_key(namespace: &str, family: &str, key: &[u8]) -> Result<Vec<u8>, Refusal> {
    let length = |name: &str| {
        u32::try_from(name.len())
            .map(u32::to_be_bytes)
            .map_err(|_| amphora_api::invalid("cross-group key", "a name past four bytes' length"))
    };
    let mut out = RECORD.to_vec();
    out.extend(length(namespace)?);
    out.extend(namespace.as_bytes());
    out.extend(length(family)?);
    out.extend(family.as_bytes());
    out.extend(key);
    Ok(out)
}

/// A transaction's key as the Records row it names: its namespace, family and key; none for a
/// key of the key-value space. A key with the prefix that doesn't decode is refused.
pub fn record_of(bytes: &[u8]) -> anyhow::Result<Option<(&str, &str, &[u8])>> {
    let Some(rest) = bytes.strip_prefix(RECORD) else {
        return Ok(None);
    };
    let decoded = name_at(rest).and_then(|(namespace, rest)| {
        let (family, key) = name_at(rest)?;
        Some((namespace, family, key))
    });
    decoded
        .map(Some)
        .ok_or_else(|| anyhow::anyhow!("a Records key across groups doesn't decode"))
}

/// A name as [`record_key`] writes it, and what follows it.
fn name_at(bytes: &[u8]) -> Option<(&str, &[u8])> {
    let (length, rest) = bytes.split_first_chunk::<4>()?;
    let (name, rest) = rest.split_at_checked(usize::try_from(u32::from_be_bytes(*length)).ok()?)?;
    Some((std::str::from_utf8(name).ok()?, rest))
}

/// Whether a transaction across groups holds any lock, a read's or a write's, on a Records row
/// here: a single-group commit touching it is refused as a conflict, as the lock step would refuse
/// it (AC-T1.10.1). Unlike [`locked`], a read lock counts: the commit would change what it read.
pub(crate) fn lock_held(
    db: &Db,
    namespace: &str,
    family: &str,
    key: &[u8],
) -> anyhow::Result<bool> {
    let key = record_key(namespace, family, key).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut held = false;
    db.query(
        "SELECT 1 FROM __amphora_xg_lock WHERE key = ?1 LIMIT 1",
        [blob(&key)?],
        |_| {
            held = true;
            Ok(false)
        },
    )?;
    Ok(held)
}

/// The cross-group key of a Records row here a transaction across groups holds a write lock on
/// (a put or a delete): the row's own, if it is locked (`LockedReads::Wait` and `WaitOnLock` wait
/// on it). A read lock changes no row, so a read needn't wait on it; `kind <> 0` is [`READ`].
pub(crate) fn locked(
    db: &Db,
    namespace: &str,
    family: &str,
    key: &[u8],
) -> anyhow::Result<Option<Vec<u8>>> {
    let key = record_key(namespace, family, key).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(if held(db, &key)? { Some(key) } else { None })
}

/// Whether a transaction across groups holds a write lock here on the row of cross-group key
/// `key` ([`record_key`]; a read lock changes no row, as [`locked`] says).
pub(crate) fn held(db: &Db, key: &[u8]) -> anyhow::Result<bool> {
    let mut held = false;
    db.query(
        "SELECT 1 FROM __amphora_xg_lock WHERE key = ?1 AND kind <> 0 LIMIT 1",
        [blob(key)?],
        |_| {
            held = true;
            Ok(false)
        },
    )?;
    Ok(held)
}

/// Exclusive end of a Records range in the lock table. Every binary row key begins
/// below the successor of the family prefix, including keys starting with 0xff.
fn lock_range_end(namespace: &str, family: &str, upper: Option<&[u8]>) -> anyhow::Result<Vec<u8>> {
    let key = |row: &[u8]| record_key(namespace, family, row).map_err(|e| anyhow::anyhow!("{e}"));
    match upper {
        Some(upper) => key(upper),
        None => crate::database::successor(&key(&[])?)
            .ok_or_else(|| anyhow::anyhow!("a family's cross-group keys have no end")),
    }
}

/// The cross-group key of the first Records row of `family` in a scan's range a transaction
/// across groups holds a write lock on (as [`locked`]), if one is (T-T1.10.W10): rows from `lower` (past it when
/// `continued`) below `upper` (none: the family's end), and no further than `last` when the
/// page stopped there (none: it covered the range). A lock on a row the page doesn't show yet (a
/// put of a new row) counts as one on a row it shows: either changes what it answers.
pub(crate) fn locked_in(
    db: &Db,
    namespace: &str,
    family: &str,
    lower: &[u8],
    continued: bool,
    upper: Option<&[u8]>,
    last: Option<&[u8]>,
) -> anyhow::Result<Option<Vec<u8>>> {
    let at = |key: &[u8]| record_key(namespace, family, key).map_err(|e| anyhow::anyhow!("{e}"));
    let from = at(lower)?;
    let to = lock_range_end(namespace, family, upper)?;
    let last = last.map(at).transpose()?;
    let mut held = None;
    db.query(
        "SELECT key FROM __amphora_xg_lock WHERE key >= ?1 AND key < ?2 AND kind <> 0 ORDER BY key",
        [blob(&from)?, blob(&to)?],
        |row| {
            let Some(key) = bytes_at(row, 0) else {
                return Ok(true);
            };
            if continued && key == from {
                return Ok(true);
            }
            if last.as_ref().is_some_and(|last| key > *last) {
                return Ok(false);
            }
            held = Some(key);
            Ok(false)
        },
    )?;
    Ok(held)
}

/// The first Records row of `namespace`'s `family` from `lower` up to `upper` (none: the family's
/// end) a transaction across groups holds a write lock on here with its lock stamp at or before
/// `at`, by its cross-group key (lane split-merge-21, T-T1.9.R28): its commit stamp may then be at
/// or before `at`, so a read as of `at` can't answer until it resolves, and waits on that row's
/// waiter list (lane split-merge-22). A read lock writes nothing and holds no read.
pub(crate) fn held_at(
    db: &Db,
    namespace: &str,
    family: &str,
    lower: &[u8],
    upper: Option<&[u8]>,
    at: u64,
) -> anyhow::Result<Option<Vec<u8>>> {
    let refused = |e: Refusal| anyhow::anyhow!("{e}");
    let from = record_key(namespace, family, lower).map_err(refused)?;
    let to = lock_range_end(namespace, family, upper)?;
    let mut held = None;
    db.query(
        "SELECT l.key FROM __amphora_xg_lock l JOIN __amphora_xg_lock_at t ON t.coordinator = l.coordinator AND t.epoch = l.epoch AND t.seq = l.seq WHERE l.key >= ?1 AND l.key < ?2 AND l.kind != ?3 AND t.stamp <= ?4 ORDER BY l.key LIMIT 1",
        [
            blob(&from)?,
            blob(&to)?,
            Value::from_i64(READ),
            Value::from_i64(i64::try_from(at)?),
        ],
        |row| {
            held = bytes_at(row, 0);
            Ok(false)
        },
    )?;
    Ok(held)
}

/// A group's database as a [`Tablet`], inside the step's one transaction.
pub(crate) struct SqlTablet<'a> {
    db: &'a Db,
    /// Each namespace's revision this step writes its Records rows at, and the next row's place.
    revs: BTreeMap<String, (amphora_api::Rev, u64)>,
    /// Records keys to publish when this step commits (T-T1.10.W9).
    dropped: Vec<Vec<u8>>,
}

impl<'a> SqlTablet<'a> {
    pub(crate) fn new(db: &'a Db) -> Self {
        Self {
            db,
            revs: BTreeMap::new(),
            dropped: Vec::new(),
        }
    }

    /// A transaction's stamp as `select` reads it, one of the stamp tables' [`LOCK_STAMP`] and
    /// [`COMMIT_STAMP`].
    fn stamp_in(&self, select: &'static str, txn: TxnId) -> anyhow::Result<Option<u64>> {
        let mut stamp = None;
        self.db.query(select, txn_params(txn)?, |row| {
            stamp = Some(u64_at(row, 0)?);
            Ok(false)
        })?;
        Ok(stamp)
    }
}

/// A group's private writer result. The durable callback publishes the released rows even if
/// the coordinator has stopped waiting for this answer (T-T1.10.W9–W11).
pub(crate) struct AppliedStep {
    reply: Reply,
    pub(crate) dropped: Vec<Vec<u8>>,
}

fn apply_step(db: &Db, here: GroupId, envelope: &Envelope) -> anyhow::Result<AppliedStep> {
    let mut tablet = SqlTablet::new(db);
    let reply = apply(here, &mut tablet, envelope)?;
    Ok(AppliedStep {
        reply,
        dropped: tablet.dropped,
    })
}

impl Tablet for SqlTablet<'_> {
    fn value(&mut self, key: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
        if let Some((namespace, family, row)) = record_of(key)? {
            return crate::database::newest(self.db, namespace, family, row);
        }
        let mut value = None;
        self.db.query(
            "SELECT value FROM __amphora_xg_data WHERE key = ?1",
            [blob(key)?],
            |row| {
                value = bytes_at(row, 0);
                Ok(false)
            },
        )?;
        Ok(value)
    }

    fn set_value(&mut self, key: &[u8], value: Option<&[u8]>) -> anyhow::Result<()> {
        self.set_value_at(key, value, None)
    }

    fn set_value_at(
        &mut self,
        key: &[u8],
        value: Option<&[u8]>,
        stamp: Option<u64>,
    ) -> anyhow::Result<()> {
        if let Some((namespace, family, row)) = record_of(key)? {
            let (at, seq) = match self.revs.get(namespace) {
                Some(&(at, seq)) => (Some(at), seq),
                None => (None, 0),
            };
            let at = crate::database::write_across(
                self.db, namespace, family, row, value, at, seq, stamp,
            )?;
            let next = seq
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("a step's rows ran out of places"))?;
            self.revs.insert(namespace.to_owned(), (at, next));
            return Ok(());
        }
        match value {
            Some(value) => self.db.query(
                "INSERT INTO __amphora_xg_data(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [blob(key)?, blob(value)?],
                |_| Ok(true),
            ),
            None => self.db.query(
                "DELETE FROM __amphora_xg_data WHERE key = ?1",
                [blob(key)?],
                |_| Ok(true),
            ),
        }
    }

    fn stamp(&mut self, key: &[u8]) -> anyhow::Result<Option<u64>> {
        // `conditioned` refuses a condition on a key-value row before any step is sent.
        let Some((namespace, family, row)) = record_of(key)? else {
            anyhow::bail!("a condition names a key-value row, which has no commit stamps");
        };
        crate::database::newest_stamp(self.db, namespace, family, row)
    }

    fn locks(&mut self, key: &[u8]) -> anyhow::Result<Vec<Lock>> {
        let mut locks = Vec::new();
        self.db.query(
            "SELECT coordinator, epoch, seq, anchor, kind, value FROM __amphora_xg_lock WHERE key = ?1",
            [blob(key)?],
            |row| {
                locks.push(Lock {
                    txn: TxnId {
                        coordinator: u64_at(row, 0)?,
                        epoch: u64_at(row, 1)?,
                        seq: u64_at(row, 2)?,
                    },
                    anchor: GroupId::try_from(row.get::<i64>(3)?)?,
                    intent: intent_of(row.get::<i64>(4)?, bytes_at(row, 5))?,
                });
                Ok(true)
            },
        )?;
        Ok(locks)
    }

    fn locks_of(&mut self, txn: TxnId) -> anyhow::Result<Vec<(Vec<u8>, Lock)>> {
        let mut locks = Vec::new();
        self.db.query(
            "SELECT key, anchor, kind, value FROM __amphora_xg_lock WHERE coordinator = ?1 AND epoch = ?2 AND seq = ?3",
            txn_params(txn)?,
            |row| {
                let key = bytes_at(row, 0)
                    .ok_or_else(|| anyhow::anyhow!("a lock's key isn't bytes"))?;
                let lock = Lock {
                    txn,
                    anchor: GroupId::try_from(row.get::<i64>(1)?)?,
                    intent: intent_of(row.get::<i64>(2)?, bytes_at(row, 3))?,
                };
                locks.push((key, lock));
                Ok(true)
            },
        )?;
        Ok(locks)
    }

    fn put_lock(&mut self, key: &[u8], lock: &Lock) -> anyhow::Result<()> {
        let (kind, value) = match &lock.intent {
            Intent::Read => (READ, Value::Null),
            Intent::Put(value) => (PUT, blob(value)?),
            Intent::Delete => (DELETE, Value::Null),
        };
        let [coordinator, epoch, seq] = txn_params(lock.txn)?;
        // The transaction's lock stamp here (lane split-merge-21, T-T1.9.R28): this group's clock
        // as it takes the first lock, after the coordinator's carried stamp. Its commit stamp is at
        // or past it, so a read across groups as of a stamp before it never waits on it, and one
        // at or past it waits for the resolution. A group that stamps nothing keeps none.
        if let Some(hooks) = self.db.stamps() {
            let stamp = hooks.marks.next_micros()?;
            let [c, e, s] = txn_params(lock.txn)?;
            self.db.query(
                "INSERT OR IGNORE INTO __amphora_xg_lock_at(coordinator, epoch, seq, stamp) VALUES (?1, ?2, ?3, ?4)",
                [c, e, s, int(stamp)?],
                |_| Ok(true),
            )?;
        }
        self.db.query(
            "INSERT INTO __amphora_xg_lock(key, coordinator, epoch, seq, anchor, kind, value) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            [
                blob(key)?,
                coordinator,
                epoch,
                seq,
                Value::from_i64(i64::from(lock.anchor)),
                Value::from_i64(kind),
                value,
            ],
            |_| Ok(true),
        )
    }

    fn drop_locks_of(&mut self, txn: TxnId) -> anyhow::Result<()> {
        let dropped = &mut self.dropped;
        self.db.query(
            "SELECT key FROM __amphora_xg_lock WHERE coordinator = ?1 AND epoch = ?2 AND seq = ?3",
            txn_params(txn)?,
            |row| {
                if let Some(key) = bytes_at(row, 0).filter(|key| key.starts_with(RECORD)) {
                    dropped.push(key);
                }
                Ok(true)
            },
        )?;
        self.db.query(
            "DELETE FROM __amphora_xg_lock_at WHERE coordinator = ?1 AND epoch = ?2 AND seq = ?3",
            txn_params(txn)?,
            |_| Ok(true),
        )?;
        self.db.query(
            "DELETE FROM __amphora_xg_lock WHERE coordinator = ?1 AND epoch = ?2 AND seq = ?3",
            txn_params(txn)?,
            |_| Ok(true),
        )
    }

    fn record(&mut self, txn: TxnId) -> anyhow::Result<Option<Record>> {
        let mut record = None;
        self.db.query(
            "SELECT status, participants FROM __amphora_xg_record WHERE coordinator = ?1 AND epoch = ?2 AND seq = ?3",
            txn_params(txn)?,
            |row| {
                let status = row.get::<i64>(0)?;
                record = Some(if status == STAGING {
                    let bytes = bytes_at(row, 1).unwrap_or_default();
                    Record::Staging(participants_of(&bytes)?)
                } else {
                    Record::Decided(outcome_of(status)?)
                });
                Ok(false)
            },
        )?;
        Ok(record)
    }

    fn set_record(&mut self, txn: TxnId, record: &Record) -> anyhow::Result<()> {
        let (status, participants) = match record {
            Record::Staging(groups) => (STAGING, participants_bytes(groups)),
            Record::Decided(outcome) => (outcome_code(*outcome), Vec::new()),
        };
        let [coordinator, epoch, seq] = txn_params(txn)?;
        self.db.query(
            "INSERT INTO __amphora_xg_record(coordinator, epoch, seq, status, participants) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(coordinator, epoch, seq) DO UPDATE SET status = excluded.status, participants = excluded.participants",
            [coordinator, epoch, seq, Value::from_i64(status), blob(&participants)?],
            |_| Ok(true),
        )
    }

    fn done(&mut self, txn: TxnId) -> anyhow::Result<Option<Outcome>> {
        let mut done = None;
        self.db.query(
            "SELECT outcome FROM __amphora_xg_done WHERE coordinator = ?1 AND epoch = ?2 AND seq = ?3",
            txn_params(txn)?,
            |row| {
                done = Some(outcome_of(row.get::<i64>(0)?)?);
                Ok(false)
            },
        )?;
        Ok(done)
    }

    fn set_done(&mut self, txn: TxnId, outcome: Outcome) -> anyhow::Result<()> {
        let [coordinator, epoch, seq] = txn_params(txn)?;
        self.db.query(
            "INSERT INTO __amphora_xg_done(coordinator, epoch, seq, outcome) VALUES (?1, ?2, ?3, ?4)",
            [coordinator, epoch, seq, Value::from_i64(outcome_code(outcome))],
            |_| Ok(true),
        )
    }

    fn lock_stamp(&mut self, txn: TxnId) -> anyhow::Result<Option<u64>> {
        self.stamp_in(LOCK_STAMP, txn)
    }

    fn commit_stamp(&mut self, txn: TxnId) -> anyhow::Result<Option<u64>> {
        self.stamp_in(COMMIT_STAMP, txn)
    }

    fn set_commit_stamp(&mut self, txn: TxnId, at: u64) -> anyhow::Result<()> {
        let [coordinator, epoch, seq] = txn_params(txn)?;
        self.db.query(
            "INSERT OR IGNORE INTO __amphora_xg_commit_at(coordinator, epoch, seq, stamp) VALUES (?1, ?2, ?3, ?4)",
            [coordinator, epoch, seq, int(at)?],
            |_| Ok(true),
        )
    }

    fn guard(&mut self, coordinator: u64) -> anyhow::Result<Guard> {
        let mut guard = Guard::default();
        self.db.query(
            "SELECT fence, floor_epoch, floor_seq FROM __amphora_xg_guard WHERE coordinator = ?1",
            [int(coordinator)?],
            |row| {
                let floor = match (optional_at(row, 1)?, optional_at(row, 2)?) {
                    (Some(epoch), Some(seq)) => Some(Mark {
                        epoch: u64::try_from(epoch)?,
                        seq: u64::try_from(seq)?,
                    }),
                    _ => None,
                };
                guard = Guard {
                    fence: u64_at(row, 0)?,
                    floor,
                };
                Ok(false)
            },
        )?;
        Ok(guard)
    }

    fn set_guard(&mut self, coordinator: u64, guard: Guard) -> anyhow::Result<()> {
        let (epoch, seq) = match guard.floor {
            Some(floor) => (int(floor.epoch)?, int(floor.seq)?),
            None => (Value::Null, Value::Null),
        };
        self.db.query(
            "INSERT INTO __amphora_xg_guard(coordinator, fence, floor_epoch, floor_seq) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(coordinator) DO UPDATE SET fence = excluded.fence, floor_epoch = excluded.floor_epoch, floor_seq = excluded.floor_seq",
            [int(coordinator)?, int(guard.fence)?, epoch, seq],
            |_| Ok(true),
        )
    }

    fn forget_below(&mut self, coordinator: u64, floor: Mark) -> anyhow::Result<()> {
        let params = || -> anyhow::Result<[Value; 3]> {
            Ok([int(coordinator)?, int(floor.epoch)?, int(floor.seq)?])
        };
        self.db.query(
            "DELETE FROM __amphora_xg_done WHERE coordinator = ?1 AND (epoch < ?2 OR (epoch = ?2 AND seq < ?3))",
            params()?,
            |_| Ok(true),
        )?;
        self.db.query(
            "DELETE FROM __amphora_xg_commit_at WHERE coordinator = ?1 AND (epoch < ?2 OR (epoch = ?2 AND seq < ?3))",
            params()?,
            |_| Ok(true),
        )?;
        self.db.query(
            "DELETE FROM __amphora_xg_record WHERE coordinator = ?1 AND status <> 0 AND (epoch < ?2 OR (epoch = ?2 AND seq < ?3))",
            params()?,
            |_| Ok(true),
        )
    }

    fn next_epoch(&mut self, coordinator: u64) -> anyhow::Result<u64> {
        self.db.query(
            "INSERT INTO __amphora_xg_epoch(coordinator, epoch) VALUES (?1, 1) ON CONFLICT(coordinator) DO UPDATE SET epoch = epoch + 1",
            [int(coordinator)?],
            |_| Ok(true),
        )?;
        let mut epoch = None;
        self.db.query(
            "SELECT epoch FROM __amphora_xg_epoch WHERE coordinator = ?1",
            [int(coordinator)?],
            |row| {
                epoch = Some(u64_at(row, 0)?);
                Ok(false)
            },
        )?;
        epoch.ok_or_else(|| anyhow::anyhow!("coordinator {coordinator}'s incarnation wasn't kept"))
    }

    fn open(
        &mut self,
        coordinator: u64,
        below: u64,
        after: Option<TxnId>,
        limit: usize,
    ) -> anyhow::Result<OpenPage> {
        // No transaction precedes (-1, -1): every epoch and count is at least 0.
        let (after_epoch, after_seq) = match after {
            Some(txn) => (i64::try_from(txn.epoch)?, i64::try_from(txn.seq)?),
            None => (-1, -1),
        };
        let fetch = i64::try_from(limit.checked_add(1).unwrap_or(limit))?;
        let params = || -> anyhow::Result<[Value; 5]> {
            Ok([
                int(coordinator)?,
                int(below)?,
                Value::from_i64(after_epoch),
                Value::from_i64(after_seq),
                Value::from_i64(fetch),
            ])
        };
        let mut found: BTreeMap<TxnId, Option<GroupId>> = BTreeMap::new();
        let txn_at = |row: &amphora_engine::Row| -> anyhow::Result<TxnId> {
            Ok(TxnId {
                coordinator,
                epoch: u64_at(row, 0)?,
                seq: u64_at(row, 1)?,
            })
        };
        self.db.query(
            "SELECT DISTINCT epoch, seq, anchor FROM __amphora_xg_lock WHERE coordinator = ?1 AND epoch < ?2 AND (epoch > ?3 OR (epoch = ?3 AND seq > ?4)) ORDER BY epoch, seq LIMIT ?5",
            params()?,
            |row| {
                found.insert(txn_at(row)?, Some(GroupId::try_from(row.get::<i64>(2)?)?));
                Ok(true)
            },
        )?;
        self.db.query(
            "SELECT epoch, seq FROM __amphora_xg_record WHERE coordinator = ?1 AND status = 0 AND epoch < ?2 AND (epoch > ?3 OR (epoch = ?3 AND seq > ?4)) ORDER BY epoch, seq LIMIT ?5",
            params()?,
            |row| {
                found.entry(txn_at(row)?).or_insert(None);
                Ok(true)
            },
        )?;
        let more = found.len() > limit;
        Ok((found.into_iter().take(limit).collect(), more))
    }
}

/// A group as a coordinator reaches it (xgroup.md, "Over the transport"): its [`Database`] in this
/// process, or its owner on a node, reached over the transport by [`crate::transport::Nodes`].
#[derive(Clone)]
pub enum Member {
    Local(Database),
    /// The group's owner at `node`'s endpoint in `nodes`; the node made its tables ([`prepare`]).
    Node {
        nodes: Arc<crate::transport::Nodes>,
        node: crate::reshard::NodeId,
    },
}

/// Makes a group's tables for transactions across groups, if it hasn't them: the node that owns a
/// group does it before it serves steps over the transport.
pub fn prepare(group: &Database, until: Instant) -> Result<(), Refusal> {
    group.call_internal(until, |db| db.exec(SCHEMA))
}

/// A step that came over the transport for this owner's group `here` (`Request::Step`): applied as
/// a local one is, the coordinator's stamp `carried` taken in first; answers the reply, the group's
/// newest stamp after it (none: the group makes no stamps), and its clock after it, the one
/// cross-group clock's microseconds ([`Database::cross_clock`]: a replicated group's closed time, a
/// backed-up group's writer clock; none: it keeps neither; AC-T1.10.1, T-T1.10.W4). Taken after
/// the step answered, the clock covers the step's own commit: an internal call waits
/// for its write's index to apply here before answering.
pub(crate) fn serve(
    group: &Database,
    here: GroupId,
    carried: u64,
    envelope: Envelope,
    until: Instant,
) -> Result<(Reply, Option<u64>, Option<u64>), Refusal> {
    let stamp = AtomicU64::new(carried);
    let applied = step(group, &stamp, until, move |db| {
        apply_step(db, here, &envelope)
    })?;
    // The group's clock after the step, the one cross-group clock as it keeps it (lane
    // split-merge-22, as `step_on` reads a local group's): a backed-up group's writer clock, which
    // a standalone group's session token never had, so its lock replies carry a stamp too.
    let clock = group.cross_clock(until)?;
    Ok((
        applied.reply,
        clock.map(|_| stamp.load(Ordering::Acquire)),
        clock,
    ))
}

/// Runs one step on `member`, its stamps carried either way. A group on a node answers its closed
/// time after the step too, kept in `closed` as the newest heard from it ([`closed_at`]); a
/// local group's is read where it is.
fn step_on(
    member: &Member,
    carried: &AtomicU64,
    closed: Option<&AtomicU64>,
    until: Instant,
    here: GroupId,
    envelope: Envelope,
) -> Result<(Reply, Option<u64>), Refusal> {
    match member {
        Member::Local(group) => {
            let applied = step(group, carried, until, move |db| {
                apply_step(db, here, &envelope)
            })?;
            // Read after the step answered, the group's clock is at or past the step's commit:
            // the one cross-group clock as this group keeps it (lane split-merge-21,
            // `Database::cross_clock`), so a standalone backed-up group's lock reply carries its
            // writer's clock, which a session token never had.
            let clock = group.cross_clock(until)?;
            Ok((applied.reply, clock))
        }
        Member::Node { nodes, node } => {
            let (reply, latest, micros) = nodes
                .step(
                    *node,
                    here,
                    carried.load(Ordering::Acquire),
                    envelope,
                    until,
                )
                .map_err(Refusal::from)?;
            if let Some(latest) = latest {
                carried.fetch_max(latest, Ordering::AcqRel);
            }
            if let (Some(closed), Some(micros)) = (closed, micros) {
                closed.fetch_max(micros, Ordering::AcqRel);
            }
            Ok((reply, micros))
        }
    }
}

/// Which group a key is in: the caller's, so the groups' routing stays the routing lanes' own.
pub type Route = Arc<dyn Fn(&[u8]) -> GroupId + Send + Sync>;

/// A refusal a group answers in place of taking a step, as a sick or overloaded group would: asked
/// before each step goes to group `here`; `Some` is the step's reply instead (the live scenarios'
/// and tests' seam, AC-T1.10.6). None in production.
pub type Fault = Arc<dyn Fn(GroupId, &Envelope) -> Option<Refusal> + Send + Sync>;

/// What the driver does with a refusal that came before the step was taken (AC-T1.10.6, xgroup.md
/// "A refusal before the step"): `NotLeader` or `Full`, a group that ran nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OnRefused {
    /// Today's, the control: the step counts as lost, as one whose answer never came (a lock step
    /// lost aborts its transaction; any other is sent again at once), and a step past its deadline
    /// stops the coordinator.
    #[default]
    Stop,
    /// The same step sent again after the steps' learned timeout (RFC 6298 §2), doubled for each
    /// refusal of it (§5.5), while that is within the step's deadline; past it, as `Stop`.
    Retry,
}

/// When a transaction across groups answers its caller (AC-T1.10.1, lane xgroup-sql-8; the A/B
/// switch for read-your-writes across groups).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AnswerAt {
    /// Today's, the control: once its outcome is decided ([`Event::Answer`]). Its groups apply or
    /// drop its rows as they resolve it afterwards, so a read straight after the answer can miss
    /// its rows in a group that hasn't resolved it yet, and a retry straight after an abort can
    /// run into the transaction's own locks.
    #[default]
    Decided,
    /// Once every group it touched has resolved it ([`Event::Finished`]), aborts included: every
    /// row it wrote is in its group and every lock it held is gone before the caller hears. The
    /// caller waits the resolution's one more round of steps for it.
    Resolved,
}

/// How a [`Cluster`] runs beyond its groups and protocol.
#[derive(Clone, Default)]
pub struct Options {
    pub on_refused: OnRefused,
    pub fault: Option<Fault>,
    pub answer_at: AnswerAt,
}

/// What a [`Cluster`] counts (AC-T1.10.6): refusals by kind, retries, steps and transactions held
/// for a retry (now and at most), coordinator stops, and each answered transaction's time.
#[derive(Default, Clone)]
pub struct Counted {
    pub refused_not_leader: u64,
    pub refused_full: u64,
    pub refused_other: u64,
    pub retries: u64,
    pub held_steps: u64,
    pub held_txns: u64,
    pub held_txns_most: u64,
    pub stops: u64,
    /// Answers decided and held for their resolution ([`AnswerAt::Resolved`]): now, and at most.
    pub answers_held: u64,
    pub answers_held_most: u64,
    /// Commits answered unknown because their token or fast-path stamp couldn't be read
    /// ([`commit_records`], `Cluster::stamped`).
    pub stamps_unread: u64,
    pub latencies: Latencies,
}

impl std::fmt::Debug for Counted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Counted")
            .field("refused_not_leader", &self.refused_not_leader)
            .field("refused_full", &self.refused_full)
            .field("refused_other", &self.refused_other)
            .field("retries", &self.retries)
            .field("held_steps", &self.held_steps)
            .field("held_txns", &self.held_txns)
            .field("held_txns_most", &self.held_txns_most)
            .field("stops", &self.stops)
            .field("answers_held", &self.answers_held)
            .field("answers_held_most", &self.answers_held_most)
            .field("stamps_unread", &self.stamps_unread)
            .field("answered", &self.latencies.count())
            .field("p50_p99_p999", &self.percentiles())
            .finish()
    }
}

impl Counted {
    /// The answered transactions' times at the 50th, 99th and 99.9th percentiles.
    pub fn percentiles(&self) -> [Duration; 3] {
        [500, 990, 999].map(|m| self.latencies.quantile(m))
    }
}

type Instruments = Arc<Mutex<Counted>>;

fn counted(instruments: &Instruments, f: impl FnOnce(&mut Counted)) {
    // A poisoned lock holds counts a panicked thread left; counting goes on over them.
    let mut guard = match instruments.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut guard);
}

/// A refusal that says the group ran nothing of the step.
fn before_taken(refusal: &Refusal) -> bool {
    matches!(refusal, Refusal::NotLeader { .. } | Refusal::Full { .. })
}

enum Input {
    Begin {
        txn: Txn,
        until: Instant,
        answer: SyncSender<Result<Answer, Refusal>>,
    },
    Reply {
        id: u64,
        reply: Result<Reply, Refusal>,
        /// The group's clock once it took the step ([`step_on`]).
        clock: Option<u64>,
    },
    Stop,
}

/// A coordinator over fixed groups, each a [`Database`]: group `i` is `groups[i]`, group 0 the
/// registry. One driver thread owns the [`Coordinator`]; each step runs as one call on a thread of
/// its own, at most `in_flight` transactions' steps at once. Dropped, it stops: what it left is
/// finished by the next [`Cluster::open`] with the same `id`.
pub struct Cluster {
    inputs: SyncSender<Input>,
    driver: Option<JoinHandle<()>>,
    /// The groups, the route and the carried stamp the driver has too: [`Cluster::commit`]'s one
    /// group fast path runs on them without it.
    groups: Arc<Vec<Member>>,
    route: Route,
    carried: Arc<AtomicU64>,
    /// Each group's newest closed time a step on it answered with, by group: only groups on a
    /// node fill theirs ([`closed_at`]); 0, none heard.
    closed: Arc<Vec<AtomicU64>>,
    instruments: Instruments,
}

/// `by` from now; an instant past the clock's range can't be waited for, so it is now.
fn later(by: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(by).unwrap_or(now)
}

/// The steps' observed times, learned as TCP learns a round trip's (RFC 6298 §2, after Jacobson,
/// "Congestion Avoidance and Control", 1988): a smoothed mean and a smoothed mean deviation of each
/// answered step's time from send to reply, and from them the time past which a step's reply is
/// unlikely to come, mean + 4 × deviation. The gains 1/8 and 1/4 and the factor 4 are RFC 6298's;
/// the first sample seeds the mean, and half of it the deviation (§2.2). Only steps answered at
/// their first send are sampled (Karn's rule, §3): a step sent again can't say which send answered.
#[derive(Debug, Default)]
struct StepTimes {
    mean: Option<Duration>,
    deviation: Duration,
}

impl StepTimes {
    fn observe(&mut self, took: Duration) {
        let Some(mean) = self.mean else {
            self.mean = Some(took);
            self.deviation = took.checked_div(2).unwrap_or_default();
            return;
        };
        let off = mean.abs_diff(took);
        let eighth = |d: Duration| d.checked_div(8).unwrap_or_default();
        let quarter = |d: Duration| d.checked_div(4).unwrap_or_default();
        // deviation ← 3/4 deviation + 1/4 |mean − took|; mean ← 7/8 mean + 1/8 took.
        self.deviation = self
            .deviation
            .saturating_sub(quarter(self.deviation))
            .saturating_add(quarter(off));
        self.mean = Some(
            mean.saturating_sub(eighth(mean))
                .saturating_add(eighth(took)),
        );
    }

    /// The time a step is given before its reply counts as lost, doubled for each time the
    /// transaction's resolution already lost one (RFC 6298 §5.5's back-off): none before a sample.
    fn timeout(&self, lost: u32) -> Option<Duration> {
        let mean = self.mean?;
        let base = mean.saturating_add(self.deviation.saturating_mul(4));
        // A back-off past the clock's range stays at the longest wait that fits it.
        Some(
            2u32.checked_pow(lost)
                .and_then(|n| base.checked_mul(n))
                .unwrap_or(Duration::MAX),
        )
    }
}

/// When a transaction's steps must be answered by.
#[derive(Debug, Clone, Copy)]
enum Deadline {
    /// Until the client hears: its caller's own deadline; `lost`, its steps lost so far.
    Caller { until: Instant, lost: u32 },
    /// After it heard, its resolution: each step given the steps' learned timeout, backed off by
    /// the resolution steps lost so far.
    Resolving { lost: u32 },
}

impl Deadline {
    fn lost(self) -> u32 {
        match self {
            Self::Caller { lost, .. } | Self::Resolving { lost } => lost,
        }
    }
}

fn stopped() -> Refusal {
    Refusal::Unavailable(
        "the cross-group coordinator stopped; open it again to finish what it left".into(),
    )
}

/// A step's refusal the protocol may retry: its outcome unknown, or refused before it was taken.
fn retryable(refusal: &Refusal) -> bool {
    matches!(
        refusal,
        Refusal::Timeout
            | Refusal::UnknownOutcome
            | Refusal::NotLeader { .. }
            | Refusal::Full { .. }
    )
}

impl Cluster {
    /// Opens coordinator `id` over `groups` running `protocol`: makes each group's tables, takes a
    /// new incarnation, and finishes what earlier ones left, all by `until`. `in_flight`: the most
    /// transactions it runs at once, the caller's (the threads it has calling [`Cluster::transact`]);
    /// a begin that finds it full waits for one to finish (a caller's last one may still resolve).
    pub fn open(
        groups: Vec<Database>,
        route: Route,
        id: u64,
        protocol: Protocol,
        in_flight: usize,
        until: Instant,
    ) -> Result<Self, Refusal> {
        for group in &groups {
            prepare(group, until)?;
        }
        let groups = groups.into_iter().map(Member::Local).collect();
        Self::open_members(groups, route, id, protocol, in_flight, until)
    }

    /// [`Cluster::open`] over groups each reached as its [`Member`] says: a group on a node made its
    /// tables when its node started serving ([`prepare`]).
    pub fn open_members(
        groups: Vec<Member>,
        route: Route,
        id: u64,
        protocol: Protocol,
        in_flight: usize,
        until: Instant,
    ) -> Result<Self, Refusal> {
        Self::open_with(
            groups,
            route,
            id,
            protocol,
            in_flight,
            Options::default(),
            until,
        )
    }

    /// [`Cluster::open_members`] with `options`: what it does with a refusal before a step was
    /// taken, and the fault seam.
    pub fn open_with(
        groups: Vec<Member>,
        route: Route,
        id: u64,
        protocol: Protocol,
        in_flight: usize,
        options: Options,
        until: Instant,
    ) -> Result<Self, Refusal> {
        let count = GroupId::try_from(groups.len())
            .map_err(|_| amphora_api::invalid("cross-group cluster", "too many groups"))?;
        let config = Config {
            id,
            protocol,
            registry: 0,
            groups: (0..count).collect(),
            in_flight,
        };
        let (coordinator, first) = Coordinator::start(config)?;
        // Every input outstanding at once: a reply per step out (each transaction, recovered ones
        // among them, has at most one step out per group and one more at its anchor) and a begin
        // per caller.
        let room = in_flight
            .checked_add(1)
            .and_then(|n| n.checked_mul(groups.len().checked_add(2)?))
            .ok_or_else(|| amphora_api::invalid("cross-group cluster", "too many steps out"))?;
        let (inputs, receiver) = mpsc::sync_channel(room);
        let (ready, is_ready) = mpsc::sync_channel(1);
        let groups = Arc::new(groups);
        let carried: Arc<AtomicU64> = Arc::default();
        let closed: Arc<Vec<AtomicU64>> =
            Arc::new(groups.iter().map(|_| AtomicU64::new(0)).collect());
        let instruments: Instruments = Arc::default();
        let driver = Driver {
            groups: Arc::clone(&groups),
            route: Arc::clone(&route),
            coordinator,
            inputs: inputs.clone(),
            waiters: BTreeMap::new(),
            parked: VecDeque::new(),
            in_flight,
            deadlines: BTreeMap::new(),
            out: BTreeMap::new(),
            times: StepTimes::default(),
            open_until: until,
            ready: Some(ready),
            carried: Arc::clone(&carried),
            closed: Arc::clone(&closed),
            options,
            held: BTreeMap::new(),
            answers: BTreeMap::new(),
            instruments: Arc::clone(&instruments),
        };
        let driver = std::thread::Builder::new()
            .name("amphora-xgroup".into())
            .spawn(move || driver.run(first, receiver))
            .map_err(|e| {
                Refusal::Unavailable(format!("the cross-group coordinator's thread: {e}"))
            })?;
        let cluster = Self {
            inputs,
            driver: Some(driver),
            groups,
            route,
            carried,
            closed,
            instruments,
        };
        match is_ready.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok(result) => result.map(|()| cluster),
            Err(RecvTimeoutError::Timeout) => Err(Refusal::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(stopped()),
        }
    }

    /// Runs `txn`: committed in every group or aborted in every group, as the answer says.
    /// Unanswered by `until`, its outcome is unknown ([`Refusal::UnknownOutcome`]).
    /// A committed answer carries its commit stamp, fixed by the coordinator from its lock
    /// replies' clocks ([`Coordinator::reply`]).
    pub fn transact(&self, txn: Txn, until: Instant) -> Result<Answer, Refusal> {
        conditioned(&txn)?;
        let began = Instant::now();
        let _touched = self.touched(&txn);
        let (answer, answered) = mpsc::sync_channel(1);
        self.inputs
            .send(Input::Begin { txn, until, answer })
            .map_err(|_| stopped())?;
        let result = match answered.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(Refusal::UnknownOutcome),
            Err(RecvTimeoutError::Disconnected) => Err(stopped()),
        };
        if result.is_ok() {
            counted(&self.instruments, |c| c.latencies.record(began.elapsed()));
        }
        result
    }

    /// The groups, group `i` at `i`: what a database's reads across groups read
    /// ([`Database::across`]).
    pub(crate) fn groups(&self) -> &[Member] {
        &self.groups
    }

    /// The group a routing key's row is in: what a database's reads of one key across groups read
    /// (lane split-merge-19, T-T1.9.R24).
    pub(crate) fn group_of(&self, key: &[u8]) -> GroupId {
        (self.route)(key)
    }

    /// The groups `txn`'s keys are in: the groups the coordinator steps it through.
    fn touched(&self, txn: &Txn) -> BTreeSet<GroupId> {
        txn.reads
            .iter()
            .chain(txn.writes.iter().map(|(key, _)| key))
            .chain(txn.conds.iter().map(|(key, _)| key))
            .map(|key| (self.route)(key))
            .collect()
    }

    /// `answer` with its commit stamp when it committed in one group on the fast path (AC-T1.10.1,
    /// lane xgroup-sql-10): the group's closed time, read once it answered ([`closed_at`]). Read
    /// after the answer, it is at or past the commit in every group that resolved it by then:
    /// every one under [`AnswerAt::Resolved`]. A stamp that can't be read by `until` leaves the
    /// caller no token past the commit's rows, so it hears the outcome as unknown, as a commit
    /// whose answer was lost ([`Counted::stamps_unread`]).
    fn stamped(
        &self,
        answer: Result<Answer, Refusal>,
        touched: &BTreeSet<GroupId>,
        until: Instant,
    ) -> Result<Answer, Refusal> {
        match answer {
            Ok(Answer::Committed { reads, .. }) => {
                match closed_at(&self.groups, &self.closed, touched, until) {
                    Ok(at) => Ok(Answer::Committed { reads, at }),
                    Err(_) => {
                        counted(&self.instruments, |c| {
                            c.stamps_unread = c.stamps_unread.saturating_add(1);
                        });
                        Err(Refusal::UnknownOutcome)
                    }
                }
            }
            other => other,
        }
    }

    /// What it counted so far (AC-T1.10.6).
    pub fn counted(&self) -> Counted {
        let mut out = Counted::default();
        counted(&self.instruments, |c| out = c.clone());
        out
    }
}

impl Cluster {
    /// Commits a SQL session's transaction (AC-T1.10.1, lane xgroup-sql): the rows it read and
    /// wrote, by key. Rows all in one group this process holds commit in that group's one database
    /// transaction, no coordinator step (the fast path); a row another transaction holds a lock on
    /// aborts it, as a lock step would. Rows in several groups, or a group on a node, commit through
    /// the coordinator ([`Cluster::transact`]): in every group or none.
    pub fn commit(&self, txn: Txn, until: Instant) -> Result<Answer, Refusal> {
        // Refused as the coordinator refuses it, whichever path it takes.
        super::coordinator::validate(&txn)?;
        conditioned(&txn)?;
        let touched = self.touched(&txn);
        let one = match (touched.first(), touched.len()) {
            (Some(group), 1) => usize::try_from(*group).ok(),
            _ => None,
        };
        match one.and_then(|i| self.groups.get(i)) {
            Some(Member::Local(group)) => {
                let answer = step(group, &self.carried, until, move |db| {
                    one_group(&mut SqlTablet::new(db), &txn)
                });
                self.stamped(answer, &touched, until)
            }
            _ => self.transact(txn, until),
        }
    }
}

/// Refuses a transaction with a condition on a key-value row (lane split-merge-20, T-T1.9.R27):
/// only a Records row's versions have commit stamps for a condition to name.
fn conditioned(txn: &Txn) -> Result<(), Refusal> {
    for (key, _) in &txn.conds {
        if !matches!(record_of(key), Ok(Some(_))) {
            return Err(amphora_api::invalid(
                "condition",
                "only a Records row has commit stamps to name",
            ));
        }
    }
    Ok(())
}

/// A Records transaction of `namespace` whose rows span groups, committed through `cluster`
/// (AC-T1.10.1, lane xgroup-sql-6): its writes as [`record_key`]s, in every group or none, each
/// group's rows at one revision there; a row written twice keeps its last write, as a single-group
/// commit does. Answers the cross-group timestamp, a hybrid logical clock's (Ada's decision: the
/// commit's revision across groups is the one the session tokens use, [`Token::at`]): the later of
/// the answer's commit stamp and the newest closed time of every group it touched, taken once the
/// commit answered. Under
/// [`AnswerAt::Resolved`] every group has applied its rows by then, so a read that carries the
/// token reads them in any group; under the control a group may not have yet.
/// Its conditions are carried into the lock steps as [`commit_stamp`] carries them.
pub fn commit_records(
    cluster: &Cluster,
    namespace: &str,
    tx: &amphora_api::Tx,
    until: Instant,
) -> Result<Token, Refusal> {
    let (at, touched) = commit_across(cluster, namespace, tx, until)?;
    // The token is the commit stamp, or the newest closed time of the groups it touched read once
    // it answered when that is later: a replicated group's replica may have closed past the stamp
    // before it applied the resolution. No group it touched keeps a clock: its reads see every
    // write, at any token. Unread by `until`, the caller has no token past its rows, so it hears
    // the outcome as unknown, as a commit whose answer was lost (Counted::stamps_unread).
    match closed_at(&cluster.groups, &cluster.closed, &touched, until) {
        Ok(closed) => Ok(Token::at(at.max(closed).unwrap_or(0))),
        Err(_) => {
            counted(&cluster.instruments, |c| {
                c.stamps_unread = c.stamps_unread.saturating_add(1);
            });
            Err(Refusal::UnknownOutcome)
        }
    }
}

/// `Records::commit` of `namespace` across `cluster`'s groups (lane split-merge-22, T-T1.9.R29;
/// reshard.md, "Decision (lane split-merge-21): one clock, one commit stamp"): answers the
/// transaction's one commit stamp, `Answer::Committed`'s `at`, the stamp every row it wrote carries
/// in every group (the coordinator resolves it with `Step::ResolveAt` at that stamp), so the
/// revision it answers is the one reads across groups answer for its rows. A transaction in one
/// group commits there with no coordinator step, and answers that group's clock read once it
/// committed, at or past its rows' stamp. Each condition names a row as a read across groups
/// answered it: [`amphora_api::Cond::At`]'s `rev` is a commit stamp, and the condition holds while
/// the row's newest version is stamped at or before it (`xgroup::holds`), checked under the
/// transaction's locks; [`amphora_api::Cond::Absent`], while the row has none. One that fails is
/// refused `Conflict` on its row and writes nothing. A commit no group gave a stamp for (none keeps
/// the clock) is refused `Unavailable`: reads across groups need every group stamped.
pub(crate) fn commit_stamp(
    cluster: &Cluster,
    namespace: &str,
    tx: &amphora_api::Tx,
    until: Instant,
) -> Result<amphora_api::Rev, Refusal> {
    let (at, _) = commit_across(cluster, namespace, tx, until)?;
    at.ok_or_else(|| {
        Refusal::Unavailable(
            "the transaction across groups committed, but no group it touched keeps the cross-group clock"
                .into(),
        )
    })
}

/// Commits `tx` of `namespace` through `cluster`: its writes and conditions as [`record_key`]s
/// (a row written twice keeps its last write). Answers the commit stamp the coordinator answered,
/// and the groups it touched.
fn commit_across(
    cluster: &Cluster,
    namespace: &str,
    tx: &amphora_api::Tx,
    until: Instant,
) -> Result<(Option<u64>, BTreeSet<GroupId>), Refusal> {
    use amphora_api::{Cond, Write};
    let mut writes = BTreeMap::new();
    for write in &tx.writes {
        let (key, value) = match write {
            Write::Put { family, key, value } => {
                (record_key(namespace, family, key)?, Some(value.clone()))
            }
            Write::Delete { family, key } => (record_key(namespace, family, key)?, None),
        };
        writes.insert(key, value);
    }
    let mut conds = Vec::with_capacity(tx.conds.len());
    for cond in &tx.conds {
        conds.push(match cond {
            Cond::Absent { family, key } => (record_key(namespace, family, key)?, None),
            Cond::At { family, key, rev } => (record_key(namespace, family, key)?, Some(*rev)),
        });
    }
    let txn = Txn {
        reads: Vec::new(),
        writes: writes.into_iter().collect(),
        conds,
    };
    let touched = cluster.touched(&txn);
    let row_refused = |key: &[u8]| match record_of(key) {
        Ok(Some((_, family, row))) => Refusal::Conflict {
            family: family.into(),
            key: row.to_vec(),
            at: None,
        },
        _ => Refusal::Unavailable("a cross-group conflict on a key that isn't a row".into()),
    };
    match cluster.commit(txn, until)? {
        Answer::Committed { at, .. } => Ok((at, touched)),
        Answer::Aborted(Why::Conflict(key) | Why::Unmet(key)) => Err(row_refused(&key)),
        Answer::Aborted(why) => Err(Refusal::Unavailable(format!(
            "the cross-group transaction aborted: {why:?}"
        ))),
        Answer::Unknown => Err(Refusal::UnknownOutcome),
    }
}

/// The commit stamp of a commit in `touched` groups, read after it answered: the newest reading
/// of the one cross-group clock any of them gave ([`Database::cross_clock`], microseconds since the
/// Unix epoch: a replicated group's closed time, so a replica of any of them serves a read that
/// carries it once it has applied everything up to that time; a backed-up group's writer clock,
/// at or past the stamp of every row it wrote; lane split-merge-22). A group that keeps neither
/// (standalone and not backed up: its reads see every write) adds nothing; none of them keeping
/// one, none. A group on a node adds
/// the newest closed time its steps answered with, kept in `closed` (T-T1.10.W4): under
/// [`AnswerAt::Resolved`] its last step resolved the commit there before the answer came, so that
/// time is at or past the commit's rows.
fn closed_at(
    groups: &[Member],
    closed: &[AtomicU64],
    touched: &BTreeSet<GroupId>,
    until: Instant,
) -> Result<Option<u64>, Refusal> {
    let mut newest = None;
    for i in touched
        .iter()
        .filter_map(|group| usize::try_from(*group).ok())
    {
        let heard = match groups.get(i) {
            Some(Member::Local(database)) => database.cross_clock(until)?,
            // 0: nothing heard from it yet.
            Some(Member::Node { .. }) => closed
                .get(i)
                .map(|closed| closed.load(Ordering::Acquire))
                .filter(|micros| *micros > 0),
            None => None,
        };
        newest = newest.max(heard);
    }
    Ok(newest)
}

/// A transaction whose rows are all in this group, in the group's one database transaction:
/// aborted at the first row another transaction holds a lock on, or at the first condition that
/// fails, else its reads as they stood (each key once, in key order) and its writes applied.
fn one_group(tablet: &mut impl Tablet, txn: &Txn) -> anyhow::Result<Answer> {
    for key in txn
        .reads
        .iter()
        .chain(txn.writes.iter().map(|(key, _)| key))
        .chain(txn.conds.iter().map(|(key, _)| key))
    {
        if !tablet.locks(key)?.is_empty() {
            return Ok(Answer::Aborted(Why::Conflict(key.clone())));
        }
    }
    for (key, named) in &txn.conds {
        if !super::holds(*named, tablet.stamp(key)?) {
            return Ok(Answer::Aborted(Why::Unmet(key.clone())));
        }
    }
    let mut reads = BTreeMap::new();
    for key in &txn.reads {
        if !reads.contains_key(key) {
            reads.insert(key.clone(), tablet.value(key)?);
        }
    }
    for (key, value) in &txn.writes {
        tablet.set_value(key, value.as_deref())?;
    }
    Ok(Answer::Committed {
        reads: reads.into_iter().collect(),
        // Stamped by its shell on the fast path ([`Cluster::stamped`]).
        at: None,
    })
}

impl Drop for Cluster {
    fn drop(&mut self) {
        // A driver that already stopped has dropped its receiver; there is nothing to tell it.
        let _ = self.inputs.send(Input::Stop);
        if let Some(driver) = self.driver.take() {
            // A driver thread that panicked has nothing left to finish here; the next open recovers.
            let _ = driver.join();
        }
    }
}

/// Runs one step on `group`, its stamps carried (T-T1.12.X2): the group takes in the latest stamp
/// the coordinator heard of before its commit, and answers with its own latest after it. Groups not
/// backed up or replicated here make no stamps and carry none. The stamps are the cross-group
/// clock's, in microseconds (lane split-merge-21, T-T1.9.R28), the unit of the commit stamps and
/// the tokens: a backed-up group's writer clock, a replicated group's closed time
/// (`Database::observe_clock`, `Database::cross_clock`).
fn step<R, F>(group: &Database, carried: &AtomicU64, until: Instant, f: F) -> Result<R, Refusal>
where
    R: Send + 'static,
    F: FnOnce(&mut Db) -> anyhow::Result<R> + Send + 'static,
{
    group.observe_clock(until, carried.load(Ordering::Acquire))?;
    let reply = group.call_internal(until, f)?;
    if let Some(latest) = group.cross_clock(until)? {
        carried.fetch_max(latest, Ordering::AcqRel);
    }
    Ok(reply)
}

/// A step held to be sent again: its group, transaction, envelope and the refusals of it so far.
type Held = (GroupId, Option<TxnId>, Envelope, u32);

/// Where a caller waits for its transaction's answer.
type Waiter = SyncSender<Result<Answer, Refusal>>;

/// A step out: its transaction, when it was sent, whether it is a first send, and, kept only under
/// [`OnRefused::Retry`], its group, envelope and the refusals of it so far, to send it again.
struct Out {
    txn: Option<TxnId>,
    at: Instant,
    first: bool,
    again: Option<(GroupId, Envelope, u32)>,
}

struct Driver {
    groups: Arc<Vec<Member>>,
    route: Route,
    coordinator: Coordinator,
    inputs: SyncSender<Input>,
    waiters: BTreeMap<TxnId, SyncSender<Result<Answer, Refusal>>>,
    /// Begins that found the coordinator full, begun in arrival order as transactions finish (a
    /// caller's next transaction may arrive while its last one still resolves): at most
    /// `in_flight`, the callers the shell was opened for; a begin past that is refused `Full`.
    parked: VecDeque<(Txn, Instant, Waiter)>,
    in_flight: usize,
    /// Each running transaction's deadline.
    deadlines: BTreeMap<TxnId, Deadline>,
    /// Each step out: its transaction, when it was sent, and whether it is a first send (only those
    /// are sampled, Karn's rule). At most the steps the coordinator has out.
    out: BTreeMap<u64, Out>,
    times: StepTimes,
    open_until: Instant,
    ready: Option<SyncSender<Result<(), Refusal>>>,
    /// The latest commit stamp any step's group answered with, carried into every step sent after
    /// (T-T1.12.X2, docs/design/backup.md, "Cuts across machines"): a group a step reaches stamps
    /// its commit after every commit the coordinator heard of before sending it.
    carried: Arc<AtomicU64>,
    /// Each group's newest closed time a step on it answered with ([`Cluster`]'s, shared).
    closed: Arc<Vec<AtomicU64>>,
    options: Options,
    /// Steps a group refused before taking, each to be sent again at its instant
    /// ([`OnRefused::Retry`]): its group, transaction, envelope and refusals so far. At most the
    /// steps the coordinator has out.
    held: BTreeMap<(Instant, u64), Held>,
    /// Each decided transaction's answer, held for its caller until every group it touched has
    /// resolved it ([`AnswerAt::Resolved`]). At most the transactions the coordinator runs.
    answers: BTreeMap<TxnId, Answer>,
    instruments: Instruments,
}

impl Driver {
    fn run(mut self, first: Vec<Event>, receiver: Receiver<Input>) {
        let mut events = first;
        loop {
            if let Err(refusal) = self.handle(events) {
                self.stop(&refusal);
                return;
            }
            // Each transaction finished makes room for a parked begin.
            events = self.admit_parked();
            if !events.is_empty() {
                continue;
            }
            let input = match self.held.first_key_value() {
                Some(((at, _), _)) => {
                    receiver.recv_timeout(at.saturating_duration_since(Instant::now()))
                }
                None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            events = match input {
                Err(RecvTimeoutError::Timeout) => {
                    if let Err(refusal) = self.send_held() {
                        self.stop(&refusal);
                        return;
                    }
                    Vec::new()
                }
                Ok(Input::Begin { txn, until, answer }) => {
                    // Parked begins went first while there was room: one arriving to a full
                    // coordinator waits behind them.
                    if self.coordinator.ready() && self.coordinator.in_flight() >= self.in_flight {
                        if self.parked.len() < self.in_flight {
                            self.parked.push_back((txn, until, answer));
                        } else {
                            // A caller gone by now needs no answer.
                            let _ = answer.send(Err(Refusal::Full {
                                limit: self.in_flight,
                            }));
                        }
                        Vec::new()
                    } else {
                        self.begin(txn, until, answer)
                    }
                }
                Ok(Input::Reply { id, reply, clock }) => {
                    let (txn, sent_at, first, again) = match self.out.remove(&id) {
                        Some(out) => (out.txn, Some(out.at), out.first, out.again),
                        None => (None, None, false, None),
                    };
                    if let Err(refusal) = &reply {
                        counted(&self.instruments, |c| match refusal {
                            Refusal::NotLeader { .. } => {
                                c.refused_not_leader = c.refused_not_leader.saturating_add(1);
                            }
                            Refusal::Full { .. } => {
                                c.refused_full = c.refused_full.saturating_add(1)
                            }
                            _ => c.refused_other = c.refused_other.saturating_add(1),
                        });
                    }
                    if let (Err(refusal), Some((group, envelope, refusals))) = (&reply, again)
                        && before_taken(refusal)
                    {
                        // A refusal is a reply: answered at a first send, it times the round trip.
                        if let (Some(at), true) = (sent_at, first) {
                            self.times.observe(at.elapsed());
                        }
                        if self.hold(id, group, txn, envelope, refusals) {
                            continue;
                        }
                    }
                    let reply = match reply {
                        Ok(reply) => {
                            if let (Some(at), true) = (sent_at, first) {
                                self.times.observe(at.elapsed());
                            }
                            Ok(reply)
                        }
                        Err(refusal) if retryable(&refusal) => {
                            match txn.and_then(|txn| self.deadlines.get_mut(&txn)) {
                                Some(Deadline::Caller { lost, .. }) => {
                                    *lost = lost.saturating_add(1);
                                }
                                Some(Deadline::Resolving { lost }) => {
                                    // The back-off doubles while the doubled wait still fits the
                                    // clock's range; past it, it stays at the last that fit.
                                    let more = lost.saturating_add(1);
                                    let fits = self
                                        .times
                                        .timeout(more)
                                        .and_then(|t| Instant::now().checked_add(t))
                                        .is_some();
                                    if fits {
                                        *lost = more;
                                    }
                                }
                                None => {}
                            }
                            Err(Lost)
                        }
                        Err(refusal) => {
                            self.stop(&refusal);
                            return;
                        }
                    };
                    match self.coordinator.reply(id, reply, clock) {
                        Ok(events) => events,
                        Err(refusal) => {
                            self.stop(&refusal);
                            return;
                        }
                    }
                }
                Ok(Input::Stop) | Err(RecvTimeoutError::Disconnected) => {
                    self.stop(&stopped());
                    return;
                }
            };
        }
    }

    fn handle(&mut self, events: Vec<Event>) -> Result<(), Refusal> {
        for event in events {
            match event {
                Event::Send(send) => self.send(send.id, send.group, send.txn, send.envelope, 0)?,
                Event::Answer { txn, answer, .. } => {
                    // An unknown outcome has no resolution of this incarnation's to wait for.
                    let hold = self.options.answer_at == AnswerAt::Resolved
                        && !matches!(answer, Answer::Unknown)
                        && self.waiters.contains_key(&txn);
                    if hold {
                        self.answers.insert(txn, answer);
                        self.count_answers();
                    } else {
                        self.answer(txn, answer);
                    }
                    // Without a step timed yet (every step of it lost), the caller's deadline holds.
                    if let Some(deadline) = self.deadlines.get_mut(&txn)
                        && self.times.timeout(0).is_some()
                    {
                        *deadline = Deadline::Resolving { lost: 0 };
                    }
                }
                Event::Finished { txn, .. } => {
                    self.deadlines.remove(&txn);
                    if let Some(answer) = self.answers.remove(&txn) {
                        self.count_answers();
                        self.answer(txn, answer);
                    }
                }
                Event::Ready => {
                    if let Some(ready) = self.ready.take() {
                        let _ = ready.send(Ok(()));
                    }
                }
                Event::Fenced => {
                    return Err(Refusal::Fenced(
                        "a newer incarnation of this coordinator took over".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Answers `txn`'s caller, if it still waits.
    fn answer(&mut self, txn: TxnId, answer: Answer) {
        if let Some(waiter) = self.waiters.remove(&txn) {
            // A caller gone by now needs no answer.
            let _ = waiter.send(Ok(answer));
        }
    }

    fn count_answers(&self) {
        let held = u64::try_from(self.answers.len()).unwrap_or(u64::MAX);
        counted(&self.instruments, |c| {
            c.answers_held = held;
            c.answers_held_most = c.answers_held_most.max(held);
        });
    }

    /// When step of `txn` must be answered by, and whether it may be sampled.
    fn due(&self, txn: Option<TxnId>) -> (Instant, bool) {
        let deadline = txn.and_then(|txn| self.deadlines.get(&txn)).copied();
        let until = match deadline {
            Some(Deadline::Caller { until, .. }) => until,
            // Resolving only once a step was timed: there is a sample.
            Some(Deadline::Resolving { lost }) => {
                self.times.timeout(lost).map_or(self.open_until, later)
            }
            None => self.open_until,
        };
        // After one of its steps was lost, a transaction's sends may be resends: not sampled
        // (Karn's rule).
        (until, deadline.is_none_or(|d| d.lost() == 0))
    }

    /// Sends step `id` to group `here` on a thread of its own; `refusals`: the times a group
    /// refused it before taking it.
    fn send(
        &mut self,
        id: u64,
        here: GroupId,
        txn: Option<TxnId>,
        envelope: Envelope,
        refusals: u32,
    ) -> Result<(), Refusal> {
        let (until, first) = self.due(txn);
        let first = first && refusals == 0;
        if Instant::now() >= until {
            // A step that found no answer within its budget: this incarnation stops, as a crash
            // would, and the next one's recovery finishes the transaction.
            return Err(Refusal::Unavailable(
                "a cross-group step found no answer within its budget; open the coordinator again to finish it".into(),
            ));
        }
        let index = usize::try_from(here)
            .map_err(|_| amphora_api::invalid("cross-group step", "group number"))?;
        let group = self
            .groups
            .get(index)
            .cloned()
            .ok_or_else(|| amphora_api::invalid("cross-group step", "no such group"))?;
        let inputs = self.inputs.clone();
        let carried = Arc::clone(&self.carried);
        let closed = Arc::clone(&self.closed);
        let fault = self.options.fault.clone();
        let again = (self.options.on_refused == OnRefused::Retry)
            .then(|| (here, envelope.clone(), refusals));
        self.out.insert(
            id,
            Out {
                txn,
                at: Instant::now(),
                first,
                again,
            },
        );
        std::thread::Builder::new()
            .name("amphora-xgroup-step".into())
            .spawn(move || {
                let (reply, clock) = match fault.and_then(|fault| fault(here, &envelope)) {
                    Some(refusal) => (Err(refusal), None),
                    None => {
                        match step_on(&group, &carried, closed.get(index), until, here, envelope) {
                            Ok((reply, clock)) => (Ok(reply), clock),
                            Err(refusal) => (Err(refusal), None),
                        }
                    }
                };
                // A driver gone by now has stopped: its successor recovers the step.
                let _ = inputs.send(Input::Reply { id, reply, clock });
            })
            .map_err(|e| Refusal::Unavailable(format!("a cross-group step's thread: {e}")))?;
        Ok(())
    }

    /// Holds step `id`, refused before it was taken, to send again after the learned timeout
    /// backed off by its refusals, if that comes before its deadline: `false` when it doesn't (or
    /// nothing is learned yet), and the refusal goes on as today's.
    fn hold(
        &mut self,
        id: u64,
        group: GroupId,
        txn: Option<TxnId>,
        envelope: Envelope,
        refusals: u32,
    ) -> bool {
        let (until, _) = self.due(txn);
        let at = self
            .times
            .timeout(refusals)
            .and_then(|wait| Instant::now().checked_add(wait));
        let Some(at) = at.filter(|at| *at < until) else {
            return false;
        };
        self.held
            .insert((at, id), (group, txn, envelope, refusals.saturating_add(1)));
        self.count_held(true);
        true
    }

    /// Sends again every held step whose instant came.
    fn send_held(&mut self) -> Result<(), Refusal> {
        let now = Instant::now();
        while let Some(entry) = self.held.first_entry() {
            if entry.key().0 > now {
                break;
            }
            let ((_, id), (group, txn, envelope, refusals)) = entry.remove_entry();
            self.count_held(false);
            self.send(id, group, txn, envelope, refusals)?;
        }
        Ok(())
    }

    fn count_held(&self, retry: bool) {
        let steps = u64::try_from(self.held.len()).unwrap_or(u64::MAX);
        let txns: std::collections::BTreeSet<_> = self
            .held
            .values()
            .filter_map(|(_, txn, _, _)| *txn)
            .collect();
        let txns = u64::try_from(txns.len()).unwrap_or(u64::MAX);
        counted(&self.instruments, |c| {
            c.retries = c.retries.saturating_add(u64::from(retry));
            c.held_steps = steps;
            c.held_txns = txns;
            c.held_txns_most = c.held_txns_most.max(txns);
        });
    }

    /// Begins `txn` for the caller waiting on `answer`, its steps due by `until`.
    fn begin(
        &mut self,
        txn: Txn,
        until: Instant,
        answer: SyncSender<Result<Answer, Refusal>>,
    ) -> Vec<Event> {
        match self.coordinator.begin(txn, |key| (self.route)(key)) {
            Ok((id, events)) => {
                self.waiters.insert(id, answer);
                self.deadlines
                    .insert(id, Deadline::Caller { until, lost: 0 });
                events
            }
            Err(refusal) => {
                // A caller gone by now needs no answer.
                let _ = answer.send(Err(refusal));
                Vec::new()
            }
        }
    }

    /// Parked begins, oldest first, while the coordinator has room; one whose caller's deadline
    /// passed while it waited is answered `Timeout` and never begun.
    fn admit_parked(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        while self.coordinator.in_flight() < self.in_flight
            && let Some((txn, until, answer)) = self.parked.pop_front()
        {
            if Instant::now() >= until {
                // A caller gone by now needs no answer.
                let _ = answer.send(Err(Refusal::Timeout));
                continue;
            }
            events.extend(self.begin(txn, until, answer));
        }
        events
    }

    fn stop(&mut self, refusal: &Refusal) {
        counted(&self.instruments, |c| c.stops = c.stops.saturating_add(1));
        // A held answer's resolution is the next incarnation's to finish, so its rows may not be
        // readable yet: its caller hears the outcome is unknown to it, never "stopped", which a
        // caller may take for "not done" and run a committed transaction again.
        for txn in std::mem::take(&mut self.answers).into_keys() {
            if let Some(waiter) = self.waiters.remove(&txn) {
                let _ = waiter.send(Err(Refusal::UnknownOutcome));
            }
        }
        self.count_answers();
        for (_, waiter) in std::mem::take(&mut self.waiters) {
            let _ = waiter.send(Err(refusal.clone()));
        }
        for (_, _, waiter) in std::mem::take(&mut self.parked) {
            let _ = waiter.send(Err(refusal.clone()));
        }
        if let Some(ready) = self.ready.take() {
            let _ = ready.send(Err(refusal.clone()));
        }
    }
}

#[cfg(test)]
mod step_times {
    use super::StepTimes;
    use std::time::Duration;

    #[test]
    fn cp_t1_10_the_resolution_timeout_is_learned_from_the_steps_times() {
        let ms = Duration::from_millis;
        let mut times = StepTimes::default();
        assert_eq!(times.timeout(0), None, "no timeout before a step was timed");
        // The first sample seeds the mean and half of it the deviation: 10 + 4 × 5.
        times.observe(ms(10));
        assert_eq!(times.timeout(0), Some(ms(30)));
        // Steady steps shrink the deviation toward none: the timeout falls toward the mean.
        for _ in 0..200 {
            times.observe(ms(10));
        }
        let steady = times.timeout(0).unwrap_or(Duration::MAX);
        assert!(steady >= ms(10) && steady < ms(11), "steady: {steady:?}");
        // Slower steps raise it: it follows where the steps run, not a number given it.
        for _ in 0..200 {
            times.observe(ms(40));
        }
        let slower = times.timeout(0).unwrap_or_default();
        assert!(slower >= ms(40), "slower: {slower:?}");
        // Each resolution step lost doubles it (RFC 6298 §5.5), and an overflow stays the longest.
        assert_eq!(times.timeout(2), slower.checked_mul(4));
        assert_eq!(times.timeout(200), Some(Duration::MAX));
    }
}

/// AC-T1.10.1 (lane xgroup-sql): a SQL session's transactions across groups.
#[cfg(test)]
mod sessions {
    use super::*;

    fn until() -> Instant {
        Instant::now() + crate::transport::HANG
    }

    /// A seeded generator (splitmix64), so every run makes the same transactions.
    struct Seeded(u64);

    impl Seeded {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    fn txns(seed: u64, count: usize, keys: u64) -> Vec<Txn> {
        let mut rng = Seeded(seed);
        (0..count)
            .map(|i| {
                let key = |rng: &mut Seeded| vec![u8::try_from(rng.below(keys)).unwrap()];
                let mut reads: Vec<Vec<u8>> = (0..=rng.below(3)).map(|_| key(&mut rng)).collect();
                reads.sort();
                reads.dedup();
                let mut writes: Vec<(Vec<u8>, Option<Vec<u8>>)> = Vec::new();
                for _ in 0..=rng.below(3) {
                    let k = key(&mut rng);
                    if writes.iter().all(|(w, _)| *w != k) {
                        let value = (rng.below(4) != 0).then(|| i.to_be_bytes().to_vec());
                        writes.push((k, value));
                    }
                }
                Txn {
                    reads,
                    writes,
                    conds: Vec::new(),
                }
            })
            .collect()
    }

    /// Commits `txn`, again while another transaction's locks are still being resolved (as a SQL
    /// client retries a conflict), by `until`.
    fn commit(cluster: &Cluster, txn: &Txn) -> Answer {
        let deadline = until();
        loop {
            let answer = cluster.commit(txn.clone(), deadline).unwrap();
            if !matches!(answer, Answer::Aborted(Why::Conflict(_))) || Instant::now() >= deadline {
                return in_key_order(answer);
            }
        }
    }

    /// A SQL answer's rows are a set: the coordinator answers reads as their groups reply. Its
    /// commit stamp is a clock's, so two clusters' answers compare without it.
    fn in_key_order(answer: Answer) -> Answer {
        match answer {
            Answer::Committed { mut reads, .. } => {
                reads.sort();
                reads.dedup();
                Answer::Committed { reads, at: None }
            }
            other => other,
        }
    }

    /// Routes by the row's first byte: a Records row's own key, or a key of the key-value space.
    fn route_of(groups: u32) -> Route {
        Arc::new(move |key: &[u8]| {
            let row = record_of(key).ok().flatten().map_or(key, |(_, _, row)| row);
            row.first().map_or(0, |b| GroupId::from(*b) % groups)
        })
    }

    /// Records row `k` of namespace `ns`'s family `rows`, as a transaction's key.
    fn row(k: u8) -> Vec<u8> {
        record_key("ns", "rows", &[k]).unwrap()
    }

    fn open(dir: &std::path::Path, groups: u32, id: u64, protocol: Protocol) -> Cluster {
        let databases = (0..groups)
            .map(|g| Database::open(&dir.join(format!("group{g}.db")), until()).unwrap())
            .collect();
        let route = route_of(groups);
        Cluster::open(databases, route, id, protocol, 1, until()).unwrap()
    }

    #[test]
    fn ac_t1_10_1_a_sessions_answers_across_groups_equal_the_same_on_one_group() {
        for (seed, protocol) in [(1, Protocol::TwoPhase), (2, Protocol::Parallel)] {
            let spread_dir = tempfile::tempdir().unwrap();
            let one_dir = tempfile::tempdir().unwrap();
            let spread = open(spread_dir.path(), 3, 1, protocol);
            let one = open(one_dir.path(), 1, 1, protocol);
            let keys = 6;
            let mut across = 0;
            for txn in txns(seed, 40, keys) {
                let groups: BTreeSet<u8> = txn
                    .reads
                    .iter()
                    .chain(txn.writes.iter().map(|(k, _)| k))
                    .map(|k| k[0] % 3)
                    .collect();
                across += usize::from(groups.len() > 1);
                assert_eq!(
                    commit(&spread, &txn),
                    commit(&one, &txn),
                    "{protocol:?} {txn:?}"
                );
            }
            assert!(across > 0, "{protocol:?}: no transaction spanned groups");
            let all = Txn {
                reads: (0..keys).map(|k| vec![u8::try_from(k).unwrap()]).collect(),
                writes: Vec::new(),
                conds: Vec::new(),
            };
            assert_eq!(commit(&spread, &all), commit(&one, &all), "{protocol:?}");
        }
    }

    fn databases(dir: &std::path::Path, groups: u32) -> Vec<Database> {
        (0..groups)
            .map(|g| Database::open(&dir.join(format!("group{g}.db")), until()).unwrap())
            .collect()
    }

    fn open_over(databases: &[Database], id: u64, protocol: Protocol, options: Options) -> Cluster {
        let groups = u32::try_from(databases.len()).unwrap();
        for group in databases {
            prepare(group, until()).unwrap();
        }
        let route = route_of(groups);
        let members = databases.iter().cloned().map(Member::Local).collect();
        Cluster::open_with(members, route, id, protocol, 1, options, until()).unwrap()
    }

    /// A fault that refuses each listed group's first `times` steps as `refusal` makes it.
    fn refusing(groups: &[GroupId], times: u64, refusal: fn() -> Refusal) -> Fault {
        let calls: Arc<Vec<AtomicU64>> = Arc::new((0..3).map(|_| AtomicU64::new(0)).collect());
        let groups = groups.to_vec();
        // Only the steps of transactions this incarnation began: an open's recovery scans, and its
        // finishing of what the last incarnation left (whose resolution may still have been under
        // way when it stopped), aren't the scenario's and would spend its refusals.
        Arc::new(move |here, envelope: &Envelope| {
            let calls = calls.get(usize::try_from(here).ok()?)?;
            (envelope
                .step
                .txn()
                .is_some_and(|txn| txn.epoch == envelope.epoch)
                && groups.contains(&here)
                && calls.fetch_add(1, Ordering::AcqRel) < times)
                .then(refusal)
        })
    }

    /// Records rows 0, 1 and 2, one in each of three groups.
    fn across(i: u8) -> Txn {
        Txn {
            reads: vec![row(0)],
            writes: (0..3).map(|k| (row(k), Some(vec![i]))).collect(),
            conds: Vec::new(),
        }
    }

    /// AC-T1.10.6: one group that keeps refusing before it takes a step (its leader moving), and
    /// many refusing at once (a fleet restarting): retried, the transaction commits with no stop;
    /// under today's control, a lock step's refusal counts as lost and aborts it.
    #[test]
    fn ac_t1_10_6_refusals_before_a_step_retried_commit_and_the_control_aborts() {
        let not_leader = || Refusal::NotLeader { leader: None };
        let full = || Refusal::Full { limit: 1 };
        for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
            for (name, groups, times, refusal) in [
                (
                    "one keeps refusing",
                    vec![1],
                    3,
                    not_leader as fn() -> Refusal,
                ),
                ("many at once", vec![0, 1, 2], 2, full),
            ] {
                for on_refused in [OnRefused::Stop, OnRefused::Retry] {
                    let dir = tempfile::tempdir().unwrap();
                    let dbs = databases(dir.path(), 3);
                    // Every group answers once first, so the steps' times have a sample.
                    let warm = open_over(&dbs, 1, protocol, Options::default());
                    assert!(matches!(
                        commit(&warm, &across(1)),
                        Answer::Committed { .. }
                    ));
                    drop(warm);
                    let options = Options {
                        on_refused,
                        fault: Some(refusing(&groups, times, refusal)),
                        ..Options::default()
                    };
                    let cluster = open_over(&dbs, 1, protocol, options);
                    let answer = cluster.commit(across(2), until());
                    let counted = cluster.counted();
                    let refused = counted.refused_not_leader + counted.refused_full;
                    assert!(
                        refused > 0,
                        "{protocol:?} {name} {on_refused:?}: {counted:?}"
                    );
                    match on_refused {
                        OnRefused::Retry => {
                            assert!(
                                matches!(answer, Ok(Answer::Committed { .. })),
                                "{protocol:?} {name}: {answer:?} {counted:?}"
                            );
                            assert!(counted.retries >= times, "{protocol:?} {name}: {counted:?}");
                            assert_eq!(counted.stops, 0, "{protocol:?} {name}: {counted:?}");
                            assert!(counted.held_txns_most >= 1, "{counted:?}");
                            assert_eq!(counted.latencies.count(), 1, "{counted:?}");
                        }
                        OnRefused::Stop => {
                            assert!(
                                !matches!(answer, Ok(Answer::Committed { .. })),
                                "{protocol:?} {name}: the control committed: {counted:?}"
                            );
                            assert_eq!(counted.retries, 0, "{counted:?}");
                        }
                    }
                }
            }
        }
    }

    /// AC-T1.10.1: a group failing mid-commit of a session's transaction over three real groups, at
    /// each of its steps in turn (the coordinator stops, as a crash would), leaves the transaction's
    /// rows all written or none once the next incarnation recovers, under both protocols; and the
    /// rows then read as on one group that ran the transaction or didn't.
    #[test]
    fn ac_t1_10_1_a_group_failure_mid_commit_through_a_session_is_all_or_nothing() {
        for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
            let (mut committed, mut aborted) = (0, 0);
            for group in 0..3 {
                for at in 0..10u64 {
                    let dir = tempfile::tempdir().unwrap();
                    let dbs = databases(dir.path(), 3);
                    let calls = Arc::new(AtomicU64::new(0));
                    let seen = Arc::clone(&calls);
                    let fault: Fault = Arc::new(move |here, envelope: &Envelope| {
                        (envelope.step.txn().is_some()
                            && here == group
                            && seen.fetch_add(1, Ordering::AcqRel) == at)
                            .then(|| Refusal::Unavailable("the group's quorum is lost".into()))
                    });
                    let options = Options {
                        on_refused: OnRefused::Stop,
                        fault: Some(fault),
                        ..Options::default()
                    };
                    let failing = open_over(&dbs, 1, protocol, options);
                    // The caller hears the failure or the answer; the rows decide.
                    let _ = failing.commit(across(7), until());
                    drop(failing);
                    let recovered = open_over(&dbs, 1, protocol, Options::default());
                    let all = Txn {
                        reads: (0..3).map(row).collect(),
                        writes: Vec::new(),
                        conds: Vec::new(),
                    };
                    let got = commit(&recovered, &all);
                    let reads = match &got {
                        Answer::Committed { reads, .. } => reads.clone(),
                        Answer::Aborted(_) | Answer::Unknown => Vec::new(),
                    };
                    let written = reads
                        .iter()
                        .filter(|(_, v)| v.as_deref() == Some(&[7][..]))
                        .count();
                    assert!(
                        written == 0 || written == 3,
                        "{protocol:?} group {group} at {at}: a partial commit {reads:?}"
                    );
                    let one_dir = tempfile::tempdir().unwrap();
                    let one = open(one_dir.path(), 1, 1, protocol);
                    if written == 3 {
                        committed += 1;
                        commit(&one, &across(7));
                    } else {
                        aborted += 1;
                    }
                    assert_eq!(
                        commit(&one, &all),
                        got,
                        "{protocol:?} group {group} at {at}"
                    );
                }
            }
            // Non-vacuity: the failures found both ends.
            assert!(
                committed > 0 && aborted > 0,
                "{protocol:?}: {committed} committed, {aborted} aborted"
            );
        }
    }

    /// AC-T1.10.1 (lane xgroup-sql-6): seeded transactions over Records rows in three groups,
    /// under both protocols: every row reads back through Records' get, scan and feed in its own
    /// group as the committed transactions left it, and every answer equals a one-group run's.
    #[test]
    fn ac_t1_10_1_a_cross_group_commit_reads_back_through_records() {
        use amphora_api::{Consistency, Records, Scan};
        for (seed, protocol) in [(3, Protocol::TwoPhase), (4, Protocol::Parallel)] {
            let dir = tempfile::tempdir().unwrap();
            let dbs = databases(dir.path(), 3);
            let spread = open_over(&dbs, 1, protocol, Options::default());
            let one_dir = tempfile::tempdir().unwrap();
            let one = open_over(
                &databases(one_dir.path(), 1),
                1,
                protocol,
                Options::default(),
            );
            let keys = 6u8;
            let mut model: BTreeMap<u8, Option<Vec<u8>>> = BTreeMap::new();
            let mut across = 0;
            for raw in txns(seed, 30, u64::from(keys)) {
                let txn = Txn {
                    reads: raw.reads.iter().map(|k| row(k[0])).collect(),
                    writes: raw
                        .writes
                        .iter()
                        .map(|(k, v)| (row(k[0]), v.clone()))
                        .collect(),
                    conds: Vec::new(),
                };
                let groups: BTreeSet<u8> = raw
                    .reads
                    .iter()
                    .chain(raw.writes.iter().map(|(k, _)| k))
                    .map(|k| k[0] % 3)
                    .collect();
                across += usize::from(groups.len() > 1);
                let answer = commit(&spread, &txn);
                assert_eq!(answer, commit(&one, &txn), "{protocol:?} {raw:?}");
                if matches!(answer, Answer::Committed { .. }) {
                    for (k, v) in &raw.writes {
                        model.insert(k[0], v.clone());
                    }
                }
            }
            assert!(across > 0, "{protocol:?}: no transaction spanned groups");
            // A commit answers once decided; its groups apply its rows as they resolve it. Reading
            // every row through the coordinator, again while a lock is still held, returns once
            // every earlier transaction is resolved in every group.
            let all = Txn {
                reads: (0..keys).map(row).collect(),
                writes: Vec::new(),
                conds: Vec::new(),
            };
            commit(&spread, &all);
            for g in 0..3u8 {
                let ns = dbs[usize::from(g)].namespace("ns").unwrap();
                let held: BTreeMap<u8, Option<Vec<u8>>> = model
                    .iter()
                    .filter(|(k, _)| *k % 3 == g)
                    .map(|(k, v)| (*k, v.clone()))
                    .collect();
                for k in (0..keys).filter(|k| k % 3 == g) {
                    let got = ns
                        .get("rows", &[k], Consistency::Linearizable, until())
                        .unwrap()
                        .map(|r| r.value);
                    assert_eq!(got, held.get(&k).cloned().flatten(), "{protocol:?} get {k}");
                }
                let scan = Scan {
                    prefix: Vec::new(),
                    after: None,
                    limit: 100,
                    at: Consistency::Linearizable,
                };
                let scanned: Vec<(u8, Vec<u8>)> = ns
                    .scan("rows", scan, until())
                    .unwrap()
                    .rows
                    .into_iter()
                    .map(|r| (r.key[0], r.value))
                    .collect();
                let live: Vec<(u8, Vec<u8>)> = held
                    .iter()
                    .filter_map(|(k, v)| Some((*k, v.clone()?)))
                    .collect();
                assert_eq!(scanned, live, "{protocol:?} scan of group {g}");
                // The feed's last change of each row is what the row holds; revisions never fall.
                let mut last = BTreeMap::new();
                let mut newest = 0;
                for change in ns.changes("rows", 0, 1000, until()).unwrap().changes {
                    assert!(change.rev >= newest, "{protocol:?} feed of group {g}");
                    newest = change.rev;
                    last.insert(change.key[0], change.value);
                }
                assert_eq!(last, held, "{protocol:?} feed of group {g}");
            }
        }
    }

    /// AC-T1.10.1 (lane xgroup-sql-6): a single-group Records commit on a row a transaction across
    /// groups holds a lock on, a read's or a write's, is refused as a conflict; once the lock is
    /// resolved it commits.
    #[test]
    fn ac_t1_10_1_a_single_group_commit_respects_a_cross_group_lock() {
        use amphora_api::{Records, Tx, Write};
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("group.db"), until()).unwrap();
        let ns = db.namespace("ns").unwrap();
        let put = |v: u8| Tx {
            conds: Vec::new(),
            writes: vec![Write::Put {
                family: "rows".into(),
                key: vec![1],
                value: vec![v],
            }],
        };
        ns.commit(put(1), until()).unwrap();
        let txn = TxnId {
            coordinator: 9,
            epoch: 1,
            seq: 1,
        };
        for intent in [Intent::Read, Intent::Put(vec![2])] {
            let lock = Lock {
                txn,
                anchor: 0,
                intent,
            };
            let key = row(1);
            db.call_internal(until(), move |db| SqlTablet::new(db).put_lock(&key, &lock))
                .unwrap();
            let refused = ns.commit(put(3), until());
            assert!(
                matches!(refused, Err(Refusal::Conflict { .. })),
                "{refused:?}"
            );
            db.call_internal(until(), move |db| SqlTablet::new(db).drop_locks_of(txn))
                .unwrap();
            assert!(ns.commit(put(3), until()).is_ok());
        }
    }

    /// AC-T1.10.1 (lanes xgroup-sql-6 and xgroup-sql-8): a Records transaction across three groups
    /// through [`commit_records`], both protocols, answered once resolved: its rows read back in
    /// each group straight after the answer, the answer is a cross-group timestamp, a row under
    /// another transaction's lock is refused as a conflict, and so is a condition that fails, its
    /// rows unwritten.
    #[test]
    fn ac_t1_10_1_commit_records_writes_rows_in_every_group() {
        use amphora_api::{Cond, Consistency, Records, Tx, Write};
        for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
            let dir = tempfile::tempdir().unwrap();
            let dbs = databases(dir.path(), 3);
            let options = Options {
                answer_at: AnswerAt::Resolved,
                ..Options::default()
            };
            let cluster = open_over(&dbs, 1, protocol, options);
            let tx = |v: u8| Tx {
                conds: Vec::new(),
                writes: (0..3u8)
                    .map(|k| Write::Put {
                        family: "rows".into(),
                        key: vec![k],
                        value: vec![v],
                    })
                    .collect(),
            };
            let token = commit_records(&cluster, "ns", &tx(5), until()).unwrap();
            assert_eq!(token.index, 0, "{protocol:?}");
            for k in 0..3u8 {
                let ns = dbs[usize::from(k)].namespace("ns").unwrap();
                let got = ns
                    .get("rows", &[k], Consistency::Linearizable, until())
                    .unwrap();
                assert_eq!(got.map(|r| r.value), Some(vec![5]), "{protocol:?} row {k}");
            }
            let lock = Lock {
                txn: TxnId {
                    coordinator: 9,
                    epoch: 1,
                    seq: 1,
                },
                anchor: 2,
                intent: Intent::Read,
            };
            let key = row(2);
            dbs[2]
                .call_internal(until(), move |db| SqlTablet::new(db).put_lock(&key, &lock))
                .unwrap();
            let refused = commit_records(&cluster, "ns", &tx(6), until());
            assert!(
                matches!(refused, Err(Refusal::Conflict { .. })),
                "{protocol:?} {refused:?}"
            );
            // Clear of the held row 2: the condition is what refuses it (lane split-merge-22).
            let mut conditional = tx(7);
            conditional.writes.truncate(2);
            conditional.conds.push(Cond::Absent {
                family: "rows".into(),
                key: vec![0],
            });
            let unmet = commit_records(&cluster, "ns", &conditional, until());
            assert!(
                matches!(&unmet, Err(Refusal::Conflict { key, .. }) if key == &[0u8]),
                "{protocol:?} {unmet:?}"
            );
            for k in 0..2u8 {
                let ns = dbs[usize::from(k)].namespace("ns").unwrap();
                let got = ns
                    .get("rows", &[k], Consistency::Linearizable, until())
                    .unwrap();
                assert_eq!(got.map(|r| r.value), Some(vec![5]), "{protocol:?} row {k}");
            }
        }
    }

    /// A first transaction held back at its resolution in one group, as a group behind the
    /// decision is (slow, or its leader moving): the fault, the instant the resolution reached
    /// the group, and the gate the test opens to let it through.
    struct Gate {
        fault: Fault,
        reached: Receiver<()>,
        open: SyncSender<()>,
    }

    /// Holds the first transaction this incarnation begins at its first resolution step in group
    /// `at` until the gate opens. With `recovered_first`, a recovery decides that transaction
    /// aborted at its anchor just before its two-phase decision lands there, so it aborts with
    /// every lock of it held.
    fn gate(dbs: &[Database], at: GroupId, recovered_first: bool) -> Gate {
        let (reach, reached) = mpsc::sync_channel(1);
        let (open, opened) = mpsc::sync_channel(1);
        let opened = Mutex::new(opened);
        let first: Mutex<Option<TxnId>> = Mutex::new(None);
        let held = std::sync::atomic::AtomicBool::new(false);
        let dbs = dbs.to_vec();
        let fault: Fault = Arc::new(move |here, envelope: &Envelope| {
            let txn = envelope
                .step
                .txn()
                .filter(|txn| txn.epoch == envelope.epoch)?;
            if *first.lock().unwrap().get_or_insert(txn) != txn {
                return None;
            }
            match &envelope.step {
                crate::xgroup::Step::Decide {
                    outcome: Outcome::Committed,
                    ..
                }
                | crate::xgroup::Step::DecideAt {
                    outcome: Outcome::Committed,
                    ..
                } if recovered_first => {
                    let anchor = &dbs[usize::try_from(here).unwrap()];
                    anchor
                        .call_internal(until(), move |db| {
                            SqlTablet::new(db).set_record(txn, &Record::Decided(Outcome::Aborted))
                        })
                        .unwrap();
                }
                crate::xgroup::Step::Resolve { .. } | crate::xgroup::Step::ResolveAt { .. }
                    if here == at && !held.swap(true, Ordering::AcqRel) =>
                {
                    reach.send(()).unwrap();
                    opened.lock().unwrap().recv().unwrap();
                }
                _ => {}
            }
            None
        });
        Gate {
            fault,
            reached,
            open,
        }
    }

    /// A cluster over `dbs` answering at `answer_at` under `gate`, for `in_flight` callers.
    fn open_gated(
        dbs: &[Database],
        protocol: Protocol,
        gate: &Gate,
        answer_at: AnswerAt,
        in_flight: usize,
    ) -> Cluster {
        for group in dbs {
            prepare(group, until()).unwrap();
        }
        let route = route_of(u32::try_from(dbs.len()).unwrap());
        let members = dbs.iter().cloned().map(Member::Local).collect();
        let options = Options {
            fault: Some(Arc::clone(&gate.fault)),
            answer_at,
            ..Options::default()
        };
        Cluster::open_with(members, route, 1, protocol, in_flight, options, until()).unwrap()
    }

    /// AC-T1.10.1 (lane xgroup-sql-8), the A/B of when a transaction across groups answers,
    /// forced: group 1's resolution held back (a group behind the decision). Under the control
    /// the answer comes first and a read of group 1 straight after it misses the row it wrote;
    /// answered once resolved, it waits for group 1, holding the answer meanwhile, and the read
    /// finds the row. Both protocols.
    #[test]
    fn ac_t1_10_1_a_read_straight_after_the_answer_in_a_group_behind() {
        use amphora_api::{Consistency, Records};
        let read = |db: &Database| {
            db.namespace("ns")
                .unwrap()
                .get("rows", &[1], Consistency::Linearizable, until())
                .unwrap()
                .map(|r| r.value)
        };
        for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
            for answer_at in [AnswerAt::Decided, AnswerAt::Resolved] {
                let dir = tempfile::tempdir().unwrap();
                let dbs = databases(dir.path(), 3);
                let gate = gate(&dbs, 1, false);
                let cluster = open_gated(&dbs, protocol, &gate, answer_at, 1);
                std::thread::scope(|scope| {
                    let (done, answered) = mpsc::sync_channel(1);
                    let cluster = &cluster;
                    scope.spawn(move || done.send(cluster.commit(across(4), until())).unwrap());
                    gate.reached.recv().unwrap();
                    if answer_at == AnswerAt::Resolved {
                        // Group 1 hasn't resolved it: no answer can have come.
                        assert!(
                            matches!(answered.try_recv(), Err(mpsc::TryRecvError::Empty)),
                            "{protocol:?}: answered before group 1 resolved"
                        );
                        assert_eq!(cluster.counted().answers_held, 1, "{protocol:?}");
                        gate.open.send(()).unwrap();
                    }
                    let answer = answered.recv().unwrap();
                    assert!(
                        matches!(answer, Ok(Answer::Committed { .. })),
                        "{protocol:?} {answer_at:?}: {answer:?}"
                    );
                    match answer_at {
                        AnswerAt::Decided => {
                            assert_eq!(read(&dbs[1]), None, "{protocol:?}: group 1 wasn't behind");
                            gate.open.send(()).unwrap();
                        }
                        AnswerAt::Resolved => {
                            assert_eq!(read(&dbs[1]), Some(vec![4]), "{protocol:?}");
                            let counted = cluster.counted();
                            assert_eq!(counted.answers_held, 0, "{protocol:?} {counted:?}");
                            assert_eq!(counted.answers_held_most, 1, "{protocol:?} {counted:?}");
                        }
                    }
                });
            }
        }
    }

    /// AC-T1.10.1 (lane xgroup-sql-8), the A/B of when a transaction across groups answers,
    /// forced: a two-phase transaction aborts with every lock of it held (a recovery decided it
    /// aborted first) and its resolution in group 0 is held back, while a second caller may begin.
    /// Under the control the abort answers first and the caller's retry runs into the first try's
    /// own locks; answered once resolved, the abort comes once every lock is gone and the retry
    /// commits.
    #[test]
    fn ac_t1_10_1_a_retry_straight_after_an_abort_meets_no_lock_of_its_own() {
        for answer_at in [AnswerAt::Decided, AnswerAt::Resolved] {
            let dir = tempfile::tempdir().unwrap();
            let dbs = databases(dir.path(), 3);
            let gate = gate(&dbs, 0, true);
            // Two callers' room: with one, a retry waits behind the first try's resolution.
            let cluster = open_gated(&dbs, Protocol::TwoPhase, &gate, answer_at, 2);
            std::thread::scope(|scope| {
                let (done, answered) = mpsc::sync_channel(1);
                let cluster = &cluster;
                scope.spawn(move || done.send(cluster.commit(across(5), until())).unwrap());
                gate.reached.recv().unwrap();
                if answer_at == AnswerAt::Resolved {
                    assert!(
                        matches!(answered.try_recv(), Err(mpsc::TryRecvError::Empty)),
                        "answered before group 0 dropped its locks"
                    );
                    gate.open.send(()).unwrap();
                }
                let answer = answered.recv().unwrap();
                assert!(
                    matches!(answer, Ok(Answer::Aborted(_))),
                    "{answer_at:?}: {answer:?}"
                );
                let retry = cluster.commit(across(6), until());
                match answer_at {
                    AnswerAt::Decided => {
                        assert!(
                            matches!(&retry, Ok(Answer::Aborted(Why::Conflict(key)))
                                if (0..3).any(|k| row(k) == *key)),
                            "the control's retry met no lock of the first try's: {retry:?}"
                        );
                        gate.open.send(()).unwrap();
                    }
                    AnswerAt::Resolved => {
                        assert!(matches!(retry, Ok(Answer::Committed { .. })), "{retry:?}");
                    }
                }
            });
        }
    }

    /// A replicated group in this process, each member its own database whose owner listens at
    /// its own endpoint, their records relayed in process: each member's records taken as they are
    /// sent ([`crate::replication::ReplicaHandle::drain_wait`], woken by the send, never a timer),
    /// and every record to the member `cut` names (0: none) dropped, as a network that lost it.
    struct Replicas {
        paths: Vec<std::path::PathBuf>,
        dbs: Vec<Database>,
        cut: Arc<AtomicU64>,
        stop: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Replicas {
        fn open(dir: &std::path::Path, name: &str, voters: u64) -> Self {
            let paths: Vec<_> = (1..=voters)
                .map(|n| dir.join(format!("{name}-{n}.db")))
                .collect();
            let dbs: Vec<Database> = (1..=voters)
                .zip(&paths)
                .map(|(node_id, path)| {
                    let config = amphora_api::replication::ReplicaConfig {
                        node_id,
                        voters: (1..=voters).collect(),
                        learners: Vec::new(),
                        bootstrap: None,
                    };
                    Database::open_replicated(path, &path.with_extension("log"), config, until())
                        .unwrap()
                })
                .collect();
            let cut = Arc::new(AtomicU64::new(0));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            for from in 1..=voters {
                let handles: Vec<_> = dbs.iter().map(|d| d.replica().unwrap()).collect();
                let (cut, stop) = (Arc::clone(&cut), Arc::clone(&stop));
                std::thread::spawn(move || {
                    let mine = &handles[usize::try_from(from - 1).unwrap()];
                    while !stop.load(Ordering::Acquire) {
                        // An owner gone ends its relay.
                        let Ok(records) = mine.drain_wait(64, until()) else {
                            return;
                        };
                        for record in records {
                            if record.to == cut.load(Ordering::Acquire) {
                                continue;
                            }
                            let to = usize::try_from(record.to - 1).unwrap();
                            // A record a member can't take now is lost, as on a network: Raft
                            // sends it again.
                            let _ = handles[to].receive(from, record.bytes, until());
                        }
                    }
                });
            }
            Self {
                paths,
                dbs,
                cut,
                stop,
            }
        }

        /// The member that leads, found by the group's first write: the cross-group tables
        /// ([`prepare`]), made through whichever member it is asked of. A write before any member
        /// leads waits for one (an owner holds a submit for want of a leader); one off the leader
        /// is refused naming it.
        fn leader(&self) -> usize {
            let mut at = 0;
            for _ in 0..=self.dbs.len() {
                match prepare(&self.dbs[at], until()) {
                    Ok(()) => {
                        let status = self.dbs[at].replica_status(until()).unwrap().unwrap();
                        return usize::try_from(status.leader - 1).unwrap();
                    }
                    Err(Refusal::NotLeader { leader: Some(id) }) => {
                        at = usize::try_from(id - 1).unwrap();
                    }
                    Err(refusal) => panic!("{refusal:?}"),
                }
            }
            panic!("no member led");
        }
    }

    impl Drop for Replicas {
        fn drop(&mut self) {
            // Each relay ends at its member's next record, or once its owner is gone.
            self.stop.store(true, Ordering::Release);
        }
    }

    /// A cluster over group 0, `local`, in this process, and group 1, `replicas` reached only
    /// over the transport at member `at`'s endpoint; answering once resolved.
    fn across_nodes(
        local: &Database,
        replicas: &Replicas,
        at: usize,
        protocol: Protocol,
    ) -> Cluster {
        prepare(local, until()).unwrap();
        let node = crate::reshard::NodeId(1);
        let nodes = Arc::new(crate::transport::Nodes::new([(
            node,
            replicas.paths[at].clone(),
        )]));
        let members = vec![Member::Local(local.clone()), Member::Node { nodes, node }];
        let options = Options {
            answer_at: AnswerAt::Resolved,
            ..Options::default()
        };
        Cluster::open_with(members, route_of(2), 1, protocol, 1, options, until()).unwrap()
    }

    /// Rows 0 and 1 of namespace `ns`'s family `rows` put to `v`: one in each of two groups.
    fn puts(v: u8) -> amphora_api::Tx {
        amphora_api::Tx {
            conds: Vec::new(),
            writes: (0..2u8)
                .map(|k| amphora_api::Write::Put {
                    family: "rows".into(),
                    key: vec![k],
                    value: vec![v],
                })
                .collect(),
        }
    }

    /// Namespace `ns`'s head as `db` reads it in a session carrying `token` (none: a local read,
    /// which waits for nothing); none before its first commit is there.
    /// The newest revision of the rows these tests' transactions write (`puts`' keys 0 and 1 in
    /// family "rows" of namespace "ns") that this group holds, read in a session carrying `token`:
    /// each commit that reaches the group moves it, as it moves the namespace's head. Read
    /// through Records, as a client would: the session's SQL refuses the `__amphora_*` tables that
    /// hold them (product metadata).
    fn head(db: &Database, token: Option<Token>) -> Result<Option<i64>, Refusal> {
        db.query_session(crate::database::ReadMode::Session, token, until(), |c| {
            let mut newest = None;
            for key in 0..2u8 {
                if let Some(row) = c.records_get("ns", "rows", &[key])? {
                    newest = newest.max(Some(i64::try_from(row.rev)?));
                }
            }
            Ok(newest)
        })
    }

    /// T-T1.10.W4 (AC-T1.10.1, lane xgroup-sql-9): a group on a second node adds its closed time
    /// to the token. Do: group 0 standalone in this process (the registry), group 1 one replicated
    /// member whose owner listens at its own endpoint, reached only over the transport; a Records
    /// transaction across both through [`commit_records`], answered once resolved, both protocols.
    /// Expect: the token is past 0 (only group 1 keeps a closed time, and only its steps' replies
    /// carry it here) and no later than group 1's own token taken after it; a read carrying it on
    /// group 1 sees the commit.
    #[test]
    fn t_t1_10_w4_a_group_on_a_second_node_adds_its_closed_time_to_the_token() {
        for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
            let dir = tempfile::tempdir().unwrap();
            let replicas = Replicas::open(dir.path(), "group1", 1);
            let leader = replicas.leader();
            let local = Database::open(&dir.path().join("group0.db"), until()).unwrap();
            let cluster = across_nodes(&local, &replicas, leader, protocol);
            let token = commit_records(&cluster, "ns", &puts(5), until()).unwrap();
            let after = replicas.dbs[leader].closed_token(until()).unwrap().unwrap();
            assert!(
                token.micros > 0 && token.micros <= after.micros,
                "{protocol:?}: {token:?} after {after:?}"
            );
            let seen = head(&replicas.dbs[leader], Some(token)).unwrap();
            assert!(seen.is_some_and(|h| h >= 1), "{protocol:?}: {seen:?}");
            drop(cluster);
        }
    }

    /// T-T1.10.W5 (AC-T1.10.1, lane xgroup-sql-9): a session reads at a follower that is behind,
    /// straight after a commit across groups. Do: group 1 three replicated members relayed in
    /// process, reached over the transport at its leader; one follower cut off (every record to it
    /// lost); a Records transaction across groups 0 and 1 through [`commit_records`], answered once
    /// resolved, both protocols; a read with no token on the follower; a read carrying the token
    /// on the follower, begun while it is cut off; then the follower reached again. Expect: the
    /// read with no token misses the commit (the follower is behind); the token read never answers
    /// without it: it waits for the follower (on its owner's publication) and sees it, or it is
    /// redirected to the leader (NotLeader naming it), where the same token sees it; counted once.
    #[test]
    fn t_t1_10_w5_a_token_read_at_a_follower_behind_waits_or_is_redirected() {
        for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
            let dir = tempfile::tempdir().unwrap();
            let replicas = Replicas::open(dir.path(), "group1", 3);
            let leader = replicas.leader();
            let follower = (leader + 1) % replicas.dbs.len();
            let local = Database::open(&dir.path().join("group0.db"), until()).unwrap();
            let cluster = across_nodes(&local, &replicas, leader, protocol);
            replicas
                .cut
                .store(u64::try_from(follower + 1).unwrap(), Ordering::Release);
            let token = commit_records(&cluster, "ns", &puts(7), until()).unwrap();
            assert!(token.micros > 0, "{protocol:?}: {token:?}");
            let want = head(&replicas.dbs[leader], Some(token)).unwrap();
            assert!(want.is_some_and(|h| h >= 1), "{protocol:?}: {want:?}");
            // Cut off since before the commit, the follower can't have it.
            let stale = head(&replicas.dbs[follower], None);
            assert!(
                !matches!(&stale, Ok(h) if *h == want),
                "{protocol:?}: the follower wasn't behind: {stale:?}"
            );
            let reader = replicas.dbs[follower].clone();
            let (begun, began) = mpsc::sync_channel(1);
            let read = std::thread::spawn(move || {
                begun.send(()).unwrap();
                head(&reader, Some(token))
            });
            began.recv().unwrap();
            replicas.cut.store(0, Ordering::Release);
            match read.join().unwrap() {
                Ok(seen) => assert_eq!(seen, want, "{protocol:?}: served without the commit"),
                Err(Refusal::NotLeader { leader: Some(id) }) => {
                    assert_eq!(usize::try_from(id - 1).unwrap(), leader, "{protocol:?}");
                    assert_eq!(head(&replicas.dbs[leader], Some(token)).unwrap(), want);
                }
                Err(refusal) => panic!("{protocol:?}: {refusal:?}"),
            }
            let counted = replicas.dbs[follower]
                .session_counters(until())
                .unwrap()
                .unwrap();
            assert_eq!(
                counted.at_once + counted.waited + counted.redirects,
                1,
                "{protocol:?}: {counted:?}"
            );
            drop(cluster);
        }
    }

    /// T-T1.10.W6 (AC-T1.10.1, lane xgroup-sql-10; for lane split-merge-20's conditional commits
    /// across groups): a committed answer carries its commit stamp. Do: group 0 standalone in this
    /// process, group 1 one replicated member; a cluster reaching group 1 over the transport and
    /// one holding it in process, answered once resolved, both protocols; through each, a commit
    /// across both groups, a second, then one in group 1 alone (in process, the one-group fast
    /// path); and a commit across two standalone groups. Expect: each stamp is past 0, no later
    /// than group 1's own token taken after it, and no earlier than the stamp before it; the
    /// standalone groups' answer has none (they keep no closed time).
    #[test]
    fn t_t1_10_w6_a_committed_answer_carries_its_commit_stamp() {
        let stamp = |answer: Answer| match answer {
            Answer::Committed { at, .. } => at,
            other => panic!("{other:?}"),
        };
        let writes = |rows: &[u8], v: u8| Txn {
            reads: Vec::new(),
            writes: rows.iter().map(|k| (row(*k), Some(vec![v]))).collect(),
            conds: Vec::new(),
        };
        for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
            let dir = tempfile::tempdir().unwrap();
            let replicas = Replicas::open(dir.path(), "group1", 1);
            let leader = replicas.leader();
            let group1 = replicas.dbs[leader].clone();
            let local = Database::open(&dir.path().join("group0.db"), until()).unwrap();
            let over = across_nodes(&local, &replicas, leader, protocol);
            let options = Options {
                answer_at: AnswerAt::Resolved,
                ..Options::default()
            };
            let members = vec![Member::Local(local.clone()), Member::Local(group1.clone())];
            let here =
                Cluster::open_with(members, route_of(2), 2, protocol, 1, options, until()).unwrap();
            let mut last = 0;
            for (cluster, reached) in [(&over, "over the transport"), (&here, "in process")] {
                for (rows, v) in [(&[0u8, 1][..], 1u8), (&[0, 1][..], 2), (&[1][..], 3)] {
                    let at = stamp(cluster.commit(writes(rows, v), until()).unwrap());
                    let after = group1.closed_token(until()).unwrap().unwrap();
                    assert!(
                        at.is_some_and(|at| at > 0 && at >= last && at <= after.micros),
                        "{protocol:?} {reached} {rows:?}: {at:?} after {last}, before {after:?}"
                    );
                    last = at.unwrap_or(last);
                }
            }
            drop(here);
            drop(over);
            let standalone = tempfile::tempdir().unwrap();
            let cluster = open(standalone.path(), 2, 3, protocol);
            let at = stamp(cluster.commit(writes(&[0, 1], 4), until()).unwrap());
            assert_eq!(at, None, "{protocol:?}");
        }
    }

    /// T-T1.10.W7 (AC-T1.10.1, lane xgroup-sql-10): xgroup-sql-7's option 2, reads that wait on a
    /// held lock ([`LockedReads::Wait`]), against [`AnswerAt::Resolved`], forced: group 1's
    /// resolution held back while a read of the row the commit writes there comes. Do: three
    /// standalone groups, both protocols, each of (Decided, Committed), the control, (Resolved,
    /// Committed), option 1, and (Decided, Wait), option 2: a commit across all three, group 1's
    /// resolve held; a read of group 1 once the answer came, under Wait begun before group 1 is
    /// let go and let go only once the read waits on the lock. Expect: the control's read misses
    /// the row; Resolved's answer doesn't come before group 1 resolves, and its read finds the
    /// row with no wait; Wait's answer comes while group 1 still holds the lock, its read waits
    /// (counted once) and answers the row once group 1 resolves. What each adds, Resolved's to
    /// the commit and Wait's to the read, is printed, never asserted.
    ///
    /// [`LockedReads::Wait`]: crate::database::LockedReads::Wait
    #[test]
    fn t_t1_10_w7_a_read_during_the_resolve_waits_on_the_lock_or_the_answer_does() {
        use crate::database::LockedReads;
        use amphora_api::{Consistency, Records};
        let read = |db: &Database| {
            db.namespace("ns")
                .unwrap()
                .get("rows", &[1], Consistency::Linearizable, until())
                .unwrap()
                .map(|r| r.value)
        };
        let modes = [
            (AnswerAt::Decided, LockedReads::Committed),
            (AnswerAt::Resolved, LockedReads::Committed),
            (AnswerAt::Decided, LockedReads::Wait),
            (AnswerAt::Decided, LockedReads::WaitOnLock),
        ];
        for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
            for (answer_at, reads) in modes {
                let mode = format!("{protocol:?} {answer_at:?} {reads:?}");
                let dir = tempfile::tempdir().unwrap();
                let dbs = databases(dir.path(), 3);
                for db in &dbs {
                    db.set_locked_reads(reads);
                }
                let gate = gate(&dbs, 1, false);
                let cluster = open_gated(&dbs, protocol, &gate, answer_at, 1);
                std::thread::scope(|scope| {
                    let (done, answered) = mpsc::sync_channel(1);
                    let cluster = &cluster;
                    let began = Instant::now();
                    scope.spawn(move || {
                        let answer = cluster.commit(across(4), until());
                        done.send((answer, began.elapsed())).unwrap();
                    });
                    gate.reached.recv().unwrap();
                    if answer_at == AnswerAt::Resolved {
                        assert!(
                            matches!(answered.try_recv(), Err(mpsc::TryRecvError::Empty)),
                            "{mode}: answered before group 1 resolved"
                        );
                        gate.open.send(()).unwrap();
                    }
                    let (answer, committed_in) = answered.recv().unwrap();
                    assert!(
                        matches!(answer, Ok(Answer::Committed { .. })),
                        "{mode}: {answer:?}"
                    );
                    let reader = &dbs[1];
                    let (seen, read_in) = match reads {
                        LockedReads::Committed => {
                            let began = Instant::now();
                            (read(reader), began.elapsed())
                        }
                        LockedReads::Wait | LockedReads::WaitOnLock => {
                            let waiting = scope.spawn(move || {
                                let began = Instant::now();
                                (read(reader), began.elapsed())
                            });
                            reader.locked_waits_reach(1, until()).unwrap();
                            gate.open.send(()).unwrap();
                            waiting.join().unwrap()
                        }
                    };
                    if (answer_at, reads) == (AnswerAt::Decided, LockedReads::Committed) {
                        assert_eq!(seen, None, "{mode}: group 1 wasn't behind");
                        gate.open.send(()).unwrap();
                    } else {
                        assert_eq!(seen, Some(vec![4]), "{mode}");
                    }
                    let waits = reader.locked_waits();
                    assert_eq!(waits, u64::from(reads != LockedReads::Committed), "{mode}");
                    eprintln!("{mode}: committed in {committed_in:?}, read in {read_in:?}");
                });
            }
        }
    }

    /// T-T1.10.W8 (AC-T1.10.1, lane xgroup-sql-10): a read waiting on a held lock
    /// ([`LockedReads::Wait`]) of a transaction that aborts answers the row as it was before it.
    /// Do: three standalone groups reading under Wait, the control's answer at the decision; a
    /// two-phase transaction a recovery decides aborted first, so it aborts with every lock held,
    /// its resolution in group 0 held back; a read of its row in group 0 once the abort answered,
    /// group 0 let go once that read waits. Expect: the read waits (counted once) and answers no
    /// row, the one before the transaction.
    ///
    /// [`LockedReads::Wait`]: crate::database::LockedReads::Wait
    #[test]
    fn t_t1_10_w8_a_read_waiting_on_an_aborted_lock_answers_the_row_before() {
        use crate::database::LockedReads;
        use amphora_api::{Consistency, Records};
        for reads in [LockedReads::Wait, LockedReads::WaitOnLock] {
            let dir = tempfile::tempdir().unwrap();
            let dbs = databases(dir.path(), 3);
            for db in &dbs {
                db.set_locked_reads(reads);
            }
            let gate = gate(&dbs, 0, true);
            let cluster = open_gated(&dbs, Protocol::TwoPhase, &gate, AnswerAt::Decided, 1);
            std::thread::scope(|scope| {
                let answer = {
                    let cluster = &cluster;
                    scope.spawn(move || cluster.commit(across(5), until()))
                };
                gate.reached.recv().unwrap();
                let answer = answer.join().unwrap();
                assert!(
                    matches!(answer, Ok(Answer::Aborted(_))),
                    "{reads:?}: {answer:?}"
                );
                let reader = &dbs[0];
                let read = scope.spawn(move || {
                    reader.namespace("ns").unwrap().get(
                        "rows",
                        &[0],
                        Consistency::Linearizable,
                        until(),
                    )
                });
                reader.locked_waits_reach(1, until()).unwrap();
                gate.open.send(()).unwrap();
                let seen = read.join().unwrap();
                assert!(matches!(seen, Ok(None)), "{reads:?}: {seen:?}");
                assert_eq!(reader.locked_waits(), 1, "{reads:?}");
            });
        }
    }

    /// T-T1.10.W9 (AC-T1.10.1, lane xgroup-sql-11): a read waiting on a held lock under
    /// [`LockedReads::WaitOnLock`] is woken by its own row's resolution only, where under
    /// [`LockedReads::Wait`] each change in the group wakes it to read again (the thundering herd
    /// at scale). Do: three standalone groups, the answer at the decision, group 1's resolution
    /// held back; a read of row 1 in group 1, waiting; then commits of another row in group 1;
    /// then group 1 let go. Expect: under WaitOnLock one wake, its row's resolution; under Wait
    /// at least that one; both find the row. Each setting's wakes are printed.
    ///
    /// [`LockedReads::WaitOnLock`]: crate::database::LockedReads::WaitOnLock
    /// [`LockedReads::Wait`]: crate::database::LockedReads::Wait
    #[test]
    fn t_t1_10_w9_a_read_waiting_on_its_lock_is_woken_by_its_own_resolution_only() {
        use crate::database::LockedReads;
        use amphora_api::{Consistency, Records, Tx, Write};
        const OTHERS: u8 = 8;
        for reads in [LockedReads::Wait, LockedReads::WaitOnLock] {
            let dir = tempfile::tempdir().unwrap();
            let dbs = databases(dir.path(), 3);
            for db in &dbs {
                db.set_locked_reads(reads);
            }
            let gate = gate(&dbs, 1, false);
            let cluster = open_gated(&dbs, Protocol::TwoPhase, &gate, AnswerAt::Decided, 1);
            std::thread::scope(|scope| {
                let answer = {
                    let cluster = &cluster;
                    scope.spawn(move || cluster.commit(across(6), until()))
                };
                gate.reached.recv().unwrap();
                let answer = answer.join().unwrap();
                assert!(
                    matches!(answer, Ok(Answer::Committed { .. })),
                    "{reads:?}: {answer:?}"
                );
                let reader = &dbs[1];
                let read = scope.spawn(move || {
                    reader
                        .namespace("ns")
                        .unwrap()
                        .get("rows", &[1], Consistency::Linearizable, until())
                        .unwrap()
                        .map(|r| r.value)
                });
                reader.locked_waits_reach(1, until()).unwrap();
                let others = reader.namespace("ns").unwrap();
                for v in 0..OTHERS {
                    let tx = Tx {
                        conds: Vec::new(),
                        writes: vec![Write::Put {
                            family: "rows".into(),
                            key: vec![4],
                            value: vec![v],
                        }],
                    };
                    others.commit(tx, until()).unwrap();
                }
                gate.open.send(()).unwrap();
                assert_eq!(read.join().unwrap(), Some(vec![6]), "{reads:?}");
                let wakes = reader.locked_wakes();
                match reads {
                    LockedReads::WaitOnLock => assert_eq!(wakes, 1, "{reads:?}"),
                    _ => assert!(wakes >= 1, "{reads:?}: {wakes}"),
                }
                eprintln!("{reads:?}: {wakes} wakes, {OTHERS} other commits in the group");
            });
        }
    }

    /// T-T1.10.W9–W11: the writer carries out a resolution whose caller has already timed out.
    /// Its retry finds a done row. The first commit must still have rung the sleeping reader.
    #[test]
    fn t_t1_10_w9_a_resolutions_lost_reply_does_not_lose_the_wake() {
        use crate::database::LockedReads;
        use crate::xgroup::Step;
        use amphora_api::{Consistency, Records};

        let dir = tempfile::tempdir().unwrap();
        let group = Database::open(&dir.path().join("group.db"), until()).unwrap();
        group.set_locked_reads(LockedReads::WaitOnLock);
        let txn = TxnId {
            coordinator: 1,
            epoch: 1,
            seq: 0,
        };
        let lock = Envelope {
            coordinator: txn.coordinator,
            epoch: txn.epoch,
            floor: None,
            step: Step::Lock {
                txn,
                anchor: 0,
                reads: Vec::new(),
                writes: vec![(row(1), Some(vec![7]))],
                stage: None,
            },
        };
        let started = Instant::now();
        let (locked, _, _) = serve(&group, 0, 0, lock.clone(), until()).unwrap();
        let lock_time = started.elapsed();
        assert_eq!(locked, Reply::Locked { reads: Vec::new() });
        let resolution = Envelope {
            coordinator: txn.coordinator,
            epoch: txn.epoch,
            floor: None,
            step: Step::Resolve {
                txn,
                outcome: Outcome::Committed,
            },
        };
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                group
                    .namespace("ns")
                    .unwrap()
                    .get("rows", &[1], Consistency::Linearizable, until())
            });
            group.locked_waits_reach(1, until()).unwrap();
            // Hold the actual writer until its caller has lost the answer. If an attempt
            // expired before it reached the writer, give the next one longer to enter.
            let limit = until();
            let mut budget = lock_time;
            loop {
                let now = Instant::now();
                assert!(now < limit, "the resolution never entered the writer");
                let deadline = now + budget.min(limit.saturating_duration_since(now));
                let (entered, applied) = mpsc::channel();
                let (resume, held) = mpsc::channel();
                let first = resolution.clone();
                let outcome = step(&group, &AtomicU64::new(0), deadline, move |db| {
                    let result = apply_step(db, 0, &first)?;
                    entered.send(())?;
                    held.recv_timeout(limit.saturating_duration_since(Instant::now()))?;
                    Ok(result)
                });
                match applied.recv_timeout(limit.saturating_duration_since(Instant::now())) {
                    Ok(()) => {
                        resume.send(()).unwrap();
                        assert_eq!(outcome.err(), Some(Refusal::UnknownOutcome));
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        assert!(matches!(
                            outcome,
                            Err(Refusal::Timeout | Refusal::UnknownOutcome)
                        ));
                        budget = budget.saturating_mul(2);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => panic!("writer did not report entry"),
                }
            }
            // A new lock request is Late only once the original writer made its done row.
            let (locked_again, _, _) = serve(&group, 0, 0, lock, until()).unwrap();
            assert_eq!(locked_again, Reply::Late);
            let (reply, _, _) = serve(&group, 0, 0, resolution, until()).unwrap();
            assert_eq!(reply, Reply::Resolved);
            assert_eq!(reader.join().unwrap().unwrap().unwrap().value, [7]);
            assert_eq!(group.locked_wakes(), 1);
        });
    }

    /// T-T1.10.W10 (AC-T1.10.1, lane xgroup-sql-11): a Records scan waits on a held lock in the
    /// range its page covers, under [`LockedReads::Wait`] and [`LockedReads::WaitOnLock`]. Do:
    /// three standalone groups, the answer at the decision, group 1's resolution held back, rows
    /// 0 and 2 already in group 1; scans of group 1 there: one of prefix 9, one of a page of one
    /// row (it stops at row 0, before the locked row 1), then one of the whole family, group 1
    /// let go once it waits. Expect: the first two answer at once, unwaited (rows none, then row
    /// 0 with more); the whole-family scan waits (counted once) and answers rows 0, 1 and 2, row
    /// 1 the transaction's.
    ///
    /// [`LockedReads::WaitOnLock`]: crate::database::LockedReads::WaitOnLock
    /// [`LockedReads::Wait`]: crate::database::LockedReads::Wait
    #[test]
    fn t_t1_10_w10_a_scan_waits_on_a_held_lock_in_its_range_only() {
        use crate::database::LockedReads;
        use amphora_api::{Consistency, Records, Scan, Tx, Write};
        let scan = |db: &Database, prefix: Vec<u8>, limit: usize| {
            db.namespace("ns")
                .unwrap()
                .scan(
                    "rows",
                    Scan {
                        prefix,
                        after: None,
                        limit,
                        at: Consistency::Linearizable,
                    },
                    until(),
                )
                .unwrap()
        };
        for reads in [LockedReads::Wait, LockedReads::WaitOnLock] {
            let dir = tempfile::tempdir().unwrap();
            let dbs = databases(dir.path(), 3);
            for db in &dbs {
                db.set_locked_reads(reads);
            }
            let tx = Tx {
                conds: Vec::new(),
                writes: [0u8, 2]
                    .map(|k| Write::Put {
                        family: "rows".into(),
                        key: vec![k],
                        value: vec![k],
                    })
                    .to_vec(),
            };
            dbs[1].namespace("ns").unwrap().commit(tx, until()).unwrap();
            let gate = gate(&dbs, 1, false);
            let cluster = open_gated(&dbs, Protocol::TwoPhase, &gate, AnswerAt::Decided, 1);
            std::thread::scope(|scope| {
                let answer = {
                    let cluster = &cluster;
                    scope.spawn(move || cluster.commit(across(7), until()))
                };
                gate.reached.recv().unwrap();
                let answer = answer.join().unwrap();
                assert!(
                    matches!(answer, Ok(Answer::Committed { .. })),
                    "{reads:?}: {answer:?}"
                );
                let reader = &dbs[1];
                let outside = scan(reader, vec![9], 10);
                assert!(outside.rows.is_empty() && !outside.more, "{reads:?}");
                let short = scan(reader, Vec::new(), 1);
                let keys: Vec<_> = short.rows.iter().map(|r| r.key.clone()).collect();
                assert_eq!((keys, short.more), (vec![vec![0]], true), "{reads:?}");
                assert_eq!(reader.locked_waits(), 0, "{reads:?}");
                let whole = scope.spawn(move || scan(reader, Vec::new(), 10));
                reader.locked_waits_reach(1, until()).unwrap();
                gate.open.send(()).unwrap();
                let whole = whole.join().unwrap();
                let rows: Vec<_> = whole
                    .rows
                    .iter()
                    .map(|r| (r.key.clone(), r.value.clone()))
                    .collect();
                assert_eq!(
                    rows,
                    vec![(vec![0], vec![0]), (vec![1], vec![7]), (vec![2], vec![2])],
                    "{reads:?}"
                );
                assert_eq!(reader.locked_waits(), 1, "{reads:?}");
            });
        }
    }

    /// T-T1.10.W11 (AC-T1.10.1, lane xgroup-sql-11): a SQL session's Records read of a row under
    /// a held lock, run by `Database::query_waiting`, runs again once the lock resolves, under
    /// [`LockedReads::Wait`] and [`LockedReads::WaitOnLock`]; `Database::query` reads as today.
    /// Do: three standalone groups, the answer at the decision, group 1's resolution held back; a
    /// session reading row 1 in group 1 through `query`, then one through `query_waiting`, group
    /// 1 let go once it waits. Expect: `query`'s session reads no row, unwaited;
    /// `query_waiting`'s waits (counted once) and reads the transaction's row, its session run
    /// at least twice.
    ///
    /// [`LockedReads::WaitOnLock`]: crate::database::LockedReads::WaitOnLock
    /// [`LockedReads::Wait`]: crate::database::LockedReads::Wait
    #[test]
    fn t_t1_10_w11_a_sql_read_of_a_held_lock_runs_again_once_it_resolves() {
        use crate::database::LockedReads;
        use std::sync::atomic::AtomicUsize;
        for reads in [LockedReads::Wait, LockedReads::WaitOnLock] {
            let dir = tempfile::tempdir().unwrap();
            let dbs = databases(dir.path(), 3);
            for db in &dbs {
                db.set_locked_reads(reads);
            }
            let gate = gate(&dbs, 1, false);
            let cluster = open_gated(&dbs, Protocol::TwoPhase, &gate, AnswerAt::Decided, 1);
            std::thread::scope(|scope| {
                let answer = {
                    let cluster = &cluster;
                    scope.spawn(move || cluster.commit(across(8), until()))
                };
                gate.reached.recv().unwrap();
                let answer = answer.join().unwrap();
                assert!(
                    matches!(answer, Ok(Answer::Committed { .. })),
                    "{reads:?}: {answer:?}"
                );
                let reader = &dbs[1];
                let today = reader
                    .query(until(), |c| {
                        Ok(c.records_get("ns", "rows", &[1])?.map(|r| r.value))
                    })
                    .unwrap();
                assert_eq!(today, None, "{reads:?}");
                assert_eq!(reader.locked_waits(), 0, "{reads:?}");
                let runs = Arc::new(AtomicUsize::new(0));
                let waiting = {
                    let runs = Arc::clone(&runs);
                    scope.spawn(move || {
                        reader.query_waiting(until(), move |c| {
                            runs.fetch_add(1, Ordering::AcqRel);
                            Ok(c.records_get("ns", "rows", &[1])?.map(|r| r.value))
                        })
                    })
                };
                reader.locked_waits_reach(1, until()).unwrap();
                gate.open.send(()).unwrap();
                let seen = waiting.join().unwrap();
                assert_eq!(seen, Ok(Some(vec![8])), "{reads:?}");
                assert_eq!(reader.locked_waits(), 1, "{reads:?}");
                // Wait runs it again after the wake; WaitOnLock once more first, as it joins
                // the row's list.
                let runs = runs.load(Ordering::Acquire);
                assert!(runs >= 2, "{reads:?}: {runs}");
                eprintln!("{reads:?}: the session ran {runs} times");
            });
        }
    }

    /// T-T1.10.W13 (AC-T1.10.1, lane xgroup-sql-12): under [`LockedReads::WaitOnLock`] a read on
    /// a follower is rung by its own replica's application of the resolution, not by the process
    /// that ran the step. Do: group 0 standalone, group 1 three replicated members relayed in
    /// process, its leader in the cluster; the answer at the decision, group 1's resolution held
    /// back; once the follower has the locks (a token read there), a local read of row 1 waiting
    /// on the follower; then commits of another row of group 1 at the leader; then group 1 let
    /// go. Expect: the read finds the transaction's row, woken once, by its row's resolution.
    ///
    /// [`LockedReads::WaitOnLock`]: crate::database::LockedReads::WaitOnLock
    #[test]
    fn t_t1_10_w13_a_follower_read_is_rung_by_its_own_replica_applying_the_resolution() {
        use crate::database::LockedReads;
        use amphora_api::{Records, Tx, Write};
        const OTHERS: u8 = 8;
        let dir = tempfile::tempdir().unwrap();
        let replicas = Replicas::open(dir.path(), "group1", 3);
        let leader = replicas.leader();
        let follower = (leader + 1) % replicas.dbs.len();
        let local = Database::open(&dir.path().join("group0.db"), until()).unwrap();
        prepare(&local, until()).unwrap();
        let group1 = replicas.dbs[leader].clone();
        let dbs = [local.clone(), group1.clone()];
        for db in dbs.iter().chain(&replicas.dbs) {
            db.set_locked_reads(LockedReads::WaitOnLock);
        }
        let gate = gate(&dbs, 1, false);
        let options = Options {
            fault: Some(Arc::clone(&gate.fault)),
            answer_at: AnswerAt::Decided,
            ..Options::default()
        };
        let members = dbs.iter().cloned().map(Member::Local).collect();
        let cluster = Cluster::open_with(
            members,
            route_of(2),
            1,
            Protocol::TwoPhase,
            1,
            options,
            until(),
        )
        .unwrap();
        let txn = Txn {
            reads: Vec::new(),
            writes: (0..2).map(|k| (row(k), Some(vec![6]))).collect(),
            conds: Vec::new(),
        };
        std::thread::scope(|scope| {
            let answer = {
                let cluster = &cluster;
                scope.spawn(move || cluster.commit(txn, until()))
            };
            gate.reached.recv().unwrap();
            let answer = answer.join().unwrap();
            assert!(matches!(answer, Ok(Answer::Committed { .. })), "{answer:?}");
            // The follower applies the locks before the read begins: a read carrying the leader's
            // token waits for them there, on its owner's publication. Past its measured apply lag
            // it is redirected (NotLeader), still behind, and waits again.
            let token = group1.closed_token(until()).unwrap().unwrap();
            while let Err(refusal) = head(&replicas.dbs[follower], Some(token)) {
                assert!(matches!(refusal, Refusal::NotLeader { .. }), "{refusal:?}");
            }
            let reader = &replicas.dbs[follower];
            let read = scope.spawn(move || {
                reader.query_local_waiting(until(), |c| {
                    Ok(c.records_get("ns", "rows", &[1])?.map(|r| r.value))
                })
            });
            reader.locked_waits_reach(1, until()).unwrap();
            let others = group1.namespace("ns").unwrap();
            for v in 0..OTHERS {
                let tx = Tx {
                    conds: Vec::new(),
                    writes: vec![Write::Put {
                        family: "rows".into(),
                        key: vec![3],
                        value: vec![v],
                    }],
                };
                others.commit(tx, until()).unwrap();
            }
            gate.open.send(()).unwrap();
            assert_eq!(read.join().unwrap(), Ok(Some(vec![6])));
            let wakes = reader.locked_wakes();
            assert_eq!(wakes, 1, "{OTHERS} other commits in the group");
            assert_eq!(reader.locked_waits(), 1);
        });
        drop(cluster);
    }
}
