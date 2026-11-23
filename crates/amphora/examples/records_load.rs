//! The Records API under load (AUDIT 2.5 and 4, perf/writes): one-key gets and one-key commits
//! from 1 to 1,024 clients on a local database, each client a thread, every call durable.
//! One line of JSON a (operation, clients): calls answered, seconds, calls a second.
//!
//! `cargo run --release -p amphora --example records_load -- <empty folder> <seconds> <clients,...> [get|commit|both]`
#![allow(clippy::disallowed_macros, clippy::disallowed_methods)]

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use amphora::database::Database;
use amphora_api::{Consistency, Records, Tx, Write};

/// The keys the gets read: written once before the gets begin.
const KEYS: u64 = 10_000;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            let _ = writeln!(std::io::stderr(), "records_load: {why}");
            ExitCode::from(1)
        }
    }
}

fn until() -> Result<Instant, String> {
    Instant::now()
        .checked_add(Duration::from_secs(60))
        .ok_or_else(|| "the clock can't reach the deadline".to_owned())
}

fn key(i: u64) -> Vec<u8> {
    format!("k{i:08}").into_bytes()
}

fn put(i: u64) -> Tx {
    Tx {
        conds: Vec::new(),
        writes: vec![Write::Put {
            family: "f".into(),
            key: key(i),
            value: vec![7; 100],
        }],
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(
        args.next()
            .ok_or("usage: records_load <folder> <seconds> <clients,...> [get|commit|both]")?,
    );
    let seconds: u64 = args
        .next()
        .ok_or("seconds")?
        .parse()
        .map_err(|e| format!("seconds: {e}"))?;
    let clients = args
        .next()
        .ok_or("clients")?
        .split(',')
        .map(|c| c.parse::<u64>().map_err(|e| format!("clients: {e}")))
        .collect::<Result<Vec<_>, _>>()?;
    let which = args.next().unwrap_or_else(|| "both".into());
    let db = Database::open(&dir.join("records.db"), until()?).map_err(|e| e.to_string())?;
    let ns = db.namespace("bench").map_err(|e| e.to_string())?;
    for i in 0..KEYS {
        ns.commit(put(i), until()?).map_err(|e| e.to_string())?;
    }
    let ops: &[&str] = match which.as_str() {
        "get" => &["get"],
        "commit" => &["commit"],
        _ => &["get", "commit"],
    };
    for op in ops {
        for &n in &clients {
            let stop = AtomicBool::new(false);
            let done = AtomicU64::new(0);
            let failed = AtomicU64::new(0);
            let started = Instant::now();
            std::thread::scope(|s| {
                for c in 0..n {
                    let (ns, stop, done, failed) = (&ns, &stop, &done, &failed);
                    s.spawn(move || {
                        let mut i = c.wrapping_mul(7919);
                        while !stop.load(Ordering::Relaxed) {
                            i = i.wrapping_add(1);
                            let Ok(deadline) = until() else { break };
                            let answered = if *op == "get" {
                                ns.get("f", &key(i % KEYS), Consistency::Linearizable, deadline)
                                    .map(|_| ())
                            } else {
                                ns.commit(put(i % KEYS), deadline).map(|_| ())
                            };
                            match answered {
                                Ok(()) => done.fetch_add(1, Ordering::Relaxed),
                                Err(_) => failed.fetch_add(1, Ordering::Relaxed),
                            };
                        }
                    });
                }
                // The measuring window: the main thread waits on the clock, not a worker.
                let end = started.checked_add(Duration::from_secs(seconds));
                while end.is_some_and(|end| Instant::now() < end) {
                    std::thread::park_timeout(Duration::from_millis(100));
                }
                stop.store(true, Ordering::Relaxed);
            });
            let secs = started.elapsed().as_secs_f64();
            let calls = done.load(Ordering::Relaxed);
            #[allow(clippy::cast_precision_loss)]
            let rate = calls as f64 / secs;
            println!(
                "{{\"op\":\"{op}\",\"clients\":{n},\"calls\":{calls},\"failed\":{},\"seconds\":{secs:.3},\"per_second\":{rate:.0}}}",
                failed.load(Ordering::Relaxed)
            );
        }
    }
    Ok(())
}
