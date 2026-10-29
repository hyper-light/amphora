//! Amphora against vanilla Turso, SQLite and clustered MySQL (Vitess) on kiln's conversation
//! workload (K0; docs/design/store.md).
//!
//! One workload, one harness, a driver per database:
//! - **preload**: every session gets its history first (not measured, but timed);
//! - **mixed**: writers append events durably while readers read sessions' latest pages, for a
//!   while. A writer is a client with one write outstanding at a time, its next begun as the last
//!   is durable: logical, not a thread, so a run can have a million of them, handed over by a few
//!   submitter threads (a database taking a connection per transaction gets them through a pool of
//!   connections, as a pooler would);
//! - **search**: a word across every session, newest first.
//!
//! Each run prints one JSON line: throughput and latency percentiles of each part.

// Tests may unwrap, assert and index (CLAUDE.md §1): clippy reads the workspace's clippy.toml here.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::disallowed_macros))]

mod blink;
mod clickhouse;
mod eqp;
mod mysql;
mod oracle;
mod pg;
mod planner;
mod pool;
mod probe;
mod query;
mod schema;
mod store;
mod suite;
mod suite_db;
mod vanilla;
mod vitess;

use std::collections::BinaryHeap;
use std::sync::{Arc, Condvar, Mutex};
use std::cmp::Reverse;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// An event as the workload writes it: kiln's event row (kiln crates/kiln/src/convo/rows.rs).
#[derive(Clone)]
pub struct Event {
    pub session: String,
    pub id: String,
    pub seq: u64,
    pub at: i64,
    pub kind: &'static str,
    pub text: String,
    pub body: String,
}

/// Told once a write is durable, or why it won't be, from whichever thread learns it.
pub type Done = Box<dyn FnOnce(anyhow::Result<()>) + Send>;

/// A database the workload runs against.
pub trait Driver: Send + Sync {
    /// Hands the events over to be made durable and returns, blocking only while the database has
    /// no room: `done` is told once they are durable.
    fn write(&self, events: Vec<Event>, done: Done) -> anyhow::Result<()>;
    /// A session's latest `limit` events: how many came.
    fn history(&self, session: &str, limit: usize) -> anyhow::Result<usize>;
    /// Events whose text holds `word`, newest first, at most `limit`: how many came.
    fn search(&self, word: &str, limit: usize) -> anyhow::Result<usize>;
    /// What the database says of itself after the run, if it says anything.
    fn stats(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
}

struct Opts {
    target: String,
    dir: String,
    dsn: String,
    sessions: usize,
    preload: usize,
    writers: usize,
    batch: usize,
    readers: usize,
    seconds: u64,
    searches: usize,
    text_bytes: usize,
    /// Readers read sessions no writer writes (the upper half; writers the lower): what reads cost
    /// apart from waiting for their own session's writes.
    disjoint: bool,
    /// The writers' writes per second in all, paced (0: as fast as each goes): reads compared under
    /// the same write load.
    write_rate: u64,
    /// Threads handing the writers' writes over.
    submitters: usize,
    /// Connections a database that takes one per transaction gets (its pool's), and the writes
    /// queued for them at most.
    connections: usize,
    queue: usize,
    /// Seconds of the mixed phase before latencies count (the first writes of a million writers wait
    /// for every other's first).
    warmup: u64,
}

fn opts() -> anyhow::Result<Opts> {
    let mut o = Opts {
        target: "local".into(),
        dir: "/tmp/kilndb-bench".into(),
        dsn: "mysql://root@127.0.0.1:33577/convo".into(),
        sessions: 1000,
        preload: 200,
        writers: 32,
        batch: 1,
        readers: 4,
        seconds: 30,
        searches: 20,
        text_bytes: 600,
        disjoint: false,
        write_rate: 0,
        submitters: 8,
        connections: 64,
        queue: 4096,
        warmup: 0,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || args.next().ok_or_else(|| anyhow::anyhow!("{a} takes a value"));
        match a.as_str() {
            "--target" => o.target = v()?,
            "--dir" => o.dir = v()?,
            "--dsn" => o.dsn = v()?,
            "--sessions" => o.sessions = v()?.parse()?,
            "--preload" => o.preload = v()?.parse()?,
            "--writers" => o.writers = v()?.parse()?,
            "--batch" => o.batch = v()?.parse()?,
            "--readers" => o.readers = v()?.parse()?,
            "--seconds" => o.seconds = v()?.parse()?,
            "--searches" => o.searches = v()?.parse()?,
            "--text-bytes" => o.text_bytes = v()?.parse()?,
            "--disjoint" => o.disjoint = true,
            "--write-rate" => o.write_rate = v()?.parse()?,
            "--submitters" => o.submitters = v()?.parse()?,
            "--connections" => o.connections = v()?.parse()?,
            "--queue" => o.queue = v()?.parse()?,
            "--warmup" => o.warmup = v()?.parse()?,
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    Ok(o)
}

/// A small, fast generator: the same workload every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % (n.max(1) as u64)).unwrap_or(0)
    }
}

const WORDS: [&str; 24] = [
    "the", "agent", "ran", "cargo", "test", "and", "read", "file", "then", "wrote", "patch", "store", "writer", "while", "feed", "waited", "build", "deploy", "review", "shard", "raft", "leader", "commit",
    "quorum",
];
/// A word few events hold, and one none does: the searches that look at the most.
const RARE: &str = "zephyr";
const NONE: &str = "zzqxv";

fn text(rng: &mut Rng, bytes: usize) -> String {
    let mut s = String::with_capacity(bytes + 16);
    // One event in a thousand says the rare word.
    if rng.below(1000) == 0 {
        s.push_str(RARE);
    }
    while s.len() < bytes {
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(WORDS[rng.below(WORDS.len())]);
    }
    s
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn event(rng: &mut Rng, session: usize, seq: u64, text_bytes: usize) -> Event {
    let kinds = ["assistant", "user", "tool", "thinking"];
    let kind = kinds[rng.below(kinds.len())];
    let text = text(rng, text_bytes);
    let body = serde_json::json!({ "kind": kind, "text": text, "n": seq }).to_string();
    Event { session: format!("s-{session:06}"), id: format!("e-{seq}"), seq, at: now_ms(), kind, text, body }
}

/// The heap's bytes now and at most: what each database holds, apart from code, stacks and the
/// allocator's own keeping (which the resident set counts too). Each thread counts its own and adds
/// it to the shared count once it passes SLACK either way: a shared count touched at every
/// allocation was a cache line every thread fought over, half of every thread's time at 128
/// writers (fedora, 2026-10-20). The shared count is off by at most SLACK a thread.
struct Counting;

const SLACK: isize = 16 << 10;
static HEAP: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static HEAP_PEAK: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

thread_local! {
    static OWN: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

fn count(delta: isize) {
    let passed = OWN
        .try_with(|own| {
            let mine = own.get() + delta;
            if mine.abs() < SLACK {
                own.set(mine);
                0
            } else {
                own.set(0);
                mine
            }
        })
        .unwrap_or(delta);
    if passed != 0 {
        let now = HEAP.fetch_add(passed, Ordering::Relaxed) + passed;
        if passed > 0 {
            HEAP_PEAK.fetch_max(now, Ordering::Relaxed);
        }
    }
}

fn size(n: usize) -> isize {
    isize::try_from(n).unwrap_or(isize::MAX)
}

unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let p = unsafe { std::alloc::System.alloc(layout) };
        if !p.is_null() {
            count(size(layout.size()));
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(ptr, layout) };
        count(-size(layout.size()));
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new: usize) -> *mut u8 {
        let p = unsafe { std::alloc::System.realloc(ptr, layout, new) };
        if !p.is_null() {
            count(size(new) - size(layout.size()));
        }
        p
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn mb(bytes: usize) -> f64 {
    (bytes as f64 / (1 << 20) as f64 * 10.0).round() / 10.0
}

fn heap_mb(count: &std::sync::atomic::AtomicIsize) -> f64 {
    mb(usize::try_from(count.load(Ordering::Relaxed)).unwrap_or(0))
}

/// A database that does nothing: what the harness holds of itself.
struct Null;

impl Driver for Null {
    fn write(&self, _events: Vec<Event>, done: Done) -> anyhow::Result<()> {
        done(Ok(()));
        Ok(())
    }

    fn history(&self, _session: &str, _limit: usize) -> anyhow::Result<usize> {
        Ok(0)
    }

    fn search(&self, _word: &str, _limit: usize) -> anyhow::Result<usize> {
        Ok(0)
    }
}

/// This process's resident memory that no file backs (its heap, stacks and anonymous maps: what it
/// allocated, apart from code and files it maps), in MiB; none where the system doesn't say (Linux's
/// `RssAnon`).
fn rss_anon_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: f64 = status.lines().find_map(|l| l.strip_prefix("RssAnon:"))?.trim().trim_end_matches("kB").trim().parse().ok()?;
    Some((kb / 1024.0 * 10.0).round() / 10.0)
}

/// What this process has had written to storage (bytes) and the CPU it has used (s), where the
/// system says: Linux's `/proc/self/io` (`write_bytes`, counted as pages are dirtied, so what
/// in-process stores write to their logs and files) and `/proc/self/stat` (`utime` and `stime`, in
/// USER_HZ, 100 a second on Linux).
fn written_and_cpu() -> (Option<u64>, Option<f64>) {
    let written = std::fs::read_to_string("/proc/self/io").ok().and_then(|io| io.lines().find_map(|l| l.strip_prefix("write_bytes:")).and_then(|v| v.trim().parse().ok()));
    let cpu = std::fs::read_to_string("/proc/self/stat").ok().and_then(|stat| {
        // After the command's closing parenthesis: the state, then utime and stime 11 and 12 on.
        let fields: Vec<&str> = stat.get(stat.rfind(')')? + 1..)?.split_whitespace().collect();
        let ticks = fields.get(11)?.parse::<f64>().ok()? + fields.get(12)?.parse::<f64>().ok()?;
        Some(ticks / 100.0)
    });
    (written, cpu)
}

/// This process's resident memory now, in MiB (Linux's `VmRSS`); none where the system doesn't say.
fn rss_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: f64 = status.lines().find_map(|l| l.strip_prefix("VmRSS:"))?.trim().trim_end_matches("kB").trim().parse().ok()?;
    Some((kb / 1024.0 * 10.0).round() / 10.0)
}

/// The most memory this process held at once (its resident set's high-water mark), in MiB; none
/// where the system doesn't say (it's Linux's `VmHWM`).
fn rss_max_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: f64 = status.lines().find_map(|l| l.strip_prefix("VmHWM:"))?.trim().trim_end_matches("kB").trim().parse().ok()?;
    Some((kb / 1024.0 * 10.0).round() / 10.0)
}

/// Latencies (µs) counted in log-linear buckets: 64 to each power of two, so a value reads back within
/// 1/64 of itself and a run's every sample fits in a few KB (keeping each sample, the harness held
/// more memory than the databases it measured).
#[derive(Clone)]
struct Hist {
    counts: Vec<u64>,
    n: u64,
    max: u64,
}

const SUB: u32 = 6;

impl Hist {
    fn new() -> Self {
        Self { counts: vec![0; ((64 - SUB as usize + 1) << SUB) + 1], n: 0, max: 0 }
    }

    fn index(v: u64) -> usize {
        if v < (1 << SUB) {
            return v as usize;
        }
        let k = 63 - v.leading_zeros();
        let sub = (v >> (k - SUB)) & ((1 << SUB) - 1);
        (((k - SUB + 1) as usize) << SUB) + sub as usize
    }

    /// The least value of bucket `i`.
    fn value(i: usize) -> u64 {
        if i < (1 << SUB) {
            return i as u64;
        }
        let block = (i >> SUB) as u32;
        let sub = (i & ((1 << SUB) - 1)) as u64;
        let k = block + SUB - 1;
        (1u64 << k) | (sub << (k - SUB))
    }

    fn record(&mut self, us: u64) {
        self.counts[Self::index(us)] += 1;
        self.n += 1;
        self.max = self.max.max(us);
    }

    fn merge(&mut self, o: &Hist) {
        for (a, b) in self.counts.iter_mut().zip(&o.counts) {
            *a += b;
        }
        self.n += o.n;
        self.max = self.max.max(o.max);
    }

    fn quantile(&self, q: f64) -> u64 {
        let want = ((self.n as f64) * q).ceil().max(1.0) as u64;
        let mut seen = 0;
        for (i, c) in self.counts.iter().enumerate() {
            seen += c;
            if seen >= want {
                return Self::value(i).min(self.max);
            }
        }
        self.max
    }
}

/// [`Hist`] recorded from any thread at once (writes are told durable on the databases' threads).
struct Shared {
    counts: Vec<AtomicU64>,
    n: AtomicU64,
    max: AtomicU64,
}

impl Shared {
    fn new() -> Self {
        Self { counts: (0..Hist::new().counts.len()).map(|_| AtomicU64::new(0)).collect(), n: AtomicU64::new(0), max: AtomicU64::new(0) }
    }

    fn record(&self, us: u64) {
        if let Some(c) = self.counts.get(Hist::index(us)) {
            c.fetch_add(1, Ordering::Relaxed);
        }
        self.n.fetch_add(1, Ordering::Relaxed);
        self.max.fetch_max(us, Ordering::Relaxed);
    }

    fn hist(&self) -> Hist {
        let mut h = Hist::new();
        for (a, b) in h.counts.iter_mut().zip(&self.counts) {
            *a = b.load(Ordering::Relaxed);
        }
        h.n = self.n.load(Ordering::Relaxed);
        h.max = self.max.load(Ordering::Relaxed);
        h
    }
}

/// Writes `events` and waits until they are durable.
fn write_wait(driver: &Arc<dyn Driver>, events: Vec<Event>) -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    driver.write(
        events,
        Box::new(move |r| {
            let _ = tx.send(r);
        }),
    )?;
    rx.recv().map_err(|_| anyhow::anyhow!("a write was never answered"))?
}

/// The writers waiting to begin their next write, earliest first: who, and from when its latency
/// counts.
struct Ready {
    heap: Mutex<BinaryHeap<Reverse<(Instant, usize)>>>,
    wake: Condvar,
}

impl Ready {
    fn push(&self, at: Instant, writer: usize) {
        if let Ok(mut heap) = self.heap.lock() {
            heap.push(Reverse((at, writer)));
        }
        self.wake.notify_one();
    }

    /// The earliest writer due, once it is (none: stop came first).
    fn pop(&self, stop: &AtomicBool) -> Option<(Instant, usize)> {
        let mut heap = self.heap.lock().ok()?;
        loop {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            let wait = match heap.peek() {
                Some(Reverse((at, _))) => match at.checked_duration_since(Instant::now()) {
                    None => return heap.pop().map(|Reverse(next)| next),
                    Some(wait) => wait.min(Duration::from_millis(100)),
                },
                None => Duration::from_millis(100),
            };
            heap = self.wake.wait_timeout(heap, wait).ok()?.0;
        }
    }
}

/// Percentiles of latencies in ms.
fn summary(h: Hist) -> serde_json::Value {
    if h.n == 0 {
        return serde_json::json!(null);
    }
    let ms = |us: u64| us as f64 / 1000.0;
    serde_json::json!({ "n": h.n, "p50": ms(h.quantile(0.5)), "p90": ms(h.quantile(0.9)), "p99": ms(h.quantile(0.99)), "p999": ms(h.quantile(0.999)), "max": ms(h.max) })
}

fn main() -> anyhow::Result<()> {
    // The comprehensive suite: every kind of statement, each on its own (`suite.rs`).
    if std::env::args().any(|a| a == "--suite") {
        return suite::main();
    }
    // A statement on a database a suite run left behind: plans and times (`query.rs`).
    if std::env::args().any(|a| a == "--query") {
        return query::main();
    }
    // The SQLite language's probes on Amphora or Turso (`probe.rs`, bench/corpus).
    if std::env::args().any(|a| a == "--probe") {
        return probe::main();
    }
    // The oracles for logic bugs and slow plans on Amphora, SQLite the reference (`oracle/`).
    if std::env::args().any(|a| a == "--oracle") {
        return oracle::main();
    }
    // JOB and TPC-H on Amphora, its estimates beside its counts, and on SQLite, DuckDB and
    // PostgreSQL (`planner/`).
    if std::env::args().any(|a| a == "--planner") {
        return planner::main();
    }
    // A SQL script's plans: the planner audit's demonstrations (`eqp.rs`).
    if std::env::args().any(|a| a == "--eqp") {
        return eqp::main();
    }
    // The versioned storage's B-link tree on its own (`blink.rs`).
    if std::env::args().any(|a| a == "--blink") {
        return blink::main();
    }
    let o = opts()?;
    let (k, q) = (o.connections, o.queue);
    // A server's own port on this machine (bench/servers/<engine>.sh) unless --dsn names another.
    let dsn = |local: &str| if std::env::args().any(|a| a == "--dsn") { o.dsn.clone() } else { local.to_owned() };
    let driver: Arc<dyn Driver> = match o.target.as_str() {
        "amphora" => Arc::new(store::Amphora::open(&o.dir)?),
        "turso" => Arc::new(pool::Pooled::new(vanilla::Turso::open(&o.dir)?, k, q)?),
        "sqlite" => Arc::new(pool::Pooled::new(vanilla::Sqlite::open(&o.dir)?, k, q)?),
        "null" => Arc::new(Null),
        "vitess" => Arc::new(pool::Pooled::new(vitess::Vitess::connect(&o.dsn, k + o.readers + 2)?, k, q)?),
        "mysql" => Arc::new(pool::Pooled::new(mysql::MySql::connect(&dsn("mysql://root@127.0.0.1:33306/convo"), k + o.readers + 2)?, k, q)?),
        "postgres" => Arc::new(pool::Pooled::new(pg::Pg::connect(&dsn("postgres://postgres@127.0.0.1:55432/postgres"), pg::Engine::Postgres)?, k, q)?),
        "orioledb" => Arc::new(pool::Pooled::new(pg::Pg::connect(&dsn("postgres://postgres@127.0.0.1:55433/postgres"), pg::Engine::Orioledb)?, k, q)?),
        "cedardb" => Arc::new(pool::Pooled::new(pg::Pg::connect(&dsn("postgres://postgres:amphorabenchlocalonly2026@127.0.0.1:55434/postgres"), pg::Engine::Cedardb)?, k, q)?),
        "clickhouse" => Arc::new(pool::Pooled::new(clickhouse::ClickHouse::connect(&dsn("http://127.0.0.1:58123"), false, k + o.readers + 2)?, k, q)?),
        "clickhouse-durable" => Arc::new(pool::Pooled::new(clickhouse::ClickHouse::connect(&dsn("http://127.0.0.1:58123"), true, k + o.readers + 2)?, k, q)?),
        t => anyhow::bail!("unknown target {t} (amphora, turso, sqlite, vitess, mysql, postgres, orioledb, cedardb, clickhouse, clickhouse-durable, null)"),
    };

    // Preload: every session's history, a hundred events a write, writers side by side.
    let seqs: Arc<Vec<AtomicU64>> = Arc::new((0..o.sessions).map(|_| AtomicU64::new(0)).collect());
    let began = Instant::now();
    // Preloaded by at most 64 threads, whatever the writers.
    let loaders = o.writers.clamp(1, 64);
    std::thread::scope(|s| -> anyhow::Result<()> {
        let mut handles = Vec::new();
        for w in 0..loaders {
            let (driver, seqs, o) = (driver.clone(), seqs.clone(), &o);
            handles.push(s.spawn(move || -> anyhow::Result<()> {
                let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (w as u64 + 1));
                for session in (w..o.sessions).step_by(loaders) {
                    let mut left = o.preload;
                    while left > 0 {
                        let n = left.min(100);
                        let batch: Vec<Event> = (0..n).map(|_| event(&mut rng, session, seqs[session].fetch_add(1, Ordering::Relaxed), o.text_bytes)).collect();
                        write_wait(&driver, batch)?;
                        left -= n;
                    }
                }
                Ok(())
            }));
        }
        for h in handles {
            h.join().map_err(|_| anyhow::anyhow!("a preload writer panicked"))??;
        }
        Ok(())
    })?;
    let preload_s = began.elapsed().as_secs_f64();
    let stats_after_preload = driver.stats();
    let heap_after_preload = heap_mb(&HEAP);

    // AMPHORA_PROFILE=<prefix>: the mixed phase sampled 997 times a second, written as
    // <prefix>.svg (a flamegraph) and <prefix>.txt (the frames most samples were in).
    let profile = std::env::var("AMPHORA_PROFILE").ok();
    let profiler = match &profile {
        Some(_) => Some(crate::profile_start()?),
        None => None,
    };

    // Mixed: writers append, readers read, for a while.
    let stop = Arc::new(AtomicBool::new(false));
    let until = Duration::from_secs(o.seconds);
    // Each second's writes finished, and the longest of them (µs), by the second it finished in.
    let slots = usize::try_from(o.seconds).unwrap_or(0) + 2;
    let per_second: Arc<Vec<(AtomicU64, AtomicU64)>> = Arc::new((0..slots).map(|_| (AtomicU64::new(0), AtomicU64::new(0))).collect());
    let mixed = Instant::now();
    let (written_before, cpu_before) = written_and_cpu();
    let count_from = mixed + Duration::from_secs(o.warmup);
    // The most anonymous memory held during the mixed phase, looked at every 100 ms.
    let anon_peak = Arc::new(AtomicU64::new(0));
    let sampler = {
        let (stop, anon_peak) = (stop.clone(), anon_peak.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Some(mb) = rss_anon_mb() {
                    anon_peak.fetch_max((mb * 10.0) as u64, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };
    let written = Arc::new(Shared::new());
    let ready = Arc::new(Ready { heap: Mutex::new((0..o.writers).map(|w| Reverse((mixed, w))).collect()), wake: Condvar::new() });
    let inflight = Arc::new(AtomicU64::new(0));
    let failures = Arc::new(AtomicU64::new(0));
    // Paced: each writer a write every writers/rate seconds, from when it was due (not when the last
    // was answered), so a slow answer can't hide its own wait.
    let interval = (o.write_rate > 0).then(|| Duration::from_secs_f64(o.writers as f64 / o.write_rate as f64));
    let reads = std::thread::scope(|s| -> anyhow::Result<Hist> {
        let mut subs = Vec::new();
        for t in 0..o.submitters.max(1) {
            let (driver, seqs, stop, o, per_second, ready, written, inflight, failures) =
                (driver.clone(), seqs.clone(), stop.clone(), &o, per_second.clone(), ready.clone(), written.clone(), inflight.clone(), failures.clone());
            subs.push(s.spawn(move || -> anyhow::Result<()> {
                let mut rng = Rng(0xd1b5_4a32_d192_ed03 ^ (t as u64 + 1));
                while let Some((due, w)) = ready.pop(&stop) {
                    let session = rng.below(if o.disjoint { o.sessions / 2 } else { o.sessions });
                    let batch: Vec<Event> = (0..o.batch.max(1)).map(|_| event(&mut rng, session, seqs[session].fetch_add(1, Ordering::Relaxed), o.text_bytes)).collect();
                    let (ready, written, per_second, stop, inflight, failures) = (ready.clone(), written.clone(), per_second.clone(), stop.clone(), inflight.clone(), failures.clone());
                    inflight.fetch_add(1, Ordering::Relaxed);
                    driver.write(
                        batch,
                        Box::new(move |answer| {
                            let now = Instant::now();
                            if answer.is_err() {
                                failures.fetch_add(1, Ordering::Relaxed);
                            }
                            // From when the writer was due: its wait for a submitter and for room
                            // counts, as a client's would.
                            let us = u64::try_from(now.saturating_duration_since(due).as_micros()).unwrap_or(u64::MAX);
                            if now >= count_from {
                                written.record(us);
                            }
                            if let Some((n, most)) = per_second.get(usize::try_from(now.saturating_duration_since(mixed).as_secs()).unwrap_or(usize::MAX)) {
                                n.fetch_add(1, Ordering::Relaxed);
                                most.fetch_max(us, Ordering::Relaxed);
                            }
                            inflight.fetch_sub(1, Ordering::Relaxed);
                            if !stop.load(Ordering::Relaxed) {
                                ready.push(interval.map_or(now, |i| (due + i).max(now)), w);
                            }
                        }),
                    )?;
                }
                Ok(())
            }));
        }
        let mut rs = Vec::new();
        for r in 0..o.readers {
            let (driver, stop, o) = (driver.clone(), stop.clone(), &o);
            rs.push(s.spawn(move || -> anyhow::Result<Hist> {
                let mut rng = Rng(0x2545_f491_4f6c_dd1d ^ (r as u64 + 1));
                let mut lat = Hist::new();
                while !stop.load(Ordering::Relaxed) {
                    let session = format!("s-{:06}", if o.disjoint { o.sessions / 2 + rng.below(o.sessions - o.sessions / 2) } else { rng.below(o.sessions) });
                    let t = Instant::now();
                    driver.history(&session, 50)?;
                    lat.record(u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX));
                }
                Ok(lat)
            }));
        }
        std::thread::sleep(until);
        stop.store(true, Ordering::Relaxed);
        ready.wake.notify_all();
        for h in subs {
            h.join().map_err(|_| anyhow::anyhow!("a submitter panicked"))??;
        }
        let mut reads = Hist::new();
        for h in rs {
            reads.merge(&h.join().map_err(|_| anyhow::anyhow!("a reader panicked"))??);
        }
        Ok(reads)
    })?;
    // What was handed over and not yet answered is waited for (a minute at most): it counts.
    let drain = Instant::now() + Duration::from_secs(60);
    while inflight.load(Ordering::Relaxed) > 0 && Instant::now() < drain {
        std::thread::sleep(Duration::from_millis(10));
    }
    let unanswered = inflight.load(Ordering::Relaxed);
    let _ = sampler.join();
    let writes = written.hist();
    let stats_after_mixed = driver.stats();
    let (written_after, cpu_after) = written_and_cpu();
    if let (Some(prefix), Some(guard)) = (&profile, profiler) {
        write_profile(prefix, &guard)?;
    }

    // Search: a common word, the rare one, and one none says.
    let mut searches = Hist::new();
    for i in 0..o.searches {
        let word = match i % 3 {
            0 => "quorum",
            1 => RARE,
            _ => NONE,
        };
        let t = Instant::now();
        driver.search(word, 20)?;
        searches.record(u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX));
    }

    let events_written = writes.n as usize * o.batch.max(1);
    // Throughput over the seconds counted (after the warmup, before the stop).
    let counted: u64 = per_second.iter().skip(usize::try_from(o.warmup).unwrap_or(0)).take(usize::try_from(o.seconds.saturating_sub(o.warmup)).unwrap_or(0)).map(|(n, _)| n.load(Ordering::Relaxed)).sum();
    let counted_s = o.seconds.saturating_sub(o.warmup).max(1) as f64;
    let report = serde_json::json!({
        "target": o.target,
        "sessions": o.sessions,
        "preload_events": o.sessions * o.preload,
        "preload_s": preload_s,
        "preload_events_per_s": (o.sessions * o.preload) as f64 / preload_s.max(1e-9),
        "writers": o.writers,
        "submitters": o.submitters,
        "connections": o.connections,
        "queue": o.queue,
        "warmup": o.warmup,
        "failures": failures.load(Ordering::Relaxed),
        "unanswered": unanswered,
        "batch": o.batch,
        "readers": o.readers,
        "seconds": o.seconds,
        "writes": writes.n,
        "events_written": events_written,
        "write_events_per_s": (counted as usize * o.batch.max(1)) as f64 / counted_s,
        "write_ms": summary(writes),
        "read_ms": summary(reads),
        "search_ms": summary(searches),
        "write_rate": o.write_rate,
        "disjoint": o.disjoint,
        // The mixed phase's: what this process wrote to storage (an in-process store's log and
        // files) and the CPU it used, warmup included.
        "mixed_written_mb": written_before.zip(written_after).map(|(b, a)| (a.saturating_sub(b) as f64 / 1048576.0 * 10.0).round() / 10.0),
        "mixed_cpu_s": cpu_before.zip(cpu_after).map(|(b, a)| ((a - b) * 100.0).round() / 100.0),
        "rss_max_mb": rss_max_mb(),
        "rss_anon_max_mb": anon_peak.load(Ordering::Relaxed) as f64 / 10.0,
        "heap_peak_mb": heap_mb(&HEAP_PEAK),
        "heap_end_mb": heap_mb(&HEAP),
        "heap_after_preload_mb": heap_after_preload,
        "writes_each_second": per_second.iter().map(|(n, _)| n.load(Ordering::Relaxed)).collect::<Vec<_>>(),
        "longest_write_ms_each_second": per_second.iter().map(|(_, m)| m.load(Ordering::Relaxed) / 1000).collect::<Vec<_>>(),
        "stats_after_preload": stats_after_preload,
        "stats_after_mixed": stats_after_mixed,
    });
    println!("{report}");
    Ok(())
}

/// A sampling profiler (pprof's: a timer signal and backtraces), on Unix only.
#[cfg(unix)]
pub type Profiler = pprof::ProfilerGuard<'static>;
#[cfg(not(unix))]
pub struct Profiler;

/// Samples every thread 997 times a second until the profiler is dropped.
#[cfg(unix)]
pub fn profile_start() -> anyhow::Result<Profiler> {
    Ok(pprof::ProfilerGuardBuilder::default().frequency(997).blocklist(&["libc", "libgcc", "pthread", "vdso"]).build()?)
}

#[cfg(not(unix))]
pub fn profile_start() -> anyhow::Result<Profiler> {
    anyhow::bail!("AMPHORA_PROFILE samples with a timer signal: Unix only")
}

#[cfg(not(unix))]
pub fn write_profile(_prefix: &str, _guard: &Profiler) -> anyhow::Result<()> {
    anyhow::bail!("AMPHORA_PROFILE samples with a timer signal: Unix only")
}

/// Writes what `guard` sampled: `<prefix>.svg` (a flamegraph), `<prefix>.txt` (the frames most
/// samples were in) and `<prefix>.folded` (every stack, for any tally after the run).
#[cfg(unix)]
pub fn write_profile(prefix: &str, guard: &Profiler) -> anyhow::Result<()> {
    let report = guard.report().build()?;
    report.flamegraph(std::fs::File::create(format!("{prefix}.svg"))?)?;
    // The frames most samples were in, by leaf and by any frame on the stack: every thread's,
    // then the writer's alone.
    type Counts = std::collections::HashMap<String, usize>;
    let mut threads: Counts = Counts::new();
    let tally = |only: Option<&str>| {
        let (mut leaf, mut any, mut total) = (Counts::new(), Counts::new(), 0usize);
        for (frames, count) in &report.data {
            if only.is_some_and(|t| frames.thread_name != t) {
                continue;
            }
            let count = usize::try_from(*count).unwrap_or(0);
            total += count;
            let names: Vec<String> = frames.frames.iter().map(|f| f.iter().map(|s| s.name()).next().unwrap_or_default()).collect();
            if let Some(first) = names.first() {
                *leaf.entry(first.clone()).or_default() += count;
            }
            let mut seen = std::collections::HashSet::new();
            for n in names {
                if seen.insert(n.clone()) {
                    *any.entry(n).or_default() += count;
                }
            }
        }
        (leaf, any, total)
    };
    for (frames, count) in &report.data {
        *threads.entry(frames.thread_name.clone()).or_default() += usize::try_from(*count).unwrap_or(0);
    }
    let top = |m: Counts, total: usize, n: usize| {
        let mut v: Vec<(String, usize)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v.into_iter().take(n).map(|(k, c)| format!("{:6.2}% {k}", c as f64 * 100.0 / total.max(1) as f64)).collect::<Vec<_>>().join("\n")
    };
    let mut out = String::new();
    let all: usize = threads.values().sum();
    out.push_str(&format!("== threads ==\n{}\n\n", top(threads, all, 40)));
    for (title, only) in [("writer", Some("amphora-writer")), ("all", None)] {
        let (leaf, any, total) = tally(only);
        out.push_str(&format!("== {title}: samples {total}, by leaf ==\n{}\n\n== {title}: inclusive ==\n{}\n\n", top(leaf, total, 60), top(any, total, 150)));
    }
    std::fs::write(format!("{prefix}.txt"), out)?;
    // Every stack, root first, inlined frames each their own, for any tally after the run.
    let mut folded = String::new();
    for (frames, count) in &report.data {
        let mut names: Vec<String> = Vec::new();
        for f in frames.frames.iter().rev() {
            names.extend(f.iter().rev().map(|s| s.name()));
        }
        folded.push_str(&format!("{};{} {count}\n", frames.thread_name, names.join(";")));
    }
    std::fs::write(format!("{prefix}.folded"), folded)?;
    Ok(())
}
