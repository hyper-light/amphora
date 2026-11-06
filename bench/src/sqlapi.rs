//! kiln's hot statements through Amphora's public SQL API (`amphora::sql`, the unify audit's
//! finding 4): each a `prepare_cached` statement run again and again inside `Database::call`,
//! timed per execute, best of `--runs`. The shapes are kiln's unify branch's own statements:
//! - `reapi`: a digest's three statements, ci/services/memo.rs `missing`'s EXISTS, then
//!   ci/services/cas.rs `object`'s SELECT size and its UPDATE of the last use;
//! - `logsearch`: ci/logsearch.rs's search page, its cursor stepping down the documents, every
//!   page's rows folded into a checksum, so two builds' answers can be compared;
//! - `insert`: ci/logsearch.rs's posting INSERT.
//! - `parse`: what the session's `validate` does to each of those statements on every execute at e2e67247 (a whole
//!   parse, then a walk of its tokens for reserved names), timed alone, so a shape's time a statement can be
//!   set against it.
//!
//! Each run reports, per execute, the time inside the calls' closures (the statements alone) and
//! the calls' whole time (their commits too): one JSON line a run, then the best of each. Where Linux
//! keeps it, the closures' own CPU time too (the writer thread's, /proc/thread-self/schedstat): on a
//! loaded machine a capped run's wall clock counts the time it waits for a CPU, its CPU time doesn't.
//! AMPHORA_PROFILE=<prefix> (Unix only) samples each run 997 times a second, written as
//! <prefix>-<shape>-<run>.{svg,txt,folded} (main.rs, write_profile); bench/prof_share.py gives the
//! SQL parser's share of a shape's loop from the .folded file.
//!
//! usage: amphora-bench --sql-api --dir <dir> [--shapes reapi,logsearch,insert] [--runs 3]
//!        [--digests 100000] [--pages 10000] [--rows 100000] [--per-call 1000]
//!        [--pages-per-call 100] [--parses 20000] [--label <commit>]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use amphora::database::Database;
use amphora_api::Value;
use amphora_engine_parser::{
    ast::fmt::{BlankContext, ToTokens, TokenStream},
    parser::Parser,
    token::TokenType,
};

const EXISTS: &str = "SELECT EXISTS(SELECT 1 FROM work_records WHERE family = ?1 AND key = ?2 AND at >= ?3) AND EXISTS(SELECT 1 FROM cas_index WHERE digest = ?4)";
const SIZE: &str = "SELECT size FROM cas_index WHERE digest = ?1";
const TOUCH: &str = "UPDATE cas_index SET last_used_at = ?2 WHERE digest = ?1";
const PAGE: &str = "SELECT s.leg, s.step, s.line, s.text, j.run_id, j.key, r.number, r.repo_id, COALESCE(r.workflow_id, r.pipeline, ''), r.branch, r.conclusion, r.created_at,
                (SELECT name FROM steps WHERE job_id = j.id AND idx = s.step), s.id
         FROM ci_log_grams p JOIN ci_log_docs s ON s.id = p.doc JOIN jobs j ON j.id = s.leg JOIN runs r ON r.id = j.run_id
         WHERE p.gram = ?1 AND p.doc <= ?2 AND (?3 IS NULL OR r.repo_id = ?3) AND (?4 IS NULL OR r.workflow_id = ?4) AND (?5 IS NULL OR r.branch = ?5)
           AND (?6 IS NULL OR r.conclusion = ?6) AND (?7 IS NULL OR r.created_at >= ?7) AND (?8 IS NULL OR r.created_at < ?8)
           AND (?9 IS NULL OR EXISTS (SELECT 1 FROM ci_log_grams g WHERE g.gram = ?9 AND g.doc = p.doc))
           AND (?10 IS NULL OR EXISTS (SELECT 1 FROM ci_log_grams g WHERE g.gram = ?10 AND g.doc = p.doc))
           AND (?11 IS NULL OR EXISTS (SELECT 1 FROM ci_log_grams g WHERE g.gram = ?11 AND g.doc = p.doc))
         ORDER BY p.doc DESC LIMIT 8";
const POSTING: &str = "INSERT INTO ci_log_grams (gram, doc) VALUES (?1, ?2)";

const REAPI_SCHEMA: &str = "CREATE TABLE work_records (family TEXT NOT NULL, key TEXT NOT NULL, value BLOB NOT NULL, at INTEGER NOT NULL, PRIMARY KEY (family, key)) WITHOUT ROWID;
CREATE TABLE cas_index (digest TEXT PRIMARY KEY, size INTEGER NOT NULL, created_at INTEGER NOT NULL, last_used_at INTEGER NOT NULL);";
// kiln's runs, jobs and steps cut to the columns the page reads; the log tables as kiln's
// migration 20261029-1110_u05_log_search.sql makes them.
const LOG_SCHEMA: &str = "CREATE TABLE runs (id TEXT PRIMARY KEY, number INTEGER, repo_id TEXT, workflow_id TEXT, pipeline TEXT, branch TEXT, conclusion TEXT, status TEXT, created_at INTEGER);
CREATE TABLE jobs (id TEXT PRIMARY KEY, run_id TEXT, key TEXT, finished_at INTEGER);
CREATE TABLE steps (job_id TEXT NOT NULL, idx INTEGER NOT NULL, name TEXT NOT NULL, PRIMARY KEY (job_id, idx));
CREATE TABLE ci_log_docs (id INTEGER PRIMARY KEY, text TEXT NOT NULL, leg TEXT NOT NULL, step INTEGER NOT NULL, line INTEGER NOT NULL);
CREATE INDEX ci_log_docs_leg ON ci_log_docs (leg, id);
CREATE TABLE ci_log_grams (gram INTEGER NOT NULL, doc INTEGER NOT NULL, PRIMARY KEY (gram, doc));
CREATE INDEX ci_log_grams_doc ON ci_log_grams (doc, gram);";

// The page's grams: every document has both, so each page is 8 documents.
const ANCHOR: i64 = 1;
const ALSO: i64 = 2;
const JOBS: u64 = 1_000;
const STEPS: u64 = 5;
const NOW: i64 = 1_793_000_000;

struct Opts {
    dir: PathBuf,
    shapes: Vec<String>,
    runs: u32,
    digests: u64,
    pages: u64,
    rows: u64,
    per_call: u64,
    pages_per_call: u64,
    parses: u64,
    label: String,
}

/// What a run measured: its executes, the time inside its closures (and their CPU time, where it's
/// known), and the calls' whole time.
struct Timed {
    executes: u64,
    inside: Duration,
    cpu: Option<u64>,
    whole: Duration,
    checksum: u64,
}

impl Timed {
    fn new() -> Self {
        Timed { executes: 0, inside: Duration::ZERO, cpu: Some(0), whole: Duration::ZERO, checksum: 0 }
    }
    /// One call's figures added in.
    fn add(&mut self, executes: u64, inside: Duration, cpu: Option<u64>, whole: Duration) {
        self.executes += executes;
        self.inside += inside;
        self.cpu = self.cpu.zip(cpu).map(|(a, b)| a + b);
        self.whole += whole;
    }
    fn per_execute(d: Duration, executes: u64) -> f64 {
        d.as_nanos() as f64 / executes.max(1) as f64
    }
}

fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^ (x >> 33)
}

/// The calling thread's time on a CPU so far, in nanoseconds: Linux's schedstat, elsewhere unknown.
fn cpu_ns() -> Option<u64> {
    std::fs::read_to_string("/proc/thread-self/schedstat").ok()?.split_whitespace().next()?.parse().ok()
}

/// The CPU time since `from` (a `cpu_ns` reading on this thread).
fn cpu_since(from: Option<u64>) -> Option<u64> {
    from.zip(cpu_ns()).map(|(a, b)| b.saturating_sub(a))
}

fn until() -> Instant {
    Instant::now() + Duration::from_secs(600)
}

fn refused(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

fn digest(i: u64) -> String {
    format!("{:016x}{:016x}{:016x}{:016x}", mix(i), mix(i ^ 1), mix(i ^ 2), mix(i ^ 3))
}

fn key(d: &str) -> String {
    format!("acme/app|trusted|{d}")
}

/// A fresh database file: one that is there already is refused, never reused.
fn fresh(dir: &Path, name: &str) -> anyhow::Result<Database> {
    let path = dir.join(name);
    anyhow::ensure!(!path.exists(), "{} is there already: --sql-api wants fresh files", path.display());
    Database::open(&path, until()).map_err(refused)
}

/// FNV-1a over a row's values, each tagged with its kind.
fn fold(mut h: u64, value: &Value) -> u64 {
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    match value {
        Value::Null => eat(b"n"),
        Value::Integer(n) => {
            eat(b"i");
            eat(&n.to_le_bytes());
        }
        Value::Real(n) => {
            eat(b"r");
            eat(&n.to_bits().to_le_bytes());
        }
        Value::Text(s) => {
            eat(b"t");
            eat(s.as_bytes());
        }
        Value::Blob(b) => {
            eat(b"b");
            eat(b);
        }
    }
    h
}

fn reapi_setup(db: &Database, digests: &[String]) -> anyhow::Result<()> {
    db.call(until(), |c| Ok(c.execute_batch(REAPI_SCHEMA)?)).map_err(refused)?;
    for (n, chunk) in digests.chunks(1_000).enumerate() {
        let chunk = chunk.to_vec();
        let first = n as i64 * 1_000;
        db.call(until(), move |c| {
            let mut record = c.prepare_cached("INSERT INTO work_records (family, key, value, at) VALUES (?1, ?2, ?3, ?4)")?;
            let mut object = c.prepare_cached("INSERT INTO cas_index (digest, size, created_at, last_used_at) VALUES (?1, ?2, ?3, ?3)")?;
            for (i, d) in chunk.iter().enumerate() {
                record.execute(vec![Value::Text("object".into()), Value::Text(key(d)), Value::Blob(Vec::new()), Value::Integer(NOW)])?;
                object.execute(vec![Value::Text(d.clone()), Value::Integer((first + i as i64) * 1_000), Value::Integer(NOW)])?;
            }
            Ok(())
        })
        .map_err(refused)?;
    }
    Ok(())
}

fn reapi_run(db: &Database, digests: &[String], per_call: u64) -> anyhow::Result<Timed> {
    let mut t = Timed::new();
    for chunk in digests.chunks(usize::try_from(per_call.max(1))?) {
        let chunk = chunk.to_vec();
        let began = Instant::now();
        let (executes, inside, cpu) = db
            .call(until(), move |c| {
                let (started, cpu) = (Instant::now(), cpu_ns());
                let mut held = c.prepare_cached(EXISTS)?;
                let mut size = c.prepare_cached(SIZE)?;
                let mut touch = c.prepare_cached(TOUCH)?;
                for d in &chunk {
                    let kept: bool = held.query_row(vec![Value::Text("object".into()), Value::Text(key(d)), Value::Integer(NOW - 3_600), Value::Text(d.clone())], |r| r.get(0))?;
                    anyhow::ensure!(kept, "digest {d} isn't held");
                    let _: i64 = size.query_row(vec![Value::Text(d.clone())], |r| r.get(0))?;
                    anyhow::ensure!(touch.execute(vec![Value::Text(d.clone()), Value::Integer(NOW + 1)])? == 1, "digest {d} wasn't touched");
                }
                Ok((chunk.len() as u64 * 3, started.elapsed(), cpu_since(cpu)))
            })
            .map_err(refused)?;
        t.add(executes, inside, cpu, began.elapsed());
    }
    Ok(t)
}

fn logsearch_setup(db: &Database, docs: u64) -> anyhow::Result<()> {
    db.call(until(), |c| Ok(c.execute_batch(LOG_SCHEMA)?)).map_err(refused)?;
    db.call(until(), |c| {
        let mut run = c.prepare_cached("INSERT INTO runs (id, number, repo_id, workflow_id, pipeline, branch, conclusion, status, created_at) VALUES (?1, ?2, ?3, 'build', NULL, 'main', 'success', 'done', ?4)")?;
        let mut job = c.prepare_cached("INSERT INTO jobs (id, run_id, key, finished_at) VALUES (?1, ?2, 'test', ?3)")?;
        let mut step = c.prepare_cached("INSERT INTO steps (job_id, idx, name) VALUES (?1, ?2, ?3)")?;
        for r in 0..JOBS / 10 {
            run.execute(vec![Value::Text(format!("run-{r}")), Value::Integer(r as i64), Value::Text(format!("repo-{}", r % 5)), Value::Integer(NOW + r as i64)])?;
            for j in 0..10 {
                let id = format!("job-{}", r * 10 + j);
                job.execute(vec![Value::Text(id.clone()), Value::Text(format!("run-{r}")), Value::Integer(NOW + r as i64)])?;
                for s in 0..STEPS {
                    step.execute(vec![Value::Text(id.clone()), Value::Integer(s as i64), Value::Text(format!("step {s}"))])?;
                }
            }
        }
        Ok(())
    })
    .map_err(refused)?;
    let ids: Vec<u64> = (1..=docs).collect();
    for chunk in ids.chunks(1_000) {
        let chunk = chunk.to_vec();
        db.call(until(), move |c| {
            let mut doc = c.prepare_cached("INSERT INTO ci_log_docs (id, text, leg, step, line) VALUES (?1, ?2, ?3, ?4, ?5)")?;
            let mut gram = c.prepare_cached(POSTING)?;
            for &i in &chunk {
                let text = format!("{i:>8} | Compiling crate-{} v0.{}.0 | warning: unused variable `x{}` | error[E0308]: mismatched types", i % 97, i % 13, mix(i) % 1_000);
                doc.execute(vec![Value::Integer(i as i64), Value::Text(text), Value::Text(format!("job-{}", i % JOBS)), Value::Integer((i % STEPS) as i64), Value::Integer(i as i64)])?;
                for g in [ANCHOR, ALSO, 3 + (mix(i) % 50) as i64] {
                    gram.execute(vec![Value::Integer(g), Value::Integer(i as i64)])?;
                }
            }
            Ok(())
        })
        .map_err(refused)?;
    }
    Ok(())
}

fn logsearch_run(db: &Database, pages: u64, per_call: u64) -> anyhow::Result<Timed> {
    let mut t = Timed { checksum: 0xcbf29ce484222325, ..Timed::new() };
    let (mut cursor, mut left) = (i64::MAX, pages);
    while left > 0 {
        let n = left.min(per_call.max(1));
        let (h, start) = (t.checksum, cursor);
        let began = Instant::now();
        let (h, next, inside, cpu) = db
            .call(until(), move |c| {
                let (started, cpu) = (Instant::now(), cpu_ns());
                let (mut h, mut cursor) = (h, start);
                let mut page = c.prepare_cached(PAGE)?;
                for _ in 0..n {
                    let mut params = vec![Value::Integer(ANCHOR), Value::Integer(cursor)];
                    params.extend(std::iter::repeat_n(Value::Null, 6));
                    params.extend([Value::Integer(ALSO), Value::Null, Value::Null]);
                    let mut last = None;
                    for row in page.query_map(params, |r| {
                        let mut values = Vec::with_capacity(14);
                        for k in 0..14usize {
                            values.push(r.get::<_, Value>(k)?);
                        }
                        Ok(values)
                    })? {
                        let values = row?;
                        for v in &values {
                            h = fold(h, v);
                        }
                        if let Some(Value::Integer(id)) = values.last() {
                            last = Some(*id);
                        }
                    }
                    let Some(id) = last else { anyhow::bail!("a page below {cursor} came back empty: --pages is past the documents") };
                    cursor = id - 1;
                }
                Ok((h, cursor, started.elapsed(), cpu_since(cpu)))
            })
            .map_err(refused)?;
        t.add(n, inside, cpu, began.elapsed());
        (t.checksum, cursor) = (h, next);
        left -= n;
    }
    Ok(t)
}

/// The reserved-name walk of crates/amphora/src/sql.rs's `validate_reserved_command`, its checks made the same
/// way: that function is private to the session.
struct Reserved {
    tokens: usize,
}

impl TokenStream for Reserved {
    type Error = anyhow::Error;
    fn append(&mut self, kind: TokenType, value: Option<&str>) -> anyhow::Result<()> {
        self.tokens += 1;
        if kind == TokenType::TK_ID
            && let Some(name) = value
        {
            let name = match name.as_bytes().first() {
                Some(b'\'' | b'"' | b'`' | b'[') => name.get(1..).unwrap_or(""),
                _ => name,
            };
            anyhow::ensure!(!name.get(..10).is_some_and(|p| p.eq_ignore_ascii_case("__amphora_")), "{name} is reserved");
        }
        Ok(())
    }
}

/// `validate`'s work on one statement: parse it whole, then walk its tokens.
fn validate_like(sql: &str) -> anyhow::Result<usize> {
    let mut parser = Parser::new(sql.as_bytes());
    let mut tokens = 0;
    while let Some(command) = parser.next_cmd().map_err(refused)? {
        let mut walk = Reserved { tokens: 0 };
        command.to_tokens(&mut walk, &BlankContext)?;
        tokens += walk.tokens;
    }
    Ok(tokens)
}

/// Nanoseconds a validation of each statement, over `parses` of each: wall clock, and CPU time where known.
fn parse_run(parses: u64) -> anyhow::Result<Vec<(&'static str, f64, Option<f64>)>> {
    let mut each = Vec::new();
    for (name, sql) in [("exists", EXISTS), ("size", SIZE), ("touch", TOUCH), ("page", PAGE), ("posting", POSTING)] {
        let (began, cpu) = (Instant::now(), cpu_ns());
        let mut tokens = 0usize;
        for _ in 0..parses {
            tokens = tokens.wrapping_add(validate_like(std::hint::black_box(sql))?);
        }
        std::hint::black_box(tokens);
        let wall = Timed::per_execute(began.elapsed(), parses);
        each.push((name, wall, cpu_since(cpu).map(|ns| ns as f64 / parses.max(1) as f64)));
    }
    Ok(each)
}

fn insert_run(db: &Database, rows: u64, per_call: u64) -> anyhow::Result<Timed> {
    db.call(until(), |c| Ok(c.execute_batch("CREATE TABLE ci_log_grams (gram INTEGER NOT NULL, doc INTEGER NOT NULL, PRIMARY KEY (gram, doc)); CREATE INDEX ci_log_grams_doc ON ci_log_grams (doc, gram);")?))
        .map_err(refused)?;
    let mut t = Timed::new();
    let (mut next, step) = (0u64, per_call.max(1));
    while next < rows {
        let (from, to) = (next, rows.min(next + step));
        let began = Instant::now();
        let (inside, cpu) = db
            .call(until(), move |c| {
                let (started, cpu) = (Instant::now(), cpu_ns());
                let mut posting = c.prepare_cached(POSTING)?;
                for i in from..to {
                    // A hundred postings a document, each its own gram: (gram, doc) never repeats.
                    let doc = i / 100 + 1;
                    let gram = (i % 100) * 100_000 + mix(doc) % 100_000;
                    posting.execute(vec![Value::Integer(gram as i64), Value::Integer(doc as i64)])?;
                }
                Ok((started.elapsed(), cpu_since(cpu)))
            })
            .map_err(refused)?;
        t.add(to - from, inside, cpu, began.elapsed());
        next = to;
    }
    Ok(t)
}

fn opts() -> anyhow::Result<Opts> {
    let mut o = Opts {
        dir: PathBuf::new(),
        shapes: vec!["reapi".into(), "logsearch".into(), "insert".into()],
        runs: 3,
        digests: 100_000,
        pages: 10_000,
        rows: 100_000,
        per_call: 1_000,
        pages_per_call: 100,
        parses: 20_000,
        label: String::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || args.next().ok_or_else(|| anyhow::anyhow!("{a} takes a value"));
        match a.as_str() {
            "--sql-api" => {}
            "--dir" => o.dir = PathBuf::from(v()?),
            "--shapes" => o.shapes = v()?.split(',').map(str::to_owned).collect(),
            "--runs" => o.runs = v()?.parse()?,
            "--digests" => o.digests = v()?.parse()?,
            "--pages" => o.pages = v()?.parse()?,
            "--rows" => o.rows = v()?.parse()?,
            "--per-call" => o.per_call = v()?.parse()?,
            "--pages-per-call" => o.pages_per_call = v()?.parse()?,
            "--parses" => o.parses = v()?.parse()?,
            "--label" => o.label = v()?,
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    anyhow::ensure!(!o.dir.as_os_str().is_empty(), "--dir names where the databases go");
    std::fs::create_dir_all(&o.dir)?;
    Ok(o)
}

pub fn main() -> anyhow::Result<()> {
    let o = opts()?;
    for shape in &o.shapes {
        if shape == "parse" {
            let mut best: Vec<(&str, f64, Option<f64>)> = Vec::new();
            for run in 1..=o.runs {
                let load = crate::planner::load_average();
                for (statement, ns, cpu) in parse_run(o.parses)? {
                    println!(
                        "{}",
                        serde_json::json!({ "record": "sql_api", "label": o.label, "shape": "parse", "run": run, "statement": statement,
                            "parses": o.parses, "ns_per_parse": ns, "cpu_ns_per_parse": cpu, "load": load })
                    );
                    match best.iter_mut().find(|(s, _, _)| *s == statement) {
                        Some((_, b, c)) => {
                            *b = b.min(ns);
                            *c = c.zip(cpu).map(|(c, cpu)| c.min(cpu));
                        }
                        None => best.push((statement, ns, cpu)),
                    }
                }
            }
            for (statement, ns, cpu) in best {
                println!(
                    "{}",
                    serde_json::json!({ "record": "best", "label": o.label, "shape": "parse", "runs": o.runs, "statement": statement,
                        "ns_per_parse": ns, "cpu_ns_per_parse": cpu })
                );
            }
            continue;
        }
        let mut best: Option<(f64, f64, Option<f64>)> = None;
        let mut checksums = Vec::new();
        let reapi = (shape == "reapi").then(|| (0..o.digests).map(digest).collect::<Vec<_>>());
        let db = match shape.as_str() {
            "reapi" => {
                let db = fresh(&o.dir, "reapi.db")?;
                reapi_setup(&db, reapi.as_deref().unwrap_or_default())?;
                Some(db)
            }
            "logsearch" => {
                let db = fresh(&o.dir, "logsearch.db")?;
                logsearch_setup(&db, o.pages * 8 + 8)?;
                Some(db)
            }
            "insert" => None,
            other => anyhow::bail!("no shape {other}: reapi, logsearch, insert or parse"),
        };
        let profile = std::env::var("AMPHORA_PROFILE").ok();
        for run in 1..=o.runs {
            let load = crate::planner::load_average();
            let profiler = match &profile {
                Some(_) => Some(crate::profile_start()?),
                None => None,
            };
            let t = match (shape.as_str(), &db) {
                ("reapi", Some(db)) => reapi_run(db, reapi.as_deref().unwrap_or_default(), o.per_call)?,
                ("logsearch", Some(db)) => logsearch_run(db, o.pages, o.pages_per_call)?,
                _ => insert_run(&fresh(&o.dir, &format!("insert-{run}.db"))?, o.rows, o.per_call)?,
            };
            if let (Some(prefix), Some(guard)) = (&profile, profiler) {
                crate::write_profile(&format!("{prefix}-{shape}-{run}"), &guard)?;
            }
            let (inside, whole) = (Timed::per_execute(t.inside, t.executes), Timed::per_execute(t.whole, t.executes));
            let cpu = t.cpu.map(|ns| ns as f64 / t.executes.max(1) as f64);
            best = Some(best.map_or((inside, whole, cpu), |(i, w, c)| (i.min(inside), w.min(whole), c.zip(cpu).map(|(c, cpu)| c.min(cpu)))));
            if shape == "logsearch" {
                checksums.push(t.checksum);
            }
            println!(
                "{}",
                serde_json::json!({ "record": "sql_api", "label": o.label, "shape": shape, "run": run, "executes": t.executes,
                    "inside_ns_per_execute": inside, "inside_cpu_ns_per_execute": cpu, "whole_ns_per_execute": whole, "load": load,
                    "checksum": (shape == "logsearch").then(|| format!("{:016x}", t.checksum)) })
            );
        }
        anyhow::ensure!(checksums.windows(2).all(|w| w.first() == w.last()), "the {shape} runs answered differently: {checksums:x?}");
        if let Some((inside, whole, cpu)) = best {
            println!(
                "{}",
                serde_json::json!({ "record": "best", "label": o.label, "shape": shape, "runs": o.runs,
                    "inside_ns_per_execute": inside, "inside_cpu_ns_per_execute": cpu, "whole_ns_per_execute": whole })
            );
        }
    }
    Ok(())
}
