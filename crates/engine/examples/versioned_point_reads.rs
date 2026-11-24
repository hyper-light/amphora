//! AUDIT 2.4 A/B: autocommit point SELECTs by primary key from `clients` threads, each on its own
//! connection, against the versioned storage (`versioned`) or the default one (`default`), on a
//! file database. Prints one line: storage, rows, clients, reads, seconds, reads per second.
//!
//! usage: versioned_point_reads <versioned|default> <dir> <rows> <clients> <reads per client>
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_macros)]

use amphora_engine::{
    ConnectOpts, Database, DatabaseOpts, OpenOptions, PlatformIO, SqliteDialect, StepResult, Value,
};
use std::error::Error;
use std::num::NonZeroUsize;
use std::sync::{mpsc, Arc, RwLock};
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn main() -> Result<()> {
    let usage = "usage: versioned_point_reads <versioned|default> <dir> <rows> <clients> <reads per client>";
    let mut args = std::env::args().skip(1);
    let storage = args.next().ok_or(usage)?;
    let versioned = match storage.as_str() {
        "versioned" => true,
        "default" => false,
        _ => return Err(usage.into()),
    };
    let dir = std::path::PathBuf::from(args.next().ok_or(usage)?);
    let rows: i64 = args.next().ok_or(usage)?.parse()?;
    let clients: usize = args.next().ok_or(usage)?.parse()?;
    let reads: usize = args.next().ok_or(usage)?.parse()?;
    if rows <= 0 || clients == 0 || reads == 0 {
        return Err(usage.into());
    }
    let row_count = u128::try_from(rows)?;
    let expected = clients.checked_mul(reads).ok_or("read count overflow")?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("reads-{storage}.db"));
    for suffix in ["", "-wal", "-shm"] {
        let file = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
        if file.try_exists()? {
            std::fs::remove_file(file)?;
        }
    }
    let side = std::path::PathBuf::from(format!("{}-versioned", path.display()));
    if side.try_exists()? {
        std::fs::remove_dir_all(side)?;
    }
    let io = Arc::new(PlatformIO::new()?);
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(
        io,
        path.to_str().ok_or("database path is not UTF-8")?,
        options,
    )?;
    let opts = || {
        if versioned {
            ConnectOpts::new().with_versioned_storage()
        } else {
            ConnectOpts::new()
        }
    };
    let conn = db.connect(opts())?;
    conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)")?;
    conn.execute("BEGIN")?;
    let values: Arc<Vec<_>> = Arc::new((0..rows).map(|id| format!("value-{id:08}")).collect());
    for (id, value) in values.iter().enumerate() {
        conn.execute(format!("INSERT INTO t VALUES ({id}, '{value}')"))?;
    }
    conn.execute("COMMIT")?;

    let gate = Arc::new(RwLock::new(()));
    let closed = gate.write().map_err(|_| "start lock poisoned")?;
    let (ready, prepared) = mpsc::channel();
    let mut workers = Vec::new();
    for c in 0..clients {
        let conn = db.connect(opts())?;
        let gate = Arc::clone(&gate);
        let ready = ready.clone();
        let values = Arc::clone(&values);
        workers.push(std::thread::Builder::new().spawn(move || -> Result<usize> {
            let mut stmt = conn.prepare("SELECT v FROM t WHERE id = ?")?;
            let mut key = (c as u128 * 7919) % row_count;
            ready.send(())?;
            drop(ready);
            drop(gate.read().map_err(|_| "start lock poisoned")?);
            let mut found = 0;
            for _ in 0..reads {
                stmt.reset()?;
                stmt.bind_at(NonZeroUsize::MIN, Value::from_i64(i64::try_from(key)?))?;
                let want = values
                    .get(usize::try_from(key)?)
                    .ok_or("key out of range")?;
                let mut received = false;
                loop {
                    match stmt.step()? {
                        StepResult::Row => {
                            let value = stmt.row().and_then(|row| row.get_value(0).to_text());
                            if received || value != Some(want.as_str()) {
                                return Err(format!("wrong or duplicate row for key {key}").into());
                            }
                            received = true;
                            found += 1;
                        }
                        StepResult::Done if received => break,
                        StepResult::Done => return Err(format!("missing key {key}").into()),
                        StepResult::IO | StepResult::Yield | StepResult::Sleep { .. } => {
                            stmt._io().step()?
                        }
                        StepResult::Busy => return Err("point read busy".into()),
                        StepResult::Interrupt => return Err("point read interrupted".into()),
                    }
                }
                key = (key + 104_729) % row_count;
            }
            Ok(found)
        })?);
    }
    drop(ready);
    let prepared = prepared.into_iter().count();
    let start = Instant::now();
    drop(closed);
    let mut total = 0usize;
    for worker in workers {
        total += worker.join().map_err(|_| "point-read worker failed")??;
    }
    let secs = start.elapsed().as_secs_f64();
    if prepared != clients || total != expected {
        return Err(
            format!("{prepared}/{clients} clients prepared, {total}/{expected} reads").into(),
        );
    }
    println!(
        "storage={storage} rows={rows} clients={clients} reads={total} seconds={secs:.3} reads_per_s={:.0}",
        total as f64 / secs
    );
    Ok(())
}
