//! The PostgreSQL listener (docs/design/postgres.md, section 11's unit 2) over a real socket, driven
//! by a protocol client written here: SCRAM-SHA-256 against a verifier given in PostgreSQL's stored
//! form, a statement refused `0A000`, a session past `max_connections` or the memory budget refused
//! `53300` after its startup packet, a frame past the budget refused `53200`, and an invalid
//! startup length closed with nothing sent; and pgbench's and psql's statements on two sessions,
//! answered as a PostgreSQL 19 server answered them.

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
use std::future::Future;
use std::net::SocketAddr;
use std::rc::Rc;

use amphora::memory::{Budget, Reading, Readings};
use amphora::postgres::execute::Database;
use amphora::postgres::listener::{Listener, StoredRoles};
use amphora::postgres::scram;
use hmac::{Hmac, Mac};
use hyper_rt::runtime::{LocalRuntime, RuntimeConfig};
use hyper_rt::tcp::{Ipv4Addr, TcpListener, TcpStream};
use sha2::{Digest, Sha256};

const PASSWORD: &str = "pencil";
const SALT: &[u8] = b"0123456789abcdef";

struct Limit(u64);

impl Readings for Limit {
    fn read(&self) -> Reading {
        Reading {
            limit: Some(self.0),
            resident: None,
        }
    }
}

fn config() -> RuntimeConfig {
    RuntimeConfig {
        shards: 1,
        tasks_per_shard: 64,
        timers_per_shard: 64,
        interests_per_shard: 64,
        ring_entries: 64,
        step_budget_ns: 1_000_000_000,
        timer_tick_ns: 100_000,
        batch: 64,
        pin: false,
        cores: Vec::new(),
        page_bytes: 4096,
        spin_ns: 0,
        wake_tracking: None,
        latency_objective_ns: None,
    }
}

fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// RFC 5802 §2.2's Hi, written here apart from the server's.
fn salted_password(salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut block = salt.to_vec();
    block.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac(PASSWORD.as_bytes(), &block);
    let mut out = u;
    for _ in 1..iterations {
        u = hmac(PASSWORD.as_bytes(), &u);
        for (o, x) in out.iter_mut().zip(u) {
            *o ^= x;
        }
    }
    out
}

/// `ada`'s verifier as `pg_authid.rolpassword` keeps it.
fn stored_roles() -> StoredRoles {
    let salted = salted_password(SALT, scram::DEFAULT_ITERATIONS);
    let stored_key: [u8; 32] = Sha256::digest(hmac(&salted, b"Client Key")).into();
    let server_key = hmac(&salted, b"Server Key");
    let line = format!(
        "ada SCRAM-SHA-256${}:{}${}:{} superuser\n",
        scram::DEFAULT_ITERATIONS,
        scram::base64_encode(SALT),
        scram::base64_encode(&stored_key),
        scram::base64_encode(&server_key)
    );
    StoredRoles::parse(&line).unwrap()
}

/// A listener on a database of `dir`'s, its sessions' room `limit` bytes.
fn listener(dir: &tempfile::TempDir, limit: u64, max_connections: usize) -> Listener {
    let path = dir.path().join("postgres.db");
    Listener::new(
        stored_roles(),
        Database::open(
            path.to_str().unwrap(),
            amphora_engine::storage::versioned::memory::machine(),
        )
        .unwrap(),
        Budget::new(Box::new(Limit(limit))),
        max_connections,
    )
    .unwrap()
}

/// Serves `listener` on a loopback port and runs `client` against it to its end.
fn run<F: Future<Output = ()> + 'static>(
    listener: Listener,
    client: impl FnOnce(SocketAddr) -> F + 'static,
) {
    let mut rt = LocalRuntime::new(&config()).unwrap();
    rt.block_on(async move {
        let tcp = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), 8).unwrap();
        let addr = tcp.local_addr().unwrap();
        hyper_rt::futures::spawn_detached(async move {
            Rc::new(listener).serve(tcp).await.unwrap();
        })
        .unwrap();
        client(addr).await;
    })
    .unwrap();
}

fn frontend(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut m = vec![tag];
    m.extend_from_slice(&i32::try_from(body.len() + 4).unwrap().to_be_bytes());
    m.extend_from_slice(body);
    m
}

fn startup_packet() -> Vec<u8> {
    let mut body = 0x0003_0000u32.to_be_bytes().to_vec();
    body.extend_from_slice(b"user\0ada\0database\0ada\0\0");
    let mut p = u32::try_from(body.len() + 4)
        .unwrap()
        .to_be_bytes()
        .to_vec();
    p.extend_from_slice(&body);
    p
}

/// A client's end of one connection, reading whole backend messages.
struct Client {
    stream: TcpStream,
    held: Vec<u8>,
}

impl Client {
    async fn connect(addr: SocketAddr) -> Self {
        Self {
            stream: TcpStream::connect(addr).await.unwrap(),
            held: Vec::new(),
        }
    }

    async fn send(&self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.unwrap();
    }

    /// The next message, or `None` once the server closed between messages.
    async fn recv(&mut self) -> Option<(u8, Vec<u8>)> {
        loop {
            if self.held.len() >= 5 {
                let len = usize::try_from(u32::from_be_bytes(self.held[1..5].try_into().unwrap()))
                    .unwrap();
                if self.held.len() > len {
                    let message: Vec<u8> = self.held.drain(..=len).collect();
                    return Some((message[0], message[5..].to_vec()));
                }
            }
            let mut chunk = [0u8; 4096];
            let n = self.stream.read(&mut chunk).await.unwrap();
            if n == 0 {
                assert!(self.held.is_empty(), "the server closed mid-message");
                return None;
            }
            self.held.extend_from_slice(&chunk[..n]);
        }
    }

    /// A Query's answer, through its ReadyForQuery.
    async fn query(&mut self, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let mut body = sql.as_bytes().to_vec();
        body.push(0);
        self.send(&frontend(b'Q', &body)).await;
        let mut answer = Vec::new();
        loop {
            let message = self.recv().await.unwrap();
            let ended = message.0 == b'Z';
            answer.push(message);
            if ended {
                return answer;
            }
        }
    }

    /// Start-up and SCRAM-SHA-256 as `ada`, through to ReadyForQuery.
    async fn log_in(&mut self) {
        self.send(&startup_packet()).await;
        let (tag, body) = self.recv().await.unwrap();
        assert_eq!(
            (tag, &body[..4]),
            (b'R', &10i32.to_be_bytes()[..]),
            "AuthenticationSASL"
        );
        assert!(body[4..].starts_with(b"SCRAM-SHA-256\0"));
        let client_first_bare = "n=,r=rOprNGfwEbeRWgbNEkqO";
        let data = format!("n,,{client_first_bare}");
        let mut initial = scram::MECHANISM.as_bytes().to_vec();
        initial.push(0);
        initial.extend_from_slice(&i32::try_from(data.len()).unwrap().to_be_bytes());
        initial.extend_from_slice(data.as_bytes());
        self.send(&frontend(b'p', &initial)).await;
        let (tag, body) = self.recv().await.unwrap();
        assert_eq!(
            (tag, &body[..4]),
            (b'R', &11i32.to_be_bytes()[..]),
            "AuthenticationSASLContinue"
        );
        let server_first = String::from_utf8(body[4..].to_vec()).unwrap();
        let attr = |name: &str| {
            server_first
                .split(',')
                .find_map(|a| a.strip_prefix(name))
                .unwrap()
                .to_owned()
        };
        let salted = salted_password(
            &scram::base64_decode(attr("s=").as_bytes()).unwrap(),
            attr("i=").parse().unwrap(),
        );
        let client_key = hmac(&salted, b"Client Key");
        let stored_key: [u8; 32] = Sha256::digest(client_key).into();
        let without_proof = format!("c=biws,r={}", attr("r="));
        let auth_message = format!("{client_first_bare},{server_first},{without_proof}");
        let signature = hmac(&stored_key, auth_message.as_bytes());
        let proof: Vec<u8> = client_key
            .iter()
            .zip(signature)
            .map(|(k, s)| k ^ s)
            .collect();
        self.send(&frontend(
            b'p',
            format!("{without_proof},p={}", scram::base64_encode(&proof)).as_bytes(),
        ))
        .await;
        let (tag, body) = self.recv().await.unwrap();
        let server_signature = hmac(&hmac(&salted, b"Server Key"), auth_message.as_bytes());
        assert_eq!(tag, b'R');
        assert_eq!(&body[..4], &12i32.to_be_bytes(), "AuthenticationSASLFinal");
        assert_eq!(
            body[4..],
            format!("v={}", scram::base64_encode(&server_signature)).into_bytes()[..],
            "the server proved it holds the verifier"
        );
        let (tag, body) = self.recv().await.unwrap();
        assert_eq!(
            (tag, body),
            (b'R', 0i32.to_be_bytes().to_vec()),
            "AuthenticationOk"
        );
        let mut parameters = 0;
        loop {
            match self.recv().await.unwrap() {
                (b'S', _) => parameters += 1,
                (b'K', _) => {}
                (b'Z', body) => {
                    assert_eq!(body, b"I");
                    break;
                }
                other => panic!("unexpected {other:?} before ReadyForQuery"),
            }
        }
        assert_eq!(
            parameters, 15,
            "PostgreSQL 19's fifteen ParameterStatus values"
        );
    }
}

fn fields(body: &[u8]) -> Vec<(u8, String)> {
    body.split(|&b| b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| (f[0], String::from_utf8(f[1..].to_vec()).unwrap()))
        .collect()
}

#[test]
fn t_pg_2_1_a_client_logs_in_with_scram_over_a_socket_and_a_statement_is_refused_0a000() {
    let dir = tempfile::tempdir().unwrap();
    run(listener(&dir, u64::MAX, 4), |addr| async move {
        let mut client = Client::connect(addr).await;
        client.log_in().await;
        // A set operation's rules aren't ported (section 3), so 19 takes it and the parser refuses it.
        client
            .send(&frontend(b'Q', b"SELECT 1 UNION SELECT 2\0"))
            .await;
        let (tag, body) = client.recv().await.unwrap();
        assert_eq!(tag, b'E');
        assert!(
            fields(&body).contains(&(b'C', "0A000".to_owned())),
            "{:?}",
            fields(&body)
        );
        assert_eq!(client.recv().await.unwrap(), (b'Z', b"I".to_vec()));
        client.send(&frontend(b'X', b"")).await;
        assert_eq!(client.recv().await, None, "Terminate closes the connection");
    });
}

#[test]
fn t_pg_2_2_a_session_past_max_connections_is_refused_53300_after_its_startup_packet() {
    let dir = tempfile::tempdir().unwrap();
    run(listener(&dir, u64::MAX, 1), |addr| async move {
        let mut first = Client::connect(addr).await;
        first.log_in().await;
        let mut second = Client::connect(addr).await;
        second.send(&startup_packet()).await;
        let (tag, body) = second.recv().await.unwrap();
        assert_eq!(tag, b'E');
        let fields = fields(&body);
        assert!(fields.contains(&(b'S', "FATAL".to_owned())));
        assert!(fields.contains(&(b'C', "53300".to_owned())));
        assert!(fields.contains(&(b'M', "sorry, too many clients already".to_owned())));
        assert_eq!(second.recv().await, None);
        first.send(&frontend(b'Q', b"SELECT 1\0")).await;
        assert_eq!(
            first.recv().await.unwrap().0,
            b'T',
            "the live session still answers"
        );
    });
}

#[test]
fn t_pg_2_2_a_session_the_memory_budget_has_no_room_for_is_refused_53300() {
    let dir = tempfile::tempdir().unwrap();
    run(listener(&dir, 0, 4), |addr| async move {
        let mut client = Client::connect(addr).await;
        client.send(&startup_packet()).await;
        let (tag, body) = client.recv().await.unwrap();
        assert_eq!(tag, b'E');
        assert!(fields(&body).contains(&(b'C', "53300".to_owned())));
        assert_eq!(client.recv().await, None);
    });
}

#[test]
fn t_pg_2_3_a_frame_past_the_memory_budget_is_refused_53200_before_it_is_held() {
    let dir = tempfile::tempdir().unwrap();
    run(listener(&dir, 1 << 16, 4), |addr| async move {
        let mut client = Client::connect(addr).await;
        client.log_in().await;
        // Only the header goes out: the refusal comes from its length alone.
        let mut header = vec![b'Q'];
        header.extend_from_slice(&(1u32 << 17).to_be_bytes());
        client.send(&header).await;
        let (tag, body) = client.recv().await.unwrap();
        assert_eq!(tag, b'E');
        let fields = fields(&body);
        assert!(fields.contains(&(b'S', "FATAL".to_owned())));
        assert!(fields.contains(&(b'C', "53200".to_owned())));
        assert_eq!(client.recv().await, None);
    });
}

#[test]
fn t_pg_2_4_an_invalid_startup_length_closes_with_nothing_sent() {
    let dir = tempfile::tempdir().unwrap();
    run(listener(&dir, u64::MAX, 4), |addr| async move {
        for length in [7u32, 10_009, u32::MAX] {
            let mut client = Client::connect(addr).await;
            client.send(&length.to_be_bytes()).await;
            assert_eq!(
                client.recv().await,
                None,
                "length {length}: closed with nothing sent"
            );
        }
    });
}

/// T-PG.9.3: the statements psql and pgbench send (pgbench's tables, its rows, its TPC-B script on two
/// sessions statement by statement, ROLLBACK, comparisons, refusals, several statements in one Query),
/// each a Query to the listener, answer as a PostgreSQL 19.0 server answered the same statements
/// (scripts/postgres-tpcb-answers.sh): every message of each, but a table's OID, a CURRENT_TIMESTAMP's
/// value, and an error's or a notice's source file, line and routine.
#[test]
fn t_pg_9_3_psql_and_pgbench_statements_answer_as_a_19_server_does() {
    let dir = tempfile::tempdir().unwrap();
    run(listener(&dir, u64::MAX, 4), |addr| async move {
        let mut sessions = BTreeMap::new();
        for name in ["s1", "s2"] {
            let mut client = Client::connect(addr).await;
            client.log_in().await;
            sessions.insert(name.to_owned(), client);
        }
        let answers = tpcb::answers(include_str!("tpcb/listener-answers.txt"));
        let statements: Vec<&str> = include_str!("tpcb/listener.txt").lines().collect();
        assert_eq!(answers.len(), statements.len());
        let mut wrong = Vec::new();
        let mut compared = 0;
        for ((session, sql, theirs), line) in answers.iter().zip(statements) {
            assert_eq!(line, format!("{session}|{sql}"));
            let ours = sessions.get_mut(session).unwrap().query(sql).await;
            let (got, want) = (tpcb::told(sql, &ours), tpcb::told(sql, theirs));
            compared += want.len();
            if got != want {
                wrong.push(format!(
                    "{session}|{sql}:\n  here: {got:#?}\n  19:   {want:#?}"
                ));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
        assert!(
            compared > 200,
            "every message of every statement is compared: {compared}"
        );
    });
}

/// T-PG.9.3: a client that ends its session in an open transaction leaves none of its writes, and
/// nothing held: the row it wrote is another session's to write.
#[test]
fn t_pg_9_3_a_session_that_ends_in_a_transaction_leaves_nothing_of_it() {
    let dir = tempfile::tempdir().unwrap();
    run(listener(&dir, u64::MAX, 4), |addr| async move {
        let mut first = Client::connect(addr).await;
        first.log_in().await;
        for sql in [
            "create table t(a int not null, b int)",
            "insert into t(a, b) values (1, 10)",
            "begin",
            "update t set b = 20 where a = 1",
        ] {
            let answer = first.query(sql).await;
            assert_eq!(answer[0].0, b'C', "{sql}: {answer:?}");
        }
        first.send(&frontend(b'X', b"")).await;
        assert_eq!(first.recv().await, None, "Terminate closes the connection");
        let mut second = Client::connect(addr).await;
        second.log_in().await;
        let answer = second.query("update t set b = b + 1 where a = 1").await;
        assert_eq!(answer[0], (b'C', b"UPDATE 1\0".to_vec()));
        let answer = second.query("select b from t where a = 1").await;
        assert_eq!(answer[1], (b'D', b"\x00\x01\x00\x00\x00\x0211".to_vec()));
    });
}

/// T-PG.14.3: over sockets, a session whose statement waits for a row another session's open
/// transaction changed is answered when that transaction commits, and stops no other: a third
/// session's statement is answered meanwhile.
#[test]
fn t_pg_14_3_a_waiting_session_holds_no_other_and_is_answered_when_its_holder_commits() {
    let dir = tempfile::tempdir().unwrap();
    run(listener(&dir, u64::MAX, 4), |addr| async move {
        let mut clients = Vec::new();
        for _ in 0..3 {
            let mut client = Client::connect(addr).await;
            client.log_in().await;
            clients.push(client);
        }
        let [holder, waiter, other] = clients.as_mut_slice() else {
            panic!("three clients");
        };
        for sql in [
            "create table t(a int not null, b int)",
            "insert into t(a, b) values (1, 10)",
            "begin",
            "update t set b = b + 1 where a = 1",
        ] {
            let answer = holder.query(sql).await;
            assert_eq!(answer[0].0, b'C', "{sql}: {answer:?}");
        }
        let mut body = b"update t set b = b + 100 where a = 1".to_vec();
        body.push(0);
        waiter.send(&frontend(b'Q', &body)).await;
        let answer = other.query("select b from t where a = 1").await;
        assert_eq!(answer[1], (b'D', b"\x00\x01\x00\x00\x00\x0210".to_vec()));
        let answer = holder.query("commit").await;
        assert_eq!(answer[0], (b'C', b"COMMIT\0".to_vec()));
        assert_eq!(
            waiter.recv().await,
            Some((b'C', b"UPDATE 1\0".to_vec())),
            "the statement that waited applies to the committed value"
        );
        assert_eq!(waiter.recv().await, Some((b'Z', b"I".to_vec())));
        let answer = other.query("select b from t where a = 1").await;
        assert_eq!(answer[1], (b'D', b"\x00\x01\x00\x00\x00\x03111".to_vec()));
    });
}

/// T-PG.15.3: over sockets, the two deadlines end a wait by themselves: of two sessions that wait for
/// each other, the one whose `deadlock_timeout` passes is refused `40P01` and the other, whose own
/// is far off, is answered; and a wait longer than its `lock_timeout` is refused `55P03`.
#[test]
fn t_pg_15_3_a_deadlock_and_a_lock_timeout_end_a_wait_by_themselves() {
    let dir = tempfile::tempdir().unwrap();
    run(listener(&dir, u64::MAX, 4), |addr| async move {
        let mut clients = Vec::new();
        for _ in 0..2 {
            let mut client = Client::connect(addr).await;
            client.log_in().await;
            clients.push(client);
        }
        let [first, second] = clients.as_mut_slice() else {
            panic!("two clients");
        };
        for sql in [
            "create table t(a int not null, b int)",
            "insert into t(a, b) values (1, 10)",
            "insert into t(a, b) values (2, 20)",
            "begin",
            "set deadlock_timeout = '1h'",
            "update t set b = b + 1 where a = 1",
        ] {
            let answer = first.query(sql).await;
            assert_eq!(answer[0].0, b'C', "{sql}: {answer:?}");
        }
        for sql in [
            "begin",
            "set deadlock_timeout = '1ms'",
            "update t set b = b + 1 where a = 2",
        ] {
            let answer = second.query(sql).await;
            assert_eq!(answer[0].0, b'C', "{sql}: {answer:?}");
        }
        first
            .send(&frontend(b'Q', b"update t set b = b + 1 where a = 2\0"))
            .await;
        let answer = second.query("update t set b = b + 1 where a = 1").await;
        let refused = |answer: &[(u8, Vec<u8>)], code: &[u8]| {
            answer[0].0 == b'E' && answer[0].1.windows(code.len()).any(|w| w == code)
        };
        assert!(refused(&answer, b"C40P01"), "{answer:?}");
        assert_eq!(
            first.recv().await,
            Some((b'C', b"UPDATE 1\0".to_vec())),
            "the victim's refusal lets the other go on before its ROLLBACK"
        );
        assert_eq!(first.recv().await, Some((b'Z', b"T".to_vec())));
        second.query("rollback").await;
        second.query("set lock_timeout = '1ms'").await;
        let answer = second.query("update t set b = b + 1 where a = 1").await;
        assert!(refused(&answer, b"C55P03"), "{answer:?}");
        first.query("commit").await;
        let answer = second.query("update t set b = b + 1 where a = 1").await;
        assert_eq!(answer[0], (b'C', b"UPDATE 1\0".to_vec()));
    });
}

/// Parse, Bind of text parameters and Execute of the unnamed statement and portal.
fn statement(sql: &str, params: &[&str]) -> Vec<u8> {
    let count = |n: usize| u16::try_from(n).unwrap().to_be_bytes();
    let mut parse = vec![0];
    parse.extend_from_slice(sql.as_bytes());
    parse.extend_from_slice(&[0, 0, 0]);
    let mut bind = vec![0, 0, 0, 0];
    bind.extend_from_slice(&count(params.len()));
    for param in params {
        bind.extend_from_slice(&u32::try_from(param.len()).unwrap().to_be_bytes());
        bind.extend_from_slice(param.as_bytes());
    }
    bind.extend_from_slice(&count(0));
    let mut messages = frontend(b'P', &parse);
    messages.extend(frontend(b'B', &bind));
    messages.extend(frontend(b'E', &[0, 0, 0, 0, 0]));
    messages
}

/// T-PG.10.2: over sockets, an Execute that waits for a row another session's open transaction changed
/// is answered once that transaction ends, and what a Flush before it asked for is sent meanwhile; one
/// whose `lock_timeout` passes is refused, and the messages until Sync with it, and what its transaction
/// had changed is gone.
#[test]
fn t_pg_10_2_an_execute_waits_and_what_was_flushed_before_it_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    run(listener(&dir, u64::MAX, 4), |addr| async move {
        let mut clients = Vec::new();
        for _ in 0..2 {
            let mut client = Client::connect(addr).await;
            client.log_in().await;
            clients.push(client);
        }
        let [holder, waiter] = clients.as_mut_slice() else {
            panic!("two clients");
        };
        for sql in [
            "create table t(a int not null, b int)",
            "insert into t(a, b) values (1, 10)",
            "insert into t(a, b) values (2, 20)",
            "begin",
            "update t set b = b + 1 where a = 1",
        ] {
            let answer = holder.query(sql).await;
            assert_eq!(answer[0].0, b'C', "{sql}: {answer:?}");
        }
        let mut batch = statement("select b from t where a = $1", &["1"]);
        batch.extend(frontend(b'H', b""));
        batch.extend(statement(
            "update t set b = b + $1 where a = $2",
            &["100", "1"],
        ));
        batch.extend(frontend(b'S', b""));
        waiter.send(&batch).await;
        for expected in [
            (b'1', Vec::new()),
            (b'2', Vec::new()),
            (b'D', b"\x00\x01\x00\x00\x00\x0210".to_vec()),
            (b'C', b"SELECT 1\0".to_vec()),
        ] {
            assert_eq!(
                waiter.recv().await,
                Some(expected),
                "flushed before the wait"
            );
        }
        let answer = holder.query("commit").await;
        assert_eq!(answer[0], (b'C', b"COMMIT\0".to_vec()));
        for expected in [
            (b'1', Vec::new()),
            (b'2', Vec::new()),
            (b'C', b"UPDATE 1\0".to_vec()),
            (b'Z', b"I".to_vec()),
        ] {
            assert_eq!(waiter.recv().await, Some(expected), "once the holder ended");
        }
        let answer = holder.query("select b from t where a = 1").await;
        assert_eq!(answer[1], (b'D', b"\x00\x01\x00\x00\x00\x03111".to_vec()));

        waiter.query("set lock_timeout = '1ms'").await;
        holder.query("begin").await;
        holder.query("update t set b = 0 where a = 1").await;
        let mut batch = statement("update t set b = $1 where a = 2", &["21"]);
        batch.extend(statement("update t set b = $1 where a = 1", &["2"]));
        batch.extend(statement("select b from t where a = 2", &[]));
        batch.extend(frontend(b'S', b""));
        waiter.send(&batch).await;
        for tag in [b'1', b'2', b'C', b'1', b'2'] {
            assert_eq!(waiter.recv().await.unwrap().0, tag);
        }
        let (tag, body) = waiter.recv().await.unwrap();
        assert_eq!(tag, b'E');
        assert!(fields(&body).contains(&(b'C', "55P03".to_owned())));
        assert_eq!(waiter.recv().await, Some((b'Z', b"I".to_vec())));
        holder.query("rollback").await;
        let answer = holder.query("select b from t where a = 2").await;
        assert_eq!(
            answer[1],
            (b'D', b"\x00\x01\x00\x00\x00\x0220".to_vec()),
            "the refusal undid the Execute before it"
        );
    });
}

/// T-PG.10.3: a session's prepared statements are charged to the process's budget: one there is no room
/// for is refused `53200`, the session goes on, and the room a Close gives back is the next one's.
#[test]
fn t_pg_10_3_prepared_statements_are_charged_to_the_budget() {
    let dir = tempfile::tempdir().unwrap();
    run(listener(&dir, 1 << 16, 4), |addr| async move {
        let mut client = Client::connect(addr).await;
        client.log_in().await;
        let parse = |name: &str| {
            let mut body = name.as_bytes().to_vec();
            body.extend_from_slice(b"\0SELECT 1 + 2 + 3\0\0\0");
            frontend(b'P', &body)
        };
        let sync = frontend(b'S', b"");
        let mut held = 0;
        let refused = loop {
            client.send(&parse(&format!("s{held}"))).await;
            client.send(&sync).await;
            let (tag, body) = client.recv().await.unwrap();
            assert_eq!(client.recv().await, Some((b'Z', b"I".to_vec())));
            if tag == b'E' {
                break fields(&body);
            }
            assert_eq!(tag, b'1');
            held += 1;
            assert!(held < 10_000, "65,536 bytes hold this many statements");
        };
        assert!(held > 0);
        assert!(refused.contains(&(b'S', "ERROR".to_owned())), "{refused:?}");
        assert!(refused.contains(&(b'C', "53200".to_owned())), "{refused:?}");
        client.send(&frontend(b'C', b"Ss0\0")).await;
        client.send(&parse("again")).await;
        client.send(&sync).await;
        for tag in [b'3', b'1', b'Z'] {
            assert_eq!(client.recv().await.unwrap().0, tag);
        }
    });
}
