//! Transactions across groups (CP-T1.10): the group's part on both tablets (the conformance suite),
//! a coordinator crash at every step (T-T1.10.1), and seeded simulations under lost and late steps
//! and crashes, checked Elle-style (AC-T1.10.5).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use super::sql::{SCHEMA, SqlTablet};
use super::*;
use crate::db::Db;

fn txn(seq: u64) -> TxnId {
    TxnId {
        coordinator: 7,
        epoch: 1,
        seq,
    }
}

fn env(epoch: u64, floor: Option<Mark>, step: Step) -> Envelope {
    Envelope {
        coordinator: 7,
        epoch,
        floor,
        step,
    }
}

fn lock_step(
    id: TxnId,
    reads: &[&[u8]],
    writes: &[(&[u8], &[u8])],
    stage: Option<Vec<GroupId>>,
) -> Step {
    Step::Lock {
        txn: id,
        anchor: 0,
        reads: reads.iter().map(|k| k.to_vec()).collect(),
        writes: writes
            .iter()
            .map(|(k, v)| (k.to_vec(), Some(v.to_vec())))
            .collect(),
        stage,
    }
}

/// The group's rules, the same on every tablet (CLAUDE.md §9's conformance suite).
fn conformance(t: &mut dyn Tablet) {
    let at =
        |t: &mut dyn Tablet, epoch, floor, step| apply(0, t, &env(epoch, floor, step)).unwrap();
    assert_eq!(
        apply(0, t, &env(0, None, Step::Begin)).unwrap(),
        Reply::Epoch(1)
    );
    // A write lock holds out a reader and a writer; reads share.
    assert_eq!(
        at(
            t,
            1,
            None,
            lock_step(txn(0), &[b"k"], &[(b"k", b"a")], Some(vec![0, 1]))
        ),
        Reply::Locked {
            reads: vec![(b"k".to_vec(), None)]
        }
    );
    assert_eq!(
        at(t, 1, None, lock_step(txn(1), &[b"k"], &[], None)),
        Reply::Conflict { key: b"k".to_vec() }
    );
    assert_eq!(
        at(t, 1, None, lock_step(txn(1), &[b"r"], &[], None)),
        Reply::Locked {
            reads: vec![(b"r".to_vec(), None)]
        }
    );
    assert_eq!(
        at(t, 1, None, lock_step(txn(2), &[b"r"], &[], None)),
        Reply::Locked {
            reads: vec![(b"r".to_vec(), None)]
        }
    );
    assert_eq!(
        at(t, 1, None, lock_step(txn(3), &[], &[(b"r", b"x")], None)),
        Reply::Conflict { key: b"r".to_vec() }
    );
    // A staging record names its participants; a second staging is refused Late.
    assert_eq!(
        at(t, 1, None, Step::Recover { txn: txn(0) }),
        Reply::Staging(vec![0, 1])
    );
    assert_eq!(
        at(
            t,
            1,
            None,
            lock_step(txn(0), &[], &[(b"z", b"z")], Some(vec![0]))
        ),
        Reply::Late
    );
    // Prevention: a transaction holding locks is present; one without never will.
    assert_eq!(
        at(t, 1, None, Step::Prevent { txn: txn(0) }),
        Reply::Present(true)
    );
    assert_eq!(
        at(t, 1, None, Step::Prevent { txn: txn(4) }),
        Reply::Present(false)
    );
    assert_eq!(
        at(t, 1, None, lock_step(txn(4), &[], &[(b"q", b"q")], None)),
        Reply::Late
    );
    // A decision stands; resolution applies writes once and unlocks.
    assert_eq!(
        at(
            t,
            1,
            None,
            Step::Decide {
                txn: txn(0),
                outcome: Outcome::Committed
            }
        ),
        Reply::Decided(Outcome::Committed)
    );
    assert_eq!(
        at(
            t,
            1,
            None,
            Step::Decide {
                txn: txn(0),
                outcome: Outcome::Aborted
            }
        ),
        Reply::Decided(Outcome::Committed)
    );
    assert_eq!(
        at(
            t,
            1,
            None,
            Step::Resolve {
                txn: txn(0),
                outcome: Outcome::Committed
            }
        ),
        Reply::Resolved
    );
    assert_eq!(
        at(
            t,
            1,
            None,
            Step::Resolve {
                txn: txn(0),
                outcome: Outcome::Committed
            }
        ),
        Reply::Resolved
    );
    assert_eq!(t.value(b"k").unwrap(), Some(b"a".to_vec()));
    assert!(t.locks(b"k").unwrap().is_empty());
    assert_eq!(
        at(t, 1, None, lock_step(txn(5), &[b"k"], &[], None)),
        Reply::Locked {
            reads: vec![(b"k".to_vec(), Some(b"a".to_vec()))]
        }
    );
    // No record: recovery aborts it, for good.
    assert_eq!(
        at(t, 1, None, Step::Recover { txn: txn(6) }),
        Reply::Decided(Outcome::Aborted)
    );
    assert_eq!(
        at(
            t,
            1,
            None,
            Step::Decide {
                txn: txn(6),
                outcome: Outcome::Committed
            }
        ),
        Reply::Decided(Outcome::Aborted)
    );
    // A scan lists the old incarnation's open transactions and fences it.
    let open = at(
        t,
        2,
        None,
        Step::Scan {
            below: 2,
            after: None,
        },
    );
    let Reply::Open { txns, more } = open else {
        panic!("{open:?}")
    };
    assert!(!more);
    let listed: Vec<u64> = txns.iter().map(|(id, _)| id.seq).collect();
    assert_eq!(listed, vec![1, 2, 5]);
    assert_eq!(at(t, 1, None, Step::Prevent { txn: txn(1) }), Reply::Fenced);
    // The floor: steps below it are Late, and done rows below it are forgotten.
    for seq in [1, 2, 5] {
        assert_eq!(
            at(
                t,
                2,
                None,
                Step::Resolve {
                    txn: txn(seq),
                    outcome: Outcome::Aborted
                }
            ),
            Reply::Resolved
        );
    }
    let floor = Some(Mark { epoch: 2, seq: 0 });
    let next = TxnId {
        coordinator: 7,
        epoch: 2,
        seq: 0,
    };
    assert_eq!(
        at(t, 2, floor, Step::Prevent { txn: next }),
        Reply::Present(false)
    );
    assert_eq!(
        at(
            t,
            2,
            floor,
            Step::Resolve {
                txn: txn(5),
                outcome: Outcome::Aborted
            }
        ),
        Reply::Late
    );
    assert_eq!(t.done(txn(4)).unwrap(), None);
    assert_eq!(t.record(txn(6)).unwrap(), None);
    assert_eq!(t.done(next).unwrap(), Some(Outcome::Aborted));
    assert_eq!(
        apply(0, t, &env(0, None, Step::Begin)).unwrap(),
        Reply::Epoch(2)
    );
}

#[test]
fn t_t1_10_c_the_group_rules_hold_in_memory() {
    conformance(&mut Memory::default());
}

#[test]
fn t_t1_10_c_the_group_rules_hold_in_a_groups_database() {
    let io = Arc::new(amphora_engine::MemoryIO::new());
    let db = Db::open_with(Path::new(":memory:xgroup"), io).unwrap();
    db.exec(SCHEMA).unwrap();
    conformance(&mut SqlTablet::new(&db));
}

fn lock_if(id: TxnId, writes: &[(&[u8], &[u8])], conds: &[(&[u8], Option<u64>)]) -> Step {
    Step::LockIf {
        txn: id,
        anchor: 0,
        reads: vec![],
        writes: writes
            .iter()
            .map(|(k, v)| (k.to_vec(), Some(v.to_vec())))
            .collect(),
        stage: None,
        conds: conds.iter().map(|(k, s)| (k.to_vec(), *s)).collect(),
    }
}

/// T-T1.9.R27 (CP-T1.9; reshard.md, "Decision (lane split-merge-20): conditions across groups
/// name commit stamps"), a group's part on the simulator's tablet. Do: k written (its stamp kept),
/// then written again; lock steps with conditions: k as of its first stamp; k as of its new one
/// and m absent, writing w; a write lock on m from another transaction; a step writing w on k's
/// first stamp; and one on k absent. Expect: the first unmet at k's new stamp with nothing locked;
/// the second locked, k and m read-locked beside w's write; the write lock on m refused, a
/// conflict; the step on w a conflict before its condition is looked at; the last unmet, k there.
/// T-T1.9.R28 (lane split-merge-21; reshard.md, "Decision (lane split-merge-21): one clock, one
/// commit stamp"), on the simulator's group. Do: two groups, the second's clock ahead of the first's; one
/// transaction locks a key in each and resolves at one commit stamp in both; then each writes a
/// key of its own. Expect: both keys stamped the one commit stamp; each group's next write stamped
/// after it; a transaction resolved with `Step::Resolve` (no stamp) stamped by its group's clock.
#[test]
fn t_t1_9_r28_one_transaction_has_one_stamp_in_every_group() {
    let mut groups = [Memory::default(), Memory::default()];
    // The second group's clock is ahead: 40 writes before.
    for i in 0..40u8 {
        groups[1].set_value(&[b'z', i], Some(b"v")).unwrap();
    }
    let stamp = 1_000;
    for (g, t) in groups.iter_mut().enumerate() {
        let key = [b'k', u8::try_from(g).unwrap()];
        let locked = apply(
            0,
            t,
            &env(1, None, lock_step(txn(1), &[], &[(&key, b"x")], None)),
        );
        assert!(matches!(locked.unwrap(), Reply::Locked { .. }));
        let resolve = Step::ResolveAt {
            txn: txn(1),
            outcome: Outcome::Committed,
            at: stamp,
        };
        assert_eq!(
            apply(0, t, &env(1, None, resolve)).unwrap(),
            Reply::Resolved
        );
        assert_eq!(t.stamp(&key).unwrap(), Some(stamp), "group {g}");
        t.set_value(b"after", Some(b"v")).unwrap();
        assert!(t.stamp(b"after").unwrap() > Some(stamp), "group {g}");
    }
    let t = &mut groups[0];
    apply(
        0,
        t,
        &env(1, None, lock_step(txn(2), &[], &[(b"m", b"x")], None)),
    )
    .unwrap();
    let resolve = Step::Resolve {
        txn: txn(2),
        outcome: Outcome::Committed,
    };
    assert_eq!(
        apply(0, t, &env(1, None, resolve)).unwrap(),
        Reply::Resolved
    );
    assert!(t.stamp(b"m").unwrap() > t.stamp(b"k\x00").unwrap());
}

#[test]
fn t_t1_9_r27_a_lock_step_holds_to_its_conditions() {
    let mut t = Memory::default();
    let at = |t: &mut Memory, step: Step| apply(0, t, &env(1, None, step)).unwrap();
    assert_eq!(
        apply(0, &mut t, &env(0, None, Step::Begin)).unwrap(),
        Reply::Epoch(1)
    );
    t.set_value(b"k", Some(b"1")).unwrap();
    let first = t.stamp(b"k").unwrap();
    t.set_value(b"k", Some(b"2")).unwrap();
    let now = t.stamp(b"k").unwrap();
    assert!(now > first, "{now:?} {first:?}");

    assert_eq!(
        at(&mut t, lock_if(txn(1), &[(b"w", b"x")], &[(b"k", first)])),
        Reply::Unmet {
            key: b"k".to_vec(),
            at: now
        }
    );
    assert!(t.held().is_empty(), "{:?}", t.held());
    assert_eq!(
        at(
            &mut t,
            lock_if(txn(2), &[(b"w", b"x")], &[(b"k", now), (b"m", None)])
        ),
        Reply::Locked { reads: vec![] }
    );
    let held: Vec<(Vec<u8>, Intent)> = t
        .held()
        .iter()
        .filter(|((_, of), _)| *of == txn(2))
        .map(|((key, _), lock)| (key.clone(), lock.intent.clone()))
        .collect();
    assert_eq!(
        held,
        vec![
            (b"k".to_vec(), Intent::Read),
            (b"m".to_vec(), Intent::Read),
            (b"w".to_vec(), Intent::Put(b"x".to_vec())),
        ]
    );
    assert_eq!(
        at(&mut t, lock_step(txn(3), &[], &[(b"m", b"y")], None)),
        Reply::Conflict { key: b"m".to_vec() }
    );
    assert_eq!(
        at(&mut t, lock_if(txn(4), &[(b"w", b"z")], &[(b"k", first)])),
        Reply::Conflict { key: b"w".to_vec() }
    );
    assert_eq!(
        at(&mut t, lock_if(txn(5), &[], &[(b"k", None)])),
        Reply::Unmet {
            key: b"k".to_vec(),
            at: now
        }
    );
}

// ---- The simulator: groups in memory, steps carried with seeded latencies, losses and lateness,
// the coordinator crashed and started again.

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

enum Msg {
    Apply {
        life: u64,
        send: Send,
        lose_reply: bool,
    },
    Reply {
        life: u64,
        id: u64,
        reply: Result<Reply, Lost>,
    },
}

/// The keys a transaction read and the values it saw (`None`: absent).
type Reads = Vec<(Vec<u8>, Option<Vec<u8>>)>;

struct Sim {
    rng: Rng,
    groups: Vec<Memory>,
    config: Config,
    coordinator: Option<Coordinator>,
    life: u64,
    queue: BTreeMap<(u64, u64), Msg>,
    now: u64,
    seq: u64,
    /// Per mille of steps lost, their reply lost, or landing late.
    faults: u64,
    inputs: u64,
    applied_reads: BTreeMap<TxnId, Reads>,
    finished: BTreeMap<TxnId, Outcome>,
    /// What the groups applied: each transaction's resolution, the truth the check reads. A
    /// coordinator that crashes after its resolutions land never reports the transaction
    /// [`Event::Finished`], and no later incarnation finds it (nothing of it is left open).
    resolved: BTreeMap<TxnId, Outcome>,
    /// Transactions resolved one way in one group and the other way in another.
    split: BTreeSet<TxnId>,
    answers: BTreeMap<TxnId, (Answer, u64, u32)>,
    rounds: Vec<u32>,
    /// A group that failed (its quorum lost): it takes no step until it is back (T-T1.10.X3).
    down: Option<GroupId>,
    /// The step in flight as `down` failed landed first, its answer lost.
    land_on_failure: bool,
}

impl Sim {
    fn new(seed: u64, groups: u32, protocol: Protocol, faults: u64, in_flight: usize) -> Self {
        Self {
            rng: Rng(seed),
            groups: (0..groups).map(|_| Memory::default()).collect(),
            config: Config {
                id: 7,
                protocol,
                registry: 0,
                groups: (0..groups).collect(),
                in_flight,
            },
            coordinator: None,
            life: 0,
            queue: BTreeMap::new(),
            now: 0,
            seq: 0,
            faults,
            inputs: 0,
            applied_reads: BTreeMap::new(),
            finished: BTreeMap::new(),
            resolved: BTreeMap::new(),
            split: BTreeSet::new(),
            answers: BTreeMap::new(),
            rounds: Vec::new(),
            down: None,
            land_on_failure: false,
        }
    }

    fn at(&mut self, delay: u64, msg: Msg) {
        self.seq += 1;
        self.queue.insert((self.now + delay, self.seq), msg);
    }

    fn latency(&mut self) -> u64 {
        1 + self.rng.below(3)
    }

    fn start(&mut self) {
        self.life += 1;
        let (coordinator, events) = Coordinator::start(self.config.clone()).unwrap();
        self.coordinator = Some(coordinator);
        self.inputs += 1;
        self.dispatch(events);
    }

    fn crash(&mut self) {
        self.coordinator = None;
        self.life += 1;
    }

    fn dispatch(&mut self, events: Vec<Event>) {
        for event in events {
            match event {
                Event::Send(send) => {
                    let life = self.life;
                    let roll = self.rng.below(1000);
                    let lat = self.latency();
                    if roll < self.faults / 3 {
                        // Never lands.
                        self.at(
                            lat,
                            Msg::Reply {
                                life,
                                id: send.id,
                                reply: Err(Lost),
                            },
                        );
                    } else if roll < self.faults * 2 / 3 {
                        self.at(
                            lat,
                            Msg::Apply {
                                life,
                                send,
                                lose_reply: true,
                            },
                        );
                    } else if roll < self.faults {
                        // Answered lost first, lands after.
                        self.at(
                            lat,
                            Msg::Reply {
                                life,
                                id: send.id,
                                reply: Err(Lost),
                            },
                        );
                        let later = lat + self.latency();
                        self.at(
                            later,
                            Msg::Apply {
                                life,
                                send,
                                lose_reply: true,
                            },
                        );
                    } else {
                        self.at(
                            lat,
                            Msg::Apply {
                                life,
                                send,
                                lose_reply: false,
                            },
                        );
                    }
                }
                Event::Answer {
                    txn,
                    answer,
                    rounds,
                } => {
                    self.rounds.push(rounds);
                    self.answers.insert(txn, (answer, self.now, rounds));
                }
                Event::Finished { txn, outcome, .. } => {
                    assert!(
                        self.finished.insert(txn, outcome).is_none(),
                        "{txn:?} finished twice"
                    );
                }
                Event::Ready => {}
                Event::Fenced => self.coordinator = None,
            }
        }
    }

    /// One message; false once none is left.
    fn step(&mut self) -> bool {
        let Some(((time, _), msg)) = self.queue.pop_first() else {
            return false;
        };
        self.now = time;
        // A failed group takes no step: the step's call is refused, and the shell stops that
        // incarnation, as `Cluster`'s driver does on a refusal it can't retry. The step in flight
        // as it failed may have landed first, its answer lost.
        let msg = match msg {
            Msg::Apply { life, send, .. } if self.down == Some(send.group) => {
                if life == self.life {
                    self.crash();
                }
                if !std::mem::take(&mut self.land_on_failure) {
                    return true;
                }
                Msg::Apply {
                    life,
                    send,
                    lose_reply: true,
                }
            }
            msg => msg,
        };
        match msg {
            Msg::Apply {
                life,
                send,
                lose_reply,
            } => {
                let group = self
                    .groups
                    .get_mut(usize::try_from(send.group).unwrap())
                    .unwrap();
                let reply = apply(send.group, group, &send.envelope).unwrap();
                if let (
                    Step::Resolve { txn, outcome } | Step::ResolveAt { txn, outcome, .. },
                    Reply::Resolved,
                ) = (&send.envelope.step, &reply)
                    && *self.resolved.entry(*txn).or_insert(*outcome) != *outcome
                {
                    self.split.insert(*txn);
                }
                if let (Some(txn), Reply::Locked { reads }) = (send.envelope.step.txn(), &reply) {
                    self.applied_reads
                        .entry(txn)
                        .or_default()
                        .extend(reads.iter().cloned());
                }
                let lat = self.latency();
                let reply = if lose_reply { Err(Lost) } else { Ok(reply) };
                self.at(
                    lat,
                    Msg::Reply {
                        life,
                        id: send.id,
                        reply,
                    },
                );
            }
            Msg::Reply { life, id, reply } => {
                if life == self.life
                    && let Some(coordinator) = self.coordinator.as_mut()
                {
                    // The simulator's groups keep no clock.
                    let events = coordinator.reply(id, reply, None).unwrap();
                    self.inputs += 1;
                    self.dispatch(events);
                }
            }
        }
        true
    }

    fn ready(&self) -> bool {
        self.coordinator.as_ref().is_some_and(Coordinator::ready)
    }

    fn begin(&mut self, t: Txn) -> Option<TxnId> {
        let groups = u32::try_from(self.groups.len()).unwrap();
        let coordinator = self.coordinator.as_mut()?;
        let (id, events) = coordinator
            .begin(t, |key| u32::from(key[0]) % groups)
            .ok()?;
        self.inputs += 1;
        self.dispatch(events);
        Some(id)
    }

    /// Runs out every message, starting a crashed coordinator again, until it has nothing left.
    fn settle(&mut self) {
        for _ in 0..1_000_000 {
            if !self.step() {
                if self.coordinator.is_none() {
                    self.start();
                    continue;
                }
                return;
            }
        }
        panic!("the simulation made no end");
    }

    /// Committed or aborted everywhere: no lock and no staging record left in any group.
    fn assert_settled(&self) {
        assert!(self.ready());
        assert_eq!(self.coordinator.as_ref().unwrap().in_flight(), 0);
        for (g, group) in self.groups.iter().enumerate() {
            assert!(
                group.held().is_empty(),
                "group {g} still holds {:?}",
                group.held()
            );
            assert!(
                !group
                    .records()
                    .values()
                    .any(|r| matches!(r, Record::Staging(_))),
                "group {g} still has a staging record"
            );
        }
    }
}

fn value_of(groups: &[Memory], key: &[u8]) -> Option<Vec<u8>> {
    groups.iter().find_map(|g| g.data().get(key).cloned())
}

/// One transaction over three groups: a read-modify-write in each and a read, with a coordinator
/// crash after its `crash`th input, the steps it had out then landing or lost (`land`).
fn crash_once(
    protocol: Protocol,
    crash: Option<u64>,
    land: bool,
    crash_again: Option<u64>,
) -> (Sim, u64) {
    let mut sim = Sim::new(1, 3, protocol, 0, 4);
    sim.start();
    while !sim.ready() {
        assert!(sim.step());
    }
    let base = sim.inputs;
    let t = Txn {
        reads: vec![vec![0], vec![1], vec![2], vec![3]],
        writes: vec![
            (vec![0], Some(b"a".to_vec())),
            (vec![1], Some(b"b".to_vec())),
            (vec![2], Some(b"c".to_vec())),
        ],
        conds: Vec::new(),
    };
    let id = sim.begin(t).unwrap();
    let mut crashes = [crash, crash_again].into_iter().flatten().peekable();
    loop {
        if let Some(at) = crashes.peek().copied()
            && sim.coordinator.is_some()
            && sim.inputs - base >= at
        {
            crashes.next();
            sim.crash();
            if !land {
                sim.queue.clear();
            }
            sim.start();
            continue;
        }
        if !sim.step() {
            break;
        }
    }
    sim.settle();
    sim.assert_settled();
    let written: Vec<Option<Vec<u8>>> = (0..3u8).map(|k| value_of(&sim.groups, &[k])).collect();
    let all = written.iter().all(Option::is_some);
    let none = written.iter().all(Option::is_none);
    assert!(
        all || none,
        "{protocol:?}, crash at {crash:?}/{crash_again:?}, land {land}: partly applied {written:?}"
    );
    match sim.answers.get(&id).map(|(a, _, _)| a) {
        Some(Answer::Committed { .. }) => assert!(all, "answered committed, not applied"),
        Some(Answer::Aborted(_)) => assert!(none, "answered aborted, applied"),
        Some(Answer::Unknown) | None => {}
    }
    let inputs = sim.inputs - base;
    (sim, inputs)
}

#[test]
fn t_t1_10_1_a_coordinator_crash_at_every_step_ends_committed_or_aborted_everywhere() {
    for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
        let (clean, steps) = crash_once(protocol, None, true, None);
        assert!(matches!(
            clean.answers.values().next(),
            Some((Answer::Committed { .. }, _, _))
        ));
        let mut committed = 0;
        let mut aborted = 0;
        for at in 0..=steps {
            for land in [true, false] {
                let (sim, _) = crash_once(protocol, Some(at), land, None);
                match value_of(&sim.groups, &[0]) {
                    Some(_) => committed += 1,
                    None => aborted += 1,
                }
                // The recovering incarnation crashed too, at each of its steps.
                for again in at + 1..=at + steps {
                    crash_once(protocol, Some(at), land, Some(again));
                }
            }
        }
        // Non-vacuity: the crashes found both ends.
        assert!(
            committed > 0 && aborted > 0,
            "{protocol:?}: {committed} committed, {aborted} aborted"
        );
    }
}

/// Group `group` fails at the coordinator's `at`th input (the step in flight then landed first if
/// `land`) and stays down until nothing else is left to run; then it is back, and a new incarnation
/// finishes what the stopped one left (T-T1.10.X3).
fn fail_group_once(protocol: Protocol, group: GroupId, at: u64, land: bool) -> Sim {
    let mut sim = Sim::new(1, 3, protocol, 0, 4);
    sim.start();
    while !sim.ready() {
        assert!(sim.step());
    }
    let base = sim.inputs;
    let t = Txn {
        reads: vec![vec![0], vec![1], vec![2], vec![3]],
        writes: vec![
            (vec![0], Some(b"a".to_vec())),
            (vec![1], Some(b"b".to_vec())),
            (vec![2], Some(b"c".to_vec())),
        ],
        conds: Vec::new(),
    };
    let id = sim.begin(t).unwrap();
    let mut failed = false;
    loop {
        if !failed && sim.inputs - base >= at {
            failed = true;
            sim.down = Some(group);
            sim.land_on_failure = land;
        }
        if !sim.step() {
            break;
        }
    }
    sim.down = None;
    sim.settle();
    sim.assert_settled();
    let written: Vec<Option<Vec<u8>>> = (0..3u8).map(|k| value_of(&sim.groups, &[k])).collect();
    let all = written.iter().all(Option::is_some);
    let none = written.iter().all(Option::is_none);
    assert!(
        all || none,
        "{protocol:?}, group {group} failed at {at}, land {land}: partly applied {written:?}"
    );
    assert!(sim.split.is_empty(), "{protocol:?}: resolved both ways");
    match sim.answers.get(&id).map(|(a, _, _)| a) {
        Some(Answer::Committed { .. }) => assert!(all, "answered committed, not applied"),
        Some(Answer::Aborted(_)) => assert!(none, "answered aborted, applied"),
        Some(Answer::Unknown) | None => {}
    }
    sim
}

#[test]
fn t_t1_10_x3_a_group_failure_at_every_step_ends_committed_or_aborted_everywhere() {
    for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
        let (_, steps) = crash_once(protocol, None, true, None);
        let (mut committed, mut aborted) = (0, 0);
        // The registry and anchor (group 0) and each participant, at every input.
        for group in 0..3 {
            for at in 0..=steps {
                for land in [true, false] {
                    let sim = fail_group_once(protocol, group, at, land);
                    match value_of(&sim.groups, &[0]) {
                        Some(_) => committed += 1,
                        None => aborted += 1,
                    }
                }
            }
        }
        // Non-vacuity: the failures found both ends.
        assert!(
            committed > 0 && aborted > 0,
            "{protocol:?}: {committed} committed, {aborted} aborted"
        );
    }
}

#[test]
fn ac_t1_10_2_parallel_commits_answers_after_one_group_commit_two_phase_after_two() {
    for (protocol, rounds) in [(Protocol::TwoPhase, 2), (Protocol::Parallel, 1)] {
        let (sim, _) = crash_once(protocol, None, true, None);
        assert_eq!(sim.rounds, vec![rounds], "{protocol:?}");
    }
}

// ---- Seeded runs with an Elle-style check (Kingsbury and Alvaro, PVLDB 14(3)): every write is a
// read-modify-write of a unique value, so each key's versions form a chain recovered from what
// transactions read, and the dependency graph (ww, wr, rw, real time) must have no cycle.

struct Op {
    id: TxnId,
    invoke: u64,
    writes: BTreeMap<Vec<u8>, Vec<u8>>,
}

fn check(sim: &Sim, ops: &[Op]) -> Vec<String> {
    let mut anomalies = Vec::new();
    let mut writer: BTreeMap<Vec<u8>, TxnId> = BTreeMap::new();
    for op in ops {
        for value in op.writes.values() {
            writer.insert(value.clone(), op.id);
        }
    }
    // What the groups applied decides; a transaction no group resolved committed is aborted.
    let outcome = |id: &TxnId| sim.resolved.get(id).copied().unwrap_or(Outcome::Aborted);
    for id in &sim.split {
        anomalies.push(format!("{id:?} committed in one group, aborted in another"));
    }
    let mut reads: BTreeMap<TxnId, BTreeMap<Vec<u8>, Option<Vec<u8>>>> = BTreeMap::new();
    for op in ops {
        if let Some(finished) = sim.finished.get(&op.id)
            && *finished != outcome(&op.id)
        {
            anomalies.push(format!(
                "{:?} finished {finished:?}, its groups resolved it {:?}",
                op.id,
                outcome(&op.id)
            ));
        }
        let answered = sim.answers.get(&op.id).map(|(a, _, _)| a);
        match (answered, outcome(&op.id)) {
            (Some(Answer::Committed { .. }), Outcome::Aborted) => {
                anomalies.push(format!("{:?} answered committed, ended aborted", op.id))
            }
            (Some(Answer::Aborted(_)), Outcome::Committed) => {
                anomalies.push(format!("{:?} answered aborted, ended committed", op.id))
            }
            _ => {}
        }
        if outcome(&op.id) == Outcome::Committed {
            let seen = match answered {
                Some(Answer::Committed { reads, .. }) => reads.iter().cloned().collect(),
                _ => sim
                    .applied_reads
                    .get(&op.id)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            };
            reads.insert(op.id, seen);
        }
    }
    // G1a and garbage reads; per key, each version's successor.
    let mut next: BTreeMap<(Vec<u8>, Option<Vec<u8>>), TxnId> = BTreeMap::new();
    for op in ops.iter().filter(|op| reads.contains_key(&op.id)) {
        let seen = &reads[&op.id];
        for value in seen.values().flatten() {
            match writer.get(value) {
                Some(w) if outcome(w) == Outcome::Aborted => {
                    anomalies.push(format!("G1a: {:?} read {value:?} of aborted {w:?}", op.id))
                }
                None => anomalies.push(format!("{:?} read {value:?}, which nobody wrote", op.id)),
                _ => {}
            }
        }
        for key in op.writes.keys() {
            let Some(prev) = seen.get(key) else {
                anomalies.push(format!("{:?} wrote {key:?} without reading it", op.id));
                continue;
            };
            if let Some(other) = next.insert((key.clone(), prev.clone()), op.id) {
                anomalies.push(format!(
                    "lost update: {other:?} and {:?} both wrote {key:?} after {prev:?}",
                    op.id
                ));
            }
        }
    }
    // Each key's chain from nothing must hold every committed write and end at its final value.
    let keys: BTreeSet<Vec<u8>> = ops
        .iter()
        .flat_map(|op| op.writes.keys().cloned())
        .collect();
    let mut edges: BTreeSet<(TxnId, TxnId)> = BTreeSet::new();
    for key in &keys {
        let mut version: Option<Vec<u8>> = None;
        let mut chain: Vec<TxnId> = Vec::new();
        while let Some(w) = next.get(&(key.clone(), version.clone())) {
            // rw: every committed reader of this version precedes its overwriter.
            for (r, seen) in &reads {
                if r != w && seen.get(key) == Some(&version) {
                    edges.insert((*r, *w));
                }
            }
            if let Some(prev) = chain.last() {
                edges.insert((*prev, *w)); // ww
            }
            chain.push(*w);
            version = ops
                .iter()
                .find(|op| op.id == *w)
                .and_then(|op| op.writes.get(key).cloned());
            if chain.len() > ops.len() {
                anomalies.push(format!("{key:?}'s versions loop"));
                break;
            }
        }
        let committed_writes = ops
            .iter()
            .filter(|op| reads.contains_key(&op.id) && op.writes.contains_key(key))
            .count();
        if chain.len() != committed_writes {
            anomalies.push(format!(
                "{key:?}: {} committed writes, {} in its chain",
                committed_writes,
                chain.len()
            ));
        }
        if value_of(&sim.groups, key) != version {
            anomalies.push(format!(
                "{key:?} ends {:?}, its chain {version:?}",
                value_of(&sim.groups, key)
            ));
        }
    }
    // wr.
    for (r, seen) in &reads {
        for value in seen.values().flatten() {
            if let Some(w) = writer.get(value).filter(|w| *w != r) {
                edges.insert((*w, *r));
            }
        }
    }
    // Real time: a transaction answered before another began precedes it.
    let committed: Vec<&Op> = ops.iter().filter(|op| reads.contains_key(&op.id)).collect();
    for a in &committed {
        if let Some((_, done, _)) = sim.answers.get(&a.id) {
            for b in &committed {
                if *done < b.invoke {
                    edges.insert((a.id, b.id));
                }
            }
        }
    }
    // A cycle, found without recursion: Kahn's algorithm leaves the nodes on cycles.
    let mut indegree: BTreeMap<TxnId, usize> = committed.iter().map(|op| (op.id, 0)).collect();
    for (_, to) in &edges {
        *indegree.entry(*to).or_default() += 1;
    }
    let mut free: Vec<TxnId> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut left = indegree.len();
    while let Some(id) = free.pop() {
        left -= 1;
        for (_, to) in edges
            .range(
                (
                    id,
                    TxnId {
                        coordinator: 0,
                        epoch: 0,
                        seq: 0,
                    },
                )..,
            )
            .take_while(|(from, _)| *from == id)
        {
            let d = indegree.get_mut(to).unwrap();
            *d -= 1;
            if *d == 0 {
                free.push(*to);
            }
        }
    }
    if left != 0 {
        anomalies.push(format!("a dependency cycle among {left} transactions"));
    }
    anomalies
}

struct Ran {
    committed: usize,
    crossed: usize,
    aborted: usize,
    crashes: usize,
}

fn run_seed(seed: u64, protocol: Protocol) -> Ran {
    let mut rng = Rng(seed ^ 0x5eed);
    let groups = 2 + u32::try_from(rng.below(3)).unwrap();
    let keys = 4 + u8::try_from(rng.below(5)).unwrap();
    let clients = 1 + usize::try_from(rng.below(4)).unwrap();
    let faults = rng.below(150);
    let total = 40;
    let mut sim = Sim::new(seed, groups, protocol, faults, clients);
    sim.start();
    let mut ops: Vec<Op> = Vec::new();
    let mut busy: Vec<Option<TxnId>> = vec![None; clients];
    let mut crashes = 0;
    let mut uid = 0u64;
    for _ in 0..1_000_000 {
        // Clients whose transaction was answered, or lost with a crashed coordinator, take another.
        for slot in &mut busy {
            if slot.is_some_and(|id| {
                sim.answers.contains_key(&id)
                    || id.epoch != sim.coordinator.as_ref().map_or(0, Coordinator::epoch)
            }) {
                *slot = None;
            }
        }
        if sim.ready() && ops.len() < total {
            for slot in &mut busy {
                if slot.is_some() || ops.len() >= total {
                    continue;
                }
                let picked: BTreeSet<u8> = (0..1 + rng.below(3))
                    .map(|_| u8::try_from(rng.below(u64::from(keys))).unwrap())
                    .collect();
                let mut t = Txn::default();
                let mut writes = BTreeMap::new();
                for k in picked {
                    t.reads.push(vec![k]);
                    if rng.below(3) != 0 {
                        uid += 1;
                        let value = uid.to_be_bytes().to_vec();
                        t.writes.push((vec![k], Some(value.clone())));
                        writes.insert(vec![k], value);
                    }
                }
                let invoke = sim.now;
                if let Some(id) = sim.begin(t) {
                    *slot = Some(id);
                    ops.push(Op { id, invoke, writes });
                }
            }
        }
        if sim.coordinator.is_some() && crashes < 3 && rng.below(400) < faults / 10 {
            crashes += 1;
            sim.crash();
            sim.start();
        }
        if !sim.step() {
            if sim.coordinator.is_none() {
                sim.start();
                continue;
            }
            if ops.len() >= total || !sim.ready() {
                break;
            }
        }
    }
    sim.settle();
    sim.assert_settled();
    let anomalies = check(&sim, &ops);
    assert!(
        anomalies.is_empty(),
        "seed {seed} {protocol:?}: {anomalies:#?}"
    );
    let committed: Vec<&Op> = ops
        .iter()
        .filter(|op| sim.resolved.get(&op.id) == Some(&Outcome::Committed))
        .collect();
    let route = |k: &Vec<u8>| u32::from(k[0]) % groups;
    Ran {
        committed: committed.len(),
        crossed: committed
            .iter()
            .filter(|op| op.writes.keys().map(route).collect::<BTreeSet<_>>().len() > 1)
            .count(),
        aborted: ops.len() - committed.len(),
        crashes,
    }
}

/// AC-T1.10.5: zero anomalies over seeds; `AMPHORA_XGROUP_SEEDS` sets how many (the coordinator's
/// run takes 10,000).
#[test]
fn ac_t1_10_5_seeded_runs_under_lost_steps_and_crashes_show_no_anomaly() {
    let seeds: u64 = std::env::var("AMPHORA_XGROUP_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
        let mut total = Ran {
            committed: 0,
            crossed: 0,
            aborted: 0,
            crashes: 0,
        };
        for seed in 0..seeds {
            let ran = run_seed(seed, protocol);
            total.committed += ran.committed;
            total.crossed += ran.crossed;
            total.aborted += ran.aborted;
            total.crashes += ran.crashes;
        }
        // Non-vacuity: transactions committed across groups, aborted, and lived through crashes.
        assert!(
            total.crossed > 0 && total.aborted > 0 && total.crashes > 0,
            "{protocol:?}: {} committed, {} across groups, {} aborted, {} crashes",
            total.committed,
            total.crossed,
            total.aborted,
            total.crashes
        );
    }
}

/// The checker finds what it looks for: a lost update and a fractured commit.
#[test]
fn ac_t1_10_5_the_check_finds_a_lost_update_and_a_partial_commit() {
    let mut sim = Sim::new(0, 2, Protocol::Parallel, 0, 1);
    let a = TxnId {
        coordinator: 7,
        epoch: 1,
        seq: 0,
    };
    let b = TxnId {
        coordinator: 7,
        epoch: 1,
        seq: 1,
    };
    sim.resolved.insert(a, Outcome::Committed);
    sim.resolved.insert(b, Outcome::Committed);
    sim.applied_reads.insert(a, vec![(vec![0], None)]);
    sim.applied_reads.insert(b, vec![(vec![0], None)]);
    sim.groups[0].set_value(&[0], Some(b"b")).unwrap();
    let ops = vec![
        Op {
            id: a,
            invoke: 0,
            writes: [(vec![0], b"a".to_vec())].into(),
        },
        Op {
            id: b,
            invoke: 0,
            writes: [(vec![0], b"b".to_vec())].into(),
        },
    ];
    assert!(
        check(&sim, &ops)
            .iter()
            .any(|a| a.starts_with("lost update"))
    );
    // One write of a committed two-key transaction missing.
    let mut sim = Sim::new(0, 2, Protocol::Parallel, 0, 1);
    sim.resolved.insert(a, Outcome::Committed);
    sim.applied_reads
        .insert(a, vec![(vec![0], None), (vec![1], None)]);
    sim.groups[0].set_value(&[0], Some(b"a")).unwrap();
    let ops = vec![Op {
        id: a,
        invoke: 0,
        writes: [(vec![0], b"a".to_vec()), (vec![1], b"c".to_vec())].into(),
    }];
    assert!(!check(&sim, &ops).is_empty());
}

/// AC-T1.10.5, seed 0's interleaving: the client heard committed, every resolution landed, and the
/// coordinator crashed before their replies came back. Nothing of the transaction is left open, so
/// the next incarnation finds nothing to recover and no coordinator reports it finished; the
/// groups hold it committed, and the check must read it so (reading the coordinator's report, it
/// called it aborted, the reads of its writes G1a, and its edges a cycle).
#[test]
fn ac_t1_10_5_a_crash_after_every_resolution_landed_leaves_it_committed() {
    for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
        let mut sim = Sim::new(1, 2, protocol, 0, 1);
        sim.start();
        while !sim.ready() {
            assert!(sim.step());
        }
        let writes = [(vec![0], b"a".to_vec()), (vec![1], b"b".to_vec())];
        let t = Txn {
            reads: vec![vec![0], vec![1]],
            writes: writes
                .iter()
                .map(|(k, v)| (k.clone(), Some(v.clone())))
                .collect(),
            conds: Vec::new(),
        };
        let id = sim.begin(t).unwrap();
        while !sim.answers.contains_key(&id) {
            assert!(sim.step());
        }
        assert!(matches!(sim.answers[&id].0, Answer::Committed { .. }));
        // The crash: every step it had out (each group's resolution) lands; no reply is heard.
        sim.crash();
        while sim.step() {}
        sim.start();
        sim.settle();
        sim.assert_settled();
        assert!(!sim.finished.contains_key(&id), "{protocol:?}");
        assert_eq!(sim.resolved.get(&id), Some(&Outcome::Committed));
        let ops = vec![Op {
            id,
            invoke: 0,
            writes: writes.into_iter().collect(),
        }];
        assert_eq!(check(&sim, &ops), Vec::<String>::new(), "{protocol:?}");
    }
}

/// AC-T1.10.1 (lane xgroup-sql-11; Ada's decision: the commit's revision across groups is the
/// cross-group HLC timestamp): a committed answer's `at` is the latest clock its lock replies
/// carried, fixed before the decision, and no other step's clock moves it. Do: three groups in
/// memory, both protocols; each group's lock reply carries its own clock (30, 50, 40), every
/// other reply a later one (999); a transaction writing a key in each group. Expect: committed,
/// `at` 50.
#[test]
fn ac_t1_10_1_the_commit_stamp_is_the_latest_lock_replys_clock() {
    use std::collections::VecDeque;
    let lock_clocks = [30u64, 50, 40];
    for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
        let mut groups: Vec<Memory> = (0..3).map(|_| Memory::default()).collect();
        let config = Config {
            id: 7,
            protocol,
            registry: 0,
            groups: vec![0, 1, 2],
            in_flight: 1,
        };
        let (mut coordinator, first) = Coordinator::start(config).unwrap();
        let mut queue: VecDeque<Event> = first.into();
        let mut answer = None;
        let mut begun = false;
        let txn = Txn {
            reads: Vec::new(),
            writes: (0..3u8).map(|g| (vec![g], Some(vec![g]))).collect(),
            conds: Vec::new(),
        };
        let route = |key: &[u8]| key.first().map_or(0, |g| GroupId::from(*g));
        while let Some(event) = queue.pop_front() {
            match event {
                Event::Send(send) => {
                    let at = usize::try_from(send.group).unwrap();
                    let reply = apply(send.group, &mut groups[at], &send.envelope).unwrap();
                    let clock = match send.envelope.step {
                        Step::Lock { .. } => lock_clocks[at],
                        _ => 999,
                    };
                    queue.extend(coordinator.reply(send.id, Ok(reply), Some(clock)).unwrap());
                }
                Event::Ready if !begun => {
                    begun = true;
                    let (_, events) = coordinator.begin(txn.clone(), route).unwrap();
                    queue.extend(events);
                }
                Event::Answer { answer: a, .. } => answer = Some(a),
                _ => {}
            }
        }
        assert!(
            matches!(answer, Some(Answer::Committed { at: Some(50), .. })),
            "{protocol:?}: {answer:?}"
        );
    }
}

/// What [`drive`] does with a step about to land in its group.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fate {
    Land,
    /// Never lands, and no reply comes before the crash that follows.
    Lose,
    /// Lands, then the coordinator crashes before its reply.
    LandThenCrash,
    /// The coordinator crashes before it lands.
    Crash,
}

/// Runs a coordinator's events on groups in memory, each lock reply carrying `clocks`' entry for
/// its group: `begin` begun once it is ready, each step's fate asked before it lands. Ends at a
/// crash or once nothing is left; answers what the client heard.
fn drive(
    groups: &mut [Memory],
    coordinator: &mut Coordinator,
    first: Vec<Event>,
    mut begin: Option<Txn>,
    clocks: &[u64],
    mut fate: impl FnMut(&Step) -> Fate,
) -> Option<Answer> {
    let mut queue: std::collections::VecDeque<Event> = first.into();
    let mut answer = None;
    while let Some(event) = queue.pop_front() {
        match event {
            Event::Send(send) => {
                let fated = fate(&send.envelope.step);
                match fated {
                    Fate::Crash => return answer,
                    Fate::Lose => continue,
                    Fate::Land | Fate::LandThenCrash => {}
                }
                let at = usize::try_from(send.group).unwrap();
                let reply = apply(send.group, &mut groups[at], &send.envelope).unwrap();
                if fated == Fate::LandThenCrash {
                    return answer;
                }
                let clock = match send.envelope.step {
                    Step::Lock { .. } | Step::LockIf { .. } => clocks.get(at).copied(),
                    _ => None,
                };
                queue.extend(coordinator.reply(send.id, Ok(reply), clock).unwrap());
            }
            Event::Ready => {
                if let Some(txn) = begin.take() {
                    let route = |key: &[u8]| key.first().map_or(0, |g| GroupId::from(*g));
                    let (_, events) = coordinator.begin(txn, route).unwrap();
                    queue.extend(events);
                }
            }
            Event::Answer { answer: heard, .. } => answer = Some(heard),
            _ => {}
        }
    }
    answer
}

/// T-T1.9.R30 (CP-T1.9; reshard.md, "Decision (lane split-merge-22): Records::commit and
/// recovery on the one stamp"). Do: three groups in memory, their clocks 0, 10 and 20 writes
/// along, both protocols; a transaction writing a key in each, its lock replies carrying clocks
/// 30, 50 and 40. First the coordinator crashes once the first group resolved it (under Parallel
/// Commits its explicit commit lost before), and a new incarnation recovers it. Then, under
/// Parallel Commits, a crash with every lock in place and nothing decided or resolved, and a
/// recovery. Expect: the client heard committed at 50, and after the recovery every group's key
/// is stamped 50, the stamp the first group resolved at; in the second, every key carries one
/// stamp, the latest of the groups' lock stamps (21), at or before the 50 the client heard.
#[test]
fn t_t1_9_r30_a_recovered_transaction_resolves_at_its_one_stamp() {
    let lock_clocks = [30u64, 50, 40];
    let txn = Txn {
        reads: Vec::new(),
        writes: (0..3u8).map(|g| (vec![g], Some(vec![g]))).collect(),
        conds: Vec::new(),
    };
    let fresh = || -> Vec<Memory> {
        (0..3u8)
            .map(|g| {
                let mut group = Memory::default();
                for i in 0..g * 10 {
                    group.set_value(&[b'z', i], Some(b"v")).unwrap();
                }
                group
            })
            .collect()
    };
    let stamps = |groups: &mut [Memory]| -> Vec<Option<u64>> {
        (0..3u8)
            .map(|g| groups[usize::from(g)].stamp(&[g]).unwrap())
            .collect()
    };
    for protocol in [Protocol::TwoPhase, Protocol::Parallel] {
        let config = Config {
            id: 7,
            protocol,
            registry: 0,
            groups: vec![0, 1, 2],
            in_flight: 1,
        };
        let mut groups = fresh();
        let (mut first, events) = Coordinator::start(config.clone()).unwrap();
        let heard = drive(
            &mut groups,
            &mut first,
            events,
            Some(txn.clone()),
            &lock_clocks,
            |step| match step {
                Step::Decide { .. } | Step::DecideAt { .. } if protocol == Protocol::Parallel => {
                    Fate::Lose
                }
                Step::Resolve { .. } | Step::ResolveAt { .. } => Fate::LandThenCrash,
                _ => Fate::Land,
            },
        );
        assert!(
            matches!(heard, Some(Answer::Committed { at: Some(50), .. })),
            "{protocol:?}: {heard:?}"
        );
        let (mut second, events) = Coordinator::start(config.clone()).unwrap();
        drive(&mut groups, &mut second, events, None, &[], |_| Fate::Land);
        assert_eq!(
            stamps(groups.as_mut_slice()),
            vec![Some(50); 3],
            "{protocol:?}"
        );
        assert!(groups.iter().all(|g| g.held().is_empty()), "{protocol:?}");
    }

    let config = Config {
        id: 7,
        protocol: Protocol::Parallel,
        registry: 0,
        groups: vec![0, 1, 2],
        in_flight: 1,
    };
    let mut groups = fresh();
    let (mut first, events) = Coordinator::start(config.clone()).unwrap();
    let heard = drive(
        &mut groups,
        &mut first,
        events,
        Some(txn),
        &lock_clocks,
        |step| match step {
            Step::Decide { .. } | Step::DecideAt { .. } => Fate::Crash,
            _ => Fate::Land,
        },
    );
    assert!(
        matches!(heard, Some(Answer::Committed { at: Some(50), .. })),
        "{heard:?}"
    );
    let (mut second, events) = Coordinator::start(config).unwrap();
    drive(&mut groups, &mut second, events, None, &[], |_| Fate::Land);
    assert_eq!(stamps(groups.as_mut_slice()), vec![Some(21); 3]);
    assert!(groups.iter().all(|g| g.held().is_empty()));
}
