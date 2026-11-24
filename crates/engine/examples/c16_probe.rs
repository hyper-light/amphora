use amphora_engine::morsel::ParallelMode;
use amphora_engine::{ConnectOpts, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect, Value};
use std::sync::Arc;
fn main() {
    let text = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    for (setting, parallel, statistics) in [("default", false, false), ("parallel", true, false), ("statistics", false, true), ("parallel, statistics", true, true)] {
        println!("== {setting}");
        let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
        let db = Database::open(Arc::new(MemoryIO::new()), &format!("probe_{setting}.db"), options).unwrap();
        let conn = db.connect(ConnectOpts::new()).unwrap();
        let mut analyzed = false;
        if parallel { conn.set_parallel_mode(ParallelMode::On); }
        for sql in text.split(";\n").map(str::trim).filter(|s| !s.is_empty()) {
            let head = sql.trim_start_matches(|c: char| c == '(').to_ascii_uppercase();
            let query = head.starts_with("SELECT") || head.starts_with("WITH") || head.starts_with("EXPLAIN");
            if !query { if let Err(e) = conn.execute(sql) { println!("{sql}: {e}"); } continue; }
            if statistics && !analyzed { conn.execute("ANALYZE").unwrap(); analyzed = true; }
            let explain = head.starts_with("EXPLAIN");
            if explain && setting != "default" { continue; }
            println!("-- {sql}");
            match conn.prepare(sql).and_then(|mut s| s.run_collect_rows()) {
                Ok(rows) => for row in rows { println!("   {}", row.iter().map(|v| match v { Value::Null => "NULL".into(), v => v.to_string() }).collect::<Vec<_>>().join(" | ")); },
                Err(e) => println!("   ERROR {e}"),
            }
        }
    }
}
