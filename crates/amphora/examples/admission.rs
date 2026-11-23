//! The audit's §1 measurement (AUDIT 2026-11-23, cross-cutting): what reading the memory budget
//! costs a request, and what a request through `Database` costs on a ladder of clients. Every local
//! request (`Database::call`, `execute`, Records) is admitted by charging the budget's room for
//! queues (MB.1), which reads the process's limit and resident bytes ([`amphora::memory`]).
//!
//! cargo run --release -p amphora --example admission -- <dir> <reads> <seconds> <clients,...>
//!
//! One line a measurement: the budget's `room` in ns a call (best of five rounds of <reads>), then,
//! for each client count, `Database::call`s of an empty closure a second over <seconds> seconds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use amphora::database::Database;
use amphora::memory::{self, Use};

fn main() {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.into_string().expect("arguments are Unicode"))
        .collect();
    let usage = "usage: admission <dir> <reads> <seconds> <clients,...>";
    let dir = std::path::PathBuf::from(args.first().expect(usage));
    let reads: u32 = args.get(1).expect(usage).parse().expect(usage);
    let seconds: f64 = args.get(2).expect(usage).parse().expect(usage);
    let ladder: Vec<usize> = args
        .get(3)
        .expect(usage)
        .split(',')
        .map(|n| n.parse().expect(usage))
        .collect();

    let budget = memory::process();
    let mut best = f64::MAX;
    for _ in 0..5 {
        let started = Instant::now();
        for _ in 0..reads {
            black_box(budget.room(Use::Queues).expect("room"));
        }
        best = best.min(started.elapsed().as_nanos() as f64 / f64::from(reads));
    }
    println!("{{\"what\":\"room\",\"ns_per_call\":{best:.1}}}");

    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("admission.db");
    let _ = std::fs::remove_file(&path);
    let until = || Instant::now() + Duration::from_secs(600);
    let db = Database::open(&path, until()).expect("open");
    for clients in ladder {
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicU64::new(0));
        let refused = Arc::new(AtomicU64::new(0));
        let threads: Vec<_> = (0..clients)
            .map(|_| {
                let (db, stop, done, refused) =
                    (db.clone(), stop.clone(), done.clone(), refused.clone());
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        match db.call(Instant::now() + Duration::from_secs(60), |_| Ok(())) {
                            Ok(()) => done.fetch_add(1, Ordering::Relaxed),
                            Err(_) => refused.fetch_add(1, Ordering::Relaxed),
                        };
                    }
                })
            })
            .collect();
        // Warm up a second, then count.
        std::thread::sleep(Duration::from_secs(1));
        let (d0, r0) = (done.load(Ordering::Relaxed), refused.load(Ordering::Relaxed));
        let started = Instant::now();
        std::thread::sleep(Duration::from_secs_f64(seconds));
        let elapsed = started.elapsed().as_secs_f64();
        let (d1, r1) = (done.load(Ordering::Relaxed), refused.load(Ordering::Relaxed));
        stop.store(true, Ordering::Relaxed);
        for t in threads {
            t.join().expect("client");
        }
        println!(
            "{{\"what\":\"call\",\"clients\":{clients},\"per_s\":{:.0},\"refused_per_s\":{:.0}}}",
            (d1 - d0) as f64 / elapsed,
            (r1 - r0) as f64 / elapsed
        );
    }
}
