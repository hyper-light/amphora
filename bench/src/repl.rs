//! Replication measured (CP-T1.8; kiln plan "G12 Replication", steps 1 and 3): Amphora's commit
//! classes, failover and catch-up, and the same three measures on dqlite, rqlite, LiteFS and
//! libSQL, each a three-node cluster, on the same workload: kiln's event upserts (K0's generator,
//! `crate::event`), `--batch` events a write, one write outstanding per writer.
//!
//! - `commit` (AC-T1.8.2, AC-T1.8.3): `--writers` closed-loop writers for `--seconds` after
//!   `--warmup-s`; commit latency p50/p99/p99.9 and throughput.
//! - `failover` (AC-T1.8.4; step 3's "kill -9 of the leader 100 times under load"): writers run
//!   throughout; `--kills` times the leader is killed and a probe writes through the survivors until
//!   one write is acknowledged: the time from the kill to that acknowledgement. Every writer resends
//!   a refused write unchanged (same ids), so every sequence number up to a writer's last
//!   acknowledged one is acknowledged; at the end each writer's acknowledged events are counted on
//!   the leader: `lost` must be 0 (AC-T1.8.5's lab half).
//! - `catchup`: a follower is killed, `--behind` events are written, it is started again, and its
//!   own local reads are polled every `--poll-us` until it holds every row the leader held: the time
//!   to rejoin.
//!
//! Every size, duration and interval is an input with no default: nothing is picked here.
//!
//! Amphora's three members run in this process, relayed by the host's part as `cluster.rs` does; a
//! killed member is cut off from the relay, which its peers can't tell from kill -9 (nothing more
//! comes from it, nothing reaches it). The competitors run as containers through their scripts,
//! `bench/servers/repl/<system>.sh up|down|kill N|start N|leader|endpoint N` (a kill is
//! `docker kill`, SIGKILL). A system without a measure's concept, or one whose cluster can't be
//! brought up here, is refused, typed, and the refusal is recorded beside the measures.
//!
//! usage: amphora-bench --repl <commit|failover|catchup> --system <amphora|rqlite|dqlite|litefs|libsql>
//!        --batch N --text-bytes N --deadline-s N [--record FILE]
//!        commit:   --writers N --warmup-s N --seconds N
//!        failover: --writers N --kills N --settle-s N
//!        catchup:  --behind N --poll-us N
//!        amphora:  --dir DIR --class persisted|replicated [--spanning N] --relay-records N (records the relay
//!                  moves from a member at a time)
//!        others:   [--scripts DIR (bench/servers/repl)] [--shell PROGRAM (bash)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{Event, Hist, Rng, summary};

/// A statement's argument, as every system's driver turns it into its own.
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Text(String),
    Int(i64),
    Null,
}

/// The arguments of [`crate::schema::EVENT_UPSERT`] for `e`, in its column order.
fn event_args(e: &Event) -> anyhow::Result<Vec<Arg>> {
    Ok(vec![
        Arg::Text(e.session.clone()),
        Arg::Text(e.id.clone()),
        Arg::Int(i64::try_from(e.seq)?),
        Arg::Int(e.at),
        Arg::Null,
        Arg::Null,
        Arg::Text(e.kind.into()),
        Arg::Null,
        Arg::Null,
        Arg::Null,
        Arg::Text(e.text.clone()),
        Arg::Text(e.body.clone()),
        Arg::Int(0),
    ])
}

/// What a system can't be asked, and why: recorded beside the measures, never skipped silently.
#[derive(Debug)]
pub enum Refusal {
    /// The system has no such concept (libSQL's sqld has one primary and no election).
    NoConcept(String),
    /// The system's cluster couldn't be brought up on this machine: what's needed is in the reason.
    Unavailable(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConcept(why) => write!(f, "no-concept: {why}"),
            Self::Unavailable(why) => write!(f, "unavailable: {why}"),
        }
    }
}

impl std::error::Error for Refusal {}

/// A three-node replicated cluster, nodes 0 to 2.
pub trait Cluster: Sync {
    /// `stmts` in one transaction through node `via`: returns once the system acknowledges it.
    fn exec(&self, via: usize, stmts: &[(String, Vec<Arg>)], until: Instant) -> anyhow::Result<()>;
    /// A count read on node `on` from its own copy (catch-up and the loss check read this).
    fn count(&self, on: usize, sql: &str, args: Vec<Arg>, until: Instant) -> anyhow::Result<u64>;
    /// The node the cluster names its leader now, if it names one.
    fn leader(&self, until: Instant) -> anyhow::Result<Option<usize>>;
    /// Kills node `n` at once (SIGKILL, or cut off for Amphora's in-process members).
    fn kill(&self, n: usize) -> anyhow::Result<()>;
    /// Starts node `n` again on its data.
    fn start(&self, n: usize) -> anyhow::Result<()>;
    /// Why the system has no failover, if it has none.
    fn no_failover(&self) -> Option<String> {
        None
    }
    /// Why a member's catch-up can't be read on it, if it can't.
    fn no_catch_up(&self) -> Option<String> {
        None
    }
}

pub const NODES: usize = 3;

struct Opts {
    measure: String,
    system: String,
    batch: usize,
    text_bytes: usize,
    deadline: Duration,
    record: Option<PathBuf>,
    writers: Option<usize>,
    warmup: Option<Duration>,
    seconds: Option<Duration>,
    kills: Option<usize>,
    settle: Option<Duration>,
    behind: Option<usize>,
    poll: Option<Duration>,
    dir: Option<PathBuf>,
    class: Option<String>,
    spanning: Option<u32>,
    relay_records: Option<usize>,
    scripts: PathBuf,
    shell: String,
}

fn need<T>(v: Option<T>, flag: &str, measure: &str) -> anyhow::Result<T> {
    v.ok_or_else(|| anyhow::anyhow!("{measure} takes {flag}: every size and duration is an input"))
}

fn parse(args: &[String]) -> anyhow::Result<Opts> {
    let mut measure = None;
    let (mut system, mut batch, mut text_bytes, mut deadline) = (None, None, None, None);
    let mut o = Opts {
        measure: String::new(),
        system: String::new(),
        batch: 0,
        text_bytes: 0,
        deadline: Duration::ZERO,
        record: None,
        writers: None,
        warmup: None,
        seconds: None,
        kills: None,
        settle: None,
        behind: None,
        poll: None,
        dir: None,
        class: None,
        spanning: None,
        relay_records: None,
        scripts: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/servers/repl")),
        shell: "bash".into(),
    };
    let mut it = args.iter();
    let secs = |v: &str| -> anyhow::Result<Duration> { Ok(Duration::try_from_secs_f64(v.parse::<f64>()?)?) };
    while let Some(a) = it.next() {
        let mut v = || it.next().cloned().ok_or_else(|| anyhow::anyhow!("{a} takes a value"));
        match a.as_str() {
            "--repl" => measure = Some(v()?),
            "--system" => system = Some(v()?),
            "--batch" => batch = Some(v()?.parse::<usize>()?),
            "--text-bytes" => text_bytes = Some(v()?.parse::<usize>()?),
            "--deadline-s" => deadline = Some(secs(&v()?)?),
            "--record" => o.record = Some(PathBuf::from(v()?)),
            "--writers" => o.writers = Some(v()?.parse()?),
            "--warmup-s" => o.warmup = Some(secs(&v()?)?),
            "--seconds" => o.seconds = Some(secs(&v()?)?),
            "--kills" => o.kills = Some(v()?.parse()?),
            "--settle-s" => o.settle = Some(secs(&v()?)?),
            "--behind" => o.behind = Some(v()?.parse()?),
            "--poll-us" => o.poll = Some(Duration::from_micros(v()?.parse()?)),
            "--dir" => o.dir = Some(PathBuf::from(v()?)),
            "--class" => o.class = Some(v()?),
            "--spanning" => o.spanning = Some(v()?.parse()?),
            "--relay-records" => o.relay_records = Some(v()?.parse()?),
            "--scripts" => o.scripts = PathBuf::from(v()?),
            "--shell" => o.shell = v()?,
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    o.measure = measure.ok_or_else(|| anyhow::anyhow!("--repl names the measure: commit, failover or catchup"))?;
    let m = o.measure.clone();
    o.system = need(system, "--system", &m)?;
    o.batch = need(batch, "--batch", &m)?;
    o.text_bytes = need(text_bytes, "--text-bytes", &m)?;
    o.deadline = need(deadline, "--deadline-s", &m)?;
    anyhow::ensure!(o.batch > 0, "--batch takes at least 1");
    match m.as_str() {
        "commit" => {
            anyhow::ensure!(need(o.writers, "--writers", &m)? > 0, "--writers takes at least 1");
            need(o.warmup, "--warmup-s", &m)?;
            need(o.seconds, "--seconds", &m)?;
        }
        "failover" => {
            anyhow::ensure!(need(o.writers, "--writers", &m)? > 0, "--writers takes at least 1");
            need(o.kills, "--kills", &m)?;
            need(o.settle, "--settle-s", &m)?;
        }
        "catchup" => {
            need(o.behind, "--behind", &m)?;
            need(o.poll, "--poll-us", &m)?;
        }
        other => anyhow::bail!("unknown measure {other} (commit, failover, catchup)"),
    }
    if o.system == "amphora" {
        need(o.dir.as_ref(), "--dir", &m)?;
        anyhow::ensure!(need(o.relay_records, "--relay-records", &m)? > 0, "--relay-records takes at least 1");
        match need(o.class.as_deref(), "--class", &m)? {
            "persisted" => {}
            "replicated" => anyhow::ensure!(need(o.spanning, "--spanning", &m)? >= 2, "--spanning takes at least 2"),
            other => anyhow::bail!("unknown class {other} (persisted, replicated)"),
        }
    }
    Ok(o)
}

fn us(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// One write of `events` as statements.
fn stmts(events: &[Event]) -> anyhow::Result<Vec<(String, Vec<Arg>)>> {
    events.iter().map(|e| Ok((crate::schema::EVENT_UPSERT.to_owned(), event_args(e)?))).collect()
}

/// The schema through any node that takes it before the deadline.
fn schema(c: &dyn Cluster, until: Instant) -> anyhow::Result<()> {
    let s: Vec<(String, Vec<Arg>)> = crate::schema::SCHEMA.iter().map(|s| ((*s).to_owned(), Vec::new())).collect();
    let mut last = None;
    let mut via = 0;
    while Instant::now() < until {
        match c.exec(via % NODES, &s, until) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        via += 1;
    }
    Err(anyhow::anyhow!("no node took the schema before the deadline: {last:?}"))
}

/// A writer's events: session `writer`, sequence numbers from 1, `batch` a write.
struct Writer {
    rng: Rng,
    session: usize,
    seq: u64,
}

impl Writer {
    fn new(session: usize) -> Self {
        Self { rng: Rng(0x9e37_79b9_7f4a_7c15 ^ (session as u64).wrapping_add(1)), session, seq: 0 }
    }

    fn next(&mut self, batch: usize, text_bytes: usize) -> Vec<Event> {
        (0..batch)
            .map(|_| {
                self.seq += 1;
                crate::event(&mut self.rng, self.session, self.seq, text_bytes)
            })
            .collect()
    }
}

/// Sends `events` through the live nodes, starting at `via`, until one acknowledges them or the
/// deadline passes: the same ids every time, so a resend is the same write.
fn send(c: &dyn Cluster, down: &[AtomicBool; NODES], via: usize, s: &[(String, Vec<Arg>)], deadline: Duration) -> anyhow::Result<usize> {
    let until = Instant::now() + deadline;
    let mut n = via;
    let mut last = None;
    while Instant::now() < until {
        let node = n % NODES;
        n += 1;
        if down.get(node).is_some_and(|d| d.load(Ordering::Relaxed)) {
            continue;
        }
        match c.exec(node, s, until) {
            Ok(()) => return Ok(node),
            Err(e) => last = Some(e),
        }
    }
    Err(anyhow::anyhow!("no live node acknowledged a write within --deadline-s: {last:?}"))
}

/// The record's common fields: what ran, where, when, at which commit.
fn record_head(o: &Opts, c: &str) -> serde_json::Value {
    let host = std::env::var("HOSTNAME").or_else(|_| std::env::var("COMPUTERNAME")).unwrap_or_default();
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_default();
    let load = std::fs::read_to_string("/proc/loadavg").ok().and_then(|s| s.split_whitespace().next().map(str::to_owned));
    let date = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    serde_json::json!({
        "bench": format!("repl_{}", o.measure), "system": c, "nodes": NODES, "batch": o.batch, "text_bytes": o.text_bytes,
        "host": host, "cpus": std::thread::available_parallelism().map_or(0, usize::from), "commit": commit, "unix_s": date, "load1": load,
    })
}

fn emit(o: &Opts, mut head: serde_json::Value, body: serde_json::Value) -> anyhow::Result<()> {
    if let (Some(h), Some(b)) = (head.as_object_mut(), body.as_object()) {
        for (k, v) in b {
            h.insert(k.clone(), v.clone());
        }
    }
    let line = head.to_string();
    println!("{line}");
    if let Some(path) = &o.record {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(f, "{line}")?;
    }
    Ok(())
}

fn commit(c: &dyn Cluster, o: &Opts) -> anyhow::Result<serde_json::Value> {
    let writers = need(o.writers, "--writers", "commit")?;
    let (warmup, seconds) = (need(o.warmup, "--warmup-s", "commit")?, need(o.seconds, "--seconds", "commit")?);
    let down: [AtomicBool; NODES] = Default::default();
    let began = Instant::now();
    let (counting, ends) = (began + warmup, began + warmup + seconds);
    let hists = std::thread::scope(|scope| -> anyhow::Result<Vec<(Hist, u64)>> {
        let handles: Vec<_> = (0..writers)
            .map(|w| {
                let down = &down;
                scope.spawn(move || -> anyhow::Result<(Hist, u64)> {
                    let mut wr = Writer::new(w);
                    let (mut h, mut writes) = (Hist::new(), 0u64);
                    loop {
                        let t = Instant::now();
                        if t >= ends {
                            return Ok((h, writes));
                        }
                        let s = stmts(&wr.next(o.batch, o.text_bytes))?;
                        send(c, down, w, &s, o.deadline)?;
                        if t >= counting {
                            h.record(us(t));
                            writes += 1;
                        }
                    }
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().map_err(|_| anyhow::anyhow!("a writer panicked"))?).collect()
    })?;
    let mut all = Hist::new();
    let mut writes = 0u64;
    for (h, n) in &hists {
        all.merge(h);
        writes += n;
    }
    let secs = seconds.as_secs_f64();
    Ok(serde_json::json!({
        "writers": writers, "warmup_s": warmup.as_secs_f64(), "seconds": secs, "latency_ms": summary(all),
        "writes_per_s": writes as f64 / secs, "events_per_s": (writes as f64) * (o.batch as f64) / secs,
    }))
}

/// The leader the cluster names, waiting for one until the deadline.
fn leader_by(c: &dyn Cluster, deadline: Duration, poll: Duration) -> anyhow::Result<usize> {
    let until = Instant::now() + deadline;
    loop {
        if let Some(l) = c.leader(until)? {
            return Ok(l);
        }
        anyhow::ensure!(Instant::now() < until, "no leader within --deadline-s");
        std::thread::park_timeout(poll);
    }
}

fn failover(c: &dyn Cluster, o: &Opts) -> anyhow::Result<serde_json::Value> {
    if let Some(why) = c.no_failover() {
        return Err(Refusal::NoConcept(why).into());
    }
    let writers = need(o.writers, "--writers", "failover")?;
    let (kills, settle) = (need(o.kills, "--kills", "failover")?, need(o.settle, "--settle-s", "failover")?);
    let down: [AtomicBool; NODES] = Default::default();
    let stop = AtomicBool::new(false);
    // Each writer's last acknowledged sequence number: everything up to it is acknowledged.
    let acked: Vec<AtomicU64> = (0..writers).map(|_| AtomicU64::new(0)).collect();
    let mut times = Hist::new();
    let mut each_ms = Vec::new();
    let failed: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    std::thread::scope(|scope| -> anyhow::Result<()> {
        for w in 0..writers {
            let (down, stop, acked, failed) = (&down, &stop, &acked, &failed);
            scope.spawn(move || {
                let mut wr = Writer::new(w);
                while !stop.load(Ordering::Relaxed) {
                    let events = wr.next(o.batch, o.text_bytes);
                    let r = stmts(&events).and_then(|s| send(c, down, w, &s, o.deadline));
                    match r {
                        Ok(_) => {
                            if let Some(a) = acked.get(w) {
                                a.store(wr.seq, Ordering::Relaxed);
                            }
                        }
                        Err(e) => {
                            if let Ok(mut f) = failed.lock() {
                                f.get_or_insert(e);
                            }
                            return;
                        }
                    }
                }
            });
        }
        let ran = (|| -> anyhow::Result<()> {
            // The probe's own session, after the writers'.
            let mut probe = Writer::new(writers);
            for _ in 0..kills {
                std::thread::park_timeout(settle);
                let leader = leader_by(c, o.deadline, settle)?;
                let s = stmts(&probe.next(o.batch, o.text_bytes))?;
                let at = Instant::now();
                down.get(leader).ok_or_else(|| anyhow::anyhow!("no node {leader}"))?.store(true, Ordering::Relaxed);
                c.kill(leader)?;
                send(c, &down, leader + 1, &s, o.deadline)?;
                let t = us(at);
                times.record(t);
                each_ms.push(t as f64 / 1000.0);
                c.start(leader)?;
                down.get(leader).ok_or_else(|| anyhow::anyhow!("no node {leader}"))?.store(false, Ordering::Relaxed);
            }
            std::thread::park_timeout(settle);
            Ok(())
        })();
        stop.store(true, Ordering::Relaxed);
        ran
    })?;
    if let Some(e) = failed.lock().map_err(|_| anyhow::anyhow!("a writer's failure was poisoned"))?.take() {
        return Err(e);
    }
    // Every acknowledged event, counted on the leader.
    let until = Instant::now() + o.deadline;
    let leader = leader_by(c, o.deadline, settle)?;
    let (mut acknowledged, mut lost) = (0u64, 0u64);
    for (w, a) in acked.iter().enumerate() {
        let want = a.load(Ordering::Relaxed);
        let session = Writer::new(w).session;
        let held = c.count(leader, "SELECT count(*) FROM events WHERE session = ?1 AND seq <= ?2", vec![Arg::Text(format!("s-{session:06}")), Arg::Int(i64::try_from(want)?)], until)?;
        acknowledged += want;
        lost += want.saturating_sub(held);
    }
    Ok(serde_json::json!({
        "writers": writers, "kills": kills, "settle_s": settle.as_secs_f64(), "failover_ms": summary(times), "each_ms": each_ms,
        "acknowledged_events": acknowledged, "lost": lost,
    }))
}

fn catchup(c: &dyn Cluster, o: &Opts) -> anyhow::Result<serde_json::Value> {
    if let Some(why) = c.no_catch_up() {
        return Err(Refusal::NoConcept(why).into());
    }
    let (behind, poll) = (need(o.behind, "--behind", "catchup")?, need(o.poll, "--poll-us", "catchup")?);
    let down: [AtomicBool; NODES] = Default::default();
    let leader = leader_by(c, o.deadline, poll)?;
    let member = (leader + 1) % NODES;
    down.get(member).ok_or_else(|| anyhow::anyhow!("no node {member}"))?.store(true, Ordering::Relaxed);
    c.kill(member)?;
    let mut wr = Writer::new(0);
    let mut left = behind;
    while left > 0 {
        let n = left.min(o.batch);
        send(c, &down, leader, &stmts(&wr.next(n, o.text_bytes))?, o.deadline)?;
        left -= n;
    }
    let until = Instant::now() + o.deadline;
    let target = c.count(leader, "SELECT count(*) FROM events", Vec::new(), until)?;
    let at = Instant::now();
    c.start(member)?;
    loop {
        // A member not yet answering is a member not yet caught up.
        if c.count(member, "SELECT count(*) FROM events", Vec::new(), until).is_ok_and(|n| n >= target) {
            break;
        }
        anyhow::ensure!(Instant::now() < until, "node {member} didn't catch up within --deadline-s");
        std::thread::park_timeout(poll);
    }
    let t = us(at);
    Ok(serde_json::json!({ "behind": behind, "rows": target, "poll_us": poll.as_micros() as u64, "catchup_ms": t as f64 / 1000.0 }))
}

fn open(o: &Opts) -> anyhow::Result<Box<dyn Cluster>> {
    Ok(match o.system.as_str() {
        "amphora" => Box::new(inproc::Amphora::open(o)?),
        "rqlite" | "dqlite" | "litefs" | "libsql" => Box::new(http::Http::up(&o.system, &o.scripts, &o.shell, o.deadline)?),
        other => anyhow::bail!("unknown system {other} (amphora, rqlite, dqlite, litefs, libsql)"),
    })
}

pub fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = parse(&args)?;
    let label = match (&o.system[..], &o.class) {
        ("amphora", Some(class)) => format!("amphora-{class}"),
        (s, _) => s.to_owned(),
    };
    let ran = open(&o).and_then(|c| {
        let until = Instant::now() + o.deadline;
        schema(c.as_ref(), until)?;
        match o.measure.as_str() {
            "commit" => commit(c.as_ref(), &o),
            "failover" => failover(c.as_ref(), &o),
            _ => catchup(c.as_ref(), &o),
        }
    });
    let head = record_head(&o, &label);
    match ran {
        Ok(body) => emit(&o, head, body),
        Err(e) => match e.downcast_ref::<Refusal>() {
            Some(r) => {
                let (kind, why) = match r {
                    Refusal::NoConcept(w) => ("no-concept", w.clone()),
                    Refusal::Unavailable(w) => ("unavailable", w.clone()),
                };
                emit(&o, head, serde_json::json!({ "refused": kind, "why": why }))?;
                Err(e)
            }
            None => Err(e),
        },
    }
}

/// Amphora's three members in this process (`cluster.rs`'s relay), a kill a cut from the relay.
mod inproc {
    use super::*;
    use amphora::database::Database;
    use amphora_api::replication::{Durability, Fates, ReplicaConfig};

    pub struct Amphora {
        members: Vec<Database>,
        cut: Arc<[AtomicBool; NODES]>,
        stop: Arc<AtomicBool>,
        relay: Option<std::thread::JoinHandle<()>>,
    }

    fn value(a: Arg) -> amphora_api::Value {
        match a {
            Arg::Text(s) => amphora_api::Value::Text(s),
            Arg::Int(i) => amphora_api::Value::Integer(i),
            Arg::Null => amphora_api::Value::Null,
        }
    }

    impl Amphora {
        pub fn open(o: &Opts) -> anyhow::Result<Self> {
            let dir = need(o.dir.clone(), "--dir", &o.measure)?;
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir)?;
            let ids: Vec<u64> = (1..=NODES as u64).collect();
            let durability = match o.class.as_deref() {
                // Each member its own fate (the operator's number for what it fails with: here its
                // node ID, three members on three fates).
                Some("replicated") => Durability::Replicated(Fates { of: ids.iter().map(|&i| (i, i)).collect(), spanning: need(o.spanning, "--spanning", &o.measure)? }),
                _ => Durability::Persisted,
            };
            let mut members = Vec::new();
            for &node_id in &ids {
                let config = ReplicaConfig { node_id, voters: ids.clone(), learners: Vec::new(), bootstrap: None };
                let db = Database::open_replicated_with(&dir.join(format!("state-{node_id}.db")), &dir.join(format!("raft-{node_id}.log")), config, durability.clone(), Instant::now() + o.deadline)
                    .map_err(|e| anyhow::anyhow!("member {node_id}: {e:?}"))?;
                members.push(db);
            }
            let handles: Vec<amphora::replication::ReplicaHandle> = members.iter().filter_map(Database::replica).collect();
            anyhow::ensure!(handles.len() == NODES, "a member has no replica handle");
            let cut: Arc<[AtomicBool; NODES]> = Arc::new(Default::default());
            let stop = Arc::new(AtomicBool::new(false));
            let (c, s, wait) = (cut.clone(), stop.clone(), o.deadline);
            let records = need(o.relay_records, "--relay-records", &o.measure)?;
            let relay = std::thread::spawn(move || relay(&handles, &c, &s, wait, records));
            Ok(Self { members, cut, stop, relay: Some(relay) })
        }

        fn member(&self, n: usize) -> anyhow::Result<&Database> {
            self.members.get(n).ok_or_else(|| anyhow::anyhow!("no node {n}"))
        }

        fn set_cut(&self, n: usize, to: bool) -> anyhow::Result<()> {
            self.cut.get(n).ok_or_else(|| anyhow::anyhow!("no node {n}"))?.store(to, Ordering::Relaxed);
            Ok(())
        }
    }

    /// Every member's outgoing records to the member they name, until `stop`; a cut member's
    /// records, out or in, are lost as a dead node's are. When nothing moved it yields its core and
    /// looks again: no sleep of a picked length between a record and its delivery.
    fn relay(handles: &[amphora::replication::ReplicaHandle], cut: &[AtomicBool; NODES], stop: &AtomicBool, wait: Duration, records: usize) {
        let is_cut = |i: usize| cut.get(i).is_some_and(|c| c.load(Ordering::Relaxed));
        while !stop.load(Ordering::Relaxed) {
            let mut moved = 0usize;
            for (from, handle) in handles.iter().enumerate() {
                for record in handle.drain(records).unwrap_or_default() {
                    let to = usize::try_from(record.to).unwrap_or(0).saturating_sub(1);
                    moved += 1;
                    if is_cut(from) || is_cut(to) {
                        continue;
                    }
                    if let Some(target) = handles.get(to) {
                        let _ = target.receive(from as u64 + 1, record.bytes, Instant::now() + wait);
                    }
                }
            }
            if moved == 0 {
                std::thread::yield_now();
            }
        }
    }

    impl Drop for Amphora {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(r) = self.relay.take() {
                let _ = r.join();
            }
        }
    }

    impl Cluster for Amphora {
        fn exec(&self, via: usize, stmts: &[(String, Vec<Arg>)], until: Instant) -> anyhow::Result<()> {
            anyhow::ensure!(!self.cut.get(via).is_some_and(|c| c.load(Ordering::Relaxed)), "node {via} is down");
            let stmts = stmts.to_vec();
            self.member(via)?
                .call(until, move |c| {
                    for (sql, args) in stmts {
                        if args.is_empty() {
                            c.execute_batch(&sql)?;
                        } else {
                            c.prepare_cached(&sql)?.execute(args.into_iter().map(value).collect::<Vec<_>>())?;
                        }
                    }
                    Ok(())
                })
                .map_err(|e| anyhow::anyhow!("node {via} refused a write: {e:?}"))
        }

        fn count(&self, on: usize, sql: &str, args: Vec<Arg>, until: Instant) -> anyhow::Result<u64> {
            let sql = sql.to_owned();
            let n = self
                .member(on)?
                .query_local(until, move |c| {
                    let mut s = c.prepare_cached(&sql)?;
                    let mut rows = s.query_map(args.into_iter().map(value).collect::<Vec<_>>(), |r| r.get::<_, i64>(0usize))?;
                    Ok(rows.next().transpose()?.unwrap_or(0))
                })
                .map_err(|e| anyhow::anyhow!("node {on} refused a read: {e:?}"))?;
            Ok(u64::try_from(n)?)
        }

        fn leader(&self, _until: Instant) -> anyhow::Result<Option<usize>> {
            // The leader a live member names, if that member is live too.
            for (i, m) in self.members.iter().enumerate() {
                if self.cut.get(i).is_some_and(|c| c.load(Ordering::Relaxed)) {
                    continue;
                }
                let Some(status) = m.replica().and_then(|h| h.status().ok()) else { continue };
                let l = usize::try_from(status.leader).unwrap_or(0);
                if l >= 1 && !self.cut.get(l - 1).is_some_and(|c| c.load(Ordering::Relaxed)) {
                    return Ok(Some(l - 1));
                }
            }
            Ok(None)
        }

        fn kill(&self, n: usize) -> anyhow::Result<()> {
            self.set_cut(n, true)
        }

        fn start(&self, n: usize) -> anyhow::Result<()> {
            self.set_cut(n, false)
        }
    }
}

/// The competitors over HTTP, each cluster brought up and down by its script.
mod http {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub(super) enum Dialect {
        /// rqlite's `/db/execute` and `/db/query` (plain JSON arguments).
        Rqlite,
        /// sqld's Hrana-over-HTTP `/v2/pipeline` (typed arguments).
        Hrana,
        /// The bench's own shim beside dqlite's Go client and LiteFS's mounted SQLite
        /// (`servers/repl/shim`): `/exec` and `/count`.
        Shim,
    }

    pub struct Http {
        system: String,
        dialect: Dialect,
        script: PathBuf,
        shell: String,
        endpoints: Vec<String>,
        agent: ureq::Agent,
    }

    fn plain(a: &Arg) -> serde_json::Value {
        match a {
            Arg::Text(s) => serde_json::json!(s),
            Arg::Int(i) => serde_json::json!(i),
            Arg::Null => serde_json::Value::Null,
        }
    }

    fn hrana(a: &Arg) -> serde_json::Value {
        match a {
            Arg::Text(s) => serde_json::json!({ "type": "text", "value": s }),
            Arg::Int(i) => serde_json::json!({ "type": "integer", "value": i.to_string() }),
            Arg::Null => serde_json::json!({ "type": "null" }),
        }
    }

    /// rqlite's body: one array per statement, its SQL first.
    pub(super) fn rqlite_body(stmts: &[(String, Vec<Arg>)]) -> serde_json::Value {
        serde_json::Value::Array(stmts.iter().map(|(sql, args)| serde_json::Value::Array(std::iter::once(serde_json::json!(sql)).chain(args.iter().map(plain)).collect())).collect())
    }

    /// sqld's pipeline: the statements in one transaction on one stream, then its close.
    pub(super) fn hrana_body(stmts: &[(String, Vec<Arg>)], transaction: bool) -> serde_json::Value {
        let exec = |sql: &str, args: &[Arg]| serde_json::json!({ "type": "execute", "stmt": { "sql": sql, "args": args.iter().map(hrana).collect::<Vec<_>>() } });
        let mut requests = Vec::new();
        if transaction {
            requests.push(exec("BEGIN", &[]));
        }
        requests.extend(stmts.iter().map(|(sql, args)| exec(sql, args)));
        if transaction {
            requests.push(exec("COMMIT", &[]));
        }
        requests.push(serde_json::json!({ "type": "close" }));
        serde_json::json!({ "requests": requests })
    }

    /// The first error any of rqlite's results carries.
    pub(super) fn rqlite_error(v: &serde_json::Value) -> Option<String> {
        if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
            return Some(e.to_owned());
        }
        v.get("results")?.as_array()?.iter().find_map(|r| r.get("error").and_then(|e| e.as_str()).map(str::to_owned))
    }

    /// The first error any of sqld's results carries.
    pub(super) fn hrana_error(v: &serde_json::Value) -> Option<String> {
        let results = v.get("results").and_then(|r| r.as_array());
        let Some(results) = results else { return Some(format!("no results in {v}")) };
        results.iter().find_map(|r| (r.get("type").and_then(|t| t.as_str()) == Some("error")).then(|| r.get("error").map_or_else(|| r.to_string(), |e| e.to_string())))
    }

    /// A count read back: rqlite's `results[0].values[0][0]`, sqld's `results[0].response.result.rows[0][0].value`.
    pub(super) fn count_of(dialect: Dialect, v: &serde_json::Value) -> Option<u64> {
        match dialect {
            Dialect::Rqlite => v.get("results")?.get(0)?.get("values")?.get(0)?.get(0)?.as_u64(),
            Dialect::Hrana => v.get("results")?.get(0)?.get("response")?.get("result")?.get("rows")?.get(0)?.get(0)?.get("value")?.as_str()?.parse().ok(),
            Dialect::Shim => v.get("count")?.as_u64(),
        }
    }

    impl Http {
        pub fn up(system: &str, scripts: &std::path::Path, shell: &str, deadline: Duration) -> anyhow::Result<Self> {
            let dialect = match system {
                "rqlite" => Dialect::Rqlite,
                "libsql" => Dialect::Hrana,
                _ => Dialect::Shim,
            };
            let agent: ureq::Agent = ureq::Agent::config_builder().no_delay(true).http_status_as_error(false).timeout_global(Some(deadline)).build().into();
            let mut h = Self { system: system.to_owned(), dialect, script: PathBuf::from(format!("{}/{system}.sh", scripts.display())), shell: shell.to_owned(), endpoints: Vec::new(), agent };
            h.script(&["up"]).map_err(|e| Refusal::Unavailable(format!("{system}'s cluster didn't come up through {}: {e}", h.script.display())))?;
            for n in 0..NODES {
                h.endpoints.push(h.script(&["endpoint", &n.to_string()])?.trim().to_owned());
            }
            Ok(h)
        }

        /// The script run by `--shell` (on Windows, name Git's bash by its full path: a bare `bash`
        /// can resolve to WSL's, which can't reach Docker Desktop's CLI the same way).
        fn script(&self, args: &[&str]) -> anyhow::Result<String> {
            let out = std::process::Command::new(&self.shell).arg(&self.script).args(args).output()?;
            anyhow::ensure!(out.status.success(), "{} {}: {}", self.script.display(), args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
            Ok(String::from_utf8(out.stdout)?)
        }

        fn endpoint(&self, n: usize) -> anyhow::Result<&str> {
            self.endpoints.get(n).map(String::as_str).ok_or_else(|| anyhow::anyhow!("no node {n}"))
        }

        fn post(&self, url: &str, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
            let mut r = self.agent.post(url).header("content-type", "application/json").send(body.to_string())?;
            let status = r.status();
            let text = r.body_mut().read_to_string()?;
            anyhow::ensure!(status.is_success(), "{url}: {status}: {text}");
            Ok(serde_json::from_str(&text)?)
        }
    }

    impl Drop for Http {
        fn drop(&mut self) {
            if let Err(e) = self.script(&["down"]) {
                eprintln!("{} down: {e}", self.system);
            }
        }
    }

    impl Cluster for Http {
        fn exec(&self, via: usize, stmts: &[(String, Vec<Arg>)], _until: Instant) -> anyhow::Result<()> {
            let ep = self.endpoint(via)?;
            let (url, body) = match self.dialect {
                Dialect::Rqlite => (format!("{ep}/db/execute?transaction"), rqlite_body(stmts)),
                Dialect::Hrana => (format!("{ep}/v2/pipeline"), hrana_body(stmts, stmts.len() > 1)),
                Dialect::Shim => (format!("{ep}/exec"), serde_json::json!({ "stmts": rqlite_body(stmts) })),
            };
            let v = self.post(&url, &body)?;
            let err = match self.dialect {
                Dialect::Rqlite => rqlite_error(&v),
                Dialect::Hrana => hrana_error(&v),
                Dialect::Shim => v.get("error").and_then(|e| e.as_str()).map(str::to_owned),
            };
            match err {
                Some(e) => anyhow::bail!("{} node {via}: {e}", self.system),
                None => Ok(()),
            }
        }

        fn count(&self, on: usize, sql: &str, args: Vec<Arg>, _until: Instant) -> anyhow::Result<u64> {
            let ep = self.endpoint(on)?;
            let stmt = [(sql.to_owned(), args)];
            let (url, body) = match self.dialect {
                // `level=none`: the node's own copy, not the leader's.
                Dialect::Rqlite => (format!("{ep}/db/query?level=none"), rqlite_body(&stmt)),
                Dialect::Hrana => (format!("{ep}/v2/pipeline"), hrana_body(&stmt, false)),
                Dialect::Shim => (format!("{ep}/count"), serde_json::json!({ "stmts": rqlite_body(&stmt) })),
            };
            let v = self.post(&url, &body)?;
            count_of(self.dialect, &v).ok_or_else(|| anyhow::anyhow!("{} node {on}: no count in {v}", self.system))
        }

        fn leader(&self, _until: Instant) -> anyhow::Result<Option<usize>> {
            let out = self.script(&["leader"])?;
            Ok(out.trim().parse::<usize>().ok().filter(|&n| n < NODES))
        }

        fn kill(&self, n: usize) -> anyhow::Result<()> {
            self.script(&["kill", &n.to_string()]).map(|_| ())
        }

        fn start(&self, n: usize) -> anyhow::Result<()> {
            self.script(&["start", &n.to_string()]).map(|_| ())
        }

        fn no_failover(&self) -> Option<String> {
            match self.system.as_str() {
                "libsql" => Some("sqld runs one primary and its replicas follow it: no election, so no failover to time".into()),
                "litefs" => Some("servers/repl/litefs.sh runs LiteFS's static lease (one fixed primary); failover needs its Consul lease, which the script doesn't run yet".into()),
                _ => None,
            }
        }

        fn no_catch_up(&self) -> Option<String> {
            match self.system.as_str() {
                "dqlite" => Some("dqlite serves every read through its leader: a member's own copy can't be read, so its catch-up can't be seen".into()),
                _ => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    // T-T1.8 (G12 step 1): every size and duration is an input; a missing one is refused, naming it.
    #[test]
    fn g12_repl_inputs_are_required_not_picked() {
        let base = "--repl commit --system rqlite --batch 1 --text-bytes 100 --deadline-s 5";
        let e = parse(&args(base)).err().unwrap().to_string();
        assert!(e.contains("--writers"), "{e}");
        let e = parse(&args(&format!("{base} --writers 2 --warmup-s 1"))).err().unwrap().to_string();
        assert!(e.contains("--seconds"), "{e}");
        assert!(parse(&args(&format!("{base} --writers 2 --warmup-s 1 --seconds 3"))).is_ok());
        let e = parse(&args("--repl catchup --system rqlite --batch 1 --text-bytes 1 --deadline-s 5 --behind 10")).err().unwrap().to_string();
        assert!(e.contains("--poll-us"), "{e}");
        let e = parse(&args("--repl failover --system amphora --batch 1 --text-bytes 1 --deadline-s 5 --writers 1 --kills 1 --settle-s 1 --dir d --relay-records 64 --class replicated")).err().unwrap().to_string();
        assert!(e.contains("--spanning"), "{e}");
        assert!(parse(&args("--repl failover --system amphora --batch 1 --text-bytes 1 --deadline-s 5 --writers 1 --kills 1 --settle-s 1 --dir d --relay-records 64 --class persisted")).is_ok());
    }

    // G12 step 1: the same write reaches rqlite and sqld as each one's dialect says.
    #[test]
    fn g12_repl_dialects_carry_the_same_write() {
        let s = vec![("INSERT INTO t VALUES (?1, ?2, ?3)".to_owned(), vec![Arg::Text("a".into()), Arg::Int(7), Arg::Null])];
        assert_eq!(http::rqlite_body(&s), serde_json::json!([["INSERT INTO t VALUES (?1, ?2, ?3)", "a", 7, null]]));
        let h = http::hrana_body(&s, true);
        let reqs = h["requests"].as_array().unwrap();
        assert_eq!(reqs.len(), 4, "BEGIN, the insert, COMMIT, close");
        assert_eq!(reqs[1]["stmt"]["args"], serde_json::json!([{ "type": "text", "value": "a" }, { "type": "integer", "value": "7" }, { "type": "null" }]));
        assert_eq!(reqs[3]["type"], "close");
        assert_eq!(http::hrana_body(&s, false)["requests"].as_array().unwrap().len(), 2);
    }

    // G12 step 1: an error in any result is a refused write, never an acknowledged one.
    #[test]
    fn g12_repl_errors_are_refusals() {
        assert_eq!(http::rqlite_error(&serde_json::json!({ "results": [{ "last_insert_id": 1 }, { "error": "not leader" }] })).as_deref(), Some("not leader"));
        assert_eq!(http::rqlite_error(&serde_json::json!({ "results": [{ "rows_affected": 1 }] })), None);
        assert!(http::hrana_error(&serde_json::json!({ "results": [{ "type": "ok" }, { "type": "error", "error": { "message": "busy" } }] })).unwrap().contains("busy"));
        assert_eq!(http::hrana_error(&serde_json::json!({ "results": [{ "type": "ok" }] })), None);
        assert!(http::hrana_error(&serde_json::json!({})).is_some(), "no results is no acknowledgement");
    }

    // A writer resends the same ids: its events are numbered from 1 in one session.
    #[test]
    fn g12_repl_writer_numbers_its_events() {
        let mut w = Writer::new(4);
        let a = w.next(2, 10);
        let b = w.next(1, 10);
        assert_eq!(a.iter().chain(&b).map(|e| e.seq).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!(a.iter().chain(&b).all(|e| e.session == "s-000004"));
        assert_eq!(event_args(&b[0]).unwrap().len(), 13);
    }
}
