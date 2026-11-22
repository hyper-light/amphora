//! G11 step 2 (CS.2): the column copy's two prototypes against no copy, on the versioned storage
//! (`crates/engine/storage/versioned/columns.rs`; docs/design/columnar.md, decision 1 and "Step 2").
//!
//! For each copy (`none`, `plain`: prototype A, `packed`: prototype B) and each client count: a
//! fresh database in `--dir`, preloaded with `--rows` rows; the copy's first generation made from
//! them; then `--clients` threads each commit one row put at a time, durably (group commit), for
//! `--seconds`, while the copy's own thread applies its feed and rebuilds when its measured rule
//! says so, and one reader scans the copy at fresh snapshots. Then, writers stopped, the copy is
//! scanned and the rows are scanned, `--scans` times each. One JSON line a run:
//! - write cost: commits a second, and commit latency p50, p99, p99.9 (ns);
//! - freshness: the lag from a commit's hand-over to its application in the copy, p50 to p99.9
//!   (ns), the rows scans read from the B-tree because the queue held them, the scans that fell
//!   back to the rows, the commits the queue refused;
//! - memory: the copy's resident bytes a row against the rows' own key and row bytes a row;
//! - scan speed: rows a second scanning the copy and scanning the rows, best of `--scans`;
//! - the learned values: the block size the sizer settled at and the block count.
//!
//! usage: amphora-bench --columns --dir DIR [--copies none,plain,packed] [--clients 1,32]
//!        [--rows 100000] [--value 64] [--seconds 10] [--scans 5] [--queue-bytes N]
//!
//! `--queue-bytes` stands for the share of the database's memory budget the copy's queue is charged
//! to (CLAUDE.md §2); unbounded by default, so the queue's own high-water mark is what is measured.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use amphora_engine::storage::versioned::blink::{Comparator, Descender};
use amphora_engine::storage::versioned::columns::{
    Block, Columns, Feed, Packed, Plain, Rows, ScanCounts,
};
use amphora_engine::storage::versioned::disk::Files;
use amphora_engine::storage::versioned::memory::Capped;
use amphora_engine::storage::versioned::durable::Database;
use amphora_engine::storage::versioned::version::Snapshot;

fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^ (x >> 33)
}

fn e(err: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}

struct At<'a> {
    db: &'a Database,
    snap: &'a Snapshot,
}

impl Rows for At<'_> {
    fn get(&self, key: &[u8]) -> amphora_engine::Result<Option<Vec<u8>>> {
        self.db.get(&mut Descender::new(), self.snap, 0, key)
    }
    fn all(&self) -> amphora_engine::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        self.db
            .scan(&mut Descender::new(), self.snap, 0, None, usize::MAX)
    }
}

fn pct(v: &mut [u64], p: f64) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    let i = ((v.len() - 1) as f64 * p).round() as usize;
    v[i.min(v.len() - 1)]
}

struct Opts {
    dir: std::path::PathBuf,
    copies: Vec<String>,
    clients: Vec<usize>,
    rows: u64,
    value: usize,
    seconds: f64,
    scans: usize,
    queue_bytes: usize,
}

pub fn main() -> anyhow::Result<()> {
    let mut o = Opts {
        dir: std::path::PathBuf::new(),
        copies: vec!["none".into(), "plain".into(), "packed".into()],
        clients: vec![1, 32],
        rows: 100_000,
        value: 64,
        seconds: 10.0,
        scans: 5,
        queue_bytes: usize::MAX,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || {
            args.next()
                .ok_or_else(|| anyhow::anyhow!("{a} takes a value"))
        };
        match a.as_str() {
            "--columns" => {}
            "--dir" => o.dir = v()?.into(),
            "--copies" => o.copies = v()?.split(',').map(str::to_owned).collect(),
            "--clients" => o.clients = v()?.split(',').map(str::parse).collect::<Result<_, _>>()?,
            "--rows" => o.rows = v()?.parse()?,
            "--value" => o.value = v()?.parse()?,
            "--seconds" => o.seconds = v()?.parse()?,
            "--scans" => o.scans = v()?.parse()?,
            "--queue-bytes" => o.queue_bytes = v()?.parse()?,
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    anyhow::ensure!(!o.dir.as_os_str().is_empty(), "--dir is required");
    for copy in &o.copies {
        for &clients in &o.clients {
            match copy.as_str() {
                "none" => run::<Plain>(&o, copy, clients, false)?,
                "plain" => run::<Plain>(&o, copy, clients, true)?,
                "packed" => run::<Packed>(&o, copy, clients, true)?,
                other => anyhow::bail!("unknown copy {other}: none, plain or packed"),
            }
        }
    }
    Ok(())
}

fn rebuild<B: Block>(db: &Database, copy: &Columns<B>, epoch: Instant) -> anyhow::Result<u128> {
    let t = Instant::now();
    let snap = db.snapshot().map_err(e)?;
    let rows = At { db, snap: &snap }.all().map_err(e)?;
    copy.rebuild(snap.start, &rows, epoch.elapsed().as_nanos() as u64)
        .map_err(e)?;
    db.close_snapshot(snap);
    let took = t.elapsed().as_nanos();
    copy.rebuilt(took);
    Ok(took)
}

fn run<B: Block + 'static>(o: &Opts, name: &str, clients: usize, keep: bool) -> anyhow::Result<()> {
    let dir = o.dir.join(format!("{name}-{clients}"));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    // A page holds about half its bytes of cells after splits (bench/src/blink.rs's sizing): room
    // for the preload and what the run adds.
    let row = 8 + o.value as u64 + 6;
    let frames = usize::try_from((o.rows * 4) * row / 1_500 + 1_000)?;
    let db = Database::open(
        Arc::new(Files::open(&dir, 1).map_err(e)?),
        frames,
        1,
        || -> Comparator { Box::new(|a: &[u8], b: &[u8]| a.cmp(b)) },
    )
    .map_err(e)?;
    let v = vec![7u8; o.value];
    let mut d = Descender::new();
    let mut i = 0;
    while i < o.rows {
        let mut t = db.begin().map_err(e)?;
        let end = (i + 1_000).min(o.rows);
        for k in i..end {
            db.put(&mut d, &mut t, 0, &mix(k).to_be_bytes(), &v)
                .map_err(e)?;
        }
        db.commit(t, &mut d, db.lane(), false).map_err(e)?;
        i = end;
    }
    db.sync().map_err(e)?;
    let epoch = Instant::now();
    let copy = if keep {
        let feed = Arc::new(Feed::new(
            0,
            // The queue's bytes capped at --queue-bytes, as before the feed took a memory source (CS.5).
            Arc::new(Capped::new(o.queue_bytes as u64)),
            Box::new(move || epoch.elapsed().as_nanos() as u64),
        ));
        db.keep_columns(feed.clone()).map_err(e)?;
        let c = Columns::<B>::new(feed);
        rebuild(&db, &c, epoch)?;
        Some(c)
    } else {
        None
    };
    let stop = AtomicBool::new(false);
    let commits = AtomicU64::new(0);
    let latencies = Mutex::new(Vec::<u64>::new());
    let scan_counts = Mutex::new(ScanCounts::default());
    let mut lags = Vec::new();
    let mut rebuilds = 0u64;
    let began = Instant::now();
    std::thread::scope(|sc| -> anyhow::Result<()> {
        let mut handles = Vec::new();
        for w in 0..clients as u64 {
            let (db, stop, commits, latencies, v) = (&db, &stop, &commits, &latencies, &v);
            handles.push(sc.spawn(move || -> anyhow::Result<()> {
                let mut d = Descender::new();
                let mut mine = Vec::new();
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let k = mix((w << 40) ^ n) % (o.rows * 2);
                    n += 1;
                    let t0 = Instant::now();
                    let mut t = db.begin().map_err(e)?;
                    if db.put(&mut d, &mut t, 0, &mix(k).to_be_bytes(), v).is_err() {
                        db.abort(t, &mut d).map_err(e)?;
                        continue;
                    }
                    db.commit(t, &mut d, db.lane(), true).map_err(e)?;
                    mine.push(t0.elapsed().as_nanos() as u64);
                    commits.fetch_add(1, Ordering::Relaxed);
                }
                latencies
                    .lock()
                    .map_err(|_| anyhow::anyhow!("poisoned"))?
                    .extend(mine);
                Ok(())
            }));
        }
        let worker = copy.as_ref().map(|c| {
            let (db, stop) = (&db, &stop);
            sc.spawn(move || -> anyhow::Result<(Vec<u64>, u64)> {
                let mut lags = Vec::new();
                let mut rebuilds = 0;
                while !stop.load(Ordering::Relaxed) {
                    c.catch_up(epoch.elapsed().as_nanos() as u64).map_err(e)?;
                    lags.extend(c.take_lags());
                    if c.wants_rebuild() {
                        rebuild(db, c, epoch)?;
                        rebuilds += 1;
                    }
                    c.release(db.v.clock.low_watermark());
                    std::thread::yield_now();
                }
                Ok((lags, rebuilds))
            })
        });
        let reader = copy.as_ref().map(|c| {
            let (db, stop, scan_counts) = (&db, &stop, &scan_counts);
            sc.spawn(move || -> anyhow::Result<()> {
                while !stop.load(Ordering::Relaxed) {
                    let snap = db.snapshot().map_err(e)?;
                    let t = Instant::now();
                    let (_, k) = c.scan(snap.start, &At { db, snap: &snap }).map_err(e)?;
                    c.measured(t.elapsed().as_nanos(), &k);
                    db.close_snapshot(snap);
                    let mut s = scan_counts
                        .lock()
                        .map_err(|_| anyhow::anyhow!("poisoned"))?;
                    s.queued += k.queued;
                    s.fallbacks += k.fallbacks;
                    s.copy += 1;
                }
                Ok(())
            })
        });
        while began.elapsed().as_secs_f64() < o.seconds {
            std::thread::park_timeout(std::time::Duration::from_millis(100));
        }
        stop.store(true, Ordering::Relaxed);
        for h in handles {
            h.join()
                .map_err(|_| anyhow::anyhow!("a writer panicked"))??;
        }
        if let Some(h) = worker {
            let (l, r) = h
                .join()
                .map_err(|_| anyhow::anyhow!("the copy's worker panicked"))??;
            lags = l;
            rebuilds = r;
        }
        if let Some(h) = reader {
            h.join()
                .map_err(|_| anyhow::anyhow!("the reader panicked"))??;
        }
        Ok(())
    })?;
    let secs = began.elapsed().as_secs_f64();
    let mut lat = latencies
        .into_inner()
        .map_err(|_| anyhow::anyhow!("poisoned"))?;
    // Scans with writers stopped: the copy (caught up) and the rows, best of --scans.
    let mut best_copy = f64::INFINITY;
    let mut best_rows = f64::INFINITY;
    let mut n_rows = 0usize;
    let mut resident = 0usize;
    let mut shape = (0, None, 0);
    if let Some(c) = &copy {
        c.catch_up(epoch.elapsed().as_nanos() as u64).map_err(e)?;
        lags.extend(c.take_lags());
        resident = c.resident();
        shape = c.shape();
    }
    for _ in 0..o.scans {
        let snap = db.snapshot().map_err(e)?;
        let at = At {
            db: &db,
            snap: &snap,
        };
        let t = Instant::now();
        let rows = at.all().map_err(e)?;
        best_rows = best_rows.min(t.elapsed().as_secs_f64());
        n_rows = rows.len();
        if let Some(c) = &copy {
            let t = Instant::now();
            let (got, k) = c.scan(snap.start, &at).map_err(e)?;
            best_copy = best_copy.min(t.elapsed().as_secs_f64());
            anyhow::ensure!(
                got == rows && k.fallbacks == 0,
                "the copy's scan differs from the rows at {}",
                snap.start
            );
        }
        db.close_snapshot(snap);
    }
    let own = n_rows as f64 * (8 + o.value) as f64;
    let sc = scan_counts
        .into_inner()
        .map_err(|_| anyhow::anyhow!("poisoned"))?;
    let counts = copy.as_ref().map(|c| c.feed().counts()).unwrap_or_default();
    println!(
        "{}",
        serde_json::json!({
            "copy": name,
            "clients": clients,
            "rows": n_rows,
            "value": o.value,
            "seconds": secs,
            "commits_per_s": (commits.load(Ordering::Relaxed) as f64 / secs).round(),
            "commit_ns": {"p50": pct(&mut lat, 0.5), "p99": pct(&mut lat, 0.99), "p999": pct(&mut lat, 0.999)},
            "lag_ns": {"p50": pct(&mut lags, 0.5), "p99": pct(&mut lags, 0.99), "p999": pct(&mut lags, 0.999), "applied": lags.len()},
            "scans_during_writes": sc.copy,
            "rows_read_for_queue": sc.queued,
            "scan_fallbacks": sc.fallbacks,
            "queue_refused": counts.lost,
            "rebuilds": rebuilds,
            "copy_bytes_per_row": if n_rows > 0 { resident as f64 / n_rows as f64 } else { 0.0 },
            "row_bytes_per_row": if n_rows > 0 { own / n_rows as f64 } else { 0.0 },
            "scan_rows_per_s": {"copy": if keep { n_rows as f64 / best_copy } else { 0.0 }, "rows": n_rows as f64 / best_rows},
            "block_size": shape.1,
            "blocks": shape.2,
        })
    );
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
