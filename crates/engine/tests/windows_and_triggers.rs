//! Oracle findings CO, CP and DC (lane C17): window functions over each other, triggers on rowid and
//! WITHOUT ROWID tables, and the cursors a grouped outer join's subqueries and an automatic index read,
//! answer as SQLite 3.54.0 does, with no panic (CLAUDE.md 1; AC-T1.1.1: reads as committed; AC-T1.8.6:
//! the differential suite). A conflict in a trigger's statement is judged by that statement's clause
//! (lane C22). Foreign keys are counted and checked as SQLite counts and checks them (lane C26).

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, Numeric, OpenOptions, SqliteDialect,
    StepResult, Value, IO,
};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A line of a fixture (`bench/sqlite_windows.py`, `bench/sqlite_triggers.py`,
/// `bench/sqlite_unread_cursors.py`, `bench/sqlite_trigger_conflicts.py`,
/// `bench/sqlite_foreign_keys.py`, `bench/sqlite_instead_of.py`): a statement that fills the database,
/// or a query or a statement with what SQLite answered.
enum Step {
    Fill(&'static str),
    Ask(&'static str, &'static str),
}

struct Case {
    name: &'static str,
    steps: Vec<Step>,
}

fn cases(fixture: &'static str) -> Vec<Case> {
    let mut out: Vec<Case> = Vec::new();
    for line in fixture.lines() {
        let fields: Vec<&'static str> = line.split('\t').collect();
        match fields.as_slice() {
            ["db", name] => out.push(Case {
                name,
                steps: Vec::new(),
            }),
            ["sql", sql] => out.last_mut().unwrap().steps.push(Step::Fill(sql)),
            ["q", sql, want] => out.last_mut().unwrap().steps.push(Step::Ask(sql, want)),
            _ => assert!(line.starts_with('#'), "{line}"),
        }
    }
    out
}

/// What a user meets a statement under.
#[derive(Clone, Copy, Debug)]
struct Settings {
    parallel: bool,
    statistics: bool,
}

const SETTINGS: [Settings; 4] = [
    Settings {
        parallel: false,
        statistics: false,
    },
    Settings {
        parallel: true,
        statistics: false,
    },
    Settings {
        parallel: false,
        statistics: true,
    },
    Settings {
        parallel: true,
        statistics: true,
    },
];

/// A database of its own: one opened by the same name is shared across a process's tests.
fn open(settings: Settings) -> (Arc<MemoryIO>, Arc<Connection>) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let io = Arc::new(MemoryIO::new());
    let options = OpenOptions::new(Arc::new(SqliteDialect))
        .db_opts(DatabaseOpts::new().with_without_rowid(true));
    let name = format!(
        "windows_and_triggers_{}.db",
        N.fetch_add(1, Ordering::Relaxed)
    );
    let db = Database::open(io.clone(), &name, options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    if settings.parallel {
        conn.set_parallel_mode(amphora_engine::morsel::ParallelMode::On);
    }
    (io, conn)
}

/// The rows of `sql` as the fixtures write SQLite's, or its error.
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
                        Value::Numeric(Numeric::Float(real)) => format!("{:.9}", f64::from(*real)),
                        value => value.to_string(),
                    })
                    .collect();
                lines.push(values.join(","));
            }
            StepResult::Done => break,
            StepResult::IO | StepResult::Yield => io.step().map_err(|error| error.to_string())?,
            other => return Err(format!("step: {other:?}")),
        }
    }
    lines.sort();
    Ok(format!("{}:{}", lines.len(), lines.join(";")))
}

/// What `sql` answers, or the panic it ended in. SQLite's refusal of a statement is held as a refusal
/// alone, `ERROR` and no words, but for a constraint's in `trigger_conflicts.tsv` and
/// `foreign_keys.tsv`: which one refused a statement says whose clause judged it.
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

/// `ERROR~words` is a refusal whose words end so, whatever kind of refusal Amphora says it is first.
fn agrees(got: &str, want: &str) -> bool {
    if want == "ERROR" {
        got.starts_with("ERROR ")
    } else if let Some(words) = want.strip_prefix("ERROR~") {
        got.starts_with("ERROR ") && got.ends_with(words)
    } else {
        got == want
    }
}

/// Do: fill each database of the fixture and ask it every question of it, under each of `SETTINGS`.
/// Expect: SQLite's answer to each, and no panic.
fn answers_as_sqlite(fixture: &'static str) {
    let mut wrong: Vec<String> = Vec::new();
    let mut ran = 0;
    for case in cases(fixture) {
        for settings in SETTINGS {
            let (mut io, mut conn) = open(settings);
            let mut analyzed = false;
            for (at, step) in case.steps.iter().enumerate() {
                match step {
                    Step::Fill(sql) => {
                        let got = outcome(&io, &conn, sql);
                        if got.starts_with("ERROR") || got.starts_with("PANIC") {
                            wrong.push(format!(
                                "{} {settings:?}: {sql}\n    SQLite takes it\n    Amphora {got}",
                                case.name
                            ));
                        }
                    }
                    Step::Ask(sql, want) => {
                        if settings.statistics && !analyzed {
                            analyzed = true;
                            outcome(&io, &conn, "ANALYZE");
                        }
                        ran += 1;
                        let got = outcome(&io, &conn, sql);
                        if agrees(&got, want) {
                            continue;
                        }
                        wrong.push(format!(
                            "{} {settings:?}: {sql}\n    SQLite {want}\n    Amphora {got}",
                            case.name
                        ));
                        if got.starts_with("PANIC") {
                            // What a panic left behind is not the next question's to answer for.
                            (io, conn) = open(settings);
                            for earlier in &case.steps[..at] {
                                if let Step::Fill(sql) = earlier {
                                    outcome(&io, &conn, sql);
                                }
                            }
                        }
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

/// Finding CO: every window function, alone, in an expression and read twice, under one to three windows
/// of a query, beside joins, groups, DISTINCT, ORDER BY and subqueries.
#[test]
fn windows_over_windows_answer_as_sqlite() {
    answers_as_sqlite(include_str!("fixtures/windows.tsv"));
}

/// Finding CP: triggers of every event and time on rowid and WITHOUT ROWID tables, fired by every
/// statement under every conflict clause.
#[test]
fn triggers_fire_as_sqlite_fires_them() {
    answers_as_sqlite(include_str!("fixtures/triggers.tsv"));
}

/// Lane C22: every clause on a statement, on a trigger's, and on a statement of a trigger that one fires;
/// DO UPDATE, DELETE and foreign keys' actions, which have a clause of their own; constraints declared
/// with one; in transactions and savepoints. Expect what the tables and the triggers' log hold, whether
/// the transaction stands, and which constraint refused.
#[test]
fn a_conflict_is_judged_by_the_clause_of_the_statement_that_meets_it() {
    answers_as_sqlite(include_str!("fixtures/trigger_conflicts.tsv"));
}

/// Lane C28 (lang_createtrigger.html, section 3): INSTEAD OF INSERT, DELETE, UPDATE and UPDATE OF
/// triggers on views of every kind, in main and in temp, fired by every statement under every conflict
/// clause. Expect what the statement returns, what the tables and the triggers' log hold, `changes()`,
/// `total_changes()`, `last_insert_rowid()`, and SQLite's words for what it refuses.
#[test]
fn instead_of_triggers_fire_as_sqlite_fires_them() {
    answers_as_sqlite(include_str!("fixtures/instead_of.tsv"));
}

/// Lane C26: foreign keys under every action, immediate and deferred, of tables of every shape, beside
/// triggers that write them, and the order a WITHOUT ROWID table's rows go in. Expect what the tables
/// and the triggers' log hold, whether the transaction stands, and which constraint refused.
#[test]
fn foreign_keys_and_triggers_leave_what_sqlite_leaves() {
    answers_as_sqlite(include_str!("fixtures/foreign_keys.tsv"));
}

/// Finding DC: subqueries of grouped and aggregate outer joins, and row-value ranges across a join.
#[test]
fn grouped_outer_joins_and_row_ranges_answer_as_sqlite() {
    answers_as_sqlite(include_str!("fixtures/unread_cursors.tsv"));
}
