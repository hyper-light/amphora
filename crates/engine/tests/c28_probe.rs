//! Oracle findings CO, CP and DC (lane C17): window functions over each other, triggers on rowid and
//! WITHOUT ROWID tables, and the cursors a grouped outer join's subqueries and an automatic index read,
//! answer as SQLite 3.54.0 does, with no panic (CLAUDE.md 1; AC-T1.1.1: reads as committed; AC-T1.8.6:
//! the differential suite). A conflict in a trigger's statement is judged by that statement's clause
//! (lane C22).

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, Numeric, OpenOptions, SqliteDialect,
    StepResult, Value, IO,
};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A line of a fixture (`bench/sqlite_windows.py`, `bench/sqlite_triggers.py`,
/// `bench/sqlite_unread_cursors.py`, `bench/sqlite_trigger_conflicts.py`, `bench/sqlite_instead_of.py`):
/// a statement that fills the database, or a query or a statement with what SQLite answered.
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
/// alone, `ERROR` and no words, but for a constraint's in `trigger_conflicts.tsv`: which one refused a
/// statement says whose clause judged it.
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


fn probe(fixture: &'static str, settings: Settings) {
    use std::io::Write;
    let only = std::env::var("C28_ONLY").ok();
    let mut wrong = 0usize;
    let mut ran = 0usize;
    let out = std::env::var("C28_OUT").unwrap();
    let mut trace = std::fs::File::create(format!("{out}.trace")).unwrap();
    let mut diff = std::fs::File::create(format!("{out}.diff")).unwrap();
    for case in cases(fixture) {
        if only.as_ref().is_some_and(|only| !case.name.contains(only.as_str())) {
            continue;
        }
        let (mut io, mut conn) = open(settings);
        let mut analyzed = false;
        for (at, step) in case.steps.iter().enumerate() {
            match step {
                Step::Fill(sql) => {
                    writeln!(trace, "{}: {sql}", case.name).unwrap();
                    let got = outcome(&io, &conn, sql);
                    if got.starts_with("ERROR") || got.starts_with("PANIC") {
                        wrong += 1;
                        writeln!(diff, "{}: {sql}\n    SQLite takes it\n    Amphora {got}", case.name).unwrap();
                    }
                }
                Step::Ask(sql, want) => {
                    if settings.statistics && !analyzed {
                        analyzed = true;
                        outcome(&io, &conn, "ANALYZE");
                    }
                    ran += 1;
                    writeln!(trace, "{}: {sql}", case.name).unwrap();
                    let got = outcome(&io, &conn, sql);
                    if agrees(&got, want) {
                        continue;
                    }
                    wrong += 1;
                    writeln!(diff, "{}: {sql}\n    SQLite  {want}\n    Amphora {got}", case.name).unwrap();
                    if got.starts_with("PANIC") {
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
    writeln!(diff, "{wrong} of {ran} differ").unwrap();
    println!("{wrong} of {ran} differ");
}

#[test]
fn probe_instead_of() {
    let at: usize = std::env::var("C28_SETTING").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let path = std::env::var("C28_FIXTURE").unwrap();
    let fixture: &'static str = Box::leak(std::fs::read_to_string(path).unwrap().into_boxed_str());
    probe(fixture, SETTINGS[at]);
}
