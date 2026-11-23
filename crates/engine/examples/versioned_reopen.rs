//! AUDIT 7.5 (perf/versioned): times reopening a versioned database whose last checkpoint
//! finished, as a process start after a clean shutdown does.
//!
//! `cargo run --release -p amphora-engine --example versioned_reopen -- <dir> <rows> <reopens>`:
//! makes `<rows>` rows of 100 bytes under `<dir>`, one commit each, flushed and checkpointed, then
//! reopens the same files `<reopens>` times. Prints the reopens' minimum, median and maximum in
//! microseconds, and the rows each reopen read back.

use amphora_engine::storage::versioned::blink::{Comparator, Descender};
use amphora_engine::storage::versioned::disk::Files;
use amphora_engine::storage::versioned::durable::Database;
use std::sync::Arc;
use std::time::Instant;

fn cmp() -> Comparator {
    Box::new(|a: &[u8], b: &[u8]| a.cmp(b))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let usage = "usage: versioned_reopen <dir> <rows> <reopens>";
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().ok_or(usage)?);
    let rows: u64 = args.next().ok_or(usage)?.parse()?;
    let reopens: usize = args.next().ok_or(usage)?.parse()?;
    std::fs::create_dir_all(&dir)?;
    // Every row resident: a page holds at least one.
    let frames = usize::try_from(rows)?;
    {
        let db = Database::open(Arc::new(Files::open(&dir, 1)?), frames, 1, cmp)?;
        let mut d = Descender::new();
        let lane = db.lane();
        for key in 0..rows {
            let mut t = db.begin()?;
            db.put(&mut d, &mut t, 0, &key.to_be_bytes(), &[7; 100])?;
            db.commit(t, &mut d, lane, false)?;
        }
        db.sync()?;
        db.checkpoint()?;
    }
    let mut micros = Vec::with_capacity(reopens);
    let mut read = Vec::with_capacity(reopens);
    for _ in 0..reopens {
        let start = Instant::now();
        let db = Database::open(Arc::new(Files::open(&dir, 1)?), frames, 1, cmp)?;
        micros.push(start.elapsed().as_secs_f64() * 1e6);
        let snap = db.snapshot()?;
        read.push(db.scan(&mut Descender::new(), &snap, 0, None, usize::MAX)?.len());
        db.close_snapshot(snap);
    }
    micros.sort_by(f64::total_cmp);
    let at = |i: usize| micros.get(i).copied().unwrap_or(f64::NAN);
    println!(
        "reopen: n={reopens} min={:.1}us median={:.1}us max={:.1}us rows_read={read:?}",
        at(0),
        at(reopens / 2),
        at(reopens.saturating_sub(1)),
    );
    Ok(())
}
