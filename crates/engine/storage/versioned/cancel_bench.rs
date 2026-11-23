//! Amphora (G7 step 2, `AC-NS.2.7`): what cancelling costs on the versioned storage, measured.
//! The harness prints what it measured and asserts only behaviour (every cancel it counts ended
//! its statement with `Interrupt`; a watched count equals an unwatched one), never the clock.
//!
//! - **Cancel-to-stop.** `statements` connections each run `SELECT count(*)` over one table of
//!   many leaves, again and again, side by side. The `Count` instruction walks every leaf inside
//!   one instruction, so only the cursor's check at each leaf stops it. A canceller interrupts
//!   one statement at a time, picked by a seeded generator; the time from `interrupt()` to that
//!   statement's step returning, over every cancel, at p50, p99 and p99.9. A cancel that lands
//!   after the statement's last check finishes its count instead, and is counted as missed.
//! - **The check's cost.** The same count over the same tree by a cursor watched by its
//!   connection (asking at each leaf) and by one unwatched, alternated A B B A over `rounds`
//!   rounds: each one's median time per row, and watched over unwatched.
//!
//! Sizes come from the environment: `AMPHORA_CANCEL_ROWS` (the table's rows),
//! `AMPHORA_CANCEL_STATEMENTS` (statements side by side), `AMPHORA_CANCEL_CANCELS` (cancels
//! measured) and `AMPHORA_CANCEL_ROUNDS` (watched and unwatched rounds). Without them it runs its
//! tiny instance, which only shows the harness runs: its numbers measure nothing. Run it with
//! `cargo test -p amphora-engine --lib --release -- --ignored --exact
//! storage::versioned::cancel_bench::ns_2_7_cancel_bench --nocapture`.

use crate::io::{MemoryIO, IO};
use crate::storage::btree::CursorTrait;
use crate::sync::atomic::{AtomicBool, Ordering};
use crate::sync::Arc;
use crate::types::IOResult;
use crate::{ConnectOpts, Connection, Database, LimboError, SqliteDialect, StepResult};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn size(name: &str, tiny: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(tiny)
}

/// The nearest-rank percentile `q` of sorted `samples`.
fn percentile(samples: &[Duration], q: f64) -> Duration {
    if samples.is_empty() {
        return Duration::ZERO;
    }
    let rank = (q * samples.len() as f64).ceil() as usize;
    samples
        .get(rank.clamp(1, samples.len()) - 1)
        .copied()
        .unwrap_or_default()
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    percentile(&samples, 0.5)
}

/// How a statement's run ended.
enum Ended {
    Counted,
    Interrupted,
}

/// Runs `SELECT count(*) FROM t` to its end on `conn`.
fn count(conn: &Arc<Connection>) -> Ended {
    let mut stmt = conn.prepare("SELECT count(*) FROM t").unwrap();
    loop {
        match stmt.step() {
            Ok(StepResult::Row) => {}
            Ok(StepResult::Done) => return Ended::Counted,
            Ok(StepResult::Interrupt) => return Ended::Interrupted,
            Ok(StepResult::IO | StepResult::Yield | StepResult::Sleep { .. }) => {
                stmt.get_pager().io.step().unwrap()
            }
            Ok(StepResult::Busy) => panic!("a read-only count was busy"),
            Err(LimboError::Interrupt) => return Ended::Interrupted,
            Err(e) => panic!("count: {e}"),
        }
    }
}

/// A versioned database whose table `t` holds `rows` rows of 100 bytes each: many leaves.
fn table(rows: usize) -> (Arc<Database>, Arc<Connection>, i64) {
    let io: Arc<dyn IO> = Arc::new(MemoryIO::new());
    let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
    let conn = db
        .connect(ConnectOpts::new().with_versioned_storage())
        .unwrap();
    conn.execute("CREATE TABLE t (x INTEGER, pad BLOB)")
        .unwrap();
    conn.execute("BEGIN").unwrap();
    for i in 0..rows {
        conn.execute(format!("INSERT INTO t VALUES ({i}, zeroblob(100))"))
            .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    let mut stmt = conn
        .prepare("SELECT rootpage FROM sqlite_schema WHERE name = 't'")
        .unwrap();
    let found = stmt.run_collect_rows().unwrap();
    let root = found
        .first()
        .and_then(|row| row.first())
        .map(|v| v.to_string().parse::<i64>().unwrap())
        .unwrap();
    (db, conn, root)
}

#[test]
#[ignore = "a measurement: run it by name (the module's doc)"]
fn ns_2_7_cancel_bench() {
    let rows = size("AMPHORA_CANCEL_ROWS", 2_000);
    let statements = size("AMPHORA_CANCEL_STATEMENTS", 4).max(1);
    let cancels = size("AMPHORA_CANCEL_CANCELS", 20);
    let rounds = size("AMPHORA_CANCEL_ROUNDS", 8);
    let (db, conn, root) = table(rows);

    // Cancel-to-stop, `statements` side by side.
    let stop = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();
    let mut conns = Vec::new();
    for _ in 0..statements {
        let c = db
            .connect(ConnectOpts::new().with_versioned_storage())
            .unwrap();
        conns.push(c.clone());
        let (tx, rx) = mpsc::channel::<(Ended, Instant)>();
        let stop = stop.clone();
        let handle = std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let ended = count(&c);
                if tx.send((ended, Instant::now())).is_err() {
                    break;
                }
            }
        });
        workers.push((handle, rx));
    }
    let mut rng = 0x9E37_79B9_7F4A_7C15u64;
    let mut stopped = Vec::with_capacity(cancels);
    let mut missed = 0usize;
    for _ in 0..cancels {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let k = (rng % statements as u64) as usize;
        let (c, (_, rx)) = (&conns[k], &workers[k]);
        // Ends already reported belong to runs before this cancel.
        while rx.try_recv().is_ok() {}
        // Asked only while a statement runs: SQLite's interrupt is ignored between them.
        let asked = loop {
            c.interrupt();
            let at = Instant::now();
            if c.is_interrupted() {
                break at;
            }
            std::thread::yield_now();
        };
        match rx.recv().unwrap() {
            (Ended::Interrupted, at) => stopped.push(at.saturating_duration_since(asked)),
            (Ended::Counted, _) => missed += 1,
        }
    }
    stop.store(true, Ordering::SeqCst);
    for (handle, rx) in workers {
        drop(rx);
        handle.join().unwrap();
    }
    stopped.sort_unstable();
    println!(
        "ns_2_7 cancel-to-stop: rows {rows}, statements {statements}, cancels {}, missed {missed}: \
         p50 {:?}, p99 {:?}, p99.9 {:?}, max {:?}",
        stopped.len(),
        percentile(&stopped, 0.5),
        percentile(&stopped, 0.99),
        percentile(&stopped, 0.999),
        stopped.last().copied().unwrap_or_default(),
    );

    // The per-leaf check's cost: watched against unwatched, A B B A.
    let switch = conn.versioned_for(crate::MAIN_DB_ID).unwrap();
    let pager = conn.pager.load().clone();
    let timed = |watched: bool| -> Duration {
        let cursor = switch.cursor(root, None, pager.clone()).unwrap();
        let mut cursor = if watched {
            cursor.watched_by(&conn)
        } else {
            cursor
        };
        let start = Instant::now();
        let n = cursor.count();
        let took = start.elapsed();
        assert!(matches!(n, Ok(IOResult::Done(n)) if n == rows), "{n:?}");
        took
    };
    let (mut watched, mut unwatched) = (Vec::new(), Vec::new());
    for round in 0..rounds {
        let order = if round % 2 == 0 {
            [true, false, false, true]
        } else {
            [false, true, true, false]
        };
        for w in order {
            let took = timed(w);
            if w {
                watched.push(took);
            } else {
                unwatched.push(took);
            }
        }
    }
    switch.finish(true).unwrap();
    let per_row = |d: Duration| d.as_nanos() as f64 / rows.max(1) as f64;
    let (w, u) = (median(watched), median(unwatched));
    println!(
        "ns_2_7 per-leaf check: rows {rows}, rounds {rounds}: watched {:.2} ns/row, unwatched \
         {:.2} ns/row, watched/unwatched {:.4}",
        per_row(w),
        per_row(u),
        per_row(w) / per_row(u).max(f64::MIN_POSITIVE),
    );
    assert_eq!(missed + stopped.len(), cancels);
}
