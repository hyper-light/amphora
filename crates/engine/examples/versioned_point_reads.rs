//! AUDIT 2.4 A/B: autocommit point SELECTs by primary key from `clients` threads, each on its own
//! connection, against the versioned storage (`versioned`) or the default one (`default`), on a
//! file database. Prints one line: storage, rows, clients, reads, seconds, reads per second.
//!
//! usage: versioned_point_reads <versioned|default> <dir> <rows> <clients> <reads per client>
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_macros)]

use amphora_engine::{
    ConnectOpts, Database, DatabaseOpts, OpenOptions, PlatformIO, SqliteDialect, StepResult,
    Value,
};
use std::num::NonZero;
use std::sync::{Arc, Barrier};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let versioned = args[1] == "versioned";
    let dir = std::path::PathBuf::from(&args[2]);
    let rows: i64 = args[3].parse().unwrap();
    let clients: usize = args[4].parse().unwrap();
    let reads: usize = args[5].parse().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("reads-{}.db", args[1]));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let _ = std::fs::remove_dir_all(format!("{}.versioned", path.display()));
    let io = Arc::new(PlatformIO::new().unwrap());
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io, path.to_str().unwrap(), options).unwrap();
    let opts = || {
        if versioned {
            ConnectOpts::new().with_versioned_storage()
        } else {
            ConnectOpts::new()
        }
    };
    let conn = db.connect(opts()).unwrap();
    conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)")
        .unwrap();
    conn.execute("BEGIN").unwrap();
    for id in 0..rows {
        conn.execute(format!("INSERT INTO t VALUES ({id}, 'value-{id:08}')"))
            .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    let barrier = Arc::new(Barrier::new(clients + 1));
    let handles: Vec<_> = (0..clients)
        .map(|c| {
            let conn = db.connect(opts()).unwrap();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut stmt = conn.prepare("SELECT v FROM t WHERE id = ?").unwrap();
                // A fixed stride walk over the keys, different per client.
                let mut key = (c as i64 * 7919) % rows;
                barrier.wait();
                let mut found = 0usize;
                for _ in 0..reads {
                    stmt.reset().unwrap();
                    stmt.bind_at(NonZero::new(1).unwrap(), Value::from_i64(key))
                        .unwrap();
                    loop {
                        match stmt.step().unwrap() {
                            StepResult::Row => found += 1,
                            StepResult::Done => break,
                            StepResult::IO | StepResult::Yield | StepResult::Sleep { .. } => {
                                stmt._io().step().unwrap()
                            }
                            StepResult::Busy => panic!("busy"),
                            StepResult::Interrupt => panic!("interrupt"),
                        }
                    }
                    key = (key + 104_729) % rows;
                }
                assert_eq!(found, reads);
            })
        })
        .collect();
    barrier.wait();
    let start = Instant::now();
    for h in handles {
        h.join().unwrap();
    }
    let secs = start.elapsed().as_secs_f64();
    let total = clients * reads;
    println!(
        "storage={} rows={rows} clients={clients} reads={total} seconds={secs:.3} reads_per_s={:.0}",
        args[1],
        total as f64 / secs
    );
}
