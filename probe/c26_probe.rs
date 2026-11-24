//! Lane C26's scratch probe (never merged): asks the fixture named by C26_FIXTURE, at default and
//! parallel, and prints each answer that differs. A refusal compares by its words when the fixture has
//! them.
use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, Numeric, OpenOptions, SqliteDialect,
    StepResult, Value, IO,
};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn open(parallel: bool) -> (Arc<MemoryIO>, Arc<Connection>) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let io = Arc::new(MemoryIO::new());
    let options = OpenOptions::new(Arc::new(SqliteDialect))
        .db_opts(DatabaseOpts::new().with_without_rowid(true));
    let name = format!("c26_probe_{}.db", N.fetch_add(1, Ordering::Relaxed));
    let db = Database::open(io.clone(), &name, options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    if parallel {
        conn.set_parallel_mode(amphora_engine::morsel::ParallelMode::On);
    }
    (io, conn)
}

fn answer(io: &MemoryIO, conn: &Arc<Connection>, sql: &str, explain: bool) -> Result<String, String> {
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
                lines.push(values.join(if explain { "  " } else { "," }));
            }
            StepResult::Done => break,
            StepResult::IO | StepResult::Yield => io.step().map_err(|error| error.to_string())?,
            other => return Err(format!("step: {other:?}")),
        }
    }
    if explain {
        return Ok(lines.join("\n"));
    }
    lines.sort();
    Ok(format!("{}:{}", lines.len(), lines.join(";")))
}

fn outcome(io: &MemoryIO, conn: &Arc<Connection>, sql: &str, explain: bool) -> String {
    match catch_unwind(AssertUnwindSafe(|| answer(io, conn, sql, explain))) {
        Ok(Ok(rows)) => rows,
        Ok(Err(error)) => format!("ERROR {error}"),
        Err(panic) => format!(
            "PANIC {}",
            panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|text| text.to_string()))
                .unwrap_or_default()
        ),
    }
}

#[test]
fn probe() {
    let text = std::fs::read_to_string(std::env::var("C26_FIXTURE").unwrap()).unwrap();
    let only = std::env::var("C26_ONLY").ok();
    let verbose = std::env::var("C26_VERBOSE").is_ok();
    let mut dbs: Vec<(String, Vec<Vec<String>>)> = Vec::new();
    for line in text.lines() {
        let fields: Vec<String> = line.split('\t').map(str::to_owned).collect();
        match fields[0].as_str() {
            "db" => dbs.push((fields[1].clone(), Vec::new())),
            "sql" | "q" | "x" => dbs.last_mut().unwrap().1.push(fields),
            _ => assert!(line.starts_with('#'), "{line}"),
        }
    }
    let (mut ran, mut differ, mut words, mut panics) = (0, 0, 0, 0);
    let mut first: Vec<String> = Vec::new();
    for (name, steps) in &dbs {
        if only.as_ref().is_some_and(|only| only != name) {
            continue;
        }
        let mut differs_by_setting = Vec::new();
        for parallel in [false, true] {
            let (io, conn) = open(parallel);
            let mut lines = Vec::new();
            for step in steps {
                if step[0] == "x" {
                    println!("EXPLAIN {}\n{}", step[1], outcome(&io, &conn, &format!("EXPLAIN {}", step[1]), true));
                    continue;
                }
                let got = outcome(&io, &conn, &step[1], false);
                if step[0] == "sql" {
                    if got.starts_with("ERROR") || got.starts_with("PANIC") {
                        lines.push(format!("  FILL {} => {got}", step[1]));
                    }
                    continue;
                }
                ran += 1;
                let want = &step[2];
                if verbose {
                    println!("{name} parallel={parallel}: {}\n    SQLite  {want}\n    Amphora {got}", step[1]);
                }
                if got.starts_with("PANIC") {
                    panics += 1;
                }
                if &got == want || (want == "ERROR" && got.starts_with("ERROR ")) {
                    continue;
                }
                if want.starts_with("ERROR ") && got.starts_with("ERROR ") {
                    words += 1;
                    lines.push(format!("  WORDS {}\n    SQLite  {want}\n    Amphora {got}", step[1]));
                    continue;
                }
                differ += 1;
                lines.push(format!("  DIFFER {}\n    SQLite  {want}\n    Amphora {got}", step[1]));
            }
            differs_by_setting.push(lines);
        }
        if differs_by_setting.iter().any(|lines| !lines.is_empty()) {
            let same = differs_by_setting[0] == differs_by_setting[1];
            println!("##### {name} (parallel {})\n{}", if same { "the same" } else { "DIFFERENT" }, differs_by_setting[0].join("\n"));
            if !same {
                println!("  ---- parallel\n{}", differs_by_setting[1].join("\n"));
            }
            first.push(name.clone());
        }
    }
    println!("ASKED {ran}, DIFFER {differ}, WORDS {words}, PANICS {panics}, DATABASES {}: {}", first.len(), first.join(" | "));
}
