//! AC-T1.8.6: joins whose ON holds terms on the left tables alone, answered as SQLite 3.54.0 answers
//! them. An outer join's ON term on a left table says which rows match, and the left row it refuses is
//! kept, NULL-extended; a multi-index AND took such a term for a filter of its table and dropped the
//! row (the oracles' finding CG, lane C9).

use amphora_engine::morsel::ParallelMode;
use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect, Value,
};
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
    let name = format!("on_terms_{}.db", N.fetch_add(1, Ordering::Relaxed));
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

/// The rows of `sql`, sorted, each row's values joined by commas.
fn rows(conn: &Arc<Connection>, sql: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
    let mut rows: Vec<String> = stmt
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
    rows.sort();
    Ok(rows)
}

fn plan(conn: &Arc<Connection>, sql: &str) -> String {
    let mut stmt = conn.prepare(format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    stmt.run_collect_rows()
        .unwrap()
        .iter()
        .map(|row| row[3].to_string())
        .collect::<Vec<_>>()
        .join(" | ")
}

/// FNV-1a 64 of lines, as `bench/sqlite_on_terms.py` sums SQLite's rows.
fn lines_sum(lines: &[String]) -> u64 {
    lines
        .join("\n")
        .bytes()
        .fold(0xCBF2_9CE4_8422_2325_u64, |sum, byte| {
            (sum ^ u64::from(byte)).wrapping_mul(0x100_0000_01B3)
        })
}

/// One database of the fixture: the statements that fill it, and its queries with SQLite's answers.
#[derive(Default)]
struct OnTerms {
    fill: Vec<&'static str>,
    queries: Vec<(&'static str, &'static str)>,
}

fn on_terms(name: &str) -> OnTerms {
    let mut out = OnTerms::default();
    let mut here = false;
    for line in include_str!("fixtures/on_terms.tsv").lines() {
        match line.split('\t').collect::<Vec<_>>().as_slice() {
            ["db", db] => here = *db == name,
            ["sql", sql] if here => out.fill.push(sql),
            ["q", sql, answer] if here => out.queries.push((sql, answer)),
            ["sql" | "q", ..] => {}
            _ => assert!(line.starts_with('#'), "{line}"),
        }
    }
    out
}

/// Do: run every query of the fixture's database `name` under each setting. Expect: SQLite's rows; and
/// that the plans were worth it: multi-index ANDs and ORs among them, under the settings without
/// statistics (and with them where `analyzed_and`).
fn answers_as_sqlite(name: &str, analyzed_and: bool) {
    let db = on_terms(name);
    assert!(!db.queries.is_empty(), "{name}");
    let mut wrong: Vec<String> = Vec::new();
    for (setting, parallel, statistics) in SETTINGS {
        let conn = open(&db.fill, parallel, statistics);
        let (mut ands, mut ors, mut bad) = (0, 0, 0);
        for (sql, want) in &db.queries {
            let plan = plan(&conn, sql);
            ands += usize::from(plan.contains("MULTI-INDEX AND"));
            ors += usize::from(plan.contains("MULTI-INDEX OR"));
            let got = rows(&conn, sql);
            let answer = match &got {
                Ok(rows) => format!("{}:{:016x}", rows.len(), lines_sum(rows)),
                Err(error) => format!("refused: {error}"),
            };
            if answer != *want {
                bad += 1;
                wrong.push(format!(
                    "{name}, {setting}: got {answer}, SQLite {want}: {sql}: first rows {:?}: {plan}",
                    got.iter().flatten().take(6).collect::<Vec<_>>()
                ));
            }
        }
        eprintln!(
            "{name}, {setting}: {} queries, {bad} wrong, {ands} with a multi-index AND, {ors} with a multi-index OR",
            db.queries.len()
        );
        if !statistics || analyzed_and {
            assert!(ands > 0, "{name}, {setting}: no multi-index AND was planned");
        }
        if !statistics {
            assert!(ors > 0, "{name}, {setting}: no multi-index OR was planned");
        }
    }
    for line in &wrong {
        eprintln!("WRONG {line}");
    }
    assert!(wrong.is_empty(), "{} wrong", wrong.len());
}

#[test]
fn joins_with_on_terms_of_either_side_answer_as_sqlite_small() {
    answers_as_sqlite("small", false);
}

#[test]
fn joins_with_on_terms_of_either_side_answer_as_sqlite_partial() {
    answers_as_sqlite("partial", false);
}

#[test]
fn joins_with_on_terms_of_either_side_answer_as_sqlite_large() {
    answers_as_sqlite("large", true);
}
