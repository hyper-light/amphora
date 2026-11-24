//! AUDIT §7 item 1 (perf/coldstart): times a process's database opens as Amphora makes them (a new
//! `PlatformIO` each, `crates/amphora/src/db.rs`), and each open's first statement.
//!
//! `cargo run --release -p amphora-engine --example open_time -- <dir> <opens>`: opens `<opens>`
//! new database files in a fresh directory under `<dir>`, then reopens each, and prints each phase's
//! median and maximum in microseconds.

use amphora_engine::{ConnectOpts, Database, OpenOptions, PlatformIO, SqliteDialect, IO};
use std::sync::Arc;
use std::time::Instant;

fn summary(name: &str, micros: &mut [f64]) {
    micros.sort_by(f64::total_cmp);
    let median = micros.get(micros.len() / 2).copied().unwrap_or(f64::NAN);
    let max = micros.last().copied().unwrap_or(f64::NAN);
    let first = micros.first().copied().unwrap_or(f64::NAN);
    println!(
        "{name}: n={} min={first:.1}us median={median:.1}us max={max:.1}us",
        micros.len()
    );
}

fn open(path: &str) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let io: Arc<dyn IO> = Arc::new(PlatformIO::new()?);
    let db = Database::open(io, path, OpenOptions::new(Arc::new(SqliteDialect)))?;
    let opened = start.elapsed().as_secs_f64() * 1e6;
    let start = Instant::now();
    let conn = db.connect(ConnectOpts::new())?;
    conn.execute("CREATE TABLE IF NOT EXISTS t (id INTEGER PRIMARY KEY, body TEXT)")?;
    let first = start.elapsed().as_secs_f64() * 1e6;
    Ok((opened, first))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().ok_or("usage: open_time <dir> <opens>")?;
    let opens: usize = args.next().ok_or("usage: open_time <dir> <opens>")?.parse()?;
    if opens == 0 {
        return Err("open_time needs at least one measured open".into());
    }
    std::fs::create_dir_all(&dir)?;
    let fixture = tempfile::tempdir_in(&dir)?;
    let dir = fixture.path().to_str().ok_or("database path is not UTF-8")?;
    let start = Instant::now();
    let (process_first, _) = open(&format!("{dir}/first.db"))?;
    println!(
        "process's first open: {process_first:.1}us (wall {:.1}us)",
        start.elapsed().as_secs_f64() * 1e6
    );
    for phase in ["new", "reopen"] {
        let mut opened = Vec::with_capacity(opens);
        let mut first = Vec::with_capacity(opens);
        for i in 0..opens {
            let (o, f) = open(&format!("{dir}/open-{i}.db"))?;
            opened.push(o);
            first.push(f);
        }
        summary(&format!("{phase} open"), &mut opened);
        summary(&format!("{phase} first statement"), &mut first);
    }
    Ok(())
}
