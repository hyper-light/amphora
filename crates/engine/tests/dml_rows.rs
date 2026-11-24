//! AC-T1.8.6: the rows INSERT, UPSERT, REPLACE, UPDATE and DELETE store, and the NEW and OLD values
//! their triggers see, as SQLite 3.54.0 has them: `excluded.*` for every column, the rowid and its
//! aliases, STRICT or not, with a rowid or without, generated columns among them (the oracles' findings
//! DA and CV, lane C14), and what Amphora refused of them: STRICT beside WITHOUT ROWID, triggers on
//! WITHOUT ROWID tables, OR IGNORE of a NULL in a STRICT table's key, a subquery in DO UPDATE (lane C19).
//! T-T1.7.1: every record read back as written.

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

/// Do: run `statements` on a fresh database under each setting. Expect: each answers as SQLite does,
/// whose answer stands beside it: a query's rows, "ok", or the refusal's text.
fn answers_as_sqlite(statements: &[(&str, &str)]) {
    for (setting, parallel, statistics) in SETTINGS {
        let conn = open(parallel);
        let mut analyzed = false;
        for (sql, want) in statements {
            let got = match rows(&conn, sql) {
                Ok(rows) if sql.starts_with("SELECT") => rows.join(" / "),
                Ok(_) => "ok".to_owned(),
                Err(error) => error,
            };
            let same = got == *want || *want != "ok" && !want.is_empty() && got.contains(want);
            assert!(same, "{setting}: {sql}: got {got}, SQLite {want}");
            if statistics && !analyzed && sql.starts_with("INSERT") {
                analyzed = true;
                conn.execute("ANALYZE").unwrap();
            }
        }
    }
}

/// Do: UPSERTs that read `excluded.<INTEGER PRIMARY KEY>` in SET and WHERE, on a STRICT table and on
/// one that is not, the key listed in the INSERT, given as NULL, as text, by the rowid's name, or not
/// given. Expect: SQLite's rows, where it is the key the row would take. It was NULL on a STRICT table,
/// whose copy of the row to insert holds the NULL its record stores there, and NULL on any table whose
/// INSERT doesn't list it (finding DA: same-569e139fa912bea6, same-ab15d9e824b1c576).
#[test]
fn excluded_rowid_alias_is_the_key_the_row_would_take() {
    for strict in [" STRICT", ""] {
        answers_as_sqlite(&[
            (
                &format!("CREATE TABLE item(id INTEGER PRIMARY KEY, qty INT, last INT){strict}"),
                "ok",
            ),
            ("INSERT INTO item VALUES (10, 1, 0)", "ok"),
            ("INSERT INTO item VALUES (10, 5, 0) ON CONFLICT(id) DO UPDATE SET qty = item.qty + excluded.qty, last = excluded.id", "ok"),
            ("SELECT * FROM item", "10,6,10"),
            ("INSERT INTO item VALUES (10, 5, 0) ON CONFLICT(id) DO UPDATE SET qty = excluded.qty WHERE excluded.id = 10", "ok"),
            ("SELECT * FROM item", "10,5,10"),
            ("INSERT INTO item VALUES (' 10 ', 5, 0) ON CONFLICT(id) DO UPDATE SET id = excluded.id + 1, last = excluded.rowid", "ok"),
            ("SELECT * FROM item", "11,5,10"),
        ]);
        answers_as_sqlite(&[
            (
                &format!("CREATE TABLE kv(id INTEGER PRIMARY KEY, name TEXT UNIQUE, n INT){strict}"),
                "ok",
            ),
            ("INSERT INTO kv VALUES (1, 'a', 0)", "ok"),
            ("INSERT INTO kv(name, n) VALUES ('a', 5) ON CONFLICT(name) DO UPDATE SET n = excluded.id", "ok"),
            ("SELECT * FROM kv", "1,'a',2"),
            ("INSERT INTO kv VALUES (NULL, 'a', 5) ON CONFLICT(name) DO UPDATE SET n = excluded.id + 10", "ok"),
            ("SELECT * FROM kv", "1,'a',12"),
            ("INSERT INTO kv(_rowid_, name) VALUES (9, 'a') ON CONFLICT(name) DO UPDATE SET n = excluded.id", "ok"),
            ("SELECT * FROM kv", "1,'a',9"),
            ("INSERT INTO kv VALUES (7, 'a', 5) ON CONFLICT(name) DO UPDATE SET id = excluded.id, n = excluded.n", "ok"),
            ("SELECT * FROM kv", "7,'a',5"),
        ]);
    }
}

/// Do: INSERT and UPDATE values that their columns convert, on a STRICT table whose BEFORE triggers log
/// NEW, a generated column among them. Expect: the trigger sees the converted values, as SQLite's does:
/// the type check that converts ran after the triggers (finding CV: same-575728fb9efb3c2c,
/// same-d8a85a33a5c7fde1).
#[test]
fn a_before_trigger_on_a_strict_table_sees_converted_values() {
    answers_as_sqlite(&[
        ("CREATE TABLE price(id INTEGER PRIMARY KEY, amount REAL, n INT, name TEXT, g TEXT AS (typeof(n)) VIRTUAL) STRICT", "ok"),
        ("CREATE TABLE audit(id, amount, n, name, g)", "ok"),
        ("CREATE TRIGGER b BEFORE INSERT ON price BEGIN INSERT INTO audit VALUES (new.id, new.amount, new.n, new.name, new.g); END", "ok"),
        ("INSERT INTO price(id, amount, n, name) VALUES (' 1 ', 100, '7', 2.5)", "ok"),
        ("INSERT INTO price(amount, n) VALUES (3, 1.0), ('4', NULL)", "ok"),
        ("SELECT * FROM audit", "-1,r4008000000000000,1,NULL,'integer' / -1,r4010000000000000,NULL,NULL,'null' / 1,r4059000000000000,7,'2.5','integer'"),
        ("SELECT * FROM price", "1,r4059000000000000,7,'2.5','integer' / 2,r4008000000000000,1,NULL,'integer' / 3,r4010000000000000,NULL,NULL,'null'"),
    ]);
    answers_as_sqlite(&[
        ("CREATE TABLE n(id INTEGER PRIMARY KEY, v INTEGER, w REAL) STRICT", "ok"),
        ("CREATE TABLE audit(i, v, w)", "ok"),
        ("CREATE TRIGGER b BEFORE UPDATE ON n BEGIN INSERT INTO audit VALUES (new.id, new.v, new.w); END", "ok"),
        ("INSERT INTO n VALUES (1, 2, 3)", "ok"),
        ("UPDATE n SET v = '7' WHERE id = 1", "ok"),
        ("UPDATE n SET w = 8, id = '5'", "ok"),
        ("SELECT * FROM audit", "1,7,r4008000000000000 / 5,7,r4020000000000000"),
        ("SELECT * FROM n", "5,7,r4020000000000000"),
    ]);
}

/// Do: INSERT a rowid as text and as a REAL beside a BEFORE trigger that logs it, on a table that is
/// not STRICT. Expect: SQLite's NEW: the integer it is stored as, -1 where none is given yet, and a
/// generated column computed from that.
#[test]
fn a_before_trigger_sees_the_rowid_as_an_integer() {
    answers_as_sqlite(&[
        ("CREATE TABLE p(id INTEGER PRIMARY KEY, g INT AS (id + 1) VIRTUAL, c)", "ok"),
        ("CREATE TABLE audit(id, t, g, r)", "ok"),
        ("CREATE TRIGGER b BEFORE INSERT ON p BEGIN INSERT INTO audit VALUES (new.id, typeof(new.id), new.g, new.rowid); END", "ok"),
        ("INSERT INTO p(id, c) VALUES ('10', 1), (3.0, 2), (NULL, 3)", "ok"),
        ("INSERT INTO p(c) VALUES (4)", "ok"),
        ("INSERT INTO p(id, c) VALUES ('x', 5)", "datatype mismatch"),
        ("INSERT INTO p(id, c) VALUES (' 20 ', 6)", "ok"),
        ("SELECT * FROM audit", "-1,'integer',0,-1 / -1,'integer',0,-1 / 10,'integer',11,10 / 20,'integer',21,20 / 3,'integer',4,3"),
        ("SELECT * FROM p", "10,11,1 / 11,12,3 / 12,13,4 / 20,21,6 / 3,4,2"),
    ]);
}

/// Do: statements a BEFORE trigger ignores whose values a STRICT table refuses: a wrong type, and a
/// NULL key. Expect: the type is refused before the trigger runs and the NULL after it, as SQLite does.
#[test]
fn a_strict_tables_types_are_checked_before_its_triggers_and_not_null_after() {
    answers_as_sqlite(&[
        ("CREATE TABLE k(id TEXT PRIMARY KEY, v INT) STRICT", "ok"),
        (
            "CREATE TRIGGER b BEFORE INSERT ON k WHEN new.v = 0 BEGIN SELECT RAISE(IGNORE); END",
            "ok",
        ),
        (
            "CREATE TRIGGER u BEFORE UPDATE ON k WHEN new.v = 0 BEGIN SELECT RAISE(IGNORE); END",
            "ok",
        ),
        ("INSERT INTO k VALUES (NULL, 0)", "ok"),
        ("INSERT INTO k VALUES ('a', 1)", "ok"),
        (
            "INSERT INTO k VALUES (x'41', 0)",
            "cannot store BLOB value in TEXT column k.id",
        ),
        (
            "INSERT INTO k VALUES (NULL, 1)",
            "NOT NULL constraint failed: k.id",
        ),
        ("UPDATE k SET id = NULL, v = 0", "ok"),
        (
            "UPDATE k SET id = x'41', v = 0",
            "cannot store BLOB value in TEXT column k.id",
        ),
        ("UPDATE k SET id = NULL", "NOT NULL constraint failed: k.id"),
        ("SELECT * FROM k", "'a',1"),
    ]);
}

/// Do: an UPSERT that sets a REAL column to an integer, with a generated column, its index and an AFTER
/// trigger reading it. Expect: all three have what SQLite has, computed from the REAL that is stored:
/// the affinity was applied after the generated column was computed.
#[test]
fn do_update_computes_generated_columns_from_converted_values() {
    answers_as_sqlite(&[
        (
            "CREATE TABLE d(id INTEGER PRIMARY KEY, c REAL, g TEXT AS (c || 'z') VIRTUAL)",
            "ok",
        ),
        ("CREATE INDEX i ON d(g)", "ok"),
        ("CREATE TABLE audit(g)", "ok"),
        (
            "CREATE TRIGGER a AFTER UPDATE ON d BEGIN INSERT INTO audit VALUES (new.g); END",
            "ok",
        ),
        ("INSERT INTO d(id, c) VALUES (1, 1)", "ok"),
        (
            "INSERT INTO d(id, c) VALUES (1, 5) ON CONFLICT(id) DO UPDATE SET c = 6",
            "ok",
        ),
        ("SELECT * FROM d", "1,r4018000000000000,'6.0z'"),
        ("SELECT * FROM audit", "'6.0z'"),
        ("SELECT id FROM d WHERE g = '6.0z'", "1"),
        ("SELECT * FROM pragma_integrity_check", "'ok'"),
    ]);
}

/// Do: compare excluded values with literals and with the row's columns, STRICT or not. Expect: SQLite's
/// answer, the same on both: an excluded value has no affinity and no collation of its own.
#[test]
fn an_excluded_value_is_compared_alike_on_a_strict_table() {
    for strict in [" STRICT", ""] {
        answers_as_sqlite(&[
            (
                &format!("CREATE TABLE e(id INTEGER PRIMARY KEY, c TEXT COLLATE NOCASE, n INT, f INT, u INT UNIQUE){strict}"),
                "ok",
            ),
            ("INSERT INTO e VALUES (1, 'abc', 5, NULL, 1)", "ok"),
            ("INSERT INTO e VALUES (2, 'ABC', 5, 0, 1) ON CONFLICT(u) DO UPDATE SET f = (excluded.c = 'abc') * 10 + (excluded.n = '5')", "ok"),
            ("SELECT f FROM e", "0"),
            ("INSERT INTO e VALUES (2, 'ABC', 5, 0, 1) ON CONFLICT(u) DO UPDATE SET f = (excluded.c = e.c) * 10 + (e.n = '5')", "ok"),
            ("SELECT f FROM e", "11"),
            ("INSERT INTO e VALUES (2, '15', 5, 0, 1) ON CONFLICT(u) DO UPDATE SET f = excluded.c < 10", "ok"),
            ("SELECT f FROM e", "0"),
        ]);
    }
}

/// Do: UPSERTs whose DO UPDATE names no column, bare, bracketed and double-quoted. Expect: refused as
/// SQLite refuses them, but for the double-quoted name, a string. A bare name was stored as its text.
#[test]
fn do_update_refuses_a_name_that_is_no_column() {
    answers_as_sqlite(&[
        ("CREATE TABLE t(id INTEGER PRIMARY KEY, c TEXT)", "ok"),
        ("INSERT INTO t VALUES (1, 'a')", "ok"),
        (
            "INSERT INTO t VALUES (1, 'b') ON CONFLICT(id) DO UPDATE SET c = nosuch",
            "no such column: nosuch",
        ),
        (
            "INSERT INTO t VALUES (1, 'b') ON CONFLICT(id) DO UPDATE SET c = [nosuch]",
            "no such column: nosuch",
        ),
        (
            "INSERT INTO t VALUES (1, 'b') ON CONFLICT(id) DO UPDATE SET c = 'x' WHERE nosuch = 1",
            "no such column: nosuch",
        ),
        ("SELECT * FROM t", "1,'a'"),
        (
            "INSERT INTO t VALUES (1, 'b') ON CONFLICT(id) DO UPDATE SET c = \"nosuch\"",
            "ok",
        ),
        ("SELECT * FROM t", "1,'nosuch'"),
        (
            "INSERT INTO t VALUES (1, 'b') ON CONFLICT(id) DO UPDATE SET c = true",
            "ok",
        ),
        ("SELECT * FROM t", "1,'1'"),
    ]);
}

/// Do: UPSERTs and a buffered INSERT on tables with columns named oid and rowid, and UPSERTs that name
/// the rowid of a table that has none. Expect: SQLite's rows: such a column is itself, and what isn't
/// there is refused. `excluded.oid` was the key, and a buffered INSERT made the column's value an integer.
#[test]
fn a_column_named_as_the_rowid_is_that_column() {
    answers_as_sqlite(&[
        ("CREATE TABLE o(id INTEGER PRIMARY KEY, oid TEXT, rowid REAL, m INT) STRICT", "ok"),
        ("INSERT INTO o VALUES (1, 'x', 2, 0)", "ok"),
        ("INSERT INTO o VALUES (1, 'y', 3, 0) ON CONFLICT(id) DO UPDATE SET oid = oid || excluded.oid, rowid = rowid + excluded.rowid, m = excluded._rowid_ + o._rowid_", "ok"),
        ("SELECT id, oid, rowid, m FROM o", "1,'xy',r4014000000000000,2"),
    ]);
    answers_as_sqlite(&[
        ("CREATE TABLE o(id INTEGER PRIMARY KEY, oid TEXT)", "ok"),
        (
            "CREATE TRIGGER a AFTER INSERT ON o BEGIN SELECT 1; END",
            "ok",
        ),
        ("INSERT INTO o(id, oid) VALUES (1, 1.0), (2, 2.0)", "ok"),
        ("SELECT id, oid FROM o", "1,'1.0' / 2,'2.0'"),
    ]);
    answers_as_sqlite(&[
        (
            "CREATE TABLE w(id INT PRIMARY KEY, c INT) WITHOUT ROWID",
            "ok",
        ),
        ("INSERT INTO w VALUES (1, 1)", "ok"),
        (
            "INSERT INTO w VALUES (1, 2) ON CONFLICT(id) DO UPDATE SET c = excluded.rowid",
            "no such column",
        ),
        (
            "INSERT INTO w VALUES (1, 2) ON CONFLICT(id) DO UPDATE SET c = rowid",
            "no such column",
        ),
        (
            "INSERT INTO w VALUES (1, 2) ON CONFLICT(id) DO UPDATE SET c = excluded.id + 10",
            "ok",
        ),
        ("SELECT * FROM w", "1,11"),
    ]);
}

/// Do: create a table STRICT and WITHOUT ROWID, in both orders, and write to it. Expect: SQLite's types, its NULL key refused, and no rowid. Its options were written to the schema
/// with no comma between them, and the schema refused: 7,701 of the oracles' setup refusals.
#[test]
fn a_table_is_strict_and_without_rowid() {
    for options in [
        "STRICT, WITHOUT ROWID",
        "WITHOUT ROWID, STRICT",
        "strict,without rowid",
    ] {
        answers_as_sqlite(&[
            (
                &format!("CREATE TABLE t(k TEXT PRIMARY KEY, n INT, r REAL) {options}"),
                "ok",
            ),
            ("INSERT INTO t VALUES (7, '8', 9), ('a', 1, NULL)", "ok"),
            (
                "INSERT INTO t VALUES ('a', 1, 1) ON CONFLICT(k) DO UPDATE SET r = excluded.n",
                "ok",
            ),
            ("UPDATE t SET n = n + 1, k = k || 'z'", "ok"),
            ("SELECT * FROM pragma_integrity_check", "'ok'"),
            (
                "INSERT INTO t VALUES ('b', 'x', 1)",
                "cannot store TEXT value in INT column t.n",
            ),
            (
                "INSERT INTO t VALUES (NULL, 1, 1)",
                "NOT NULL constraint failed: t.k",
            ),
            (
                "INSERT INTO t VALUES ('az', 1, 1)",
                "UNIQUE constraint failed: t.k",
            ),
            ("SELECT rowid FROM t", "no such column: rowid"),
            (
                "SELECT * FROM t",
                "'7z',9,r4022000000000000 / 'az',2,r3ff0000000000000",
            ),
        ]);
    }
}

/// Do: OR IGNORE, OR REPLACE and OR ABORT of a NULL in a STRICT table's key and in a NOT NULL column, on
/// INSERT and UPDATE, one beside a value of a type its column refuses. Expect: SQLite's rows: a STRICT
/// table's key is NOT NULL as any column is (stricttables.html), asked before the types are, and the
/// default put in for a NULL is converted. A NULL key was refused whatever the OR clause, and the
/// default was stored as typed.
#[test]
fn a_strict_tables_key_is_not_null_as_any_column_is() {
    answers_as_sqlite(&[
        (
            "CREATE TABLE t(k TEXT PRIMARY KEY, n INT NOT NULL DEFAULT '7') STRICT",
            "ok",
        ),
        (
            "INSERT OR IGNORE INTO t VALUES ('a', 1), (NULL, 2), ('b', NULL), (NULL, 'x'), ('c', 3)",
            "ok",
        ),
        ("SELECT * FROM t", "'a',1 / 'c',3"),
        ("INSERT INTO t VALUES (NULL, 2)", "NOT NULL constraint failed: t.k"),
        ("INSERT OR REPLACE INTO t VALUES (NULL, 2)", "NOT NULL constraint failed: t.k"),
        ("INSERT OR REPLACE INTO t VALUES ('b', NULL)", "ok"),
        ("UPDATE OR IGNORE t SET k = NULL WHERE k = 'a'", "ok"),
        ("UPDATE t SET k = NULL", "NOT NULL constraint failed: t.k"),
        ("SELECT k, n, typeof(n) FROM t", "'a',1,'integer' / 'b',7,'integer' / 'c',3,'integer'"),
        ("SELECT name, \"notnull\" FROM pragma_table_info('t')", "'k',1 / 'n',1"),
    ]);
    answers_as_sqlite(&[
        (
            "CREATE TABLE t(a INT DEFAULT 5, b TEXT, PRIMARY KEY(a, b)) STRICT",
            "ok",
        ),
        (
            "INSERT OR IGNORE INTO t VALUES (1, NULL), (NULL, 'x'), (2, 'y')",
            "ok",
        ),
        ("INSERT OR REPLACE INTO t VALUES (NULL, 'z')", "ok"),
        ("SELECT * FROM t", "2,'y' / 5,'z'"),
        (
            "CREATE TABLE i(a INTEGER PRIMARY KEY, b) STRICT",
            "missing datatype for i.b",
        ),
        ("CREATE TABLE i(a INTEGER PRIMARY KEY, b ANY) STRICT", "ok"),
        ("INSERT INTO i VALUES (NULL, 1)", "ok"),
        ("SELECT * FROM i", "1,1"),
    ]);
}

/// Do: subqueries in DO UPDATE's SET and WHERE, reading excluded, the conflicting row, the target table
/// and a CTE, on several rows, with a rowid and without. Expect: SQLite's rows: each was refused
/// ("Subquery is not supported in this position").
#[test]
fn do_update_holds_subqueries() {
    for options in ["", " STRICT", " WITHOUT ROWID", " STRICT, WITHOUT ROWID"] {
        answers_as_sqlite(&[
            (
                &format!("CREATE TABLE kv(id INTEGER PRIMARY KEY, n INT, m INT){options}"),
                "ok",
            ),
            ("CREATE TABLE s(b INT)", "ok"),
            ("INSERT INTO s VALUES (2), (30)", "ok"),
            ("INSERT INTO kv VALUES (1, 5, 0), (2, 6, 0)", "ok"),
            ("INSERT INTO kv VALUES (1, 7, 1), (2, 70, 2) ON CONFLICT(id) DO UPDATE SET n = (SELECT excluded.n + 1), m = (SELECT max(b) FROM s WHERE b > excluded.n)", "ok"),
            ("SELECT * FROM kv", "1,8,30 / 2,71,NULL"),
            ("INSERT INTO kv VALUES (1, 7, 1), (2, 70, 2) ON CONFLICT(id) DO UPDATE SET (n, m) = (SELECT kv.n + excluded.n, count(*) FROM s WHERE s.b < kv.n)", "ok"),
            ("SELECT * FROM kv", "1,15,1 / 2,141,2"),
            ("INSERT INTO kv VALUES (1, 1, 1), (2, 2, 2) ON CONFLICT(id) DO UPDATE SET n = excluded.n IN (SELECT b FROM s) WHERE EXISTS (SELECT 1 FROM s WHERE b = excluded.m)", "ok"),
            ("SELECT * FROM kv", "1,15,1 / 2,1,2"),
            ("INSERT INTO kv VALUES (1, 1, 1), (2, 2, 2) ON CONFLICT(id) DO UPDATE SET n = (SELECT sum(n) FROM kv), m = (SELECT count(*) FROM kv x WHERE x.n < kv.n)", "ok"),
            ("SELECT * FROM kv", "1,16,1 / 2,16,0"),
            ("INSERT INTO kv AS q VALUES (1, 1, 1) ON CONFLICT(id) DO UPDATE SET n = (SELECT q.n + excluded.id FROM s LIMIT 1)", "ok"),
            ("WITH c(x) AS (SELECT 100) INSERT INTO kv SELECT 2, 1, 1 WHERE true ON CONFLICT(id) DO UPDATE SET n = (SELECT x FROM c)", "ok"),
            ("SELECT * FROM kv", "1,17,1 / 2,100,0"),
            ("INSERT INTO kv VALUES (1, 9, 1), (2, 10, 1) ON CONFLICT(id) DO UPDATE SET n = (SELECT t.y FROM (SELECT excluded.n AS y) AS t), m = (SELECT b FROM s AS excluded WHERE excluded.b = 30)", "ok"),
            ("SELECT * FROM kv", "1,9,30 / 2,10,30"),
            ("INSERT INTO kv VALUES (1, 1, 1) ON CONFLICT(id) DO UPDATE SET n = (SELECT excluded.nosuch)", "no such column: excluded.nosuch"),
            ("INSERT INTO kv VALUES (1, 1, 1) ON CONFLICT(id) DO UPDATE SET (n, m) = (SELECT 1)", "2 columns assigned 1 values"),
        ]);
    }
}

/// One line of a case: what it is (a CREATE, a statement, a query), its SQL, and SQLite's answer.
type Line = (&'static str, &'static str, &'static str);

fn cases() -> Vec<Vec<Line>> {
    let mut cases: Vec<Vec<Line>> = Vec::new();
    for line in include_str!("fixtures/dml_rows.tsv").lines() {
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
