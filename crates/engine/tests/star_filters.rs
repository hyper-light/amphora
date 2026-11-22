//! Query-speed step 5's lookahead filters (QT.5, T-QT.5.3): a star join's dimensions, each read once
//! into a Bloom filter its fact table's scan checks (`translate/optimizer/lip.rs`). Every query runs
//! with the filters off (the control), on wherever they are safe, and by cost; the answers must be
//! the control's, over a star, a cyclic join graph and outer joins, where a filter may pass from an
//! inner join's table only; and the statement's counters must show the filters ran.

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, JoinMethods, OpenOptions, PlatformIO,
    SqliteDialect, StarFilters,
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
