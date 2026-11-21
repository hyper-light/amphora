//! Index nested loops against a hash join over the same two tables, at build sizes from 10^2 to
//! 10^7 rows (QT.5). These times are the check on the planner's join costs, not their source: the
//! costs are learned where the engine runs (Ada's rule, 2026-11-21). Each size loads `a(id, k)` and `b(id, key UNIQUE, v)` of `n` rows each, `a.k` naming
//! every `b.key` once in shuffled order, with the cache sized to hold the database whole. Then, in
//! turns, best of `rounds`: the join planned with index nested loops only (b's unique index searched
//! once an outer row), the same join with hash joins only (`Connection::set_join_methods`), and each
//! table's scan alone, the baseline the joins' own work is measured over.
//! Each run's start load is recorded, and each query's plan, so a run that didn't take the method
//! it names says so. One JSON object a line on stdout.
//!
//!   join-methods <dir> [rounds] [sizes...]

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, JoinMethods, OpenOptions, PlatformIO,
    SqliteDialect, Value,
};
use std::sync::Arc;
use std::time::Instant;

const JOIN: &str = "SELECT sum(b.v) FROM a JOIN b ON b.key = a.k";

/// Each query, the join methods it is planned under, and its SQL.
const QUERIES: [(&str, JoinMethods, &str); 4] = [
    ("index_nested_loops", JoinMethods::NestedLoopsOnly, JOIN),
    ("hash_join", JoinMethods::HashJoinsOnly, JOIN),
    ("scan_a", JoinMethods::ByCost, "SELECT sum(k) FROM a"),
    ("scan_b", JoinMethods::ByCost, "SELECT sum(v) FROM b NOT INDEXED"),
];

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().ok_or("usage: join-methods <dir> [rounds] [sizes...]")?;
    let rounds: usize = args.next().map_or(Ok(3), |r| r.parse()).map_err(|e| format!("{e}"))?;
    let mut sizes: Vec<i64> = args.map(|s| s.parse().map_err(|e| format!("{e}"))).collect::<Result<_, String>>()?;
    if sizes.is_empty() {
        sizes = (2..=7).map(|e| 10i64.pow(e)).collect();
    }
    for n in sizes {
        let path = std::path::Path::new(&dir).join(format!("jm-{n}.db"));
        for suffix in ["", "-wal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        let conn = load(&path, n)?;
        let pages = single_int(&conn, "PRAGMA page_count")?;
        // Twice the file's pages: the database and its working pages held whole.
        conn.execute(format!("PRAGMA cache_size = {}", pages * 2)).map_err(|e| e.to_string())?;
        let plans: Vec<String> = QUERIES
            .iter()
            .map(|(_, methods, sql)| {
                conn.set_join_methods(*methods);
                plan(&conn, sql)
            })
            .collect::<Result<_, _>>()?;
        let mut best = [f64::INFINITY; QUERIES.len()];
        let mut answers = vec![String::new(); QUERIES.len()];
        // A warm pass, untimed, then the rounds, the queries in turns within each.
        for round in 0..=rounds {
            for (i, (name, methods, sql)) in QUERIES.iter().enumerate() {
                conn.set_join_methods(*methods);
                let load = load_average();
                let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
                let start = Instant::now();
                let rows = stmt.run_collect_rows().map_err(|e| e.to_string())?;
                let seconds = start.elapsed().as_secs_f64();
                answers[i] = format!("{rows:?}");
                if round > 0 {
                    best[i] = best[i].min(seconds);
                    println!(
                        "{{\"n\":{n},\"query\":\"{name}\",\"round\":{round},\"seconds\":{seconds:.9},\"load\":\"{load}\"}}"
                    );
                }
            }
        }
        for (i, (name, _, _)) in QUERIES.iter().enumerate() {
            println!(
                "{{\"n\":{n},\"query\":\"{name}\",\"best_seconds\":{:.9},\"cache_pages\":{},\"plan\":{:?},\"answer\":{:?}}}",
                best[i],
                pages * 2,
                plans[i],
                answers[i]
            );
        }
        drop(conn);
        for suffix in ["", "-wal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
    Ok(())
}

fn load(path: &std::path::Path, n: i64) -> Result<Arc<Connection>, String> {
    let io = Arc::new(PlatformIO::new().map_err(|e| e.to_string())?);
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io, &path.display().to_string(), options).map_err(|e| e.to_string())?;
    let conn = db.connect(ConnectOpts::new()).map_err(|e| e.to_string())?;
    for sql in [
        "CREATE TABLE a (id INTEGER PRIMARY KEY, k INTEGER)".to_string(),
        "CREATE TABLE b (id INTEGER PRIMARY KEY, key INTEGER UNIQUE, v INTEGER)".to_string(),
        "BEGIN".to_string(),
        // b.key = (i * 7) mod n + 1 over i in 0..n: a permutation of 1..n, 7 being coprime to every
        // 10^k; a.k the identity, so every outer row finds one inner row, in an order unlike b's.
        format!(
            "WITH RECURSIVE s(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM s WHERE i < {n} - 1) \
             INSERT INTO b SELECT i + 1, (i * 7) % {n} + 1, i % 1000 FROM s"
        ),
        format!(
            "WITH RECURSIVE s(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM s WHERE i < {n}) \
             INSERT INTO a SELECT i, i FROM s"
        ),
        "COMMIT".to_string(),
        "ANALYZE".to_string(),
    ] {
        conn.execute(&sql).map_err(|e| format!("{sql}: {e}"))?;
    }
    Ok(conn)
}

fn single_int(conn: &Arc<Connection>, sql: &str) -> Result<i64, String> {
    let rows = conn.prepare(sql).and_then(|mut s| s.run_collect_rows()).map_err(|e| e.to_string())?;
    match rows.first().and_then(|r| r.first()) {
        Some(Value::Numeric(amphora_engine::Numeric::Integer(i))) => Ok(*i),
        other => Err(format!("{sql}: {other:?}")),
    }
}

fn plan(conn: &Arc<Connection>, sql: &str) -> Result<String, String> {
    let rows = conn
        .prepare(format!("EXPLAIN QUERY PLAN {sql}"))
        .and_then(|mut s| s.run_collect_rows())
        .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|r| r.last().map(|v| format!("{v}")))
        .collect::<Vec<_>>()
        .join(" | "))
}

/// The one-minute load average as the OS reports it.
fn load_average() -> String {
    #[cfg(target_os = "linux")]
    let out = std::fs::read_to_string("/proc/loadavg").ok();
    #[cfg(not(target_os = "linux"))]
    let out = std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    out.unwrap_or_default().trim().to_string()
}
