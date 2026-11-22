//! Query-speed step 5's lookahead filters (QT.5, T-QT.5.3): a star join's dimensions, each read once
//! into a Bloom filter its fact table's scan checks (`translate/optimizer/lip.rs`). Every query runs
//! with the filters off (the control), on wherever they are safe, and by cost; the answers must be
//! the control's, over a star, a cyclic join graph and outer joins, where a filter may pass from an
//! inner join's table only; and the statement's counters must show the filters ran.

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, JoinMethods, OpenOptions, PlatformIO,
    SqliteDialect, StarFilterOrder, StarFilters,
};
use std::sync::Arc;

const FACTS: i64 = 2000;
const DIMS: i64 = 50;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempfile::tempdir().unwrap();
    let io = Arc::new(PlatformIO::new().unwrap());
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io, dir.path().join("star.db").to_str().unwrap(), options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    // Index nested loops read the fact table first and look each dimension up by its key: the
    // plan whose fact scan the filters cut, whatever the learned weights. Each test then runs
    // the query by cost as well.
    conn.set_join_methods(JoinMethods::NestedLoopsOnly);
    conn.execute("CREATE TABLE f (id INTEGER PRIMARY KEY, a, b, c, v)")
        .unwrap();
    for t in ["da", "db", "dc"] {
        conn.execute(format!("CREATE TABLE {t} (k INTEGER PRIMARY KEY, x, j)"))
            .unwrap();
    }
    conn.execute("BEGIN").unwrap();
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut draw = |m: i64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % m as u64) as i64
    };
    for t in ["da", "db", "dc"] {
        for k in 1..=DIMS {
            let (x, j) = (draw(10), draw(DIMS) + 1);
            conn.execute(format!("INSERT INTO {t} VALUES ({k}, {x}, {j})"))
                .unwrap();
        }
    }
    for id in 1..=FACTS {
        // A key past the dimensions' or NULL now and then: neither joins anything.
        let key = |d: i64| {
            if d == 0 {
                "NULL".to_string()
            } else {
                d.to_string()
            }
        };
        let (a, b, c) = (
            key(draw(DIMS + 5)),
            key(draw(DIMS + 5)),
            key(draw(DIMS + 5)),
        );
        conn.execute(format!(
            "INSERT INTO f VALUES ({id}, {a}, {b}, {c}, {})",
            draw(1000)
        ))
        .unwrap();
    }
    conn.execute("COMMIT").unwrap();
    (dir, conn)
}

/// The sorted answer and the statement's star-filter probes and rejects.
fn run(conn: &Arc<Connection>, rule: StarFilters, sql: &str) -> (Vec<String>, u64, u64) {
    conn.set_star_filters(rule);
    let mut stmt = conn.prepare(sql).unwrap();
    let mut out: Vec<String> = stmt
        .run_collect_rows()
        .unwrap()
        .into_iter()
        .map(|row| format!("{row:?}"))
        .collect();
    out.sort();
    let metrics = stmt.metrics();
    (out, metrics.star_filter_probes, metrics.star_filter_rejects)
}

/// The fact table first, so each dimension is joined after it.
const STAR: &str =
    "SELECT f.id, f.v, da.x, db.x, dc.x FROM f CROSS JOIN da CROSS JOIN db CROSS JOIN dc \
    WHERE f.a = da.k AND f.b = db.k AND f.c = dc.k AND da.x < 3 AND db.x < 5 AND dc.x > 6";

/// Do: the star join, filters off, on and by cost. Expect: one answer; on, the filters checked
/// every fact row and rejected some; by cost, the filters ran too, for they rule out most rows.
#[test]
fn t_qt_5_3_star_filters_keep_the_answer_and_run() {
    let (_dir, conn) = open();
    let (control, probes, _) = run(&conn, StarFilters::Off, STAR);
    assert_eq!(probes, 0, "the control checks no filter");
    assert!(!control.is_empty());
    let (always, probes, rejects) = run(&conn, StarFilters::Always, STAR);
    assert_eq!(always, control);
    assert!(
        probes >= FACTS as u64,
        "every fact row checks a filter: {probes}"
    );
    assert!(rejects > 0, "the filters rule fact rows out");
    conn.set_join_methods(JoinMethods::ByCost);
    let (by_cost, _, _) = run(&conn, StarFilters::ByCost, STAR);
    assert_eq!(by_cost, control);
}

/// Do: a cyclic join graph, the dimensions also joined to each other, filters off and on.
/// Expect: one answer.
#[test]
fn t_qt_5_3_star_filters_on_a_cyclic_join_graph() {
    let (_dir, conn) = open();
    let sql = "SELECT f.id, da.k, db.k, dc.k FROM f CROSS JOIN da CROSS JOIN db CROSS JOIN dc \
        WHERE f.a = da.k AND f.b = db.k AND f.c = dc.k AND da.j = db.k AND db.j >= dc.k \
        AND da.x < 6 AND db.x <> 4 AND dc.x >= 2";
    let (control, _, _) = run(&conn, StarFilters::Off, sql);
    let (always, probes, _) = run(&conn, StarFilters::Always, sql);
    assert_eq!(always, control);
    assert!(
        probes > 0,
        "the cyclic graph's dimensions filter the fact scan"
    );
    conn.set_join_methods(JoinMethods::ByCost);
    let (control, _, _) = run(&conn, StarFilters::Off, sql);
    assert_eq!(run(&conn, StarFilters::ByCost, sql).0, control);
}

/// Do: outer joins, filters off and on: a LEFT JOIN's right table must not filter the fact table
/// (its unmatched fact rows stay, null-extended), while an inner dimension in the same query may.
/// Expect: one answer; the inner dimension's filter ran and the outer one's rejected nothing it
/// should keep.
#[test]
fn t_qt_5_3_star_filters_pass_one_way_over_an_outer_join() {
    let (_dir, conn) = open();
    let sql =
        "SELECT f.id, da.x, db.x FROM f CROSS JOIN da LEFT JOIN db ON f.b = db.k AND db.x < 2 \
        WHERE f.a = da.k AND da.x < 3";
    let (control, _, _) = run(&conn, StarFilters::Off, sql);
    let (always, probes, _) = run(&conn, StarFilters::Always, sql);
    assert_eq!(always, control);
    assert!(probes > 0, "the inner dimension filters the fact scan");
    // Only the outer side: no filter may run, and the answer keeps every fact row.
    let only_outer =
        "SELECT f.id, db.x FROM f LEFT JOIN db ON f.b = db.k WHERE db.x IS NULL OR db.x < 2";
    let (control, _, _) = run(&conn, StarFilters::Off, only_outer);
    let (always, probes, _) = run(&conn, StarFilters::Always, only_outer);
    assert_eq!(always, control);
    assert_eq!(probes, 0, "a LEFT JOIN's right table filters nothing");
    // The fact table on the outer side: its rows may not be filtered by the preserved side's
    // dimension in either direction once it is null-supplying.
    let fact_outer = "SELECT da.k, f.id FROM da LEFT JOIN f ON f.a = da.k WHERE da.x < 3";
    let (control, _, _) = run(&conn, StarFilters::Off, fact_outer);
    let (always, probes, _) = run(&conn, StarFilters::Always, fact_outer);
    assert_eq!(always, control);
    assert_eq!(probes, 0, "a null-supplying fact table is not filtered");
}

/// T-QT.5.11 (lane qs5-18). Do: a star join whose three filters the planner estimates alike (each
/// dimension's term a range), so it checks them in join order, the first two passing nearly every
/// fact row and the last about one in ten; run it with the filters checked in the planner's order
/// (the control) and in the run's measured order. Expect: one answer, and fewer filter checks in
/// the measured order, which moves the last filter first.
#[test]
fn t_qt_5_11_star_filters_checked_in_their_measured_order_check_less() {
    let (_dir, conn) = open();
    let sql = "SELECT f.id, f.v, da.x, db.x, dc.x FROM f CROSS JOIN da CROSS JOIN db CROSS JOIN dc \
        WHERE f.a = da.k AND f.b = db.k AND f.c = dc.k AND da.x < 1000 AND db.x < 1000 AND dc.x > 8";
    let (control, _, _) = run(&conn, StarFilters::Off, sql);
    conn.set_star_filter_order(StarFilterOrder::Planned);
    let (planned, planned_probes, _) = run(&conn, StarFilters::Always, sql);
    conn.set_star_filter_order(StarFilterOrder::Measured);
    let (measured, measured_probes, _) = run(&conn, StarFilters::Always, sql);
    assert_eq!(planned, control);
    assert_eq!(measured, control);
    assert!(
        measured_probes < planned_probes,
        "measured order {measured_probes} checks, the planner's {planned_probes}"
    );
}

/// T-QT.5.3 (lane qs5-18): the filters' memory within the statement's stated bound. Do: the
/// smallest cache a connection takes (200 pages, the statement's memory), and a star join whose
/// three dimensions each give tens of thousands of keys, more than their shares of that memory
/// hold, under both orders. Expect: the control's answer, and the most bytes the filters held
/// together, as their builds ended and once sized, within the statement's memory.
#[test]
fn t_qt_5_3_star_filters_stay_within_the_statements_memory() {
    let dir = tempfile::tempdir().unwrap();
    let io = Arc::new(PlatformIO::new().unwrap());
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io, dir.path().join("big.db").to_str().unwrap(), options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    conn.set_join_methods(JoinMethods::NestedLoopsOnly);
    let keys = 60_000;
    for sql in [
        "PRAGMA cache_size = 200".to_owned(),
        "CREATE TABLE f (id INTEGER PRIMARY KEY, a, b, c, v)".to_owned(),
        "CREATE TABLE da (k INTEGER PRIMARY KEY, x)".to_owned(),
        "CREATE TABLE db (k INTEGER PRIMARY KEY, x)".to_owned(),
        "CREATE TABLE dc (k INTEGER PRIMARY KEY, x)".to_owned(),
        format!("WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < {keys}) INSERT INTO da SELECT n, n % 10 FROM s"),
        format!("WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < {keys}) INSERT INTO db SELECT n, n % 7 FROM s"),
        format!("WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < {keys}) INSERT INTO dc SELECT n, n % 3 FROM s"),
        format!("WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < 5000) INSERT INTO f SELECT n, (n * 7919) % {keys} + 1, (n * 104729) % {keys} + 1, (n * 1299709) % {keys} + 1, n % 1000 FROM s"),
    ] {
        conn.execute(sql).unwrap();
    }
    let page_size = conn
        .prepare("PRAGMA page_size")
        .unwrap()
        .run_collect_rows()
        .unwrap()[0][0]
        .to_string()
        .parse::<u64>()
        .unwrap();
    let budget = 200 * page_size;
    let sql = "SELECT count(*), sum(f.v) FROM f CROSS JOIN da CROSS JOIN db CROSS JOIN dc \
        WHERE f.a = da.k AND f.b = db.k AND f.c = dc.k AND da.x < 9 AND db.x < 6 AND dc.x < 2";
    conn.set_star_filters(StarFilters::Off);
    let control = conn.prepare(sql).unwrap().run_collect_rows().unwrap();
    conn.set_star_filters(StarFilters::Always);
    for order in [StarFilterOrder::Planned, StarFilterOrder::Measured] {
        conn.set_star_filter_order(order);
        let mut stmt = conn.prepare(sql).unwrap();
        assert_eq!(stmt.run_collect_rows().unwrap(), control, "{order:?}");
        let metrics = stmt.metrics();
        assert!(metrics.star_filter_probes > 0, "{order:?}: the filters ran");
        assert!(
            metrics.star_filter_bytes <= budget,
            "{order:?}: the filters held {} bytes, the statement's memory {budget}",
            metrics.star_filter_bytes
        );
    }
}

/// A star of `facts` fact rows and three dimensions of 50 keys, every fact row joining each: `da`
/// and `db` keep 9 keys in 10, `dc` 1 in 10 (`dc.x = 0`). With `drift`, the fact rows' `c` keys
/// change halfway: the first half name only kept `dc` keys, the second half none, so an order
/// learned on the first half is wrong for the second (the measured order's failure mode); without
/// it the planner's estimates tie and it checks `dc` last (the planned order's).
fn harness_star(dir: &std::path::Path, facts: i64, drift: bool) -> Arc<Connection> {
    let io = Arc::new(PlatformIO::new().unwrap());
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io, dir.join("h.db").to_str().unwrap(), options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    conn.set_join_methods(JoinMethods::NestedLoopsOnly);
    let c = if drift {
        format!("CASE WHEN n <= {facts} / 2 THEN (n % 5) * 10 + 10 ELSE (n % 45) + 1 + (n % 45) / 9 END")
    } else {
        "n % 50 + 1".to_owned()
    };
    for sql in [
        "CREATE TABLE f (id INTEGER PRIMARY KEY, a, b, c, v)".to_owned(),
        "CREATE TABLE da (k INTEGER PRIMARY KEY, x)".to_owned(),
        "CREATE TABLE db (k INTEGER PRIMARY KEY, x)".to_owned(),
        "CREATE TABLE dc (k INTEGER PRIMARY KEY, x)".to_owned(),
        "WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < 50) INSERT INTO da SELECT n, n % 10 FROM s".to_owned(),
        "WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < 50) INSERT INTO db SELECT n, n % 10 FROM s".to_owned(),
        "WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < 50) INSERT INTO dc SELECT n, n % 10 FROM s".to_owned(),
        format!("WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s WHERE n < {facts}) INSERT INTO f SELECT n, n % 50 + 1, (n * 7) % 50 + 1, {c}, n % 1000 FROM s"),
    ] {
        conn.execute(sql).unwrap();
    }
    conn
}

/// The coordinator's harness (T-QT.5.11): the planned and measured orders over a star whose
/// estimates tie and one whose rows drift, at `AMPHORA_QT511_FACTS` fact rows (default 2,000 and
/// 1,000,000), `AMPHORA_QT511_RUNS` runs each (default 10): a line each with the filter checks a
/// fact row, the rejects, and run latency p50, p99 and p99.9. Tiny instance:
/// `AMPHORA_QT511_FACTS=2000 AMPHORA_QT511_RUNS=3`.
#[test]
#[ignore = "the coordinator's harness: run with --ignored --nocapture"]
fn qt_5_11_star_order_harness() {
    let list = |name: &str, default: &[i64]| -> Vec<i64> {
        std::env::var(name).map_or_else(
            |_| default.to_vec(),
            |t| t.split(',').map(|n| n.trim().parse().unwrap()).collect(),
        )
    };
    let runs = list("AMPHORA_QT511_RUNS", &[10])[0];
    let sql = "SELECT count(*), sum(f.v) FROM f CROSS JOIN da CROSS JOIN db CROSS JOIN dc \
        WHERE f.a = da.k AND f.b = db.k AND f.c = dc.k AND da.x < 9 AND db.x < 9 AND dc.x = 0";
    for facts in list("AMPHORA_QT511_FACTS", &[2_000, 1_000_000]) {
        for drift in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let conn = harness_star(dir.path(), facts, drift);
            conn.set_star_filters(StarFilters::Off);
            let control = conn.prepare(sql).unwrap().run_collect_rows().unwrap();
            conn.set_star_filters(StarFilters::Always);
            for order in [StarFilterOrder::Planned, StarFilterOrder::Measured] {
                conn.set_star_filter_order(order);
                let mut stmt = conn.prepare(sql).unwrap();
                let mut nanos = Vec::new();
                for _ in 0..runs {
                    let began = std::time::Instant::now();
                    assert_eq!(stmt.run_collect_rows().unwrap(), control, "{order:?}");
                    nanos.push(u64::try_from(began.elapsed().as_nanos()).unwrap_or(u64::MAX));
                    stmt.reset().unwrap();
                }
                nanos.sort_unstable();
                let at = |p: f64| nanos[((nanos.len() - 1) as f64 * p).round() as usize];
                let m = stmt.metrics();
                let rows = (facts * runs) as f64;
                println!(
                    "qt5-11 facts={facts} drift={drift} order={order:?} runs={runs} checks_per_row={:.3} rejects_per_row={:.3} filter_bytes={} run_ns_p50={} p99={} p999={}",
                    m.star_filter_probes as f64 / rows,
                    m.star_filter_rejects as f64 / rows,
                    m.star_filter_bytes,
                    at(0.5),
                    at(0.99),
                    at(0.999),
                );
            }
        }
    }
}
