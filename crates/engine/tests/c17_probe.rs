//! Lane C17's scratch probe (not committed): runs the SQL file named by C17_SQL under the settings.
use amphora_engine::{ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect, StepResult, Value, IO};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
fn open(n: usize) -> (Arc<MemoryIO>, Arc<Connection>) {
    let io = Arc::new(MemoryIO::new());
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new().with_without_rowid(true));
    let db = Database::open(io.clone(), &format!("c17_probe_{n}.db"), options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    (io, conn)
}
fn answer(io: &MemoryIO, conn: &Arc<Connection>, sql: &str, explain: bool) -> Result<String, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let mut lines = Vec::new();
    loop {
        match stmt.step().map_err(|e| e.to_string())? {
            StepResult::Row => {
                let row = stmt.row().unwrap();
                let v: Vec<String> = row.get_values().map(|v| match v { Value::Null => "NULL".to_owned(), v => v.to_string() }).collect();
                lines.push(v.join(if explain { "  " } else { "," }));
            }
            StepResult::Done => break,
            StepResult::IO => io.step().map_err(|e| e.to_string())?,
            other => return Err(format!("step: {other:?}")),
        }
    }
    Ok(if explain { lines.join("\n") } else { format!("{}:{}", lines.len(), lines.join(";")) })
}
fn outcome(io: &MemoryIO, conn: &Arc<Connection>, sql: &str, explain: bool) -> String {
    match catch_unwind(AssertUnwindSafe(|| answer(io, conn, sql, explain))) {
        Ok(Ok(rows)) => rows,
        Ok(Err(e)) => format!("ERROR {e}"),
        Err(p) => format!("PANIC {}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|t| t.to_string())).unwrap_or_default()),
    }
}
#[test]
fn probe() {
    let text = std::fs::read_to_string(std::env::var("C17_SQL").unwrap()).unwrap();
    let settings: &[(&str, bool, bool)] = &[("default", false, false), ("parallel", true, false), ("statistics", false, true), ("both", true, true)];
    let mut n = 0;
    let mut dbs: Vec<(String, Vec<(char, String)>)> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("db ") { dbs.push((name.to_owned(), vec![])); }
        else if let Some(s) = line.strip_prefix("sql ") { dbs.last_mut().unwrap().1.push(('s', s.to_owned())); }
        else if let Some(s) = line.strip_prefix("q ") { dbs.last_mut().unwrap().1.push(('q', s.to_owned())); }
        else if let Some(s) = line.strip_prefix("x ") { dbs.last_mut().unwrap().1.push(('x', s.to_owned())); }
    }
    let quiet = std::env::var("C17_QUIET").is_ok();
    for (name, steps) in &dbs {
        println!("##### {name}");
        for (set, parallel, statistics) in settings {
            if std::env::var("C17_DEFAULT").is_ok() && *set != "default" { continue; }
            n += 1;
            let (io, conn) = open(n);
            if *parallel { conn.set_parallel_mode(amphora_engine::morsel::ParallelMode::On); }
            let mut analyzed = false;
            for (kind, s) in steps {
                match kind {
                    's' => { let out = outcome(&io, &conn, s, false); if out.starts_with("ERROR") || out.starts_with("PANIC") || !quiet { println!("[{set}] {s}\n     => {out}"); } }
                    'q' => { if *statistics && !analyzed { outcome(&io, &conn, "ANALYZE", false); analyzed = true; } println!("[{set}] {s}\n     => {}", outcome(&io, &conn, s, false)); }
                    _ => { println!("[{set}] EXPLAIN {s}\n{}", outcome(&io, &conn, &format!("EXPLAIN {s}"), true)); }
                }
            }
        }
    }
}
