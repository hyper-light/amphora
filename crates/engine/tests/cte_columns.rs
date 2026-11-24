//! AC-T1.8.6: a column of a recursive or ordinary CTE, a derived table or a compound, compared, joined,
//! sorted and grouped by as SQLite 3.54.0 does it: by the affinity and the collation of the expression the
//! column is made from. A recursive CTE's own arms read its columns as a BLOB column's, so a number never
//! met the text of a TEXT column (the oracles' finding CU, lane C16).

use amphora_engine::morsel::ParallelMode;
use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect, Value,
};
use std::collections::BTreeMap;
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
    let name = format!("cte_columns_{}.db", N.fetch_add(1, Ordering::Relaxed));
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

/// The rows of `sql` in its order, each row's values joined by commas.
fn rows(conn: &Arc<Connection>, sql: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
    Ok(stmt
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
        .collect())
}

/// FNV-1a 64 of lines, as `bench/sqlite_cte_columns.py` sums SQLite's rows.
fn lines_sum(lines: &[String]) -> u64 {
    lines
        .join("\n")
        .bytes()
        .fold(0xCBF2_9CE4_8422_2325_u64, |sum, byte| {
            (sum ^ u64::from(byte)).wrapping_mul(0x100_0000_01B3)
        })
}

/// The fixture: the statements that fill the database, and the parts its queries are put together from.
#[derive(Default)]
struct Sweep {
    fill: Vec<&'static str>,
    sources: BTreeMap<&'static str, &'static str>,
    shapes: BTreeMap<&'static str, &'static str>,
    steps: BTreeMap<&'static str, &'static str>,
    outers: BTreeMap<&'static str, &'static str>,
    /// Shape, source, tail, SQLite's answer.
    queries: Vec<[&'static str; 4]>,
}

impl Sweep {
    fn read() -> Self {
        let mut sweep = Self::default();
        for line in include_str!("fixtures/cte_columns.tsv").lines() {
            match line.split('\t').collect::<Vec<_>>().as_slice() {
                ["sql", sql] => sweep.fill.push(sql),
                ["source", name, text] => drop(sweep.sources.insert(name, text)),
                ["shape", name, text] => drop(sweep.shapes.insert(name, text)),
                ["tail", "step", name, text] => drop(sweep.steps.insert(name, text)),
                ["tail", "outer", name, text] => drop(sweep.outers.insert(name, text)),
                ["q", shape, source, tail, answer] => {
                    sweep.queries.push([shape, source, tail, answer]);
                }
                _ => assert!(line.starts_with('#'), "{line}"),
            }
        }
        sweep
    }

    fn sql(&self, shape: &str, source: &str, tail: &str) -> String {
        let text = self.shapes[shape];
        let step = shape.starts_with("step") || shape.starts_with("mixed_step");
        let tail = if step {
            self.steps[tail]
        } else {
            self.outers[tail]
        };
        let text = match text.strip_prefix("@TAIL:") {
            Some(from) => tail.replace("@W@", from.trim_end_matches('@')),
            None => text.replace("@TAIL@", &tail.replace("@W@", "w")),
        };
        text.replace("@E@", self.sources[source])
    }
}

/// Do: run every query of the fixture whose shape `shapes` names under one setting. Expect: SQLite's rows.
fn answers_as_sqlite(setting: usize, shapes: impl Fn(&str) -> bool) {
    let sweep = Sweep::read();
    let (setting, parallel, statistics) = SETTINGS[setting];
    let conn = open(&sweep.fill, parallel, statistics);
    let mut wrong: Vec<String> = Vec::new();
    let mut run = 0_usize;
    for [shape, source, tail, want] in &sweep.queries {
        if !shapes(shape) {
            continue;
        }
        run += 1;
        let sql = sweep.sql(shape, source, tail);
        let got = rows(&conn, &sql).map(|mut rows| {
            if !tail.starts_with("ordered") {
                rows.sort();
            }
            rows
        });
        let answer = match &got {
            Ok(rows) => format!("{}:{:016x}", rows.len(), lines_sum(rows)),
            Err(error) => format!("refused: {error}"),
        };
        if answer != *want {
            wrong.push(format!(
                "{shape} {source} {tail}: got {answer}, SQLite {want}: {sql}"
            ));
        }
    }
    eprintln!("{setting}: {run} queries, {} wrong", wrong.len());
    for line in &wrong {
        eprintln!("WRONG {setting}: {line}");
    }
    assert!(run > 0 && wrong.is_empty(), "{} wrong", wrong.len());
}

fn recursive(shape: &str) -> bool {
    shape.starts_with("rec")
        || shape.starts_with("step")
        || shape.contains("_rec_")
        || shape.contains("_step_")
}

#[test]
fn a_recursive_ctes_columns_compare_as_sqlites() {
    answers_as_sqlite(0, recursive);
}

#[test]
fn a_recursive_ctes_columns_compare_as_sqlites_parallel() {
    answers_as_sqlite(1, recursive);
}

#[test]
fn a_recursive_ctes_columns_compare_as_sqlites_statistics() {
    answers_as_sqlite(2, recursive);
}

#[test]
fn a_recursive_ctes_columns_compare_as_sqlites_parallel_statistics() {
    answers_as_sqlite(3, recursive);
}

#[test]
fn a_subquerys_columns_compare_as_sqlites() {
    answers_as_sqlite(0, |shape| !recursive(shape));
}

#[test]
fn a_subquerys_columns_compare_as_sqlites_parallel() {
    answers_as_sqlite(1, |shape| !recursive(shape));
}

#[test]
fn a_subquerys_columns_compare_as_sqlites_statistics() {
    answers_as_sqlite(2, |shape| !recursive(shape));
}

#[test]
fn a_subquerys_columns_compare_as_sqlites_parallel_statistics() {
    answers_as_sqlite(3, |shape| !recursive(shape));
}
