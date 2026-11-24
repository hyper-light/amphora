//! The versioned storage's commit validations against each other (Amphora's docs/design/engine.md
//! decision 5, G8 step 1): precision locking, SSI and OCC on the same commit path, on the
//! comprehensive suite's write classes and K0, in memory, at `--writers` writers (32 by the step).
//!
//! Each class runs on a database of its own, `--items` rows of `--row-bytes` (the suite's 200,000
//! `items`, about 600 bytes a row: engine.md decision 6) with one secondary index, each writer a
//! thread in a closed loop for `--seconds`, writing in its own slice of the ids as the suite's
//! clients do, and running a refused transaction again (a first writer's win or a validation's
//! refusal) until it commits. At the storage, `update_text`, `update_json` and `upsert` are
//! `update_plain` (a row read by key and written whole). K0 (docs/design/store.md): 1,000 sessions
//! of 200 events preloaded, then each writer appends an event to a random session, reading and
//! rewriting its session's row, while 4 readers each read a session's latest 50 events in a
//! transaction (a reader validated where the scheme validates readers). An event's value is its
//! text (`--text-bytes`, K0's 600) alone: the storage holds no row past a page's largest cell yet.
//!
//! One JSON line a validation and class: commits a second, refusals by the first writer and by
//! validation, the share of attempts refused, and the latency of a committed transaction from its
//! first attempt (p50, p99, p99.9, in ms); for K0 the readers' too.
//!
//! usage: amphora-bench --cc [--validation precision,ssi,occ] [--classes all|name,...]
//!        [--writers 32] [--seconds 10] [--items 200000] [--row-bytes 600] [--text-bytes 600]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use amphora_engine::LimboError;
use amphora_engine::storage::versioned::blink::Descender;
use amphora_engine::storage::versioned::page::PAGE_SIZE;
use amphora_engine::storage::versioned::version::{Txn, Validation, Versioned};

use crate::{Hist, Rng, summary};

const CLASSES: [&str; 9] = [
    "insert_one",
    "insert_batch_100",
    "update_plain",
    "update_indexed",
    "update_range",
    "delete_insert",
    "delete_range_insert",
    "txn_read_write",
    "k0",
];

// K0's shape (bench/src/main.rs's defaults, docs/design/store.md).
const SESSIONS: u64 = 1000;
const PRELOAD: u64 = 200;
const READERS: usize = 4;
const PAGE_EVENTS: usize = 50;

const ITEMS: u64 = 0;
const INDEX: u64 = 1;
const EVENTS: u64 = 0;
const SESSION_ROWS: u64 = 1;

struct Opts {
    validations: Vec<Validation>,
    classes: Vec<String>,
    writers: usize,
    seconds: f64,
    items: u64,
    row_bytes: usize,
    text_bytes: usize,
}

fn opts() -> anyhow::Result<Opts> {
    let mut o = Opts {
        validations: vec![Validation::Precision, Validation::Ssi, Validation::Occ],
        classes: CLASSES.iter().map(|c| c.to_string()).collect(),
        writers: 32,
        seconds: 10.0,
        items: 200_000,
        row_bytes: 600,
        text_bytes: 600,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || {
            args.next()
                .ok_or_else(|| anyhow::anyhow!("{a} takes a value"))
        };
        match a.as_str() {
            "--cc" => {}
            "--validation" => {
                o.validations = v()?
                    .split(',')
                    .map(|s| match s {
                        "snapshot" => Ok(Validation::Snapshot),
                        "precision" => Ok(Validation::Precision),
                        "ssi" => Ok(Validation::Ssi),
                        "occ" => Ok(Validation::Occ),
                        other => Err(anyhow::anyhow!(
                            "no validation {other}: snapshot, precision, ssi or occ"
                        )),
                    })
                    .collect::<anyhow::Result<_>>()?
            }
            "--classes" => {
                let c = v()?;
                if c != "all" {
                    o.classes = c.split(',').map(str::to_owned).collect();
                    if let Some(bad) = o.classes.iter().find(|c| !CLASSES.contains(&c.as_str())) {
                        anyhow::bail!("no class {bad}: {}", CLASSES.join(", "));
                    }
                }
            }
            "--writers" => o.writers = v()?.parse()?,
            "--seconds" => o.seconds = v()?.parse()?,
            "--items" => o.items = v()?.parse()?,
            "--row-bytes" => o.row_bytes = v()?.parse()?,
            "--text-bytes" => o.text_bytes = v()?.parse()?,
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    Ok(o)
}

fn e(err: LimboError) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}

fn k(id: u64) -> [u8; 8] {
    id.to_be_bytes()
}

/// An index entry: the indexed value, then the row's id.
fn ix(value: u64, id: u64) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&value.to_be_bytes());
    out[8..].copy_from_slice(&id.to_be_bytes());
    out
}

/// A row: its indexed value first, then filler to `bytes`.
fn row(indexed: u64, bytes: usize) -> Vec<u8> {
    let mut out = indexed.to_be_bytes().to_vec();
    out.resize(bytes.max(8), 7);
    out
}

fn indexed_of(row: &[u8]) -> u64 {
    row.get(..8)
        .and_then(|b| b.try_into().ok())
        .map_or(0, u64::from_be_bytes)
}

fn event_key(session: u64, seq: u64) -> [u8; 16] {
    ix(session, seq)
}

/// A database of `trees` trees in a pool of as many pages as the memory this machine has
/// available now holds (the storage keeps every page in memory until eviction comes: a frame's
/// page costs memory once it is used). A run that fills it ends there, and says so.
fn database(validation: Validation, trees: usize) -> anyhow::Result<Versioned> {
    let meminfo = std::fs::read_to_string("/proc/meminfo")
        .map_err(|err| anyhow::anyhow!("--cc sizes its pool by /proc/meminfo (Linux): {err}"))?;
    let kib: u64 = meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemAvailable:"))
        .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse().ok())
        .ok_or_else(|| anyhow::anyhow!("/proc/meminfo has no MemAvailable"))?;
    let frames = usize::try_from(kib * 1024 / PAGE_SIZE as u64)?;
    Ok(
        Versioned::new(frames, trees, |_| Box::new(|a: &[u8], b: &[u8]| a.cmp(b)))
            .map_err(e)?
            .with_validation(validation),
    )
}

/// What a run counted, across its writers.
#[derive(Default)]
struct Counts {
    commits: u64,
    conflicts: u64,
    refused: u64,
}

/// Runs `body` in a transaction until it commits, counting what refused it: the first writer's
/// win (`WriteWriteConflict`) or validation (`SerializationFailure`). False: the pool is full.
fn until_committed(
    v: &Versioned,
    d: &mut Descender,
    c: &mut Counts,
    mut body: impl FnMut(&Versioned, &mut Descender, &mut Txn) -> Result<(), LimboError>,
) -> anyhow::Result<bool> {
    loop {
        let mut txn = v.clock.begin().map_err(e)?;
        match body(v, d, &mut txn) {
            Ok(()) => {}
            Err(LimboError::WriteWriteConflict) => {
                c.conflicts += 1;
                v.abort(txn, d).map_err(e)?;
                continue;
            }
            Err(LimboError::DatabaseFull) => {
                v.abort(txn, d).map_err(e)?;
                return Ok(false);
            }
            Err(err) => {
                v.abort(txn, d).map_err(e)?;
                return Err(e(err));
            }
        }
        match v.commit(txn, d) {
            Ok(_) => {
                c.commits += 1;
                return Ok(true);
            }
            Err(LimboError::SerializationFailure) => c.refused += 1,
            Err(err) => return Err(e(err)),
        }
    }
}

/// Writer `w` of `o.writers`, with its own random draws.
struct Writer<'a> {
    w: usize,
    rng: &'a mut Rng,
}

/// One write class's transaction by writer `w` of `n`, ids `next` handing out new rows' ids.
fn class_txn(
    class: &str,
    o: &Opts,
    Writer { w, rng }: Writer<'_>,
    next: &AtomicU64,
    v: &Versioned,
    d: &mut Descender,
    c: &mut Counts,
) -> anyhow::Result<bool> {
    let per = (o.items / o.writers.max(1) as u64).max(1);
    let first = per * w as u64;
    let own = |rng: &mut Rng| first + rng.next() % per;
    let own_range = |rng: &mut Rng, len: u64| first + rng.next() % per.saturating_sub(len).max(1);
    let (items, index) = (v.tree(ITEMS).map_err(e)?, v.tree(INDEX).map_err(e)?);
    let bytes = o.row_bytes;
    match class {
        "insert_one" | "insert_batch_100" => {
            let n = if class == "insert_one" { 1 } else { 100 };
            let ids: Vec<u64> = (0..n)
                .map(|_| next.fetch_add(1, Ordering::Relaxed))
                .collect();
            until_committed(v, d, c, |v, d, t| {
                for id in &ids {
                    items.put(&v.pool, d, &v.clock, t, &k(*id), &row(*id, bytes))?;
                    index.put(&v.pool, d, &v.clock, t, &ix(*id, *id), &[])?;
                }
                Ok(())
            })
        }
        "update_plain" => {
            let id = own(rng);
            until_committed(v, d, c, |v, d, t| {
                let r = items
                    .read(&v.pool, d, &v.clock, t, &k(id))?
                    .unwrap_or_default();
                items.put(&v.pool, d, &v.clock, t, &k(id), &row(indexed_of(&r), bytes))?;
                Ok(())
            })
        }
        "update_indexed" => {
            let (id, value) = (own(rng), rng.next());
            until_committed(v, d, c, |v, d, t| {
                let r = items
                    .read(&v.pool, d, &v.clock, t, &k(id))?
                    .unwrap_or_default();
                index.remove(&v.pool, d, &v.clock, t, &ix(indexed_of(&r), id))?;
                index.put(&v.pool, d, &v.clock, t, &ix(value, id), &[])?;
                items.put(&v.pool, d, &v.clock, t, &k(id), &row(value, bytes))?;
                Ok(())
            })
        }
        "update_range" => {
            let a = own_range(rng, 10);
            until_committed(v, d, c, |v, d, t| {
                for (key, r) in items.read_range(&v.pool, d, &v.clock, t, Some(&k(a)), 10)? {
                    items.put(&v.pool, d, &v.clock, t, &key, &r)?;
                }
                Ok(())
            })
        }
        "delete_insert" => {
            let id = own(rng);
            until_committed(v, d, c, |v, d, t| {
                let r = items
                    .read(&v.pool, d, &v.clock, t, &k(id))?
                    .unwrap_or_default();
                items.remove(&v.pool, d, &v.clock, t, &k(id))?;
                items.put(&v.pool, d, &v.clock, t, &k(id), &r)?;
                Ok(())
            })
        }
        "delete_range_insert" => {
            let a = own_range(rng, 10);
            until_committed(v, d, c, |v, d, t| {
                let rows = items.read_range(&v.pool, d, &v.clock, t, Some(&k(a)), 10)?;
                for (key, _) in &rows {
                    items.remove(&v.pool, d, &v.clock, t, key)?;
                }
                for (key, r) in &rows {
                    items.put(&v.pool, d, &v.clock, t, key, r)?;
                }
                Ok(())
            })
        }
        "txn_read_write" => {
            // The suite's: two rows of the client's own read and updated, one inserted.
            let (a, b, new) = (own(rng), own(rng), next.fetch_add(1, Ordering::Relaxed));
            until_committed(v, d, c, |v, d, t| {
                for id in [a, b] {
                    let r = items
                        .read(&v.pool, d, &v.clock, t, &k(id))?
                        .unwrap_or_default();
                    items.put(&v.pool, d, &v.clock, t, &k(id), &row(indexed_of(&r), bytes))?;
                }
                items.put(&v.pool, d, &v.clock, t, &k(new), &row(new, bytes))?;
                index.put(&v.pool, d, &v.clock, t, &ix(new, new), &[])?;
                Ok(())
            })
        }
        other => anyhow::bail!("no class {other}"),
    }
}

/// The write classes' database: `items` rows and their index, a commit a row.
fn load_items(v: &Versioned, o: &Opts) -> anyhow::Result<()> {
    let (items, index) = (v.tree(ITEMS).map_err(e)?, v.tree(INDEX).map_err(e)?);
    let mut d = Descender::new();
    for id in 0..o.items {
        let mut t = v.clock.begin().map_err(e)?;
        items
            .put(
                &v.pool,
                &mut d,
                &v.clock,
                &mut t,
                &k(id),
                &row(id, o.row_bytes),
            )
            .map_err(e)?;
        index
            .put(&v.pool, &mut d, &v.clock, &mut t, &ix(id, id), &[])
            .map_err(e)?;
        v.commit(t, &mut d).map_err(e)?;
    }
    v.collect(&mut d).map_err(e)?;
    Ok(())
}

/// K0's database: each session's row and its preloaded events.
fn load_k0(v: &Versioned, o: &Opts) -> anyhow::Result<()> {
    let (events, sessions) = (v.tree(EVENTS).map_err(e)?, v.tree(SESSION_ROWS).map_err(e)?);
    let mut d = Descender::new();
    for s in 0..SESSIONS {
        let mut t = v.clock.begin().map_err(e)?;
        sessions
            .put(
                &v.pool,
                &mut d,
                &v.clock,
                &mut t,
                &k(s),
                &PRELOAD.to_be_bytes(),
            )
            .map_err(e)?;
        for seq in 0..PRELOAD {
            events
                .put(
                    &v.pool,
                    &mut d,
                    &v.clock,
                    &mut t,
                    &event_key(s, seq),
                    &vec![b'e'; o.text_bytes],
                )
                .map_err(e)?;
        }
        v.commit(t, &mut d).map_err(e)?;
    }
    v.collect(&mut d).map_err(e)?;
    Ok(())
}

/// One K0 write: an event appended to `session`, its row's count read and written.
fn k0_write(
    o: &Opts,
    session: u64,
    v: &Versioned,
    d: &mut Descender,
    c: &mut Counts,
) -> anyhow::Result<bool> {
    let (events, sessions) = (v.tree(EVENTS).map_err(e)?, v.tree(SESSION_ROWS).map_err(e)?);
    let text = vec![b'e'; o.text_bytes];
    until_committed(v, d, c, |v, d, t| {
        let n = sessions
            .read(&v.pool, d, &v.clock, t, &k(session))?
            .and_then(|r| r.try_into().ok())
            .map_or(0, u64::from_be_bytes);
        events.put(&v.pool, d, &v.clock, t, &event_key(session, n), &text)?;
        sessions.put(&v.pool, d, &v.clock, t, &k(session), &(n + 1).to_be_bytes())?;
        Ok(())
    })
}

/// One K0 read: a session's latest 50 events, in a transaction.
fn k0_read(session: u64, v: &Versioned, d: &mut Descender, c: &mut Counts) -> anyhow::Result<bool> {
    let (events, sessions) = (v.tree(EVENTS).map_err(e)?, v.tree(SESSION_ROWS).map_err(e)?);
    until_committed(v, d, c, |v, d, t| {
        let n = sessions
            .read(&v.pool, d, &v.clock, t, &k(session))?
            .and_then(|r| r.try_into().ok())
            .map_or(0, u64::from_be_bytes);
        let from = n.saturating_sub(PAGE_EVENTS as u64);
        events.read_range(
            &v.pool,
            d,
            &v.clock,
            t,
            Some(&event_key(session, from)),
            PAGE_EVENTS,
        )?;
        Ok(())
    })
}

pub fn main() -> anyhow::Result<()> {
    let o = Arc::new(opts()?);
    for &validation in &o.validations {
        for class in &o.classes {
            let k0 = class == "k0";
            let v = database(validation, 2)?;
            if k0 {
                load_k0(&v, &o)?;
            } else {
                load_items(&v, &o)?;
            }
            let v = Arc::new(v);
            let stop = Arc::new(crate::Stop::default());
            // When the pool filled, in microseconds from the start (0: it didn't).
            let full = Arc::new(AtomicU64::new(0));
            let next = Arc::new(AtomicU64::new(o.items));
            let began = Instant::now();
            let spawn = |w: usize, reader: bool| {
                let (v, stop, full, next, o, class) = (
                    v.clone(),
                    stop.clone(),
                    full.clone(),
                    next.clone(),
                    o.clone(),
                    class.clone(),
                );
                std::thread::spawn(move || -> anyhow::Result<(Counts, Hist)> {
                    let mut d = Descender::new();
                    let mut rng = Rng(0x9e37_79b9_7f4a_7c15
                        ^ (w as u64 + 1).wrapping_mul(0xbf58_476d_1ce4_e5b9)
                        | 1);
                    let (mut c, mut h) = (Counts::default(), Hist::new());
                    while !stop.stopped() {
                        let at = Instant::now();
                        let session = rng.next() % SESSIONS;
                        let room = match (k0, reader) {
                            (true, true) => k0_read(session, &v, &mut d, &mut c)?,
                            (true, false) => k0_write(&o, session, &v, &mut d, &mut c)?,
                            _ => class_txn(
                                &class,
                                &o,
                                Writer { w, rng: &mut rng },
                                &next,
                                &v,
                                &mut d,
                                &mut c,
                            )?,
                        };
                        if !room {
                            let _ = full.compare_exchange(
                                0,
                                u64::try_from(began.elapsed().as_micros())
                                    .unwrap_or(u64::MAX)
                                    .max(1),
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            );
                            stop.stop();
                            break;
                        }
                        h.record(u64::try_from(at.elapsed().as_micros()).unwrap_or(u64::MAX));
                    }
                    Ok((c, h))
                })
            };
            let writers: Vec<_> = (0..o.writers).map(|w| spawn(w, false)).collect();
            let readers: Vec<_> = if k0 {
                (0..READERS).map(|r| spawn(o.writers + r, true)).collect()
            } else {
                Vec::new()
            };
            stop.wait_until(Instant::now() + Duration::from_secs_f64(o.seconds));
            stop.stop();
            let filled = full.load(Ordering::Acquire);
            let window = if filled == 0 {
                began.elapsed().as_secs_f64()
            } else {
                filled as f64 / 1e6
            };
            let gather = |handles: Vec<std::thread::JoinHandle<anyhow::Result<(Counts, Hist)>>>| -> anyhow::Result<(Counts, Hist)> {
                let (mut all, mut hist) = (Counts::default(), Hist::new());
                for h in handles {
                    let (c, h) = h.join().map_err(|_| anyhow::anyhow!("a thread panicked"))??;
                    all.commits += c.commits;
                    all.conflicts += c.conflicts;
                    all.refused += c.refused;
                    hist.merge(&h);
                }
                Ok((all, hist))
            };
            let (w, wh) = gather(writers)?;
            let (r, rh) = gather(readers)?;
            let share = |c: &Counts| {
                let attempts = c.commits + c.conflicts + c.refused;
                if attempts == 0 {
                    0.0
                } else {
                    (c.conflicts + c.refused) as f64 / attempts as f64
                }
            };
            println!(
                "{}",
                serde_json::json!({
                    "validation": format!("{validation:?}").to_lowercase(),
                    "class": class,
                    "writers": o.writers,
                    "seconds": window,
                    "commits_per_s": (w.commits as f64 / window).round(),
                    "conflicts": w.conflicts,
                    "refused": w.refused,
                    "refused_share": share(&w),
                    "latency_ms": summary(wh),
                    "reads_per_s": if k0 { serde_json::json!((r.commits as f64 / window).round()) } else { serde_json::json!(null) },
                    "reads_refused": if k0 { serde_json::json!(r.refused) } else { serde_json::json!(null) },
                    "read_latency_ms": if k0 { summary(rh) } else { serde_json::json!(null) },
                    "recent_commits": v.recent_commits(),
                    "pool_full": filled != 0,
                })
            );
        }
    }
    Ok(())
}
