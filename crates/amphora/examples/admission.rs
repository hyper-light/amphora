//! The audit's §1 measurement (AUDIT 2026-11-23, cross-cutting): what reading the memory budget
//! costs a request, and what a request through `Database` costs on a ladder of clients. Every local
//! request (`Database::call`, `execute`, Records) is admitted by charging the budget's room for
//! queues (MB.1), which reads the process's limit and resident bytes ([`amphora::memory`]).
//!
//! cargo run --release -p amphora --example admission -- <dir> <reads> <seconds> <clients,...> [call|get]
//!
//! One line a measurement: the budget's `room` in ns a call (best of five rounds of <reads>), then,
//! for each client count, requests a second over <seconds> seconds: `Database::call`s of an empty
//! closure (`call`, the default), or Records gets of one absent key (`get`), and the refused ones.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::cast_precision_loss
)]

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use amphora::database::Database;
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
                        db.call(until, |_| Ok(()))
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
