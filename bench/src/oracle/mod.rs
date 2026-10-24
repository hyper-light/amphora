//! The oracles (`--oracle`, G2 step 3; docs/research/oracles.md): random databases and queries over
//! the SQLite language's inventory, each query checked by TLP, NoREC, PQS, DQP, EET and CERT on
//! Amphora, and against SQLite's own answer. A failure is cut down to its smallest form (reduce.rs)
//! and written as a finding; a panic in Amphora is one too.
//!
//! `amphora-bench --oracle [--oracles diff,tlp,norec,pqs,dqp,eet,cert] [--seconds N] [--seed S]
//!  [--per-state N] [--reduce-seconds N] [--features bench/corpus/features.json] [--out DIR] [--label L]
//!  [--sqlite LIBRARY]`: `--sqlite` loads the reference from LIBRARY (bench/oracles/sqlite.sh builds
//! it), in place of the system's libsqlite3.so.0, here and with `--replay` and `--replay-check`.
//!
//! Writes `<out>/summary.json` (each oracle's checks, passes, skips by reason, refusals by message,
//! failures, and how often DQP's other plan was a different plan) and `<out>/findings/<id>.md`.

pub mod checks;
pub mod engines;
pub mod synth;
pub mod reduce;

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use checks::{Case, Failure, Kind, Oracle, Outcome};
use engines::{Amphora, Engine, Sqlite};
use reduce::Repro;
use serde_json::json;

struct Opts {
    oracles: Vec<Oracle>,
    seconds: u64,
    seed: u64,
    per_state: usize,
    reduce_seconds: u64,
    features: String,
    out: String,
    label: String,
    /// Only this database of the run (its cases as the run made them: each database's seed is its
    /// own), to reproduce one.
    state: Option<u64>,
}

fn opts() -> anyhow::Result<Opts> {
    let mut o = Opts { oracles: Oracle::ALL.to_vec(), seconds: 60, seed: 1, per_state: 40, reduce_seconds: 20, features: "bench/corpus/features.json".into(), out: "oracle-out".into(), label: String::new(), state: None };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || args.next().ok_or_else(|| anyhow::anyhow!("{a} takes a value"));
        match a.as_str() {
            "--oracle" => {}
            "--oracles" => o.oracles = v()?.split(',').map(|s| Oracle::parse(s).ok_or_else(|| anyhow::anyhow!("no oracle {s}"))).collect::<anyhow::Result<_>>()?,
            "--seconds" => o.seconds = v()?.parse()?,
            "--seed" => o.seed = v()?.parse()?,
            "--per-state" => o.per_state = v()?.parse()?,
            "--reduce-seconds" => o.reduce_seconds = v()?.parse()?,
            "--features" => o.features = v()?,
            "--out" => o.out = v()?,
            "--label" => o.label = v()?,
            "--state" => o.state = Some(v()?.parse()?),
            "--sqlite" => {
                v()?;
            }
            "--replay" => {
                v()?;
            }
            other => anyhow::bail!("unknown argument {other} (--oracle takes --oracles, --seconds, --seed, --per-state, --reduce-seconds, --features, --out, --label, --sqlite)"),
        }
    }
    Ok(o)
}

/// Both engines with the same database: each statement on SQLite first, inside a savepoint, then on
/// Amphora; one SQLite refuses is left out, one Amphora refuses is undone on SQLite, so the two
/// always hold the same rows. Returns the statements both took.
pub fn setup(stmts: &[String], n: u64) -> anyhow::Result<(Amphora, Sqlite, Vec<String>, Vec<String>)> {
    let mut a = Amphora::new(n)?;
    let mut s = Sqlite::new()?;
    let mut kept = Vec::new();
    let mut refused = Vec::new();
    let on_sqlite = |s: &mut Sqlite, sql: &str| s.run(sql).map(|_| ()).map_err(|e| anyhow::anyhow!("{sql}: {e}"));
    for st in stmts {
        on_sqlite(&mut s, "SAVEPOINT oracle")?;
        if s.run(st).is_err() {
            on_sqlite(&mut s, "ROLLBACK TO oracle")?;
            on_sqlite(&mut s, "RELEASE oracle")?;
            continue;
        }
        match a.run(st) {
            Ok(_) => {
                on_sqlite(&mut s, "RELEASE oracle")?;
                kept.push(st.clone());
            }
            Err(e) => {
                on_sqlite(&mut s, "ROLLBACK TO oracle")?;
                on_sqlite(&mut s, "RELEASE oracle")?;
                refused.push(e);
            }
        }
    }
    Ok((a, s, kept, refused))
}

fn panic_text(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned())).unwrap_or_else(|| "a panic".into())
}

/// Runs the case on fresh databases made from its statements.
fn rerun(r: &Repro, n: u64) -> Option<Failure> {
    let got = std::panic::catch_unwind(AssertUnwindSafe(|| -> Option<Failure> {
        let (mut a, mut s, _, _) = setup(&r.stmts, n).ok()?;
        match checks::check(&r.case, &mut a, &mut s) {
            Outcome::Fail(f) => Some(f),
            _ => None,
        }
    }));
    match got {
        Ok(f) => f,
        Err(p) => Some(Failure { kind: Kind::Panic, detail: panic_text(p), queries: checks::sqls(&r.case), cause: None }),
    }
}

/// An error's message with its numbers taken out, so refusals group by kind ("no such function:
/// median" stays whole: which functions Amphora lacks is what the count is for).
fn shape(e: &str) -> String {
    e.chars().take(120).map(|c| if c.is_ascii_digit() { '#' } else { c }).collect()
}

#[derive(Default)]
struct Tally {
    checks: u64,
    passed: u64,
    plans_changed: u64,
    failed: u64,
    skipped: BTreeMap<String, u64>,
    refused: BTreeMap<String, u64>,
}

/// A finding's name: an error by its cause (Amphora's message, whichever oracle met it: one cause
/// reaches every oracle), anything else by its case cut down.
fn finding_id(case: &Case, f: &Failure) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let (Kind::Error, Some(cause)) = (f.kind, &f.cause) {
        shape(cause).hash(&mut h);
        return format!("error-{:016x}", h.finish());
    }
    case.oracle().name().hash(&mut h);
    format!("{:?}", f.kind).hash(&mut h);
    f.queries.join("\n").hash(&mut h);
    format!("{}-{:016x}", case.oracle().name(), h.finish())
}

fn write_finding(dir: &std::path::Path, id: &str, r: &Repro, f: &Failure, seed: u64, state: u64) -> anyhow::Result<()> {
    let mut md = format!("# {} {:?}: {}\n\nseed {seed}, state {state}\n\n## Statements\n\n```sql\n", r.case.oracle().name(), f.kind, f.detail.lines().next().unwrap_or_default());
    for s in &r.stmts {
        md.push_str(s);
        md.push_str(";\n");
    }
    md.push_str("```\n\n## Queries\n\n```sql\n");
    for q in &f.queries {
        md.push_str(q);
        md.push_str(";\n");
    }
    md.push_str(&format!("```\n\n## What differed\n\n```\n{}\n```\n", f.detail));
    std::fs::write(dir.join(format!("{id}.md")), md)?;
    Ok(())
}

/// `--replay FINDING.md`: the finding's statements on both engines, then each of its queries with
/// both answers and Amphora's plan (text and JSON), for triage.
fn replay(path: &str) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let blocks: Vec<&str> = text.split("```sql\n").skip(1).filter_map(|b| b.split("```").next()).collect();
    let stmts: Vec<String> = blocks.first().map(|b| b.split(";\n").map(str::trim).filter(|x| !x.is_empty()).map(str::to_owned).collect()).unwrap_or_default();
    let queries: Vec<String> = blocks.get(1).map(|b| b.split(";\n").map(str::trim).filter(|x| !x.is_empty()).map(str::to_owned).collect()).unwrap_or_default();
    let (mut a, mut s, kept, refused) = setup(&stmts, 0)?;
    println!("statements: {} run, {} refused by Amphora {refused:?}", kept.len(), stmts.len() - kept.len());
    for q in &queries {
        println!("\n== {q}\n-- SQLite:  {}\n-- Amphora: {}", engines::show_up_to(&s.run(q), usize::MAX), engines::show_up_to(&a.run(q), usize::MAX));
        if q.trim_start().to_ascii_uppercase().starts_with("SELECT") {
            println!("-- Amphora's plan: {}", engines::show(&a.run(&format!("EXPLAIN QUERY PLAN {q}"))));
            println!("-- Amphora's plan (JSON): {}", engines::show(&a.run(&format!("EXPLAIN QUERY PLAN FORMAT=JSON {q}"))));
        }
    }
    Ok(())
}

/// `--replay-check FINDING.md`: whether every query of the finding answers on Amphora as on SQLite
/// (exit 0) or not (exit 1): told against two builds, which one a finding is the fault of.
fn replay_check(path: &str) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let blocks: Vec<&str> = text.split("```sql\n").skip(1).filter_map(|b| b.split("```").next()).collect();
    let split = |b: &str| -> Vec<String> { b.split(";\n").map(str::trim).filter(|x| !x.is_empty()).map(str::to_owned).collect() };
    let stmts = blocks.first().map(|b| split(b)).unwrap_or_default();
    let queries = blocks.get(1).map(|b| split(b)).unwrap_or_default();
    let (mut a, mut s, _, _) = setup(&stmts, 0)?;
    // A CERT finding's two queries: the more restrictive (the second) estimated at no more rows.
    if std::path::Path::new(path).file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("cert-")) {
        if let [q, more] = queries.as_slice() {
            let (Some(eq), Some(er)) = (checks::estimate_any(&mut a, q), checks::estimate_any(&mut a, more)) else {
                println!("no estimate: {q}");
                std::process::exit(1);
            };
            if !checks::no_more(er, eq) {
                println!("estimates {er} rows for the more restrictive query, {eq} for the query: {more}");
                std::process::exit(1);
            }
            return Ok(());
        }
    }
    for q in &queries {
        let same = match (a.run(q), s.run(q)) {
            (Ok(x), Ok(y)) => engines::same_rows_by(&x, &y, true),
            (Err(_), Err(_)) => true,
            _ => false,
        };
        if !same {
            println!("differs: {q}");
            std::process::exit(1);
        }
    }
    Ok(())
}

pub fn main() -> anyhow::Result<()> {
    if let Some(i) = std::env::args().position(|a| a == "--sqlite") {
        engines::use_library(std::env::args().nth(i + 1).ok_or_else(|| anyhow::anyhow!("--sqlite takes a library's path"))?).map_err(|e| anyhow::anyhow!(e))?;
    }
    if let Some(i) = std::env::args().position(|a| a == "--replay-check") {
        return replay_check(&std::env::args().nth(i + 1).ok_or_else(|| anyhow::anyhow!("--replay-check takes a finding's file"))?);
    }
    if let Some(i) = std::env::args().position(|a| a == "--replay") {
        return replay(&std::env::args().nth(i + 1).ok_or_else(|| anyhow::anyhow!("--replay takes a finding's file"))?);
    }
    let o = opts()?;
    let features: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&o.features)?)?;
    let mut catalog = Sqlite::new()?;
    let funcs = synth::functions(&features, &mut catalog)?;
    let sqlite_version = match catalog.run("SELECT sqlite_version()").map_err(|e| anyhow::anyhow!(e))?.first().and_then(|r| r.first()) {
        Some(engines::V::Text(v)) => v.clone(),
        _ => String::new(),
    };
    let sqlite_options: Vec<String> = catalog
        .run("PRAGMA compile_options")
        .map_err(|e| anyhow::anyhow!(e))?
        .into_iter()
        .filter_map(|r| match r.into_iter().next() {
            Some(engines::V::Text(v)) => Some(v),
            _ => None,
        })
        .collect();
    let out = std::path::Path::new(&o.out);
    let fdir = out.join("findings");
    std::fs::create_dir_all(&fdir)?;
    let began = Instant::now();
    let until = began + Duration::from_secs(o.seconds);
    let mut tally: BTreeMap<Oracle, Tally> = std::iter::once(Oracle::Same).chain(o.oracles.iter().copied()).map(|x| (x, Tally::default())).collect();
    let mut findings: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut duplicates = 0u64;
    let mut states = 0u64;
    let mut setup_refused: BTreeMap<String, u64> = BTreeMap::new();
    let mut db_n = 0u64;
    if let Some(n) = o.state {
        states = n.saturating_sub(1);
    }
    while Instant::now() < until && o.state.is_none_or(|n| states < n) {
        states += 1;
        let state_seed = o.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ states;
        let mut rng = synth::Rng::new(state_seed);
        let (stmts, schema) = synth::database(&mut rng);
        db_n += 1;
        let built = std::panic::catch_unwind(AssertUnwindSafe(|| setup(&stmts, db_n)));
        let (mut a, mut s, kept) = match built {
            Ok(Ok((a, s, kept, refused))) => {
                for e in refused {
                    *setup_refused.entry(shape(&e)).or_default() += 1;
                }
                (a, s, kept)
            }
            Ok(Err(e)) => anyhow::bail!("setting up a database: {e:#}"),
            Err(p) => {
                let f = Failure { kind: Kind::Panic, detail: format!("Amphora panicked making the database: {}", panic_text(p)), queries: vec![], cause: None };
                let r = Repro { stmts: stmts.clone(), case: Case::Diff(synth::query(&mut rng, &schema, &funcs)) };
                let id = format!("setup-panic-{:016x}", state_seed);
                write_finding(&fdir, &id, &r, &f, o.seed, states)?;
                findings.insert(id, json!({ "oracle": "setup", "kind": "Panic", "detail": f.detail }));
                continue;
            }
        };
        // first, that both engines hold the same rows: nothing differential means anything otherwise
        let same = Case::Same(schema.tables.iter().map(|t| t.name.clone()).collect());
        let mut cases: Vec<Case> = vec![same];
        for i in 0..o.per_state {
            let oracle = o.oracles[(states as usize * o.per_state + i) % o.oracles.len()];
            if let Some(case) = checks::build(oracle, &mut rng, &schema, &funcs, &mut s) {
                cases.push(case);
            }
        }
        for case in cases {
            if Instant::now() >= until {
                break;
            }
            let oracle = case.oracle();
            let t = tally.get_mut(&oracle).ok_or_else(|| anyhow::anyhow!("an oracle not asked for"))?;
            t.checks += 1;
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| checks::check(&case, &mut a, &mut s)));
            let failure = match outcome {
                Ok(Outcome::Pass { plan_changed }) => {
                    t.passed += 1;
                    t.plans_changed += u64::from(plan_changed);
                    None
                }
                Ok(Outcome::Skip(why)) => {
                    *t.skipped.entry(why.into()).or_default() += 1;
                    None
                }
                Ok(Outcome::Refused(e)) => {
                    *t.refused.entry(shape(&e)).or_default() += 1;
                    None
                }
                Ok(Outcome::Fail(f)) => Some(f),
                Err(p) => Some(Failure { kind: Kind::Panic, detail: panic_text(p), queries: checks::sqls(&case), cause: None }),
            };
            let Some(f) = failure else { continue };
            t.failed += 1;
            let start = Repro { stmts: kept.clone(), case: case.clone() };
            let deadline = Instant::now() + Duration::from_secs(o.reduce_seconds);
            let kind = f.kind;
            let small = reduce::reduce(start, kind, deadline, |r| {
                db_n += 1;
                rerun(r, db_n)
            });
            db_n += 1;
            let final_f = rerun(&small, db_n).unwrap_or(f);
            let id = finding_id(&small.case, &final_f);
            if findings.contains_key(&id) {
                duplicates += 1;
            } else {
                write_finding(&fdir, &id, &small, &final_f, o.seed, states)?;
                findings.insert(id.clone(), json!({ "oracle": oracle.name(), "kind": format!("{:?}", final_f.kind), "detail": final_f.detail.lines().next().unwrap_or_default(), "statements": small.stmts.len(), "at_seconds": began.elapsed().as_secs_f64() }));
                eprintln!("finding {id}: {}", final_f.detail.lines().next().unwrap_or_default());
            }
            if kind == Kind::Panic || oracle == Oracle::Same {
                break; // the engine may be left anyhow, or the rows differ: a new database
            }
        }
    }
    let per: serde_json::Map<String, serde_json::Value> = tally.iter().map(|(k, t)| (k.name().to_owned(), json!({
        "checks": t.checks, "passed": t.passed, "failed": t.failed, "plans_changed": t.plans_changed,
        "skipped": t.skipped, "refused": t.refused,
    }))).collect();
    let summary = json!({
        "label": o.label, "seed": o.seed, "seconds": began.elapsed().as_secs_f64(), "states": states, "sqlite": sqlite_version,
        "sqlite_options": sqlite_options,
        "functions": funcs.len(), "oracles": per, "setup_refused": setup_refused,
        "findings": findings, "duplicates": duplicates,
    });
    std::fs::write(out.join("summary.json"), serde_json::to_string_pretty(&summary)?)?;
    eprintln!("{states} databases, {} checks, {} findings ({duplicates} more of the same), in {:.0} s", tally.values().map(|t| t.checks).sum::<u64>(), findings.len(), began.elapsed().as_secs_f64());
    Ok(())
}
