//! AC-T1.8.6: the rows INSERT, UPSERT, REPLACE and UPDATE store, and the NEW and OLD values their
//! triggers see, as SQLite 3.54.0 has them: `excluded.*` for every column, the rowid and its aliases,
//! STRICT or not, with a rowid or without, generated columns among them (the oracles' findings DA and
//! CV, lane C14). T-T1.7.1: every record read back as written.

use amphora_engine::morsel::ParallelMode;
use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, Numeric, OpenOptions, SqliteDialect,
    Value,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// The settings a user meets a statement under: the parallel mode, and whether the database has
/// statistics.
const SETTINGS: [(&str, bool, bool); 3] = [
    ("default", false, false),
    ("parallel", true, false),
    ("statistics", false, true),
];

fn open(parallel: bool) -> Arc<Connection> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let opts = DatabaseOpts::new()
        .with_without_rowid(true)
        .with_generated_columns(true);
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(opts);
    let name = format!("dml_rows_{}.db", N.fetch_add(1, Ordering::Relaxed));
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
        Value::Blob(blob) => format!(
            "x'{}'",
            blob.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ),
        value => value.to_string(),
    }
}

/// The rows of `sql`, sorted, or why it is refused.
fn rows(conn: &Arc<Connection>, sql: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
    let mut rows: Vec<String> = stmt
        .run_collect_rows()
        .map_err(|error| error.to_string())?
        .iter()
        .map(|row| row.iter().map(cell).collect::<Vec<_>>().join(","))
        .collect();
    rows.sort();
    Ok(rows)
}

/// One line of a case: what it is (a CREATE, a statement, a query), its SQL, and SQLite's answer.
type Line = (&'static str, &'static str, &'static str);

fn cases() -> Vec<Vec<Line>> {
    let mut cases: Vec<Vec<Line>> = Vec::new();
    for line in Box::leak(std::fs::read_to_string(std::env::var("C14_FIXTURE").unwrap()).unwrap().into_boxed_str()).lines() {
        match line.split('\t').collect::<Vec<_>>().as_slice() {
            ["case"] => cases.push(Vec::new()),
            ["sql", sql] => cases.last_mut().unwrap().push(("sql", sql, "ok")),
            [kind @ ("do" | "rows"), sql, want] => {
                cases.last_mut().unwrap().push((*kind, *sql, *want));
            }
            _ => assert!(line.starts_with('#'), "{line}"),
        }
    }
    cases
}

/// What Amphora answers to each line of `case`, beside whether it is SQLite's answer.
fn answers(case: &[Line], parallel: bool, statistics: bool) -> Vec<(String, bool)> {
    let conn = open(parallel);
    let mut analyzed = false;
    case.iter()
        .map(|(kind, sql, want)| {
            let got = rows(&conn, sql);
            if *kind == "do" && statistics && !analyzed {
                analyzed = true;
                conn.execute("ANALYZE").unwrap();
            }
            let text = match got {
                Ok(rows) if *kind == "rows" => rows.join(" / "),
                Ok(_) => "ok".to_owned(),
                Err(error) => format!("refused: {error}"),
            };
            let same =
                text == *want || text.starts_with("refused: ") && want.starts_with("refused: ");
            (text, same)
        })
        .collect()
}

/// Do: run the fixture's cases of `part` of `PARTS` under each setting. Expect: what SQLite answers to
/// each statement (done or refused) and the rows it leaves in the table and in its triggers' log.
fn stores_as_sqlite(part: usize) {
    const PARTS: usize = 8;
    let mut wrong = 0;
    let mut ran = 0;
    for (index, case) in cases().iter().enumerate().filter(|(i, _)| i % PARTS == part) {
        for (setting, parallel, statistics) in SETTINGS {
            ran += 1;
            let answers = std::panic::catch_unwind(|| answers(case, parallel, statistics));
            let lines = match &answers {
                Ok(answers) if answers.iter().all(|(_, same)| *same) => continue,
                Ok(answers) => case
                    .iter()
                    .zip(answers)
                    .map(|((_, sql, want), (got, same))| {
                        if *same {
                            format!("  {sql}  -- {want}")
                        } else {
                            format!("  {sql}\n    SQLite  {want}\n    Amphora {got}")
                        }
                    })
                    .collect::<Vec<_>>(),
                Err(panic) => case
                    .iter()
                    .map(|(_, sql, want)| format!("  {sql}  -- {want}"))
                    .chain([format!(
                        "    Amphora PANIC {:?}",
                        panic
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| panic.downcast_ref::<&str>().copied())
                    )])
                    .collect(),
            };
            wrong += 1;
            eprintln!("WRONG case {index}, {setting}:\n{}", lines.join("\n"));
        }
    }
    eprintln!("{ran} runs, {wrong} wrong");
    assert!(ran > 0, "the fixture holds no case of this part");
    assert_eq!(wrong, 0, "{wrong} of {ran} runs store other rows");
}

#[test]
fn statements_store_sqlites_rows_0() {
    stores_as_sqlite(0);
}

#[test]
fn statements_store_sqlites_rows_1() {
    stores_as_sqlite(1);
}

#[test]
fn statements_store_sqlites_rows_2() {
    stores_as_sqlite(2);
}

#[test]
fn statements_store_sqlites_rows_3() {
    stores_as_sqlite(3);
}

#[test]
fn statements_store_sqlites_rows_4() {
    stores_as_sqlite(4);
}

#[test]
fn statements_store_sqlites_rows_5() {
    stores_as_sqlite(5);
}

#[test]
fn statements_store_sqlites_rows_6() {
    stores_as_sqlite(6);
}

#[test]
fn statements_store_sqlites_rows_7() {
    stores_as_sqlite(7);
}
