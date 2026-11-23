//! Follower reads and search on three members (CP-T1.10; docs/design/replication.md, "Follower
//! reads at a closed time" and "Search over partitions"): AC-T1.10.3's bench amphora_read and
//! AC-T1.10.4's bench amphora_search.
//!
//! Every partition is a replicated group of three voters, members 1 to 3, each member's state and
//! log under its own directory, the three relayed in this process (the host's part: Amphora carries
//! no network of its own). Three nodes here are three members of every group on one machine's
//! disks and cores: the in-process measure; a lab across machines relays the same bytes.
//!
//! - load: `--events` events (kiln's event rows, K0's generator) spread over the partitions by
//!   session, `--batch` a write, through member 1;
//! - `read`: while member 1 writes one event a write to partition 0 as fast as it goes, member 3
//!   reads a session's latest page at `follower_read_micros` (now less its measured lag),
//!   `--reads` times: AC-T1.10.3's follower read latency;
//! - `search`: member 3 searches every partition at once, each at its closed time, and merges
//!   newest first (`amphora::search`), `--searches` times, words as K0's search phase takes them:
//!   AC-T1.10.4's search latency.
//!
//! - `ryw` (kiln goal 8119f25d, replication.md "Decision (lane ryw-1): read-your-writes by session
//!   tokens"): at each count of `--ladder` (Ada's ladder, 1 to 4096), that many sessions at once each
//!   write one event through member 1, keep the token its answer carries (`call_session`), and read
//!   the event straight back on a follower (members 2 and 3 in turn) in `--read-mode` (`latest`,
//!   the control, or `session`), `--reads` pairs in all: read and write latency, reads that missed
//!   their own write (must be 0), throughput, each follower's waits, redirects and apply lag, and
//!   the client's CPU seconds, share of the host's cores and peak memory, so a run shows whether
//!   the client was the bottleneck. Both read modes run through this one client on one platform,
//!   with the same settings: a comparison between them is fair only so.
//!
//! The relay (lane ryw-4, replication.md, "The bench's relay wakes on records") waits, when no
//! member had records, on one bell every member's records and end ring (`ReplicaHandle::listen`),
//! and `stop` rings it too: no timer between a record and its delivery. `--relay park` is the 100µs
//! timer it replaced, kept only as the control of the A/B run and removed once that run decides.
//! Each `ryw` record says which relay ran and what it did (passes, records moved, waits, and
//! wakes that found nothing to move).
//!
//! usage: amphora-bench --cluster --dir <dir> [--shapes read,search,ryw] [--partitions N]
//!        [--events 1000000] [--sessions 1000] [--batch 1000] [--reads 10000] [--searches 300]
//!        [--read-mode latest|session] [--ladder 1,8,32,128,512,2048,4096] [--relay bell|park]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use amphora::database::{AnswerMax, Database, ReadMode};
use amphora_api::Refusal;
use amphora_api::replication::ReplicaConfig;

use crate::{Hist, Rng, NONE, RARE, schema, summary};

/// The deadline of one call or read: a hang guard, never a verdict.
const WAIT: Duration = Duration::from_secs(60);
/// The members of every group, by node ID.
const MEMBERS: [u64; 3] = [1, 2, 3];

struct Opts {
    dir: PathBuf,
    shapes: Vec<String>,
    partitions: usize,
    events: usize,
    sessions: usize,
    batch: usize,
    reads: usize,
    searches: usize,
    text_bytes: usize,
    read_mode: ReadMode,
    ladder: Vec<usize>,
    relay: RelayWait,
}

fn opts() -> anyhow::Result<Opts> {
    let mut o = Opts {
        dir: PathBuf::new(),
        shapes: vec!["read".into(), "search".into()],
        // One partition a core, so a search's partitions are read on cores of their own.
        partitions: std::thread::available_parallelism().map_or(1, usize::from),
        events: 1_000_000,
        sessions: 1000,
        batch: 1000,
        reads: 10_000,
        searches: 300,
        text_bytes: 600,
        read_mode: ReadMode::Latest,
        // Ada's concurrency ladder (2026-11-21): a tiny instance may stop at 128.
        ladder: vec![1, 8, 32, 128, 512, 2048, 4096],
        relay: RelayWait::Bell,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || args.next().ok_or_else(|| anyhow::anyhow!("{a} takes a value"));
        match a.as_str() {
            "--cluster" => {}
            "--dir" => o.dir = PathBuf::from(v()?),
            "--shapes" => o.shapes = v()?.split(',').map(str::to_owned).collect(),
            "--partitions" => o.partitions = v()?.parse()?,
            "--events" => o.events = v()?.parse()?,
            "--sessions" => o.sessions = v()?.parse()?,
            "--batch" => o.batch = v()?.parse()?,
            "--reads" => o.reads = v()?.parse()?,
            "--searches" => o.searches = v()?.parse()?,
            "--text-bytes" => o.text_bytes = v()?.parse()?,
            "--read-mode" => {
                o.read_mode = match v()?.as_str() {
                    "latest" => ReadMode::Latest,
                    "session" => ReadMode::Session,
                    other => anyhow::bail!("--read-mode is latest or session, not {other}"),
                }
            }
            "--ladder" => o.ladder = v()?.split(',').map(str::parse).collect::<Result<_, _>>()?,
            "--relay" => {
                o.relay = match v()?.as_str() {
                    "bell" => RelayWait::Bell,
                    "park" => RelayWait::Park,
                    other => anyhow::bail!("--relay is bell or park, not {other}"),
                }
            }
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    if o.ladder.is_empty() || o.ladder.contains(&0) {
        anyhow::bail!("--ladder is counts of sessions, each at least 1");
    }
    anyhow::ensure!(!o.dir.as_os_str().is_empty(), "--dir names where the members' files go");
    anyhow::ensure!(o.partitions > 0 && o.batch > 0 && o.sessions > 0, "--partitions, --batch and --sessions take at least 1");
    Ok(o)
}

/// One partition: its group's three members, by node ID order.
struct Partition {
    members: Vec<Database>,
}

impl Partition {
    fn open(dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let mut members = Vec::new();
        for node_id in MEMBERS {
            let config = ReplicaConfig { node_id, voters: MEMBERS.to_vec(), learners: Vec::new(), bootstrap: None };
            let db = Database::open_replicated(&dir.join(format!("state-{node_id}.db")), &dir.join(format!("raft-{node_id}.log")), config, Instant::now() + WAIT)
                .map_err(|e| anyhow::anyhow!("member {node_id}: {e:?}"))?;
            members.push(db);
        }
        Ok(Self { members })
    }

    fn member(&self, node_id: u64) -> anyhow::Result<&Database> {
        self.members.get(usize::try_from(node_id)?.saturating_sub(1)).ok_or_else(|| anyhow::anyhow!("no member {node_id}"))
    }
}

/// How the relay waits when no member had records to move (`--relay`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RelayWait {
    /// On the bell the members' records and `stop` ring: event-driven.
    Bell,
    /// A 100µs `park_timeout`, the timer `Bell` replaced: the A/B run's control only.
    Park,
}

/// What wakes the relay: a member's records joining its outgoing queue, or its owner going (the
/// listener each member's handle is given, `ReplicaHandle::listen`), or `stop`. A ring given while the relay is moving
/// records is kept, so the relay's next wait finds it: no record waits for a later ring.
#[derive(Default)]
struct Bell {
    rung: Mutex<bool>,
    ringing: Condvar,
}

impl Bell {
    fn ring(&self) {
        if let Ok(mut rung) = self.rung.lock() {
            *rung = true;
            self.ringing.notify_one();
        }
    }

    /// Waits until rung, and takes the ring. False when the bell is poisoned (only a panic while
    /// it is held does that): the relay then ends, and the calls it no longer serves meet their
    /// deadlines.
    fn wait(&self) -> bool {
        let Ok(rung) = self.rung.lock() else { return false };
        match self.ringing.wait_while(rung, |rung| !*rung) {
            Ok(mut rung) => {
                *rung = false;
                true
            }
            Err(_) => false,
        }
    }
}

impl std::task::Wake for Bell {
    fn wake(self: Arc<Self>) {
        self.ring();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.ring();
    }
}

/// What the relay did, counted as it goes: each `ryw` record reports its rung's share.
#[derive(Default)]
struct RelayCounts {
    /// Passes over every member's queue.
    passes: AtomicU64,
    /// Records moved to the member they name.
    moved: AtomicU64,
    /// Waits for records, on the bell or the timer.
    waits: AtomicU64,
    /// Waits whose next pass found nothing to move: the timer's expiries, or a ring whose records
    /// an earlier pass had already moved.
    idle_wakes: AtomicU64,
}

impl RelayCounts {
    fn snapshot(&self) -> [u64; 4] {
        [&self.passes, &self.moved, &self.waits, &self.idle_wakes].map(|c| c.load(Ordering::Relaxed))
    }
}

/// The host's relay: every member's outgoing messages to the member they name, until `stop`
/// (which rings `bell` once set). When a pass moved nothing it waits as `wait` says.
fn relay(partitions: &[Partition], stop: &AtomicBool, bell: &Arc<Bell>, wait: RelayWait, counts: &RelayCounts) {
    let handles: Vec<Vec<amphora::replication::ReplicaHandle>> =
        partitions.iter().map(|p| p.members.iter().filter_map(Database::replica).collect()).collect();
    if wait == RelayWait::Bell {
        for handle in handles.iter().flatten() {
            // Refused only for a member already fenced: its calls are refused, and the bench
            // reports those.
            let _ = handle.listen(std::task::Waker::from(Arc::clone(bell)));
        }
    }
    let mut woke = false;
    while !stop.load(Ordering::Relaxed) {
        let mut moved = 0u64;
        for group in &handles {
            for (from, handle) in MEMBERS.iter().zip(group) {
                // The member's whole queue each pass: its own bound, no count picked here.
                for record in handle
                    .outgoing_bound()
                    .and_then(|bound| handle.drain(bound))
                    .unwrap_or_default()
                {
                    let to = usize::try_from(record.to).unwrap_or(0).saturating_sub(1);
                    if let Some(target) = group.get(to) {
                        let _ = target.receive(*from, record.bytes, Instant::now() + WAIT);
                        moved += 1;
                    }
                }
            }
        }
        counts.passes.fetch_add(1, Ordering::Relaxed);
        counts.moved.fetch_add(moved, Ordering::Relaxed);
        if woke && moved == 0 {
            counts.idle_wakes.fetch_add(1, Ordering::Relaxed);
        }
        woke = moved == 0;
        if woke {
            counts.waits.fetch_add(1, Ordering::Relaxed);
            match wait {
                RelayWait::Bell => {
                    if !bell.wait() {
                        return;
                    }
                }
                RelayWait::Park => std::thread::park_timeout(Duration::from_micros(100)),
            }
        }
    }
}

/// The schema through member 1, once its group has a leader: it waits on the status member 1's
/// owner publishes (`ReplicaHandle::wait_leader`; lane ryw-5, replication.md, "A host waits for a
/// leader on the owner's word"), never a retry timer. A call the lead's move refused (`NotLeader`,
/// `UnknownOutcome`, `Unavailable`) is made again once a leader of a later term stands: the
/// schema's statements are `IF NOT EXISTS`, so one that did commit commits nothing again. Any
/// other refusal, or the deadline, ends the bench with the refusal.
fn schema_on(partition: &Partition) -> anyhow::Result<()> {
    let until = Instant::now() + WAIT;
    let member = partition.member(1)?;
    let handle = member.replica().ok_or_else(|| anyhow::anyhow!("member 1 isn't a replica"))?;
    let (mut past, mut refused): (u64, Option<Refusal>) = (0, None);
    loop {
        let led = handle
            .wait_leader(past, until)
            .map_err(|e| anyhow::anyhow!("no leader past term {past} for the schema: {e:?} (last refusal: {refused:?})"))?;
        match member.call(until, AnswerMax::fixed(), |c| {
            for s in schema::SCHEMA {
                c.execute_batch(s)?;
            }
            Ok(())
        }) {
            Ok(()) => return Ok(()),
            Err(e @ (Refusal::NotLeader { .. } | Refusal::UnknownOutcome | Refusal::Unavailable(_))) => {
                past = led.term;
                refused = Some(e);
            }
            Err(e) => anyhow::bail!("the schema was refused: {e:?}"),
        }
    }
}

/// `events` written through member 1 in one call, answered with the call's own session token.
fn write(partition: &Partition, events: Vec<crate::Event>) -> anyhow::Result<Option<amphora::database::Token>> {
    partition
        .member(1)?
        .call_session(Instant::now() + WAIT, AnswerMax::fixed(), move |c| {
            let mut upsert = c.prepare_cached(schema::EVENT_UPSERT)?;
            for e in events {
                upsert.execute(vec![
                    amphora_api::Value::Text(e.session),
                    amphora_api::Value::Text(e.id),
                    amphora_api::Value::Integer(i64::try_from(e.seq)?),
                    amphora_api::Value::Integer(e.at),
                    amphora_api::Value::Null,
                    amphora_api::Value::Null,
                    amphora_api::Value::Text(e.kind.into()),
                    amphora_api::Value::Null,
                    amphora_api::Value::Null,
                    amphora_api::Value::Null,
                    amphora_api::Value::Text(e.text),
                    amphora_api::Value::Text(e.body),
                    amphora_api::Value::Integer(0),
                ])?;
            }
            Ok(())
        })
        .map(|((), token)| token)
        .map_err(|e| anyhow::anyhow!("a write was refused: {e:?}"))
}

/// Every partition's events, written through member 1, the partitions loaded at once.
fn load(partitions: &[Partition], o: &Opts) -> anyhow::Result<()> {
    let per = o.events.div_ceil(partitions.len());
    std::thread::scope(|scope| {
        let mut loading = Vec::new();
        for (p, partition) in partitions.iter().enumerate() {
            loading.push(scope.spawn(move || -> anyhow::Result<()> {
                let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (u64::try_from(p)? + 1));
                let sessions = o.sessions.div_ceil(partitions.len()).max(1);
                let mut seq = 0u64;
                let mut left = per;
                while left > 0 {
                    let n = left.min(o.batch);
                    let events = (0..n)
                        .map(|_| {
                            seq += 1;
                            let session = p * sessions + rng.below(sessions);
                            crate::event(&mut rng, session, seq, o.text_bytes)
                        })
                        .collect();
                    write(partition, events)?;
                    left -= n;
                }
                Ok(())
            }));
        }
        for l in loading {
            l.join().map_err(|_| anyhow::anyhow!("a loader panicked"))??;
        }
        Ok(())
    })
}

/// The newest `limit` events of partition `partition`'s replica on node 3 whose text holds `word`,
/// at its closed time: (at, session, id), newest first.
fn search_one(partition: &Partition, word: &str, limit: usize) -> anyhow::Result<Vec<(i64, (String, String))>> {
    let pattern = format!("%{word}%");
    let read = partition
        .member(3)?
        .query_closed(0, Instant::now() + WAIT, move |c| {
            let mut s = c.prepare_cached("SELECT at, session, id FROM events WHERE text LIKE ?1 ORDER BY at DESC LIMIT ?2")?;
            let rows = s.query_map(vec![amphora_api::Value::Text(pattern), amphora_api::Value::Integer(i64::try_from(limit)?)], |r| {
                Ok((r.get::<_, i64>(0usize)?, (r.get::<_, String>(1usize)?, r.get::<_, String>(2usize)?)))
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(|e| anyhow::anyhow!("a search was refused: {e:?}"))?;
    Ok(read.value)
}

/// This process's CPU seconds (user and system) and peak resident kilobytes, from /proc on Linux;
/// none elsewhere.
fn process_use() -> (Option<f64>, Option<u64>) {
    let cpu = std::fs::read_to_string("/proc/self/stat").ok().and_then(|stat| {
        // Fields after the command's closing parenthesis: utime and stime are the 12th and 13th.
        let rest = stat.rsplit_once(')')?.1.to_owned();
        let f: Vec<&str> = rest.split_whitespace().collect();
        let ticks = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
        // Linux's USER_HZ, a format value: 100 on every architecture it ships (times(2)).
        Some(ticks as f64 / 100.0)
    });
    let hwm = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:"))?.split_whitespace().nth(1)?.parse().ok());
    (cpu, hwm)
}

/// The `ryw` shape at one count of sessions: each session writes one event through member 1, keeps
/// the token the write's answer carries, and reads the event back on a follower in `o.read_mode`.
fn ryw(partition: &Partition, o: &Opts, relayed: &RelayCounts, sessions: usize, base: u64) -> anyhow::Result<serde_json::Value> {
    let rounds = o.reads.div_ceil(sessions).max(1);
    let relayed0 = relayed.snapshot();
    let (cpu0, _) = process_use();
    let began = Instant::now();
    let (writes, reads, missed) = std::thread::scope(|scope| -> anyhow::Result<(Hist, Hist, u64)> {
        let mut running = Vec::new();
        for s in 0..sessions {
            running.push(scope.spawn(move || -> anyhow::Result<(Hist, Hist, u64)> {
                let (mut w, mut r, mut missed) = (Hist::new(), Hist::new(), 0u64);
                let mut rng = Rng(0x7279_7731 ^ (u64::try_from(s)? + 1));
                let reader = partition.member(2 + u64::try_from(s % 2)?)?;
                let mut token: Option<amphora::database::Token> = None;
                for i in 0..rounds {
                    let seq = base + u64::try_from(s * rounds + i)?;
                    let event = crate::event(&mut rng, 1_000_000 + s, seq, o.text_bytes);
                    let id = event.id.clone();
                    let t = Instant::now();
                    let mine = write(partition, vec![event])?;
                    w.record(us(t));
                    token = match (token, mine) {
                        (Some(held), Some(mine)) => Some(held.after(mine)),
                        (held, mine) => mine.or(held),
                    };
                    let t = Instant::now();
                    let found = reader
                        .query_session(o.read_mode, token, Instant::now() + WAIT, move |c| {
                            let mut q = c.prepare_cached("SELECT id FROM events WHERE id = ?1")?;
                            let rows = q.query_map(vec![amphora_api::Value::Text(id)], |row| row.get::<_, String>(0usize))?;
                            Ok(rows.count())
                        })
                        .map_err(|e| anyhow::anyhow!("a read was refused: {e:?}"))?;
                    r.record(us(t));
                    if found == 0 {
                        missed += 1;
                    }
                }
                Ok((w, r, missed))
            }));
        }
        let (mut w, mut r, mut missed) = (Hist::new(), Hist::new(), 0u64);
        for run in running {
            let (wi, ri, mi) = run.join().map_err(|_| anyhow::anyhow!("a session panicked"))??;
            w.merge(&wi);
            r.merge(&ri);
            missed += mi;
        }
        Ok((w, r, missed))
    })?;
    let took = began.elapsed().as_secs_f64();
    let (cpu1, hwm) = process_use();
    let [passes, moved, waits, idle_wakes] = relayed.snapshot();
    let [passes0, moved0, waits0, idle0] = relayed0;
    let relay = serde_json::json!({
        "wait": format!("{:?}", o.relay), "passes": passes - passes0, "moved": moved - moved0,
        "waits": waits - waits0, "idle_wakes": idle_wakes - idle0,
    });
    let cpu = cpu0.zip(cpu1).map(|(a, b)| b - a);
    // The share of the host's cores this process used: near 1, the client (with the in-process
    // members) was CPU-bound, and the rung measures the host, not the database.
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut followers = Vec::new();
    for node in [2u64, 3] {
        let c = partition.member(node)?.session_counters(Instant::now() + WAIT).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        followers.push(c.map(|c| serde_json::json!({
            "node": node, "at_once": c.at_once, "waited": c.waited, "wait_ms": c.wait_nanos as f64 / 1e6,
            "redirects": c.redirects, "apply_lag_us": c.apply_lag_micros,
        })));
    }
    let pairs = sessions * rounds;
    Ok(serde_json::json!({
        "bench": "amphora_ryw", "client": "amphora-bench --cluster, in process", "platform": std::env::consts::OS,
        "read_mode": format!("{:?}", o.read_mode), "sessions": sessions, "pairs": pairs, "missed_own_write": missed,
        "pairs_per_s": pairs as f64 / took, "write_ms": summary(writes), "read_after_write_ms": summary(reads),
        "followers": followers, "cpu_s": cpu, "wall_s": took, "peak_rss_kb": hwm,
        "cores": cores, "client_core_share": cpu.map(|c| c / (took * cores as f64)), "threads": sessions,
        "relay": relay,
    }))
}

fn us(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX)
}

pub fn main() -> anyhow::Result<()> {
    let o = opts()?;
    let _ = std::fs::remove_dir_all(&o.dir);
    let partitions: Vec<Partition> = (0..o.partitions).map(|p| Partition::open(&o.dir.join(format!("p{p}")))).collect::<anyhow::Result<_>>()?;
    let stop = AtomicBool::new(false);
    let bell = Arc::new(Bell::default());
    let relayed = RelayCounts::default();
    std::thread::scope(|scope| -> anyhow::Result<()> {
        let relaying = scope.spawn(|| relay(&partitions, &stop, &bell, o.relay, &relayed));
        let ran = (|| -> anyhow::Result<()> {
            for p in &partitions {
                schema_on(p)?;
            }
            let loaded = Instant::now();
            load(&partitions, &o)?;
            let load_s = loaded.elapsed().as_secs_f64();
            let report = |shape: &str, h: Hist| {
                println!("{}", serde_json::json!({
                    "bench": format!("amphora_{shape}"), "partitions": o.partitions, "events": o.events, "sessions": o.sessions,
                    "members": MEMBERS.len(), "load_s": load_s, "latency_ms": summary(h),
                }));
            };
            if o.shapes.iter().any(|s| s == "read") {
                let first = partitions.first().ok_or_else(|| anyhow::anyhow!("no partition"))?;
                let writing = AtomicBool::new(true);
                let written = std::thread::scope(|inner| -> anyhow::Result<Hist> {
                    let writer = inner.spawn(|| -> anyhow::Result<()> {
                        let mut rng = Rng(0x5eed);
                        let mut seq = u64::try_from(o.events)?;
                        while writing.load(Ordering::Relaxed) {
                            seq += 1;
                            write(first, vec![crate::event(&mut rng, 0, seq, o.text_bytes)])?;
                        }
                        Ok(())
                    });
                    let follower = first.member(3)?;
                    let mut h = Hist::new();
                    let session = format!("s-{:06}", 0);
                    for _ in 0..o.reads {
                        let s = session.clone();
                        let t = Instant::now();
                        let at = follower.follower_read_micros(Instant::now() + WAIT).unwrap_or(0);
                        follower
                            .query_closed(at, Instant::now() + WAIT, move |c| {
                                let mut q = c.prepare_cached(schema::EVENTS_LAST)?;
                                let rows = q.query_map(vec![amphora_api::Value::Text(s), amphora_api::Value::Integer(50)], |r| r.get::<_, String>(1usize))?;
                                Ok(rows.count())
                            })
                            .map_err(|e| anyhow::anyhow!("a follower read was refused: {e:?}"))?;
                        h.record(us(t));
                    }
                    writing.store(false, Ordering::Relaxed);
                    writer.join().map_err(|_| anyhow::anyhow!("the writer panicked"))??;
                    Ok(h)
                })?;
                report("read", written);
            }
            if o.shapes.iter().any(|s| s == "ryw") {
                let first = partitions.first().ok_or_else(|| anyhow::anyhow!("no partition"))?;
                let mut base = u64::try_from(o.events)? + 1_000_000_000;
                for &sessions in &o.ladder {
                    println!("{}", ryw(first, &o, &relayed, sessions, base)?);
                    base += u64::try_from(o.reads.saturating_mul(2).max(sessions * 2))?;
                }
            }
            if o.shapes.iter().any(|s| s == "search") {
                let mut h = Hist::new();
                for i in 0..o.searches {
                    let word = match i % 3 {
                        0 => "quorum",
                        1 => RARE,
                        _ => NONE,
                    };
                    let t = Instant::now();
                    let each = amphora::search::fan_out(&partitions, |p| search_one(p, word, 20))?;
                    std::hint::black_box(amphora::search::newest(each, 20));
                    h.record(us(t));
                }
                report("search", h);
            }
            Ok(())
        })();
        stop.store(true, Ordering::Relaxed);
        bell.ring();
        relaying.join().map_err(|_| anyhow::anyhow!("the relay panicked"))?;
        ran
    })?;
    Ok(())
}
