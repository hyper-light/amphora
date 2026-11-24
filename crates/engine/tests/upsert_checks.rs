//! AC-T1.8.6: what SQLite 3.54.0 checks an UPSERT's new row for, in its order: NOT NULL and CHECK after the
//! BEFORE UPDATE triggers, on the row they left; and what it answers after a statement a constraint ended
//! (lane C25).

use amphora_engine::morsel::ParallelMode;
use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, MemoryIO, Numeric, OpenOptions, SqliteDialect,
    Value,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// The settings a user meets a statement under: the parallel mode, and whether the database has
/// statistics.
const SETTINGS: [(&str, bool, bool); 4] = [
    ("default", false, false),
    ("parallel", true, false),
    ("statistics", false, true),
    ("both", true, true),
];

fn open(parallel: bool) -> Arc<Connection> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let options = OpenOptions::new(Arc::new(SqliteDialect))
        .db_opts(DatabaseOpts::new().with_without_rowid(true));
    let name = format!("upsert_checks_{}.db", N.fetch_add(1, Ordering::Relaxed));
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

/// What Amphora answers to `sql`, and whether it is `want`: a query's rows, "ok", or "refused: " and the
/// refusal's text.
fn answer(conn: &Arc<Connection>, query: bool, sql: &str, want: &str) -> (String, bool) {
    match rows(conn, sql) {
        Ok(rows) => {
            let got = if query {
                rows.join(" / ")
            } else {
                "ok".to_owned()
            };
            let same = got == want;
            (got, same)
        }
        Err(error) => {
            let same = want
                .strip_prefix("refused: ")
                .is_some_and(|want| error.contains(want));
            (format!("refused: {error}"), same)
        }
    }
}

/// Do: run `statements` on a fresh database under each setting. Expect: each answers as SQLite does,
/// whose answer stands beside it.
fn answers_as_sqlite(statements: &[(&str, &str)]) {
    for (setting, parallel, statistics) in SETTINGS {
        let conn = open(parallel);
        let mut analyzed = false;
        for (sql, want) in statements {
            let (got, same) = answer(&conn, sql.starts_with("SELECT"), sql, want);
            assert!(same, "{setting}: {sql}: got {got}, SQLite {want}");
            if statistics && !analyzed && sql.starts_with("INSERT") {
                analyzed = true;
                conn.execute("ANALYZE").unwrap();
            }
        }
    }
}

/// Do: UPSERTs that set a NULL in a NOT NULL column and a value a CHECK refuses, beside BEFORE UPDATE
/// triggers that RAISE or delete the row. Expect: the trigger's outcome, as SQLite has it: it runs first.
#[test]
fn a_before_update_trigger_runs_before_not_null_and_check() {
    for (key, options) in [
        ("INTEGER PRIMARY KEY", ""),
        ("INT PRIMARY KEY", ""),
        ("TEXT PRIMARY KEY", "WITHOUT ROWID"),
        ("INTEGER PRIMARY KEY", "STRICT"),
    ] {
        let table =
            format!("CREATE TABLE t(id {key}, n INT NOT NULL, c INT CHECK (c > 0)) {options}");
        answers_as_sqlite(&[
            (&table, "ok"),
            ("CREATE TRIGGER b BEFORE UPDATE ON t BEGIN SELECT RAISE(ABORT, 'before'); END", "ok"),
            ("INSERT INTO t VALUES (1, 1, 1)", "ok"),
            ("INSERT INTO t VALUES (1, 5, 5) ON CONFLICT(id) DO UPDATE SET n = NULL", "refused: before"),
            ("INSERT INTO t VALUES (1, 5, 5) ON CONFLICT(id) DO UPDATE SET c = -1", "refused: before"),
            ("DROP TRIGGER b", "ok"),
            ("INSERT INTO t VALUES (1, 5, 5) ON CONFLICT(id) DO UPDATE SET n = NULL", "refused: NOT NULL constraint failed: t.n"),
            ("INSERT INTO t VALUES (1, 5, 5) ON CONFLICT(id) DO UPDATE SET c = -1", "refused: CHECK constraint failed: c > 0"),
            ("CREATE TABLE lg(n, c)", "ok"),
            ("CREATE TRIGGER b BEFORE UPDATE ON t BEGIN INSERT INTO lg VALUES (new.n, new.c); SELECT RAISE(IGNORE); END", "ok"),
            ("INSERT INTO t VALUES (1, 5, 5) ON CONFLICT(id) DO UPDATE SET n = NULL", "ok"),
            ("INSERT INTO t VALUES (1, 5, 5) ON CONFLICT(id) DO UPDATE SET c = -1", "ok"),
            ("SELECT n, c FROM t", "1,1"),
            ("SELECT * FROM lg", "1,-1 / NULL,1"),
            ("DROP TRIGGER b", "ok"),
            ("CREATE TRIGGER b BEFORE UPDATE ON t BEGIN DELETE FROM t WHERE id = old.id; END", "ok"),
            ("INSERT INTO t VALUES (1, 5, 5) ON CONFLICT(id) DO UPDATE SET n = NULL", "ok"),
            ("SELECT n, c FROM t", ""),
            ("SELECT * FROM pragma_integrity_check", "'ok'"),
        ]);
    }
}

/// Do: UPSERTs beside a BEFORE UPDATE trigger that changes a column of the row a CHECK compares with the
/// one set. Expect: SQLite's answer: the CHECK reads what the trigger left, so a row that breaks it is
/// refused, and so is one it made right only in the old values.
#[test]
fn a_check_reads_the_row_the_triggers_left() {
    answers_as_sqlite(&[
        ("CREATE TABLE t(id INTEGER PRIMARY KEY, a INT, b INT, CHECK (a < b))", "ok"),
        ("CREATE TRIGGER g BEFORE UPDATE OF a ON t BEGIN UPDATE t SET b = 13 - b WHERE id = old.id; END", "ok"),
        ("INSERT INTO t VALUES (1, 1, 10)", "ok"),
        ("INSERT INTO t VALUES (1, 1, 1) ON CONFLICT(id) DO UPDATE SET a = 5", "refused: CHECK constraint failed: a < b"),
        ("SELECT * FROM t", "1,1,10"),
        ("INSERT INTO t VALUES (1, 1, 1) ON CONFLICT(id) DO UPDATE SET a = 2", "ok"),
        ("SELECT * FROM t", "1,2,3"),
        ("INSERT INTO t VALUES (1, 1, 1) ON CONFLICT(id) DO UPDATE SET a = 9", "ok"),
        ("SELECT * FROM t", "1,9,10"),
        ("SELECT * FROM pragma_integrity_check", "'ok'"),
    ]);
}

/// Do: UPSERTs that break two constraints at once, set a column twice, and leave alone a column whose
/// stored value breaks its CHECK. Expect: SQLite's refusals: the table's first NOT NULL column; NOT NULL
/// before a STRICT table's types, and after them where a BEFORE UPDATE trigger sees the row; the last
/// assignment alone; no CHECK of columns the UPSERT doesn't set.
#[test]
fn an_upsert_meets_its_constraints_in_sqlites_order() {
    answers_as_sqlite(&[
        (
            "CREATE TABLE t(id INTEGER PRIMARY KEY, a INT NOT NULL, b INT NOT NULL)",
            "ok",
        ),
        ("INSERT INTO t VALUES (1, 1, 10)", "ok"),
        (
            "INSERT INTO t VALUES (1, 1, 1) ON CONFLICT(id) DO UPDATE SET b = NULL, a = NULL",
            "refused: NOT NULL constraint failed: t.a",
        ),
        (
            "INSERT INTO t VALUES (1, 1, 1) ON CONFLICT(id) DO UPDATE SET a = NULL, a = 5",
            "ok",
        ),
        ("SELECT * FROM t", "1,5,10"),
    ]);
    answers_as_sqlite(&[
        (
            "CREATE TABLE t(id INTEGER PRIMARY KEY, a INT NOT NULL, b INT) STRICT",
            "ok",
        ),
        ("INSERT INTO t VALUES (1, 1, 10)", "ok"),
        (
            "INSERT INTO t VALUES (1, 1, 1) ON CONFLICT(id) DO UPDATE SET b = 'x', a = NULL",
            "refused: NOT NULL constraint failed: t.a",
        ),
        (
            "CREATE TRIGGER g BEFORE UPDATE ON t BEGIN SELECT RAISE(ABORT, 'before'); END",
            "ok",
        ),
        (
            "INSERT INTO t VALUES (1, 1, 1) ON CONFLICT(id) DO UPDATE SET b = 'x', a = NULL",
            "refused: cannot store TEXT value in INT column t.b",
        ),
        (
            "INSERT INTO t VALUES (1, 1, 1) ON CONFLICT(id) DO UPDATE SET b = 2, a = NULL",
            "refused: before",
        ),
        ("SELECT * FROM t", "1,1,10"),
    ]);
    answers_as_sqlite(&[
        (
            "CREATE TABLE t(id INTEGER PRIMARY KEY, a INT, b INT CHECK (b > 0))",
            "ok",
        ),
        ("PRAGMA ignore_check_constraints = ON", "ok"),
        ("INSERT INTO t VALUES (1, 1, -10)", "ok"),
        ("PRAGMA ignore_check_constraints = OFF", "ok"),
        (
            "INSERT INTO t VALUES (1, 1, 1) ON CONFLICT(id) DO UPDATE SET a = 5",
            "ok",
        ),
        ("SELECT * FROM t", "1,5,-10"),
        (
            "INSERT INTO t VALUES (1, 1, 1) ON CONFLICT(id) DO UPDATE SET b = b",
            "refused: CHECK constraint failed: b > 0",
        ),
    ]);
}

/// Do: read the pragmas' table functions after statements that a constraint, a type and a trigger ended.
/// Expect: SQLite's rows: the pragma's statement waits where its pages are read anew, and that wait was
/// taken for a row ("No row available").
#[test]
fn a_pragmas_table_function_answers_after_a_refused_statement() {
    for options in ["", "STRICT", "WITHOUT ROWID", "STRICT, WITHOUT ROWID"] {
        let table = format!("CREATE TABLE k(id TEXT PRIMARY KEY, v INT NOT NULL CHECK (v > 0), w INT UNIQUE) {options}");
        let refused = [
            (
                "INSERT INTO k VALUES (NULL, NULL, 1)",
                "refused: NOT NULL constraint failed: k.",
            ),
            (
                "INSERT INTO k VALUES ('a', 1, 1)",
                "refused: UNIQUE constraint failed: k.",
            ),
            (
                "INSERT INTO k VALUES ('b', 1, 1)",
                "refused: UNIQUE constraint failed: k.w",
            ),
            (
                "INSERT INTO k VALUES ('b', -1, 2)",
                "refused: CHECK constraint failed: v > 0",
            ),
            (
                "UPDATE k SET v = NULL",
                "refused: NOT NULL constraint failed: k.v",
            ),
            (
                "UPDATE k SET v = -1",
                "refused: CHECK constraint failed: v > 0",
            ),
            (
                "INSERT INTO k VALUES ('a', 1, 1) ON CONFLICT(id) DO UPDATE SET v = -1",
                "refused: CHECK constraint failed: v > 0",
            ),
            ("DELETE FROM k", "refused: no"),
        ];
        let functions = [
            ("SELECT * FROM pragma_integrity_check", "'ok'"),
            ("SELECT * FROM pragma_quick_check", "'ok'"),
            ("SELECT count(*) FROM pragma_table_info('k')", "3"),
            ("SELECT name FROM pragma_table_info('k') AS i, pragma_integrity_check AS c WHERE i.cid = 1", "'v'"),
            ("SELECT * FROM pragma_foreign_key_check", ""),
        ];
        let mut statements = vec![
            (table.as_str(), "ok"),
            (
                "CREATE TRIGGER g BEFORE DELETE ON k BEGIN SELECT RAISE(ABORT, 'no'); END",
                "ok",
            ),
            ("INSERT INTO k VALUES ('a', 1, 1)", "ok"),
        ];
        for statement in refused {
            for function in functions {
                statements.extend([statement, function, function]);
            }
        }
        answers_as_sqlite(&statements);
    }
}

/// One line of a case: what it is (a CREATE, a statement, a query), its SQL, and SQLite's answer.
type Line = (&'static str, &'static str, &'static str);

fn cases() -> Vec<Vec<Line>> {
    let mut cases: Vec<Vec<Line>> = Vec::new();
    for line in include_str!("fixtures/upsert_checks.tsv").lines() {
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
            let answered = answer(&conn, *kind == "rows", sql, want);
            if statistics && !analyzed && *kind == "do" && sql.starts_with("INSERT INTO t") {
                analyzed = true;
                conn.execute("ANALYZE").unwrap();
            }
            answered
        })
        .collect()
}

/// Do: run the fixture's cases of `part` of `PARTS` under each setting. Expect: what SQLite answers to
/// each statement (done, or the refusal), to each query after a refusal, and the rows it leaves.
fn checks_as_sqlite(part: usize) {
    const PARTS: usize = 8;
    let mut wrong = 0;
    let mut ran = 0;
    for (index, case) in cases()
        .iter()
        .enumerate()
        .filter(|(i, _)| i % PARTS == part)
    {
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
    assert_eq!(wrong, 0, "{wrong} of {ran} runs answer otherwise");
}

#[test]
fn upserts_are_checked_as_sqlite_checks_them_0() {
    checks_as_sqlite(0);
}

#[test]
fn upserts_are_checked_as_sqlite_checks_them_1() {
    checks_as_sqlite(1);
}

#[test]
fn upserts_are_checked_as_sqlite_checks_them_2() {
    checks_as_sqlite(2);
}

#[test]
fn upserts_are_checked_as_sqlite_checks_them_3() {
    checks_as_sqlite(3);
}

#[test]
fn upserts_are_checked_as_sqlite_checks_them_4() {
    checks_as_sqlite(4);
}

#[test]
fn upserts_are_checked_as_sqlite_checks_them_5() {
    checks_as_sqlite(5);
}

#[test]
fn upserts_are_checked_as_sqlite_checks_them_6() {
    checks_as_sqlite(6);
}

#[test]
fn upserts_are_checked_as_sqlite_checks_them_7() {
    checks_as_sqlite(7);
}
