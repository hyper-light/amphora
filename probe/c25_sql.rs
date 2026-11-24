//! Not for main: runs a file of SQL ($C25_SQL; cases split by a line `----`) under four settings, each case in a database of its own, and prints rows, refusals or panics.
use amphora_engine::morsel::ParallelMode;
use amphora_engine::{ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect, Value, Numeric};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
fn run(conn: &Arc<Connection>, sql: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt.run_collect_rows().map_err(|e| e.to_string())?;
    Ok(rows.iter().map(|row| row.iter().map(|v| match v {
        Value::Null => "NULL".to_owned(),
        Value::Numeric(Numeric::Float(f)) => format!("{:?}", f64::from(*f)),
        Value::Text(t) => format!("'{}'", t.as_str()),
        Value::Blob(b) => format!("x'{}'", b.iter().map(|x| format!("{x:02x}")).collect::<String>()),
        v => v.to_string(),
    }).collect::<Vec<_>>().join(", ")).collect())
}
#[test]
fn c25_sql() {
    let text = std::fs::read_to_string(std::env::var("C25_SQL").unwrap()).unwrap();
    let generated = std::env::var("C25_GENERATED").is_ok();
    let mut n = 0;
    for case in text.split("----\n") {
        println!("=====");
        for (name, parallel, statistics) in [("default", false, false), ("parallel", true, false), ("statistics", false, true), ("both", true, true)] {
            n += 1;
            let opts = DatabaseOpts::new().with_without_rowid(true).with_generated_columns(generated);
            let db = Database::open(Arc::new(MemoryIO::new()), &format!("c25_{n}.db"), OpenOptions::new(Arc::new(SqliteDialect)).db_opts(opts)).unwrap();
            let conn = db.connect(ConnectOpts::new()).unwrap();
            if parallel { conn.set_parallel_mode(ParallelMode::On); }
            let mut analyzed = false;
            for sql in case.split(";\n").map(str::trim).filter(|s| !s.is_empty()) {
                let setup = sql.starts_with("CREATE");
                let is_setup = setup || sql.starts_with("INSERT") && !sql.contains("ON CONFLICT") || sql.starts_with("PRAGMA foreign");
                if statistics && !analyzed && !is_setup { analyzed = true; run(&conn, "ANALYZE").unwrap(); }
                if name != "default" && sql.starts_with("EXPLAIN") { continue; }
                match catch_unwind(AssertUnwindSafe(|| run(&conn, sql))) {
                    Ok(Ok(rows)) if rows.is_empty() && !sql.starts_with("SELECT") => if !setup && name == "default" { println!("   [{name}] ok {sql}") },
                    Ok(Ok(rows)) if sql.starts_with("EXPLAIN") => { println!("   {sql}"); for r in rows { println!("      {r}"); } },
                    Ok(Ok(rows)) => println!("   [{name}] {sql}\n      {}", rows.join(" / ")),
                    Ok(Err(e)) => println!("   [{name}] {sql}\n      ERROR {e}"),
                    Err(p) => println!("   [{name}] {sql}\n      PANIC {:?}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|t| t.to_string()))),
                }
            }
        }
    }
}
