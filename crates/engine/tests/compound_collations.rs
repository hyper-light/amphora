//! The oracles' finding CY (lanes C11 and C27): a compound tells its rows apart by its arms' collations as lang_select.html 3
//! says, "as if the columns of the left and right-hand SELECT statements were the left and right-hand operands of the equals
//! (=) operator, except that greater precedence is not assigned to a collation sequence specified with the postfix COLLATE
//! operator": the first arm's that has one, a column that declares none having BINARY. The answers of SQLite 3.54.0, over
//! arms of every kind (G2 step 3; AC-T1.1.1: reads as committed; AC-T1.8.6: the differential suite).

use amphora_engine::morsel::ParallelMode;
use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect, Value,
};
use std::panic::{catch_unwind, AssertUnwindSafe};
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

/// The tables bench/sqlite_compound_collations.py asked SQLite over.
const FILL: [&str; 16] = [
    "CREATE TABLE u(a TEXT)",
    "CREATE TABLE t(a TEXT COLLATE NOCASE)",
    "CREATE TABLE r(a TEXT COLLATE RTRIM)",
    "CREATE TABLE b(a TEXT COLLATE BINARY)",
    "CREATE TABLE n(a)",
    "CREATE TABLE e(a TEXT COLLATE NOCASE)",
    "CREATE TABLE k(a TEXT COLLATE NOCASE PRIMARY KEY) WITHOUT ROWID",
    "CREATE TABLE o(a TEXT COLLATE NOCASE)",
    "INSERT INTO u VALUES ('ABC'), ('abc'), ('x'), ('q '), (NULL)",
    "INSERT INTO t VALUES ('Q'), ('q'), ('ABC'), ('m ')",
    "INSERT INTO r VALUES ('m'), ('m '), ('abc'), ('ABC ')",
    "INSERT INTO b VALUES ('aBC'), ('Q'), ('M')",
    "INSERT INTO n VALUES ('X'), ('abc'), (7)",
    "INSERT INTO k VALUES ('Abc'), ('Q'), ('z')",
    "CREATE VIEW v AS SELECT a FROM t",
    "CREATE VIEW f AS SELECT lower(a) AS a FROM r",
];

fn open(parallel: bool, statistics: bool) -> Arc<Connection> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let options = OpenOptions::new(Arc::new(SqliteDialect))
        .db_opts(DatabaseOpts::new().with_without_rowid(true));
    let name = format!(
        "compound_collations_{}.db",
        N.fetch_add(1, Ordering::Relaxed)
    );
    let db = Database::open(Arc::new(MemoryIO::new()), &name, options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    for sql in FILL {
        conn.execute(sql).unwrap();
    }
    if statistics {
        conn.execute("ANALYZE").unwrap();
    }
    if parallel {
        conn.set_parallel_mode(ParallelMode::On);
    }
    conn
}

/// The rows of `sql`, each value folded as the fixture folds SQLite's (which of several equal texts a
/// compound keeps no page says), sorted unless `ordered`.
fn rows(conn: &Arc<Connection>, sql: &str, ordered: bool) -> Result<String, String> {
    let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
    let mut lines: Vec<String> = stmt
        .run_collect_rows()
        .map_err(|error| error.to_string())?
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| match value {
                    Value::Null => "NULL".to_owned(),
                    Value::Text(text) => text.as_str().trim_end_matches(' ').to_ascii_lowercase(),
                    value => value.to_string(),
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect();
    if !ordered {
        lines.sort();
    }
    Ok(format!("{}:{}", lines.len(), lines.join(";")))
}

/// What `sql` answers: its rows, or for an INSERT the ones its table then holds.
fn outcome(conn: &Arc<Connection>, sql: &str, how: &str) -> String {
    let answered = catch_unwind(AssertUnwindSafe(|| match how {
        "dml" => {
            conn.execute("DELETE FROM o").map_err(|e| e.to_string())?;
            conn.execute(sql).map_err(|e| e.to_string())?;
            rows(conn, "SELECT a FROM o", false)
        }
        _ => rows(conn, sql, how == "ordered"),
    }));
    match answered {
        Ok(Ok(rows)) => rows,
        Ok(Err(error)) => format!("error: {error}"),
        Err(_) => "a panic".to_owned(),
    }
}

/// Do: every query of `fixture` under the four settings. Expect: SQLite's rows.
fn check(fixture: &str) {
    let mut wrong = Vec::new();
    let mut runs = 0;
    for (setting, parallel, statistics) in SETTINGS {
        let conn = open(parallel, statistics);
        for line in fixture.lines() {
            let mut fields = line.splitn(4, '\t');
            let (kind, how, sql, want) = (
                fields.next().unwrap(),
                fields.next().unwrap(),
                fields.next().unwrap(),
                fields.next().unwrap(),
            );
            runs += 1;
            let got = outcome(&conn, sql, how);
            if got != want {
                wrong.push(format!(
                    "[{setting}] {kind}: {sql}\n  sqlite3 {want}\n  Amphora {got}"
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {runs} answers differ from sqlite3 3.54.0's:\n{}",
        wrong.len(),
        wrong[..wrong.len().min(40)].join("\n")
    );
}

/// Do: two and three arms, of columns, expressions, literals, COLLATEs, aggregates, subqueries, views and
/// windows, under UNION, EXCEPT, INTERSECT and UNION ALL, read whole, in order, in a FROM clause, a CTE and
/// an IN, and inserted. Expect: SQLite's rows.
#[test]
fn a_compounds_rows_are_told_apart_by_its_first_arm_with_a_collation() {
    check(include_str!("fixtures/compound_collations.tsv"));
}

/// Do: an EXCEPT and an INTERSECT of two arms with no collation, before an arm that has one, and a VALUES of two
/// rows before a NOCASE column. Expect: what lang_select.html says, where SQLite answers otherwise: the
/// operators group from the left, so the first two compare by BINARY (SQLite loses 'Q' from the first and finds
/// it in the third, by the last arm's), and a VALUES means its rows' SELECTs under UNION ALL, whose literals have
/// no collation (SQLite reads them as columns and keeps 'q' beside 'Q').
#[test]
fn a_compounds_operators_group_from_the_left() {
    let conn = open(false, false);
    for (sql, want) in [
        ("SELECT 'Q' EXCEPT SELECT 'q' UNION SELECT a FROM e", "1:q"),
        (
            "SELECT 'Q' EXCEPT SELECT 'q' UNION ALL SELECT a FROM e",
            "1:q",
        ),
        (
            "SELECT 'Q' INTERSECT SELECT 'q' UNION SELECT a FROM e",
            "0:",
        ),
        (
            "SELECT 'Q' INTERSECT SELECT 'q' UNION ALL SELECT a FROM e",
            "0:",
        ),
        (
            "SELECT 'Q' UNION SELECT 'q' EXCEPT SELECT NULL UNION ALL SELECT a FROM e",
            "2:q;q",
        ),
        ("VALUES ('q'), ('ABC') UNION SELECT a FROM t", "3:abc;m;q"),
    ] {
        assert_eq!(rows(&conn, sql, false).unwrap(), want, "{sql}");
    }
}

/// Do: two columns under an IN of another collation, in a table read by its own. Expect: the compound
/// compares by its own columns' collation, not the IN's: 'abc' is left of u EXCEPT t, whose rows differ in
/// case alone, and 'ABC' finds it by NOCASE.
#[test]
fn a_compound_in_an_in_keeps_its_own_collation() {
    let conn = open(false, false);
    assert_eq!(
        rows(
            &conn,
            "SELECT 'ABC' COLLATE NOCASE IN (SELECT a FROM u WHERE a LIKE 'abc' EXCEPT SELECT a FROM t)",
            false
        )
        .unwrap(),
        "1:1"
    );
}
