//! Two sessions' interleaved statements on one database (docs/design/postgres.md section 8), each step answered as
//! a PostgreSQL 19 server answered it: READ COMMITTED's statement snapshots, the wait for a row another
//! session's open transaction changed (woken by that transaction's end), REPEATABLE READ's refusal, and
//! the levels SHOW names. Each step is a Query to a session's [`Executor`], so a statement that waits is one the
//! test saw waiting (T-PG.14.1).

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros
    )
)]

mod tpcb;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use amphora::postgres::execute::{Database, Executor};
use amphora::postgres::session::{Flow, Statements, TxStatus};

/// Counts the wakes of a session's wait.
#[derive(Default)]
struct Woken(AtomicUsize);

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// A session, the wakes of its wait, and what its Query answered before a statement waited.
struct Session {
    executor: Executor,
    woken: Arc<Woken>,
    answered: Vec<u8>,
}

/// The messages in `out` (ReadyForQuery's are the session's to write, so none is here).
fn messages(out: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut found = Vec::new();
    let mut at = 0;
    while at < out.len() {
        let length = i32::from_be_bytes(out[at + 1..at + 5].try_into().unwrap());
        let end = at + 1 + usize::try_from(length).unwrap();
        found.push((out[at], out[at + 5..end].to_vec()));
        at = end;
    }
    found
}

fn status(executor: &Executor) -> u8 {
    match executor.status() {
        TxStatus::Idle => b'I',
        TxStatus::InTransaction => b'T',
        TxStatus::Failed => b'E',
    }
}

fn sessions(dir: &tempfile::TempDir) -> BTreeMap<String, Session> {
    let path = dir.path().join("postgres.db");
    let database = Database::open(
        path.to_str().unwrap(),
        amphora_engine::storage::versioned::memory::machine(),
    )
    .unwrap();
    ["s1", "s2"]
        .into_iter()
        .map(|name| {
            let session = Session {
                executor: database.session("ada", "UTC").unwrap(),
                woken: Arc::new(Woken::default()),
                answered: Vec::new(),
            };
            (name.to_owned(), session)
        })
        .collect()
}

/// T-PG.14.1: each step of two sessions' interleaved statements is answered as a 19.0 server answered
/// it (scripts/postgres-isolation-answers.sh), and a statement that waits there waits here: it has
/// given no answer until the other session's transaction ends, whose end alone wakes it.
#[test]
fn t_pg_14_1_two_sessions_interleave_as_a_19_server_does() {
    let dir = tempfile::tempdir().unwrap();
    let mut sessions = sessions(&dir);
    let answers = tpcb::answers(include_str!("isolation/answers.txt"));
    let steps: Vec<&str> = include_str!("isolation/steps.txt")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .collect();
    assert_eq!(answers.len(), steps.len());
    let mut waited = 0;
    let mut compared = 0;
    for ((step, sql, theirs), line) in answers.iter().zip(&steps) {
        let (name, kind) = if let Some(name) = step.strip_suffix('>') {
            (name, Kind::Waits)
        } else if let Some(name) = step.strip_suffix('<') {
            (name, Kind::Ends)
        } else {
            (step.as_str(), Kind::Runs)
        };
        let expected = match kind {
            Kind::Runs => format!("{name}|{sql}"),
            Kind::Waits => format!("{name}>{sql}"),
            Kind::Ends => format!("{name}<"),
        };
        assert_eq!(*line, expected);
        let session = sessions.get_mut(name).unwrap();
        let mut out = Vec::new();
        match kind {
            Kind::Runs => {
                let flow = session.executor.simple_query(sql, &mut out).unwrap();
                assert_eq!(flow, Flow::Done, "{line}");
            }
            Kind::Waits => {
                let flow = session.executor.simple_query(sql, &mut out).unwrap();
                assert_eq!(flow, Flow::Waiting, "{line}");
                session.answered = out;
                let waker = Waker::from(Arc::clone(&session.woken));
                let mut context = Context::from_waker(&waker);
                assert_eq!(
                    session.executor.poll_wait(&mut context),
                    Poll::Pending,
                    "{line}"
                );
                assert_eq!(session.woken.0.swap(0, Ordering::SeqCst), 0, "{line}");
                waited += 1;
                continue;
            }
            Kind::Ends => {
                assert!(
                    session.woken.0.swap(0, Ordering::SeqCst) > 0,
                    "{line}: the end of the transaction it waits for wakes it"
                );
                let waker = Waker::noop();
                let mut context = Context::from_waker(waker);
                assert_eq!(
                    session.executor.poll_wait(&mut context),
                    Poll::Ready(()),
                    "{line}"
                );
                out = std::mem::take(&mut session.answered);
                let flow = session.executor.resume(&mut out).unwrap();
                assert_eq!(flow, Flow::Done, "{line}");
            }
        }
        let mut theirs = theirs.clone();
        let (z, body) = theirs.pop().unwrap();
        assert_eq!((z, body[0]), (b'Z', status(&session.executor)), "{line}");
        let ours = messages(&out);
        let (got, want) = (tpcb::told(sql, &ours), tpcb::told(sql, &theirs));
        assert_eq!(got, want, "{line}");
        compared += want.len();
    }
    assert!(waited >= 5, "statements waited: {waited}");
    assert!(compared > 100, "messages compared: {compared}");
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Runs,
    Waits,
    Ends,
}

/// T-PG.14.1: a transaction that changes the catalog is validated in full at any level: one that
/// builds an index beside a commit that adds a row to its table is refused `40001` (the index would
/// lack the row), where a READ COMMITTED transaction that changes rows only is not.
#[test]
fn t_pg_14_1_an_index_built_beside_a_commit_is_refused_at_any_level() {
    let dir = tempfile::tempdir().unwrap();
    let mut sessions = sessions(&dir);
    let mut run = |name: &str, sql: &str| {
        let mut out = Vec::new();
        let session = sessions.get_mut(name).unwrap();
        let flow = session.executor.simple_query(sql, &mut out).unwrap();
        assert_eq!(flow, Flow::Done, "{sql}");
        out
    };
    for (name, sql) in [
        ("s1", "create table t(id int not null, v int)"),
        ("s1", "insert into t(id, v) values (1, 10)"),
        ("s1", "begin"),
        ("s1", "alter table t add primary key (id)"),
        ("s2", "insert into t(id, v) values (2, 20)"),
    ] {
        let out = run(name, sql);
        assert_eq!(out[0], b'C', "{name}|{sql}: {out:?}");
    }
    let out = run("s1", "commit");
    assert!(
        out.starts_with(b"E") && out.windows(5).any(|w| w == b"40001"),
        "{out:?}"
    );
    let out = run("s2", "select v from t where id = 2");
    assert!(out.windows(2).any(|w| w == b"20"), "{out:?}");
}

/// T-PG.14.2: two sessions that wait for each other are not left to wait: the one whose wait would
/// close the ring is refused `40P01` at once (19 refuses one of the two once `deadlock_timeout`
/// passes: which one is section 11's unit 15), and its rollback lets the other go on.
#[test]
fn t_pg_14_2_two_sessions_waiting_for_each_other_are_refused_40p01() {
    let dir = tempfile::tempdir().unwrap();
    let mut sessions = sessions(&dir);
    let mut run = |name: &str, sql: &str| {
        let mut out = Vec::new();
        let session = sessions.get_mut(name).unwrap();
        (session.executor.simple_query(sql, &mut out).unwrap(), out)
    };
    for sql in [
        "create table t(id int not null, v int)",
        "insert into t(id, v) values (1, 10)",
        "insert into t(id, v) values (2, 20)",
        "begin",
        "update t set v = v + 1 where id = 1",
    ] {
        let (flow, _) = run("s1", sql);
        assert_eq!(flow, Flow::Done, "{sql}");
    }
    for sql in ["begin", "update t set v = v + 1 where id = 2"] {
        let (flow, _) = run("s2", sql);
        assert_eq!(flow, Flow::Done, "{sql}");
    }
    let (flow, _) = run("s1", "update t set v = v + 1 where id = 2");
    assert_eq!(flow, Flow::Waiting);
    // The wait is registered when the session polls it, as the listener does: the wait graph then names it.
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    let session = sessions.get_mut("s1").unwrap();
    assert_eq!(session.executor.poll_wait(&mut context), Poll::Pending);
    let session = sessions.get_mut("s2").unwrap();
    let mut out = Vec::new();
    let flow = session
        .executor
        .simple_query("update t set v = v + 1 where id = 1", &mut out)
        .unwrap();
    assert_eq!(flow, Flow::Waiting);
    assert_eq!(
        session.executor.poll_wait(&mut context),
        Poll::Ready(()),
        "a wait that closes a ring of waits is over at once"
    );
    let mut answer = Vec::new();
    assert_eq!(session.executor.resume(&mut answer).unwrap(), Flow::Done);
    assert!(answer.starts_with(b"E"), "{answer:?}");
    assert!(
        answer.windows(5).any(|w| w == b"40P01"),
        "the refusal's SQLSTATE: {answer:?}"
    );
    let (flow, _) = {
        let mut out = Vec::new();
        (
            session.executor.simple_query("rollback", &mut out).unwrap(),
            out,
        )
    };
    assert_eq!(flow, Flow::Done);
    let session = sessions.get_mut("s1").unwrap();
    assert_eq!(session.executor.poll_wait(&mut context), Poll::Ready(()));
    let mut out = Vec::new();
    assert_eq!(session.executor.resume(&mut out).unwrap(), Flow::Done);
    assert_eq!(out[0], b'C', "{out:?}");
}
