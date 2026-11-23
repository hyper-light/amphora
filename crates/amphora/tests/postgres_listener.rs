//! The PostgreSQL listener (docs/design/postgres.md, section 11's unit 2) over a real socket, driven
//! by a protocol client written here: SCRAM-SHA-256 against a verifier given in PostgreSQL's stored
//! form, a statement refused `0A000`, a session past `max_connections` or the memory budget refused
//! `53300` after its startup packet, a frame past the budget refused `53200`, and an invalid
//! startup length closed with nothing sent.

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

use std::future::Future;
use std::net::SocketAddr;
use std::rc::Rc;

use amphora::memory::{Budget, Reading, Readings};
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

fn listener(limit: u64, max_connections: usize) -> Listener {
    Listener::new(stored_roles(), Budget::new(Box::new(Limit(limit))), max_connections).unwrap()
}

/// Serves `listener` on a loopback port and runs `client` against it to its end.
fn run<F: Future<Output = ()> + 'static>(listener: Listener, client: impl FnOnce(SocketAddr) -> F + 'static) {
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
    let mut p = u32::try_from(body.len() + 4).unwrap().to_be_bytes().to_vec();
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
                let len = usize::try_from(u32::from_be_bytes(self.held[1..5].try_into().unwrap())).unwrap();
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

    /// Start-up and SCRAM-SHA-256 as `ada`, through to ReadyForQuery.
    async fn log_in(&mut self) {
        self.send(&startup_packet()).await;
        let (tag, body) = self.recv().await.unwrap();
        assert_eq!((tag, &body[..4]), (b'R', &10i32.to_be_bytes()[..]), "AuthenticationSASL");
        assert!(body[4..].starts_with(b"SCRAM-SHA-256\0"));
        let client_first_bare = "n=,r=rOprNGfwEbeRWgbNEkqO";
        let data = format!("n,,{client_first_bare}");
        let mut initial = scram::MECHANISM.as_bytes().to_vec();
        initial.push(0);
        initial.extend_from_slice(&i32::try_from(data.len()).unwrap().to_be_bytes());
        initial.extend_from_slice(data.as_bytes());
        self.send(&frontend(b'p', &initial)).await;
        let (tag, body) = self.recv().await.unwrap();
        assert_eq!((tag, &body[..4]), (b'R', &11i32.to_be_bytes()[..]), "AuthenticationSASLContinue");
        let server_first = String::from_utf8(body[4..].to_vec()).unwrap();
        let attr = |name: &str| server_first.split(',').find_map(|a| a.strip_prefix(name)).unwrap().to_owned();
        let salted = salted_password(&scram::base64_decode(attr("s=").as_bytes()).unwrap(), attr("i=").parse().unwrap());
        let client_key = hmac(&salted, b"Client Key");
        let stored_key: [u8; 32] = Sha256::digest(client_key).into();
        let without_proof = format!("c=biws,r={}", attr("r="));
        let auth_message = format!("{client_first_bare},{server_first},{without_proof}");
        let signature = hmac(&stored_key, auth_message.as_bytes());
        let proof: Vec<u8> = client_key.iter().zip(signature).map(|(k, s)| k ^ s).collect();
        self.send(&frontend(b'p', format!("{without_proof},p={}", scram::base64_encode(&proof)).as_bytes())).await;
        let (tag, body) = self.recv().await.unwrap();
        let server_signature = hmac(&hmac(&salted, b"Server Key"), auth_message.as_bytes());
        assert_eq!(tag, b'R');
        assert_eq!(&body[..4], &12i32.to_be_bytes(), "AuthenticationSASLFinal");
        assert_eq!(body[4..], format!("v={}", scram::base64_encode(&server_signature)).into_bytes()[..], "the server proved it holds the verifier");
        let (tag, body) = self.recv().await.unwrap();
        assert_eq!((tag, body), (b'R', 0i32.to_be_bytes().to_vec()), "AuthenticationOk");
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
        assert_eq!(parameters, 15, "PostgreSQL 19's fifteen ParameterStatus values");
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
    run(listener(u64::MAX, 4), |addr| async move {
        let mut client = Client::connect(addr).await;
        client.log_in().await;
        client.send(&frontend(b'Q', b"SELECT 1\0")).await;
        let (tag, body) = client.recv().await.unwrap();
        assert_eq!(tag, b'E');
        assert!(fields(&body).contains(&(b'C', "0A000".to_owned())), "{:?}", fields(&body));
        assert_eq!(client.recv().await.unwrap(), (b'Z', b"I".to_vec()));
        client.send(&frontend(b'X', b"")).await;
        assert_eq!(client.recv().await, None, "Terminate closes the connection");
    });
}

#[test]
fn t_pg_2_2_a_session_past_max_connections_is_refused_53300_after_its_startup_packet() {
    run(listener(u64::MAX, 1), |addr| async move {
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
        assert_eq!(first.recv().await.unwrap().0, b'E', "the live session still answers");
    });
}

#[test]
fn t_pg_2_2_a_session_the_memory_budget_has_no_room_for_is_refused_53300() {
    run(listener(0, 4), |addr| async move {
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
    run(listener(1 << 16, 4), |addr| async move {
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
    run(listener(u64::MAX, 4), |addr| async move {
        for length in [7u32, 10_009, u32::MAX] {
            let mut client = Client::connect(addr).await;
            client.send(&length.to_be_bytes()).await;
            assert_eq!(client.recv().await, None, "length {length}: closed with nothing sent");
        }
    });
}
