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
/// that the plans were worth it: each text of `planned` is in some query's plan in some setting.
fn answers_as_sqlite(name: &str, planned: &[&str]) {
    let db = on_terms(name);
    assert!(!db.queries.is_empty(), "{name}");
    let mut wrong: Vec<String> = Vec::new();
    let mut counts = vec![0_usize; planned.len()];
    for (setting, parallel, statistics) in SETTINGS {
        let conn = open(&db.fill, parallel, statistics);
        let before = wrong.len();
        for (sql, want) in &db.queries {
            let plan = plan(&conn, sql);
            for (count, text) in counts.iter_mut().zip(planned) {
                *count += usize::from(plan.contains(text));
            }
            let got = rows(&conn, sql);
            let answer = match &got {
                Ok(rows) => format!("{}:{:016x}", rows.len(), lines_sum(rows)),
                Err(error) => format!("refused: {error}"),
            };
            if answer != *want {
                wrong.push(format!(
                    "{name}, {setting}: got {answer}, SQLite {want}: {sql}: first rows {:?}: {plan}",
                    got.iter().flatten().take(6).collect::<Vec<_>>()
                ));
            }
        }
        eprintln!(
            "{name}, {setting}: {} queries, {} wrong, plans so far with each of {planned:?}: {counts:?}",
            db.queries.len(),
            wrong.len() - before
        );
    }
    for line in &wrong {
        eprintln!("WRONG {line}");
    }
    assert!(wrong.is_empty(), "{} wrong", wrong.len());
    assert!(!counts.contains(&0), "{name}: {planned:?}: {counts:?}");
}

#[test]
fn joins_with_on_terms_of_either_side_answer_as_sqlite_small() {
    answers_as_sqlite("small", &["MULTI-INDEX AND", "MULTI-INDEX OR"]);
}

#[test]
fn joins_with_on_terms_of_either_side_answer_as_sqlite_large() {
    answers_as_sqlite("large", &["MULTI-INDEX AND", "MULTI-INDEX OR"]);
}

/// The indexes on x and y are partial: a plan that names one has proven its filter.
#[test]
fn joins_with_on_terms_of_either_side_answer_as_sqlite_partial_indexes() {
    answers_as_sqlite("partial", &["_x", "_y"]);
}

/// Do: an inner join whose right table has a partial index, then a LEFT JOIN whose ON holds the index's
/// own term on that table (a soft-deleted row's flag, say), under each setting. Expect: SQLite's rows.
/// The ON term was taken for the index's proof, so the read of the index dropped the left rows the
/// LEFT JOIN keeps.
#[test]
fn a_left_joins_on_term_proves_no_partial_index_of_a_left_table() {
    for (setting, parallel, statistics) in SETTINGS {
        let conn = open(
            &[
                "CREATE TABLE a(id INTEGER PRIMARY KEY, x INT)",
                "CREATE TABLE b(id INTEGER PRIMARY KEY, aid INT, live INT)",
                "CREATE INDEX b_live ON b(aid) WHERE live = 1",
                "CREATE TABLE c(id INTEGER PRIMARY KEY)",
                "INSERT INTO a VALUES (1, 1), (2, 2), (3, 3)",
                "INSERT INTO b VALUES (1, 1, 1), (2, 1, 0), (3, 3, 1), (4, 3, 0), (5, 3, NULL)",
                "INSERT INTO c VALUES (1), (2), (3)",
            ],
            parallel,
            statistics,
        );
        for (sql, want) in [
            (
                "SELECT a.id, b.id, c.id FROM a JOIN b ON b.aid = a.id LEFT JOIN c ON b.live = 1 AND c.id = b.id",
                ["1,1,1", "1,2,NULL", "3,3,3", "3,4,NULL", "3,5,NULL"].as_slice(),
            ),
            (
                "SELECT a.id, b.id, c.id FROM a JOIN b ON b.aid = a.id AND b.live = 1 LEFT JOIN c ON c.id = b.id",
                &["1,1,1", "3,3,3"],
            ),
            (
                "SELECT a.id, b.id, c.id FROM a LEFT JOIN b ON b.aid = a.id AND b.live = 1 LEFT JOIN c ON c.id = b.id",
                &["1,1,1", "2,NULL,NULL", "3,3,3"],
            ),
            (
                "SELECT a.id, b.id, c.id FROM a JOIN b ON b.aid = a.id LEFT JOIN c ON c.id = b.id WHERE b.live = 1",
                &["1,1,1", "3,3,3"],
            ),
        ] {
            assert_eq!(rows(&conn, sql).unwrap(), want, "{setting}: {sql}: {}", plan(&conn, sql));
        }
    }
}

/// Do: plan, without statistics, a table's read under terms that filter it, each on an index of its own:
/// a WHERE's two; an inner join's ON beside a WHERE's; the ON of the LEFT JOIN the table is the right one
/// of; a WHERE's two beside another table's LEFT JOIN, whose ON holds a term on the table's third index,
/// alone and over an inner join; and an OR in a WHERE, in an inner join's ON and in the table's own LEFT
/// JOIN's. Expect: the multi-index reads those terms had, and nothing read by the ON term of another
/// table's LEFT JOIN.
#[test]
fn the_terms_that_filter_a_table_keep_its_multi_index_reads() {
    let conn = open(
        &[
            "CREATE TABLE a(id INTEGER PRIMARY KEY, total REAL, code TEXT UNIQUE, k INT)",
            "CREATE INDEX a_total ON a(total)",
            "CREATE INDEX a_k ON a(k)",
            "CREATE TABLE b(id INTEGER PRIMARY KEY, v INT)",
        ],
        false,
        false,
    );
    for (sql, read) in [
        (
            "SELECT a.id FROM a WHERE a.total = 100 AND a.k = 1",
            "MULTI-INDEX AND a (a_total, a_k)",
        ),
        (
            "SELECT a.id, b.id FROM a JOIN b ON a.total = 100 WHERE a.k = 1",
            "MULTI-INDEX AND a (a_total, a_k)",
        ),
        (
            "SELECT a.id, b.id FROM b LEFT JOIN a ON a.total = 100 AND a.k = 1",
            "MULTI-INDEX AND a (a_total, a_k)",
        ),
        (
            "SELECT a.id, b.id FROM a LEFT JOIN b ON b.id = a.id WHERE a.total = 100 AND a.k = 1",
            "MULTI-INDEX AND a (a_total, a_k)",
        ),
        (
            "SELECT a.id, b.id FROM a LEFT JOIN b ON a.code = '1' AND b.id = a.id WHERE a.total = 100 AND a.k = 1",
            "MULTI-INDEX AND a (a_total, a_k)",
        ),
        (
            "SELECT a.id, b.id FROM a JOIN b ON b.id = a.id LEFT JOIN b AS c ON a.code = '1' WHERE a.total = 100 AND a.k = 1",
            "MULTI-INDEX AND a (a_total, a_k)",
        ),
        (
            "SELECT a.id FROM a WHERE a.total = 100 OR a.code = '1'",
            "MULTI-INDEX OR a (a_total, sqlite_autoindex_a_1)",
        ),
        (
            "SELECT a.id, b.id FROM a JOIN b ON (a.total = 100 OR a.code = '1')",
            "MULTI-INDEX OR a (a_total, sqlite_autoindex_a_1)",
        ),
        (
            "SELECT a.id, b.id FROM b LEFT JOIN a ON (a.total = 100 OR a.code = '1')",
            "MULTI-INDEX OR a (a_total, sqlite_autoindex_a_1)",
        ),
    ] {
        let plan = plan(&conn, sql);
        assert!(plan.contains(read), "{sql}: {plan}");
    }
}
