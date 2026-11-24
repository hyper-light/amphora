//! Oracle finding CH (lane C10): LEFT, RIGHT and FULL JOINs whose WHERE, ON or select list holds
//! subqueries, one inside another, answer as SQLite 3.54.0 does (G2 step 3; AC-T1.1.1: reads as
//! committed; AC-T1.8.6: the differential suite).

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect,
    StepResult, Value, IO,
};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// One database of `fixtures/outer_join_subqueries.tsv`: the statements that fill it, and its queries
/// with SQLite's answers (`bench/sqlite_outer_join_subqueries.py`).
struct Cases {
    name: &'static str,
    fill: Vec<&'static str>,
    queries: Vec<(&'static str, &'static str)>,
}

fn cases() -> Vec<Cases> {
    let mut out: Vec<Cases> = Vec::new();
    for line in include_str!("fixtures/outer_join_subqueries.tsv").lines() {
        let fields: Vec<&'static str> = line.split('\t').collect();
        match fields.as_slice() {
            ["db", name] => out.push(Cases {
                name,
                fill: Vec::new(),
                queries: Vec::new(),
            }),
            ["sql", sql] => out.last_mut().unwrap().fill.push(sql),
            ["q", sql, want] => out.last_mut().unwrap().queries.push((sql, want)),
            _ => assert!(line.starts_with('#'), "{line}"),
        }
    }
    out
}

/// A database of its own: one opened by the same name is shared across a process's tests.
fn open(fill: &[&str]) -> (Arc<MemoryIO>, Arc<Connection>) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let io = Arc::new(MemoryIO::new());
    let options = OpenOptions::new(Arc::new(SqliteDialect))
        .db_opts(DatabaseOpts::new().with_without_rowid(true));
    let name = format!(
        "outer_join_subqueries_{}.db",
        N.fetch_add(1, Ordering::Relaxed)
    );
    let db = Database::open(io.clone(), &name, options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    for statement in fill {
        conn.execute(*statement).unwrap();
    }
    (io, conn)
}

/// The settings a user meets a statement under: the parallel mode, and statistics.
const SETTINGS: [(&str, bool, bool); 4] = [
    ("default", false, false),
    ("parallel", true, false),
    ("statistics", false, true),
    ("parallel, statistics", true, true),
];

fn set(conn: &Arc<Connection>, parallel: bool, statistics: bool) {
    if parallel {
        conn.set_parallel_mode(amphora_engine::morsel::ParallelMode::On);
    }
    if statistics {
        conn.execute("ANALYZE").unwrap();
    }
}

/// The rows of `sql` as the fixture writes SQLite's, or its error.
fn answer(io: &MemoryIO, conn: &Arc<Connection>, sql: &str) -> Result<String, String> {
    let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
    let mut lines = Vec::new();
    loop {
        match stmt.step().map_err(|error| error.to_string())? {
            StepResult::Row => {
                let row = stmt.row().unwrap();
                let values: Vec<String> = row
                    .get_values()
                    .map(|value| match value {
                        Value::Null => "NULL".to_owned(),
                        value => value.to_string(),
                    })
                    .collect();
                lines.push(values.join(","));
            }
            StepResult::Done => break,
            StepResult::IO => io.step().map_err(|error| error.to_string())?,
            other => return Err(format!("step: {other:?}")),
        }
    }
    lines.sort();
    Ok(format!("{}:{}", lines.len(), lines.join(";")))
}

/// Do: run every query of the fixture under each setting. Expect: SQLite's rows, and no panic.
#[test]
fn outer_joins_with_nested_subqueries_answer_as_sqlite() {
    let mut wrong: Vec<String> = Vec::new();
    let mut ran = 0;
    for db in cases() {
        for (setting, parallel, statistics) in SETTINGS {
            let (mut io, mut conn) = open(&db.fill);
            set(&conn, parallel, statistics);
            for (sql, want) in &db.queries {
                ran += 1;
                match catch_unwind(AssertUnwindSafe(|| answer(&io, &conn, sql))) {
                    Ok(Ok(got)) if got == *want => {}
                    Ok(got) => wrong.push(format!(
                        "{} {setting}: {sql}\n    SQLite {want}\n    Amphora {got:?}",
                        db.name
                    )),
                    Err(panic) => {
                        let message = panic
                            .downcast_ref::<String>()
                            .cloned()
                            .or_else(|| panic.downcast_ref::<&str>().map(|text| text.to_string()))
                            .unwrap_or_default();
                        wrong.push(format!(
                            "{} {setting}: {sql}\n    SQLite {want}\n    PANIC {message}",
                            db.name
                        ));
                        // What the panic left behind is not the next query's to answer for.
                        (io, conn) = open(&db.fill);
                        set(&conn, parallel, statistics);
                    }
                }
            }
        }
    }
    assert!(ran > 0);
    assert!(
        wrong.is_empty(),
        "{} of {ran} answers differ from SQLite's:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}
