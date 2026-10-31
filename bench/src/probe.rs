//! Probes (`--probe`): the statements bench/corpus/probes.py writes for every entry of the SQLite
//! language's inventory, each on a database of its own, their answers or errors as JSON beside the
//! other engines' (bench/corpus/probe.py), so bench/corpus/matrix.py can say which engines support
//! each entry and whether they answer as SQLite does. A diagnosis of support, not a measured run.
//!
//! `amphora-bench --probe --target amphora|turso --probes <probes.json> --out <out.json> [--dir <scratch>] [--label <version>]`.
//! A probe that panics is recorded as such and the run goes on.

use std::num::NonZero;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value as J, json};

struct Opts {
    target: String,
    probes: String,
    out: String,
    dir: String,
    /// What was probed, as the matrix names it (the commit, the upstream revision).
    label: String,
}

fn opts() -> anyhow::Result<Opts> {
    let mut o = Opts { target: String::new(), probes: String::new(), out: String::new(), dir: format!("{}/.cache/amphora-probe", std::env::var("HOME").unwrap_or_default()), label: String::new() };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || args.next().ok_or_else(|| anyhow::anyhow!("{a} takes a value"));
        match a.as_str() {
            "--probe" => {}
            "--target" => o.target = v()?,
            "--probes" => o.probes = v()?,
            "--out" => o.out = v()?,
            "--dir" => o.dir = v()?,
            "--label" => o.label = v()?,
            other => anyhow::bail!("unknown argument {other} (--probe takes --target, --probes, --out, --dir, --label)"),
        }
    }
    anyhow::ensure!(!o.probes.is_empty() && !o.out.is_empty(), "--probe needs --probes and --out");
    Ok(o)
}

/// One engine's way to run a statement on a fresh database and hand back its rows.
trait Engine {
    fn run(&mut self, sql: &str, params: &[J]) -> anyhow::Result<Vec<Vec<J>>>;
}

fn param_values(params: &J) -> Vec<J> {
    match params {
        J::Array(a) => a.clone(),
        // Named parameters are numbered in the order they appear; the probes name one each.
        J::Object(m) => m.values().cloned().collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------- Amphora

struct Amphora {
    db: amphora::db::Db,
}

fn amphora_param(v: &J) -> amphora_engine::Value {
    use amphora_engine::Value;
    match v {
        J::Null => Value::Null,
        J::Bool(b) => Value::from_i64(i64::from(*b)),
        J::Number(n) => n.as_i64().map(Value::from_i64).unwrap_or_else(|| Value::from_f64(n.as_f64().unwrap_or(0.0))),
        J::String(s) => Value::from_text(s.clone()),
        _ => Value::Null,
    }
}

fn amphora_json(v: &amphora_engine::Value) -> J {
    use amphora_engine::{Numeric, Value};
    match v {
        Value::Null => J::Null,
        Value::Numeric(Numeric::Integer(i)) => json!(i),
        Value::Numeric(Numeric::Float(f)) => json!(f64::from(*f)),
        Value::Text(_) => json!(v.to_text().unwrap_or_default()),
        Value::Blob(_) => json!(format!("x'{}'", v.to_blob().unwrap_or_default().iter().map(|b| format!("{b:02x}")).collect::<String>())),
    }
}

impl Engine for Amphora {
    fn run(&mut self, sql: &str, params: &[J]) -> anyhow::Result<Vec<Vec<J>>> {
        // The engine keeps statements by their text's address: a probe's few are leaked.
        let sql: &'static str = Box::leak(sql.to_owned().into_boxed_str());
        let mut rows = Vec::new();
        self.db.query(sql, params.iter().map(amphora_param), |r| {
            rows.push(r.get_values().map(amphora_json).collect());
            Ok(true)
        })?;
        Ok(rows)
    }
}

// ---------------------------------------------------------------- Turso, upstream at 3e09a82

struct Turso {
    io: Arc<dyn turso_core::IO>,
    conn: Arc<turso_core::Connection>,
}

fn turso_param(v: &J) -> turso_core::Value {
    use turso_core::Value;
    match v {
        J::Null => Value::Null,
        J::Bool(b) => Value::from_i64(i64::from(*b)),
        J::Number(n) => n.as_i64().map(Value::from_i64).unwrap_or_else(|| Value::from_f64(n.as_f64().unwrap_or(0.0))),
        J::String(s) => Value::from_text(s.clone()),
        _ => Value::Null,
    }
}

fn turso_json(v: &turso_core::Value) -> J {
    use turso_core::{Numeric, Value};
    match v {
        Value::Null => J::Null,
        Value::Numeric(Numeric::Integer(i)) => json!(i),
        Value::Numeric(Numeric::Float(f)) => json!(f64::from(*f)),
        Value::Text(_) => json!(v.to_text().unwrap_or_default()),
        Value::Blob(_) => json!(format!("x'{}'", v.to_blob().unwrap_or_default().iter().map(|b| format!("{b:02x}")).collect::<String>())),
    }
}

impl Engine for Turso {
    fn run(&mut self, sql: &str, params: &[J]) -> anyhow::Result<Vec<Vec<J>>> {
        let mut stmt = self.conn.prepare(sql).map_err(|e| anyhow::anyhow!("{e}"))?;
        for (i, p) in params.iter().enumerate() {
            stmt.bind_at(NonZero::new(i + 1).ok_or_else(|| anyhow::anyhow!("parameter 0"))?, turso_param(p)).map_err(|e| anyhow::anyhow!("bind: {e}"))?;
        }
        let mut rows = Vec::new();
        loop {
            match stmt.step().map_err(|e| anyhow::anyhow!("{e}"))? {
                turso_core::StepResult::Row => {
                    if let Some(r) = stmt.row() {
                        rows.push(r.get_values().map(turso_json).collect());
                    }
                }
                turso_core::StepResult::Done => return Ok(rows),
                turso_core::StepResult::IO => self.io.step().map_err(|e| anyhow::anyhow!("{e}"))?,
                other => anyhow::bail!("step: {other:?}"),
            }
        }
    }
}

fn fresh(target: &str, path: &Path) -> anyhow::Result<Box<dyn Engine>> {
    Ok(match target {
        "amphora" => Box::new(Amphora { db: amphora::db::Db::open(path)? }),
        "turso" => {
            let io: Arc<dyn turso_core::IO> = Arc::new(turso_core::PlatformIO::new().map_err(|e| anyhow::anyhow!("{e}"))?);
            let name = path.to_str().ok_or_else(|| anyhow::anyhow!("a path that isn't UTF-8"))?;
            let db = turso_core::Database::open(io.clone(), name, turso_core::OpenOptions::new(Arc::new(turso_core::SqliteDialect)).db_opts(turso_core::DatabaseOpts::new()))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let conn = db.connect(turso_core::ConnectOpts::new()).map_err(|e| anyhow::anyhow!("{e}"))?;
            Box::new(Turso { io, conn })
        }
        t => anyhow::bail!("--probe runs amphora or turso, not {t} (bench/corpus/probe.py runs the rest)"),
    })
}

fn run_probe(target: &str, dir: &Path, n: usize, p: &J) -> anyhow::Result<Vec<Vec<J>>> {
    let path: PathBuf = dir.join(format!("{n}.db"));
    let mut e = fresh(target, &path)?;
    // What SQLite's dialect needs first to behave as the other engines do by default (foreign keys).
    for s in p["sqlite_setup"].as_array().into_iter().flatten().chain(p["setup"].as_array().into_iter().flatten()) {
        e.run(s.as_str().unwrap_or_default(), &[])?;
    }
    let parts: Vec<&str> = p["parts"].as_array().into_iter().flatten().filter_map(J::as_str).collect();
    let params = param_values(&p["params"]);
    let mut rows = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        rows = e.run(part, if last { &params } else { &[] })?;
    }
    Ok(rows)
}

pub fn main() -> anyhow::Result<()> {
    let o = opts()?;
    let probes: J = serde_json::from_str(&std::fs::read_to_string(&o.probes)?)?;
    let dir = Path::new(&o.dir).join(format!("{}-{}", o.target, std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let began = std::time::Instant::now();
    let mut results = Vec::new();
    for (n, p) in probes["probes"].as_array().into_iter().flatten().enumerate() {
        let id = p["id"].clone();
        let got = std::panic::catch_unwind(AssertUnwindSafe(|| run_probe(&o.target, &dir, n, p)));
        let mut r = match got {
            Ok(Ok(rows)) => json!({ "id": id, "status": "ok", "rows": rows }),
            Ok(Err(e)) => json!({ "id": id, "status": "error", "error": format!("{e:#}").chars().take(300).collect::<String>() }),
            Err(panic) => {
                let msg = panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned())).unwrap_or_default();
                json!({ "id": id, "status": "panic", "error": msg.chars().take(300).collect::<String>() })
            }
        };
        if let Some(req) = p.get("requires") {
            r["requires"] = req.clone();
        }
        results.push(r);
    }
    let _ = std::fs::remove_dir_all(&dir);
    let ok = results.iter().filter(|r| r["status"] == "ok").count();
    std::fs::write(&o.out, serde_json::to_string_pretty(&json!({ "engine": o.target, "version": o.label, "seconds": began.elapsed().as_secs_f64(), "results": results }))?)?;
    eprintln!("{}: {ok} of {} probes ran", o.target, results.len());
    Ok(())
}
