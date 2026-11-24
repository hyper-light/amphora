//! Not for main: runs a file of SQL ($C10_SQL) and prints rows, errors or panics; a panic reopens the database and replays the setup.
use amphora_engine::{ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect, StepResult, Value, IO};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
fn run(io: &MemoryIO, conn: &Arc<Connection>, sql: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    loop {
        match stmt.step().map_err(|e| e.to_string())? {
            StepResult::Row => {
                let row = stmt.row().unwrap();
                out.push(row.get_values().map(|v| match v { Value::Null => "NULL".into(), v => v.to_string() }).collect::<Vec<String>>().join(","));
            }
            StepResult::Done => return Ok(out),
            StepResult::IO => io.step().map_err(|e| e.to_string())?,
            other => return Err(format!("{other:?}")),
        }
    }
}
fn open(name: &str, parallel: bool, n: usize) -> (Arc<MemoryIO>, Arc<Connection>) {
    let io = Arc::new(MemoryIO::new());
    let db = Database::open(io.clone(), &format!("c10_{name}_{n}.db"), OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new().with_without_rowid(true))).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    if parallel { conn.set_parallel_mode(amphora_engine::morsel::ParallelMode::On); }
    (io, conn)
}
#[test]
fn c10_sql() {
    let text = std::fs::read_to_string(std::env::var("C10_SQL").unwrap()).unwrap();
    for (name, parallel) in [("default", false), ("parallel", true)] {
        if std::env::var("C10_ONE").is_ok() && parallel { continue; }
        let mut n = 0;
        let (mut io, mut conn) = open(name, parallel, n);
        let mut setup: Vec<&str> = Vec::new();
        for sql in text.split(";\n").map(str::trim).filter(|s| !s.is_empty()) {
            let is_setup = ["CREATE", "INSERT", "ANALYZE", "DROP", "DELETE"].iter().any(|w| sql.starts_with(w));
            if is_setup { setup.push(sql); } else { println!("--- [{name}] {sql}"); }
            match catch_unwind(AssertUnwindSafe(|| run(&io, &conn, sql))) {
                Ok(Ok(rows)) if sql.starts_with("EXPLAIN") => for r in rows { println!("{r}"); },
                Ok(Ok(mut rows)) => if !is_setup { rows.sort(); println!("{}:{}", rows.len(), rows.join(";")) },
                Ok(Err(e)) => println!("ERROR {e}"),
                Err(p) => {
                    println!("PANIC {:?}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|t| t.to_string())));
                    n += 1;
                    (io, conn) = open(name, parallel, n);
                    for s in &setup { run(&io, &conn, s).unwrap(); }
                }
            }
        }
    }
}
