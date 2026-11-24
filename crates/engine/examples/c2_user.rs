//! Lane C2, diagnosis only: T-QT.6.18's statements run through the public API as a user would.
//! usage: c2_user <fixture.sql> <queries.txt> <db dir> <seconds a statement may take> [only query]

use amphora_engine::morsel::ParallelMode;
use amphora_engine::{ConnectOpts, Database, JoinMethods, OpenOptions, PlatformIO, SqliteDialect, IO};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fixture = std::fs::read_to_string(&args[0])?;
    let queries = std::fs::read_to_string(&args[1])?;
    let deadline = Duration::from_secs(args[3].parse()?);
    let only: Option<usize> = args.get(4).map(|a| a.parse()).transpose()?;
    std::fs::create_dir_all(&args[2])?;
    let path = format!("{}/c2-{}.db", args[2], std::process::id());
    let io: Arc<dyn IO> = Arc::new(PlatformIO::new()?);
    let db = Database::open(io, &path, OpenOptions::new(Arc::new(SqliteDialect)))?;
    let conn = db.connect(ConnectOpts::new())?;
    for statement in fixture.lines() {
        conn.execute(statement)?;
    }
    for line in queries.lines() {
        let mut parts = line.splitn(3, '|');
        let query: usize = parts.next().unwrap().parse()?;
        let rows = parts.next().unwrap();
        let sql = parts.next().unwrap();
        if only.is_some_and(|only| only != query) {
            continue;
        }
        println!("query {query} rows={rows} {sql}");
        for methods in [JoinMethods::ByCost, JoinMethods::HashJoinsOnly] {
            conn.set_join_methods(methods);
            for cores in [0usize, 1, 2, 4, 8] {
                if cores == 0 {
                    conn.set_parallel_mode(ParallelMode::Off);
                } else {
                    conn.set_parallel_mode(ParallelMode::On);
                    conn.set_parallel_cores(NonZeroUsize::new(cores));
                }
                if cores == 0 {
                    let plain = sql.split(" -- ").next().unwrap();
                    let mut plan = conn.prepare(format!("EXPLAIN QUERY PLAN {plain}"))?;
                    plan.run_with_row_callback(|row| {
                        let last = row.get_values().last().map(|v| v.to_string());
                        println!("    {methods:?} plan {}", last.unwrap_or_default());
                        Ok(())
                    })?;
                }
                conn.set_query_timeout(deadline);
                let began = Instant::now();
                let mut stmt = conn.prepare(sql)?;
                let prepared = began.elapsed();
                let mut count = 0u64;
                let mut sum = 0u64;
                let ran = stmt.run_with_row_callback(|row| {
                    count += 1;
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    for value in row.get_values() {
                        value.to_string().hash(&mut hasher);
                    }
                    sum = sum.wrapping_add(hasher.finish());
                    Ok(())
                });
                println!(
                    "  {methods:?} cores {cores}: {count} rows hash {sum:016x} prepare {:.1} ms run {:.1} ms {}",
                    prepared.as_secs_f64() * 1e3,
                    (began.elapsed() - prepared).as_secs_f64() * 1e3,
                    ran.err().map_or(String::new(), |e| e.to_string()),
                );
                std::io::stdout().flush()?;
                conn.set_query_timeout(Duration::ZERO);
            }
        }
    }
    Ok(())
}
