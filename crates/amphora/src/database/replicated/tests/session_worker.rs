//! A caller's session on its owner's session worker (replication.md, "Decision (lane P6)";
//! CP-T1.13), on the driven shell: the owner goes on stepping while a session runs, and a write
//! whose lead moved while its session ran is refused, typed, its session run once.

use super::*;

/// A write of 7 to member_items whose session counts its runs in `ran`, tells `began` it runs,
/// then waits for the test's word on `go`.
fn held(
    ran: &Arc<AtomicUsize>,
    began: mpsc::Sender<()>,
    go: mpsc::Receiver<()>,
) -> impl FnOnce(&mut Db) -> anyhow::Result<Vec<u8>> + Send + 'static {
    let ran = Arc::clone(ran);
    move |db| {
        ran.fetch_add(1, Ordering::Relaxed);
        db.exec("CREATE TABLE IF NOT EXISTS member_items(value)")?;
        db.exec("INSERT INTO member_items VALUES(7)")?;
        let _ = began.send(());
        let _ = go.recv();
        Ok(Vec::new())
    }
}

/// Rounds until `done`, never waiting on work out off the driver's thread: the test holds a
/// session out there, so waiting for it would wait on the test itself. A round that relayed
/// nothing yields to the disk (`POLL`), a hang guard on the test's own pacing.
fn rounds_until(
    driven: &mut Driven,
    pump: &mut Pump<'_>,
    mut done: impl FnMut(&mut Driven) -> bool,
) {
    for _ in 0..ROUNDS {
        if done(driven) {
            return;
        }
        if pump.drive_until(driven.until()) == 0 {
            std::thread::park_timeout(POLL);
        }
        driven.step();
    }
    panic!("not done after {ROUNDS} driven rounds");
}

/// A write held on the leader's worker: the leader, the write's answer, its session's run count
/// and the word that lets its session go on.
struct Held {
    leader: usize,
    answer: mpsc::Receiver<Result<Answered, Refusal>>,
    ran: Arc<AtomicUsize>,
    go: mpsc::Sender<()>,
}
/// Owner 0 elected, then a write submitted at the leader, its session held once it began.
fn hold_a_session(driven: &mut Driven, pump: &mut Pump<'_>, clients: &[Client]) -> Held {
    let leader = driven.elect(pump, clients, 0, 0);
    let ran = Arc::new(AtomicUsize::new(0));
    let (began, started) = mpsc::channel();
    let (go, wait) = mpsc::channel();
    // A deadline an hour past the shell's: the test moves the clocks through election timeouts.
    let until = driven.until() + Duration::from_secs(3600);
    let answer = submit(clients.get(leader).unwrap(), until, held(&ran, began, wait));
    rounds_until(driven, pump, |_| started.try_recv().is_ok());
    assert!(
        driven.owner(leader).sessions.out,
        "the session runs on the worker"
    );
    Held {
        leader,
        answer,
        ran,
        go,
    }
}

/// CP-T1.13 (replication.md, "Decision (lane P6)"): a caller's session runs on its owner's
/// session worker, so the owner keeps heartbeating and answers local reads while it runs. Do: a
/// write held on the leader's worker; every clock moved a tick a round through three of the
/// leader's election timeouts, rounds going on between; a local read at the leader; then the
/// session let go. Expect: the term and the leader as before; the local read answered at the
/// applied state, without the write; the write committed, its row on the leader.
#[test]
fn cp_t1_13_a_leader_keeps_its_lead_and_answers_local_reads_while_a_session_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut driven = Driven::new();
    let clients: Vec<Client> = (1..=3)
        .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
        .collect();
    let mut pump = Pump::new(&clients);
    let Held {
        leader,
        answer,
        ran,
        go,
    } = hold_a_session(&mut driven, &mut pump, &clients);
    let id = u64::try_from(leader + 1).unwrap();
    let term = driven.owner(leader).replica.term();
    let election = driven.owner(leader).replica.core().raft.config().election_tick;
    for _ in 0..3 * election {
        for owner in 0..clients.len() {
            driven.advance_tick(owner);
        }
        let mut quiet = 0;
        rounds_until(&mut driven, &mut pump, |driven| {
            let idle = driven
                .owners
                .iter()
                .flatten()
                .all(|(owner, _)| owner.replica.in_flight() == 0);
            quiet = if idle { quiet + 1 } else { 0 };
            quiet > 2
        });
    }
    assert!(driven.owner(leader).sessions.out, "the session still runs");
    assert_eq!(driven.owner(leader).replica.term(), term, "no election");
    for client in &clients {
        assert_eq!(client.handle().status().unwrap().leader, id, "one leader");
    }
    assert_eq!(
        clients
            .get(leader)
            .unwrap()
            .read_local(Instant::now() + HANG_READ, values),
        Ok(vec![]),
        "a local read answered while the session runs"
    );
    go.send(()).unwrap();
    assert!(driven.answer(&mut pump, &answer).is_ok(), "the write commits");
    assert_eq!(ran.load(Ordering::Relaxed), 1, "the session ran once");
    assert_eq!(
        clients
            .get(leader)
            .unwrap()
            .read_local(Instant::now() + HANG_READ, values),
        Ok(vec![7])
    );
}

/// CP-T1.13 (replication.md, "Decision (lane P6)"): a write whose lead moved while its session
/// ran is refused `NotLeader`, a known outcome, and its session is never run again. Do: a write
/// held on the leader's worker; the lead handed to another member (a vote the old leader takes
/// while its session runs); then the session let go. Expect: every member follows the new leader
/// while the session still runs; the write refused `NotLeader` naming it; the session run once;
/// no member holds the write's row.
#[test]
fn cp_t1_13_a_write_whose_lead_moved_while_its_session_ran_is_refused_not_leader() {
    let dir = tempfile::tempdir().unwrap();
    let mut driven = Driven::new();
    let clients: Vec<Client> = (1..=3)
        .map(|n| driven.open(dir.path(), n, vec![1, 2, 3], None))
        .collect();
    let mut pump = Pump::new(&clients);
    let Held {
        leader,
        answer,
        ran,
        go,
    } = hold_a_session(&mut driven, &mut pump, &clients);
    let term = driven.owner(leader).replica.term();
    let next = (leader + 1) % clients.len();
    let id = u64::try_from(next + 1).unwrap();
    driven.owner_mut(leader).replica.transfer(id).unwrap();
    rounds_until(&mut driven, &mut pump, |_| {
        clients
            .iter()
            .all(|c| c.handle().status().unwrap().leader == id)
    });
    assert!(driven.owner(leader).sessions.out, "the session still runs");
    assert!(driven.owner(leader).replica.term() > term, "a new term");
    go.send(()).unwrap();
    assert_eq!(
        driven.answer(&mut pump, &answer).map(|_| ()),
        Err(Refusal::NotLeader { leader: Some(id) })
    );
    assert_eq!(ran.load(Ordering::Relaxed), 1, "the session ran once");
    driven.quiet(&mut pump);
    for client in &clients {
        assert_eq!(
            client.read_local(Instant::now() + HANG_READ, values),
            Ok(vec![]),
            "no member holds the refused write's row"
        );
    }
}
