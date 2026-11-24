//! Oracle findings DH and DI (lane C18): a CTE's name is the innermost WITH clause's that defines it where the name
//! stands, a CTE's body reads the names around its own definition, and a CTE nothing reads is no
//! part of the statement: the answers and the errors of SQLite 3.54.0, over WITH clauses inside one another in every place
//! of a statement (G2 step 3; AC-T1.1.1: reads as committed; AC-T1.8.6: the differential suite).

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

fn open(fill: &[&str], parallel: bool, statistics: bool) -> Arc<Connection> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let name = format!("cte_scopes_{}.db", N.fetch_add(1, Ordering::Relaxed));
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

/// The rows of `sql`, sorted, as the fixture writes SQLite's, or its error.
fn rows(conn: &Arc<Connection>, sql: &str) -> Result<String, String> {
    let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
    let mut lines: Vec<String> = stmt
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
    lines.sort();
    Ok(format!("{}:{}", lines.len(), lines.join(";")))
}

/// What `sql` answers: its rows, or for a write those its table then holds, which are put back.
fn answer(conn: &Arc<Connection>, sql: &str, write: bool) -> Result<String, String> {
    if !write {
        return rows(conn, sql);
    }
    conn.execute("BEGIN").unwrap();
    let held = rows(conn, sql).and_then(|_| rows(conn, "SELECT k, v FROM log"));
    conn.execute("ROLLBACK").unwrap();
    held
}

/// [answer] as the fixture writes SQLite's, or what the panic said.
fn outcome(conn: &Arc<Connection>, sql: &str, write: bool) -> String {
    match catch_unwind(AssertUnwindSafe(|| answer(conn, sql, write))) {
        Ok(Ok(rows)) => rows,
        Ok(Err(error)) => {
            let message = error.strip_prefix("Parse error: ").unwrap_or(&error);
            format!("error: {message}")
        }
        Err(panic) => {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|text| text.to_string()))
                .unwrap_or_default();
            format!("PANIC {message}")
        }
    }
}

/// An answer but the name a cycle's error gives, which is the CTE of the cycle that is met first.
fn kind_of_cycle(answer: &str) -> &str {
    if answer.starts_with("error: circular reference: ") {
        "error: circular reference"
    } else {
        answer
    }
}

/// One database of the fixture: the statements that fill it, and its statements, whether each is a
/// write, and what SQLite answers (`bench/sqlite_cte_scopes.py`).
struct Cases {
    name: &'static str,
    fill: Vec<&'static str>,
    statements: Vec<(bool, &'static str, &'static str)>,
}

fn cases() -> Vec<Cases> {
    let mut out: Vec<Cases> = Vec::new();
    for line in include_str!("fixtures/cte_scopes.tsv").lines() {
        let fields: Vec<&'static str> = line.split('\t').collect();
        match fields.as_slice() {
            ["db", name] => out.push(Cases {
                name,
                fill: Vec::new(),
                statements: Vec::new(),
            }),
            ["sql", sql] => out.last_mut().unwrap().fill.push(sql),
            [kind @ ("q" | "w"), sql, want] => {
                let write = *kind == "w";
                out.last_mut().unwrap().statements.push((write, sql, want));
            }
            _ => assert!(line.starts_with('#'), "{line}"),
        }
    }
    out
}

/// Do: run every statement of the fixture under each setting. Expect: SQLite's rows, or its error, and no panic.
#[test]
fn ctes_of_with_clauses_inside_one_another_answer_as_sqlite() {
    let mut wrong: Vec<String> = Vec::new();
    let (mut ran, mut refused) = (0, 0);
    for db in cases() {
        for (setting, parallel, statistics) in SETTINGS {
            let mut conn = open(&db.fill, parallel, statistics);
            for (write, sql, want) in &db.statements {
                ran += 1;
                refused += usize::from(want.starts_with("error:"));
                let got = outcome(&conn, sql, *write);
                if kind_of_cycle(&got) != kind_of_cycle(want) {
                    wrong.push(format!(
                        "{} {setting}: {sql}\n    SQLite {want}\n    Amphora {got}",
                        db.name
                    ));
                    // What a panic left behind is not the next statement's to answer for.
                    conn = open(&db.fill, parallel, statistics);
                }
            }
        }
    }
    assert!(ran > 0 && refused > 0 && refused < ran);
    assert!(
        wrong.is_empty(),
        "{} of {ran} answers differ from SQLite's:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}
