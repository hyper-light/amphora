//! The audit's §1 measurement (AUDIT 2026-11-23, cross-cutting): what reading the memory budget
//! costs a request, and what a request through `Database` costs on a ladder of clients. Every local
//! request (`Database::call`, `execute`, Records) is admitted by charging the budget's room for
//! queues (MB.1), which reads the process's limit and resident bytes ([`amphora::memory`]).
//!
//! cargo run --release -p amphora --example admission -- <dir> <reads> <seconds> <clients,...> [call|get]
//! cargo run --release -p amphora --example admission -- <dir> <reads> <seconds> <clients,...> ab <rounds>
//!
//! One line a measurement: the budget's `room` in ns a call (best of five rounds of <reads>), then,
//! for each client count, requests a second over <seconds> seconds: `Database::call`s of an empty
//! closure (`call`, the default), or Records gets of one absent key (`get`), and the refused ones.
//!
//! `ab` is Ada's A/B of a call's admission charge (AUDIT §1): each call reads `LIMIT` rows of a
//! seeded table, charged the flat `REQUEST_MAX` (`flat`, today's `Database::call`), its declared
//! bound (`declared`, arm a) or its measured answer (`measured`, arm b). The arms take turns,
//! round after round, each over the client ladder; a line a run gives every call's own latency's
//! p50, p99 and p99.9 (no sampling), calls a second, refusals and the machine's load and pressure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use amphora::database::{AnswerBytes, AnswerCharge, Database};
use amphora::memory::{self, Use};
use amphora_api::{Consistency, Records};

fn main() {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.into_string().expect("arguments are Unicode"))
        .collect();
    let usage = "usage: admission <dir> <reads> <seconds> <clients,...> [call|get]";
    let dir = std::path::PathBuf::from(args.first().expect(usage));
    let reads: u32 = args.get(1).expect(usage).parse().expect(usage);
    let seconds: f64 = args.get(2).expect(usage).parse().expect(usage);
    let ladder: Vec<usize> = args
        .get(3)
        .expect(usage)
        .split(',')
        .map(|n| n.parse().expect(usage))
        .collect();
    let get = match args.get(4).map(String::as_str) {
        None | Some("call") => false,
        Some("get") => true,
        Some("ab") => {
            let rounds: usize = args.get(5).expect(usage).parse().expect(usage);
            ab(&dir, seconds, &ladder, rounds);
            return;
        }
        Some(_) => panic!("{usage}"),
    };

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
                    let ns = db.namespace("admission").expect("namespace");
                    while !stop.load(Ordering::Relaxed) {
                        let until = Instant::now() + Duration::from_secs(60);
                        let answered = if get {
                            ns.get("f", b"k", Consistency::Linearizable, until).map(drop)
                        } else {
                            db.call(until, |_| Ok(()))
                        };
                        match answered {
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
            "{{\"what\":\"{}\",\"clients\":{clients},\"per_s\":{:.0},\"refused_per_s\":{:.0}}}",
            if get { "get" } else { "call" },
            (d1 - d0) as f64 / elapsed,
            (r1 - r0) as f64 / elapsed
        );
    }
}

const ROWS: i64 = 1_000;
const LIMIT: usize = 8;
const VALUE: usize = 64;

type Rows = Vec<(i64, String)>;

/// The most bytes the query's answer holds, from its own bounds: `LIMIT` rows of an id and a
/// value of `VALUE` bytes.
fn declared() -> usize {
    let row = (0i64, "x".repeat(VALUE)).answer_bytes();
    Vec::<(i64, String)>::with_capacity(LIMIT).answer_bytes() + LIMIT * row
}

fn read(c: &mut amphora::sql::Connection<'_>, from: i64) -> anyhow::Result<Rows> {
    let mut statement = c.prepare_cached("SELECT id, v FROM t WHERE id >= ?1 ORDER BY id LIMIT ?2")?;
    let rows = statement
        .query_map(
            vec![
                amphora_api::Value::Integer(from),
                amphora_api::Value::Integer(LIMIT as i64),
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    anyhow::ensure!(rows.len() == LIMIT, "read {} rows", rows.len());
    Ok(rows)
}

fn machine() -> String {
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let load = load.split_whitespace().take(3).collect::<Vec<_>>().join(" ");
    let pressure = |what: &str| {
        std::fs::read_to_string(format!("/proc/pressure/{what}"))
            .unwrap_or_default()
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("")
            .trim_start_matches("avg10=")
            .to_owned()
    };
    format!(
        "\"load\":\"{load}\",\"cpu_some10\":\"{}\",\"io_some10\":\"{}\"",
        pressure("cpu"),
        pressure("io")
    )
}

fn percentile(sorted: &[u64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let at = ((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[at] as f64 / 1_000.0
}

fn ab(dir: &std::path::Path, seconds: f64, ladder: &[usize], rounds: usize) {
    std::fs::create_dir_all(dir).expect("dir");
    let path = dir.join("admission-ab.db");
    let _ = std::fs::remove_file(&path);
    let until = || Instant::now() + Duration::from_secs(600);
    let db = Database::open(&path, until()).expect("open");
    db.call(until(), |c| {
        c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")?;
        for id in 0..ROWS {
            c.execute(
                "INSERT INTO t (id, v) VALUES (?1, ?2)",
                vec![
                    amphora_api::Value::Integer(id),
                    amphora_api::Value::Text(format!("{id:0width$}", width = VALUE)),
                ],
            )?;
        }
        Ok(())
    })
    .expect("seed");
    let declared = declared();
    println!("{{\"what\":\"ab\",\"declared_bytes\":{declared},\"rows\":{ROWS},\"limit\":{LIMIT}}}");
    let arms = [
        ("flat", None),
        ("declared", Some(AnswerCharge::Declared(declared))),
        ("measured", Some(AnswerCharge::Measured)),
    ];
    for round in 0..rounds {
        for (arm, charge) in arms {
            for &clients in ladder {
                let stop = Arc::new(AtomicBool::new(false));
                let measuring = Arc::new(AtomicBool::new(false));
                let threads: Vec<_> = (0..clients)
                    .map(|client| {
                        let (db, stop, measuring) = (db.clone(), stop.clone(), measuring.clone());
                        std::thread::Builder::new()
                            .spawn(move || {
                                let mut latencies = Vec::new();
                                let mut refused = 0u64;
                                let mut answered_at_all = 0u64;
                                let mut from = (client as i64 * 7) % (ROWS - LIMIT as i64);
                                while !stop.load(Ordering::Relaxed) {
                                    let counted = measuring.load(Ordering::Relaxed)
                                        && !stop.load(Ordering::Relaxed);
                                    let until = Instant::now() + Duration::from_secs(60);
                                    let started = Instant::now();
                                    let start = from;
                                    let answered = match charge {
                                        None => db.call(until, move |c| read(c, start)),
                                        Some(charge) => {
                                            db.call_charged(until, charge, move |c| read(c, start))
                                        }
                                    };
                                    let took = started.elapsed().as_nanos() as u64;
                                    from = (from + 1) % (ROWS - LIMIT as i64);
                                    answered_at_all += 1;
                                    if !counted {
                                        continue;
                                    }
                                    match answered {
                                        Ok(rows) => {
                                            black_box(rows);
                                            latencies.push(took);
                                        }
                                        Err(_) => refused += 1,
                                    }
                                }
                                (latencies, refused, answered_at_all)
                            })
                            .expect("spawn a client")
                    })
                    .collect();
                // Warm up a second, then count.
                std::thread::sleep(Duration::from_secs(1));
                measuring.store(true, Ordering::Relaxed);
                let started = Instant::now();
                let before = machine();
                std::thread::sleep(Duration::from_secs_f64(seconds));
                stop.store(true, Ordering::Relaxed);
                let mut latencies = Vec::new();
                let mut refused = 0;
                let mut all = 0;
                for t in threads {
                    let (l, r, a) = t.join().expect("client");
                    latencies.extend(l);
                    refused += r;
                    all += a;
                }
                // Every call started in the window is counted, however late it answers.
                let elapsed = started.elapsed().as_secs_f64();
                latencies.sort_unstable();
                println!(
                    "{{\"what\":\"ab\",\"arm\":\"{arm}\",\"round\":{round},\"clients\":{clients},\"calls\":{},\"per_s\":{:.0},\"p50_us\":{:.1},\"p99_us\":{:.1},\"p999_us\":{:.1},\"refused\":{refused},\"answered_at_all\":{all},\"window_s\":{elapsed:.1},{before},\"after\":{{{}}}}}",
                    latencies.len(),
                    latencies.len() as f64 / elapsed,
                    percentile(&latencies, 0.50),
                    percentile(&latencies, 0.99),
                    percentile(&latencies, 0.999),
                    machine()
                );
            }
        }
    }
}
