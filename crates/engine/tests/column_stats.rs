//! What ANALYZE keeps beside `sqlite_stat1`: each column's distinct count and a sample of each
//! table's rows, and the estimates that read them (docs/design/planner.md, decision 20; PL.2). An
//! equality on a column no index leads took a tenth of its table, and an IN list a row a value; a
//! statement prepared before ANALYZE kept its plan after it.

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, OpenOptions, PlatformIO, SqliteDialect,
    StatementStatusCounter,
};
use std::path::Path;
use std::sync::Arc;

fn open(dir: &Path) -> Arc<Connection> {
    let io = Arc::new(PlatformIO::new().unwrap());
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io, dir.join("stats.db").to_str().unwrap(), options).unwrap();
    db.connect(ConnectOpts::new()).unwrap()
}

/// Inserts `rows` into `table`, 500 a statement, in one transaction.
fn insert(conn: &Arc<Connection>, table: &str, rows: &[String]) {
    conn.execute("BEGIN").unwrap();
    for chunk in rows.chunks(500) {
        conn.execute(format!("INSERT INTO {table} VALUES {}", chunk.join(", ")))
            .unwrap();
    }
    conn.execute("COMMIT").unwrap();
}

/// The rows the plan of `sql` estimates it returns, as the oracles' CERT check reads them.
fn estimate(conn: &Arc<Connection>, sql: &str) -> f64 {
    let mut stmt = conn
        .prepare(format!("EXPLAIN QUERY PLAN FORMAT=JSON {sql}"))
        .unwrap();
    let rows = stmt.run_collect_rows().unwrap();
    let text = rows[0][0].to_string();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json["output_rows"]
        .as_f64()
        .unwrap_or_else(|| panic!("{sql}: no output_rows in {text}"))
}

/// The rows `sql` returns, counted.
fn count(conn: &Arc<Connection>, sql: &str) -> f64 {
    let mut stmt = conn
        .prepare(format!("SELECT count(*) FROM ({sql})"))
        .unwrap();
    let rows = stmt.run_collect_rows().unwrap();
    rows[0][0].as_int().unwrap() as f64
}

/// max(est/act, act/est), each at least one row (Moerkotte et al.), as the planner bench reads it.
fn q_error(estimated: f64, counted: f64) -> f64 {
    let (e, c) = (estimated.max(1.0), counted.max(1.0));
    (e / c).max(c / e)
}

/// JOB's keyword table at a tenth of its rows: each keyword distinct, no index on it.
fn keywords(conn: &Arc<Connection>) {
    conn.execute("CREATE TABLE keyword (id INTEGER PRIMARY KEY, keyword TEXT NOT NULL, phonetic_code TEXT)")
        .unwrap();
    let rows: Vec<String> = (1..=13_417)
        .map(|i| format!("({i}, 'keyword-{i}', 'P{}')", i % 900))
        .collect();
    insert(conn, "keyword", &rows);
}

/// Do: ANALYZE JOB's keyword table, then estimate an equality on its keyword, which no index
/// leads, as 15 of JOB's first tables filter it. Expect: within 1.1 of the one row counted, where
/// a tenth of the table (1,342 rows) was estimated (decision 20; AC-PL.2.1).
#[test]
fn an_equality_no_index_leads_is_estimated_from_its_distinct_count() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open(dir.path());
    keywords(&conn);
    conn.execute("ANALYZE").unwrap();
    for sql in [
        "SELECT id FROM keyword WHERE keyword = 'keyword-4711'",
        "SELECT id FROM keyword WHERE keyword = 'character-name-in-title'",
    ] {
        let (e, c) = (estimate(&conn, sql), count(&conn, sql));
        assert!(q_error(e, c) <= 1.1, "{sql}: estimated {e}, counted {c}");
    }
}

/// A company table of 20,000 rows: 8,000 in '[us]', the other 12,000 spread over 200 codes, 60
/// each, as JOB's company_name.country_code holds one frequent value and many rare ones.
fn companies(conn: &Arc<Connection>) {
    conn.execute("CREATE TABLE company (id INTEGER PRIMARY KEY, country_code TEXT)")
        .unwrap();
    let rows: Vec<String> = (0..20_000)
        .map(|i| {
            if i % 5 < 2 {
                format!("({i}, '[us]')")
            } else {
                format!("({i}, '[c{}]')", i % 200)
            }
        })
        .collect();
    insert(conn, "company", &rows);
}

/// Do: ANALYZE the company table, then estimate its frequent code and a rare one. Expect: the
/// frequent one within 1.25 of its 8,000 rows, its share of the sample, where one over the
/// distinct count gives 100; the rare one within 1.25 of its 60, the rows the frequent values
/// leave over the values they leave (decision 20; AC-PL.2.1, 9a's cn.country_code = '[us]').
#[test]
fn a_frequent_value_takes_its_share_of_the_sample() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open(dir.path());
    companies(&conn);
    conn.execute("ANALYZE").unwrap();
    for sql in [
        "SELECT id FROM company WHERE country_code = '[us]'",
        "SELECT id FROM company WHERE country_code = '[c17]'",
    ] {
        let (e, c) = (estimate(&conn, sql), count(&conn, sql));
        assert!(q_error(e, c) <= 1.25, "{sql}: estimated {e}, counted {c}");
    }
}

/// Do: ANALYZE a part table of 20,000 rows over 50 sizes and 7 shipping modes, then estimate
/// TPC-H q16's IN list of eight sizes and q12's of two modes. Expect: each within 1.25 of its
/// count (3,200 and 5,714), where each value of a list was taken for one row (decision 20;
/// AC-PL.2.2).
#[test]
fn an_in_list_is_estimated_as_its_values_shares() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open(dir.path());
    conn.execute("CREATE TABLE part (id INTEGER PRIMARY KEY, size INTEGER, mode TEXT)")
        .unwrap();
    let modes = ["AIR", "FOB", "MAIL", "RAIL", "REG AIR", "SHIP", "TRUCK"];
    let rows: Vec<String> = (0..20_000)
        .map(|i| format!("({i}, {}, '{}')", i % 50 + 1, modes[i % 7]))
        .collect();
    insert(&conn, "part", &rows);
    conn.execute("ANALYZE").unwrap();
    for sql in [
        "SELECT id FROM part WHERE size IN (49, 14, 23, 45, 19, 3, 36, 9)",
        "SELECT id FROM part WHERE mode IN ('MAIL', 'SHIP')",
    ] {
        let (e, c) = (estimate(&conn, sql), count(&conn, sql));
        assert!(q_error(e, c) <= 1.25, "{sql}: estimated {e}, counted {c}");
    }
}

/// Do: ANALYZE a table of 100 distinct values, prepare a query of one of them, then insert 10,000
/// rows of that value and ANALYZE again, and run the statement prepared before it. Expect: the
/// statement prepared again (its reprepare count one) and its estimate the one a statement
/// prepared after ANALYZE gives, where it kept the plan of 100 rows (decision 20; AC-PL.2.3, the
/// audit's 08-fixed-at-prepare).
#[test]
fn a_statement_prepared_before_analyze_plans_again_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open(dir.path());
    conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, c INTEGER)")
        .unwrap();
    let rows: Vec<String> = (0..100).map(|i| format!("({i}, {i})")).collect();
    insert(&conn, "t", &rows);
    conn.execute("ANALYZE").unwrap();
    let sql = "SELECT id FROM t WHERE c = 5";
    let mut stmt = conn
        .prepare(format!("EXPLAIN QUERY PLAN FORMAT=JSON {sql}"))
        .unwrap();
    let rows: Vec<String> = (100..10_100).map(|i| format!("({i}, 5)")).collect();
    insert(&conn, "t", &rows);
    conn.execute("ANALYZE").unwrap();
    let rows = stmt.run_collect_rows().unwrap();
    let text = rows[0][0].to_string();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let kept = json["output_rows"].as_f64().unwrap();
    assert_eq!(stmt.stmt_status(StatementStatusCounter::Reprepare), 1);
    let fresh = estimate(&conn, sql);
    assert_eq!(kept, fresh, "{sql}: prepared before ANALYZE {kept}, after {fresh}");
    let counted = count(&conn, sql);
    assert!(q_error(fresh, counted) <= 1.25, "{sql}: estimated {fresh}, counted {counted}");
}

/// Do: ANALYZE a table of 3,000 rows of two KiB each, six times the sample's 1 MiB bound, whose
/// column g holds 300 distinct values, ten rows each. Expect: no sample kept (its bytes would pass
/// the bound) and an equality on g still within 1.25 of its ten rows, from g's sketch, whose
/// distinct count is within its stated error (T-PL.2.1; decision 20).
#[test]
fn a_table_past_the_samples_bound_keeps_its_distinct_counts() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open(dir.path());
    conn.execute("CREATE TABLE wide (id INTEGER PRIMARY KEY, g INTEGER, pad BLOB)")
        .unwrap();
    let rows: Vec<String> = (0..3_000)
        .map(|i| format!("({i}, {}, zeroblob(2048))", i % 300))
        .collect();
    insert(&conn, "wide", &rows);
    conn.execute("ANALYZE").unwrap();
    let mut stmt = conn
        .prepare("SELECT count(*) FROM __turso_internal_column_stats WHERE tbl = 'wide' AND kind = 'sample' AND data IS NULL")
        .unwrap();
    let rows = stmt.run_collect_rows().unwrap();
    assert_eq!(rows[0][0].as_int(), Some(1), "the sample past its bound is kept");
    let sql = "SELECT id FROM wide WHERE g = 17";
    let (e, c) = (estimate(&conn, sql), count(&conn, sql));
    assert_eq!(c, 10.0);
    assert!(q_error(e, c) <= 1.25, "{sql}: estimated {e}, counted {c}");
}
