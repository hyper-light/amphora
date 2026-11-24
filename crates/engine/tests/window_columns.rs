//! AC-T1.8.6: the columns a SELECT with window functions hands the query around it, as SQLite 3.54.0 has
//! them: a `*` over a call of a second window, an aggregate or a subquery left out the column (lane C25).

use amphora_engine::morsel::ParallelMode;
use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, Numeric, OpenOptions, SqliteDialect,
    Value,
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
    ("both", true, true),
];

fn open(parallel: bool) -> Arc<Connection> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let options = OpenOptions::new(Arc::new(SqliteDialect))
        .db_opts(DatabaseOpts::new().with_without_rowid(true));
    let name = format!("window_columns_{}.db", N.fetch_add(1, Ordering::Relaxed));
    let db = Database::open(Arc::new(MemoryIO::new()), &name, options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    if parallel {
        conn.set_parallel_mode(ParallelMode::On);
    }
    conn
}

fn cell(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_owned(),
        Value::Numeric(Numeric::Float(real)) => format!("r{:016x}", f64::from(*real).to_bits()),
        Value::Text(text) => format!("'{}'", text.as_str()),
        value => value.to_string(),
    }
}

/// The column names and the rows of `sql`, or why it is refused.
fn ask(conn: &Arc<Connection>, sql: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
    let names = (0..stmt.num_columns())
        .map(|column| stmt.get_column_name(column).to_string())
        .collect();
    let rows = stmt
        .run_collect_rows()
        .map_err(|error| error.to_string())?
        .iter()
        .map(|row| row.iter().map(cell).collect::<Vec<_>>().join(","))
        .collect();
    Ok((names, rows))
}

/// A database of `setup`, under one of the settings.
fn database(setup: &[&str], parallel: bool, statistics: bool) -> Arc<Connection> {
    let conn = open(parallel);
    for sql in setup {
        conn.execute(sql).unwrap();
    }
    if statistics {
        conn.execute("ANALYZE").unwrap();
    }
    conn
}

/// Do: read through `*` SELECTs whose columns are calls of two windows, an aggregate beside a call, and
/// subquery expressions beside a call, none named. Expect: SQLite's columns, every one, by SQLite's names.
#[test]
fn a_star_over_windows_has_every_column() {
    let setup = [
        "CREATE TABLE t(a INTEGER PRIMARY KEY, g INT)",
        "CREATE TABLE s(b INT)",
        "INSERT INTO t VALUES (1, 1), (2, 1), (3, 2)",
        "INSERT INTO s VALUES (3), (5)",
        "CREATE VIEW v(a, n, m) AS SELECT a, avg(g) OVER (ORDER BY a), min(a) OVER (ORDER BY a DESC) FROM t",
        "CREATE TABLE c AS SELECT * FROM (SELECT a, sum(g) OVER (ORDER BY a), min(a) OVER (ORDER BY a DESC) FROM t)",
        "CREATE TABLE d(x, y, z)",
        "INSERT INTO d SELECT * FROM (SELECT a, sum(g) OVER (ORDER BY a), min(a) OVER (ORDER BY a DESC) FROM t)",
    ];
    let asked = [
        (
            "SELECT * FROM (SELECT a, avg(g) OVER (ORDER BY a), min(a) OVER (ORDER BY a DESC) FROM t)",
            "a|avg(g) OVER (ORDER BY a)|min(a) OVER (ORDER BY a DESC)",
            "1,r3ff0000000000000,1 / 2,r3ff0000000000000,2 / 3,r3ff5555555555555,3",
        ),
        (
            "SELECT q.* FROM (SELECT a, sum(a) OVER w, max(a) OVER u, count(*) OVER () AS n FROM t WINDOW w AS (ORDER BY a), u AS (PARTITION BY g)) AS q",
            "a|sum(a) OVER w|max(a) OVER u|n",
            "1,1,2,3 / 2,3,2,3 / 3,6,3,3",
        ),
        (
            "WITH c AS (SELECT g, count(*), sum(count(*)) OVER () FROM t GROUP BY g) SELECT * FROM c",
            "g|count(*)|sum(count(*)) OVER ()",
            "1,2,3 / 2,1,3",
        ),
        (
            "SELECT * FROM (SELECT a, (SELECT max(b) FROM s), EXISTS (SELECT 1 FROM s WHERE b = t.a), a IN (SELECT b FROM s), sum(a) OVER () FROM t)",
            "a|(SELECT max(b) FROM s)|EXISTS (SELECT 1 FROM s WHERE b = t.a)|a IN (SELECT b FROM s)|sum(a) OVER ()",
            "1,5,0,0,6 / 2,5,0,0,6 / 3,5,1,1,6",
        ),
        (
            "SELECT * FROM (SELECT g, sum(a) OVER (PARTITION BY g), count(*) OVER () FROM t UNION SELECT 1, 2, 3)",
            "g|sum(a) OVER (PARTITION BY g)|count(*) OVER ()",
            "1,2,3 / 1,3,3 / 2,3,3",
        ),
        (
            "SELECT \"min(a) OVER (ORDER BY a DESC)\" FROM (SELECT a, sum(g) OVER (ORDER BY a), min(a) OVER (ORDER BY a DESC) FROM t)",
            "min(a) OVER (ORDER BY a DESC)",
            "1 / 2 / 3",
        ),
        ("SELECT a, n, m FROM v", "a|n|m", "1,r3ff0000000000000,1 / 2,r3ff0000000000000,2 / 3,r3ff5555555555555,3"),
        ("SELECT * FROM c", "a|sum(g) OVER (ORDER BY a)|min(a) OVER (ORDER BY a DESC)", "1,1,1 / 2,2,2 / 3,4,3"),
        ("SELECT * FROM d", "x|y|z", "1,1,1 / 2,2,2 / 3,4,3"),
    ];
    for (setting, parallel, statistics) in SETTINGS {
        let conn = database(&setup, parallel, statistics);
        for (sql, want_names, want_rows) in asked {
            let (names, mut rows) =
                ask(&conn, sql).unwrap_or_else(|error| panic!("{setting}: {sql}: {error}"));
            rows.sort();
            assert_eq!(names.join("|"), want_names, "{setting}: {sql}");
            assert_eq!(rows.join(" / "), want_rows, "{setting}: {sql}");
        }
    }
}

struct Case {
    makes: Vec<&'static str>,
    ordered: bool,
    query: &'static str,
    want: &'static str,
}

fn fixture() -> (Vec<&'static str>, Vec<Case>) {
    let mut setup = Vec::new();
    let mut cases = Vec::new();
    let mut makes = Vec::new();
    for line in include_str!("fixtures/window_columns.tsv").lines() {
        match line.split('\t').collect::<Vec<_>>().as_slice() {
            ["sql", sql] => setup.push(*sql),
            ["case"] => makes = Vec::new(),
            ["make", sql] => makes.push(*sql),
            [kind @ ("rows" | "order"), query, want] => cases.push(Case {
                makes: std::mem::take(&mut makes),
                ordered: *kind == "order",
                query,
                want,
            }),
            _ => assert!(line.starts_with('#'), "{line}"),
        }
    }
    (setup, cases)
}

fn answer(conn: &Arc<Connection>, case: &Case) -> String {
    let asked = catch_unwind(AssertUnwindSafe(|| {
        for sql in &case.makes {
            ask(conn, sql)?;
        }
        ask(conn, case.query)
    }));
    match asked {
        Ok(Ok((_, mut rows))) => {
            if !case.ordered {
                rows.sort();
            }
            rows.join(" / ")
        }
        Ok(Err(error)) => format!("refused: {error}"),
        Err(panic) => format!(
            "PANIC {:?}",
            panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
        ),
    }
}

/// Do: ask the fixture's cases of `part` of `PARTS` under each setting. Expect: SQLite's rows.
fn answers_as_sqlite(part: usize) {
    const PARTS: usize = 8;
    let (setup, cases) = fixture();
    let mut wrong = 0;
    let mut ran = 0;
    for (setting, parallel, statistics) in SETTINGS {
        let mut shared: Option<Arc<Connection>> = None;
        for (index, case) in cases.iter().enumerate().filter(|(i, _)| i % PARTS == part) {
            ran += 1;
            let got = if case.makes.is_empty() {
                let conn = shared.get_or_insert_with(|| database(&setup, parallel, statistics));
                answer(conn, case)
            } else {
                answer(&database(&setup, parallel, statistics), case)
            };
            if got != case.want {
                wrong += 1;
                eprintln!(
                    "WRONG case {index}, {setting}:\n  {}\n  {}\n    SQLite  {}\n    Amphora {got}",
                    case.makes.join("\n  "),
                    case.query,
                    case.want
                );
            }
        }
    }
    eprintln!("{ran} runs, {wrong} wrong");
    assert!(ran > 0, "the fixture holds no case of this part");
    assert_eq!(wrong, 0, "{wrong} of {ran} answers are not SQLite's");
}

#[test]
fn windows_columns_are_sqlites_0() {
    answers_as_sqlite(0);
}

#[test]
fn windows_columns_are_sqlites_1() {
    answers_as_sqlite(1);
}

#[test]
fn windows_columns_are_sqlites_2() {
    answers_as_sqlite(2);
}

#[test]
fn windows_columns_are_sqlites_3() {
    answers_as_sqlite(3);
}

#[test]
fn windows_columns_are_sqlites_4() {
    answers_as_sqlite(4);
}

#[test]
fn windows_columns_are_sqlites_5() {
    answers_as_sqlite(5);
}

#[test]
fn windows_columns_are_sqlites_6() {
    answers_as_sqlite(6);
}

#[test]
fn windows_columns_are_sqlites_7() {
    answers_as_sqlite(7);
}
