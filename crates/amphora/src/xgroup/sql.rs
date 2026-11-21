//! Transactions across groups on real groups (docs/design/xgroup.md, "On a group's database"): a
//! [`Tablet`] over a group's own database, and [`Cluster`], the shell that runs a [`Coordinator`]
//! over [`Database`]s, each step one [`Database`] call: one atomic commit in its group, durable and,
//! on a replicated group, committed by its quorum before it answers.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use amphora_api::Refusal;
use amphora_engine::Value;

use super::coordinator::{Answer, Config, Coordinator, Event, Lost, Protocol, Txn};
use super::{GroupId, Guard, Intent, Lock, Mark, Outcome, Record, Reply, Tablet, TxnId, apply};
use crate::database::Database;
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
";

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
    let chunks = bytes.chunks_exact(4);
    if !chunks.remainder().is_empty() {
        anyhow::bail!(
            "a record's participants are {} bytes, not a multiple of 4",
            bytes.len()
        );
    }
    chunks
        .map(|c| Ok(GroupId::from_be_bytes(c.try_into()?)))
        .collect()
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

/// A group's database as a [`Tablet`], inside the step's one transaction.
pub(crate) struct SqlTablet<'a> {
    db: &'a Db,
}

impl<'a> SqlTablet<'a> {
    pub(crate) fn new(db: &'a Db) -> Self {
        Self { db }
    }
}

impl Tablet for SqlTablet<'_> {
    fn value(&mut self, key: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
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
    ) -> anyhow::Result<(Vec<(TxnId, Option<GroupId>)>, bool)> {
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

/// Which group a key is in: the caller's, so the groups' routing stays the routing lanes' own.
pub type Route = Arc<dyn Fn(&[u8]) -> GroupId + Send + Sync>;

enum Input {
    Begin {
        txn: Txn,
        until: Instant,
        answer: SyncSender<Result<Answer, Refusal>>,
    },
    Reply {
        id: u64,
        reply: Result<Reply, Refusal>,
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
}

/// `by` from now; an instant past the clock's range can't be waited for, so it is now.
fn later(by: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(by).unwrap_or(now)
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
    /// transactions it runs at once, the caller's (the threads it has calling [`Cluster::transact`]).
    pub fn open(
        groups: Vec<Database>,
        route: Route,
        id: u64,
        protocol: Protocol,
        in_flight: usize,
        until: Instant,
    ) -> Result<Self, Refusal> {
        for group in &groups {
            group.call_internal(until, |db| db.exec(SCHEMA))?;
        }
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
        let driver = Driver {
            groups: Arc::new(groups),
            route,
            coordinator,
            inputs: inputs.clone(),
            waiters: BTreeMap::new(),
            deadlines: BTreeMap::new(),
            open_until: until,
            ready: Some(ready),
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
        };
        match is_ready.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok(result) => result.map(|()| cluster),
            Err(RecvTimeoutError::Timeout) => Err(Refusal::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(stopped()),
        }
    }

    /// Runs `txn`: committed in every group or aborted in every group, as the answer says.
    /// Unanswered by `until`, its outcome is unknown ([`Refusal::UnknownOutcome`]).
    pub fn transact(&self, txn: Txn, until: Instant) -> Result<Answer, Refusal> {
        let (answer, answered) = mpsc::sync_channel(1);
        self.inputs
            .send(Input::Begin { txn, until, answer })
            .map_err(|_| stopped())?;
        match answered.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(Refusal::UnknownOutcome),
            Err(RecvTimeoutError::Disconnected) => Err(stopped()),
        }
    }
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

struct Driver {
    groups: Arc<Vec<Database>>,
    route: Route,
    coordinator: Coordinator,
    inputs: SyncSender<Input>,
    waiters: BTreeMap<TxnId, SyncSender<Result<Answer, Refusal>>>,
    /// Each transaction's deadline and the budget its caller gave it: until answered, the caller's
    /// own; then the same budget again from the answer for its resolution.
    deadlines: BTreeMap<TxnId, (Instant, Duration)>,
    open_until: Instant,
    ready: Option<SyncSender<Result<(), Refusal>>>,
}

impl Driver {
    fn run(mut self, first: Vec<Event>, receiver: Receiver<Input>) {
        let mut events = first;
        loop {
            if let Err(refusal) = self.handle(events) {
                self.stop(&refusal);
                return;
            }
            events = match receiver.recv() {
                Ok(Input::Begin { txn, until, answer }) => {
                    match self.coordinator.begin(txn, |key| (self.route)(key)) {
                        Ok((id, events)) => {
                            let budget = until.saturating_duration_since(Instant::now());
                            self.waiters.insert(id, answer);
                            self.deadlines.insert(id, (until, budget));
                            events
                        }
                        Err(refusal) => {
                            // A caller gone by now needs no answer.
                            let _ = answer.send(Err(refusal));
                            Vec::new()
                        }
                    }
                }
                Ok(Input::Reply { id, reply }) => {
                    let reply = match reply {
                        Ok(reply) => Ok(reply),
                        Err(refusal) if retryable(&refusal) => Err(Lost),
                        Err(refusal) => {
                            self.stop(&refusal);
                            return;
                        }
                    };
                    match self.coordinator.reply(id, reply) {
                        Ok(events) => events,
                        Err(refusal) => {
                            self.stop(&refusal);
                            return;
                        }
                    }
                }
                Ok(Input::Stop) | Err(_) => {
                    self.stop(&stopped());
                    return;
                }
            };
        }
    }

    fn handle(&mut self, events: Vec<Event>) -> Result<(), Refusal> {
        for event in events {
            match event {
                Event::Send(send) => {
                    let until = send
                        .txn
                        .and_then(|txn| self.deadlines.get(&txn))
                        .map_or(self.open_until, |(until, _)| *until);
                    if Instant::now() >= until {
                        // A step that found no answer within its budget: this incarnation stops,
                        // as a crash would, and the next one's recovery finishes the transaction.
                        return Err(Refusal::Unavailable(
                            "a cross-group step found no answer within its budget; open the coordinator again to finish it".into(),
                        ));
                    }
                    let index = usize::try_from(send.group)
                        .map_err(|_| amphora_api::invalid("cross-group step", "group number"))?;
                    let group =
                        self.groups.get(index).cloned().ok_or_else(|| {
                            amphora_api::invalid("cross-group step", "no such group")
                        })?;
                    let inputs = self.inputs.clone();
                    let (id, here, envelope) = (send.id, send.group, send.envelope);
                    std::thread::Builder::new()
                        .name("amphora-xgroup-step".into())
                        .spawn(move || {
                            let reply = group.call_internal(until, move |db| {
                                apply(here, &mut SqlTablet::new(db), &envelope)
                            });
                            // A driver gone by now has stopped: its successor recovers the step.
                            let _ = inputs.send(Input::Reply { id, reply });
                        })
                        .map_err(|e| {
                            Refusal::Unavailable(format!("a cross-group step's thread: {e}"))
                        })?;
                }
                Event::Answer { txn, answer, .. } => {
                    if let Some(waiter) = self.waiters.remove(&txn) {
                        // A caller gone by now needs no answer.
                        let _ = waiter.send(Ok(answer));
                    }
                    if let Some((until, budget)) = self.deadlines.get_mut(&txn) {
                        *until = later(*budget);
                    }
                }
                Event::Finished { txn, .. } => {
                    self.deadlines.remove(&txn);
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

    fn stop(&mut self, refusal: &Refusal) {
        for (_, waiter) in std::mem::take(&mut self.waiters) {
            let _ = waiter.send(Err(refusal.clone()));
        }
        if let Some(ready) = self.ready.take() {
            let _ = ready.send(Err(refusal.clone()));
        }
    }
}
