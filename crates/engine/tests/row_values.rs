//! AC-T1.8.6: comparisons of row values, answered as SQLite 3.54.0 answers them. A row whose first field
//! is NULL is compared by its others too: `(region, code) IN (SELECT region, code FROM closed)` read the
//! fields after the NULL from the row before, or nowhere (the oracles' finding CT, lane C15).

use amphora_engine::morsel::ParallelMode;
use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect, Value,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// The settings a user meets a statement under: the parallel mode, and whether the database has
/// statistics.
const SETTINGS: [(&str, bool, bool); 4] = [
    ("default", false, false),
    ("parallel", true, false),
    ("statistics", false, true),
    ("parallel, statistics", true, true),
];

fn open(fill: &[&str], parallel: bool, statistics: bool) -> Arc<Connection> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let options = OpenOptions::new(Arc::new(SqliteDialect))
        .db_opts(DatabaseOpts::new().with_without_rowid(true));
    let name = format!("row_values_{}.db", N.fetch_add(1, Ordering::Relaxed));
    let db = Database::open(Arc::new(MemoryIO::new()), &name, options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    for sql in fill {
        conn.execute(*sql).unwrap();
    }
    if statistics {
        conn.execute("ANALYZE").unwrap();
    }
    if parallel {
        conn.set_parallel_mode(ParallelMode::On);
    }
    conn
}

/// The rows of `sql`, sorted, each row's values joined by commas.
fn rows(conn: &Arc<Connection>, sql: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
    let mut rows: Vec<String> = stmt
        .run_collect_rows()
        .map_err(|error| error.to_string())?
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| match value {
                    Value::Null => "NULL".to_owned(),
                    value => value.to_string(),
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect();
    rows.sort();
    Ok(rows)
}

fn plan(conn: &Arc<Connection>, sql: &str) -> String {
    let mut stmt = conn.prepare(format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    stmt.run_collect_rows()
        .unwrap()
        .iter()
        .map(|row| row[3].to_string())
        .collect::<Vec<_>>()
        .join(" | ")
}

/// FNV-1a 64 of lines, as `bench/sqlite_row_values.py` sums SQLite's rows.
fn lines_sum(lines: &[String]) -> u64 {
    lines
        .join("\n")
        .bytes()
        .fold(0xCBF2_9CE4_8422_2325_u64, |sum, byte| {
            (sum ^ u64::from(byte)).wrapping_mul(0x100_0000_01B3)
        })
}

/// One database of the fixture: the statements that fill it, and its queries: the kind, the SQL and
/// SQLite's answer.
#[derive(Default)]
struct RowValues {
    fill: Vec<&'static str>,
    queries: Vec<(&'static str, &'static str, &'static str)>,
}

fn row_values(name: &str) -> RowValues {
    let mut out = RowValues::default();
    let mut here = false;
    for line in include_str!("fixtures/row_values.tsv").lines() {
        match line.split('\t').collect::<Vec<_>>().as_slice() {
            ["db", db] => here = *db == name,
            ["sql", sql] if here => out.fill.push(sql),
            ["q", kind, sql, answer] if here => out.queries.push((kind, sql, answer)),
            ["sql" | "q", ..] => {}
            _ => assert!(line.starts_with('#'), "{line}"),
        }
    }
    out
}

/// Do: run every query of the fixture's database `name` under each setting. Expect: SQLite's rows; and
/// that the plans were worth it: each text of `planned` is in some query's plan in some setting.
fn answers_as_sqlite(name: &str, planned: &[&str]) {
    let db = row_values(name);
    assert!(!db.queries.is_empty(), "{name}");
    let mut wrong: Vec<String> = Vec::new();
    let mut counts = vec![0_usize; planned.len()];
    for (setting, parallel, statistics) in SETTINGS {
        let conn = open(&db.fill, parallel, statistics);
        let mut by_kind: BTreeMap<(&str, bool), usize> = BTreeMap::new();
        let before = wrong.len();
        for (kind, sql, want) in &db.queries {
            let got = rows(&conn, sql);
            let answer = match &got {
                Ok(rows) => format!("{}:{:016x}", rows.len(), lines_sum(rows)),
                Err(error) => format!("refused: {error}"),
            };
            if answer == *want {
                let plan = plan(&conn, sql);
                for (count, text) in counts.iter_mut().zip(planned) {
                    *count += usize::from(plan.contains(text));
                }
            } else {
                *by_kind.entry((kind, got.is_ok())).or_default() += 1;
                wrong.push(format!(
                    "{name}, {setting}, {kind}: got {answer}, SQLite {want}: {sql}: first rows {:?}",
                    got.iter().flatten().take(6).collect::<Vec<_>>()
                ));
            }
        }
        eprintln!(
            "{name}, {setting}: {} queries, {} wrong {by_kind:?}, plans so far with each of {planned:?}: {counts:?}",
            db.queries.len(),
            wrong.len() - before
        );
    }
    for line in &wrong {
        eprintln!("WRONG {line}");
    }
    assert!(wrong.is_empty(), "{} wrong", wrong.len());
    assert!(!counts.contains(&0), "{name}: {planned:?}: {counts:?}");
}

#[test]
fn row_values_compare_as_sqlite_without_indexes() {
    answers_as_sqlite("plain", &["SCAN"]);
}

#[test]
fn row_values_compare_as_sqlite_with_indexes_of_rows() {
    answers_as_sqlite("indexed", &["SEARCH", "(("]);
}

#[test]
fn row_values_compare_as_sqlite_with_a_key_of_a_row() {
    answers_as_sqlite("keyed", &["SEARCH"]);
}

/// Do: ask whether rows with a NULL are in a list, as a value and as a condition, under IN and NOT IN,
/// the NULL first, last and in the middle, the rows read one way and the other, a field of another type
/// than the list's, and a row of constants. Expect: SQLite's answers: a field that is not NULL and
/// differs says the row is not the list's, whatever the NULL is, and one that doesn't leaves it unknown.
#[test]
fn a_row_with_a_null_is_compared_by_its_other_fields() {
    let fill = [
        "CREATE TABLE o(id INTEGER PRIMARY KEY, region TEXT, code INT)",
        "CREATE TABLE closed(region TEXT, code INT)",
        "INSERT INTO o VALUES (1, 'eu', 7), (2, NULL, 8), (3, 'us', 9), (4, NULL, 7)",
        "INSERT INTO closed VALUES ('eu', 7)",
        "CREATE TABLE empty(region TEXT, code INT)",
        "CREATE TABLE texts(id INTEGER PRIMARY KEY, region TEXT, code TEXT)",
        "INSERT INTO texts VALUES (1, NULL, '7'), (2, NULL, '8'), (3, 'eu', '7')",
        "CREATE TABLE wide(id INTEGER PRIMARY KEY, a TEXT, b INT, c TEXT COLLATE NOCASE)",
        "INSERT INTO wide VALUES (1, 'p', 1, 'x'), (2, NULL, 2, 'y'), (3, 'p', NULL, 'X'), (4, 'p', NULL, 'z'), (5, 'p', 1, NULL), (6, NULL, NULL, 'x'), (7, NULL, NULL, NULL)",
        "CREATE TABLE list(a TEXT, b INT, c TEXT)",
        "INSERT INTO list VALUES ('p', 1, 'x'), ('q', 1, NULL)",
    ];
    for (setting, parallel, statistics) in SETTINGS {
        let conn = open(&fill, parallel, statistics);
        for (sql, want) in [
            (
                "SELECT id, (region, code) IN (SELECT region, code FROM closed) FROM o",
                ["1,1", "2,0", "3,0", "4,NULL"].as_slice(),
            ),
            (
                "SELECT id, (region, code) NOT IN (SELECT region, code FROM closed) FROM o",
                &["1,0", "2,1", "3,1", "4,NULL"],
            ),
            (
                "SELECT id FROM o WHERE (region, code) NOT IN (SELECT region, code FROM closed)",
                &["2", "3"],
            ),
            (
                "SELECT id FROM o WHERE NOT (region, code) IN (SELECT region, code FROM closed)",
                &["2", "3"],
            ),
            (
                "SELECT id FROM o WHERE ((region, code) IN (SELECT region, code FROM closed)) IS NULL",
                &["4"],
            ),
            (
                "SELECT id FROM o WHERE (region, code) IN (SELECT region, code FROM closed)",
                &["1"],
            ),
            (
                "SELECT id, (region, code) IN (SELECT region, code FROM closed) FROM o ORDER BY id DESC",
                &["1,1", "2,0", "3,0", "4,NULL"],
            ),
            (
                "SELECT id, (region, code) IN (SELECT region, code FROM closed) FROM o WHERE id = 2",
                &["2,0"],
            ),
            (
                "SELECT id, (code, region) IN (SELECT code, region FROM closed) FROM o",
                &["1,1", "2,0", "3,0", "4,NULL"],
            ),
            (
                "SELECT id, (region, code) IN (SELECT region, code FROM empty), (region, code) NOT IN (SELECT region, code FROM empty) FROM o",
                &["1,0,1", "2,0,1", "3,0,1", "4,0,1"],
            ),
            (
                "SELECT id, (region, code) IN (SELECT region, code FROM closed) FROM texts",
                &["1,NULL", "2,0", "3,1"],
            ),
            (
                "SELECT id, (region, code) IN (SELECT region, code FROM texts WHERE texts.id = 3) FROM o",
                &["1,1", "2,0", "3,0", "4,NULL"],
            ),
            (
                "SELECT id, (a, b, c) IN (SELECT a, b, c FROM list) FROM wide",
                &["1,1", "2,0", "3,NULL", "4,0", "5,NULL", "6,NULL", "7,NULL"],
            ),
            (
                "SELECT id FROM wide WHERE (a, b, c) NOT IN (SELECT a, b, c FROM list)",
                &["2", "4"],
            ),
            (
                "SELECT (NULL, 8) IN (SELECT 'eu', 7), (NULL, 7) IN (SELECT 'eu', 7), (NULL, 8) NOT IN (SELECT 'eu', 7), (NULL, 7) NOT IN (SELECT 'eu', 7)",
                &["0,NULL,1,NULL"],
            ),
            (
                "SELECT o.id, o.id IN (SELECT x.id FROM o x WHERE (x.region, x.code) NOT IN (SELECT region, code FROM closed WHERE code <= o.code)) FROM o",
                &["1,0", "2,1", "3,1", "4,0"],
            ),
        ] {
            assert_eq!(rows(&conn, sql).unwrap(), want, "{setting}: {sql}");
        }
        // Each field is computed before any is tested, as SQLite computes it: the second one's error is
        // the query's, as a value and as a condition.
        for sql in [
            "SELECT (NULL, abs(-9223372036854775808)) IN (SELECT 1, 2)",
            "SELECT 1 WHERE (NULL, abs(-9223372036854775808)) IN (SELECT 1, 2)",
        ] {
            assert_eq!(
                rows(&conn, sql).unwrap_err(),
                "integer overflow",
                "{setting}: {sql}"
            );
        }
    }
}
