//! Lane C9's probe, not for main: runs the SQL of C9_SQL (a file: fill lines, a line `--`, then queries)
//! at the four settings, with the plan, the bytecode and the row counters.
use amphora_engine::morsel::ParallelMode;
use amphora_engine::{ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect};
use std::sync::Arc;

fn print(conn: &Arc<Connection>, sql: &str) {
    let mut stmt = conn.prepare(sql).unwrap();
    for row in stmt.run_collect_rows().unwrap() {
        println!("    {}", row.iter().map(|v| v.to_string()).collect::<Vec<_>>().join("|"));
    }
}

#[test]
fn c9_probe() {
    let text = std::fs::read_to_string(std::env::var("C9_SQL").unwrap()).unwrap();
    let (fill, queries) = text.split_once("\n--\n").unwrap();
    let verbose = std::env::var("C9_VERBOSE").is_ok();
    for (n, (setting, parallel, statistics)) in [("default", false, false), ("parallel", true, false), ("statistics", false, true), ("parallel, statistics", true, true)].into_iter().enumerate() {
        let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
        let db = Database::open(Arc::new(MemoryIO::new()), &format!("probe{n}.db"), options).unwrap();
        let conn = db.connect(ConnectOpts::new()).unwrap();
        for sql in fill.lines().filter(|l| !l.trim().is_empty()) {
            conn.execute(sql).unwrap();
        }
        if statistics {
            conn.execute("ANALYZE").unwrap();
        }
        if parallel {
            conn.set_parallel_mode(ParallelMode::On);
        }
        println!("#### {setting}");
        for sql in queries.lines().filter(|l| !l.trim().is_empty()) {
            println!("== {sql}");
            println!("  plan:");
            print(&conn, &format!("EXPLAIN QUERY PLAN {sql}"));
            println!("  rows:");
            print(&conn, sql);
            if verbose && n == 0 {
                println!("  bytecode:");
                print(&conn, &format!("EXPLAIN {sql}"));
                conn.set_row_counters(true);
                let mut stmt = conn.prepare(sql).unwrap();
                stmt.run_collect_rows().unwrap();
                for count in stmt.row_counts() {
                    println!("    count {:?} table {:?} rows {}", count.site.kind, count.site.table, count.rows);
                }
                conn.set_row_counters(false);
            }
        }
    }
}
