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

use amphora::postgres::execute::{Database, Executor, SessionRole};
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
    let role = SessionRole {
        name: "ada",
        superuser: true,
    };
    (1..=8)
        .map(|n| {
            let session = Session {
                executor: database.session(role, "UTC", 1000 + n).unwrap(),
                woken: Arc::new(Woken::default()),
                answered: Vec::new(),
            };
            (format!("s{n}"), session)
        })
        .collect()
}

/// T-PG.14.1: each step of two sessions' interleaved statements is answered as a 19.0 server answered
/// it (scripts/postgres-isolation-answers.sh), and a statement that waits there waits here: it has
/// given no answer until the other session's transaction ends, whose end alone wakes it.
#[test]
fn t_pg_14_1_two_sessions_interleave_as_a_19_server_does() {
    let compared = interleave(
        include_str!("isolation/steps.txt"),
        include_str!("isolation/answers.txt"),
    );
    assert!(compared.waited >= 5, "{compared:?}");
    assert!(compared.messages > 100, "{compared:?}");
}

/// T-PG.15.1: the interleavings of `recheck-steps.txt` (19's eval-plan-qual, deadlock-simple and
/// deadlock-hard specs within the statements Amphora has, and more of their kind) are answered as a
/// 19.0 server answered them: a row the holder committed a later version of is judged again and
/// changed, or passed by and kept locked, on that version alone, and a statement keeps what it changed
/// before it waited; the session whose deadlock check finds a ring is refused, whichever closed it, and
/// one whose `lock_timeout` passes. The deadlines are 19's: here the test says when one passes.
#[test]
fn t_pg_15_1_a_row_is_rechecked_and_a_ring_refused_as_a_19_server_does() {
    let compared = interleave(
        include_str!("isolation/recheck-steps.txt"),
        include_str!("isolation/recheck-answers.txt"),
    );
    assert!(compared.waited >= 30, "{compared:?}");
    assert!(compared.deadlocks >= 3, "{compared:?}");
    assert!(compared.timeouts >= 2, "{compared:?}");
    assert!(compared.messages > 300, "{compared:?}");
}

/// What an interleaving's comparison saw, so that a list that tests nothing fails.
#[derive(Debug, Default)]
struct Compared {
    waited: usize,
    deadlocks: usize,
    timeouts: usize,
    messages: usize,
}

/// Runs `steps`, each answered as `answers` has it.
fn interleave(steps: &str, answers: &str) -> Compared {
    let dir = tempfile::tempdir().unwrap();
    let mut sessions = sessions(&dir);
    let answers = tpcb::answers(answers);
    let steps: Vec<&str> = steps
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .collect();
    assert_eq!(answers.len(), steps.len());
    let mut compared = Compared::default();
    let mut wrong = Vec::new();
    for ((step, sql, theirs), line) in answers.iter().zip(&steps) {
        let (name, kind) = step.split_at(2);
        let kind = match (kind, sql.as_str()) {
            ("", _) => Kind::Runs,
            (">", _) => Kind::Waits,
            ("<", _) => Kind::Ends,
            ("+", _) => Kind::WaitsOn,
            ("!", "-") => Kind::Deadlocks { sent: false },
            ("!", _) => Kind::Deadlocks { sent: true },
            ("~", _) => Kind::TimesOut,
            _ => panic!("{step}"),
        };
        let expected = match kind {
            Kind::Runs => format!("{name}|{sql}"),
            Kind::Waits => format!("{name}>{sql}"),
            Kind::Ends => format!("{name}<"),
            Kind::WaitsOn => format!("{name}+"),
            Kind::Deadlocks { sent: true } => format!("{name}!{sql}"),
            Kind::Deadlocks { sent: false } => format!("{name}!"),
            Kind::TimesOut => format!("{name}~{sql}"),
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
                session.stops(sql, line);
                compared.waited += 1;
                continue;
            }
            Kind::WaitsOn => {
                let flow = session.goes_on(&mut out, line);
                assert_eq!(flow, Flow::Waiting, "{line}");
                session.answered = out;
                session.registers(line);
                compared.waited += 1;
                continue;
            }
            Kind::Ends => {
                let flow = session.goes_on(&mut out, line);
                assert_eq!(flow, Flow::Done, "{line}");
            }
            Kind::Deadlocks { sent } => {
                if sent {
                    session.stops(sql, line);
                }
                assert!(session.executor.check_deadlock(), "{line}");
                compared.deadlocks += 1;
                out = std::mem::take(&mut session.answered);
                let flow = session.executor.resume(&mut out).unwrap();
                assert_eq!(flow, Flow::Done, "{line}");
            }
            Kind::TimesOut => {
                session.stops(sql, line);
                assert!(!session.executor.check_deadlock(), "{line}");
                assert!(session.executor.wait_limits().lock.is_some(), "{line}");
                session.executor.lock_timed_out();
                compared.timeouts += 1;
                out = std::mem::take(&mut session.answered);
                let flow = session.executor.resume(&mut out).unwrap();
                assert_eq!(flow, Flow::Done, "{line}");
            }
        }
        let mut theirs = theirs.clone();
        let (z, body) = theirs.pop().unwrap();
        assert_eq!((z, body[0]), (b'Z', status(&session.executor)), "{line}\n{out:?}");
        let (ours, theirs) = (told(sql, &messages(&out)), told(sql, &theirs));
        if ours != theirs {
            wrong.push(format!("{line}\n  19 {theirs:?}\n  is {ours:?}"));
        }
        compared.messages += theirs.len();
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    compared
}

impl Session {
    /// Sends a statement that waits.
    fn stops(&mut self, sql: &str, line: &str) {
        let mut out = Vec::new();
        let flow = self.executor.simple_query(sql, &mut out).unwrap();
        assert_eq!(flow, Flow::Waiting, "{line}");
        self.answered = out;
        self.registers(line);
    }

    /// The wait is registered when the session polls it, as the listener does.
    fn registers(&mut self, line: &str) {
        let waker = Waker::from(Arc::clone(&self.woken));
        let mut context = Context::from_waker(&waker);
        let polled = self.executor.poll_wait(&mut context);
        assert_eq!(polled, Poll::Pending, "{line}");
        assert_eq!(self.woken.0.swap(0, Ordering::SeqCst), 0, "{line}");
    }

    /// Goes on once its holder has ended.
    fn goes_on(&mut self, out: &mut Vec<u8>, line: &str) -> Flow {
        assert!(
            self.woken.0.swap(0, Ordering::SeqCst) > 0,
            "{line}: the end of the transaction it waits for wakes it"
        );
        let mut context = Context::from_waker(Waker::noop());
        let polled = self.executor.poll_wait(&mut context);
        assert_eq!(polled, Poll::Ready(()), "{line}");
        *out = std::mem::take(&mut self.answered);
        self.executor.resume(out).unwrap()
    }
}

/// Messages as a comparison reads them: a refusal's severity, SQLSTATE, message, detail, hint and
/// position, the numbers of the detail's processes and transactions by their first place in it (19's
/// CONTEXT names a ctid, which Amphora's rows don't have).
fn told(sql: &str, messages: &[(u8, Vec<u8>)]) -> Vec<String> {
    messages
        .iter()
        .zip(tpcb::told(sql, messages))
        .map(|((kind, body), told)| {
            let field = |tag: u8| {
                body.split(|b| *b == 0)
                    .find(|f| f.first() == Some(&tag))
                    .map(|f| String::from_utf8_lossy(&f[1..]).into_owned())
            };
            match kind {
                b'E' => format!(
                    "{told} detail {:?} hint {:?}",
                    field(b'D').map(|detail| numbered(&detail)),
                    field(b'H')
                ),
                _ => told,
            }
        })
        .collect()
}

/// `text` with each number as its place among the distinct numbers in it.
fn numbered(text: &str) -> String {
    let mut seen: Vec<String> = Vec::new();
    let mut made = String::new();
    let mut number = String::new();
    for c in text.chars().chain(std::iter::once('\n')) {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        if !number.is_empty() {
            let at = seen.iter().position(|n| *n == number).unwrap_or_else(|| {
                seen.push(number.clone());
                seen.len() - 1
            });
            made.push_str(&format!("#{at}"));
            number.clear();
        }
        made.push(c);
    }
    made
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Runs,
    Waits,
    /// Its wait ends by its holder's end.
    Ends,
    /// Woken by its holder's end, it waits for another.
    WaitsOn,
    /// Its wait ends by its deadlock check, as it is `sent` or after.
    Deadlocks {
        sent: bool,
    },
    /// Its wait ends by its `lock_timeout`.
    TimesOut,
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
