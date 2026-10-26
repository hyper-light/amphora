//! A SQL script's plans on Amphora (`--eqp`): the demonstrations the planner's audit keeps
//! (docs/research/planner.md, "The audit"), each rerun on the commit it names and compared with
//! the output kept beside it (bench/planner/audit/run.sh). A diagnosis, not a record.
//!
//! `amphora-bench --eqp --script <file.sql> --dir <dir>`. The database is made new in `dir` for each
//! script. Statements end at a line ending in `;`. Each statement's rows are printed, a JSON value
//! (EXPLAIN QUERY PLAN FORMAT=JSON) pretty-printed; an error is printed and the script goes on.
//! Comment lines direct the statement after them:
//! - `-- keep NAME` prepares it once and keeps it; `-- rerun NAME` runs the kept statement again,
//!   its plan as it was prepared.
//! - `-- counted` runs it on a connection that counts rows (`Connection::set_row_counters`) and
//!   prints, for each table access and join, the planner's estimate beside the rows counted.
//! - `-- time` prints the least time of five to prepare it, the planner's time among it. A time is
//!   the machine's and its load's: run.sh leaves these lines out of its comparison.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use amphora_engine::{ConnectOpts, Database, DatabaseOpts, OpenOptions, PlatformIO, RowCountKind, SqliteDialect, StepResult};

struct Opts {
    script: String,
    dir: String,
}

fn opts() -> anyhow::Result<Opts> {
    let mut o = Opts { script: String::new(), dir: String::new() };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut v = || args.next().ok_or_else(|| anyhow::anyhow!("{a} takes a value"));
        match a.as_str() {
            "--eqp" => {}
            "--script" => o.script = v()?,
            "--dir" => o.dir = v()?,
            other => anyhow::bail!("unknown argument {other} (--eqp takes --script, --dir)"),
        }
    }
    anyhow::ensure!(!o.script.is_empty() && !o.dir.is_empty(), "--eqp needs --script and --dir");
    Ok(o)
}

/// How a statement of the script is run.
enum How {
    Run,
    Keep(String),
    Counted,
    Time,
}

/// One step of the script.
enum Step {
    Sql(How, String),
    Rerun(String),
}

fn steps(text: &str) -> anyhow::Result<Vec<Step>> {
    let mut out = Vec::new();
    let (mut sql, mut how) = (String::new(), How::Run);
    for line in text.lines() {
        let t = line.trim();
        if sql.is_empty() {
            if let Some(name) = t.strip_prefix("-- keep ") {
                how = How::Keep(name.trim().to_owned());
                continue;
            }
            if let Some(name) = t.strip_prefix("-- rerun ") {
                out.push(Step::Rerun(name.trim().to_owned()));
                continue;
            }
            if t == "-- counted" {
                how = How::Counted;
                continue;
            }
            if t == "-- time" {
                how = How::Time;
                continue;
            }
            if t.is_empty() || t.starts_with("--") {
                continue;
            }
        }
        if !sql.is_empty() {
            sql.push('\n');
        }
        sql.push_str(line);
        if t.ends_with(';') {
            let one = sql.trim().trim_end_matches(';').to_owned();
            out.push(Step::Sql(std::mem::replace(&mut how, How::Run), one));
            sql.clear();
        }
    }
    anyhow::ensure!(sql.trim().is_empty(), "the script ends inside a statement: {sql}");
    Ok(out)
}

/// Steps `stmt` to its end, each row's values printed unless `quiet`.
fn drive(io: &Arc<dyn amphora_engine::IO>, stmt: &mut amphora_engine::Statement, quiet: bool) -> anyhow::Result<u64> {
    let mut rows = 0u64;
    loop {
        match stmt.step().map_err(|e| anyhow::anyhow!("{e}"))? {
            StepResult::Row => {
                rows += 1;
                if quiet {
                    continue;
                }
                if let Some(row) = stmt.row() {
                    let cells: Vec<String> = row.get_values().map(|v| v.to_string()).collect();
                    let line = cells.join(" | ");
                    match serde_json::from_str::<serde_json::Value>(&line) {
                        Ok(json @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) => println!("{}", serde_json::to_string_pretty(&json)?),
                        _ => println!("{line}"),
                    }
                }
            }
            StepResult::Done => return Ok(rows),
            StepResult::IO | StepResult::Yield => io.step().map_err(|e| anyhow::anyhow!("{e}"))?,
            StepResult::Sleep { duration } => {
                std::thread::sleep(duration);
                io.step().map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            StepResult::Interrupt => anyhow::bail!("interrupted"),
            StepResult::Busy => anyhow::bail!("busy"),
        }
    }
}

pub fn main() -> anyhow::Result<()> {
    let o = opts()?;
    let text = std::fs::read_to_string(&o.script)?;
    let dir = Path::new(&o.dir);
    std::fs::create_dir_all(dir)?;
    let path = dir.join("eqp.db");
    for stale in [path.clone(), dir.join("eqp.db-wal"), dir.join("eqp.db-shm")] {
        if stale.exists() {
            std::fs::remove_file(stale)?;
        }
    }
    let io: Arc<dyn amphora_engine::IO> = Arc::new(PlatformIO::new().map_err(|e| anyhow::anyhow!("{e}"))?);
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io.clone(), path.to_str().unwrap_or_default(), options).map_err(|e| anyhow::anyhow!("{e}"))?;
    let conn = db.connect(ConnectOpts::new()).map_err(|e| anyhow::anyhow!("{e}"))?;
    let counting = db.connect(ConnectOpts::new()).map_err(|e| anyhow::anyhow!("{e}"))?;
    counting.set_row_counters(true);
    let mut kept: HashMap<String, amphora_engine::Statement> = HashMap::new();
    for step in steps(&text)? {
        let ran = match step {
            Step::Sql(How::Run, sql) => {
                println!("> {sql};");
                conn.prepare(&sql).map_err(|e| anyhow::anyhow!("{e}")).and_then(|mut stmt| drive(&io, &mut stmt, false)).map(|_| ())
            }
            Step::Sql(How::Keep(name), sql) => {
                println!("> -- keep {name}\n> {sql};");
                match conn.prepare(&sql).map_err(|e| anyhow::anyhow!("{e}")) {
                    Ok(mut stmt) => {
                        let ran = drive(&io, &mut stmt, false).map(|_| ());
                        kept.insert(name, stmt);
                        ran
                    }
                    Err(e) => Err(e),
                }
            }
            Step::Sql(How::Counted, sql) => {
                println!("> -- counted\n> {sql};");
                counted(&io, &counting, &sql)
            }
            Step::Sql(How::Time, sql) => {
                println!("> -- time\n> {sql};");
                let mut least = Duration::MAX;
                let mut failed = None;
                for _ in 0..5 {
                    let began = Instant::now();
                    match conn.prepare(&sql) {
                        Ok(stmt) => drop(stmt),
                        Err(e) => {
                            failed = Some(anyhow::anyhow!("{e}"));
                            break;
                        }
                    }
                    least = least.min(began.elapsed());
                }
                match failed {
                    Some(e) => Err(e),
                    None => {
                        println!("prepare: {:.3} ms", least.as_secs_f64() * 1000.0);
                        Ok(())
                    }
                }
            }
            Step::Rerun(name) => {
                println!("> -- rerun {name}");
                match kept.get_mut(&name) {
                    Some(stmt) => stmt.reset().map_err(|e| anyhow::anyhow!("{e}")).and_then(|()| drive(&io, stmt, false)).map(|_| ()),
                    None => Err(anyhow::anyhow!("no statement kept as {name}")),
                }
            }
        };
        if let Err(e) = ran {
            println!("error: {e}");
        }
        println!();
    }
    Ok(())
}

/// Runs `sql` counting its rows, then prints each counter: the plan it is in (0 the statement's,
/// a subquery's its own), its kind, its table, and the estimate beside the rows counted.
fn counted(io: &Arc<dyn amphora_engine::IO>, conn: &Arc<amphora_engine::Connection>, sql: &str) -> anyhow::Result<()> {
    let mut stmt = conn.prepare(sql).map_err(|e| anyhow::anyhow!("{e}"))?;
    let rows = drive(io, &mut stmt, true)?;
    println!("rows: {rows}");
    for c in stmt.row_counts() {
        let kind = match c.site.kind {
            RowCountKind::Loops => "loops",
            RowCountKind::Prefix => "prefix",
            RowCountKind::HashBuild => "hash_build",
        };
        let table = c.site.table.unwrap_or_default();
        match c.site.estimate {
            Some(e) => println!("plan {} {kind} {table} position {}: estimated {} rows, counted {}", c.site.plan, e.position, e.output_rows, c.rows),
            None => println!("plan {} {kind} {table}: no estimate, counted {}", c.site.plan, c.rows),
        }
    }
    Ok(())
}
