//! The versioned storage's log (Amphora's docs/design/engine.md, decision 11; CP-T1.4's AC-T1.4.2
//! and AC-T1.4.3): durable single-row commits from many writers, each on disjoint keys, through one
//! group-committed log (`--lanes 1`) and through per-thread logs (a lane a writer), on the portable
//! files or, on Linux, io_uring (`--uring`). Each variant runs twice: with no checkpoint, then with
//! checkpoints back to back; each commit is timed, and those that overlapped a checkpoint are told
//! apart. One JSON line a run, then one a variant: commits a second, p50, p99, p99.9 and the
//! longest; records a flush (the group size the device's flush time made); and the p99.9 a
//! checkpoint adds (AC-T1.4.3: at most 1 ms). AC-T1.4.2 compares `--lanes 1`'s p99 at 32 writers,
//! CP-T1.3's log, with the per-thread logs' (at least halved).
//!
//! usage: amphora-bench --vlog --dir <empty dir> [--writers 32] [--lanes 1,<cores>] [--seconds 10]
//!        [--keys 200000] [--value 200] [--uring]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use amphora_engine::storage::versioned::blink::Descender;
use amphora_engine::storage::versioned::disk::{Disk, Files};
use amphora_engine::storage::versioned::durable::Database;

fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^ (x >> 33)
}

fn e(err: amphora_engine::LimboError) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}

struct Opts {
    dir: std::path::PathBuf,
    writers: usize,
    lanes: Vec<usize>,
    seconds: u64,
    keys: u64,
    value: usize,
    uring: bool,
}

fn opts() -> anyhow::Result<Opts> {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let mut o = Opts {
        dir: std::path::PathBuf::new(),
        // The plan's load (CP-T1.3, AC-T1.3.2): 32 writers.
        writers: 32,
        // One log against a lane per core (Haubenschild et al.: a log per worker thread).
        lanes: vec![1, cores],
        seconds: 10,
        // The suite's `items` (200,000 rows) and a 200-byte row.
        keys: 200_000,
        value: 200,
        uring: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || args.next().ok_or_else(|| anyhow::anyhow!("{a} takes a value"));
        match a.as_str() {
            "--vlog" => {}
            "--dir" => o.dir = v()?.into(),
            "--writers" => o.writers = v()?.parse()?,
            "--lanes" => o.lanes = v()?.split(',').map(str::parse).collect::<Result<_, _>>()?,
            "--seconds" => o.seconds = v()?.parse()?,
            "--keys" => o.keys = v()?.parse()?,
            "--value" => o.value = v()?.parse()?,
            "--uring" => o.uring = true,
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    anyhow::ensure!(!o.dir.as_os_str().is_empty(), "--dir names an empty directory for the files");
    anyhow::ensure!(o.writers > 0 && o.keys >= o.writers as u64, "--keys at least --writers, and a writer");
    Ok(o)
}

fn disk(dir: &std::path::Path, lanes: usize, uring: bool) -> anyhow::Result<Arc<dyn Disk>> {
    if uring {
        #[cfg(target_os = "linux")]
        return Ok(Arc::new(amphora_engine::storage::versioned::uring::Uring::open(dir, lanes).map_err(e)?));
        #[cfg(not(target_os = "linux"))]
        anyhow::bail!("--uring: io_uring is Linux's");
    }
    Ok(Arc::new(Files::open(dir, lanes).map_err(e)?))
}

/// The latency at quantile `q` of sorted `ns`, in milliseconds.
fn at(ns: &[u64], q: f64) -> f64 {
    if ns.is_empty() {
        return 0.0;
    }
    let i = ((ns.len() as f64 * q).ceil() as usize).clamp(1, ns.len()) - 1;
    ns[i] as f64 / 1e6
}

fn quantiles(ns: &mut [u64]) -> serde_json::Value {
    ns.sort_unstable();
    serde_json::json!({
        "commits": ns.len(),
        "p50_ms": at(ns, 0.5),
        "p99_ms": at(ns, 0.99),
        "p999_ms": at(ns, 0.999),
        "max_ms": at(ns, 1.0),
    })
}

/// One run: `o.writers` writers committing for `o.seconds`, checkpoints back to back if
/// `checkpoints`; each commit's time, and whether a checkpoint ran during it.
fn run(o: &Opts, db: &Arc<Database>, checkpoints: bool) -> anyhow::Result<(Vec<u64>, Vec<u64>, u64, f64)> {
    let stop = Arc::new(crate::Stop::default());
    // Odd while a checkpoint runs; moved on as each begins and ends.
    let phase = Arc::new(AtomicU64::new(0));
    let checkpointer = checkpoints.then(|| {
        let (db, stop, phase) = (db.clone(), stop.clone(), phase.clone());
        std::thread::spawn(move || -> anyhow::Result<u64> {
            let mut n = 0;
            while !stop.stopped() {
                phase.fetch_add(1, Ordering::AcqRel);
                let done = db.checkpoint();
                phase.fetch_add(1, Ordering::AcqRel);
                done.map_err(e)?;
                n += 1;
            }
            Ok(n)
        })
    });
    // Versions no snapshot sees collected back to back, as the writers make them: the heads a
    // checkpoint's pause lists stay those of commits since the last pass.
    let collector = {
        let (db, stop) = (db.clone(), stop.clone());
        std::thread::spawn(move || -> anyhow::Result<()> {
            let mut d = Descender::new();
            while !stop.stopped() {
                db.collect(&mut d).map_err(e)?;
            }
            Ok(())
        })
    };
    let began = Instant::now();
    let each = o.keys / o.writers as u64;
    let writers: Vec<_> = (0..o.writers as u64)
        .map(|w| {
            let (db, stop, phase) = (db.clone(), stop.clone(), phase.clone());
            let value = vec![w as u8; o.value];
            std::thread::spawn(move || -> anyhow::Result<(Vec<u64>, Vec<u64>)> {
                let mut d = Descender::new();
                let lane = db.lane();
                let (mut quiet, mut during) = (Vec::new(), Vec::new());
                let mut i = 0u64;
                while !stop.stopped() {
                    i += 1;
                    let key = mix(w * each + mix(i) % each).to_be_bytes();
                    let start = Instant::now();
                    let p0 = phase.load(Ordering::Acquire);
                    let mut t = db.begin().map_err(e)?;
                    db.put(&mut d, &mut t, 0, &key, &value).map_err(e)?;
                    db.commit(t, &mut d, lane, true).map_err(e)?;
                    let p1 = phase.load(Ordering::Acquire);
                    let ns = start.elapsed().as_nanos() as u64;
                    if p0 % 2 == 1 || p1 != p0 { during.push(ns) } else { quiet.push(ns) }
                }
                Ok((quiet, during))
            })
        })
        .collect();
    stop.wait_until(Instant::now() + Duration::from_secs(o.seconds));
    stop.stop();
    let (mut quiet, mut during) = (Vec::new(), Vec::new());
    for w in writers {
        let (q, d) = w.join().map_err(|_| anyhow::anyhow!("a writer panicked"))??;
        quiet.extend(q);
        during.extend(d);
    }
    let secs = began.elapsed().as_secs_f64();
    collector.join().map_err(|_| anyhow::anyhow!("the collector panicked"))??;
    let n = match checkpointer {
        Some(c) => c.join().map_err(|_| anyhow::anyhow!("the checkpointer panicked"))??,
        None => 0,
    };
    Ok((quiet, during, n, secs))
}

pub fn main() -> anyhow::Result<()> {
    let o = opts()?;
    let host = std::fs::read_to_string("/etc/hostname").unwrap_or_default().trim().to_owned();
    for &lanes in &o.lanes {
        let dir = o.dir.join(format!("lanes-{lanes}{}", if o.uring { "-uring" } else { "" }));
        std::fs::create_dir_all(&dir)?;
        anyhow::ensure!(std::fs::read_dir(&dir)?.next().is_none(), "{} isn't empty", dir.display());
        // A frame for each 1,500 bytes of rows (a page about half full after splits, as the
        // `--blink` bench sizes its pool), and room for the splits a checkpoint's copies follow.
        let row = 8 + o.value as u64 + 6;
        let frames = usize::try_from(o.keys * row / 1_500 + 1_000)?;
        let db = Arc::new(
            Database::open(disk(&dir, lanes, o.uring)?, frames, 1, || Box::new(|a: &[u8], b: &[u8]| a.cmp(b)))
                .map_err(e)?,
        );
        // The rows first, then a checkpoint: the runs change rows that exist.
        let mut d = Descender::new();
        let lane = db.lane();
        let each = o.keys / o.writers as u64;
        let value = vec![0u8; o.value];
        for w in 0..o.writers as u64 {
            for i in 0..each {
                let mut t = db.begin().map_err(e)?;
                db.put(&mut d, &mut t, 0, &mix(w * each + i).to_be_bytes(), &value).map_err(e)?;
                db.commit(t, &mut d, lane, false).map_err(e)?;
            }
        }
        db.sync().map_err(e)?;
        db.collect(&mut d).map_err(e)?;
        db.checkpoint().map_err(e)?;
        let mut p999 = [0.0f64; 2];
        for (k, checkpoints) in [false, true].into_iter().enumerate() {
            let before = db.log_counters();
            let (mut quiet, mut during, n, secs) = run(&o, &db, checkpoints)?;
            let after = db.log_counters();
            let flushes = after.flushes - before.flushes;
            let records = after.records - before.records;
            let commits = quiet.len() + during.len();
            let mut all: Vec<u64> = quiet.iter().chain(during.iter()).copied().collect();
            p999[k] = if checkpoints {
                during.sort_unstable();
                at(&during, 0.999)
            } else {
                all.sort_unstable();
                at(&all, 0.999)
            };
            println!(
                "{}",
                serde_json::json!({
                    "bench": "vlog",
                    "host": host,
                    "lanes": lanes,
                    "uring": o.uring,
                    "writers": o.writers,
                    "keys": o.keys,
                    "value": o.value,
                    "checkpoints": n,
                    "seconds": secs,
                    "commits_per_s": (commits as f64 / secs).round(),
                    "records_per_flush": if flushes == 0 { 0.0 } else { records as f64 / flushes as f64 },
                    "all": quantiles(&mut all),
                    "quiet": quantiles(&mut quiet),
                    "during_checkpoint": quantiles(&mut during),
                })
            );
        }
        println!(
            "{}",
            serde_json::json!({
                "bench": "vlog-variant",
                "host": host,
                "lanes": lanes,
                "uring": o.uring,
                "writers": o.writers,
                "p999_added_by_checkpoints_ms": p999[1] - p999[0],
            })
        );
    }
    Ok(())
}
