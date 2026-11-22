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
//!   `count SQL`, and `cut on|off` where the member's config says `drills = on`).
//! - `amphora drill load|check`: the fire drills' writers and their loss count ([`load`], [`check`]).
//!
//! The config, one per member (`key = value` lines, `#` comments; every key below but `drills` and
//! `spanning` required, none defaulted, an unknown key refused):
//!
//! ```text
//! node = 1                                   # this member's ID
//! members = 1@a:7401,2@b:7401,3@c:7401       # every member's ID and address, as its peers reach it
//! data = /home/me/.local/share/amphora/1     # this member's database and log
//! class = persisted                          # or replicated (then spanning = N)
//! secret_file = /home/me/.config/amphora/secret
//! deadline_ms = 5000                         # how long one request, connect or open may take
//! drills = off                               # on: the member takes `cut` (a split at its transport)
//! ```
//!
//! A connection's first byte says what it carries: `R` (a peer's Raft records) or `C` (a client's
//! requests). The member answers with a challenge (32 random bytes) and the other side proves it
//! holds the cluster's secret with HMAC-SHA256(secret, challenge || kind) (RFC 2104); the secret
//! never crosses the link. The link itself is not encrypted (docs/runbook.md, "What isn't done").
//! Then `R`: the sender's node ID (u64, little endian) and records, each its length (u32, little
//! endian, at most the replica's own record bound) and its bytes; `C`: requests, one line each,
//! each answered by one line, `ok …` or `err …`, a line at most the API's request bound.
//!
//! Bounds: one records connection per peer (a new one from the same peer closes the old); each
//! client connection takes a descriptor from the process's budget ([`crate::descriptors`]) and is
//! refused when there is none; each peer's queue holds at most the replica's own outgoing bound, past
//! which records are lost as a congested link loses them, and Raft sends again what it needs.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use amphora_api::replication::{Durability, Fates, MemberChange, ReplicaConfig};
use anyhow::{anyhow, bail, ensure, Context};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::database::Database;

/// The records a peer's sender holds and one drain takes: the 4096 that
/// `replicated::OUTGOING_MESSAGES` was until QB.1 (lanes constants) bounded the outgoing queue by
/// its learned drain. Still picked: CP-T1.13's to derive from that bound (lane constants-20).
const OUTGOING_MESSAGES: usize = 4096;

const RECORDS: u8 = b'R';
const CLIENT: u8 = b'C';
/// The challenge's length: SHA-256's output length (FIPS 180-4), HMAC-SHA256's block of proof.
const NONCE: usize = 32;
/// HMAC-SHA256's tag length (RFC 2104 with SHA-256: 32 bytes).
const TAG: usize = 32;
/// The shortest secret taken: RFC 2104 §3 asks a key of at least the hash's output length.
const SECRET_MIN: usize = 32;
/// A request or answer line's most bytes: the API's request bound.
const LINE_MAX: usize = amphora_api::REQUEST_MAX;

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
}

const KEYS: [&str; 9] = [
    "node",
    "members",
    "data",
    "class",
    "spanning",
    "secret_file",
    "deadline_ms",
    "drills",
    "voters",
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
        Ok(Self {
            node,
            members,
            data: PathBuf::from(take("data")?),
            class,
            secret_file: PathBuf::from(take("secret_file")?),
            deadline: Duration::from_millis(deadline_ms),
            drills,
            voters,
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
    secret: Vec<u8>,
    drills: bool,
    /// Each peer's records connection, to close it when the peer opens another.
    links: Mutex<BTreeMap<u64, TcpStream>>,
}

/// Runs the member `config` describes until its replica stops; `ready` is told its address once
/// it listens.
pub fn serve(config: &Config, ready: impl FnOnce(&str)) -> anyhow::Result<()> {
    let secret = config.secret()?;
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
        secret,
        drills: config.drills,
        links: Mutex::new(BTreeMap::new()),
    });
    let mut peers: BTreeMap<u64, mpsc::SyncSender<Vec<u8>>> = BTreeMap::new();
    for m in config.members.iter().filter(|m| m.node != config.node) {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(OUTGOING_MESSAGES);
        let (addr, node, secret, deadline) = (
            m.addr.clone(),
            config.node,
            shared.secret.clone(),
            config.deadline,
        );
        std::thread::Builder::new()
            .name(format!("amphora-peer-{}", m.node))
            .spawn(move || to_peer(node, &addr, &secret, deadline, &rx))?;
        peers.insert(m.node, tx);
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
            if let Some(tx) = peers.get(&record.to) {
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

/// One connection, as its first byte says, once it has proved the secret.
fn connection(shared: &Shared, s: TcpStream) -> anyhow::Result<()> {
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(shared.deadline))?;
    s.set_write_timeout(Some(shared.deadline))?;
    let mut w = s.try_clone()?;
    let mut r = BufReader::new(s);
    let mut kind = [0u8; 1];
    r.read_exact(&mut kind)?;
    let mut nonce = [0u8; NONCE];
    getrandom::fill(&mut nonce).map_err(|e| anyhow!("no randomness for a challenge: {e}"))?;
    w.write_all(&nonce)?;
    let mut tag = [0u8; TAG];
    r.read_exact(&mut tag)?;
    let mut check = keyed(&shared.secret)?;
    check.update(&nonce);
    check.update(&kind);
    check
        .verify_slice(&tag)
        .map_err(|_| anyhow!("a connection without the cluster's secret"))?;
    // Past the proof, a link waits as long as its other side is quiet: a peer between heartbeats, a
    // pooled client between requests. A dead peer's link is closed when it opens its next.
    r.get_ref().set_read_timeout(None)?;
    match kind {
        [RECORDS] => {
            let mut from = [0u8; 8];
            r.read_exact(&mut from)?;
            let from = u64::from_le_bytes(from);
            ensure!(
                from != shared.node && shared.ids.contains(&from),
                "records from {from}, not a peer"
            );
            let link = w.try_clone()?;
            if let Some(old) = shared
                .links
                .lock()
                .map_err(|_| anyhow!("links poisoned"))?
                .insert(from, link)
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
            w.write_all(answer.as_bytes())?;
        },
        _ => bail!("a connection opened with {kind:?}"),
    }
}

/// One line of at most [`LINE_MAX`] bytes, its end dropped; `None` at the end of the stream.
fn read_line(r: &mut BufReader<TcpStream>) -> anyhow::Result<Option<String>> {
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
                    .call(until, move |c| {
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
            other => bail!("unknown request {other:?} (status, exec, count, cut, member)"),
        }
    }
}

/// A peer's records, in the order the member sent them, over one connection opened when the first
/// comes and again after a failure: a record that can't be written is lost, as a network loses it.
fn to_peer(
    node: u64,
    addr: &str,
    secret: &[u8],
    deadline: Duration,
    records: &mpsc::Receiver<Vec<u8>>,
) {
    let mut conn: Option<TcpStream> = None;
    for bytes in records {
        if conn.is_none() {
            conn = open(addr, secret, deadline, RECORDS)
                .and_then(|(_, mut w)| {
                    w.write_all(&node.to_le_bytes())?;
                    Ok(w)
                })
                .ok();
        }
        let Some(s) = conn.as_mut() else { continue };
        let Ok(len) = u32::try_from(bytes.len()) else {
            continue;
        };
        if s.write_all(&len.to_le_bytes())
            .and_then(|()| s.write_all(&bytes))
            .is_err()
        {
            conn = None;
        }
    }
}

/// A connection to `addr` of `kind`, the secret proved.
fn open(
    addr: &str,
    secret: &[u8],
    deadline: Duration,
    kind: u8,
) -> anyhow::Result<(BufReader<TcpStream>, TcpStream)> {
    let s = TcpStream::connect_timeout(&resolve(addr)?, deadline)
        .with_context(|| format!("connecting to {addr}"))?;
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(deadline))?;
    s.set_write_timeout(Some(deadline))?;
    let mut w = s.try_clone()?;
    let mut r = BufReader::new(s);
    w.write_all(&[kind])?;
    let mut nonce = [0u8; NONCE];
    r.read_exact(&mut nonce)?;
    let mut tag = keyed(secret)?;
    tag.update(&nonce);
    tag.update(&[kind]);
    w.write_all(&tag.finalize().into_bytes())?;
    Ok((r, w))
}

/// A client of one member: requests one line each, answered in order.
pub struct Client {
    r: BufReader<TcpStream>,
    w: TcpStream,
}

impl Client {
    pub fn connect(addr: &str, secret: &[u8], deadline: Duration) -> anyhow::Result<Self> {
        let (r, w) = open(addr, secret, deadline, CLIENT)?;
        Ok(Self { r, w })
    }

    /// The member's answer to `request`, past its `ok`; its `err` as the error.
    pub fn ask(&mut self, request: &str) -> anyhow::Result<String> {
        ensure!(!request.contains(['\n', '\r']), "a request is one line");
        let mut line = request.to_owned();
        line.push('\n');
        self.w.write_all(line.as_bytes())?;
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
    let secret = config.secret()?;
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
        match Client::connect(&m.addr, &secret, config.deadline).and_then(|mut c| c.ask("status")) {
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
    let secret = Arc::new(config.secret()?);
    let addrs: Arc<Vec<String>> = Arc::new(config.members.iter().map(|m| m.addr.clone()).collect());
    let made = after(config.deadline)?;
    let mut table_made = false;
    while !table_made && Instant::now() < made {
        for a in addrs.iter() {
            if Client::connect(a, &secret, config.deadline)
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
        let (secret, addrs, retries, run, deadline) = (
            Arc::clone(&secret),
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
                    conn = addrs.get(via).and_then(|a| Client::connect(a, &secret, deadline).ok());
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
                        via = via.saturating_add(1).checked_rem(n).unwrap_or(0);
                        retries.fetch_add(1, Ordering::Relaxed);
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
    let secret = config.secret()?;
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
                    client = Client::connect(&m.addr, &secret, config.deadline).ok();
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
            let answer =
                Client::connect(&m.addr, &c.secret()?, c.deadline)?.ask(&request.join(" "))?;
            Ok((format!("{answer}\n"), true))
        })()),
        // The change goes to each voter in turn until one, the leader, makes it: a follower
        // refuses it, and a lost member doesn't answer, so a stale idea of the leader costs nothing.
        ("member", [config, what, node]) => reply((|| {
            let c = Config::load(Path::new(config))?;
            let secret = c.secret()?;
            let request = format!("member {what} {node}");
            let mut refused = Vec::new();
            for m in c.members.iter().filter(|m| c.voters.contains(&m.node)) {
                match Client::connect(&m.addr, &secret, c.deadline)
                    .and_then(|mut k| k.ask(&request))
                {
                    Ok(answer) => {
                        return Ok((
                            format!(
                                "{answer}
"
                            ),
                            true,
                        ))
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
        ] {
            let e = format!("{:#}", Config::parse(&text).err().unwrap());
            assert!(e.contains(why), "{e} lacks {why}");
        }
        assert!(
            Config::parse(&format!("{GOOD}drills = on\n"))
                .unwrap()
                .drills
        );
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
}
