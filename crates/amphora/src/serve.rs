//! Amphora on its own (CP-T1.13; AC-T1.13.1, AC-T1.13.5, T-T1.13.1): one member of a replicated
//! database as a long-running server, its peers and its clients over TCP, set up on each machine by
//! `scripts/cluster.sh` (docs/runbook.md).
//!
//! - `amphora serve CONFIG`: runs the member CONFIG describes, on its data. It reopens from what it
//!   kept, so a member killed and started again by its service manager rejoins on its own data; a
//!   replica that fences (its outgoing queue full, its log refused) ends the process with the reason,
//!   and the service manager starts it again from durable state.
//! - `amphora health CONFIG`: asks every member for its status; healthy when every member answers,
//!   all name the same leader, each holds the configured voters and none is fenced.
//! - `amphora ask CONFIG NODE REQUEST`: one request to one member (`status`, `exec SQL`,
//!   `count SQL`, `cut on|off` and `disk cut|hang|held|back` (`held`: wait until the hung disk
//!   holds an operation) where the member's config says
//!   `drills = on`, `member …`, `reload` (its TLS files read again, for the links made from then
//!   on: a certificate renewed), and
//!   `leader WAIT_MS`: wait until the member knows its group's leader, at most WAIT_MS, and answer
//!   `leader=ID addr=HOST:PORT term=T`; woken by the election's step, never by polling).
//! - `amphora drill load|check`: the fire drills' writers and their loss count ([`load`], [`check`]).
//!
//! The config, one per member (`key = value` lines, `#` comments; every key below but `drills`,
//! `spanning`, `voters` and the TLS files required, none defaulted, an unknown key refused):
//!
//! ```text
//! node = 1                                   # this member's ID
//! members = 1@a:7401,2@b:7401,3@c:7401       # every member's ID and address, as its peers reach it
//! data = /home/me/.local/share/amphora/1     # this member's database and log
//! class = persisted                          # or replicated (then spanning = N)
//! secret_file = /home/me/.config/amphora/secret
//! deadline_ms = 5000                         # how long one request, connect or open may take
//! drills = off                               # on: the member takes `cut` (a split at its transport)
//!                                            # and `disk` (its log's disk cut or hung)
//! cert_file = /home/me/.config/amphora/member-1.pem   # with key_file and roots_file, or none
//! key_file = /home/me/.config/amphora/member-1.key
//! roots_file = /home/me/.config/amphora/roots.pem
//! ```
//!
//! With `cert_file`, `key_file` and `roots_file` (PEM: this member's certificate chain, its own
//! first; its private key; the deployment's authority), every link runs inside mutual TLS 1.3 under
//! the owner transport's rules ([`crate::transport::network`]): each side presents its chain and
//! verifies the other's under the roots, a member's certificate naming the host its peers and
//! clients reach it by, and the protocol agreed is [`ALPN`]. A client (`amphora ask`, `health`,
//! `drill`) presents the chain of the config it reads. Without them the link is not encrypted.
//!
//! A connection's first byte says what it carries: `R` (a peer's Raft records that carry its log:
//! appends and snapshots), `H` (a peer's other Raft records: heartbeats, votes and every answer),
//! `C` (a client's requests) or `Q` (a client's SQL). The member answers with a challenge (32 random bytes) and the
//! other side proves it holds the cluster's secret with HMAC-SHA256(secret, challenge || kind)
//! (RFC 2104); the secret never crosses the link.
//! Then `R` and `H`: the sender's node ID (u64, little endian) and records, each its length (u32, little
//! endian, at most the replica's own record bound) and its bytes; `C`: requests, one line each,
//! each answered by one line, `ok …` or `err …`, a line at most the API's request bound; `Q`:
//! frames, each its length (u32, little endian) and a postcard value, a request at most the API's
//! request bound and an answer's frames each at most its result bound: statements with bound
//! parameters, answered with typed rows and their columns' names, an answer past one frame in
//! several from version 2 ([`SqlClient`]; docs/design/client-protocol.md, decision 2).
//!
//! Bounds: one connection of each records kind per peer (a new one of that kind from the same
//! peer closes the old); each
//! client connection takes a descriptor from the process's budget ([`crate::descriptors`]) and is
//! refused when there is none; each peer's queue holds at most the replica's own outgoing bound, past
//! which records are lost as a congested link loses them, and Raft sends again what it needs.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use amphora_api::replication::{Durability, Fates, MemberChange, ReplicaConfig, Route};
use amphora_api::{Refusal, Statement, Value};
use anyhow::{Context, anyhow, bail, ensure};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::database::{Database, LogDisk};
use crate::memory::{self, Use};
use crate::transport::network::{self, Credentials};

/// The records a peer's sender holds and one drain takes: the 4096 that
/// `replicated::OUTGOING_MESSAGES` was until QB.1 (lanes constants) bounded the outgoing queue by
/// its learned drain. Still picked: CP-T1.13's to derive from that bound (lane constants-20).
const OUTGOING_MESSAGES: usize = 4096;

const RECORDS: u8 = b'R';
const CLIENT: u8 = b'C';
const SQL: u8 = b'Q';
/// A peer's control records (`Route::Control`), on a connection of their own beside its
/// `RECORDS` (CP-T1.13, replication.md "Decision (lane P9)"): a heartbeat, a vote and their answers
/// are never written, read or stepped behind an append. A kind appended; framed as `RECORDS` is.
const CONTROL: u8 = b'H';
/// The newest version of a `Q` connection's frames this side speaks. A variant appended to
/// [`SqlRequest`] or [`SqlAnswer`] raises it and is sent only at a version agreed that takes it
/// (docs/design/client-protocol.md, decision 2). A format value. 3: Refusal gained
/// Serialization ([`SQL_VERSION_SERIALIZATION`]).
pub const SQL_VERSION: u32 = 3;
/// The oldest version of a `Q` connection's frames this side speaks. A format value.
const SQL_VERSION_MIN: u32 = 1;
/// The first `Q` version whose clients take [`Refusal::Serialization`]; an older one is sent the
/// `Invalid` it was (local-owner-transport.md, "Version 10").
const SQL_VERSION_SERIALIZATION: u32 = 3;
/// The challenge's length: SHA-256's output length (FIPS 180-4), HMAC-SHA256's block of proof.
const NONCE: usize = 32;
/// HMAC-SHA256's tag length (RFC 2104 with SHA-256: 32 bytes).
const TAG: usize = 32;
/// The shortest secret taken: RFC 2104 §3 asks a key of at least the hash's output length.
const SECRET_MIN: usize = 32;
/// A request or answer line's most bytes: the API's request bound.
const LINE_MAX: usize = amphora_api::REQUEST_MAX;
/// The protocol a member's links agree inside mutual TLS (RFC 7301), this one's own name: a peer
/// that speaks another, the owner transport's included, is refused at the handshake. A format value.
pub const ALPN: &[u8] = b"amphora-serve";

/// One member as the config names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub node: u64,
    pub addr: String,
}

/// One member's config ([`Config::parse`]).
#[derive(Clone)]
pub struct Config {
    pub node: u64,
    pub members: Vec<Member>,
    pub data: PathBuf,
    pub class: Durability,
    pub secret_file: PathBuf,
    pub deadline: Duration,
    pub drills: bool,
    /// The members that vote from the start; the others named in `members` are spares, known by
    /// address and started as learners, for `amphora member` to add (default: every member).
    pub voters: BTreeSet<u64>,
    /// The files mutual TLS takes on every link, or none: the links unencrypted.
    pub tls: Option<TlsFiles>,
}

/// A member's mutual TLS files, PEM each ([`Config`]'s `cert_file`, `key_file`, `roots_file`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsFiles {
    /// This member's certificate chain, its own first.
    pub cert: PathBuf,
    /// Its private key.
    pub key: PathBuf,
    /// The deployment's authority: the roots every member's chain is verified under.
    pub roots: PathBuf,
}

const KEYS: [&str; 12] = [
    "node",
    "members",
    "data",
    "class",
    "spanning",
    "secret_file",
    "deadline_ms",
    "drills",
    "voters",
    "cert_file",
    "key_file",
    "roots_file",
];

impl Config {
    /// The config's text read, every value checked: refused with the line and the reason.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let mut keys: BTreeMap<&str, &str> = BTreeMap::new();
        for (n, line) in text.lines().enumerate() {
            let at = n.saturating_add(1);
            let line = line.split_once('#').map_or(line, |(kept, _)| kept).trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| anyhow!("line {at}: not `key = value`"))?;
            let (k, v) = (k.trim(), v.trim());
            ensure!(
                KEYS.contains(&k),
                "line {at}: unknown key {k} (keys: {})",
                KEYS.join(", ")
            );
            ensure!(keys.insert(k, v).is_none(), "line {at}: {k} given twice");
        }
        let take = |k: &str| {
            keys.get(k)
                .copied()
                .ok_or_else(|| anyhow!("the config names no {k}"))
        };
        let node: u64 = take("node")?.parse().context("node")?;
        let mut members = Vec::new();
        for m in take("members")?.split(',') {
            let m = m.trim();
            let (id, addr) = m
                .split_once('@')
                .ok_or_else(|| anyhow!("member {m}: not ID@HOST:PORT"))?;
            let id: u64 = id.parse().with_context(|| format!("member {m}"))?;
            ensure!(id > 0, "member {m}: IDs start at 1");
            ensure!(
                addr.rsplit_once(':')
                    .is_some_and(|(h, p)| !h.is_empty() && p.parse::<u16>().is_ok()),
                "member {m}: not ID@HOST:PORT"
            );
            members.push(Member {
                node: id,
                addr: addr.to_owned(),
            });
        }
        let ids: BTreeSet<u64> = members.iter().map(|m| m.node).collect();
        ensure!(ids.len() == members.len(), "members: an ID given twice");
        ensure!(ids.contains(&node), "node {node} isn't among the members");
        let voters: BTreeSet<u64> = match keys.get("voters").copied() {
            None => ids.clone(),
            Some(v) => v
                .split(',')
                .map(|n| {
                    n.trim()
                        .parse::<u64>()
                        .with_context(|| format!("voters: {n}"))
                })
                .collect::<anyhow::Result<_>>()?,
        };
        ensure!(!voters.is_empty(), "voters names none");
        ensure!(
            voters.is_subset(&ids),
            "voters names a node that isn't among the members"
        );
        let class = match take("class")? {
            "persisted" => {
                ensure!(
                    !keys.contains_key("spanning"),
                    "spanning is the replicated class's"
                );
                Durability::Persisted
            }
            // Each member its own fate: each on a machine of its own (docs/runbook.md).
            "replicated" => Durability::Replicated(Fates {
                of: members.iter().map(|m| (m.node, m.node)).collect(),
                spanning: take("spanning")?.parse().context("spanning")?,
            }),
            other => bail!("class {other}: persisted or replicated"),
        };
        let deadline_ms: u64 = take("deadline_ms")?.parse().context("deadline_ms")?;
        ensure!(deadline_ms > 0, "deadline_ms takes at least 1");
        let drills = match keys.get("drills").copied() {
            None | Some("off") => false,
            Some("on") => true,
            Some(other) => bail!("drills {other}: on or off"),
        };
        let tls = match (
            keys.get("cert_file").copied(),
            keys.get("key_file").copied(),
            keys.get("roots_file").copied(),
        ) {
            (None, None, None) => None,
            (Some(cert), Some(key), Some(roots)) => Some(TlsFiles {
                cert: PathBuf::from(cert),
                key: PathBuf::from(key),
                roots: PathBuf::from(roots),
            }),
            _ => bail!("cert_file, key_file and roots_file go together: all three or none"),
        };
        Ok(Self {
            node,
            members,
            data: PathBuf::from(take("data")?),
            class,
            secret_file: PathBuf::from(take("secret_file")?),
            deadline: Duration::from_millis(deadline_ms),
            drills,
            voters,
            tls,
        })
    }

    /// The config at `path`.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("in {}", path.display()))
    }

    /// The cluster's secret, its trailing whitespace dropped.
    pub fn secret(&self) -> anyhow::Result<Vec<u8>> {
        let mut bytes = std::fs::read(&self.secret_file)
            .with_context(|| format!("reading {}", self.secret_file.display()))?;
        while bytes.last().is_some_and(u8::is_ascii_whitespace) {
            bytes.pop();
        }
        ensure!(
            bytes.len() >= SECRET_MIN,
            "{}: a secret of at least {SECRET_MIN} bytes (RFC 2104 §3)",
            self.secret_file.display()
        );
        Ok(bytes)
    }

    /// What this config's links prove: the cluster's secret, inside mutual TLS from its files when
    /// it names them, each file read and checked here.
    pub fn keys(&self) -> anyhow::Result<Keys> {
        let secret = self.secret()?;
        let Some(files) = &self.tls else {
            return Ok(Keys::plain(secret));
        };
        Ok(Keys {
            secret,
            tls: Some(Arc::new(Renewable {
                now: std::sync::RwLock::new(Arc::new(Tls::read(files)?)),
                files: files.clone(),
            })),
        })
    }

    fn member(&self, node: u64) -> anyhow::Result<&Member> {
        self.members
            .iter()
            .find(|m| m.node == node)
            .ok_or_else(|| anyhow!("no member {node}"))
    }
}

fn resolve(addr: &str) -> anyhow::Result<SocketAddr> {
    addr.to_socket_addrs()
        .with_context(|| format!("resolving {addr}"))?
        .next()
        .ok_or_else(|| anyhow!("{addr} names no address"))
}

/// What a link to a member proves ([`Config::keys`]): the cluster's secret, and mutual TLS when
/// the config names its files.
#[derive(Clone)]
pub struct Keys {
    secret: Vec<u8>,
    tls: Option<Arc<Renewable>>,
}

/// A member's mutual TLS as its files hold it now, read again by [`Keys::reload`]: links made
/// after a reload use what it read; links already made keep the session they agreed.
struct Renewable {
    files: TlsFiles,
    now: std::sync::RwLock<Arc<Tls>>,
}

impl Renewable {
    fn now(&self) -> anyhow::Result<Arc<Tls>> {
        Ok(Arc::clone(
            &*self.now.read().map_err(|_| anyhow!("TLS keys poisoned"))?,
        ))
    }
}

/// Both sides of a member's mutual TLS.
struct Tls {
    listener: network::Listener,
    dialer: network::Dialer,
}

impl Tls {
    /// `files` read and checked: the chain, its key and the roots, each PEM.
    fn read(files: &TlsFiles) -> anyhow::Result<Self> {
        let refused = |e: amphora_api::Refusal| anyhow!("mutual TLS: {e:?}");
        let credentials = Credentials::from_pem_files(&files.cert, &files.key, &files.roots)
            .map_err(refused)
            .with_context(|| {
                format!(
                    "reading {}, {} and {}",
                    files.cert.display(),
                    files.key.display(),
                    files.roots.display()
                )
            })?;
        Ok(Self {
            listener: credentials.listener(ALPN).map_err(refused)?,
            dialer: credentials.dialer(ALPN).map_err(refused)?,
        })
    }
}

impl Keys {
    /// The secret alone: the links of a cluster whose configs name no TLS files.
    pub fn plain(secret: Vec<u8>) -> Self {
        Self { secret, tls: None }
    }

    /// Reads the TLS files again (CP-T1.13: a certificate renewed, `scripts/cluster.sh renew`):
    /// links made from now on present the new chain and verify under the new roots; links already
    /// made keep theirs. Files that don't read or don't check are refused, and the keys held stay.
    /// Woken by the request that asks for it, sent once the new files are in place: never a
    /// poll of the files.
    pub fn reload(&self) -> anyhow::Result<()> {
        let Some(tls) = &self.tls else {
            bail!("this config names no TLS files: nothing to reload");
        };
        let fresh = Arc::new(Tls::read(&tls.files)?);
        *tls.now.write().map_err(|_| anyhow!("TLS keys poisoned"))? = fresh;
        Ok(())
    }

    /// `s`, accepted, inside mutual TLS by `until` when these keys hold it.
    fn accepted(&self, s: TcpStream, until: Instant) -> anyhow::Result<Link> {
        match &self.tls {
            None => Ok(Link::Plain(s)),
            Some(tls) => Ok(Link::Accepted(Box::new(
                tls.now()?
                    .listener
                    .accept(s, until)
                    .map_err(|e| anyhow!("mutual TLS: {e:?}"))?,
            ))),
        }
    }

    /// `s`, connected to `addr`, inside mutual TLS by `until` when these keys hold it: the
    /// member's certificate must name `addr`'s host.
    fn dialed(&self, s: TcpStream, addr: &str, until: Instant) -> anyhow::Result<Link> {
        match &self.tls {
            None => Ok(Link::Plain(s)),
            Some(tls) => {
                let host = addr.rsplit_once(':').map_or(addr, |(host, _)| host);
                // An IPv6 address in a member's address is bracketed ([::1]:7401).
                let host = host.trim_start_matches('[').trim_end_matches(']');
                Ok(Link::Dialed(Box::new(
                    tls.now()?
                        .dialer
                        .connect(s, host, until)
                        .map_err(|e| anyhow!("mutual TLS with {addr}: {e:?}"))?,
                )))
            }
        }
    }
}

/// One link's stream: the socket, or the socket inside mutual TLS.
enum Link {
    Plain(TcpStream),
    Accepted(Box<network::Accepted>),
    Dialed(Box<network::Dialed>),
}

impl Read for Link {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            Self::Accepted(s) => s.read(buf),
            Self::Dialed(s) => s.read(buf),
        }
    }
}

impl Write for Link {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(buf),
            Self::Accepted(s) => s.write(buf),
            Self::Dialed(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Accepted(s) => s.flush(),
            Self::Dialed(s) => s.flush(),
        }
    }
}

fn keyed(secret: &[u8]) -> anyhow::Result<Hmac<Sha256>> {
    <Hmac<Sha256> as Mac>::new_from_slice(secret).map_err(|e| anyhow!("the secret as a key: {e}"))
}

fn after(deadline: Duration) -> anyhow::Result<Instant> {
    Instant::now()
        .checked_add(deadline)
        .ok_or_else(|| anyhow!("a deadline past the clock's range"))
}

/// What one member's connections share.
struct Shared {
    node: u64,
    ids: BTreeSet<u64>,
    db: Database,
    cut: AtomicBool,
    deadline: Duration,
    keys: Keys,
    drills: bool,
    /// This member's raft log, whose disk `disk` cuts or hangs.
    log: PathBuf,
    /// Each member's address as the config names it, for the `leader` request's answer.
    addrs: BTreeMap<u64, String>,
    /// Each peer's connection of each kind (`RECORDS`, `CONTROL`), to close it when the peer
    /// opens another of that kind.
    links: Mutex<BTreeMap<(u64, u8), TcpStream>>,
}

/// Runs the member `config` describes until its replica stops; `ready` is told its address once
/// it listens.
pub fn serve(config: &Config, ready: impl FnOnce(&str)) -> anyhow::Result<()> {
    let keys = config.keys()?;
    let me = config.member(config.node)?.clone();
    std::fs::create_dir_all(&config.data)
        .with_context(|| format!("creating {}", config.data.display()))?;
    let ids: BTreeSet<u64> = config.members.iter().map(|m| m.node).collect();
    let replica = ReplicaConfig {
        node_id: config.node,
        voters: config.voters.iter().copied().collect(),
        // A spare starts as a learner: the group's configuration in its log decides from there.
        learners: if config.voters.contains(&config.node) {
            Vec::new()
        } else {
            vec![config.node]
        },
        bootstrap: None,
    };
    let db = Database::open_replicated_with(
        &config.data.join("state.db"),
        &config.data.join("raft.log"),
        replica,
        config.class.clone(),
        after(config.deadline)?,
    )
    .map_err(|e| anyhow!("member {}: {e:?}", config.node))?;
    let handle = db
        .replica()
        .ok_or_else(|| anyhow!("member {} has no replica", config.node))?;
    let listener = TcpListener::bind(resolve(&me.addr)?)
        .with_context(|| format!("listening on {}", me.addr))?;
    let shared = Arc::new(Shared {
        node: config.node,
        ids,
        db,
        cut: AtomicBool::new(false),
        deadline: config.deadline,
        keys,
        drills: config.drills,
        log: config.data.join("raft.log"),
        addrs: config
            .members
            .iter()
            .map(|m| (m.node, m.addr.clone()))
            .collect(),
        links: Mutex::new(BTreeMap::new()),
    });
    // Each peer's two paths, one sender each: its log's records and its control records.
    let mut peers: BTreeMap<(u64, Route), mpsc::SyncSender<Vec<u8>>> = BTreeMap::new();
    for m in config.members.iter().filter(|m| m.node != config.node) {
        for (route, kind, name) in [
            (Route::Log, RECORDS, "peer"),
            (Route::Control, CONTROL, "peer-control"),
        ] {
            let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(OUTGOING_MESSAGES);
            let (addr, node, keys, deadline) = (
                m.addr.clone(),
                config.node,
                shared.keys.clone(),
                config.deadline,
            );
            std::thread::Builder::new()
                .name(format!("amphora-{name}-{}", m.node))
                .spawn(move || to_peer(node, &addr, &keys, deadline, kind, &rx))?;
            peers.insert((m.node, route), tx);
        }
    }
    {
        let shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("amphora-accept".into())
            .spawn(move || accept(&shared, &listener))?;
    }
    ready(&me.addr);
    // The replica's records to its peers: drain_wait waits for the next, no spinning, no picked sleep.
    loop {
        let out = handle
            .drain_wait(OUTGOING_MESSAGES, after(config.deadline)?)
            .map_err(|e| anyhow!("member {}'s replica stopped: {e:?}", config.node))?;
        for record in out {
            if shared.cut.load(Ordering::Relaxed) {
                continue;
            }
            if let Some(tx) = peers.get(&(record.to, record.route)) {
                let _ = tx.try_send(record.bytes);
            }
        }
    }
}

fn accept(shared: &Arc<Shared>, listener: &TcpListener) {
    for s in listener.incoming() {
        let Ok(s) = s else { continue };
        let held = match crate::descriptors::process().and_then(|d| d.take("server connections", 1))
        {
            Ok(held) => held,
            Err(_) => continue,
        };
        let shared = Arc::clone(shared);
        let _ = std::thread::Builder::new()
            .name("amphora-conn".into())
            .spawn(move || {
                let _held = held;
                let _ = connection(&shared, s);
            });
    }
}

/// One connection, as its first byte says, once it has proved the secret (inside mutual TLS when
/// the member's keys hold it).
fn connection(shared: &Shared, s: TcpStream) -> anyhow::Result<()> {
    s.set_nodelay(true)?;
    // The socket under the link, for its timeouts and to close it.
    let socket = s.try_clone()?;
    let link = shared.keys.accepted(s, after(shared.deadline)?)?;
    socket.set_read_timeout(Some(shared.deadline))?;
    socket.set_write_timeout(Some(shared.deadline))?;
    let mut r = BufReader::new(link);
    let mut kind = [0u8; 1];
    r.read_exact(&mut kind)?;
    let mut nonce = [0u8; NONCE];
    getrandom::fill(&mut nonce).map_err(|e| anyhow!("no randomness for a challenge: {e}"))?;
    r.get_mut().write_all(&nonce)?;
    r.get_mut().flush()?;
    let mut tag = [0u8; TAG];
    r.read_exact(&mut tag)?;
    let mut check = keyed(&shared.keys.secret)?;
    check.update(&nonce);
    check.update(&kind);
    check
        .verify_slice(&tag)
        .map_err(|_| anyhow!("a connection without the cluster's secret"))?;
    // Past the proof, a link waits as long as its other side is quiet: a peer between heartbeats, a
    // pooled client between requests. A dead peer's link is closed when it opens its next.
    socket.set_read_timeout(None)?;
    match kind {
        [RECORDS] | [CONTROL] => {
            let mut from = [0u8; 8];
            r.read_exact(&mut from)?;
            let from = u64::from_le_bytes(from);
            ensure!(
                from != shared.node && shared.ids.contains(&from),
                "records from {from}, not a peer"
            );
            let link = socket.try_clone()?;
            if let Some(old) = shared
                .links
                .lock()
                .map_err(|_| anyhow!("links poisoned"))?
                .insert((from, kind[0]), link)
            {
                let _ = old.shutdown(std::net::Shutdown::Both);
            }
            let handle = shared.db.replica().ok_or_else(|| anyhow!("no replica"))?;
            loop {
                // The replica's wire record bound in force (QB.1), read at each record.
                let wire = handle.wire_bytes();
                let mut len = [0u8; 4];
                r.read_exact(&mut len)?;
                let len = usize::try_from(u32::from_le_bytes(len))?;
                ensure!(
                    len <= wire,
                    "a record of {len} bytes, past the replica's {wire}"
                );
                let mut bytes = vec![0u8; len];
                r.read_exact(&mut bytes)?;
                if !shared.cut.load(Ordering::Relaxed) {
                    let _ = handle.receive(from, bytes, after(shared.deadline)?);
                }
            }
        }
        [CLIENT] => loop {
            let Some(line) = read_line(&mut r)? else {
                return Ok(());
            };
            let mut answer = shared.answer(&line);
            answer.push('\n');
            r.get_mut().write_all(answer.as_bytes())?;
            r.get_mut().flush()?;
        },
        [SQL] => {
            let answering = Arc::new(Answering {
                link: Mutex::new(r),
                socket,
                timed: AtomicBool::new(false),
            });
            let mut agreed = None;
            let replica = shared.db.replica().ok_or_else(|| anyhow!("no replica"))?;
            loop {
                let frame = read_frame(
                    &mut *answering.lock()?,
                    amphora_api::REQUEST_MAX,
                    shared.deadline,
                    replica.ending(),
                )?;
                let answer = match frame {
                    None => return Ok(()),
                    Some(Frame::Refused(refusal)) => Frames::one(refused_frame(refusal)?),
                    Some(Frame::Held(bytes, _charge, until)) => {
                        match postcard::from_bytes::<SqlRequest>(&bytes) {
                            Ok(request) => {
                                match request_charge(&request, until, replica.ending()) {
                                    Ok(_decoded) => {
                                        shared.sql(request, until, &mut agreed, &answering)?
                                    }
                                    Err(refusal) => Frames::one(refused_frame(refusal)?),
                                }
                            }
                            Err(e) => Frames::one(refused_frame(amphora_api::invalid(
                                "SQL request",
                                &format!(
                                    "{e}: this member speaks versions {SQL_VERSION_MIN} to {SQL_VERSION}"
                                ),
                            ))?),
                        }
                    }
                };
                let mut link = answering.lock()?;
                if answering.timed.swap(false, Ordering::Relaxed) {
                    answering.socket.set_write_timeout(Some(shared.deadline))?;
                }
                for frame in &answer {
                    write_frame(link.get_mut(), &frame.bytes)?;
                }
            }
        }
        _ => bail!("a connection opened with {kind:?}"),
    }
}

/// What a `Q` connection asks (docs/design/client-protocol.md, decision 2). Postcard encodes a
/// variant by its index: variants are only appended, each with the version that brought it.
#[derive(Debug, Serialize, Deserialize)]
enum SqlRequest {
    /// The versions the client speaks, oldest and newest: answered [`SqlAnswer::Agreed`], the
    /// newest both speak, before any other request.
    Hello { oldest: u32, newest: u32 },
    /// Statements run in order as one transaction on the group's writer: all of them, or none.
    Write(#[serde(deserialize_with = "crate::transport::bounded::statements")] Vec<Statement>),
    /// One statement run on this member's own state, asking no leader, as `count` reads.
    Read(#[serde(deserialize_with = "crate::transport::bounded::statement")] Statement),
}

/// The tables of one answer at most: a request's statements, `TX_MAX` at most.
fn bounded_tables<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Table>, D::Error> {
    crate::transport::bounded::vector::<D, Table, { amphora_api::TX_MAX }>(d)
}

/// A table's column names: a row's values at most, as `transport::bounded` counts them.
fn bounded_columns<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    crate::transport::bounded::vector::<D, String, { crate::transport::bounded::VALUE_COUNT }>(d)
}

/// A `Q` connection's answer to one request ([`SqlRequest`]); variants only appended.
#[derive(Debug, Serialize, Deserialize)]
enum SqlAnswer {
    Agreed(u32),
    /// Each statement's table, in the order asked: the answer's last frame.
    Tables(#[serde(deserialize_with = "bounded_tables")] Vec<Table>),
    /// The request refused, and why: a write's statements all rolled back, unless the refusal is
    /// `Timeout` or `UnknownOutcome`, whose write may have committed. The answer's last frame, its
    /// rows sent before it dropped. The connection goes on.
    Refused(Refusal),
    /// Version 2: rows of the request's statement `statement` (counted from 0), ahead of those its
    /// table holds in the last frame. An answer past one frame comes in frames of at most the
    /// API's result bound, so none is refused for its size alone.
    Rows {
        statement: u32,
        #[serde(deserialize_with = "crate::transport::bounded::rows")]
        rows: Vec<Vec<Value>>,
    },
}

/// One statement's answer: its columns' names, its rows of SQLite's typed values, and what SQLite
/// counts after it (the rows the last insert, update or delete changed, and the last rowid).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    #[serde(deserialize_with = "bounded_columns")]
    pub columns: Vec<String>,
    #[serde(deserialize_with = "crate::transport::bounded::rows")]
    pub rows: Vec<Vec<Value>>,
    pub affected: u64,
    pub last_insert_rowid: i64,
}

/// A frame read: its bytes, charged to the process's memory budget while they are held, and its
/// request's deadline; or the budget's refusal, the frame's bytes read past without being held.
enum Frame {
    Held(Vec<u8>, memory::Charge, Instant),
    Refused(Refusal),
}

/// One frame of at most `max` bytes ([`SqlRequest`], [`SqlAnswer`]); `None` at the end of the
/// stream. Its bytes are charged to the process's memory budget before they are read, waiting
/// for room until its request's deadline, `deadline` from its first byte, woken only by charges
/// given back and by the end of `owner`, the replica the request is for
/// ([`memory::Budget::take_until`]; lanes perf-A19 and perf-A28).
fn read_frame(
    r: &mut impl Read,
    max: usize,
    deadline: Duration,
    owner: &memory::Ending,
) -> anyhow::Result<Option<Frame>> {
    let mut first = [0u8; 1];
    let n = loop {
        match r.read(&mut first) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            read => break read?,
        }
    };
    if n == 0 {
        return Ok(None);
    }
    let mut rest = [0u8; 3];
    r.read_exact(&mut rest)?;
    let ([b0], [b1, b2, b3]) = (first, rest);
    let len = u32::from_le_bytes([b0, b1, b2, b3]);
    let size = usize::try_from(len)?;
    let refusal = match amphora_api::bound("a SQL frame", size, max) {
        Err(refusal) => refusal,
        Ok(()) => match after(deadline) {
            Err(e) => amphora_api::invalid("deadline", &e.to_string()),
            Ok(until) => {
                match memory::process().take_until(Use::Queues, u64::from(len), until, owner, || {})
                {
                    Ok(charge) => {
                        let mut bytes = vec![0u8; size];
                        r.read_exact(&mut bytes)?;
                        return Ok(Some(Frame::Held(bytes, charge, until)));
                    }
                    Err(refusal) => refusal,
                }
            }
        },
    };
    // Read past through a fixed buffer, never held: the refusal is answered, the connection goes on.
    let skipped = std::io::copy(&mut r.by_ref().take(u64::from(len)), &mut std::io::sink())?;
    ensure!(
        skipped == u64::from(len),
        "the connection ended inside a frame"
    );
    Ok(Some(Frame::Refused(refusal)))
}

/// A decoded request's memory, charged while it runs, waiting for room until its deadline `until`
/// or its `owner`'s end as its frame did: its frame was charged by its length, but a value decodes to
/// [`Value`]'s size from as little as one byte (bounded in count by `transport::bounded`'s decoders,
/// as `C` connections' requests are).
fn request_charge(
    request: &SqlRequest,
    until: Instant,
    owner: &memory::Ending,
) -> Result<memory::Charge, Refusal> {
    let statement = |s: &Statement| {
        s.params.iter().map(value_bytes).fold(
            std::mem::size_of::<Statement>().saturating_add(s.sql.len()),
            usize::saturating_add,
        )
    };
    let bytes = match request {
        SqlRequest::Hello { .. } => 0,
        SqlRequest::Write(statements) => statements
            .iter()
            .map(statement)
            .fold(0, usize::saturating_add),
        SqlRequest::Read(s) => statement(s),
    };
    let bytes = u64::try_from(bytes).map_err(|_| amphora_api::invalid("SQL bytes", "past u64"))?;
    memory::process().take_until(Use::Queues, bytes, until, owner, || {})
}

fn write_frame(w: &mut impl Write, bytes: &[u8]) -> anyhow::Result<()> {
    w.write_all(&u32::try_from(bytes.len())?.to_le_bytes())?;
    w.write_all(bytes)?;
    w.flush()?;
    Ok(())
}

/// An answer's frame, encoded, and what it holds of the process's memory budget until written.
struct Encoded {
    bytes: Vec<u8>,
    _held: Option<memory::Charge>,
}

impl Encoded {
    /// Its bytes no charge of its own holds: none for a frame built by [`frame`].
    fn unheld(&self) -> usize {
        if self._held.is_some() {
            0
        } else {
            self.bytes.capacity()
        }
    }
}

/// An answer's frames, in order, the list's own slots charged to the process's memory budget as
/// it grows and each frame holding its own bytes (lane perf-A11, AUDIT §1), so a write's call
/// declares only the list's fixed size ([`Frames::DECLARED`]) however many frames it fills.
struct Frames {
    list: Vec<Encoded>,
    spine: Charged,
}

impl Frames {
    /// What a call answering `Frames` declares ([`crate::database::AnswerMax`]): the list's own
    /// size, everything past it held by a charge.
    const DECLARED: usize = std::mem::size_of::<Self>();

    fn new() -> Self {
        Self {
            list: Vec::new(),
            spine: Charged::new(),
        }
    }

    /// One frame, uncharged: a refusal's or an agreement's, a few bytes written at once.
    fn one(frame: Encoded) -> Self {
        Self {
            list: vec![frame],
            spine: Charged::new(),
        }
    }

    /// `frame` appended, the slots the list grows by charged before they're taken: past the
    /// budget's room, its refusal, typed.
    fn push(&mut self, frame: Encoded) -> anyhow::Result<()> {
        if self.list.len() == self.list.capacity() {
            let grow = self.list.capacity().max(1);
            self.spine
                .hold(grow.saturating_mul(std::mem::size_of::<Encoded>()))?;
            self.list.try_reserve_exact(grow)?;
        }
        self.list.push(frame);
        Ok(())
    }
}

impl std::ops::Deref for Frames {
    type Target = Vec<Encoded>;

    fn deref(&self) -> &Vec<Encoded> {
        &self.list
    }
}

impl<'a> IntoIterator for &'a Frames {
    type Item = &'a Encoded;
    type IntoIter = std::slice::Iter<'a, Encoded>;

    fn into_iter(self) -> Self::IntoIter {
        self.list.iter()
    }
}

/// A call's answer of frames: its own size and any frame's bytes no charge holds.
impl crate::database::AnswerBytes for Frames {
    fn answer_bytes(&self) -> usize {
        self.list
            .iter()
            .map(Encoded::unheld)
            .fold(Self::DECLARED, usize::saturating_add)
    }
}

/// `refusal`'s frame: a few bytes, written at once, uncharged.
fn refused_frame(refusal: Refusal) -> anyhow::Result<Encoded> {
    Ok(Encoded {
        bytes: postcard::to_allocvec(&SqlAnswer::Refused(refusal))?,
        _held: None,
    })
}

/// `answer`'s frame, past the API's result bound TooLarge, its bytes charged to the process's
/// memory budget until written.
fn frame(answer: &SqlAnswer) -> anyhow::Result<Encoded> {
    let bytes = postcard::to_allocvec(answer)?;
    amphora_api::bound("a SQL answer", bytes.len(), amphora_api::RESULT_MAX)?;
    let held = memory::process().take(Use::Queues, u64::try_from(bytes.len())?)?;
    Ok(Encoded {
        bytes,
        _held: Some(held),
    })
}

/// A `Q` connection's link, shared with the read whose answer streams out through it while the
/// connection waits on that read's call.
struct Answering {
    link: Mutex<BufReader<Link>>,
    socket: TcpStream,
    /// A streamed frame set the socket's write timeout to its request's time left: the
    /// connection sets its own back before it writes again.
    timed: AtomicBool,
}

impl Answering {
    fn lock(&self) -> anyhow::Result<std::sync::MutexGuard<'_, BufReader<Link>>> {
        self.link.lock().map_err(|_| anyhow!("a SQL link poisoned"))
    }

    /// `frame` written out ahead of its answer's last, by `until`. The connection holds its link
    /// from the moment the request's call returns, so a session still stepping past then has been
    /// answered already and sends nothing.
    fn send(&self, until: Instant, frame: &Encoded) -> anyhow::Result<()> {
        let left = until
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or(Refusal::Timeout)?;
        let mut link = self.link.try_lock().map_err(|_| Refusal::Timeout)?;
        self.timed.store(true, Ordering::Relaxed);
        self.socket.set_write_timeout(Some(left))?;
        if let Err(e) = write_frame(link.get_mut(), &frame.bytes) {
            // A frame cut partway can't be followed by another: the connection ends.
            let _ = self.socket.shutdown(std::net::Shutdown::Both);
            let timed_out = e.downcast_ref::<std::io::Error>().is_some_and(|e| {
                matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                )
            });
            return Err(if timed_out {
                Refusal::Timeout.into()
            } else {
                e
            });
        }
        Ok(())
    }
}

/// Where an answer's frames go before its last. At version 1 there are none, and an answer past
/// one frame is refused TooLarge. From version 2 a read's go out on its link as each fills, and a
/// write's are held until its transaction commits, so no write waits on a client's network.
enum Spill {
    None,
    Held(Frames),
    Sent(Arc<Answering>, Instant),
}

impl Spill {
    /// The rows `tables` hold, out as one frame for each statement that has some, their memory
    /// given back once each frame holds them.
    fn rows(&mut self, tables: &mut [Table], charged: &mut Charged) -> anyhow::Result<()> {
        for (statement, table) in tables.iter_mut().enumerate() {
            if table.rows.is_empty() {
                continue;
            }
            let encoded = frame(&SqlAnswer::Rows {
                statement: u32::try_from(statement)?,
                rows: std::mem::take(&mut table.rows),
            })?;
            match self {
                Self::None => bail!("a version 1 answer spilled rows"),
                Self::Held(frames) => frames.push(encoded)?,
                Self::Sent(link, until) => link.send(*until, &encoded)?,
            }
        }
        charged.spilled();
        Ok(())
    }
}

/// `statements`' tables run on `c`, encoded and charged inside `c`'s session: a write commits only
/// once its answer is encoded and held, so no committed write is answered refused for want of room
/// to answer it. Past the API's result bound, TooLarge, unless `spill` takes the frames before the
/// last; each frame at most that bound.
fn answered(
    c: &crate::sql::Connection<'_>,
    statements: Vec<Statement>,
    mut spill: Spill,
) -> anyhow::Result<Frames> {
    let mut charged = Charged::new();
    let tables = tables(c, statements, &mut charged, &mut spill)?;
    let last = frame(&SqlAnswer::Tables(tables))?;
    let mut frames = match spill {
        Spill::Held(frames) => frames,
        Spill::None | Spill::Sent(..) => Frames::new(),
    };
    frames.push(last)?;
    Ok(frames)
}

/// What a request or an answer's rows hold as they're built: their memory, charged to the
/// process's budget (past its room, its refusal), and the memory the rows not yet in a frame take
/// once a client decodes them, counted against the API's result bound as the transport's decoders
/// count it (past it, a frame's end); the memory given back when it drops.
struct Charged {
    budget: Arc<memory::Budget>,
    held: u64,
    /// What of `held` is rows not yet in a frame.
    rows: u64,
}

impl Charged {
    fn new() -> Self {
        Self {
            budget: memory::process(),
            held: 0,
            rows: 0,
        }
    }

    fn hold(&mut self, bytes: usize) -> Result<u64, Refusal> {
        let bytes =
            u64::try_from(bytes).map_err(|_| amphora_api::invalid("SQL bytes", "past u64"))?;
        let after = self
            .held
            .checked_add(bytes)
            .ok_or_else(|| amphora_api::invalid("SQL bytes", "past u64"))?;
        self.budget.hold(Use::Queues, bytes)?;
        self.held = after;
        Ok(bytes)
    }

    /// Whether a row of `bytes` (as [`crate::sql::row_bytes`] counts it) fits the frame beside the
    /// rows not yet in one.
    fn fits(&self, bytes: usize) -> bool {
        u64::try_from(bytes)
            .ok()
            .and_then(|bytes| self.rows.checked_add(bytes))
            .is_some_and(|after| {
                u64::try_from(amphora_api::RESULT_MAX).is_ok_and(|bound| after <= bound)
            })
    }

    /// A row of `bytes` read into an answer: refused before it's kept when it passes the result
    /// bound beside the rows not yet in a frame, or the budget's room.
    fn row(&mut self, bytes: usize) -> Result<(), Refusal> {
        let after = usize::try_from(self.rows)
            .ok()
            .and_then(|rows| rows.checked_add(bytes))
            .ok_or_else(|| amphora_api::invalid("SQL bytes", "past usize"))?;
        amphora_api::bound("a SQL answer", after, amphora_api::RESULT_MAX)?;
        let held = self.hold(bytes)?;
        self.rows = self.rows.saturating_add(held);
        Ok(())
    }

    /// The rows not yet in a frame are in one: their memory given back, the frame holding it.
    fn spilled(&mut self) {
        self.budget.give(Use::Queues, self.rows);
        self.held = self.held.saturating_sub(self.rows);
        self.rows = 0;
    }
}

impl Drop for Charged {
    fn drop(&mut self) {
        self.budget.give(Use::Queues, self.held);
    }
}

/// The bytes `value` takes in memory: the value itself and what it points to.
fn value_bytes(value: &Value) -> usize {
    let held = match value {
        Value::Text(text) => text.len(),
        Value::Blob(bytes) => bytes.len(),
        Value::Null | Value::Integer(_) | Value::Real(_) => 0,
    };
    std::mem::size_of::<Value>().saturating_add(held)
}

/// `statements` run in order on `c`, each one's table, each row charged to `charged` as the engine
/// steps it, and the rows past a frame out through `spill`. A statement the engine refuses is
/// refused as `statement N` (counted from 0), typed.
fn tables(
    c: &crate::sql::Connection<'_>,
    statements: Vec<Statement>,
    charged: &mut Charged,
    spill: &mut Spill,
) -> anyhow::Result<Vec<Table>> {
    let mut out = Vec::new();
    for (i, s) in statements.into_iter().enumerate() {
        let refused = |e: crate::sql::Error| -> anyhow::Error {
            match e {
                crate::sql::Error::Refusal(refusal) => refusal.into(),
                other => amphora_api::invalid(&format!("statement {i}"), &other.to_string()).into(),
            }
        };
        let columns = c.column_names(&s.sql).map_err(refused)?;
        charged.hold(
            columns
                .iter()
                .map(|name| name.len().saturating_add(std::mem::size_of::<String>()))
                .fold(std::mem::size_of::<Table>(), usize::saturating_add),
        )?;
        out.push(Table {
            columns,
            rows: Vec::new(),
            affected: 0,
            last_insert_rowid: 0,
        });
        // Why the stream stopped, when not the engine: a frame that couldn't go out.
        let mut stopped = None;
        let mut statement = c.prepare(&s.sql).map_err(refused)?;
        let stepped = statement.each(s.params, |values| {
            let bytes = crate::sql::row_bytes(&values)
                .ok_or_else(|| amphora_api::invalid("SQL row", "size past usize"))?;
            if !charged.fits(bytes)
                && !matches!(spill, Spill::None)
                && let Err(e) = spill.rows(&mut out, charged)
            {
                let refusal = e
                    .downcast_ref::<Refusal>()
                    .cloned()
                    .unwrap_or_else(|| amphora_api::invalid("SQL answer", "its link failed"));
                stopped = Some(e);
                return Err(refusal.into());
            }
            charged.row(bytes)?;
            if let Some(table) = out.last_mut() {
                table.rows.push(values);
            }
            Ok(true)
        });
        drop(statement);
        if let Some(e) = stopped {
            return Err(e);
        }
        stepped.map_err(refused)?;
        if let Some(table) = out.last_mut() {
            table.affected = c.changes();
            table.last_insert_rowid = c.last_insert_rowid();
        }
    }
    Ok(out)
}

/// One line of at most [`LINE_MAX`] bytes, its end dropped; `None` at the end of the stream.
fn read_line(r: &mut BufReader<Link>) -> anyhow::Result<Option<String>> {
    let mut line = String::new();
    let n = r
        .by_ref()
        .take(u64::try_from(LINE_MAX)?.saturating_add(1))
        .read_line(&mut line)?;
    if n == 0 {
        return Ok(None);
    }
    ensure!(n <= LINE_MAX, "a line past {LINE_MAX} bytes");
    Ok(Some(line.trim_end_matches(['\r', '\n']).to_owned()))
}

impl Shared {
    /// One `Q` request's answer ([`SqlRequest`]): its frames to write, in order, each holding its
    /// share of the memory budget; a read's frames before its last already out on `answering`.
    fn sql(
        &self,
        request: SqlRequest,
        until: Instant,
        agreed: &mut Option<u32>,
        answering: &Arc<Answering>,
    ) -> anyhow::Result<Frames> {
        let streams = agreed.is_some_and(|version| version >= 2);
        let result = match request {
            SqlRequest::Hello { oldest, newest } => {
                let both = newest.min(SQL_VERSION);
                if both < oldest.max(SQL_VERSION_MIN) {
                    return Ok(Frames::one(refused_frame(amphora_api::invalid(
                        "SQL versions",
                        &format!(
                            "the client speaks {oldest} to {newest}, this member {SQL_VERSION_MIN} to {SQL_VERSION}"
                        ),
                    ))?));
                }
                *agreed = Some(both);
                return Ok(Frames::one(Encoded {
                    bytes: postcard::to_allocvec(&SqlAnswer::Agreed(both))?,
                    _held: None,
                }));
            }
            _ if agreed.is_none() => Err(amphora_api::invalid("SQL request", "asked before Hello")),
            SqlRequest::Write(_) if self.cut.load(Ordering::Relaxed) => Err(Refusal::Unavailable(
                "this member is cut off (a drill)".into(),
            )),
            SqlRequest::Write(statements) => {
                let spill = if streams {
                    Spill::Held(Frames::new())
                } else {
                    Spill::None
                };
                // Each frame's bytes are charged as it's built (`frame`), and the list's slots as
                // it grows (`Frames::push`): the call declares the list's fixed size alone.
                self.db.call(
                    until,
                    crate::database::AnswerMax::bytes(Frames::DECLARED),
                    move |c| answered(c, statements, spill),
                )
            }
            SqlRequest::Read(statement) => {
                let spill = if streams {
                    Spill::Sent(Arc::clone(answering), until)
                } else {
                    Spill::None
                };
                self.db
                    .query_local(until, move |c| answered(c, vec![statement], spill))
            }
        };
        match result {
            Ok(frames) => Ok(frames),
            Err(refusal) if agreed.is_some_and(|v| v >= SQL_VERSION_SERIALIZATION) => {
                Ok(Frames::one(refused_frame(refusal)?))
            }
            Err(refusal) => Ok(Frames::one(refused_frame(refusal.without_serialization())?)),
        }
    }

    fn answer(&self, line: &str) -> String {
        match self.request(line) {
            Ok(v) if v.is_empty() => "ok".to_owned(),
            Ok(v) => format!("ok {v}"),
            Err(e) => format!("err {}", format!("{e:#}").replace(['\n', '\r'], " ")),
        }
    }

    fn request(&self, line: &str) -> anyhow::Result<String> {
        let (op, rest) = line.split_once(' ').unwrap_or((line, ""));
        let until = after(self.deadline)?;
        match op {
            "status" => {
                let s = self
                    .db
                    .replica()
                    .ok_or_else(|| anyhow!("no replica"))?
                    .status()
                    .map_err(|e| anyhow!("{e:?}"))?;
                let voters = s
                    .voters
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                let learners = if s.learners.is_empty() {
                    "-".to_owned()
                } else {
                    s.learners
                        .iter()
                        .map(u64::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                };
                let fenced = s
                    .fenced
                    .as_deref()
                    .map_or_else(|| "-".to_owned(), |f| f.replace(' ', "_"));
                Ok(format!(
                    "node={} leader={} term={} applied={} durable={} voters={voters} fenced={fenced} cut={} learners={learners}",
                    s.node_id,
                    s.leader,
                    s.term,
                    s.applied,
                    s.durable_commit,
                    self.cut.load(Ordering::Relaxed)
                ))
            }
            "exec" => {
                ensure!(
                    !self.cut.load(Ordering::Relaxed),
                    "this member is cut off (a drill)"
                );
                let sql = rest.to_owned();
                self.db
                    .call(until, crate::database::AnswerMax::fixed(), move |c| {
                        c.execute_batch(&sql)?;
                        Ok(())
                    })
                    .map_err(|e| anyhow!("refused a write: {e:?}"))?;
                Ok(String::new())
            }
            "count" => {
                let sql = rest.to_owned();
                let n = self
                    .db
                    .query_local(until, move |c| {
                        Ok(c.query_row(&sql, [], |r| r.get::<_, i64>(0usize))?)
                    })
                    .map_err(|e| anyhow!("refused a read: {e:?}"))?;
                Ok(n.to_string())
            }
            "cut" => {
                ensure!(
                    self.drills,
                    "drills are off in this member's config (drills = on takes cut)"
                );
                let to = match rest {
                    "on" => true,
                    "off" => false,
                    other => bail!("cut {other}: on or off"),
                };
                self.cut.store(to, Ordering::Relaxed);
                Ok(String::new())
            }
            // A certificate renewed (CP-T1.13; `scripts/cluster.sh renew` asks once the new files
            // are in place): the TLS files read again, for the links made from now on.
            "reload" => {
                self.keys.reload()?;
                Ok(String::new())
            }
            // A disk cut or hung (T-T1.13.1): cut, every read, write and flush of this member's
            // log fails as a disk gone away fails them, its replica fences on the first and the
            // process ends with the reason; hung, each waits, its network going on, until the
            // disk is back or cut. A member started again finds its disk back.
            "disk" => {
                ensure!(
                    self.drills,
                    "drills are off in this member's config (drills = on takes disk)"
                );
                if rest == "held" {
                    let held = Database::log_disk_held(&self.log, until)
                        .map_err(|e| anyhow!("disk held: {e:?}"))?;
                    return Ok(format!("held={held}"));
                }
                let disk = match rest {
                    "cut" => LogDisk::Cut,
                    "hang" => LogDisk::Hung,
                    "back" => LogDisk::Back,
                    other => bail!("disk {other}: cut, hang, held or back"),
                };
                Database::set_log_disk(&self.log, disk)
                    .map_err(|e| anyhow!("disk {rest}: {e:?}"))?;
                Ok(String::new())
            }
            // A membership change, made by the leader (T-T1.13.1): a spare added as a learner,
            // promoted once it holds the log, a lost member demoted or removed.
            "member" => {
                let (what, n) = rest
                    .split_once(' ')
                    .ok_or_else(|| anyhow!("member add|promote|demote|remove NODE"))?;
                let n: u64 = n.trim().parse().context("member: its node")?;
                let change = match what {
                    "add" => {
                        ensure!(
                            self.ids.contains(&n),
                            "node {n} isn't in this member's config: name it in members (ID@HOST:PORT) and restart first"
                        );
                        MemberChange::AddLearner(n)
                    }
                    "promote" => MemberChange::Promote(n),
                    "demote" => MemberChange::Demote(n),
                    "remove" => MemberChange::Remove(n),
                    other => bail!("member {other}: add, promote, demote or remove"),
                };
                let s = self
                    .db
                    .replica()
                    .ok_or_else(|| anyhow!("no replica"))?
                    .change(change, until)
                    .map_err(|e| anyhow!("refused a membership change: {e:?}"))?;
                let list = |v: &[u64]| v.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
                Ok(format!(
                    "voters={} learners={}",
                    list(&s.voters),
                    list(&s.learners)
                ))
            }
            // Waits for a leader (CP-T1.13): a client that must write waits here while its group
            // elects, rather than retrying writes a follower refuses at once. It wakes on the
            // step that names the leader ([`crate::database::replicated::ReplicaHandle::wait_leader`])
            // and gives up at the caller's deadline, WAIT_MS from now.
            "leader" => {
                let wait: u64 = rest
                    .trim()
                    .parse()
                    .context("leader WAIT_MS: how long to wait, in milliseconds")?;
                let s = self
                    .db
                    .replica()
                    .ok_or_else(|| anyhow!("no replica"))?
                    .wait_leader(0, after(Duration::from_millis(wait))?)
                    .map_err(|e| anyhow!("no leader within {wait} ms: {e:?}"))?;
                Ok(format!(
                    "leader={} addr={} term={}",
                    s.leader,
                    self.addrs.get(&s.leader).map_or("-", String::as_str),
                    s.term
                ))
            }
            other => {
                bail!(
                    "unknown request {other:?} (status, exec, count, cut, disk, member, leader, reload)"
                )
            }
        }
    }
}

/// A peer's records of one route, in the order the member sent them, over one connection of
/// `kind` opened when the first comes and again after a failure: a record that can't be written
/// is lost, as a network loses it.
fn to_peer(
    node: u64,
    addr: &str,
    keys: &Keys,
    deadline: Duration,
    kind: u8,
    records: &mpsc::Receiver<Vec<u8>>,
) {
    let mut conn: Option<BufReader<Link>> = None;
    for bytes in records {
        if conn.is_none() {
            conn = open(addr, keys, deadline, kind)
                .and_then(|(mut s, _)| {
                    s.get_mut().write_all(&node.to_le_bytes())?;
                    Ok(s)
                })
                .ok();
        }
        let Some(s) = conn.as_mut() else { continue };
        let Ok(len) = u32::try_from(bytes.len()) else {
            continue;
        };
        let s = s.get_mut();
        if s.write_all(&len.to_le_bytes())
            .and_then(|()| s.write_all(&bytes))
            .and_then(|()| s.flush())
            .is_err()
        {
            conn = None;
        }
    }
}

/// A connection to `addr` of `kind`, inside mutual TLS when `keys` hold it, the secret proved; and
/// the socket under it, for its timeouts.
fn open(
    addr: &str,
    keys: &Keys,
    deadline: Duration,
    kind: u8,
) -> anyhow::Result<(BufReader<Link>, TcpStream)> {
    let s = TcpStream::connect_timeout(&resolve(addr)?, deadline)
        .with_context(|| format!("connecting to {addr}"))?;
    s.set_nodelay(true)?;
    let socket = s.try_clone()?;
    let link = keys.dialed(s, addr, after(deadline)?)?;
    socket.set_read_timeout(Some(deadline))?;
    socket.set_write_timeout(Some(deadline))?;
    let mut r = BufReader::new(link);
    r.get_mut().write_all(&[kind])?;
    r.get_mut().flush()?;
    let mut nonce = [0u8; NONCE];
    r.read_exact(&mut nonce)?;
    let mut tag = keyed(&keys.secret)?;
    tag.update(&nonce);
    tag.update(&[kind]);
    r.get_mut().write_all(&tag.finalize().into_bytes())?;
    r.get_mut().flush()?;
    Ok((r, socket))
}

/// A client of one member: requests one line each, answered in order.
pub struct Client {
    r: BufReader<Link>,
    /// The socket under the link, for the read timeout of a request that waits.
    socket: TcpStream,
    deadline: Duration,
}

impl Client {
    /// A client of the member at `addr`, its link proving `keys`.
    pub fn connect(addr: &str, keys: &Keys, deadline: Duration) -> anyhow::Result<Self> {
        let (r, socket) = open(addr, keys, deadline, CLIENT)?;
        Ok(Self {
            r,
            socket,
            deadline,
        })
    }

    /// The member's `leader` request: waits at most `wait` until the member knows its group's
    /// leader and answers its ID and address as the member's config names it (`-` for a leader
    /// it doesn't name). The connection waits `wait` and its own deadline for the answer.
    pub fn leader(&mut self, wait: Duration) -> anyhow::Result<(u64, String)> {
        let millis = u64::try_from(wait.as_millis()).context("a wait past u64 milliseconds")?;
        self.socket
            .set_read_timeout(Some(wait.saturating_add(self.deadline)))?;
        let answer = self.ask(&format!("leader {millis}"));
        self.socket.set_read_timeout(Some(self.deadline))?;
        let f = fields(&answer?);
        let id = f
            .get("leader")
            .ok_or_else(|| anyhow!("a leader answer without its leader"))?
            .parse()
            .context("the leader's ID")?;
        let addr = f
            .get("addr")
            .cloned()
            .ok_or_else(|| anyhow!("a leader answer without its address"))?;
        Ok((id, addr))
    }

    /// The member's answer to `request`, past its `ok`; its `err` as the error.
    pub fn ask(&mut self, request: &str) -> anyhow::Result<String> {
        ensure!(!request.contains(['\n', '\r']), "a request is one line");
        let mut line = request.to_owned();
        line.push('\n');
        self.r.get_mut().write_all(line.as_bytes())?;
        self.r.get_mut().flush()?;
        let back =
            read_line(&mut self.r)?.ok_or_else(|| anyhow!("the member closed the connection"))?;
        if let Some(e) = back.strip_prefix("err ") {
            bail!("{e}");
        }
        Ok(back
            .strip_prefix("ok")
            .unwrap_or(&back)
            .trim_start()
            .to_owned())
    }
}

/// A client of one member's SQL (`Q` connections; docs/design/client-protocol.md, decision 2):
/// statements with bound parameters, answered with SQLite's typed rows and their columns' names,
/// over one connection kept for every request.
pub struct SqlClient {
    r: BufReader<Link>,
    /// An exchange failed partway (a timeout, a cut link, an answer this process couldn't hold):
    /// its answer may still come, so no later request may read it as its own.
    broken: bool,
    /// How long an answer's frame may wait for room in this process's memory budget.
    deadline: Duration,
}

impl SqlClient {
    /// A SQL client of the member at `addr`, its link proving `keys`, the versions agreed.
    pub fn connect(addr: &str, keys: &Keys, deadline: Duration) -> anyhow::Result<Self> {
        let (r, _socket) = open(addr, keys, deadline, SQL)?;
        let mut client = Self {
            r,
            broken: false,
            deadline,
        };
        match client.ask(&SqlRequest::Hello {
            oldest: SQL_VERSION_MIN,
            newest: SQL_VERSION,
        })? {
            SqlAnswer::Agreed(_) => Ok(client),
            SqlAnswer::Refused(refusal) => Err(refusal.into()),
            SqlAnswer::Tables(_) | SqlAnswer::Rows { .. } => {
                bail!("the member answered its greeting with tables")
            }
        }
    }

    /// `statements` run in order as one transaction on the member's group's writer: each one's
    /// table; or the member's refusal (a [`Refusal`] inside the error), every statement rolled back.
    pub fn write(&mut self, statements: Vec<Statement>) -> anyhow::Result<Vec<Table>> {
        self.tables(&SqlRequest::Write(statements))
    }

    /// `statement` run on the member's own state, asking no leader: its table.
    pub fn read(&mut self, statement: Statement) -> anyhow::Result<Table> {
        self.tables(&SqlRequest::Read(statement))?
            .pop()
            .ok_or_else(|| anyhow!("a read answered with no table"))
    }

    fn tables(&mut self, request: &SqlRequest) -> anyhow::Result<Vec<Table>> {
        match self.ask(request)? {
            SqlAnswer::Tables(tables) => Ok(tables),
            SqlAnswer::Refused(refusal) => Err(refusal.into()),
            SqlAnswer::Agreed(_) | SqlAnswer::Rows { .. } => {
                bail!("the member answered a request with a greeting")
            }
        }
    }

    fn ask(&mut self, request: &SqlRequest) -> anyhow::Result<SqlAnswer> {
        ensure!(
            !self.broken,
            "an earlier request on this connection failed partway: open another"
        );
        let bytes = postcard::to_allocvec(request)?;
        amphora_api::bound("a SQL request", bytes.len(), amphora_api::REQUEST_MAX)?;
        let statements = match request {
            SqlRequest::Hello { .. } => 0,
            SqlRequest::Write(statements) => statements.len(),
            SqlRequest::Read(_) => 1,
        };
        self.broken = true;
        write_frame(self.r.get_mut(), &bytes)?;
        let answer = answer(&mut self.r, statements, self.deadline)?;
        self.broken = false;
        Ok(answer)
    }
}

/// The answer to a request of `statements` statements, read from `r`: its last frame, the rows of
/// the frames before it put ahead of each table's own. Each frame waits for room `deadline` from
/// its first byte: its client's own connection, whose owner no end comes to.
fn answer(r: &mut impl Read, statements: usize, deadline: Duration) -> anyhow::Result<SqlAnswer> {
    let mut early: Vec<Vec<Vec<Value>>> = Vec::new();
    loop {
        let answer = match read_frame(r, amphora_api::RESULT_MAX, deadline, &memory::Ending::new())?
        {
            Some(Frame::Held(bytes, _charge, _)) => postcard::from_bytes(&bytes)?,
            // Not the member's refusal: a write's outcome is unknown here, so no `Refusal` to read
            // as rolled back.
            Some(Frame::Refused(refusal)) => {
                bail!(
                    "this process's memory budget refused the member's answer ({refusal}): its outcome is unknown here"
                )
            }
            None => bail!("the member closed the connection"),
        };
        match answer {
            SqlAnswer::Rows {
                statement,
                mut rows,
            } => {
                let statement = usize::try_from(statement)?;
                ensure!(
                    statement < statements,
                    "rows of statement {statement} answering {statements}"
                );
                if early.len() <= statement {
                    early.resize_with(statement.saturating_add(1), Vec::new);
                }
                if let Some(ahead) = early.get_mut(statement) {
                    ahead.append(&mut rows);
                }
            }
            SqlAnswer::Tables(mut tables) => {
                ensure!(
                    early.len() <= tables.len(),
                    "rows of {} statements before {} tables",
                    early.len(),
                    tables.len()
                );
                for (table, mut ahead) in tables.iter_mut().zip(early) {
                    ahead.append(&mut table.rows);
                    table.rows = ahead;
                }
                return Ok(SqlAnswer::Tables(tables));
            }
            other => return Ok(other),
        }
    }
}

/// `key=value` pairs of a status answer.
fn fields(status: &str) -> BTreeMap<String, String> {
    status
        .split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

/// Every member's status and the verdict: healthy when every member answers, all name one leader
/// that is a member, each holds the configured voters and none is fenced or cut.
pub fn health(config: &Config) -> anyhow::Result<(String, bool)> {
    let keys = config.keys()?;
    // The config's voters are the cluster it wants: a spare, or a lost member removed, isn't asked.
    let mut want = config.voters.iter().map(u64::to_string).collect::<Vec<_>>();
    want.sort();
    let mut out = String::new();
    let mut leaders = BTreeSet::new();
    let mut healthy = true;
    for m in config
        .members
        .iter()
        .filter(|m| config.voters.contains(&m.node))
    {
        match Client::connect(&m.addr, &keys, config.deadline).and_then(|mut c| c.ask("status")) {
            Ok(s) => {
                let f = fields(&s);
                let mut voters: Vec<String> = f
                    .get("voters")
                    .map(|v| v.split(',').map(str::to_owned).collect())
                    .unwrap_or_default();
                voters.sort();
                let leader = f.get("leader").cloned().unwrap_or_default();
                let mut why = Vec::new();
                if voters != want {
                    why.push("voters differ from the config");
                }
                if f.get("fenced").is_none_or(|v| v != "-") {
                    why.push("fenced");
                }
                if f.get("cut").is_none_or(|v| v != "false") {
                    why.push("cut off by a drill");
                }
                if !want.contains(&leader) {
                    why.push("names no leader among the members");
                }
                leaders.insert(leader);
                healthy &= why.is_empty();
                out.push_str(&format!(
                    "member {} at {}: {s}{}{}\n",
                    m.node,
                    m.addr,
                    if why.is_empty() { "" } else { " — " },
                    why.join(", ")
                ));
            }
            Err(e) => {
                healthy = false;
                out.push_str(&format!(
                    "member {} at {}: unreachable: {e:#}\n",
                    m.node, m.addr
                ));
            }
        }
    }
    if leaders.len() > 1 {
        healthy = false;
        out.push_str(&format!(
            "members name different leaders: {}\n",
            leaders.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    out.push_str(if healthy { "healthy\n" } else { "unhealthy\n" });
    Ok((out, healthy))
}

/// One bucket a bit of a u64 of microseconds.
const BUCKETS: usize = 64;

/// Latencies in powers of two of microseconds: bounded whatever the run's length.
#[derive(Clone)]
struct Hist {
    buckets: [u64; BUCKETS],
    n: u64,
}

impl Default for Hist {
    fn default() -> Self {
        Self {
            buckets: [0; BUCKETS],
            n: 0,
        }
    }
}

impl Hist {
    fn add(&mut self, d: Duration) {
        let us = u64::try_from(d.as_micros()).unwrap_or(u64::MAX).max(1);
        let at = usize::try_from(
            u64::BITS
                .saturating_sub(us.leading_zeros())
                .saturating_sub(1),
        )
        .unwrap_or(0);
        if let Some(b) = self.buckets.get_mut(at) {
            *b = b.saturating_add(1);
        }
        self.n = self.n.saturating_add(1);
    }

    fn merge(&mut self, o: &Self) {
        for (a, b) in self.buckets.iter_mut().zip(o.buckets.iter()) {
            *a = a.saturating_add(*b);
        }
        self.n = self.n.saturating_add(o.n);
    }

    /// The bucket's upper bound (µs) at or below which `per_mille` of the samples fall.
    fn at(&self, per_mille: u64) -> u64 {
        let want = self.n.saturating_mul(per_mille).div_ceil(1000).max(1);
        let mut seen = 0u64;
        for (i, b) in self.buckets.iter().enumerate() {
            seen = seen.saturating_add(*b);
            if seen >= want {
                return 1u64
                    .checked_shl(u32::try_from(i).unwrap_or(63).saturating_add(1))
                    .unwrap_or(u64::MAX);
            }
        }
        0
    }
}

fn run_name(run: &str) -> anyhow::Result<&str> {
    ensure!(
        !run.is_empty()
            && run
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "--run takes letters, digits, - and _"
    );
    Ok(run)
}

const TABLE: &str = "CREATE TABLE IF NOT EXISTS drill (run TEXT NOT NULL, writer INTEGER NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY (run, writer, seq))";

/// A drill's load (AC-T1.13.5): `writers` closed-loop writers for `seconds`, each writing its
/// sequence 1, 2, 3, … one outstanding at a time, through the members in turn when one refuses.
/// A refused or unanswered write is sent again unchanged (`INSERT OR IGNORE`, the same key), so
/// every number up to a writer's last acknowledged one was acknowledged: the ledger keeps each
/// writer's last, and [`check`] counts them on every member. Returns the run's summary line.
pub fn load(
    config: &Config,
    run: &str,
    writers: u64,
    seconds: Duration,
    ledger: &Path,
) -> anyhow::Result<String> {
    let run = run_name(run)?.to_owned();
    ensure!(writers > 0, "--writers takes at least 1");
    let keys = Arc::new(config.keys()?);
    let addrs: Arc<Vec<String>> = Arc::new(config.members.iter().map(|m| m.addr.clone()).collect());
    let made = after(config.deadline)?;
    let mut table_made = false;
    while !table_made && Instant::now() < made {
        for a in addrs.iter() {
            if Client::connect(a, &keys, config.deadline)
                .and_then(|mut c| c.ask(&format!("exec {TABLE}")))
                .is_ok()
            {
                table_made = true;
                break;
            }
        }
    }
    ensure!(
        table_made,
        "no member made the drill's table within deadline_ms"
    );
    let start = Instant::now();
    let end = start
        .checked_add(seconds)
        .ok_or_else(|| anyhow!("--seconds past the clock's range"))?;
    let retries = Arc::new(AtomicU64::new(0));
    let mut threads = Vec::new();
    for w in 1..=writers {
        let (keys, addrs, retries, run, deadline) = (
            Arc::clone(&keys),
            Arc::clone(&addrs),
            Arc::clone(&retries),
            run.clone(),
            config.deadline,
        );
        threads.push(std::thread::Builder::new().name(format!("drill-writer-{w}")).spawn(move || {
            let n = addrs.len().max(1);
            let mut via = usize::try_from(w).unwrap_or(0).checked_rem(n).unwrap_or(0);
            let (mut seq, mut acked, mut hist) = (1u64, 0u64, Hist::default());
            let mut conn: Option<Client> = None;
            while Instant::now() < end {
                if conn.is_none() {
                    conn = addrs.get(via).and_then(|a| Client::connect(a, &keys, deadline).ok());
                }
                let Some(c) = conn.as_mut() else {
                    via = via.saturating_add(1).checked_rem(n).unwrap_or(0);
                    retries.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let t = Instant::now();
                match c.ask(&format!("exec INSERT OR IGNORE INTO drill (run, writer, seq) VALUES ('{run}', {w}, {seq})")) {
                    Ok(_) => {
                        hist.add(t.elapsed());
                        acked = seq;
                        seq = seq.saturating_add(1);
                    }
                    Err(_) => {
                        conn = None;
                        retries.fetch_add(1, Ordering::Relaxed);
                        // Sent again where the leader is: the next member asked for it (the
                        // `leader` request, answered once the group has one, woken by the
                        // election), as the bench's client finds it; that member itself when it
                        // can't say.
                        via = via.saturating_add(1).checked_rem(n).unwrap_or(0);
                        let named = addrs
                            .get(via)
                            .and_then(|a| Client::connect(a, &keys, deadline).ok())
                            .and_then(|mut c| c.leader(deadline).ok())
                            .and_then(|(_, at)| addrs.iter().position(|a| *a == at));
                        if let Some(at) = named {
                            via = at;
                        }
                    }
                }
            }
            (w, acked, hist)
        })?);
    }
    let (mut lines, mut total, mut hist) = (String::new(), 0u64, Hist::default());
    for t in threads {
        let (w, acked, h) = t.join().map_err(|_| anyhow!("a writer thread failed"))?;
        lines.push_str(&format!("{run} {w} {acked}\n"));
        total = total.saturating_add(acked);
        hist.merge(&h);
    }
    std::fs::write(ledger, &lines).with_context(|| format!("writing {}", ledger.display()))?;
    let secs = start.elapsed().as_secs_f64();
    let rate = if secs > 0.0 { total as f64 / secs } else { 0.0 };
    Ok(format!(
        "run {run}: {total} writes acknowledged by {writers} writers in {secs:.1} s ({rate:.0}/s), {} refused or unanswered and sent again; latency p50 <= {} us, p99 <= {} us, p99.9 <= {} us\n",
        retries.load(Ordering::Relaxed),
        hist.at(500),
        hist.at(990),
        hist.at(999)
    ))
}

/// A drill's loss count (AC-T1.13.5): on every member, each ledger writer's acknowledged writes
/// counted from the member's own copy, asked again until it holds them all or `catch_up` passes.
/// Lost is what a member still lacks then; the verdict holds when every member answered and none
/// lacks any.
pub fn check(config: &Config, ledger: &Path, catch_up: Duration) -> anyhow::Result<(String, bool)> {
    let keys = config.keys()?;
    let text =
        std::fs::read_to_string(ledger).with_context(|| format!("reading {}", ledger.display()))?;
    let mut rows = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut it = line.split_whitespace();
        let (Some(run), Some(w), Some(acked), None) = (it.next(), it.next(), it.next(), it.next())
        else {
            bail!("ledger line {line:?}: not RUN WRITER ACKED")
        };
        rows.push((
            run_name(run)?.to_owned(),
            w.parse::<u64>()?,
            acked.parse::<u64>()?,
        ));
    }
    let acked: u64 = rows.iter().map(|r| r.2).fold(0u64, u64::saturating_add);
    let until = Instant::now()
        .checked_add(catch_up)
        .ok_or_else(|| anyhow!("--catch-up-s past the clock's range"))?;
    let (mut out, mut ok) = (String::new(), true);
    // Counted on the voters the config names, as health asks them: a lost member removed isn't.
    for m in config
        .members
        .iter()
        .filter(|m| config.voters.contains(&m.node))
    {
        let mut lost = 0u64;
        let mut failed = None;
        let mut client = None;
        for (run, w, want) in &rows {
            let mut have = 0u64;
            loop {
                if client.is_none() {
                    client = Client::connect(&m.addr, &keys, config.deadline).ok();
                }
                if let Some(c) = client.as_mut() {
                    match c.ask(&format!("count SELECT count(*) FROM drill WHERE run = '{run}' AND writer = {w} AND seq <= {want}")) {
                        Ok(n) => have = n.parse().unwrap_or(0),
                        Err(e) => {
                            failed = Some(format!("{e:#}"));
                            client = None;
                        }
                    }
                }
                if have >= *want || Instant::now() >= until {
                    break;
                }
            }
            lost = lost.saturating_add(want.saturating_sub(have));
        }
        let answered = client.is_some();
        ok &= answered && lost == 0;
        match (answered, failed) {
            (false, Some(e)) => out.push_str(&format!("member {}: unreachable: {e}\n", m.node)),
            (false, None) => out.push_str(&format!("member {}: unreachable\n", m.node)),
            _ => out.push_str(&format!(
                "member {}: {acked} acknowledged, {lost} lost\n",
                m.node
            )),
        }
    }
    out.push_str(&format!(
        "{}: {acked} acknowledged writes in {}\n",
        if ok { "nothing lost" } else { "FAILED" },
        ledger.display()
    ));
    Ok((out, ok))
}

pub const USAGE: &str = "usage: amphora serve CONFIG
       amphora health CONFIG
       amphora ask CONFIG NODE REQUEST...
       amphora member CONFIG add|promote|demote|remove NODE
       amphora drill load CONFIG --run NAME --writers N --seconds S --ledger FILE
       amphora drill check CONFIG --ledger FILE --catch-up-s S
";

/// The reply to `serve`, `health`, `ask` and `drill`: `None` for any other first argument.
pub fn command(args: &[String], ready: impl FnOnce(&str)) -> Option<crate::Reply> {
    let reply = |r: anyhow::Result<(String, bool)>| match r {
        Ok((text, true)) => crate::Reply {
            text,
            to_stderr: false,
            status: 0,
        },
        Ok((text, false)) => crate::Reply {
            text,
            to_stderr: false,
            status: 1,
        },
        Err(e) => crate::Reply {
            text: format!("amphora: {e:#}\n"),
            to_stderr: true,
            status: 2,
        },
    };
    let flag = |rest: &[String], name: &str| -> anyhow::Result<String> {
        let at = rest
            .iter()
            .position(|a| a == name)
            .ok_or_else(|| anyhow!("{name} is required\n{USAGE}"))?;
        rest.get(at.saturating_add(1))
            .cloned()
            .ok_or_else(|| anyhow!("{name} takes a value"))
    };
    let secs = |v: String| -> anyhow::Result<Duration> {
        Duration::try_from_secs_f64(v.parse::<f64>()?).map_err(|e| anyhow!("{v}: {e}"))
    };
    let (first, rest) = args.split_first()?;
    Some(match (first.as_str(), rest) {
        ("serve", [config]) => reply(
            Config::load(Path::new(config))
                .and_then(|c| serve(&c, ready))
                .map(|()| (String::new(), true)),
        ),
        ("health", [config]) => reply(Config::load(Path::new(config)).and_then(|c| health(&c))),
        ("ask", [config, node, request @ ..]) if !request.is_empty() => reply((|| {
            let c = Config::load(Path::new(config))?;
            let m = c.member(node.parse()?)?;
            let mut k = Client::connect(&m.addr, &c.keys()?, c.deadline)?;
            let answer = match request {
                [what, wait] if what == "leader" => {
                    let (id, addr) = k.leader(Duration::from_millis(wait.parse()?))?;
                    format!("leader={id} addr={addr}")
                }
                _ => k.ask(&request.join(" "))?,
            };
            Ok((format!("{answer}\n"), true))
        })()),
        // The change goes to each voter in turn until one, the leader, makes it: a follower
        // refuses it, and a lost member doesn't answer, so a stale idea of the leader costs nothing.
        ("member", [config, what, node]) => reply((|| {
            let c = Config::load(Path::new(config))?;
            let keys = c.keys()?;
            let request = format!("member {what} {node}");
            let mut refused = Vec::new();
            for m in c.members.iter().filter(|m| c.voters.contains(&m.node)) {
                match Client::connect(&m.addr, &keys, c.deadline).and_then(|mut k| k.ask(&request))
                {
                    Ok(answer) => {
                        return Ok((
                            format!(
                                "{answer}
"
                            ),
                            true,
                        ));
                    }
                    Err(e) => refused.push(format!("member {}: {e:#}", m.node)),
                }
            }
            bail!("no voter made the change: {}", refused.join("; "))
        })()),
        ("drill", [what, config, flags @ ..]) if what == "load" => reply((|| {
            let c = Config::load(Path::new(config))?;
            let text = load(
                &c,
                &flag(flags, "--run")?,
                flag(flags, "--writers")?.parse()?,
                secs(flag(flags, "--seconds")?)?,
                Path::new(&flag(flags, "--ledger")?),
            )?;
            Ok((text, true))
        })()),
        ("drill", [what, config, flags @ ..]) if what == "check" => reply((|| {
            let c = Config::load(Path::new(config))?;
            check(
                &c,
                Path::new(&flag(flags, "--ledger")?),
                secs(flag(flags, "--catch-up-s")?)?,
            )
        })()),
        ("serve" | "health" | "ask" | "member" | "drill", _) => crate::Reply {
            text: USAGE.to_owned(),
            to_stderr: true,
            status: 2,
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "node = 2\nmembers = 1@a:7401, 2@b:7401,3@c:7401 # three machines\ndata = /d\nclass = persisted\nsecret_file = /s\ndeadline_ms = 5000\n";

    #[test]
    fn t_t1_13_1_config_reads_every_key_and_refuses_what_it_doesnt_know() {
        let c = Config::parse(GOOD).unwrap();
        assert_eq!(
            c.voters,
            BTreeSet::from([1, 2, 3]),
            "every member votes by default"
        );
        let spare = Config::parse(&format!("{GOOD}voters = 1,2\n")).unwrap();
        assert_eq!(spare.voters, BTreeSet::from([1, 2]));
        assert_eq!(
            (c.node, c.members.len(), c.drills, c.deadline),
            (2, 3, false, Duration::from_millis(5000))
        );
        assert_eq!(
            c.members[1],
            Member {
                node: 2,
                addr: "b:7401".into()
            }
        );
        for (text, why) in [
            (GOOD.replace("node = 2", "node = 4"), "isn't among"),
            (format!("{GOOD}colour = red\n"), "unknown key"),
            (format!("{GOOD}node = 1\n"), "twice"),
            (GOOD.replace("deadline_ms = 5000\n", ""), "no deadline_ms"),
            (GOOD.replace("3@c:7401", "2@c:7401"), "twice"),
            (GOOD.replace("3@c:7401", "3@c"), "ID@HOST:PORT"),
            (GOOD.replace("persisted", "replicated"), "no spanning"),
            (format!("{GOOD}drills = maybe\n"), "on or off"),
            (format!("{GOOD}voters = 1,4\n"), "isn't among the members"),
            (format!("{GOOD}voters = 1,x\n"), "voters: x"),
            (format!("{GOOD}cert_file = /c\n"), "go together"),
            (
                format!("{GOOD}cert_file = /c\nkey_file = /k\n"),
                "go together",
            ),
        ] {
            let e = format!("{:#}", Config::parse(&text).err().unwrap());
            assert!(e.contains(why), "{e} lacks {why}");
        }
        assert!(
            Config::parse(&format!("{GOOD}drills = on\n"))
                .unwrap()
                .drills
        );
        assert_eq!(c.tls, None, "no TLS files: the links unencrypted");
        let tls = Config::parse(&format!(
            "{GOOD}cert_file = /c\nkey_file = /k\nroots_file = /r\n"
        ))
        .unwrap();
        assert_eq!(
            tls.tls,
            Some(TlsFiles {
                cert: "/c".into(),
                key: "/k".into(),
                roots: "/r".into()
            })
        );
    }

    /// CP-T1.13: a config's TLS files are read when its keys are made: a file that isn't there
    /// refuses them, so a member never starts with TLS named and the links bare.
    #[test]
    fn cp_t1_13_keys_read_the_tls_files_or_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        std::fs::write(&secret, "0123456789abcdef0123456789abcdef-test-secret\n").unwrap();
        let text = GOOD.replace("/s", secret.to_str().unwrap());
        assert!(Config::parse(&text).unwrap().keys().unwrap().tls.is_none());
        let missing = dir.path().join("missing.pem");
        let named = Config::parse(&format!(
            "{text}cert_file = {0}\nkey_file = {0}\nroots_file = {0}\n",
            missing.display()
        ))
        .unwrap();
        let e = format!("{:#}", named.keys().err().unwrap());
        assert!(e.contains("missing.pem"), "{e}");
    }

    #[test]
    fn t_t1_13_1_histogram_bounds_hold_whatever_the_samples() {
        let mut h = Hist::default();
        for us in [1u64, 3, 900, 1000, 70_000] {
            h.add(Duration::from_micros(us));
        }
        assert_eq!(h.n, 5);
        assert_eq!(h.at(500), 1024);
        assert_eq!(h.at(999), 131_072);
        assert_eq!(Hist::default().at(500), 0);
    }

    fn statement(sql: &str, params: Vec<Value>) -> Statement {
        Statement {
            sql: sql.to_owned(),
            params,
        }
    }

    fn sql_until() -> Instant {
        Instant::now() + Duration::from_secs(30)
    }

    /// Lane perf-A19 (AUDIT 2026-11-23 §1, MB.1): a request's frame waits for room until its
    /// deadline, never refused at once. Do: with the memory budget's whole limit held by another
    /// charge, read a frame whose deadline is an hour off on another thread; once it waits, give
    /// that charge back. Expect: the frame held with its bytes as sent and charged their length,
    /// and nothing held once it drops.
    #[test]
    fn a19_a_requests_frame_waits_for_room_until_its_deadline() {
        use crate::memory::tests::{BUDGET, Fed, budget};
        let fed = Fed::new(Some(1 << 20), Some(0));
        let (budget, _unset) = budget(&fed);
        let other = budget.take(Use::Queues, 1 << 20).unwrap();
        let mut sent = Vec::new();
        write_frame(&mut sent, b"hello").unwrap();
        let on_reader = Arc::clone(&budget);
        let reader = std::thread::spawn(move || {
            BUDGET.with(|slot| *slot.borrow_mut() = Some(on_reader));
            match read_frame(
                &mut sent.as_slice(),
                amphora_api::REQUEST_MAX,
                Duration::from_secs(3600),
                &memory::Ending::new(),
            ) {
                Ok(Some(Frame::Held(bytes, charge, _))) => Some((bytes, charge.bytes())),
                _ => None,
            }
        });
        while budget.waiting() == 0 && !reader.is_finished() {
            std::thread::yield_now();
        }
        drop(other);
        assert_eq!(reader.join().unwrap(), Some((b"hello".to_vec(), 5)));
        assert_eq!(budget.held(Use::Queues), 0);
    }

    /// A member of one over `dir`, as `serve` opens it, its drills on.
    fn member_of_one(dir: &Path, deadline: Duration) -> Arc<Shared> {
        let replica = ReplicaConfig {
            node_id: 1,
            voters: vec![1],
            learners: Vec::new(),
            bootstrap: None,
        };
        let db = Database::open_replicated_with(
            &dir.join("state.db"),
            &dir.join("raft.log"),
            replica,
            Durability::Persisted,
            sql_until(),
        )
        .unwrap();
        Arc::new(Shared {
            node: 1,
            ids: BTreeSet::from([1]),
            db,
            cut: AtomicBool::new(false),
            deadline,
            keys: Keys {
                secret: vec![7; 32],
                tls: None,
            },
            drills: true,
            log: dir.join("raft.log"),
            addrs: BTreeMap::new(),
            links: Mutex::new(BTreeMap::new()),
        })
    }

    /// MB.1 (T-MB.1.7, lane perf-A28): what waits for the memory budget's room for a member's work
    /// hears its replica end, as a disk cut off ends it, and holds nothing. Do: on a member of one,
    /// another use holding the budget, a `Q` request whose frame finds no room, one whose decoded
    /// values find none, or a call on the member's replica, waits for room; its log's disk is cut and
    /// the next write fences its owner. Expect: the request answered `Refused(Fenced)`, the call
    /// refused so; none waiting, nothing held.
    #[test]
    fn a28_what_waits_for_room_hears_its_replicas_end_holding_nothing() {
        use crate::memory::tests::{BUDGET, Fed};
        const LIMIT: u64 = 1 << 20;
        for (waits, room) in [("a frame", 0), ("its values", 8 << 10), ("a call", 0)] {
            let dir = tempfile::tempdir().unwrap();
            let shared = member_of_one(dir.path(), crate::transport::HANG);
            shared
                .db
                .call(sql_until(), crate::database::AnswerMax::fixed(), |c| {
                    c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY)")?;
                    Ok(())
                })
                .unwrap();
            let budget = memory::Budget::new(Box::new(Fed::new(Some(LIMIT), Some(0))));
            let began = Instant::now();
            // Its deadline past the member's own, so that the member's answer is what it reads.
            let far = crate::transport::HANG * 2;
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap().to_string();
            let served = {
                let (shared, on_served) = (Arc::clone(&shared), Arc::clone(&budget));
                std::thread::spawn(move || {
                    BUDGET.with(|slot| *slot.borrow_mut() = Some(on_served));
                    let (s, _) = listener.accept().unwrap();
                    connection(&shared, s)
                })
            };
            let mut client = SqlClient::connect(&addr, &shared.keys, far).unwrap();
            let other = budget.take(Use::Images, LIMIT - room).unwrap();
            let on_request = Arc::clone(&budget);
            let called = Arc::clone(&shared);
            let asked = std::thread::spawn(move || -> anyhow::Result<()> {
                if waits == "a call" {
                    BUDGET.with(|slot| *slot.borrow_mut() = Some(on_request));
                    let until = after(crate::transport::HANG)?;
                    called
                        .db
                        .call(until, crate::database::AnswerMax::fixed(), |c| {
                            c.execute_batch("INSERT INTO t VALUES (1)")?;
                            Ok(())
                        })?;
                } else {
                    let nulls = vec![Value::Null; amphora_api::TX_MAX];
                    client.read(statement("SELECT 1", nulls))?;
                }
                Ok(())
            });
            while budget.waiting() == 0 {
                assert!(!asked.is_finished(), "{waits}: answered");
                assert!(began.elapsed() < crate::transport::HANG, "{waits}: a hang");
                std::thread::yield_now();
            }
            Database::set_log_disk(&shared.log, LogDisk::Cut).unwrap();
            let fenced = shared
                .db
                .call(sql_until(), crate::database::AnswerMax::fixed(), |c| {
                    c.execute_batch("INSERT INTO t VALUES (2)")?;
                    Ok(())
                });
            assert!(fenced.is_err(), "{waits}: a write on a cut disk");
            while !asked.is_finished() {
                assert!(began.elapsed() < far, "{waits}: a hang");
                std::thread::yield_now();
            }
            let refused = asked.join().unwrap().unwrap_err();
            assert!(
                matches!(refused.downcast_ref::<Refusal>(), Some(Refusal::Fenced(_))),
                "{waits}: {refused:#} after {:?}",
                began.elapsed()
            );
            assert_eq!(budget.waiting(), 0, "{waits}");
            drop(other);
            drop(shared);
            let _ = served;
            assert_eq!(budget.held(Use::Queues), 0, "{waits}");
        }
    }

    /// CP-T1.13, AC-T1.2.3 (decision 2's bounds): do decode a `Q` request with one parameter past
    /// the API's transaction bound, as a write and as a read; expect both refused while decoding,
    /// as a `C` connection's are, and the same request at the bound decoded and its memory charged.
    #[test]
    fn cp_t1_13_a_sql_request_past_the_parameter_bound_is_refused_while_decoding() {
        let nulls = |n| vec![Value::Null; n];
        for request in [
            SqlRequest::Write(vec![statement("SELECT 1", nulls(amphora_api::TX_MAX + 1))]),
            SqlRequest::Read(statement("SELECT 1", nulls(amphora_api::TX_MAX + 1))),
            SqlRequest::Write(vec![statement("SELECT 1", vec![]); amphora_api::TX_MAX + 1]),
        ] {
            let bytes = postcard::to_allocvec(&request).unwrap();
            assert!(
                postcard::from_bytes::<SqlRequest>(&bytes).is_err(),
                "decoded {} bytes past the bound",
                bytes.len()
            );
        }
        let at = SqlRequest::Read(statement("SELECT 1", nulls(amphora_api::TX_MAX)));
        let decoded: SqlRequest =
            postcard::from_bytes(&postcard::to_allocvec(&at).unwrap()).unwrap();
        let charged = request_charge(&decoded, sql_until(), &memory::Ending::new()).unwrap();
        assert!(
            charged.bytes()
                >= u64::try_from(amphora_api::TX_MAX * std::mem::size_of::<Value>()).unwrap(),
            "charged {} for {} decoded values",
            charged.bytes(),
            amphora_api::TX_MAX
        );
    }

    /// CP-T1.13, AC-T1.2.3: do read a row of two values, each one byte past half the API's result
    /// bound (its request within the request bound); expect the read refused TooLarge (not
    /// Invalid), and a write whose answer passes the bound refused TooLarge with its insert rolled
    /// back: the answer is held inside the write's transaction.
    #[test]
    fn cp_t1_13_a_sql_answer_past_the_result_bound_is_refused_too_large_and_its_write_rolled_back()
    {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("q.db"), sql_until()).unwrap();
        db.call(
            sql_until(),
            crate::database::AnswerMax::bytes(amphora_api::RESULT_MAX),
            |c| {
                answered(
                    c,
                    vec![statement("CREATE TABLE t (id INTEGER PRIMARY KEY)", vec![])],
                    Spill::None,
                )
            },
        )
        .unwrap();
        let half = || Value::Blob(vec![0; amphora_api::RESULT_MAX / 2 + 1]);
        let read = db.query_local(sql_until(), move |c| {
            answered(
                c,
                vec![statement("SELECT ?1, ?1", vec![half()])],
                Spill::None,
            )
        });
        assert!(
            matches!(read, Err(Refusal::TooLarge { .. })),
            "{:?}",
            read.map(|e| e.len())
        );
        let write = db.call(
            sql_until(),
            crate::database::AnswerMax::bytes(amphora_api::RESULT_MAX),
            move |c| {
                answered(
                    c,
                    vec![
                        statement("INSERT INTO t VALUES (1)", vec![]),
                        statement("SELECT ?1, ?1", vec![half()]),
                    ],
                    Spill::None,
                )
            },
        );
        assert!(
            matches!(write, Err(Refusal::TooLarge { .. })),
            "{:?}",
            write.map(|e| e.len())
        );
        let count = db
            .query_local(sql_until(), |c| {
                answered(
                    c,
                    vec![statement("SELECT count(*) FROM t", vec![])],
                    Spill::None,
                )
            })
            .unwrap();
        let [count] = count.as_slice() else {
            panic!("{} frames", count.len());
        };
        let SqlAnswer::Tables(tables) = postcard::from_bytes::<SqlAnswer>(&count.bytes).unwrap()
        else {
            panic!("not tables");
        };
        assert_eq!(
            tables[0].rows,
            vec![vec![Value::Integer(0)]],
            "the refused write's insert stayed"
        );
    }

    /// CP-T1.13, AC-T1.2.3: do ask a member that takes a request and never answers it, past the
    /// client's deadline; expect that request to fail and the next on the same connection refused
    /// before it is sent, so no later request reads the late answer as its own.
    #[test]
    fn cp_t1_13_a_sql_connection_is_not_reused_after_a_request_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (done, wait) = mpsc::channel::<()>();
        let member = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut kind = [0u8; 1];
            s.read_exact(&mut kind).unwrap();
            s.write_all(&[0u8; NONCE]).unwrap();
            let mut tag = [0u8; TAG];
            s.read_exact(&mut tag).unwrap();
            let Some(Frame::Held(hello, ..)) = read_frame(
                &mut s,
                amphora_api::REQUEST_MAX,
                Duration::from_secs(30),
                &memory::Ending::new(),
            )
            .unwrap() else {
                panic!("no greeting");
            };
            assert!(matches!(
                postcard::from_bytes::<SqlRequest>(&hello).unwrap(),
                SqlRequest::Hello { .. }
            ));
            write_frame(
                &mut s,
                &postcard::to_allocvec(&SqlAnswer::Agreed(SQL_VERSION)).unwrap(),
            )
            .unwrap();
            assert!(
                read_frame(
                    &mut s,
                    amphora_api::REQUEST_MAX,
                    Duration::from_secs(30),
                    &memory::Ending::new(),
                )
                .unwrap()
                .is_some(),
                "the request"
            );
            wait.recv().unwrap();
            matches!(
                read_frame(
                    &mut s,
                    amphora_api::REQUEST_MAX,
                    Duration::from_secs(30),
                    &memory::Ending::new(),
                ),
                Ok(None)
            )
        });
        let keys = Keys {
            secret: vec![7; 32],
            tls: None,
        };
        let mut client = SqlClient::connect(&addr, &keys, Duration::from_millis(200)).unwrap();
        assert!(
            client.read(statement("SELECT 1", vec![])).is_err(),
            "an unanswered request"
        );
        let again = client.read(statement("SELECT 1", vec![])).unwrap_err();
        assert!(format!("{again:#}").contains("failed partway"), "{again:#}");
        drop(client);
        done.send(()).unwrap();
        assert!(
            member.join().unwrap(),
            "a second request reached the member"
        );
    }
    const MIB: usize = 1 << 20;

    /// A database of `rows` rows `(id, v)`, each `v` a blob of a MiB but the last, of `last` bytes.
    fn blobs(dir: &Path, rows: i64, last: usize) -> Database {
        let db = Database::open(&dir.join("q.db"), sql_until()).unwrap();
        db.call(sql_until(), crate::database::AnswerMax::fixed(), move |c| {
            c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v BLOB)")?;
            for id in 1..=rows {
                let size = if id == rows { last } else { MIB };
                c.execute(
                    "INSERT INTO t VALUES (?1, ?2)",
                    [Value::Integer(id), Value::Blob(vec![7; size])],
                )?;
            }
            Ok(())
        })
        .unwrap();
        db
    }

    /// A served link and its client's socket, as a `Q` connection's past its proof.
    fn link() -> (Arc<Answering>, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (served, _) = listener.accept().unwrap();
        let answering = Arc::new(Answering {
            socket: served.try_clone().unwrap(),
            link: Mutex::new(BufReader::new(Link::Plain(served))),
            timed: AtomicBool::new(false),
        });
        (answering, client)
    }

    /// A read at version 2 on `answering`, its last frame written as a connection writes it.
    fn serve_read(db: &Database, answering: &Arc<Answering>, until: Instant, sql: &str) {
        let spill = Spill::Sent(Arc::clone(answering), until);
        let read = statement(sql, vec![]);
        let frames = db
            .query_local(until, move |c| answered(c, vec![read], spill))
            .unwrap_or_else(|refusal| Frames::one(refused_frame(refusal).unwrap()));
        let mut link = answering.lock().unwrap();
        for frame in &frames {
            let _ = write_frame(link.get_mut(), &frame.bytes);
        }
    }

    /// CP-T1.13, AC-T1.2.3 (decision 2's bounds): do hand the client an answer of Nulls a byte
    /// each whose rows, decoded, pass the result bound, as one table and as rows ahead of one;
    /// expect both refused while decoding, as the transport's decoders refuse them, and the same
    /// rows within the bound read.
    #[test]
    fn cp_t1_13_a_sql_client_decodes_answers_within_the_result_bound() {
        let past = vec![vec![Value::Null]; amphora_api::RESULT_MAX / 32 + 2];
        let table = |rows: Vec<Vec<Value>>| Table {
            columns: vec!["n".into()],
            rows,
            affected: 0,
            last_insert_rowid: 0,
        };
        for frames in [
            vec![SqlAnswer::Tables(vec![table(past.clone())])],
            vec![
                SqlAnswer::Rows {
                    statement: 0,
                    rows: past,
                },
                SqlAnswer::Tables(vec![table(vec![])]),
            ],
        ] {
            let mut wire = Vec::new();
            for frame in &frames {
                write_frame(&mut wire, &postcard::to_allocvec(frame).unwrap()).unwrap();
            }
            assert!(
                answer(&mut wire.as_slice(), 1, Duration::from_secs(30)).is_err(),
                "{} bytes decoded",
                wire.len()
            );
        }
        let within = vec![vec![Value::Null]; 1000];
        let mut wire = Vec::new();
        write_frame(
            &mut wire,
            &postcard::to_allocvec(&SqlAnswer::Tables(vec![table(within.clone())])).unwrap(),
        )
        .unwrap();
        let SqlAnswer::Tables(tables) =
            answer(&mut wire.as_slice(), 1, Duration::from_secs(30)).unwrap()
        else {
            panic!("not tables");
        };
        assert_eq!(tables[0].rows, within);
    }

    /// CP-T1.13, AC-T1.2.3 (decision 2, version 2): do write a row and read back 40 rows of a MiB
    /// each in one write, an answer past one frame; expect it in more than one frame, each within
    /// the result bound, held until the write commits, read by the client as one table of every
    /// row in order, and the write committed. The call declares only the frame list's fixed size
    /// (`Frames::DECLARED`; lane perf-A11): the frames and the list's slots hold their own charges.
    #[test]
    fn cp_t1_13_a_sql_write_answer_past_one_frame_is_held_in_frames_and_commits() {
        let dir = tempfile::tempdir().unwrap();
        let db = blobs(dir.path(), 40, MIB);
        let frames = db
            .call(
                sql_until(),
                crate::database::AnswerMax::bytes(Frames::DECLARED),
                |c| {
                    answered(
                        c,
                        vec![
                            statement("INSERT INTO t VALUES (41, NULL)", vec![]),
                            statement("SELECT id, v FROM t WHERE id <= 40 ORDER BY id", vec![]),
                        ],
                        Spill::Held(Frames::new()),
                    )
                },
            )
            .unwrap();
        assert!(frames.len() > 1, "{} frames", frames.len());
        assert!(
            frames
                .iter()
                .all(|f| f.bytes.len() <= amphora_api::RESULT_MAX)
        );
        let mut wire = Vec::new();
        for frame in &frames {
            write_frame(&mut wire, &frame.bytes).unwrap();
        }
        let SqlAnswer::Tables(tables) =
            answer(&mut wire.as_slice(), 2, Duration::from_secs(30)).unwrap()
        else {
            panic!("not tables");
        };
        assert_eq!(tables[0].affected, 1);
        let ids: Vec<_> = tables[1].rows.iter().map(|row| row[0].clone()).collect();
        assert_eq!(ids, (1..=40).map(Value::Integer).collect::<Vec<_>>());
        assert!(
            tables[1]
                .rows
                .iter()
                .all(|row| row[1] == Value::Blob(vec![7; MIB]))
        );
        let count = db
            .query_local(sql_until(), |c| {
                Ok(c.query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i64>(0))?)
            })
            .unwrap();
        assert_eq!(count, 41, "the write committed");
    }

    /// CP-T1.13, AC-T1.2.3 (decision 2, version 2): do read 40 rows of a MiB each, then rows whose
    /// last passes one frame by itself, on one link; expect the first answer whole in more than
    /// one frame, the second refused TooLarge after its first rows went out, and a third read on
    /// the same link answered: the connection goes on.
    #[test]
    fn cp_t1_13_a_sql_read_streams_past_one_frame_and_a_refusal_midway_ends_its_answer() {
        let dir = tempfile::tempdir().unwrap();
        let db = blobs(dir.path(), 40, amphora_api::RESULT_MAX / 2 + 1);
        let (answering, client) = link();
        let reader = std::thread::spawn(move || {
            let mut r = BufReader::new(client);
            let mut answers = Vec::new();
            for _ in 0..3 {
                answers.push(answer(&mut r, 1, Duration::from_secs(30)).unwrap());
            }
            answers
        });
        let until = sql_until();
        serve_read(
            &db,
            &answering,
            until,
            "SELECT id, v FROM t WHERE id < 40 ORDER BY id",
        );
        serve_read(&db, &answering, until, "SELECT id, v, v FROM t ORDER BY id");
        serve_read(&db, &answering, until, "SELECT count(*) FROM t");
        let answers = reader.join().unwrap();
        let SqlAnswer::Tables(first) = &answers[0] else {
            panic!("{:?}", answers[0]);
        };
        let ids: Vec<_> = first[0].rows.iter().map(|row| row[0].clone()).collect();
        assert_eq!(ids, (1..40).map(Value::Integer).collect::<Vec<_>>());
        assert!(
            matches!(answers[1], SqlAnswer::Refused(Refusal::TooLarge { .. })),
            "{:?}",
            answers[1]
        );
        let SqlAnswer::Tables(third) = &answers[2] else {
            panic!("{:?}", answers[2]);
        };
        assert_eq!(third[0].rows, vec![vec![Value::Integer(40)]]);
    }

    /// CP-T1.13, AC-T1.2.3 (decision 2: a streamed read never runs on the writer): do hold every
    /// reader with a session, stream a 40 MiB read to a client that doesn't read yet, write, let
    /// the readers go, then write again while the read is held on its client; expect both writes
    /// committed within their own deadlines (the read waits for a reader, and a read held on its
    /// client on the writer would hold every write back past them) and the read answered whole
    /// once its client reads.
    #[test]
    fn cp_t1_13_a_streamed_read_waits_for_a_reader_and_never_holds_a_write_back() {
        let dir = tempfile::tempdir().unwrap();
        let db = blobs(dir.path(), 40, MIB);
        let (entered_tx, entered) = std::sync::mpsc::channel();
        let mut releases = Vec::new();
        let mut parked = Vec::new();
        for _ in 0..crate::store::Config::default().readers {
            let (release, wait) = std::sync::mpsc::channel::<()>();
            releases.push(release);
            let (db, entered_tx) = (db.clone(), entered_tx.clone());
            parked.push(std::thread::spawn(move || {
                db.query_local(sql_until(), move |_| {
                    entered_tx.send(()).unwrap();
                    wait.recv().unwrap();
                    Ok(())
                })
            }));
        }
        for _ in &parked {
            entered.recv().unwrap();
        }
        let (answering, mut client) = link();
        // Past every write's deadline here: a read held on the writer would refuse the writes
        // Timeout rather than end before them.
        let reading = Instant::now() + Duration::from_secs(120);
        let streamed = {
            let (db, answering) = (db.clone(), Arc::clone(&answering));
            std::thread::spawn(move || {
                serve_read(&db, &answering, reading, "SELECT id, v FROM t ORDER BY id")
            })
        };
        let write = |id: i64| {
            db.call(sql_until(), crate::database::AnswerMax::fixed(), move |c| {
                c.execute(
                    "INSERT INTO t VALUES (?1, ?2)",
                    [Value::Integer(id), Value::Blob(vec![])],
                )?;
                Ok(())
            })
        };
        let wrote = write(41);
        assert!(
            wrote.is_ok(),
            "a write while every reader is busy and a read waits: {wrote:?}"
        );
        for release in releases {
            release.send(()).unwrap();
        }
        for p in parked {
            p.join().unwrap().unwrap();
        }
        client.peek(&mut [0u8; 1]).unwrap();
        let wrote = write(42);
        assert!(
            wrote.is_ok(),
            "a write while a read is held on its client: {wrote:?}"
        );
        let got = answer(&mut BufReader::new(&mut client), 1, Duration::from_secs(30)).unwrap();
        streamed.join().unwrap();
        let SqlAnswer::Tables(tables) = got else {
            panic!("{got:?}");
        };
        let ids: Vec<_> = tables[0].rows.iter().map(|row| row[0].clone()).collect();
        assert_eq!(
            ids,
            (1..=41).map(Value::Integer).collect::<Vec<_>>(),
            "whole, read once the first write committed"
        );
    }

    /// CP-T1.13, AC-T1.2.3 (decision 2, version 2): do read an answer of 40 MiB through a link
    /// whose client never reads; expect the read refused Timeout at its deadline, never TooLarge
    /// (its frames go out one at a time, none held past the one waiting on the client), and the
    /// link ended: the client reads less than the answer, then the end.
    #[test]
    fn cp_t1_13_a_sql_read_to_a_client_that_stops_reading_ends_at_its_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let db = blobs(dir.path(), 40, MIB);
        let (answering, mut client) = link();
        let until = Instant::now() + Duration::from_secs(2);
        let spill = Spill::Sent(Arc::clone(&answering), until);
        let read = db.query_local(until, move |c| {
            answered(
                c,
                vec![statement("SELECT id, v FROM t ORDER BY id", vec![])],
                spill,
            )
        });
        assert!(
            matches!(read, Err(Refusal::Timeout)),
            "{:?}",
            read.map(|frames| frames.len())
        );
        let mut got = Vec::new();
        let _ = client.read_to_end(&mut got);
        assert!(
            got.len() <= amphora_api::RESULT_MAX + 4,
            "{} bytes",
            got.len()
        );
    }
}
