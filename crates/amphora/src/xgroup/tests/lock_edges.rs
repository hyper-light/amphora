//! T-T1.9.R28/R30 at their edges (lane D5; reshard.md, "Publication and recovery"): a power cut
//! around a lock's vote and around its resolution, replies out of order and come again, groups that
//! missed the stamp, a late incarnation, and the bytes these steps have on the wire.

use super::*;
use crate::backup::{Hooks, MICRO_NS};
use crate::database::{AnswerMax, Database};
use crate::replication::{Applied, Machine, closed};
use hyper_durable::{EntryRef, StateMachine};
use hyper_raft::proto::{ConfState, EntryType};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// A disk that keeps each change it is given, in order, so a test can cut its power after any one
/// of them.
struct Tape {
    id: u64,
    files: Mutex<BTreeMap<String, Arc<Spool>>>,
    changes: Arc<Mutex<Vec<Change>>>,
}

struct Spool {
    path: String,
    bytes: Mutex<Vec<u8>>,
    changes: Arc<Mutex<Vec<Change>>>,
}

#[derive(Clone)]
enum Change {
    Write(String, usize, Vec<u8>),
    Truncate(String, usize),
    Flush(String),
    Remove(String),
}

static TAPES: AtomicU64 = AtomicU64::new(0);

fn write_at(bytes: &mut Vec<u8>, at: usize, data: &[u8]) {
    let end = at + data.len();
    if bytes.len() < end {
        bytes.resize(end, 0);
    }
    bytes[at..end].copy_from_slice(data);
}

impl Tape {
    fn of(image: BTreeMap<String, Vec<u8>>) -> Arc<Self> {
        let changes: Arc<Mutex<Vec<Change>>> = Arc::default();
        let files = image
            .into_iter()
            .map(|(path, bytes)| {
                let spool = Spool {
                    path: path.clone(),
                    bytes: Mutex::new(bytes),
                    changes: Arc::clone(&changes),
                };
                (path, Arc::new(spool))
            })
            .collect();
        Arc::new(Self {
            id: TAPES.fetch_add(1, Ordering::Relaxed),
            files: Mutex::new(files),
            changes,
        })
    }

    /// How many changes it was given.
    fn len(&self) -> usize {
        self.changes.lock().unwrap().len()
    }

    /// What is on it after its first `at` changes and a crash there: the process's alone, which
    /// keeps every write (`kept`), or the machine's, which keeps what a flush covered.
    fn cut(&self, at: usize, kept: bool) -> BTreeMap<String, Vec<u8>> {
        let mut seen: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut durable: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for change in &self.changes.lock().unwrap()[..at] {
            match change {
                Change::Write(path, pos, data) => {
                    write_at(seen.entry(path.clone()).or_default(), *pos, data);
                }
                Change::Truncate(path, len) => {
                    seen.entry(path.clone()).or_default().resize(*len, 0);
                }
                Change::Flush(path) => {
                    let bytes = seen.get(path).cloned().unwrap_or_default();
                    durable.insert(path.clone(), bytes);
                }
                Change::Remove(path) => {
                    seen.remove(path);
                    durable.remove(path);
                }
            }
        }
        if kept { seen } else { durable }
    }
}

impl Spool {
    fn given(&self, change: Change) {
        self.changes.lock().unwrap().push(change);
    }

    fn put(
        &self,
        pos: u64,
        data: &[u8],
        c: amphora_engine::Completion,
    ) -> amphora_engine::Result<amphora_engine::Completion> {
        let pos = usize::try_from(pos).unwrap();
        write_at(&mut self.bytes.lock().unwrap(), pos, data);
        self.given(Change::Write(self.path.clone(), pos, data.to_vec()));
        c.complete(i32::try_from(data.len()).unwrap());
        Ok(c)
    }
}

impl amphora_engine::File for Spool {
    fn lock_file(&self, _exclusive: bool) -> amphora_engine::Result<()> {
        Ok(())
    }
    fn unlock_file(&self) -> amphora_engine::Result<()> {
        Ok(())
    }
    fn pread(
        &self,
        pos: u64,
        c: amphora_engine::Completion,
    ) -> amphora_engine::Result<amphora_engine::Completion> {
        let n = {
            let bytes = self.bytes.lock().unwrap();
            let dst = c.as_read().buf().as_mut_slice();
            let src = bytes
                .get(usize::try_from(pos).unwrap()..)
                .unwrap_or_default();
            let n = src.len().min(dst.len());
            dst[..n].copy_from_slice(&src[..n]);
            n
        };
        c.complete(i32::try_from(n).unwrap());
        Ok(c)
    }
    fn pwrite(
        &self,
        pos: u64,
        buffer: Arc<amphora_engine::Buffer>,
        c: amphora_engine::Completion,
    ) -> amphora_engine::Result<amphora_engine::Completion> {
        self.put(pos, buffer.as_slice(), c)
    }
    fn pwritev(
        &self,
        pos: u64,
        buffers: Vec<Arc<amphora_engine::Buffer>>,
        c: amphora_engine::Completion,
    ) -> amphora_engine::Result<amphora_engine::Completion> {
        let data: Vec<u8> = buffers
            .iter()
            .flat_map(|b| b.as_slice().iter().copied())
            .collect();
        self.put(pos, &data, c)
    }
    fn sync(
        &self,
        c: amphora_engine::Completion,
        _sync_type: amphora_engine::io::FileSyncType,
    ) -> amphora_engine::Result<amphora_engine::Completion> {
        self.given(Change::Flush(self.path.clone()));
        c.complete(0);
        Ok(c)
    }
    fn truncate(
        &self,
        len: u64,
        c: amphora_engine::Completion,
    ) -> amphora_engine::Result<amphora_engine::Completion> {
        let len = usize::try_from(len).unwrap();
        self.bytes.lock().unwrap().resize(len, 0);
        self.given(Change::Truncate(self.path.clone(), len));
        c.complete(0);
        Ok(c)
    }
    fn size(&self) -> amphora_engine::Result<u64> {
        Ok(u64::try_from(self.bytes.lock().unwrap().len()).unwrap())
    }
}

impl amphora_engine::Clock for Tape {
    fn current_time_monotonic(&self) -> amphora_engine::MonotonicInstant {
        amphora_engine::MonotonicInstant::now()
    }
    fn current_time_wall_clock(&self) -> amphora_engine::WallClockInstant {
        amphora_engine::WallClockInstant::now()
    }
}

impl amphora_engine::IO for Tape {
    fn open_file(
        &self,
        path: &str,
        flags: amphora_engine::OpenFlags,
        _direct: bool,
    ) -> amphora_engine::Result<Arc<dyn amphora_engine::File>> {
        let mut files = self.files.lock().unwrap();
        if let Some(file) = files.get(path) {
            return Ok(file.clone());
        }
        if !flags.contains(amphora_engine::OpenFlags::Create) {
            return Err(amphora_engine::LimboError::CompletionError(
                amphora_engine::CompletionError::IOError(std::io::ErrorKind::NotFound, "open"),
            ));
        }
        let file = Arc::new(Spool {
            path: path.to_owned(),
            bytes: Mutex::default(),
            changes: Arc::clone(&self.changes),
        });
        files.insert(path.to_owned(), file.clone());
        Ok(file)
    }
    fn remove_file(&self, path: &str) -> amphora_engine::Result<()> {
        self.files.lock().unwrap().remove(path);
        self.changes
            .lock()
            .unwrap()
            .push(Change::Remove(path.to_owned()));
        Ok(())
    }
    fn file_id(&self, path: &str) -> amphora_engine::Result<amphora_engine::io::FileId> {
        Ok(amphora_engine::io::FileId::from_path_hash(&format!(
            "tape-{}:{path}",
            self.id
        )))
    }
}

/// What a group's database says of a transaction's one Records row: how many locks, the vote, the
/// row's value and stamp, and the commit stamp kept.
type Says = (
    usize,
    Option<u64>,
    Option<Vec<u8>>,
    Option<u64>,
    Option<u64>,
);

fn says(db: &mut Db, id: TxnId, key: &[u8]) -> anyhow::Result<Says> {
    let mut tablet = SqlTablet::new(db);
    let value = tablet.value(key)?;
    Ok((
        tablet.locks_of(id)?.len(),
        tablet.lock_stamp(id)?,
        value.clone(),
        match value {
            Some(_) => tablet.stamp(key)?,
            None => None,
        },
        tablet.commit_stamp(id)?,
    ))
}

/// Where a crash leaves one group, by what it says; anything else is a lock without its vote, a
/// vote without its lock, or a row at another stamp.
fn left(says: &Says, heard: u64) -> &'static str {
    match says {
        (0, None, None, None, None) => "nothing",
        (1, Some(vote), None, None, None) if *vote == heard => "locked",
        (0, None, Some(value), Some(at), Some(kept))
            if value == b"v" && *at == heard && *kept == heard =>
        {
            "resolved"
        }
        other => panic!("a crash may not leave {other:?} of a lock heard at {heard}"),
    }
}

/// What a recovery hears of a group a crash left so ([`left`]).
fn told(state: &str, heard: u64) -> Reply {
    Reply::PresentAt {
        present: state != "nothing",
        committed: (state == "resolved").then_some(heard),
        locked: (state == "locked").then_some(heard),
    }
}

/// T-T1.9.R30, a crash between a lock's reply and its durable record, in both orders, on a
/// standalone backed-up group. Do: a lock step whose publication clock moves from 21 to 50 while it
/// runs, its flush (the answer leaves after it), then its resolution and that flush; the crash
/// after every write, truncation and flush from before the lock on, once the process's alone and
/// once the machine's power; the database opened again on what it left. Expect: the lock and its
/// vote together or neither, the vote the one the reply carries; never neither once the lock's
/// flush ended; the resolved row at that stamp; and recovery told exactly that.
#[test]
fn t_t1_9_r30_a_crash_at_every_write_keeps_a_lock_and_its_vote_together() {
    let path = Path::new("/tape/owner.db");
    let disk = Tape::of(BTreeMap::new());
    let mut owner = Db::open_with(path, disk.clone()).unwrap();
    crate::database::bootstrap_db(&owner).unwrap();
    owner.exec(SCHEMA).unwrap();
    let now = Arc::new(AtomicU64::new(21 * MICRO_NS));
    let clock = Arc::clone(&now);
    let hooks = Hooks::new(Arc::new(move || clock.load(Ordering::Acquire)), 8);
    owner.back_up(Arc::clone(&hooks));
    owner.flusher()().unwrap();

    let id = txn(1);
    let key = sql::record_key("n", "f", b"k").unwrap();
    let lock = env(1, None, lock_step(id, &[], &[(&key, b"v")], Some(vec![0])));
    let before = disk.len();
    let reply = owner
        .transaction(|db| {
            let reply = apply(0, &mut SqlTablet::new(db), &lock)?;
            now.store(50 * MICRO_NS, Ordering::Release);
            Ok(reply)
        })
        .unwrap();
    assert!(matches!(reply, Reply::Locked { .. }));
    let heard = SqlTablet::new(&mut owner).lock_stamp(id).unwrap().unwrap();
    assert_eq!(heard, 50);
    owner.flusher()().unwrap();
    let answered = disk.len();
    let resolve = env(
        1,
        None,
        Step::ResolveAt {
            txn: id,
            outcome: Outcome::Committed,
            at: heard,
        },
    );
    owner
        .transaction(|db| apply(0, &mut SqlTablet::new(db), &resolve))
        .unwrap();
    owner.flusher()().unwrap();
    let resolved = disk.len();
    drop(owner);

    let mut met: BTreeMap<&str, u64> = BTreeMap::new();
    for at in before..=resolved {
        for kept in [false, true] {
            let mut owner = Db::open_with(path, Tape::of(disk.cut(at, kept))).unwrap();
            let state = left(&says(&mut owner, id, &key).unwrap(), heard);
            assert!(
                at < answered || state != "nothing",
                "a reply left and its lock is gone: cut after {at}, writes kept {kept}"
            );
            assert!(at < resolved || state == "resolved");
            let prevent = env(2, None, Step::PreventAt { txn: id });
            let heard_of = apply(0, &mut SqlTablet::new(&mut owner), &prevent).unwrap();
            assert_eq!(
                heard_of,
                told(state, heard),
                "cut after {at}, writes kept {kept}"
            );
            *met.entry(state).or_default() += 1;
        }
    }
    for state in ["nothing", "locked", "resolved"] {
        assert!(met.contains_key(state), "no crash left {state}: {met:?}");
    }
}

fn land(machine: &mut Machine, index: u64, proposed: u64, pages: &[u8]) -> Vec<Applied> {
    let mut answers = Vec::new();
    machine
        .apply(
            &EntryRef {
                index,
                term: 1,
                kind: EntryType::EntryNormal,
                context: b"lock-edge",
                data: &closed::stamp(proposed, pages),
            },
            &mut answers,
        )
        .unwrap();
    answers
}

/// T-T1.9.R30, the same crash on a replicated member, whose database flushes behind what it
/// answered. Do: a lock delta proposed at 21 after a closed time of 49, applied at 50, its
/// resolution at 50 and an unrelated entry at 90, each flushed only at the end; the crash after
/// every change from before the lock on, the process's and the machine's; the member opened again
/// and the log's entries it doesn't hold applied again, as a restart does. Expect: at every crash
/// the lock and its vote together or neither, and once the log is applied the row at 50, whatever
/// the closed time has reached.
#[test]
fn t_t1_9_r30_a_member_that_applies_its_log_again_keeps_the_vote_it_answered() {
    let dir = tempfile::tempdir().unwrap();
    let path = Path::new("/tape/member.db");
    let conf = ConfState {
        voters: vec![1],
        ..Default::default()
    };
    let open = |disk: Arc<Tape>, name: &str| {
        let lease = crate::database::exclusive_file(&dir.path().join(name)).unwrap();
        let owner = crate::database::Owned::unbudgeted(Arc::new(lease), None);
        let mut machine = Machine::open_on(path, disk, conf.clone(), owner, 1, false).unwrap();
        // The tape is no filesystem: one of the test's, all of it free.
        machine.stand_in_filesystem(0, u64::MAX);
        machine
    };
    let disk = Tape::of(BTreeMap::new());
    let mut member = open(disk.clone(), "first");
    let encoded = |member: &mut Machine, step: &Envelope| {
        member
            .prepare_db(|db| apply(0, &mut SqlTablet::new(db), step))
            .unwrap()
            .0
            .encode()
            .unwrap()
    };
    let schema = member
        .prepare_db(|db| db.exec(SCHEMA))
        .unwrap()
        .0
        .encode()
        .unwrap();
    assert!(land(&mut member, 1, 49, &schema)[0].result.is_ok());
    member.log_flusher()().unwrap();

    let id = txn(1);
    let key = sql::record_key("n", "f", b"k").unwrap();
    let lock = env(1, None, lock_step(id, &[], &[(&key, b"v")], Some(vec![0])));
    let resolve = env(
        2,
        None,
        Step::ResolveAt {
            txn: id,
            outcome: Outcome::Committed,
            at: 50,
        },
    );
    let before = disk.len();
    let lock = encoded(&mut member, &lock);
    let applied = land(&mut member, 2, 21, &lock);
    assert!(applied[0].result.is_ok());
    assert_eq!(applied[0].time, Some(50));
    let locked = disk.len();
    let other = env(1, None, lock_step(txn(2), &[b"other"], &[], None));
    let other = encoded(&mut member, &other);
    assert!(land(&mut member, 3, 90, &other)[0].result.is_ok());
    let resolve = encoded(&mut member, &resolve);
    assert!(land(&mut member, 4, 91, &resolve)[0].result.is_ok());
    member.log_flusher()().unwrap();
    let flushed = disk.len();
    drop(member);

    let log = [(2, 21, &lock), (3, 90, &other), (4, 91, &resolve)];
    let mut met: BTreeMap<(&str, u64), u64> = BTreeMap::new();
    for at in before..=flushed {
        for kept in [false, true] {
            let mut member = open(Tape::of(disk.cut(at, kept)), &format!("{at}-{kept}"));
            let held = member.read_db(|db| says(db, id, &key)).unwrap();
            let state = left(&held, 50);
            assert!(!kept || at < locked || state != "nothing");
            let from = member.applied().index;
            for (index, proposed, entry) in log {
                if index > from {
                    assert!(
                        land(&mut member, index, proposed, entry)[0].result.is_ok(),
                        "entry {index}, cut after {at}, writes kept {kept}"
                    );
                }
                if index == 2 && from < 4 {
                    let held = member.read_db(|db| says(db, id, &key)).unwrap();
                    assert_eq!(left(&held, 50), "locked");
                }
            }
            let held = member.read_db(|db| says(db, id, &key)).unwrap();
            assert_eq!(left(&held, 50), "resolved");
            *met.entry((state, from)).or_default() += 1;
        }
    }
    assert!(met.contains_key(&("nothing", 1)), "{met:?}");
    assert!(met.contains_key(&("locked", 2)), "{met:?}");
    assert!(met.contains_key(&("resolved", 4)), "{met:?}");
}

const VOTES: [u64; 3] = [30, 50, 40];

fn three_groups() -> Vec<Memory> {
    (0..3u8)
        .map(|g| {
            let mut group = Memory::default();
            for i in 0..g * 10 {
                group.set_value(&[b'z', i], Some(b"v")).unwrap();
            }
            group
        })
        .collect()
}

fn a_key_in_each() -> Txn {
    Txn {
        reads: Vec::new(),
        writes: (0..3u8).map(|g| (vec![g], Some(vec![g]))).collect(),
        conds: Vec::new(),
    }
}

fn across_three(protocol: Protocol) -> Config {
    Config {
        id: 7,
        protocol,
        registry: 0,
        groups: vec![0, 1, 2],
        in_flight: 1,
    }
}

fn row_stamps(groups: &mut [Memory]) -> Vec<Option<u64>> {
    (0..3u8)
        .map(|g| groups[usize::from(g)].stamp(&[g]).unwrap())
        .collect()
}

/// Runs `coordinator` on `groups`, each step landing as it is sent, its lock published at `VOTES`'
/// entry. The replies to a round of locks, or of a recovery's preventions, come once every group's
/// is in, in `order` and each twice; what `fate` says of a step before it lands stands, its group
/// named. What the client heard, and every step `fate` lost.
fn relayed(
    groups: &mut [Memory],
    coordinator: &mut Coordinator,
    first: Vec<Event>,
    mut begin: Option<Txn>,
    order: &[usize],
    mut fate: impl FnMut(GroupId, &Step) -> Fate,
) -> (Option<Answer>, Vec<(GroupId, Envelope)>) {
    let mut queue: VecDeque<Event> = first.into();
    let mut round: Vec<(u64, Reply, Option<u64>)> = Vec::new();
    let (mut heard, mut lost) = (None, Vec::new());
    while let Some(event) = queue.pop_front() {
        match event {
            Event::Send(send) => {
                match fate(send.group, &send.envelope.step) {
                    Fate::Crash => return (heard, lost),
                    Fate::Lose => {
                        lost.push((send.group, send.envelope));
                        continue;
                    }
                    Fate::Land | Fate::LandThenCrash => {}
                }
                let at = usize::try_from(send.group).unwrap();
                let (reply, clock) = groups[at]
                    .apply_committed(send.group, &send.envelope, VOTES.get(at).copied())
                    .unwrap();
                if matches!(
                    send.envelope.step,
                    Step::Lock { .. } | Step::PreventAt { .. }
                ) {
                    round.push((send.id, reply, clock));
                    if round.len() == groups.len() {
                        for which in order {
                            let (id, reply, clock) = round[*which].clone();
                            for _ in 0..2 {
                                let events = coordinator.reply(id, Ok(reply.clone()), clock);
                                queue.extend(events.unwrap());
                            }
                        }
                        round.clear();
                    }
                } else {
                    queue.extend(coordinator.reply(send.id, Ok(reply), clock).unwrap());
                }
            }
            Event::Ready => {
                if let Some(txn) = begin.take() {
                    let route = |key: &[u8]| key.last().map_or(0, |g| GroupId::from(*g));
                    queue.extend(coordinator.begin(txn, route).unwrap().1);
                }
            }
            Event::Answer { answer, .. } => heard = Some(answer),
            _ => {}
        }
    }
    (heard, lost)
}

const ORDERS: [[usize; 3]; 6] = [
    [0, 1, 2],
    [0, 2, 1],
    [1, 0, 2],
    [1, 2, 0],
    [2, 0, 1],
    [2, 1, 0],
];

/// T-T1.9.R30, an older reply after a newer one. Do: three groups' locks published at 30, 50 and
/// 40, the replies heard in each of the six orders and each twice, both protocols; under Parallel
/// Commits a crash with the locks alone, a recovery that hears their votes in each of the six
/// orders, and then the first incarnation's decision and resolutions, come late and at an older
/// stamp. Expect: the client hears 50 in every order, every recovered row is at 50, and the late
/// steps are fenced and change nothing.
#[test]
fn t_t1_9_r30_the_order_replies_come_in_changes_no_stamp() {
    for order in ORDERS {
        for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
            let mut groups = three_groups();
            let (mut first, events) = Coordinator::start(across_three(protocol)).unwrap();
            let (heard, _) = relayed(
                &mut groups,
                &mut first,
                events,
                Some(a_key_in_each()),
                &order,
                |_, _| Fate::Land,
            );
            assert!(
                matches!(heard, Some(Answer::Committed { at: Some(50), .. })),
                "{protocol:?} {order:?}: {heard:?}"
            );
            assert_eq!(row_stamps(&mut groups), vec![Some(50); 3]);
        }
        for again in ORDERS {
            let config = across_three(Protocol::Parallel);
            let mut groups = three_groups();
            let (mut first, events) = Coordinator::start(config.clone()).unwrap();
            let (heard, late) = relayed(
                &mut groups,
                &mut first,
                events,
                Some(a_key_in_each()),
                &order,
                |_, step| match step {
                    Step::DecideAt { .. } | Step::ResolveAt { .. } => Fate::Lose,
                    _ => Fate::Land,
                },
            );
            let Some(Answer::Committed { at, .. }) = heard else {
                panic!("{order:?}: {heard:?}");
            };
            assert_eq!(at, Some(50), "{order:?}");
            assert_eq!(row_stamps(&mut groups), vec![None; 3]);
            let (mut second, events) = Coordinator::start(config).unwrap();
            relayed(&mut groups, &mut second, events, None, &again, |_, _| {
                Fate::Land
            });
            assert_eq!(row_stamps(&mut groups), vec![at; 3], "{order:?} {again:?}");
            assert_eq!(late.len(), 4, "its decision and three resolutions");
            for (group, mut envelope) in late {
                if let Step::DecideAt { at, .. } | Step::ResolveAt { at, .. } = &mut envelope.step {
                    *at = 21;
                }
                let tablet = &mut groups[usize::try_from(group).unwrap()];
                assert_eq!(apply(group, tablet, &envelope).unwrap(), Reply::Fenced);
            }
            assert_eq!(row_stamps(&mut groups), vec![at; 3], "{order:?} {again:?}");
        }
    }
}

/// T-T1.9.R30, groups that missed the stamp. Do: the client answered at 50, the coordinator gone
/// with its decision landed or lost and its resolution landed in each of the eight sets of groups,
/// both protocols (two-phase commit answers only after its decision landed); a recovery, and a
/// second recovery after it. Expect: every row at 50, no lock left, and the second finds nothing.
#[test]
fn t_t1_9_r30_whichever_groups_missed_the_stamp_recover_at_it() {
    for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
        for decided in [false, true] {
            if !decided && protocol == Protocol::TwoPhase {
                continue;
            }
            for landed in 0u32..8 {
                let resolved = |group: GroupId| landed & (1 << group) != 0;
                let config = across_three(protocol);
                let mut groups = three_groups();
                let (mut first, events) = Coordinator::start(config.clone()).unwrap();
                let (heard, _) = relayed(
                    &mut groups,
                    &mut first,
                    events,
                    Some(a_key_in_each()),
                    &ORDERS[0],
                    |group, step| match step {
                        Step::DecideAt { .. } if !decided => Fate::Lose,
                        Step::ResolveAt { .. } if !resolved(group) => Fate::Lose,
                        _ => Fate::Land,
                    },
                );
                let Some(Answer::Committed { at, .. }) = heard else {
                    panic!("{protocol:?} {decided} {landed:03b}: {heard:?}");
                };
                assert_eq!(at, Some(50));
                let expected: Vec<_> = (0..3).map(|g| resolved(g).then_some(50)).collect();
                assert_eq!(row_stamps(&mut groups), expected);
                for finished in [false, true] {
                    let (mut next, events) = Coordinator::start(config.clone()).unwrap();
                    relayed(&mut groups, &mut next, events, None, &ORDERS[5], |_, _| {
                        Fate::Land
                    });
                    assert_eq!(
                        row_stamps(&mut groups),
                        vec![at; 3],
                        "{protocol:?} {decided} {landed:03b}"
                    );
                    assert!(groups.iter().all(|g| g.held().is_empty()));
                    let recovered = u64::from(!finished && (landed != 7 || !decided));
                    assert_eq!(
                        next.counts().recovered_committed,
                        recovered,
                        "{protocol:?} {decided} {landed:03b}"
                    );
                }
            }
        }
    }
}

/// T-T1.9.R30, a group that keeps no clock among the participants, on real owners. Do: a backed-up
/// group and one that isn't; a Parallel Commit across them answered, the coordinator gone before
/// any decision or resolution, and its successor. Expect: the unbacked group's lock reply carries
/// no stamp and it keeps no vote, the client hears the backed-up group's, and the recovery writes
/// both rows at exactly that.
#[test]
fn t_t1_9_r30_a_group_that_keeps_no_clock_recovers_at_the_others_vote() {
    use crate::backup::{Archive, CheckPolicy, ChunkGrowth, FolderBlobs};
    use std::cell::Cell;
    use std::time::Instant;

    let until = || Instant::now() + crate::transport::HANG;
    let dir = tempfile::tempdir().unwrap();
    let archive = Archive {
        blobs: Arc::new(FolderBlobs::open(&dir.path().join("archive"), 64 * 1024).unwrap()),
        prefix: "group0".into(),
        clock: Arc::new(|| 21 * MICRO_NS),
        memory: 32 * 1024,
        patience: crate::transport::HANG,
        check: CheckPolicy::Off,
        chunks: ChunkGrowth::Keep,
    };
    let (backed, _shipping) =
        Database::open_backed_up(&dir.path().join("g0.db"), archive, until()).unwrap();
    let plain = Database::open(&dir.path().join("g1.db"), until()).unwrap();
    let groups = [backed, plain];
    for group in &groups {
        sql::prepare(group, until()).unwrap();
    }
    let carried = Cell::new(0);
    let serve = |group: GroupId, envelope: &Envelope| {
        let (reply, latest, vote) = sql::serve(
            &groups[usize::try_from(group).unwrap()],
            group,
            carried.get(),
            envelope.clone(),
            until(),
        )
        .unwrap();
        carried.set(carried.get().max(latest.unwrap_or(0)));
        (reply, vote)
    };
    let config = Config {
        id: 7,
        protocol: Protocol::Parallel,
        registry: 0,
        groups: vec![0, 1],
        in_flight: 1,
    };
    let keys = [0, 1].map(|g| sql::record_key("n", "f", &[g]).unwrap());
    let tx = Txn {
        reads: vec![],
        writes: keys
            .iter()
            .map(|key| (key.clone(), Some(b"v".to_vec())))
            .collect(),
        conds: vec![],
    };
    let (mut first, events) = Coordinator::start(config.clone()).unwrap();
    let mut votes = BTreeMap::new();
    let mut locking = None;
    let answer = drive_on(
        &mut first,
        events,
        Some(tx),
        |group, envelope| {
            let (reply, vote) = serve(group, envelope);
            if matches!(reply, Reply::Locked { .. }) {
                locking = envelope.step.txn();
                votes.insert(group, vote);
            }
            (reply, vote)
        },
        |step| match step {
            Step::Decide { .. }
            | Step::DecideAt { .. }
            | Step::Resolve { .. }
            | Step::ResolveAt { .. } => Fate::Crash,
            _ => Fate::Land,
        },
    );
    let Some((Answer::Committed { at, .. }, _)) = answer else {
        panic!("the implicit commit didn't answer: {answer:?}");
    };
    assert!(at.is_some());
    assert_eq!(votes, BTreeMap::from([(0, at), (1, None)]));
    let id = locking.unwrap();
    for (group, vote) in groups.iter().zip([at, None]) {
        group
            .call_internal(until(), AnswerMax::fixed(), move |db| {
                let mut tablet = SqlTablet::new(db);
                assert_eq!(tablet.locks_of(id)?.len(), 1);
                assert_eq!(tablet.lock_stamp(id)?, vote);
                Ok(())
            })
            .unwrap();
    }
    carried.set(0);
    let (mut second, events) = Coordinator::start(config).unwrap();
    drive_on(&mut second, events, None, serve, |_| Fate::Land);
    assert_eq!(second.counts().recovered_committed, 1);
    for (group, key) in groups.iter().zip(keys) {
        group
            .call_internal(until(), AnswerMax::fixed(), move |db| {
                let mut tablet = SqlTablet::new(db);
                assert!(tablet.locks_of(id)?.is_empty());
                assert_eq!(tablet.value(&key)?, Some(b"v".to_vec()));
                assert_eq!(tablet.stamp(&key)?, at);
                Ok(())
            })
            .unwrap();
    }
}

/// T-T1.9.R30, the wire only grows (local-owner-transport.md, "Negotiated versions"). Do: encode
/// each step and reply. Expect: each variant under the number it was first sent with, and the
/// stamped ones byte for byte.
#[test]
fn t_t1_9_r30_steps_and_replies_keep_their_wire_numbers() {
    let id = txn(2);
    let outcome = Outcome::Committed;
    let lock = lock_step(id, &[], &[], None);
    let lock_when = lock_if(id, &[], &[]);
    let steps = [
        (0, Step::Begin),
        (
            1,
            Step::Scan {
                below: 1,
                after: None,
            },
        ),
        (2, lock),
        (3, Step::Decide { txn: id, outcome }),
        (4, Step::Recover { txn: id }),
        (5, Step::Prevent { txn: id }),
        (6, Step::Resolve { txn: id, outcome }),
        (7, lock_when),
        (
            8,
            Step::ResolveAt {
                txn: id,
                outcome,
                at: 50,
            },
        ),
        (
            9,
            Step::DecideAt {
                txn: id,
                outcome,
                at: 50,
            },
        ),
        (10, Step::RecoverAt { txn: id }),
        (11, Step::PreventAt { txn: id }),
    ];
    for (number, step) in &steps {
        // Every variant, so one added without its number doesn't compile.
        match step {
            Step::Begin
            | Step::Scan { .. }
            | Step::Lock { .. }
            | Step::Decide { .. }
            | Step::Recover { .. }
            | Step::Prevent { .. }
            | Step::Resolve { .. }
            | Step::LockIf { .. }
            | Step::ResolveAt { .. }
            | Step::DecideAt { .. }
            | Step::RecoverAt { .. }
            | Step::PreventAt { .. } => {}
        }
        assert_eq!(
            postcard::to_allocvec(step).unwrap().first(),
            Some(number),
            "{step:?}"
        );
    }
    let replies = [
        (0, Reply::Epoch(1)),
        (
            1,
            Reply::Open {
                txns: Vec::new(),
                more: false,
            },
        ),
        (2, Reply::Locked { reads: Vec::new() }),
        (3, Reply::Conflict { key: Vec::new() }),
        (4, Reply::Late),
        (5, Reply::Fenced),
        (6, Reply::Decided(outcome)),
        (7, Reply::Staging(Vec::new())),
        (8, Reply::Present(true)),
        (9, Reply::Resolved),
        (
            10,
            Reply::Unmet {
                key: Vec::new(),
                at: None,
            },
        ),
        (
            11,
            Reply::DecidedAt {
                outcome,
                at: Some(50),
            },
        ),
        (
            12,
            Reply::PresentAt {
                present: true,
                committed: None,
                locked: Some(50),
            },
        ),
    ];
    for (number, reply) in &replies {
        match reply {
            Reply::Epoch(_)
            | Reply::Open { .. }
            | Reply::Locked { .. }
            | Reply::Conflict { .. }
            | Reply::Late
            | Reply::Fenced
            | Reply::Decided(_)
            | Reply::Staging(_)
            | Reply::Present(_)
            | Reply::Resolved
            | Reply::Unmet { .. }
            | Reply::DecidedAt { .. }
            | Reply::PresentAt { .. } => {}
        }
        assert_eq!(
            postcard::to_allocvec(reply).unwrap().first(),
            Some(number),
            "{reply:?}"
        );
    }
    let bytes = |step: &Step| postcard::to_allocvec(step).unwrap();
    assert_eq!(bytes(&steps[8].1), [8, 7, 1, 2, 0, 50]);
    assert_eq!(bytes(&steps[9].1), [9, 7, 1, 2, 0, 50]);
    assert_eq!(bytes(&steps[10].1), [10, 7, 1, 2]);
    assert_eq!(bytes(&steps[11].1), [11, 7, 1, 2]);
    let bytes = |reply: &Reply| postcard::to_allocvec(reply).unwrap();
    assert_eq!(bytes(&replies[11].1), [11, 0, 1, 50]);
    assert_eq!(bytes(&replies[12].1), [12, 1, 0, 1, 50]);
    // A lock's reply carries nothing new: its vote rides beside it, as its clock did.
    assert_eq!(bytes(&Reply::Locked { reads: Vec::new() }), [2, 0]);
}
