//! Not for main: runs a file of SQL ($C23_SQL) and prints rows, errors or panics; a panic reopens the database and replays the setup.
use amphora_engine::{ConnectOpts, JoinMethods, Connection, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect, StepResult, Value, IO};
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
    let db = Database::open(io.clone(), &format!("c23_{name}_{n}.db"), OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new().with_without_rowid(true))).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    if parallel { conn.set_parallel_mode(amphora_engine::morsel::ParallelMode::On); }
    conn.set_join_methods(joins);
    (io, conn)
}
#[test]
fn c23_sql() {
    let text = std::fs::read_to_string(std::env::var("C23_SQL").unwrap()).unwrap();
    for (name, parallel, joins) in [("default", false, JoinMethods::ByCost), ("parallel", true, JoinMethods::ByCost), ("statistics", false, JoinMethods::ByCost), ("hash", false, JoinMethods::HashJoinsOnly), ("loops", false, JoinMethods::NestedLoopsOnly)] {
        let mut analyzed = false;
        if std::env::var("C23_ONE").is_ok_and(|one| one != name) { continue; }
        let mut n = 0;
        let (mut io, mut conn) = open(name, parallel, joins, n);
        let mut setup: Vec<&str> = Vec::new();
        let mut asked = false;
        for sql in text.split(";\n").map(str::trim).filter(|s| !s.is_empty()) {
            let is_setup = !asked && ["CREATE", "INSERT", "ANALYZE", "DROP", "DELETE"].iter().any(|w| sql.starts_with(w));
            asked |= !is_setup;
            let write = !is_setup && ["INSERT", "UPDATE", "DELETE"].iter().any(|w| sql.starts_with(w));
            if write { run(&io, &conn, "BEGIN").unwrap(); }
            if is_setup { setup.push(sql); } else if name == "statistics" && !analyzed { analyzed = true; run(&io, &conn, "ANALYZE").unwrap(); println!("--- [{name}] {sql}"); } else { println!("--- [{name}] {sql}"); }
            let answer = catch_unwind(AssertUnwindSafe(|| run(&io, &conn, sql)));
            if write { let _ = run(&io, &conn, "ROLLBACK"); }
            match answer {
                Ok(Ok(rows)) if sql.starts_with("EXPLAIN") => for r in rows { println!("{r}"); },
                Ok(Ok(mut rows)) => if !is_setup { rows.sort(); println!("{}:{}", rows.len(), rows.join(";")) },
                Ok(Err(e)) => println!("ERROR {e}"),
                Err(p) => {
                    println!("PANIC {:?}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|t| t.to_string())));
                    n += 1;
                    (io, conn) = open(name, parallel, joins, n);
                    for s in &setup { run(&io, &conn, s).unwrap(); }
                }
            }
        }
    }
}
