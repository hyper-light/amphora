//! The audit's §1 measurement (AUDIT 2026-11-23, cross-cutting): what reading the memory budget
//! costs a request, and what a request through `Database` costs on a ladder of clients. Every local
//! request (`Database::call`, `execute`, Records) is admitted by charging the budget's room for
//! queues (MB.1), which reads the process's limit and resident bytes ([`amphora::memory`]).
//!
//! cargo run --release -p amphora --example admission -- <dir> <reads> <seconds> <clients,...> [call|get]
//! cargo run --release -p amphora --example admission -- <dir> <reads> <seconds> <clients,...> ab <rounds>
//! cargo run --release -p amphora --example admission -- <dir> <reads> <seconds> <in-flight,...> nowait <rounds> <pool>
//!
//! One line a measurement: the budget's `room` in ns a call (best of five rounds of <reads>), then,
//! for each client count, requests a second over <seconds> seconds: `Database::call`s of an empty
//! closure (`call`, the default), or Records gets of one absent key (`get`), and the refused ones.
//!
//! `ab` measures a call charged its declared answer (AUDIT §1; the only charge since Ada's
//! decision on lane perf-A8's A/B, lane perf-A9): each call reads `LIMIT` rows of a seeded table,
//! declaring their bound, round after round over the client ladder; a line a run gives every
//! call's own latency's p50, p99 and p99.9 (no sampling), calls a second, refusals and the
//! machine's load and pressure.
//!
//! `nowait` is the same at 1k-10k calls in flight (lane perf-A7): a pool of <pool> threads keeps
//! each count of calls in flight through `Database::call_nowait`, each call's answer taken by its
//! waker as it is woken; a call's latency runs from its submission to its answer.
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use amphora::database::{AnswerBytes, AnswerMax, Database};
use amphora::memory::{self, Use};
use amphora_api::{Consistency, Records, Refusal};
use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let usage = "usage: admission <dir> <reads> <seconds> <clients,...> [call|get]";
    let mut args = std::env::args_os().skip(1);
    let dir = std::path::PathBuf::from(args.next().context(usage)?);
    let mut args = args.map(|arg| arg.into_string().map_err(|_| anyhow::anyhow!(usage)));
    let reads: u32 = args.next().context(usage)??.parse()?;
    let seconds: f64 = args.next().context(usage)??.parse()?;
    let measure = Duration::try_from_secs_f64(seconds)?;
    let ladder: Vec<usize> = args
        .next()
        .context(usage)??
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    let operation = args.next().transpose()?;
    let get = match operation.as_deref() {
        None | Some("call") => false,
        Some("get") => true,
        Some("ab") => {
            let rounds: usize = args.next().context(usage)??.parse()?;
            ab(&dir, seconds, &ladder, rounds);
            return Ok(());
        }
        Some("nowait") => {
            let rounds: usize = args.next().context(usage)??.parse()?;
            let pool: usize = args.next().context(usage)??.parse()?;
            nowait(&dir, seconds, &ladder, rounds, pool);
            return Ok(());
        }
        Some(_) => bail!(usage),
    };
    if reads == 0 || measure.is_zero() || ladder.contains(&0) {
        bail!(usage);
    }

    let budget = memory::process();
    let mut best = f64::MAX;
    for _ in 0..5 {
        let started = Instant::now();
        for _ in 0..reads {
            black_box(budget.room(Use::Queues)?);
        }
        best = best.min(started.elapsed().as_nanos() as f64 / f64::from(reads));
    }
    println!("{{\"what\":\"room\",\"ns_per_call\":{best:.1}}}");

    std::fs::create_dir_all(&dir)?;
    let path = dir.join("admission.db");
    if path.try_exists()? {
        std::fs::remove_file(&path)?;
    }
    let until = || Instant::now() + Duration::from_secs(600);
    let db = Database::open(&path, until())?;
    for clients in ladder {
        let stop = Arc::new(AtomicBool::new(false));
        let measuring = Arc::new(AtomicBool::new(false));
        let (ready, prepared) = mpsc::channel();
        let mut threads = Vec::new();
        for _ in 0..clients {
            let (db, stop, measuring, ready) =
                (db.clone(), stop.clone(), measuring.clone(), ready.clone());
            threads.push(std::thread::Builder::new().spawn(move || -> Result<_> {
                let ns = db.namespace("admission")?;
                ready.send(())?;
                drop(ready);
                let (mut done, mut refused, mut unexpected) = (0u64, 0u64, 0u64);
                while !stop.load(Ordering::Acquire) {
                    let counted = measuring.load(Ordering::Acquire);
                    let until = Instant::now() + Duration::from_secs(60);
                    let answered = if get {
                        match ns.get("f", b"k", Consistency::Linearizable, until) {
                            Ok(None) => Ok(()),
                            Ok(Some(_)) => bail!("expected the absent key"),
                            Err(error) => Err(error),
                        }
                    } else {
                        db.call(until, AnswerMax::fixed(), |_| Ok(()))
                    };
                    if counted {
                        match answered {
                            Ok(()) => done += 1,
                            Err(refusal) => {
                                refused += 1;
                                if !matches!(refusal, Refusal::Full { .. }) {
                                    unexpected += 1;
                                }
                            }
                        }
                    }
                }
                Ok((done, refused, unexpected))
            })?);
        }
        drop(ready);
        let prepared = prepared.into_iter().count();
        std::thread::sleep(Duration::from_secs(1));
        let started = Instant::now();
        measuring.store(true, Ordering::Release);
        std::thread::sleep(measure);
        stop.store(true, Ordering::Release);
        let (mut completed, mut refused, mut unexpected) = (0u64, 0u64, 0u64);
        for t in threads {
            let (answered, errors, other) = t
                .join()
                .map_err(|_| anyhow::anyhow!("client thread failed"))??;
            completed += answered;
            refused += errors;
            unexpected += other;
        }
        let elapsed = started.elapsed().as_secs_f64();
        println!(
            "{{\"what\":\"{}\",\"clients\":{clients},\"per_s\":{:.0},\"refused_per_s\":{:.0},\"completed\":{completed},\"refused\":{refused},\"unexpected_errors\":{unexpected}}}",
            if get { "get" } else { "call" },
            completed as f64 / elapsed,
            refused as f64 / elapsed,
        );
        if prepared != clients || unexpected != 0 {
            bail!(
                "{prepared}/{clients} clients prepared, {unexpected} errors other than admission Full"
            );
        }
    }
    Ok(())
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
    let mut statement =
        c.prepare_cached("SELECT id, v FROM t WHERE id >= ?1 ORDER BY id LIMIT ?2")?;
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
    let load = load
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
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
    // The process's own cgroup (v2): under a MemoryMax scope its memory pressure, use and limit.
    let group = std::fs::read_to_string("/proc/self/cgroup")
        .unwrap_or_default()
        .lines()
        .find_map(|line| {
            line.strip_prefix("0::")
                .map(|at| format!("/sys/fs/cgroup{at}"))
        })
        .unwrap_or_default();
    let in_group = |file: &str| {
        std::fs::read_to_string(format!("{group}/{file}"))
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let group_pressure = in_group("memory.pressure")
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("")
        .trim_start_matches("avg10=")
        .to_owned();
    format!(
        "\"load\":\"{load}\",\"cpu_some10\":\"{}\",\"io_some10\":\"{}\",\"mem_some10\":\"{}\",\"group_mem_some10\":\"{group_pressure}\",\"group_mem_current\":\"{}\",\"group_mem_max\":\"{}\",\"queue_room\":{}",
        pressure("cpu"),
        pressure("io"),
        pressure("memory"),
        in_group("memory.current"),
        in_group("memory.max"),
        memory::process().room(Use::Queues).unwrap_or(0)
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
    db.call(until(), AnswerMax::fixed(), |c| {
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
    let arms = [("declared", AnswerMax::<Rows>::bytes(declared))];
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
                                    let answered = db.call(until, charge, move |c| read(c, start));
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

/// The calls in flight and the latencies of their answers in one run of [`nowait`].
struct Flight {
    room: std::sync::Mutex<usize>,
    freed: std::sync::Condvar,
    measuring: AtomicBool,
    latencies: std::sync::Mutex<Vec<u64>>,
    /// Refusals by cause, at submit and through the answer (lane perf-A8).
    at_submit: Causes,
    at_answer: Causes,
}

/// Refusals by cause: the memory budget's room for queues (`Full`: since lane perf-A8 the
/// writer's queue has no count, so `Full` has no other source), the call's deadline (`Timeout`,
/// or `UnknownOutcome` once it passed), and the rest.
#[derive(Default)]
struct Causes {
    memory: AtomicU64,
    deadline: AtomicU64,
    other: AtomicU64,
}

impl Causes {
    fn count(&self, refusal: &amphora_api::Refusal) {
        match refusal {
            amphora_api::Refusal::Full { .. } => &self.memory,
            amphora_api::Refusal::Timeout | amphora_api::Refusal::UnknownOutcome => &self.deadline,
            _ => &self.other,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    fn json(&self) -> String {
        format!(
            "{{\"memory\":{},\"deadline\":{},\"other\":{}}}",
            self.memory.load(Ordering::Relaxed),
            self.deadline.load(Ordering::Relaxed),
            self.other.load(Ordering::Relaxed)
        )
    }
}

impl Flight {
    fn end(&self, submitted: Instant, answer: Result<Rows, amphora_api::Refusal>) {
        let took = submitted.elapsed().as_nanos() as u64;
        if self.measuring.load(Ordering::Relaxed) {
            match answer {
                Ok(rows) => {
                    black_box(rows);
                    self.latencies.lock().unwrap().push(took);
                }
                Err(refusal) => self.at_answer.count(&refusal),
            }
        }
        *self.room.lock().unwrap() += 1;
        self.freed.notify_one();
    }
}

/// One call in flight: polled once by its submitter, then by its waker when its answer comes.
struct InFlight {
    submitted: Instant,
    calling: std::sync::Mutex<Option<amphora::database::Calling<Rows>>>,
    flight: Arc<Flight>,
}

impl InFlight {
    fn poll(self: &Arc<Self>) {
        let mut calling = self.calling.lock().unwrap();
        let Some(pending) = calling.as_mut() else {
            return;
        };
        let waker = std::task::Waker::from(Arc::clone(self));
        if let std::task::Poll::Ready(answer) =
            std::pin::Pin::new(pending).poll(&mut std::task::Context::from_waker(&waker))
        {
            *calling = None;
            drop(calling);
            self.flight.end(self.submitted, answer);
        }
    }
}

impl std::task::Wake for InFlight {
    fn wake(self: Arc<Self>) {
        self.poll();
    }
}

use std::future::Future as _;

fn nowait(dir: &std::path::Path, seconds: f64, ladder: &[usize], rounds: usize, pool: usize) {
    std::fs::create_dir_all(dir).expect("dir");
    let path = dir.join("admission-nowait.db");
    let _ = std::fs::remove_file(&path);
    let until = || Instant::now() + Duration::from_secs(600);
    let db = Database::open(&path, until()).expect("open");
    db.call(until(), AnswerMax::fixed(), |c| {
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
    println!(
        "{{\"what\":\"nowait\",\"declared_bytes\":{declared},\"rows\":{ROWS},\"limit\":{LIMIT},\"pool\":{pool}}}"
    );
    let arms = [("declared", AnswerMax::<Rows>::bytes(declared))];
    for round in 0..rounds {
        for (arm, charge) in arms {
            for &in_flight in ladder {
                let flight = Arc::new(Flight {
                    room: std::sync::Mutex::new(in_flight),
                    freed: std::sync::Condvar::new(),
                    measuring: AtomicBool::new(false),
                    latencies: std::sync::Mutex::new(Vec::new()),
                    at_submit: Causes::default(),
                    at_answer: Causes::default(),
                });
                let stop = Arc::new(AtomicBool::new(false));
                let threads: Vec<_> = (0..pool)
                    .map(|worker| {
                        let (db, flight, stop) = (db.clone(), flight.clone(), stop.clone());
                        std::thread::Builder::new()
                            .spawn(move || {
                                let mut from = (worker as i64 * 7) % (ROWS - LIMIT as i64);
                                while !stop.load(Ordering::Relaxed) {
                                    {
                                        let mut room = flight.room.lock().unwrap();
                                        while *room == 0 && !stop.load(Ordering::Relaxed) {
                                            room = flight.freed.wait(room).unwrap();
                                        }
                                        if stop.load(Ordering::Relaxed) {
                                            break;
                                        }
                                        *room -= 1;
                                    }
                                    let start = from;
                                    from = (from + 1) % (ROWS - LIMIT as i64);
                                    let submitted = Instant::now();
                                    match db.call_nowait(until(), charge, move |c| read(c, start)) {
                                        Ok(calling) => Arc::new(InFlight {
                                            submitted,
                                            calling: std::sync::Mutex::new(Some(calling)),
                                            flight: flight.clone(),
                                        })
                                        .poll(),
                                        // Refused at admission: try again once a call ends.
                                        Err(refusal) => {
                                            if flight.measuring.load(Ordering::Relaxed) {
                                                flight.at_submit.count(&refusal);
                                            }
                                            let room = flight.room.lock().unwrap();
                                            let mut room = if stop.load(Ordering::Relaxed) {
                                                room
                                            } else {
                                                flight.freed.wait(room).unwrap()
                                            };
                                            *room += 1;
                                        }
                                    }
                                }
                            })
                            .expect("spawn a worker")
                    })
                    .collect();
                std::thread::sleep(Duration::from_secs(1));
                flight.measuring.store(true, Ordering::Relaxed);
                let started = Instant::now();
                let before = machine();
                std::thread::sleep(Duration::from_secs_f64(seconds));
                let window = started.elapsed().as_secs_f64();
                flight.measuring.store(false, Ordering::Relaxed);
                {
                    let _room = flight.room.lock().unwrap();
                    stop.store(true, Ordering::Relaxed);
                }
                flight.freed.notify_all();
                for t in threads {
                    t.join().expect("worker");
                }
                {
                    let mut room = flight.room.lock().unwrap();
                    while *room < in_flight {
                        room = flight.freed.wait(room).unwrap();
                    }
                }
                let mut latencies = std::mem::take(&mut *flight.latencies.lock().unwrap());
                latencies.sort_unstable();
                println!(
                    "{{\"what\":\"nowait\",\"arm\":\"{arm}\",\"round\":{round},\"in_flight\":{in_flight},\"calls\":{},\"per_s\":{:.0},\"p50_us\":{:.1},\"p99_us\":{:.1},\"p999_us\":{:.1},\"refused_at_submit\":{},\"refused_at_answer\":{},\"window_s\":{window:.1},{before},\"after\":{{{}}}}}",
                    latencies.len(),
                    latencies.len() as f64 / window,
                    percentile(&latencies, 0.50),
                    percentile(&latencies, 0.99),
                    percentile(&latencies, 0.999),
                    flight.at_submit.json(),
                    flight.at_answer.json(),
                    machine()
                );
            }
        }
    }
}
