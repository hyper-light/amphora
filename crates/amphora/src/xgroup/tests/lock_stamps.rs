//! T-T1.9.R28/R30. Exercise the storage transactions behind the lock votes.

use super::*;
use crate::backup::{Hooks, MICRO_NS};
use crate::replication::{Applied, Machine, closed};
use hyper_durable::{EntryRef, StateMachine};
use hyper_raft::proto::{ConfState, EntryType};
use std::sync::atomic::{AtomicU64, Ordering};

/// A snapshot at 25 can still miss the uncommitted lock taken at 21. The writer finishes at 50.
/// Reopen before any decision or resolution and ask for the lock's vote.
#[test]
fn t_t1_9_r30_a_backed_up_lock_keeps_its_publication_clock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("owner.db");
    let mut owner = Db::open(&path).unwrap();
    owner.exec(SCHEMA).unwrap();
    let mut reader = owner.reader().unwrap();
    let now = Arc::new(AtomicU64::new(21 * MICRO_NS));
    let clock = Arc::clone(&now);
    let hooks = Hooks::new(Arc::new(move || clock.load(Ordering::Acquire)), 8);
    owner.back_up(Arc::clone(&hooks));
    let id = txn(1);
    let step = env(1, None, lock_step(id, &[], &[(b"k", b"v")], Some(vec![0])));
    let reply = owner
        .transaction(|db| {
            let reply = apply(0, &mut SqlTablet::new(db), &step)?;
            now.store(25 * MICRO_NS, Ordering::Release);
            assert_eq!(hooks.marks.next_micros()?, 25);
            assert!(SqlTablet::new(&mut reader).locks(b"k")?.is_empty());
            now.store(50 * MICRO_NS, Ordering::Release);
            Ok(reply)
        })
        .unwrap();
    assert!(matches!(reply, Reply::Locked { .. }));
    let heard = hooks.marks.latest_micros().unwrap();
    assert_eq!(heard, 50);
    owner.flusher()().unwrap();
    drop(reader);
    drop(owner);

    let mut reopened = Db::open(&path).unwrap();
    let mut tablet = SqlTablet::new(&mut reopened);
    assert_eq!(tablet.locks_of(id).unwrap().len(), 1);
    assert_eq!(tablet.done(id).unwrap(), None);
    assert_eq!(tablet.value(b"k").unwrap(), None);
    assert_eq!(tablet.record(id).unwrap(), Some(Record::Staging(vec![0])));
    let recovered = apply(0, &mut tablet, &env(2, None, Step::PreventAt { txn: id })).unwrap();
    assert_eq!(
        recovered,
        Reply::PresentAt {
            present: true,
            committed: None,
            locked: Some(heard)
        }
    );
}

fn land(machine: &mut Machine, index: u64, proposed: u64, pages: &[u8]) -> Vec<Applied> {
    let mut answers = Vec::new();
    machine
        .apply(
            &EntryRef {
                index,
                term: 1,
                kind: EntryType::EntryNormal,
                context: b"lock-stamp",
                data: &closed::stamp(proposed, pages),
            },
            &mut answers,
        )
        .unwrap();
    answers
}

/// Two replicas apply a proposal stamped at 21 after a closed time of 49. Both must flush the
/// vote at 50 with the locks. An unrelated later entry advances the clock to 90. After reopening,
/// a different replica recovers the vote and resolves a real Records row at exactly 50.
#[test]
fn t_t1_9_r30_a_replicated_lock_keeps_its_lifted_time_on_every_member() {
    let dir = tempfile::tempdir().unwrap();
    let conf = ConfState {
        voters: vec![1, 2],
        ..Default::default()
    };
    let open = |name: &str| Machine::open(&dir.path().join(name), conf.clone()).unwrap();
    let mut members = [open("first.db"), open("second.db")];
    let schema = members[0]
        .prepare_db(|db| db.exec(SCHEMA))
        .unwrap()
        .0
        .encode()
        .unwrap();
    for member in &mut members {
        assert!(land(member, 1, 49, &schema)[0].result.is_ok());
    }

    let row = sql::record_key("n", "f", b"k").unwrap();
    let id = txn(1);
    let lock = env(1, None, lock_step(id, &[], &[(&row, b"v")], Some(vec![0])));
    let (pages, reply) = members[0]
        .prepare_db(|db| apply(0, &mut SqlTablet::new(db), &lock))
        .unwrap();
    assert!(matches!(reply, Reply::Locked { .. }));
    let data = pages.encode().unwrap();
    let other = env(1, None, lock_step(txn(2), &[b"other"], &[], None));
    let stale = members[0]
        .prepare_db(|db| apply(0, &mut SqlTablet::new(db), &other))
        .unwrap()
        .0
        .encode()
        .unwrap();
    for member in &mut members {
        let applied = land(member, 2, 21, &data);
        assert!(applied[0].result.is_ok());
        assert_eq!(applied[0].time, Some(50));
        assert!(land(member, 3, 90, &stale)[0].result.is_err());
        assert_eq!(member.closed(), Some(90));
        member.log_flusher()().unwrap();
    }
    drop(members);

    let mut members = [open("first.db"), open("second.db")];
    let vote = |db: &mut Db| {
        let mut tablet = SqlTablet::new(db);
        assert_eq!(
            tablet.lock_stamp(txn(2))?,
            None,
            "a refused delta has no vote"
        );
        assert_eq!(tablet.locks_of(id)?.len(), 1);
        assert_eq!(tablet.value(&row)?, None);
        assert_eq!(tablet.commit_stamp(id)?, None);
        apply(0, &mut tablet, &env(1, None, Step::PreventAt { txn: id }))
    };
    for member in &mut members {
        assert_eq!(
            member.read_db(vote).unwrap(),
            Reply::PresentAt {
                present: true,
                committed: None,
                locked: Some(50)
            }
        );
    }
    let resolve = env(
        2,
        None,
        Step::ResolveAt {
            txn: id,
            outcome: Outcome::Committed,
            at: 50,
        },
    );
    let resolution = members[1]
        .prepare_db(|db| apply(0, &mut SqlTablet::new(db), &resolve))
        .unwrap()
        .0
        .encode()
        .unwrap();
    for member in &mut members {
        assert!(land(member, 4, 91, &resolution)[0].result.is_ok());
        member
            .read_db(|db| {
                let mut tablet = SqlTablet::new(db);
                assert_eq!(tablet.lock_stamp(id)?, None);
                assert_eq!(tablet.value(&row)?, Some(b"v".to_vec()));
                assert_eq!(tablet.stamp(&row)?, Some(50));
                Ok(())
            })
            .unwrap();
    }
}

/// The real owners answer a Parallel Commit. Drop the coordinator before its first decision or
/// resolution can land. Its successor must give both Records rows the stamp already answered.
#[test]
fn t_t1_9_r30_real_owners_recover_the_answer_from_their_locks_alone() {
    use crate::backup::{Archive, CheckPolicy, ChunkGrowth, FolderBlobs};
    use crate::database::Database;
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
    let replica = Database::open_replicated(
        &dir.path().join("g1.db"),
        &dir.path().join("g1.log"),
        amphora_api::replication::ReplicaConfig {
            node_id: 1,
            voters: vec![1],
            learners: vec![],
            bootstrap: None,
        },
        until(),
    )
    .unwrap();
    let groups = [backed, replica];
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
        carried.set(carried.get().max(latest.unwrap()));
        (reply, vote)
    };
    let config = Config {
        id: 7,
        protocol: Protocol::Parallel,
        registry: 0,
        groups: vec![0, 1],
        in_flight: 1,
    };
    let (mut first, events) = Coordinator::start(config.clone()).unwrap();
    let keys = [0, 1].map(|g| sql::record_key("n", "f", &[g]).unwrap());
    let tx = Txn {
        reads: vec![],
        writes: keys
            .iter()
            .map(|key| (key.clone(), Some(b"v".to_vec())))
            .collect(),
        conds: vec![],
    };
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
                assert!(votes.insert(group, vote.unwrap()).is_none());
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
    let Some((
        Answer::Committed {
            at: Some(heard), ..
        },
        rounds,
    )) = answer
    else {
        panic!("implicit commit did not answer: {answer:?}");
    };
    assert_eq!(rounds, 1);
    assert_eq!(votes.len(), 2);
    assert_eq!(votes.values().max(), Some(&heard));
    let id = locking.unwrap();
    drop(first);
    for (g, group) in groups.iter().enumerate() {
        let key = keys[g].clone();
        let vote = votes[&GroupId::try_from(g).unwrap()];
        group
            .call_internal(until(), move |db| {
                let mut tablet = SqlTablet::new(db);
                assert_eq!(tablet.locks_of(id)?.len(), 1);
                assert_eq!(tablet.value(&key)?, None);
                assert_eq!(tablet.done(id)?, None);
                assert_eq!(tablet.commit_stamp(id)?, None);
                assert_eq!(tablet.lock_stamp(id)?, Some(vote));
                Ok(())
            })
            .unwrap();
    }

    // The next incarnation has lost the first one's in-memory clock and its entire send queue.
    carried.set(0);
    let (mut second, events) = Coordinator::start(config).unwrap();
    drive_on(&mut second, events, None, serve, |_| Fate::Land);
    assert_eq!(second.counts().recovered_committed, 1);
    for (group, key) in groups.iter().zip(keys) {
        group
            .call_internal(until(), move |db| {
                let mut tablet = SqlTablet::new(db);
                assert!(tablet.locks_of(id)?.is_empty());
                assert_eq!(tablet.value(&key)?, Some(b"v".to_vec()));
                assert_eq!(tablet.stamp(&key)?, Some(heard));
                Ok(())
            })
            .unwrap();
    }
}
