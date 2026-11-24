//! Oracle findings CQ, CP, CX, CR and DE (lane C21): a column read through something else keeps what
//! SQLite 3.54.0 keeps of it, its row, its affinity and its collation. Through a table function's
//! argument, a table function's own columns, a trigger's new and old, an IN beside a joined
//! subquery, an aggregate's argument (G2 step 3; AC-T1.1.1: reads as committed; AC-T1.8.6: the
//! differential suite).

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, JoinMethods, MemoryIO, Numeric, OpenOptions,
    SqliteDialect, StepResult, Value, IO,
};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// `bench/sqlite_columns_read.py`: every class's databases, each with its queries and SQLite's answers.
const FIXTURE: &str = include_str!("fixtures/columns_read.tsv");

struct Cases {
    name: &'static str,
    /// The tables, the rows, the triggers and the writes that fire them.
    statements: Vec<&'static str>,
    queries: Vec<(&'static str, &'static str)>,
}

fn cases(class: &str) -> Vec<Cases> {
    let mut out: Vec<Cases> = Vec::new();
    let mut kept = false;
    for line in FIXTURE.lines() {
        let fields: Vec<&'static str> = line.split('\t').collect();
        match fields.as_slice() {
            ["db", kind, name] => {
                kept = *kind == class;
                if kept {
                    out.push(Cases {
                        name,
                        statements: Vec::new(),
                        queries: Vec::new(),
                    });
                }
            }
            ["sql", sql] if kept => out.last_mut().unwrap().statements.push(sql),
            ["q", sql, want] if kept => out.last_mut().unwrap().queries.push((sql, want)),
            _ => assert!(!kept || line.starts_with('#'), "{line}"),
        }
    }
    out
}

/// The settings a user meets a statement under, and hash joins wherever a join can take one.
#[derive(Clone, Copy, Debug)]
struct Settings {
    parallel: bool,
    statistics: bool,
    joins: JoinMethods,
}

const fn settings(parallel: bool, statistics: bool, joins: JoinMethods) -> Settings {
    Settings {
        parallel,
        statistics,
        joins,
    }
}

const SETTINGS: [Settings; 5] = [
    settings(false, false, JoinMethods::ByCost),
    settings(true, false, JoinMethods::ByCost),
    settings(false, true, JoinMethods::ByCost),
    settings(true, true, JoinMethods::ByCost),
    settings(false, false, JoinMethods::HashJoinsOnly),
];

/// Each join method wherever a join can take it, over the tables' own statistics too.
const JOIN_SETTINGS: [Settings; 9] = [
    settings(false, false, JoinMethods::ByCost),
    settings(true, false, JoinMethods::ByCost),
    settings(false, true, JoinMethods::ByCost),
    settings(true, true, JoinMethods::ByCost),
    settings(false, false, JoinMethods::HashJoinsOnly),
    settings(true, true, JoinMethods::HashJoinsOnly),
    settings(false, false, JoinMethods::MergeJoinsOnly),
    settings(false, false, JoinMethods::NestedLoopsOnly),
    settings(true, true, JoinMethods::NestedLoopsOnly),
];

/// A database of its own (one opened by the same name is shared across a process's tests) with
/// `statements` run in it under `settings`, and what each that failed said.
fn open(statements: &[&str], settings: Settings) -> (Arc<MemoryIO>, Arc<Connection>, Vec<String>) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let io = Arc::new(MemoryIO::new());
    let options = OpenOptions::new(Arc::new(SqliteDialect))
        .db_opts(DatabaseOpts::new().with_without_rowid(true));
    let name = format!("columns_read_{}.db", N.fetch_add(1, Ordering::Relaxed));
    let db = Database::open(io.clone(), &name, options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    conn.set_join_methods(settings.joins);
    if settings.parallel {
        conn.set_parallel_mode(amphora_engine::morsel::ParallelMode::On);
    }
    let mut failed = Vec::new();
    for statement in statements {
        if let Err(error) = conn.execute(*statement) {
            failed.push(format!("{statement}: {error}"));
        }
        if settings.statistics && statement.starts_with("INSERT INTO m ") {
            conn.execute("ANALYZE").unwrap();
        }
    }
    if settings.statistics {
        conn.execute("ANALYZE").unwrap();
    }
    (io, conn, failed)
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
                        Value::Numeric(Numeric::Float(f)) => format!("{:?}", f64::from(*f)),
                        Value::Text(text) => format!("'{}'", text.as_str()),
                        Value::Blob(blob) => format!(
                            "x'{}'",
                            blob.iter().map(|b| format!("{b:02x}")).collect::<String>()
                        ),
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

/// What `sql` answers, or what its error or its panic said.
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

/// Do: make each database of the class under each of `all_settings` and run its queries. Expect: no
/// statement refused that SQLite takes, and SQLite's rows.
fn answers_as_sqlite(class: &str, all_settings: &[Settings], at_least: usize) {
    let mut wrong: Vec<String> = Vec::new();
    let mut ran = 0;
    for db in cases(class) {
        for &settings in all_settings {
            let (mut io, mut conn, failed) = open(&db.statements, settings);
            for said in failed {
                wrong.push(format!("{} {settings:?}: refused {said}", db.name));
            }
            for (sql, want) in &db.queries {
                ran += 1;
                let got = outcome(&io, &conn, sql);
                if got != *want {
                    wrong.push(format!(
                        "{} {settings:?}: {sql}\n    SQLite  {want}\n    Amphora {got}",
                        db.name
                    ));
                    if got.starts_with("PANIC") {
                        // What a panic left behind is not the next query's to answer for.
                        (io, conn, _) = open(&db.statements, settings);
                    }
                }
            }
        }
    }
    assert!(ran >= at_least, "{ran} answers, not {at_least}");
    assert!(
        wrong.is_empty(),
        "{} of {ran} answers differ from SQLite's:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// Finding CQ: a correlated subquery in a table function's argument, in every join, is run for the
/// row in hand, the NULL row of a RIGHT or FULL JOIN's unmatched rows too, and coded once.
#[test]
fn table_functions_with_correlated_arguments_answer_as_sqlite() {
    answers_as_sqlite("cq", &SETTINGS, 3_000);
}

/// Finding CP: key, value and atom of json_each and json_tree, and pragma_table_info's columns,
/// declare no type: they compare with columns of every affinity and collation as SQLite's do.
#[test]
fn table_functions_untyped_columns_compare_as_sqlites_do() {
    answers_as_sqlite("cp", &SETTINGS, 6_000);
}

/// Finding CX: new.c and old.c of every collation and affinity, in WHEN, in bodies and in their
/// subqueries, of triggers that INSERT, UPDATE, DELETE, REPLACE, IGNORE and UPSERT fire.
#[test]
fn triggers_read_their_rows_as_sqlites_do() {
    answers_as_sqlite("cx", &SETTINGS, 3_000);
}

/// Finding CR: a subquery in an EXISTS, NOT EXISTS, IN or NOT IN, its own IN's left side read from
/// either, by every join method.
#[test]
fn subqueries_in_subqueries_answer_as_sqlite_by_every_join_method() {
    answers_as_sqlite("cr", &JOIN_SETTINGS, 7_000);
}

/// Finding DE: columns of every collation in aggregates' arguments and FILTERs, grouped by columns,
/// expressions and subqueries, and in window functions.
#[test]
fn aggregates_of_collated_arguments_answer_as_sqlite() {
    answers_as_sqlite("de", &SETTINGS, 5_000);
}
