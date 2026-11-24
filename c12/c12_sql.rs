//! Not for main: runs a file of SQL ($C12_SQL) under each setting and prints rows, errors or panics.
use amphora_engine::{ConnectOpts, Connection, Database, DatabaseOpts, JoinMethods, MemoryIO, OpenOptions, SqliteDialect, StepResult, Value, IO};
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
fn open(name: &str, parallel: bool, joins: JoinMethods, n: usize) -> (Arc<MemoryIO>, Arc<Connection>) {
    let io = Arc::new(MemoryIO::new());
    let db = Database::open(io.clone(), &format!("c12_{name}_{n}.db"), OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new().with_without_rowid(true))).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    if parallel { conn.set_parallel_mode(amphora_engine::morsel::ParallelMode::On); }
    conn.set_join_methods(joins);
    (io, conn)
}
#[test]
fn c12_sql() {
    let text = std::fs::read_to_string(std::env::var("C12_SQL").unwrap()).unwrap();
    let only = std::env::var("C12_ONE").ok();
    for (name, parallel, stats, joins) in [
        ("default", false, false, JoinMethods::ByCost),
        ("parallel", true, false, JoinMethods::ByCost),
        ("stats", false, true, JoinMethods::ByCost),
        ("stats+parallel", true, true, JoinMethods::ByCost),
        ("hash", false, false, JoinMethods::HashJoinsOnly),
        ("loops", false, false, JoinMethods::NestedLoopsOnly),
    ] {
        if only.as_ref().is_some_and(|o| o != name) { continue; }
        let mut n = 0;
        let (mut io, mut conn) = open(name, parallel, joins, n);
        let mut setup: Vec<&str> = Vec::new();
        let mut analyzed = false;
        for sql in text.split(";\n").map(str::trim).filter(|s| !s.is_empty()) {
            let is_setup = ["CREATE", "INSERT", "DROP", "DELETE"].iter().any(|w| sql.starts_with(w));
            if is_setup { setup.push(sql); } else {
                if stats && !analyzed { run(&io, &conn, "ANALYZE").unwrap(); analyzed = true; }
                println!("--- [{name}] {sql}");
            }
            match catch_unwind(AssertUnwindSafe(|| run(&io, &conn, sql))) {
                Ok(Ok(rows)) if sql.starts_with("EXPLAIN") => for r in rows { println!("{r}"); },
                Ok(Ok(mut rows)) => if !is_setup { rows.sort(); println!("{}:{}", rows.len(), rows.join(";")) },
                Ok(Err(e)) => println!("ERROR {e}"),
                Err(p) => {
                    println!("PANIC {:?}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|t| t.to_string())));
                    n += 1;
                    (io, conn) = open(name, parallel, joins, n);
                    for s in &setup { run(&io, &conn, s).unwrap(); }
                    if stats { run(&io, &conn, "ANALYZE").unwrap(); }
                }
            }
        }
    }
}
