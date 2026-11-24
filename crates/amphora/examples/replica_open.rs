//! AUDIT §7.6 (perf-H3): what a replicated member's open spends on its image. `fill` makes a
//! database file of about `<gib>` GiB; `open` opens a `Machine` on it and takes what
//! `hyper_durable::Replica::open` takes from it for a log compacted before the restart: `image`,
//! the whole image made into memory (what its `prepare` did at open), or `count`, the image's
//! bytes as the machine states them (what it asks now). One open a process, so its peak resident
//! memory is the open's own.
//!
//! `cargo run --release -p amphora --example replica_open -- fill <path> <gib>`
//! `cargo run --release -p amphora --example replica_open -- open <path> image|count`
#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use amphora::db::Db;
use amphora::replication::Machine;
use hyper_durable::StateMachine;
use hyper_raft::proto::ConfState;

fn main() -> ExitCode {
    match run() {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("replica_open: {why}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<String, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [mode, path, gib] if mode == "fill" => fill(
            PathBuf::from(path),
            gib.parse().map_err(|e| format!("{e}"))?,
        ),
        [mode, path, step] if mode == "open" => open(PathBuf::from(path), step),
        _ => Err("usage: replica_open fill <path> <gib> | open <path> image|count".into()),
    }
}

fn fill(path: PathBuf, gib: u64) -> Result<String, String> {
    // The machine's own tables first: a member's database, not a local one it would import.
    drop(Machine::open(&path, ConfState::default()).map_err(|e| e.to_string())?);
    let db = Db::open(&path).map_err(|e| e.to_string())?;
    db.exec("CREATE TABLE IF NOT EXISTS t(b)")
        .map_err(|e| e.to_string())?;
    let blob = 1u64 << 16;
    let rows = (gib << 30) / blob;
    let batch = 1024;
    let mut written = 0;
    while written < rows {
        db.exec("BEGIN").map_err(|e| e.to_string())?;
        for _ in 0..batch.min(rows - written) {
            db.exec(&format!("INSERT INTO t VALUES(randomblob({blob}))"))
                .map_err(|e| e.to_string())?;
        }
        db.exec("COMMIT").map_err(|e| e.to_string())?;
        written += batch.min(rows - written);
    }
    let mut db = db;
    db.checkpoint().map_err(|e| e.to_string())?;
    let bytes = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    Ok(format!("filled {} rows, {bytes} bytes", written))
}

fn open(path: PathBuf, step: &str) -> Result<String, String> {
    let began = Instant::now();
    let mut machine = Machine::open(&path, ConfState::default()).map_err(|e| e.to_string())?;
    let opened = began.elapsed();
    let bytes = match step {
        "image" => {
            let mut image = Vec::new();
            machine.image(&mut image).map_err(|e| e.to_string())?;
            image.len() as u64
        }
        "count" => machine.image_bytes().ok_or("no image")?,
        _ => return Err(format!("{step}: image or count")),
    };
    let total = began.elapsed();
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let hwm = status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("?");
    Ok(format!(
        "{step} machine_open_ms={:.1} open_ms={:.1} image_bytes={bytes} peak_rss_kib={hwm}",
        opened.as_secs_f64() * 1e3,
        total.as_secs_f64() * 1e3
    ))
}
