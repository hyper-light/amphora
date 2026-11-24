//! PostgreSQL 19's wire core (docs/design/postgres.md, section 11's unit 1) driven by a protocol
//! client written here: RFC 7677's SCRAM-SHA-256 vectors, start-up at 3.0, 3.2 and 3.9999, the
//! opening's requests, a wrong password, the simple query loop, and hostile input to every parser.

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

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use amphora::postgres::execute::{self, Executor};
use amphora::postgres::scram::{self, Binding, ScramSecret, ScramServer};
use amphora::postgres::session::{
    Config, Next, Role, Roles, Seed, Session, Statements, direct_tls_alpn,
};
use amphora::postgres::wire::{self, PgError, SqlState};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

const PASSWORD: &str = "pencil";
const SALT: [u8; 16] = *b"0123456789abcdef";
const CERT_HASH: [u8; 32] = [0xab; 32];

struct OneRole {
    secret: ScramSecret,
}

impl Roles for OneRole {
    fn role(&self, user: &str) -> Option<Role> {
        (user == "ada").then(|| Role {
            secret: self.secret.clone(),
            superuser: true,
        })
    }
}

/// One role, its verifier derived once: 4096 rounds per session would dominate the hostile runs.
fn roles() -> &'static OneRole {
    static ROLES: OnceLock<OneRole> = OnceLock::new();
    ROLES.get_or_init(|| OneRole {
        secret: ScramSecret::derive(PASSWORD.as_bytes(), &SALT, scram::DEFAULT_ITERATIONS).unwrap(),
    })
}

fn config(tls: bool) -> Config {
    Config {
        tls: tls.then(|| Binding::TlsServerEndPoint(CERT_HASH.to_vec())),
        time_zone: "UTC".into(),
        mock_nonce: [9; 32],
    }
}

fn seed() -> Seed {
    let mut cancel_key = [0u8; 32];
    for (i, b) in cancel_key.iter_mut().enumerate() {
        *b = u8::try_from(i).unwrap().wrapping_mul(7).wrapping_add(1);
    }
    Seed {
        process_id: 4242,
        cancel_key,
        scram_nonce: [5; scram::RAW_NONCE_LEN],
    }
}

/// How the client hands bytes to the server: all a frame needs at once, or one byte at a time.
#[derive(Clone, Copy)]
enum Pace {
    Frames,
    Bytes,
}

/// Feeds `input` to the session as a shell would: hold what a Need asks (charging it first), step,
/// drop what was consumed. Returns the server's bytes and what ended the run (`None`: input ran out).
fn drive(
    session: &mut Session,
    roles: &OneRole,
    input: &[u8],
    pace: Pace,
) -> (Vec<u8>, Option<Next>) {
    let mut held = Vec::new();
    let mut fed = 0;
    let mut out = Vec::new();
    loop {
        let step = session
            .step(&held, roles, &mut out)
            .expect("a step writes its answer");
        assert!(step.consumed <= held.len(), "consumed only what was held");
        held.drain(..step.consumed);
        match step.next {
            Next::Continue | Next::Flush => {}
            Next::Need(total) => {
                assert!(total > held.len(), "a Need asks for more than is held");
                assert!(
                    total <= wire::LARGE_MESSAGE_LIMIT + 5,
                    "a Need is bounded by the protocol's limit"
                );
                if fed == input.len() {
                    return (out, None);
                }
                let want = match pace {
                    Pace::Frames => total - held.len(),
                    Pace::Bytes => 1,
                };
                let end = (fed + want).min(input.len());
                held.extend_from_slice(&input[fed..end]);
                fed = end;
            }
            other => return (out, Some(other)),
        }
    }
}

fn startup_packet(version: u32, params: &[(&str, &str)]) -> Vec<u8> {
    let mut body = version.to_be_bytes().to_vec();
    for (name, value) in params {
        body.extend_from_slice(name.as_bytes());
        body.push(0);
        body.extend_from_slice(value.as_bytes());
        body.push(0);
    }
    body.push(0);
    let mut packet = u32::try_from(body.len() + 4)
        .unwrap()
        .to_be_bytes()
        .to_vec();
    packet.extend_from_slice(&body);
    packet
}

fn request(code: u32, rest: &[u8]) -> Vec<u8> {
    let mut packet = u32::try_from(8 + rest.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    packet.extend_from_slice(&code.to_be_bytes());
    packet.extend_from_slice(rest);
    packet
}

fn frontend(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut message = vec![tag];
    message.extend_from_slice(&u32::try_from(body.len() + 4).unwrap().to_be_bytes());
    message.extend_from_slice(body);
    message
}

fn query(sql: &[u8]) -> Vec<u8> {
    let mut body = sql.to_vec();
    body.push(0);
    frontend(b'Q', &body)
}

/// The server's messages, split by their length words.
fn messages(mut bytes: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut all = Vec::new();
    while !bytes.is_empty() {
        let tag = bytes[0];
        let len = usize::try_from(u32::from_be_bytes(bytes[1..5].try_into().unwrap())).unwrap();
        all.push((tag, bytes[5..1 + len].to_vec()));
        bytes = &bytes[1 + len..];
    }
    all
}

fn i32_at(body: &[u8], at: usize) -> i32 {
    i32::from_be_bytes(body[at..at + 4].try_into().unwrap())
}

/// An ErrorResponse's fields by their codes.
fn error_fields(body: &[u8]) -> Vec<(u8, String)> {
    body.split(|&b| b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| (f[0], String::from_utf8(f[1..].to_vec()).unwrap()))
        .collect()
}

fn sqlstate(body: &[u8]) -> String {
    error_fields(body)
        .into_iter()
        .find(|(c, _)| *c == b'C')
        .unwrap()
        .1
}

fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// RFC 5802 §2.2's Hi, written here apart from the server's.
fn salted_password(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut block = salt.to_vec();
    block.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac(password, &block);
    let mut out = u;
    for _ in 1..iterations {
        u = hmac(password, &u);
        for (o, x) in out.iter_mut().zip(u) {
            *o ^= x;
        }
    }
    out
}

/// The client's half of SCRAM: the client-final-message for `server_first`, and the
/// server-final-message it expects back.
fn client_final(
    password: &str,
    client_first_bare: &str,
    server_first: &str,
    cbind: &[u8],
) -> (String, String) {
    let attr = |name: &str| {
        server_first
            .split(',')
            .find_map(|a| a.strip_prefix(name))
            .unwrap()
            .to_owned()
    };
    let salt = scram::base64_decode(attr("s=").as_bytes()).unwrap();
    let iterations: u32 = attr("i=").parse().unwrap();
    let salted = salted_password(password.as_bytes(), &salt, iterations);
    let client_key = hmac(&salted, b"Client Key");
    let stored_key: [u8; 32] = Sha256::digest(client_key).into();
    let without_proof = format!("c={},r={}", scram::base64_encode(cbind), attr("r="));
    let auth_message = format!("{client_first_bare},{server_first},{without_proof}");
    let signature = hmac(&stored_key, auth_message.as_bytes());
    let proof: Vec<u8> = client_key
        .iter()
        .zip(signature)
        .map(|(k, s)| k ^ s)
        .collect();
    let server_key = hmac(&salted, b"Server Key");
    let expected = format!(
        "v={}",
        scram::base64_encode(&hmac(&server_key, auth_message.as_bytes()))
    );
    (
        format!("{without_proof},p={}", scram::base64_encode(&proof)),
        expected,
    )
}

fn sasl_initial(mechanism: &str, data: &[u8]) -> Vec<u8> {
    let mut body = mechanism.as_bytes().to_vec();
    body.push(0);
    body.extend_from_slice(&i32::try_from(data.len()).unwrap().to_be_bytes());
    body.extend_from_slice(data);
    frontend(b'p', &body)
}

/// A whole connection: start-up, SCRAM with `password`, then `after`. Returns the server's
/// messages and what ended the run.
fn connect(
    version: u32,
    params: &[(&str, &str)],
    password: &str,
    after: &[u8],
    pace: Pace,
) -> (Vec<(u8, Vec<u8>)>, Option<Next>) {
    let roles = roles();
    let mut session = Session::new(config(false), seed());
    let (out, next) = drive(&mut session, roles, &startup_packet(version, params), pace);
    assert_eq!(next, None);
    let mut all = messages(&out);
    let bare = "n=,r=clientnonce123";
    let (out, next) = drive(
        &mut session,
        roles,
        &sasl_initial(scram::MECHANISM, format!("n,,{bare}").as_bytes()),
        pace,
    );
    assert_eq!(next, None);
    let first = messages(&out);
    let (tag, body) = &first[0];
    assert_eq!(
        (*tag, i32_at(body, 0)),
        (b'R', 11),
        "AuthenticationSASLContinue"
    );
    let server_first = String::from_utf8(body[4..].to_vec()).unwrap();
    all.extend(first);
    let (final_message, _) = client_final(password, bare, &server_first, b"n,,");
    let mut rest = frontend(b'p', final_message.as_bytes());
    rest.extend_from_slice(after);
    let (out, next) = drive(&mut session, roles, &rest, pace);
    all.extend(messages(&out));
    (all, next)
}

const REPORTED: [&str; 15] = [
    "application_name",
    "client_encoding",
    "DateStyle",
    "default_transaction_read_only",
    "in_hot_standby",
    "integer_datetimes",
    "IntervalStyle",
    "is_superuser",
    "scram_iterations",
    "search_path",
    "server_encoding",
    "server_version",
    "session_authorization",
    "standard_conforming_strings",
    "TimeZone",
];

#[test]
fn t_pg_1_1_rfc7677_scram_sha_256_vectors() {
    // RFC 7677 §3: user "user", password "pencil".
    let salt = scram::base64_decode(b"W22ZaJ0SNY7soEsUEjb6gQ==").unwrap();
    let secret = ScramSecret::derive(b"pencil", &salt, 4096).unwrap();
    let stored = format!(
        "SCRAM-SHA-256$4096:W22ZaJ0SNY7soEsUEjb6gQ==${}:{}",
        scram::base64_encode(&secret.stored_key),
        scram::base64_encode(&secret.server_key)
    );
    assert_eq!(
        ScramSecret::parse(&stored),
        Some(secret.clone()),
        "PostgreSQL's stored form reads back"
    );
    let server_nonce = "%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0";
    let (exchange, server_first) = ScramServer::first(
        secret.clone(),
        false,
        &Binding::None,
        scram::MECHANISM,
        server_nonce,
        b"n,,n=user,r=rOprNGfwEbeRWgbNEkqO",
    )
    .unwrap();
    assert_eq!(
        server_first,
        "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096"
    );
    let client_final = b"c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=";
    assert_eq!(
        exchange.last(client_final).unwrap().as_deref(),
        Some("v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=")
    );

    let wrong = b"c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let (exchange, _) = ScramServer::first(
        secret.clone(),
        false,
        &Binding::None,
        scram::MECHANISM,
        server_nonce,
        b"n,,n=user,r=rOprNGfwEbeRWgbNEkqO",
    )
    .unwrap();
    assert_eq!(exchange.last(wrong).unwrap(), None, "a wrong proof fails");
    let (exchange, _) = ScramServer::first(
        secret,
        true,
        &Binding::None,
        scram::MECHANISM,
        server_nonce,
        b"n,,n=user,r=rOprNGfwEbeRWgbNEkqO",
    )
    .unwrap();
    assert_eq!(
        exchange.last(client_final).unwrap(),
        None,
        "a doomed exchange fails even with the right proof"
    );
}

#[test]
fn t_pg_1_1_base64_is_rfc4648s_and_strict() {
    for (plain, coded) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(scram::base64_encode(plain.as_bytes()), coded);
        assert_eq!(
            scram::base64_decode(coded.as_bytes()).unwrap(),
            plain.as_bytes()
        );
    }
    for bad in [
        "Zg=", "Zh==", "Zm9=", "Z===", "Zg==Zg==", "Zm9v!A==", "====",
    ] {
        assert_eq!(
            scram::base64_decode(bad.as_bytes()),
            None,
            "{bad} is refused"
        );
    }
}

#[test]
fn t_pg_1_1_scram_sha_256_plus_binds_to_the_certificate() {
    let secret = ScramSecret::derive(b"pencil", &SALT, 4096).unwrap();
    let binding = Binding::TlsServerEndPoint(CERT_HASH.to_vec());
    assert_eq!(
        binding.mechanisms(),
        [scram::MECHANISM_PLUS, scram::MECHANISM]
    );
    assert_eq!(Binding::None.mechanisms(), [scram::MECHANISM]);
    let bare = "n=,r=abc";
    let header = "p=tls-server-end-point,,";
    let mut cbind = header.as_bytes().to_vec();
    cbind.extend_from_slice(&CERT_HASH);

    let (exchange, server_first) = ScramServer::first(
        secret.clone(),
        false,
        &binding,
        scram::MECHANISM_PLUS,
        "srv",
        format!("{header}{bare}").as_bytes(),
    )
    .unwrap();
    let (message, expected) = client_final("pencil", bare, &server_first, &cbind);
    assert_eq!(
        exchange.last(message.as_bytes()).unwrap(),
        Some(expected),
        "the bound exchange succeeds"
    );

    let (exchange, server_first) = ScramServer::first(
        secret.clone(),
        false,
        &binding,
        scram::MECHANISM_PLUS,
        "srv",
        format!("{header}{bare}").as_bytes(),
    )
    .unwrap();
    let mut other = header.as_bytes().to_vec();
    other.extend_from_slice(&[0xcd; 32]);
    let (message, _) = client_final("pencil", bare, &server_first, &other);
    let error = exchange.last(message.as_bytes()).unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str()),
        (
            SqlState::INVALID_AUTHORIZATION_SPECIFICATION,
            "SCRAM channel binding check failed"
        )
    );

    let refused = |binding: &Binding, mechanism: &str, first: &str| -> PgError {
        ScramServer::first(
            secret.clone(),
            false,
            binding,
            mechanism,
            "srv",
            first.as_bytes(),
        )
        .unwrap_err()
    };
    let error = refused(&binding, scram::MECHANISM, "y,,n=,r=abc");
    assert_eq!(
        (error.code, error.message.as_str()),
        (
            SqlState::INVALID_AUTHORIZATION_SPECIFICATION,
            "SCRAM channel binding negotiation error"
        )
    );
    assert_eq!(
        refused(&binding, scram::MECHANISM_PLUS, "n,,n=,r=abc").message,
        "malformed SCRAM message"
    );
    assert_eq!(
        refused(
            &binding,
            scram::MECHANISM,
            "p=tls-server-end-point,,n=,r=abc"
        )
        .message,
        "malformed SCRAM message"
    );
    assert_eq!(
        refused(&binding, scram::MECHANISM_PLUS, "p=tls-unique,,n=,r=abc").message,
        "unsupported SCRAM channel-binding type \"tls-unique\""
    );
    assert_eq!(
        refused(
            &Binding::None,
            scram::MECHANISM_PLUS,
            "p=tls-server-end-point,,n=,r=abc"
        )
        .code,
        SqlState::PROTOCOL_VIOLATION
    );
    assert_eq!(
        refused(&Binding::None, scram::MECHANISM, "n,a=ada,n=,r=abc").code,
        SqlState::FEATURE_NOT_SUPPORTED
    );
    assert_eq!(
        refused(&Binding::None, scram::MECHANISM, "n,,m=ext,n=,r=abc").code,
        SqlState::FEATURE_NOT_SUPPORTED
    );
    assert_eq!(
        refused(&Binding::None, scram::MECHANISM, "n,,n=,r=a\u{7f}c").message,
        "non-printable characters in SCRAM nonce"
    );
    assert_eq!(
        refused(&Binding::None, "MD5", "n,,n=,r=abc").code,
        SqlState::PROTOCOL_VIOLATION
    );
    // Without TLS, "y" says the client could bind but thinks the server can't: true, so it goes on.
    assert!(
        ScramServer::first(
            secret.clone(),
            false,
            &Binding::None,
            scram::MECHANISM,
            "srv",
            b"y,,n=,r=abc"
        )
        .is_ok()
    );
    let (exchange, _) = ScramServer::first(
        secret,
        false,
        &Binding::None,
        scram::MECHANISM,
        "srv",
        b"n,,n=,r=abc",
    )
    .unwrap();
    let error = exchange
        .last(b"c=biws,r=abcWRONG,p=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
        .unwrap_err();
    assert_eq!(
        (error.message.as_str(), error.detail.as_deref()),
        ("invalid SCRAM response", Some("Nonce does not match."))
    );
}

#[test]
fn t_pg_1_2_startup_at_3_0_and_3_2_reports_and_keys_by_version() {
    for (version, key_len) in [(wire::PROTOCOL_3_0, 4), (wire::PROTOCOL_3_2, 32)] {
        for pace in [Pace::Frames, Pace::Bytes] {
            let (all, next) = connect(
                version,
                &[("user", "ada"), ("application_name", "psql")],
                PASSWORD,
                &[],
                pace,
            );
            assert_eq!(next, None);
            assert_eq!(
                all[0].0, b'R',
                "no NegotiateProtocolVersion for a version served"
            );
            assert_eq!(i32_at(&all[0].1, 0), 10);
            assert_eq!(
                &all[0].1[4..],
                b"SCRAM-SHA-256\0\0",
                "SCRAM-SHA-256 alone without TLS"
            );
            let tags: Vec<u8> = all.iter().map(|(t, _)| *t).collect();
            let mut expected = b"RRRR".to_vec();
            expected.extend([b'S'; 15]);
            expected.extend(b"KZ");
            assert_eq!(tags, expected);
            assert_eq!(i32_at(&all[3].1, 0), 0, "AuthenticationOk");
            let key = &all[19].1;
            assert_eq!(i32_at(key, 0), 4242);
            assert_eq!(
                &key[4..],
                &seed().cancel_key[..key_len],
                "BackendKeyData's key is {key_len} bytes"
            );
            assert_eq!(all[20].1, b"I");
        }
    }
}

#[test]
fn t_pg_1_2_grease_3_9999_is_negotiated_down_never_refused() {
    let (all, next) = connect(
        (3 << 16) | 9999,
        &[("user", "ada"), ("_pq_.test_protocol_negotiation", "x")],
        PASSWORD,
        &[],
        Pace::Frames,
    );
    assert_eq!(next, None);
    let (tag, body) = &all[0];
    assert_eq!(*tag, b'v', "NegotiateProtocolVersion first");
    assert_eq!(
        u32::from_be_bytes(body[0..4].try_into().unwrap()),
        wire::PROTOCOL_3_2
    );
    assert_eq!(i32_at(body, 4), 1);
    assert_eq!(&body[8..], b"_pq_.test_protocol_negotiation\0");
    assert_eq!(
        all.last().unwrap(),
        &(b'Z', b"I".to_vec()),
        "the session carries on to ReadyForQuery"
    );
    assert_eq!(
        all[all.len() - 2].1.len(),
        36,
        "and speaks 3.2: a 32-byte cancel key"
    );

    let (all, _) = connect(
        wire::PROTOCOL_3_0,
        &[("user", "ada"), ("_pq_.other", "1")],
        PASSWORD,
        &[],
        Pace::Frames,
    );
    assert_eq!(all[0].0, b'v');
    assert_eq!(
        u32::from_be_bytes(all[0].1[0..4].try_into().unwrap()),
        wire::PROTOCOL_3_0,
        "the client's version when it is served"
    );

    let roles = roles();
    let mut session = Session::new(config(false), seed());
    let (out, next) = drive(
        &mut session,
        roles,
        &startup_packet(2 << 16, &[("user", "ada")]),
        Pace::Frames,
    );
    assert_eq!(next, Some(Next::Close));
    let all = messages(&out);
    assert_eq!(
        sqlstate(&all[0].1),
        "0A000",
        "a major version other than 3 is refused"
    );
}

#[test]
fn t_pg_1_3_parameter_status_is_postgresql_19s_fifteen() {
    let (all, _) = connect(
        wire::PROTOCOL_3_2,
        &[
            ("user", "ada"),
            ("application_name", "pgbench"),
            ("client_encoding", "UTF8"),
        ],
        PASSWORD,
        &[],
        Pace::Frames,
    );
    let reported: Vec<(String, String)> = all
        .iter()
        .filter(|(t, _)| *t == b'S')
        .map(|(_, body)| {
            let mut parts = body.split(|&b| b == 0);
            let name = String::from_utf8(parts.next().unwrap().to_vec()).unwrap();
            (
                name,
                String::from_utf8(parts.next().unwrap().to_vec()).unwrap(),
            )
        })
        .collect();
    let names: BTreeSet<&str> = reported.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, REPORTED.into_iter().collect());
    let value = |name: &str| reported.iter().find(|(n, _)| n == name).unwrap().1.clone();
    assert_eq!(value("server_version"), "19.0");
    assert_eq!(value("application_name"), "pgbench");
    assert_eq!(value("session_authorization"), "ada");
    assert_eq!(value("is_superuser"), "on");
    assert_eq!(value("standard_conforming_strings"), "on");
    assert_eq!(value("scram_iterations"), "4096");
    assert_eq!(value("TimeZone"), "UTC");
    assert_eq!(value("client_encoding"), "UTF8");

    let roles = roles();
    let mut session = Session::new(config(false), seed());
    let (out, next) = drive(
        &mut session,
        roles,
        &startup_packet(
            wire::PROTOCOL_3_2,
            &[("user", "ada"), ("client_encoding", "LATIN1")],
        ),
        Pace::Frames,
    );
    assert_eq!(next, Some(Next::Close));
    assert_eq!(
        sqlstate(&messages(&out)[0].1),
        "0A000",
        "an encoding needing conversion is refused, typed"
    );
    let mut session = Session::new(config(false), seed());
    let (out, next) = drive(
        &mut session,
        roles,
        &startup_packet(wire::PROTOCOL_3_2, &[("database", "db")]),
        Pace::Frames,
    );
    assert_eq!(next, Some(Next::Close));
    assert_eq!(
        sqlstate(&messages(&out)[0].1),
        "28000",
        "a start-up with no user is refused"
    );
}

#[test]
fn t_pg_1_4_ssl_gss_and_direct_tls_are_decided() {
    let roles = roles();
    let mut session = Session::new(config(false), seed());
    let mut input = request(wire::SSL_REQUEST_CODE, &[]);
    let (out, next) = drive(&mut session, roles, &input, Pace::Frames);
    assert_eq!(
        (out.as_slice(), next),
        (b"N".as_slice(), None),
        "SSLRequest answered N without TLS"
    );
    input = request(wire::GSS_REQUEST_CODE, &[]);
    let (out, next) = drive(&mut session, roles, &input, Pace::Frames);
    assert_eq!(
        (out.as_slice(), next),
        (b"N".as_slice(), None),
        "GSSENCRequest answered N"
    );
    let (out, next) = drive(
        &mut session,
        roles,
        &startup_packet(wire::PROTOCOL_3_2, &[("user", "ada")]),
        Pace::Frames,
    );
    assert_eq!(
        (messages(&out)[0].0, next),
        (b'R', None),
        "the start-up follows"
    );

    let mut session = Session::new(config(true), seed());
    let (out, next) = drive(
        &mut session,
        roles,
        &request(wire::SSL_REQUEST_CODE, &[]),
        Pace::Frames,
    );
    assert_eq!(
        (out.as_slice(), next),
        (b"S".as_slice(), Some(Next::StartTls)),
        "SSLRequest answered S with TLS"
    );
    let (out, _) = drive(
        &mut session,
        roles,
        &startup_packet(wire::PROTOCOL_3_2, &[("user", "ada")]),
        Pace::Frames,
    );
    assert_eq!(
        &messages(&out)[0].1[4..],
        b"SCRAM-SHA-256-PLUS\0SCRAM-SHA-256\0\0",
        "PLUS offered on TLS"
    );

    let mut session = Session::new(config(true), seed());
    let mut both = request(wire::SSL_REQUEST_CODE, &[]);
    both.extend_from_slice(&startup_packet(wire::PROTOCOL_3_2, &[("user", "ada")]));
    let mut out = Vec::new();
    let step = session.step(&both, roles, &mut out).unwrap();
    assert_eq!(step.next, Next::Close);
    assert_eq!(
        sqlstate(&messages(&out)[0].1),
        "08P01",
        "bytes sent before the handshake are refused"
    );

    let mut session = Session::new(config(false), seed());
    let mut input = request(wire::SSL_REQUEST_CODE, &[]);
    input.extend_from_slice(&request(wire::SSL_REQUEST_CODE, &[]));
    let (out, next) = drive(&mut session, roles, &input, Pace::Frames);
    assert_eq!(next, Some(Next::Close));
    assert_eq!(
        sqlstate(&messages(&out[1..])[0].1),
        "0A000",
        "a second SSLRequest is an unsupported version"
    );

    let hello = [0x16, 0x03, 0x01, 0x00, 0x10];
    let mut session = Session::new(config(false), seed());
    let (out, next) = drive(&mut session, roles, &hello, Pace::Frames);
    assert_eq!(
        (out.len(), next),
        (0, Some(Next::Close)),
        "direct TLS without TLS: closed with no reply"
    );
    let mut session = Session::new(config(true), seed());
    let (_, next) = drive(&mut session, roles, &hello, Pace::Frames);
    assert_eq!(next, Some(Next::StartDirectTls));
    assert!(direct_tls_alpn(Some(b"postgresql")).is_ok());
    assert_eq!(
        direct_tls_alpn(None).unwrap_err().code,
        SqlState::PROTOCOL_VIOLATION
    );
    assert_eq!(
        direct_tls_alpn(Some(b"http/1.1")).unwrap_err().code,
        SqlState::PROTOCOL_VIOLATION
    );

    for (key_len, next) in [(4, true), (32, true), (256, true), (0, false), (257, false)] {
        let mut session = Session::new(config(false), seed());
        let mut rest = 77i32.to_be_bytes().to_vec();
        rest.extend(vec![3u8; key_len]);
        let (out, got) = drive(
            &mut session,
            roles,
            &request(wire::CANCEL_REQUEST_CODE, &rest),
            Pace::Frames,
        );
        assert!(out.is_empty(), "a CancelRequest is never answered");
        let expected = if next {
            Next::Cancel {
                process_id: 77,
                key: vec![3; key_len],
            }
        } else {
            Next::Close
        };
        assert_eq!(got, Some(expected), "cancel key of {key_len} bytes");
    }
}

#[test]
fn t_pg_1_5_a_wrong_password_and_an_unknown_role_are_refused_28p01() {
    let (all, next) = connect(
        wire::PROTOCOL_3_2,
        &[("user", "ada")],
        "wrong",
        &[],
        Pace::Frames,
    );
    assert_eq!(next, Some(Next::Close));
    let (tag, body) = all.last().unwrap();
    assert_eq!(*tag, b'E');
    let fields = error_fields(body);
    assert!(fields.contains(&(b'S', "FATAL".into())));
    assert!(fields.contains(&(b'C', "28P01".into())));
    assert!(fields.contains(&(
        b'M',
        "password authentication failed for user \"ada\"".into()
    )));

    let (all, next) = connect(
        wire::PROTOCOL_3_2,
        &[("user", "nobody")],
        PASSWORD,
        &[],
        Pace::Frames,
    );
    assert_eq!(next, Some(Next::Close));
    assert_eq!(
        all[1].0, b'R',
        "an unknown role still gets a server-first-message"
    );
    assert_eq!(
        sqlstate(&all.last().unwrap().1),
        "28P01",
        "and fails as a wrong password does"
    );
}

#[test]
fn t_pg_1_6_a_query_is_refused_0a000_then_ready() {
    let mut after = query(b"SELECT 1");
    after.extend(query(b" ; -- nothing\n/* at /* all */ */"));
    after.extend(query(b"SELECT '\xff'"));
    after.extend(frontend(b'P', b"\0SELECT 1\0\0\0"));
    after.extend(frontend(b'B', b"\0\0\0\0\0\0\0\0"));
    after.extend(frontend(b'E', b"\0\0\0\0\0"));
    after.extend(frontend(b'H', b""));
    after.extend(frontend(b'S', b""));
    after.extend(frontend(b'd', b"stray copy data"));
    after.extend(frontend(b'X', b""));
    for pace in [Pace::Frames, Pace::Bytes] {
        let (all, next) = connect(
            wire::PROTOCOL_3_2,
            &[("user", "ada")],
            PASSWORD,
            &after,
            pace,
        );
        assert_eq!(next, Some(Next::Close), "Terminate ends the session");
        let tail: Vec<(u8, Vec<u8>)> = all.into_iter().skip(21).collect();
        let tags: Vec<u8> = tail.iter().map(|(t, _)| *t).collect();
        assert_eq!(tags, *b"EZIZEZEZ");
        assert_eq!(sqlstate(&tail[0].1), "0A000");
        assert!(error_fields(&tail[0].1).contains(&(b'S', "ERROR".into())));
        assert_eq!(tail[1].1, b"I", "ReadyForQuery idle after the error");
        assert_eq!(sqlstate(&tail[4].1), "22021", "a query that isn't UTF-8");
        assert_eq!(
            sqlstate(&tail[6].1),
            "0A000",
            "one error for the extended messages, then skipped to Sync"
        );
    }
}

/// The input that brings a fresh session to each parser: the opening, SASL's two messages, and the
/// ready loop.
fn prefixes() -> Vec<(&'static str, Vec<u8>)> {
    let start = startup_packet(wire::PROTOCOL_3_2, &[("user", "ada")]);
    let mut sasl_final = start.clone();
    sasl_final.extend(sasl_initial(scram::MECHANISM, b"n,,n=,r=abc"));
    let roles = roles();
    let mut session = Session::new(config(false), seed());
    let (out, _) = drive(&mut session, roles, &sasl_final, Pace::Frames);
    let server_first = String::from_utf8(messages(&out)[1].1[4..].to_vec()).unwrap();
    let (message, _) = client_final(PASSWORD, "n=,r=abc", &server_first, b"n,,");
    let mut ready = sasl_final.clone();
    ready.extend(frontend(b'p', message.as_bytes()));
    vec![
        ("opening", Vec::new()),
        ("sasl initial", start),
        ("sasl final", sasl_final),
        ("ready", ready),
    ]
}

/// Runs `prefix` then `hostile` and checks what a hostile input may do: never panic, never an
/// `Err`, and a closed session's last words an ErrorResponse. Says whether it closed with one.
fn survives(phase: &str, prefix: &[u8], hostile: &[u8], pace: Pace) -> bool {
    let roles = roles();
    let mut session = Session::new(config(false), seed());
    let (_, next) = drive(&mut session, roles, prefix, Pace::Frames);
    assert_eq!(next, None, "the prefix leaves the {phase} parser waiting");
    let (out, next) = drive(&mut session, roles, hostile, pace);
    match next {
        Some(Next::Close) if !out.is_empty() => {
            let all = messages(&out);
            assert_eq!(
                all.last().unwrap().0,
                b'E',
                "the {phase} parser closed for {hostile:?} says why"
            );
            true
        }
        Some(Next::CloseLogged(_)) => true,
        _ => false,
    }
}

#[test]
fn t_pg_1_7_hostile_input_to_every_parser() {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let valid: Vec<Vec<u8>> = vec![
        startup_packet(wire::PROTOCOL_3_2, &[("user", "ada"), ("database", "d")]),
        sasl_initial(scram::MECHANISM, b"n,,n=,r=abc"),
        frontend(b'p', b"c=biws,r=abc,p=AAAA"),
        query(b"SELECT 1"),
        frontend(b'P', b"\0SELECT 1\0\0\0"),
    ];
    for (phase, prefix) in prefixes() {
        let mut refused = 0;
        for message in &valid {
            for cut in 0..message.len() {
                survives(phase, &prefix, &message[..cut], Pace::Frames);
            }
            for _ in 0..64 {
                let mut mutated = message.clone();
                let at = usize::try_from(random() % u64::try_from(mutated.len()).unwrap()).unwrap();
                mutated[at] = u8::try_from(random() & 0xff).unwrap();
                refused += usize::from(survives(phase, &prefix, &mutated, Pace::Bytes));
            }
        }
        for tag in 0..=255u8 {
            for len in [
                0u32,
                1,
                3,
                4,
                5,
                9,
                10_004,
                10_005,
                65_539,
                65_540,
                0x3fff_ffff,
                0x7fff_ffff,
                0xffff_ffff,
            ] {
                let mut frame = vec![tag];
                frame.extend_from_slice(&len.to_be_bytes());
                frame.extend_from_slice(&[0; 16]);
                refused += usize::from(survives(phase, &prefix, &frame, Pace::Frames));
            }
        }
        for _ in 0..256 {
            let len = usize::try_from(random() % 64).unwrap();
            let noise: Vec<u8> = (0..len)
                .map(|_| u8::try_from(random() & 0xff).unwrap())
                .collect();
            refused += usize::from(survives(phase, &prefix, &noise, Pace::Bytes));
        }
        assert!(
            refused > 1000,
            "the {phase} parser refused only {refused} hostile inputs: the runs reached it"
        );
    }
}

#[test]
fn t_pg_1_7_lengths_past_each_limit_are_refused_before_they_are_held() {
    let roles = roles();
    let refused = |prefix: &[u8], frame: &[u8]| {
        let mut session = Session::new(config(false), seed());
        let (_, next) = drive(&mut session, roles, prefix, Pace::Frames);
        assert_eq!(next, None);
        let mut out = Vec::new();
        let step = session.step(frame, roles, &mut out).unwrap();
        assert_eq!(step.next, Next::Close, "refused from the header alone");
        assert_eq!(sqlstate(&messages(&out)[0].1), "08P01");
    };
    let header = |tag: u8, counted: u32| {
        let mut h = vec![tag];
        h.extend_from_slice(&counted.to_be_bytes());
        h
    };
    let [_, start, sasl_final, ready] = <[(&str, Vec<u8>); 4]>::try_from(prefixes()).unwrap();
    // PostgreSQL answers a startup length out of range with nothing: it logs the violation and
    // closes (`backend_startup.c:566-571`, COMMERROR), so the client can't tell the servers apart.
    for length in [
        u32::try_from(wire::MAX_STARTUP_PACKET_LENGTH + 5).unwrap(),
        7,
        3,
        0,
        u32::MAX,
    ] {
        let mut session = Session::new(config(false), seed());
        let mut out = Vec::new();
        let step = session
            .step(&length.to_be_bytes(), roles, &mut out)
            .unwrap();
        assert!(out.is_empty(), "nothing is sent for length {length}");
        match step.next {
            Next::CloseLogged(error) => {
                assert_eq!(error.code, SqlState::PROTOCOL_VIOLATION);
                assert_eq!(error.message, "invalid length of startup packet");
            }
            other => panic!("length {length}: {other:?}"),
        }
    }
    refused(
        &start.1,
        &header(b'p', u32::try_from(wire::AUTH_TOKEN_LIMIT + 5).unwrap()),
    );
    refused(&sasl_final.1, &header(b'p', 3));
    refused(
        &ready.1,
        &header(b'Q', u32::try_from(wire::LARGE_MESSAGE_LIMIT + 5).unwrap()),
    );
    refused(
        &ready.1,
        &header(b'S', u32::try_from(wire::SMALL_MESSAGE_LIMIT + 5).unwrap()),
    );

    // Wrong orders: each phase refuses a message that belongs to another.
    let wrong = |prefix: &[u8], frame: &[u8], code: &str| {
        let mut session = Session::new(config(false), seed());
        let (_, next) = drive(&mut session, roles, prefix, Pace::Frames);
        assert_eq!(next, None);
        let (out, next) = drive(&mut session, roles, frame, Pace::Frames);
        assert_eq!(next, Some(Next::Close));
        assert_eq!(sqlstate(&messages(&out)[0].1), code);
    };
    // A typed message first reads as a startup length out of range: closed with nothing sent.
    let mut session = Session::new(config(false), seed());
    let (out, next) = drive(&mut session, roles, &query(b"SELECT 1"), Pace::Frames);
    assert!(out.is_empty());
    assert!(matches!(next, Some(Next::CloseLogged(_))), "{next:?}");
    wrong(&start.1, &query(b"SELECT 1"), "08P01");
    wrong(&sasl_final.1, &frontend(b'X', b""), "08P01");
    wrong(&ready.1, &frontend(b'p', b"c=biws"), "08P01");
    wrong(
        &ready.1,
        &startup_packet(wire::PROTOCOL_3_2, &[("user", "ada")]),
        "08P01",
    );
    wrong(&ready.1, &frontend(b'Q', b"SELECT 1"), "08P01");
    wrong(&ready.1, &frontend(b'Q', b"SELECT 1\0extra"), "08P01");
}

#[test]
fn t_pg_2_2_a_refused_session_is_answered_after_its_startup_packet_as_postgresql_answers() {
    let roles = roles();
    let mut session = Session::new(config(false), seed());
    session.refuse(PgError::fatal(
        SqlState::TOO_MANY_CONNECTIONS,
        "sorry, too many clients already",
    ));
    let (out, next) = drive(
        &mut session,
        roles,
        &request(wire::GSS_REQUEST_CODE, &[]),
        Pace::Frames,
    );
    assert_eq!(next, None, "the opening's requests are answered first");
    assert_eq!(out, b"N");
    let (out, next) = drive(
        &mut session,
        roles,
        &startup_packet(wire::PROTOCOL_3_2, &[("user", "ada")]),
        Pace::Bytes,
    );
    assert_eq!(next, Some(Next::Close));
    let sent = messages(&out);
    assert_eq!(
        sent.len(),
        1,
        "no authentication request before the refusal"
    );
    assert_eq!(sent[0].0, b'E');
    let fields = error_fields(&sent[0].1);
    assert!(fields.contains(&(b'S', "FATAL".to_owned())));
    assert!(fields.contains(&(b'C', "53300".to_owned())));
    assert!(fields.contains(&(b'M', "sorry, too many clients already".to_owned())));
}

/// pgbench's tables as pgbench.c's createTables makes them, with 19's names for the types.
const TPCB_SCHEMA: [&str; 7] = [
    "CREATE TABLE pgbench_history(tid integer, bid integer, aid integer, delta integer, mtime timestamp, filler character(22))",
    "CREATE TABLE pgbench_tellers(tid integer NOT NULL, bid integer, tbalance integer, filler character(84))",
    "CREATE TABLE pgbench_accounts(aid integer NOT NULL, bid integer, abalance integer, filler character(84))",
    "CREATE TABLE pgbench_branches(bid integer NOT NULL, bbalance integer, filler character(88))",
    "CREATE UNIQUE INDEX pgbench_tellers_pkey ON pgbench_tellers(tid)",
    "CREATE UNIQUE INDEX pgbench_accounts_pkey ON pgbench_accounts(aid)",
    "CREATE UNIQUE INDEX pgbench_branches_pkey ON pgbench_branches(bid)",
];

const TPCB_STATEMENTS: &str = include_str!("tpcb/statements.txt");
const TPCB_ANSWERS: &str = include_str!("tpcb/answers.txt");

/// A role's session past its start-up and SCRAM, its statements run on `statements`.
fn log_in(statements: Box<dyn Statements>) -> Session {
    let mut session =
        Session::new(config(false), seed()).with_database(Box::new(move |_| Ok(statements)));
    let roles = roles();
    let user = [("user", "ada"), ("database", "ada")];
    let packet = startup_packet(wire::PROTOCOL_3_0, &user);
    drive(&mut session, roles, &packet, Pace::Frames);
    let bare = "n=,r=clientnonce123";
    let first = format!("n,,{bare}");
    let (out, _) = drive(
        &mut session,
        roles,
        &sasl_initial(scram::MECHANISM, first.as_bytes()),
        Pace::Frames,
    );
    let answered = messages(&out);
    let server_first = String::from_utf8(answered[0].1[4..].to_vec()).unwrap();
    let (last, _) = client_final(PASSWORD, bare, &server_first, b"n,,");
    let (out, _) = drive(
        &mut session,
        roles,
        &frontend(b'p', last.as_bytes()),
        Pace::Frames,
    );
    assert_eq!(messages(&out).last().unwrap(), &(b'Z', b"I".to_vec()));
    session
}

fn tpcb_statements(conn: &Arc<amphora_engine::Connection>) -> Box<dyn Statements> {
    let zone = amphora_pg_analysis::tz::time_zone("UTC").unwrap();
    Box::new(Executor::new(
        Arc::clone(conn),
        "ada",
        zone,
        Box::new(execute::system_clock),
    ))
}

/// T-PG.9.1: pgbench's TPC-B statements, through two sessions on the wire core, answer as a
/// PostgreSQL 19.0 server answered the same statements (scripts/postgres-tpcb-answers.py): the same
/// RowDescription types, the same DataRow text, the same command tags, refusals and transaction
/// states. Only a table's OID and a CURRENT_TIMESTAMP's value differ, and only its shape is held.
#[test]
fn t_pg_9_1_tpcb_statements_answer_as_a_19_server_does() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tpcb.db");
    let io: Arc<dyn amphora_engine::IO> = Arc::new(amphora_engine::PlatformIO::new().unwrap());
    let db = amphora_engine::Database::open_file(
        io,
        path.to_str().unwrap(),
        Arc::new(amphora_engine::PostgresDialect::new()),
    )
    .unwrap();
    let connect = || db.connect(amphora_engine::ConnectOpts::new()).unwrap();
    let setup = connect();
    for ddl in TPCB_SCHEMA {
        setup.execute(ddl).unwrap();
    }
    let mut sessions = std::collections::BTreeMap::new();
    for name in ["s1", "s2"] {
        sessions.insert(name.to_owned(), log_in(tpcb_statements(&connect())));
    }
    let answers = tpcb::answers(TPCB_ANSWERS);
    let expected: Vec<&str> = TPCB_STATEMENTS.lines().collect();
    assert_eq!(answers.len(), expected.len());
    let mut wrong = Vec::new();
    let mut compared = 0;
    for ((session, sql, theirs), line) in answers.iter().zip(expected) {
        assert_eq!(line, format!("{session}|{sql}"));
        let (out, next) = drive(
            sessions.get_mut(session).unwrap(),
            roles(),
            &query(sql.as_bytes()),
            Pace::Frames,
        );
        assert_eq!(next, None);
        let (got, want) = (tpcb::told(sql, &messages(&out)), tpcb::told(sql, theirs));
        compared += want.len();
        if got != want {
            wrong.push(format!(
                "{session}|{sql}:\n  here: {got:#?}\n  19:   {want:#?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(
        compared > 100,
        "every message of every statement is compared"
    );
}

/// A message as a client of the extended protocol can tell it: as [`tpcb::told`] does, but a row's values
/// by their bytes, whatever their format, and a refusal's context with it.
fn exactly(tag: u8, body: &[u8]) -> String {
    match tag {
        b'E' | b'N' => {
            let fields: Vec<_> = error_fields(body)
                .into_iter()
                .filter(|(field, _)| b"SVCMWP".contains(field))
                .collect();
            format!("{} {fields:?}", char::from(tag))
        }
        b'D' => format!("D {body:02x?}"),
        _ => tpcb::told("", &[(tag, body.to_vec())]).concat(),
    }
}

/// T-PG.10.1: the extended protocol's messages (pgbench's `-M extended` and `-M prepared` flows, its relkind
/// query, parameters and columns of each type in text and in binary, portals stopped and gone on with, what
/// each message refuses, one transaction to each Sync, a failed block's, Flush, and messages malformed
/// inside), each step's sent at once to the wire core, are answered as a PostgreSQL 19.0 server answered
/// them (scripts/postgres-extended-answers.sh), message by message.
#[test]
fn t_pg_10_1_extended_messages_answer_as_a_19_server_does() {
    for pace in [Pace::Frames, Pace::Bytes] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("postgres.db");
        let database = execute::Database::open(
            path.to_str().unwrap(),
            amphora_engine::storage::versioned::memory::machine(),
        )
        .unwrap();
        let role = execute::SessionRole {
            name: "ada",
            superuser: true,
        };
        let mut sessions = std::collections::BTreeMap::new();
        for (name, process_id) in [("s1", 1001), ("s2", 1002)] {
            let executor = database.session(role, "UTC", process_id).unwrap();
            sessions.insert(name.to_owned(), log_in(Box::new(executor)));
        }
        let answers = tpcb::answers(include_str!("extended/answers.txt"));
        let steps: Vec<&str> = include_str!("extended/steps.txt")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .collect();
        assert_eq!(answers.len(), steps.len());
        let mut wrong = Vec::new();
        let mut compared = std::collections::BTreeMap::new();
        for ((name, step, answer), line) in answers.iter().zip(steps) {
            assert_eq!(line, format!("{name}: {step}"));
            let ((b'>', sent), theirs) = answer.split_first().unwrap() else {
                panic!("{line}: the bytes sent come first");
            };
            let session = sessions.get_mut(name).unwrap();
            let (out, next) = drive(session, roles(), sent, pace);
            assert_eq!(next, None, "{line}");
            let told = |messages: &[(u8, Vec<u8>)]| -> Vec<String> {
                messages
                    .iter()
                    .map(|(tag, body)| exactly(*tag, body))
                    .collect()
            };
            let (got, want) = (told(&messages(&out)), told(theirs));
            for (tag, _) in theirs {
                *compared.entry(char::from(*tag)).or_insert(0) += 1;
            }
            if got != want {
                wrong.push(format!("{line}:\n  here: {got:#?}\n  19:   {want:#?}"));
            }
        }
        assert!(
            wrong.is_empty(),
            "{} steps\n{}",
            wrong.len(),
            wrong.join("\n")
        );
        for tag in [
            '1', '2', '3', 't', 'T', 'n', 'D', 'C', 's', 'I', 'E', 'N', 'Z',
        ] {
            assert!(compared.contains_key(&tag), "no {tag} among {compared:?}");
        }
        assert!(compared.values().sum::<i32>() > 1000, "{compared:?}");
    }
}

/// T-PG.10.4: each message of T-PG.10.1's steps cut at each byte, a byte longer, and (but for a Parse's, whose
/// text is the parser's own hostile input) with each byte at three others, a Sync behind it, is answered to
/// its ReadyForQuery with at most one ErrorResponse, and the session goes on.
#[test]
fn t_pg_10_4_hostile_extended_messages_are_answered_to_sync() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("postgres.db");
    let database = execute::Database::open(
        path.to_str().unwrap(),
        amphora_engine::storage::versioned::memory::machine(),
    )
    .unwrap();
    let role = execute::SessionRole {
        name: "ada",
        superuser: true,
    };
    let executor = database.session(role, "UTC", 1).unwrap();
    let mut session = log_in(Box::new(executor));
    let mut given = BTreeSet::new();
    for (_, _, answer) in tpcb::answers(include_str!("extended/answers.txt")) {
        for message in messages(&answer[0].1) {
            if b"PBDEC".contains(&message.0) {
                given.insert(message);
            }
        }
    }
    let mut send = |bytes: &[u8]| {
        let (out, next) = drive(&mut session, roles(), bytes, Pace::Frames);
        assert_eq!(next, None, "{bytes:02x?}");
        messages(&out)
    };
    for (tag, body) in given
        .iter()
        .filter(|(tag, body)| *tag == b'P' && body[0] != 0)
    {
        send(&frontend(*tag, body));
        send(&frontend(b'S', b""));
    }
    let mut sent = 0;
    for (tag, body) in &given {
        let mut hostile: Vec<Vec<u8>> = (0..body.len()).map(|at| body[..at].to_vec()).collect();
        hostile.push([body.as_slice(), &[0xff]].concat());
        for at in (0..body.len()).filter(|_| *tag != b'P') {
            for byte in [0x00, 0x7f, 0xff] {
                let mut other = body.clone();
                other[at] = byte;
                hostile.push(other);
            }
        }
        for body in hostile {
            let mut bytes = frontend(*tag, &body);
            bytes.extend(frontend(b'S', b""));
            let answer = send(&bytes);
            assert_eq!(answer.last().map(|m| m.0), Some(b'Z'), "{bytes:02x?}");
            let refusals = answer.iter().filter(|m| m.0 == b'E').count();
            assert!(refusals <= 1, "{bytes:02x?}");
            sent += 1;
        }
    }
    assert!(sent > 10_000, "{sent} hostile messages");
    let answer = send(&query(b"SELECT 1"));
    assert_eq!(answer[2], (b'C', b"SELECT 1\0".to_vec()));
}
