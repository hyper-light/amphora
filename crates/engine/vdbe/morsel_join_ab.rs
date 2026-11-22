//! T-QT.6.20 (lane qs6-13): the A/B of a simple join's latency, old path against new. Lane
//! qs6-12 replaced the join workers' direct fold over the joined row's columns with the copy of
//! the serial loop's body, run on every joined row; this harness times simple two-table
//! equi-joins with aggregates (and one returning rows) on whichever commit it is built on, at a
//! ladder of stated cores, so the same file run on 054ace65 (the fold) and on the lane's branch
//! (the body) gives the two sides of one comparison: one driver, one machine, one timing method.
//!
//! It is a harness, not a check: `#[ignore]`d, run by name with `--ignored --nocapture`, and its
//! sizes are the run's to state (`AMPHORA_AB_PROBE_ROWS`, `AMPHORA_AB_BUILD_ROWS`,
//! `AMPHORA_AB_RUNS`, `AMPHORA_AB_CORES`, `AMPHORA_AB_LABEL`); the defaults are a tiny instance
//! for a smoke run, not a measurement. Each line it prints is a TSV record: label, cores, query,
//! runs, p50, p99 and p99.9 in microseconds, and the hash tables the runs built (0 would mean the
//! parallel join never ran, and the line says nothing about it).

use crate::io::MemoryIO;
use crate::vdbe::morsel::{MachineCosts, ParallelMode};
use crate::{ConnectOpts, Connection, Database, DatabaseOpts, OpenFlags, SqliteDialect};
use std::num::NonZeroUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

fn open(name: &str) -> Arc<Connection> {
    let io = Arc::new(MemoryIO::new());
    let db = Database::open_file_with_flags(
        io,
        name,
        OpenFlags::default(),
        DatabaseOpts::new(),
        None,
        Arc::new(SqliteDialect),
    )
    .unwrap();
    db.connect(ConnectOpts::new()).unwrap()
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// `name(k INTEGER, g INTEGER, v INTEGER)` of `rows` rows: `k` cycles over `keys` values, `g`
/// over a few groups, `v` the row's number (INTEGERs only, so the body path never refuses a REAL).
fn fill(conn: &Arc<Connection>, name: &str, rows: usize, keys: usize) {
    conn.execute(format!(
        "CREATE TABLE {name}(k INTEGER, g INTEGER, v INTEGER)"
    ))
    .unwrap();
    conn.execute("BEGIN").unwrap();
    let batch = 500usize;
    let mut at = 0usize;
    while at < rows {
        let end = (at + batch).min(rows);
        let values: Vec<String> = (at..end)
            .map(|i| format!("({}, {}, {i})", i % keys.max(1), i % 13))
            .collect();
        conn.execute(format!(
            "INSERT INTO {name}(k, g, v) VALUES {}",
            values.join(", ")
        ))
        .unwrap();
        at = end;
    }
    conn.execute("COMMIT").unwrap();
}

/// The `q`-quantile of sorted `samples` (nearest rank).
fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted.get(rank - 1).copied().unwrap_or(f64::NAN)
}

#[test]
#[ignore = "a harness: run by name with --ignored --nocapture"]
fn t_qt_6_20_simple_join_latency_old_against_new() {
    let probe_rows = env_usize("AMPHORA_AB_PROBE_ROWS", 4000);
    let build_rows = env_usize("AMPHORA_AB_BUILD_ROWS", 400);
    let runs = env_usize("AMPHORA_AB_RUNS", 20).max(1);
    let label = std::env::var("AMPHORA_AB_LABEL").unwrap_or_else(|_| "unlabelled".into());
    let cores: Vec<usize> = std::env::var("AMPHORA_AB_CORES")
        .unwrap_or_else(|_| "1,2,4,8,32".into())
        .split(',')
        .filter_map(|c| c.trim().parse().ok())
        .filter(|c| *c > 0)
        .collect();
    let conn = open(":memory:qt-6-20");
    fill(&conn, "t", probe_rows, build_rows);
    fill(&conn, "u", build_rows, build_rows);
    conn.set_join_methods(crate::JoinMethods::HashJoinsOnly);
    let queries = [
        ("count", "SELECT count(*) FROM t JOIN u ON t.k = u.k"),
        (
            "group_sum",
            "SELECT u.g, count(*), sum(t.v) FROM t JOIN u ON t.k = u.k GROUP BY u.g",
        ),
        (
            "filter_min_max",
            "SELECT min(t.v), max(u.v), sum(t.v) FROM t JOIN u ON t.k = u.k WHERE t.g < 9",
        ),
        ("rows", "SELECT t.v, u.v FROM t JOIN u ON t.k = u.k"),
    ];
    // The machine's own measured costs of a worker and a dispatch, stated at each rung's cores, so
    // the cost model decides the workers as it would here (the same on both sides of the A/B).
    let measured = MachineCosts::measure().unwrap();
    let worker_start = measured.worker_start().mean().unwrap_or(0.0);
    let dispatch = measured.dispatch().mean().unwrap_or(0.0);
    println!("label\tcores\tquery\truns\tp50_us\tp99_us\tp999_us\tlinks");
    for (name, sql) in queries {
        conn.set_parallel_mode(ParallelMode::Off);
        let serial = conn.prepare(sql).unwrap().run_collect_rows().unwrap();
        conn.set_parallel_mode(ParallelMode::On);
        for &c in &cores {
            *conn.morsel_costs.lock() = Some(MachineCosts::stated(
                NonZeroUsize::new(c).unwrap(),
                worker_start,
                dispatch,
            ));
            let before = conn.morsel_join_links.load(Ordering::Relaxed);
            let mut samples = Vec::with_capacity(runs);
            for run in 0..runs {
                let began = Instant::now();
                let rows = conn.prepare(sql).unwrap().run_collect_rows().unwrap();
                samples.push(began.elapsed().as_secs_f64() * 1e6);
                if run == 0 {
                    // The A/B is of equal answers only: rows compared as sets (no ORDER BY).
                    let (mut a, mut b) = (rows.clone(), serial.clone());
                    a.sort_by(|x, y| format!("{x:?}").cmp(&format!("{y:?}")));
                    b.sort_by(|x, y| format!("{x:?}").cmp(&format!("{y:?}")));
                    assert_eq!(a, b, "{sql} on {c} cores");
                }
            }
            let links = conn
                .morsel_join_links
                .load(Ordering::Relaxed)
                .saturating_sub(before);
            samples.sort_by(f64::total_cmp);
            println!(
                "{label}\t{c}\t{name}\t{runs}\t{:.1}\t{:.1}\t{:.1}\t{links}",
                quantile(&samples, 0.5),
                quantile(&samples, 0.99),
                quantile(&samples, 0.999)
            );
        }
    }
}
