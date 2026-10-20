//! Amphora against vanilla Turso, SQLite and clustered MySQL (Vitess) on kiln's conversation
//! workload (K0; docs/design/store.md).
//!
//! One workload, one harness, a driver per database:
//! - **preload**: every session gets its history first (not measured, but timed);
//! - **mixed**: writers append events durably (a write returns once committed) while readers read
//!   sessions' latest pages, for a while;
//! - **search**: a word across every session, newest first.
//!
//! Each run prints one JSON line: throughput and latency percentiles of each part.

mod schema;
mod store;
mod vanilla;
mod vitess;

use std::sync::Arc;
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

/// A database the workload runs against.
pub trait Driver: Send + Sync {
    /// The events, durably: returns once they are committed.
    fn write(&self, events: &[Event]) -> anyhow::Result<()>;
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
    fn write(&self, _events: &[Event]) -> anyhow::Result<()> {
        Ok(())
    }

    fn history(&self, _session: &str, _limit: usize) -> anyhow::Result<usize> {
        Ok(0)
    }

    fn search(&self, _word: &str, _limit: usize) -> anyhow::Result<usize> {
        Ok(0)
    }
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

/// Percentiles of latencies in ms.
fn summary(h: Hist) -> serde_json::Value {
    if h.n == 0 {
        return serde_json::json!(null);
    }
    let ms = |us: u64| us as f64 / 1000.0;
    serde_json::json!({ "n": h.n, "p50": ms(h.quantile(0.5)), "p90": ms(h.quantile(0.9)), "p99": ms(h.quantile(0.99)), "p999": ms(h.quantile(0.999)), "max": ms(h.max) })
}

fn main() -> anyhow::Result<()> {
    let o = opts()?;
    let driver: Arc<dyn Driver> = match o.target.as_str() {
        "amphora" => Arc::new(store::Amphora::open(&o.dir)?),
        "turso" => Arc::new(vanilla::Turso::open(&o.dir)?),
        "sqlite" => Arc::new(vanilla::Sqlite::open(&o.dir)?),
        "null" => Arc::new(Null),
        "vitess" => Arc::new(vitess::Vitess::connect(&o.dsn, o.writers + o.readers + 2)?),
        t => anyhow::bail!("unknown target {t} (amphora, turso, sqlite, vitess, null)"),
    };

    // Preload: every session's history, a hundred events a write, writers side by side.
    let seqs: Arc<Vec<AtomicU64>> = Arc::new((0..o.sessions).map(|_| AtomicU64::new(0)).collect());
    let began = Instant::now();
    std::thread::scope(|s| -> anyhow::Result<()> {
        let mut handles = Vec::new();
        for w in 0..o.writers.max(1) {
            let (driver, seqs, o) = (driver.clone(), seqs.clone(), &o);
            handles.push(s.spawn(move || -> anyhow::Result<()> {
                let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (w as u64 + 1));
                for session in (w..o.sessions).step_by(o.writers.max(1)) {
                    let mut left = o.preload;
                    while left > 0 {
                        let n = left.min(100);
                        let batch: Vec<Event> = (0..n).map(|_| event(&mut rng, session, seqs[session].fetch_add(1, Ordering::Relaxed), o.text_bytes)).collect();
                        driver.write(&batch)?;
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
        Some(_) => Some(pprof::ProfilerGuardBuilder::default().frequency(997).blocklist(&["libc", "libgcc", "pthread", "vdso"]).build()?),
        None => None,
    };

    // Mixed: writers append, readers read, for a while.
    let stop = Arc::new(AtomicBool::new(false));
    let until = Duration::from_secs(o.seconds);
    // Each second's writes finished, and the longest of them (µs), by the second it finished in.
    let slots = usize::try_from(o.seconds).unwrap_or(0) + 2;
    let per_second: Arc<Vec<(AtomicU64, AtomicU64)>> = Arc::new((0..slots).map(|_| (AtomicU64::new(0), AtomicU64::new(0))).collect());
    let mixed = Instant::now();
    let (writes, reads) = std::thread::scope(|s| -> anyhow::Result<(Hist, Hist)> {
        let mut ws = Vec::new();
        for w in 0..o.writers {
            let (driver, seqs, stop, o, per_second) = (driver.clone(), seqs.clone(), stop.clone(), &o, per_second.clone());
            ws.push(s.spawn(move || -> anyhow::Result<Hist> {
                let mut rng = Rng(0xd1b5_4a32_d192_ed03 ^ (w as u64 + 1));
                let mut lat = Hist::new();
                // Paced: each writer its share of the rate, a write each interval.
                let interval = (o.write_rate > 0).then(|| Duration::from_secs_f64(o.writers as f64 / o.write_rate as f64));
                let mut next = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    if let Some(i) = interval {
                        next += i;
                        if let Some(wait) = next.checked_duration_since(Instant::now()) {
                            std::thread::sleep(wait);
                        }
                    }
                    let session = rng.below(if o.disjoint { o.sessions / 2 } else { o.sessions });
                    let batch: Vec<Event> = (0..o.batch.max(1)).map(|_| event(&mut rng, session, seqs[session].fetch_add(1, Ordering::Relaxed), o.text_bytes)).collect();
                    let t = Instant::now();
                    driver.write(&batch)?;
                    let us = u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX);
                    lat.record(us);
                    if let Some((n, most)) = per_second.get(usize::try_from(mixed.elapsed().as_secs()).unwrap_or(usize::MAX)) {
                        n.fetch_add(1, Ordering::Relaxed);
                        most.fetch_max(us, Ordering::Relaxed);
                    }
                }
                Ok(lat)
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
        let mut writes = Hist::new();
        for h in ws {
            writes.merge(&h.join().map_err(|_| anyhow::anyhow!("a writer panicked"))??);
        }
        let mut reads = Hist::new();
        for h in rs {
            reads.merge(&h.join().map_err(|_| anyhow::anyhow!("a reader panicked"))??);
        }
        Ok((writes, reads))
    })?;
    let stats_after_mixed = driver.stats();
    if let (Some(prefix), Some(guard)) = (&profile, profiler) {
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
    let report = serde_json::json!({
        "target": o.target,
        "sessions": o.sessions,
        "preload_events": o.sessions * o.preload,
        "preload_s": preload_s,
        "preload_events_per_s": (o.sessions * o.preload) as f64 / preload_s.max(1e-9),
        "writers": o.writers,
        "batch": o.batch,
        "readers": o.readers,
        "seconds": o.seconds,
        "writes": writes.n,
        "events_written": events_written,
        "write_events_per_s": events_written as f64 / o.seconds as f64,
        "write_ms": summary(writes),
        "read_ms": summary(reads),
        "search_ms": summary(searches),
        "write_rate": o.write_rate,
        "disjoint": o.disjoint,
        "rss_max_mb": rss_max_mb(),
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
