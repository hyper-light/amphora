//! Oracle findings CH (lane C10), CO and CM (lane C13): LEFT, RIGHT and FULL JOINs whose WHERE, ON or
//! select list holds subqueries, one inside another or over CTEs, and joins of WITHOUT ROWID tables,
//! answer as SQLite 3.54.0 does (G2 step 3; AC-T1.1.1: reads as committed; AC-T1.8.6: the differential
//! suite).

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, JoinMethods, MemoryIO, OpenOptions,
    SqliteDialect, StepResult, Value, IO,
};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// One database of a fixture: the statements that fill it, and its queries with SQLite's answers
/// (`bench/sqlite_outer_join_subqueries.py`, `bench/sqlite_on_ctes.py`).
struct Cases {
    name: &'static str,
    fill: Vec<&'static str>,
    queries: Vec<(&'static str, &'static str)>,
}

fn cases(fixture: &'static str) -> Vec<Cases> {
    let mut out: Vec<Cases> = Vec::new();
    for line in fixture.lines() {
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

/// The settings a user meets a statement under: the parallel mode, statistics, and hash joins
/// wherever a join can take one, as by cost over larger tables.
#[derive(Clone, Copy, Debug)]
struct Settings {
    parallel: bool,
    statistics: bool,
    joins: JoinMethods,
}

const SETTINGS: [Settings; 5] = {
    let default = Settings {
        parallel: false,
        statistics: false,
        joins: JoinMethods::ByCost,
    };
    [
        default,
        Settings {
            parallel: true,
            ..default
        },
        Settings {
            statistics: true,
            ..default
        },
        Settings {
            parallel: true,
            statistics: true,
            ..default
        },
        Settings {
            joins: JoinMethods::HashJoinsOnly,
            ..default
        },
    ]
};

fn set(conn: &Arc<Connection>, settings: Settings) {
    if settings.parallel {
        conn.set_parallel_mode(amphora_engine::morsel::ParallelMode::On);
    }
    if settings.statistics {
        conn.execute("ANALYZE").unwrap();
    }
    conn.set_join_methods(settings.joins);
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

/// What `sql` answers under `settings` in `db`, or what its panic said.
fn outcome(io: &MemoryIO, conn: &Arc<Connection>, sql: &str) -> String {
    match catch_unwind(AssertUnwindSafe(|| answer(io, conn, sql))) {
        Ok(Ok(rows)) => rows,
        Ok(Err(error)) => format!("ERROR {error}"),
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

/// Do: run every query of the fixture under each setting. Expect: SQLite's rows, no panic and no
/// error, and every table holding after the queries the rows it held before them.
fn answers_as_sqlite(fixture: &'static str) {
    let mut wrong: Vec<String> = Vec::new();
    let mut ran = 0;
    for db in cases(fixture) {
        let tables: Vec<String> = db
            .fill
            .iter()
            .filter_map(|statement| statement.strip_prefix("CREATE TABLE "))
            .filter_map(|rest| rest.split('(').next())
            .map(|table| format!("SELECT * FROM {table}"))
            .collect();
        assert!(!tables.is_empty());
        for settings in SETTINGS {
            let (mut io, mut conn) = open(&db.fill);
            set(&conn, settings);
            let held: Vec<String> = tables.iter().map(|sql| outcome(&io, &conn, sql)).collect();
            for (sql, want) in &db.queries {
                ran += 1;
                let got = outcome(&io, &conn, sql);
                let holds: Vec<String> =
                    tables.iter().map(|sql| outcome(&io, &conn, sql)).collect();
                if got != *want || holds != held {
                    wrong.push(format!(
                        "{} {settings:?}: {sql}\n    SQLite {want}\n    Amphora {got}{}",
                        db.name,
                        if holds == held {
                            String::new()
                        } else {
                            format!("\n    TABLES {holds:?}, held {held:?}")
                        }
                    ));
                    // What a panic left behind is not the next query's to answer for.
                    (io, conn) = open(&db.fill);
                    set(&conn, settings);
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

#[test]
fn outer_joins_with_nested_subqueries_answer_as_sqlite() {
    answers_as_sqlite(include_str!("fixtures/outer_join_subqueries.tsv"));
}

/// Subqueries of ON clauses that read CTEs (finding CO), and WITHOUT ROWID tables, with and without
/// secondary indexes, on either side of a join (finding CM).
#[test]
fn on_clauses_over_ctes_and_joins_of_keyed_tables_answer_as_sqlite() {
    answers_as_sqlite(include_str!("fixtures/on_ctes.tsv"));
}
