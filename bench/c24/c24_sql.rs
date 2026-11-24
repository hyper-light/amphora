//! Not for main: runs a file of cases ($C24_SQL) under every setting and prints what differs from SQLite's answers.
use amphora_engine::{ConnectOpts, Connection, Database, DatabaseOpts, JoinMethods, MemoryIO, Numeric, OpenOptions, SqliteDialect, StepResult, Value, IO};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
fn run(io: &MemoryIO, conn: &Arc<Connection>, sql: &str, sorted: bool) -> Result<String, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    loop {
        match stmt.step().map_err(|e| e.to_string())? {
            StepResult::Row => {
                let row = stmt.row().unwrap();
                out.push(row.get_values().map(|v| match v {
                    Value::Null => "NULL".to_owned(),
                    Value::Numeric(Numeric::Float(f)) => format!("{:?}", f64::from(*f)),
                    Value::Text(text) => format!("'{}'", text.as_str()),
                    Value::Blob(blob) => format!("x'{}'", blob.iter().map(|b| format!("{b:02x}")).collect::<String>()),
                    value => value.to_string(),
                }).collect::<Vec<String>>().join(","));
            }
            StepResult::Done => break,
            StepResult::IO => io.step().map_err(|e| e.to_string())?,
            other => return Err(format!("{other:?}")),
        }
    }
    if sorted { out.sort(); Ok(format!("{}:{}", out.len(), out.join(";"))) } else { Ok(out.join("\n")) }
}
fn open(name: &str, parallel: bool, joins: JoinMethods, n: usize) -> (Arc<MemoryIO>, Arc<Connection>) {
    let io = Arc::new(MemoryIO::new());
    let db = Database::open(io.clone(), &format!("c24_{name}_{n}.db"), OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new().with_without_rowid(true))).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    if parallel { conn.set_parallel_mode(amphora_engine::morsel::ParallelMode::On); }
    conn.set_join_methods(joins);
    (io, conn)
}
#[test]
fn c24_sql() {
    let text = std::fs::read_to_string(std::env::var("C24_SQL").unwrap()).unwrap();
    let only = std::env::var("C24_ONE").ok();
    let quiet = std::env::var("C24_QUIET").is_ok();
    let mut n = 0;
    for (name, parallel, statistics, joins) in [
        ("default", false, false, JoinMethods::ByCost), ("parallel", true, false, JoinMethods::ByCost),
        ("statistics", false, true, JoinMethods::ByCost), ("both", true, true, JoinMethods::ByCost),
        ("hash", false, false, JoinMethods::HashJoinsOnly), ("hash+stats", false, true, JoinMethods::HashJoinsOnly),
        ("merge", false, false, JoinMethods::MergeJoinsOnly), ("loops", false, false, JoinMethods::NestedLoopsOnly)] {
        if only.as_ref().is_some_and(|one| !one.split(',').any(|o| o == name)) { continue; }
        let (mut asked, mut wrong) = (0, 0);
        let mut setup: Vec<String> = Vec::new();
        let mut analyzed = false;
        n += 1;
        let (mut io, mut conn) = open(name, parallel, joins, n);
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                ["db", ..] => { n += 1; setup.clear(); analyzed = false; (io, conn) = open(name, parallel, joins, n); }
                ["sql", sql] => { setup.push((*sql).to_owned()); if let Err(e) = run(&io, &conn, sql, false) { println!("[{name}] REFUSED {sql}: {e}"); } }
                ["e", sql] => {
                    if statistics && !analyzed { analyzed = true; let _ = run(&io, &conn, "ANALYZE", false); }
                    match catch_unwind(AssertUnwindSafe(|| run(&io, &conn, sql, false))) { Ok(r) => println!("--- [{name}] {sql}\n{}", r.unwrap_or_else(|e| format!("ERROR {e}"))), Err(_) => println!("--- [{name}] {sql}\nPANIC") }
                }
                ["q", sql, rest @ ..] => {
                    if statistics && !analyzed { analyzed = true; let _ = run(&io, &conn, "ANALYZE", false); }
                    asked += 1;
                    let got = match catch_unwind(AssertUnwindSafe(|| run(&io, &conn, sql, true))) {
                        Ok(Ok(r)) => r,
                        Ok(Err(e)) => format!("ERROR {e}"),
                        Err(p) => {
                            n += 1; (io, conn) = open(name, parallel, joins, n);
                            for s in &setup { let _ = run(&io, &conn, s, false); }
                            if statistics { let _ = run(&io, &conn, "ANALYZE", false); }
                            format!("PANIC {:?}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|t| t.to_string())))
                        }
                    };
                    match rest.first() {
                        Some(want) if *want == got => {}
                        Some(want) => { wrong += 1; if !quiet { println!("[{name}] {sql}\n    SQLite  {want}\n    Amphora {got}"); } }
                        None => println!("[{name}] {sql}\n    {got}"),
                    }
                }
                _ => {}
            }
        }
        println!("== [{name}] {wrong} of {asked} differ");
    }
}
