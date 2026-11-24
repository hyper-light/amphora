//! AC-T1.8.6: queries Amphora refused and SQLite 3.54.0 answers: a compound SELECT's ORDER BY term in
//! parentheses, beneath a COLLATE or a sign, and a subquery in a window function's argument (the
//! oracles' refusals and findings CR and CS; bench/records/2026-11-24-adacachy2-perf-oracle-c11.md).

use amphora_engine::morsel::ParallelMode;
use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, Numeric, OpenOptions, SqliteDialect,
    Value,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// The settings a user meets a query under: the parallel mode, and whether the database has
/// statistics.
const SETTINGS: [(&str, bool, bool); 3] = [
    ("default", false, false),
    ("parallel", true, false),
    ("statistics", false, true),
];

fn open(parallel: bool) -> Arc<Connection> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let opts = DatabaseOpts::new().with_without_rowid(true);
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(opts);
    let name = format!("refused_queries_{}.db", N.fetch_add(1, Ordering::Relaxed));
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

fn rows(conn: &Arc<Connection>, sql: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare(sql).map_err(|error| error.to_string())?;
    Ok(stmt
        .run_collect_rows()
        .map_err(|error| error.to_string())?
        .iter()
        .map(|row| row.iter().map(cell).collect::<Vec<_>>().join(","))
        .collect())
}

/// Do: run `setup`, then each query of `queries` under each setting. Expect: SQLite's rows, in its
/// order, or, where it refuses, that text in the refusal.
fn answers_as_sqlite(setup: &[&str], queries: &[(&str, &str)]) {
    for (setting, parallel, statistics) in SETTINGS {
        let conn = open(parallel);
        for sql in setup {
            conn.execute(sql).unwrap();
        }
        if statistics {
            conn.execute("ANALYZE").unwrap();
        }
        for (sql, want) in queries {
            let (same, got) = match rows(&conn, sql) {
                Ok(rows) => (rows.join(" / ") == *want, rows.join(" / ")),
                Err(error) => (error.contains(want) && !want.is_empty(), error),
            };
            assert!(same, "{setting}: {sql}: got {got}, SQLite {want}");
        }
    }
}

/// Do: sort a compound SELECT by its first column's position, parenthesized, beneath a COLLATE and
/// beneath signs. Expect: SQLite's order, the term's outermost COLLATE's, and its refusals. The
/// parenthesized COLLATE was no column (62,216 refusals of the oracles' run).
#[test]
fn a_compound_sorts_by_a_position_in_parentheses() {
    let sort = |term: &str| format!("SELECT a FROM t UNION SELECT 'C' ORDER BY {term}");
    let nocase = "'A' / 'b' / 'C'";
    let binary = "'A' / 'C' / 'b'";
    let terms = [
        ("(1 COLLATE NOCASE)", nocase),
        ("((1) COLLATE NOCASE)", nocase),
        ("(1) COLLATE NOCASE", nocase),
        ("(1 COLLATE BINARY) COLLATE NOCASE", nocase),
        ("(1 COLLATE NOCASE) COLLATE BINARY", binary),
        ("(a COLLATE NOCASE)", nocase),
        ("((a)) COLLATE NOCASE", nocase),
        ("((1))", binary),
        ("(+1)", binary),
        ("- -1", binary),
        ("-(-1)", binary),
        ("(0x1)", binary),
        ("(1 COLLATE NOCASE) DESC", "'C' / 'b' / 'A'"),
        (
            "(0)",
            "1st ORDER BY term out of range - should be between 1 and 1",
        ),
        (
            "-(1)",
            "1st ORDER BY term out of range - should be between 1 and 1",
        ),
        (
            "1, (3 COLLATE NOCASE)",
            "2nd ORDER BY term out of range - should be between 1 and 1",
        ),
        (
            "+(1 COLLATE NOCASE)",
            "1st ORDER BY term does not match any column in the result set",
        ),
        (
            "(1.0)",
            "1st ORDER BY term does not match any column in the result set",
        ),
    ];
    let queries: Vec<(String, &str)> = terms.map(|(term, want)| (sort(term), want)).to_vec();
    let queries: Vec<(&str, &str)> = queries
        .iter()
        .map(|(q, want)| (q.as_str(), *want))
        .collect();
    answers_as_sqlite(
        &[
            "CREATE TABLE t(a TEXT)",
            "INSERT INTO t VALUES ('b'), ('A')",
        ],
        &queries,
    );
}

/// Do: a subquery in a window function's argument, alone and in an expression, correlated and not.
/// Expect: SQLite's rows, and its refusal of a subquery of two columns (findings CR and CS, refused).
#[test]
fn a_windows_argument_holds_a_subquery() {
    answers_as_sqlite(
        &[
            "CREATE TABLE t(a INT, g INT)",
            "CREATE TABLE s(b INT)",
            "INSERT INTO t VALUES (1, 1), (2, 1), (3, 2)",
            "INSERT INTO s VALUES (2), (3)",
        ],
        &[
            (
                "SELECT a, sum((SELECT max(b) FROM s)) OVER (ORDER BY a) FROM t",
                "1,3 / 2,6 / 3,9",
            ),
            (
                "SELECT a, sum(CASE WHEN a IN (SELECT b FROM s) THEN 1 ELSE 0 END) OVER (ORDER BY a) FROM t",
                "1,0 / 2,1 / 3,2",
            ),
            (
                "SELECT a, sum((SELECT max(b) FROM s WHERE b > t.a)) OVER (ORDER BY a) FROM t",
                "1,3 / 2,6 / 3,6",
            ),
            (
                "SELECT a, lag(a, 1, (SELECT min(b) FROM s)) OVER (ORDER BY a) FROM t",
                "1,2 / 2,1 / 3,2",
            ),
            (
                "SELECT a, json_group_array((SELECT max(b) FROM s WHERE b > t.a)) OVER (ORDER BY a) FROM t",
                "1,'[3]' / 2,'[3,3]' / 3,'[3,3,null]'",
            ),
            (
                "SELECT g, sum(sum((SELECT max(b) FROM s WHERE b <> t.a))) OVER (ORDER BY g) FROM t GROUP BY g",
                "1,6 / 2,8",
            ),
            ("SELECT sum((SELECT 1, 2)) OVER () FROM t", "row value misused"),
            (
                "SELECT sum(a IN (SELECT b, 1 FROM s)) OVER () FROM t",
                "sub-select returns 2 columns - expected 1",
            ),
        ],
    );
}

/// One line of a case: what it is (a statement, a query, a query whose order counts), its SQL, and
/// SQLite's answer.
type Line = (&'static str, &'static str, &'static str);

fn cases() -> Vec<Vec<Line>> {
    let mut cases: Vec<Vec<Line>> = Vec::new();
    for line in include_str!("fixtures/refused_queries.tsv").lines() {
        match line.split('\t').collect::<Vec<_>>().as_slice() {
            ["case"] => cases.push(Vec::new()),
            ["sql", sql] => cases.last_mut().unwrap().push(("sql", sql, "ok")),
            [kind @ ("rows" | "ordered"), sql, want] => {
                cases.last_mut().unwrap().push((*kind, *sql, *want));
            }
            _ => assert!(line.starts_with('#'), "{line}"),
        }
    }
    cases
}

/// What Amphora answers to the last line of `case`, beside whether it is SQLite's answer.
fn answer(case: &[Line], parallel: bool, statistics: bool) -> (String, bool) {
    let conn = open(parallel);
    let Some(((kind, sql, want), setup)) = case.split_last() else {
        return ("no query".to_owned(), false);
    };
    for (_, sql, _) in setup {
        conn.execute(sql).unwrap();
    }
    if statistics {
        conn.execute("ANALYZE").unwrap();
    }
    let got = match rows(&conn, sql) {
        Ok(mut rows) => {
            if *kind == "rows" {
                rows.sort();
            }
            rows.join(" / ")
        }
        Err(error) => format!("refused: {error}"),
    };
    let same = got == *want || got.starts_with("refused: ") && want.starts_with("refused: ");
    (got, same)
}

/// Do: run the fixture's cases of `part` of `PARTS` under each setting. Expect: SQLite's rows, or a
/// refusal where it refuses.
fn queries_answer_as_sqlite(part: usize) {
    const PARTS: usize = 4;
    let (mut ran, mut wrong, mut answered) = (0, 0, 0);
    for (index, case) in cases()
        .iter()
        .enumerate()
        .filter(|(i, _)| i % PARTS == part)
    {
        for (setting, parallel, statistics) in SETTINGS {
            ran += 1;
            let got = std::panic::catch_unwind(|| answer(case, parallel, statistics));
            let (got, same) = got.unwrap_or_else(|_| ("PANIC".to_owned(), false));
            answered += usize::from(same && !got.starts_with("refused: "));
            if !same {
                wrong += 1;
                let sql: Vec<_> = case.iter().map(|(_, sql, _)| format!("  {sql}")).collect();
                let want = case.last().map_or("", |(_, _, want)| *want);
                eprintln!(
                    "WRONG case {index}, {setting}:\n{}\n    SQLite  {want}\n    Amphora {got}",
                    sql.join("\n")
                );
            }
        }
    }
    eprintln!("{ran} runs, {wrong} wrong, {answered} answered");
    assert!(answered * 2 > ran, "{answered} of {ran} runs answered");
    assert_eq!(wrong, 0, "{wrong} of {ran} runs answer otherwise");
}

#[test]
fn queries_answer_as_sqlite_0() {
    queries_answer_as_sqlite(0);
}

#[test]
fn queries_answer_as_sqlite_1() {
    queries_answer_as_sqlite(1);
}

#[test]
fn queries_answer_as_sqlite_2() {
    queries_answer_as_sqlite(2);
}

#[test]
fn queries_answer_as_sqlite_3() {
    queries_answer_as_sqlite(3);
}
