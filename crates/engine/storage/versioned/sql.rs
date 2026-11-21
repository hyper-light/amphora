//! Amphora: SQL end to end on the versioned storage, against the pager's (G7 step 2, `T-NS.2.6`):
//! the same statements through the switch ([`super::switch`]) and without it must answer the same.

use crate::io::{MemoryIO, IO};
use crate::sync::Arc;
use crate::types::Value;
use crate::{ConnectOpts, Connection, Database, SqliteDialect};

fn connect(versioned: bool) -> (Arc<Database>, Arc<Connection>) {
    let io: Arc<dyn IO> = Arc::new(MemoryIO::new());
    let db = Database::open_file(io, ":memory:", Arc::new(SqliteDialect)).unwrap();
    let opts = if versioned {
        ConnectOpts::new().with_versioned_storage()
    } else {
        ConnectOpts::new()
    };
    let conn = db.connect(opts).unwrap();
    (db, conn)
}

fn rows(conn: &Arc<Connection>, sql: &str) -> Vec<Vec<Value>> {
    let mut stmt = conn.prepare(sql).unwrap();
    let mut out = Vec::new();
    stmt.run_with_row_callback(|row| {
        out.push(row.get_values().cloned().collect());
        Ok(())
    })
    .unwrap();
    out
}

fn run(conn: &Arc<Connection>, sql: &str) {
    conn.execute(sql).unwrap();
}

/// The root page `sqlite_schema` names for table `t`.
fn root(conn: &Arc<Connection>, t: &str) -> i64 {
    let found = rows(
        conn,
        &format!("SELECT rootpage FROM sqlite_schema WHERE name = '{t}'"),
    );
    match found.first().and_then(|r| r.first()) {
        Some(Value::Numeric(crate::numeric::Numeric::Integer(n))) => *n,
        other => panic!("no root page for {t}: {other:?}"),
    }
}

/// Do: CREATE TABLE, INSERT (rows past a page among them), SELECT, on both storages. Expect: the
/// same rows, and the versioned run's table on the storage's first tree made after its own three
/// (`switch`'s tree 0, `sqlite_schema` and the overflow tree), so the switch was what answered.
#[test]
fn t_ns_2_6_create_insert_select_answer_alike() {
    let script = [
        "CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT, c REAL)",
        "INSERT INTO t VALUES (1, 'one', 1.5), (2, 'two', NULL), (-7, 'minus seven', 0.25)",
        "INSERT INTO t (b, c) VALUES ('next', 9.0)",
        "INSERT INTO t VALUES (100, printf('%.*c', 9000, 'x'), 3.0)",
    ];
    let queries = [
        "SELECT a, b, c FROM t ORDER BY a",
        "SELECT a, length(b) FROM t WHERE a > 1",
        "SELECT count(*), sum(a), max(length(b)) FROM t",
        "SELECT b FROM t WHERE a = 2",
        "SELECT a FROM t ORDER BY a DESC",
    ];
    let (_pdb, pager) = connect(false);
    let (_vdb, versioned) = connect(true);
    for sql in script {
        run(&pager, sql);
        run(&versioned, sql);
    }
    assert_eq!(root(&versioned, "t"), 3);
    for q in queries {
        let want = rows(&pager, q);
        assert!(!want.is_empty(), "{q} answered nothing");
        assert_eq!(rows(&versioned, q), want, "{q}");
    }
}

/// Do: insert in an explicit transaction and roll it back, then commit another; read from a second
/// connection. Expect: the rolled-back rows gone and the committed ones seen, on both storages.
#[test]
fn t_ns_2_6_transactions_end_alike() {
    let (pdb, pager) = connect(false);
    let (vdb, versioned) = connect(true);
    for conn in [&pager, &versioned] {
        run(conn, "CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT)");
        run(conn, "BEGIN");
        run(conn, "INSERT INTO t VALUES (1, 'gone')");
        run(conn, "ROLLBACK");
        run(conn, "BEGIN");
        run(conn, "INSERT INTO t VALUES (2, 'kept')");
        run(conn, "UPDATE t SET b = 'kept twice' WHERE a = 2");
        run(conn, "INSERT INTO t VALUES (3, 'deleted')");
        run(conn, "DELETE FROM t WHERE a = 3");
        run(conn, "COMMIT");
    }
    let other_p = pdb.connect(ConnectOpts::new()).unwrap();
    let other_v = vdb.connect(ConnectOpts::new()).unwrap();
    let q = "SELECT a, b FROM t ORDER BY a";
    let want = rows(&other_p, q);
    assert_eq!(want.len(), 1);
    assert_eq!(rows(&other_v, q), want);
    assert_eq!(rows(&versioned, q), want);
}

/// Do: tables with indexes (an explicit one, a descending NOCASE one, a UNIQUE autoindex), queries that read through them, DELETE without WHERE, DROP INDEX and DROP TABLE,
/// on both storages. Expect: the same answers, and a duplicate refused by the UNIQUE index on both.
#[test]
fn t_ns_2_6_ddl_and_indexes_answer_alike() {
    let script = [
        "CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT UNIQUE, c INTEGER)",
        "CREATE INDEX tc ON t (c)",
        "CREATE INDEX tb ON t (b COLLATE NOCASE DESC)",
        "INSERT INTO t VALUES (1, 'Pear', 30), (2, 'apple', 10), (3, 'Fig', 20), (4, 'date', 10)",
        "UPDATE t SET c = 40 WHERE b = 'Fig'",
        "CREATE TABLE gone (x)",
        "INSERT INTO gone VALUES (1), (2)",
        "DELETE FROM gone",
        "DROP TABLE gone",
        "CREATE TABLE later (x)",
        "INSERT INTO later VALUES ('after a drop')",
    ];
    let queries = [
        "SELECT a FROM t WHERE c = 10 ORDER BY a",
        "SELECT c, b FROM t INDEXED BY tc WHERE c > 15",
        "SELECT b FROM t INDEXED BY tb ORDER BY b COLLATE NOCASE DESC",
        "SELECT a FROM t WHERE b = 'apple'",
        "SELECT name FROM sqlite_schema ORDER BY name",
        "SELECT x FROM later",
    ];
    let (_pdb, pager) = connect(false);
    let (_vdb, versioned) = connect(true);
    for sql in script {
        run(&pager, sql);
        run(&versioned, sql);
    }
    for q in queries {
        let want = rows(&pager, q);
        assert!(!want.is_empty(), "{q} answered nothing");
        assert_eq!(rows(&versioned, q), want, "{q}");
    }
    for conn in [&pager, &versioned] {
        assert!(conn
            .execute("INSERT INTO t VALUES (9, 'apple', 0)")
            .is_err());
        run(conn, "DROP INDEX tc");
    }
    let q = "SELECT a, b, c FROM t ORDER BY c, a";
    assert_eq!(rows(&versioned, q), rows(&pager, q));
}

/// Do: ANALYZE a table with two indexes on both storages, then read `sqlite_stat1` and let a new
/// connection load the statistics. Expect: the same statistics rows, and the new connection's
/// query answering alike.
#[test]
fn t_ns_2_6_analyze_answers_alike() {
    let (pdb, pager) = connect(false);
    let (vdb, versioned) = connect(true);
    for conn in [&pager, &versioned] {
        run(
            conn,
            "CREATE TABLE t (a INTEGER PRIMARY KEY, b INTEGER, c TEXT)",
        );
        run(conn, "CREATE INDEX tb ON t (b)");
        run(conn, "CREATE INDEX tbc ON t (b, c)");
        for i in 0..200 {
            run(
                conn,
                &format!("INSERT INTO t VALUES ({i}, {}, 'c{}')", i % 7, i % 3),
            );
        }
        run(conn, "ANALYZE");
    }
    let q = "SELECT tbl, idx, stat FROM sqlite_stat1 ORDER BY tbl, idx";
    let want = rows(&pager, q);
    assert!(!want.is_empty());
    assert_eq!(rows(&versioned, q), want);
    let other_p = pdb.connect(ConnectOpts::new()).unwrap();
    let other_v = vdb.connect(ConnectOpts::new()).unwrap();
    let q = "SELECT count(*) FROM t WHERE b = 3 AND c = 'c1'";
    assert_eq!(rows(&other_v, q), rows(&other_p, q));
}

/// Do: in an explicit transaction, a statement that writes a row and then fails on a duplicate key,
/// between two that succeed, then COMMIT. Expect: only the failed statement undone, on both
/// storages.
#[test]
fn t_ns_2_6_a_failed_statement_is_undone_alone() {
    let (_pdb, pager) = connect(false);
    let (_vdb, versioned) = connect(true);
    for conn in [&pager, &versioned] {
        run(conn, "CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT)");
        run(conn, "BEGIN");
        run(conn, "INSERT INTO t VALUES (1, 'first')");
        assert!(conn
            .execute("INSERT INTO t VALUES (2, 'undone'), (1, 'duplicate')")
            .is_err());
        run(conn, "INSERT INTO t VALUES (3, 'third')");
        run(conn, "COMMIT");
    }
    let q = "SELECT a, b FROM t ORDER BY a";
    let want = rows(&pager, q);
    assert_eq!(want.len(), 2);
    assert_eq!(rows(&versioned, q), want);
}

/// Do: in a transaction, drop a table and create another, then roll back; then drop the table and
/// commit. Expect: the rollback keeps the dropped table's rows and leaves no new table, and the
/// commit drops it, on both storages.
#[test]
fn t_ns_2_6_ddl_rolls_back_with_its_transaction() {
    let (_pdb, pager) = connect(false);
    let (_vdb, versioned) = connect(true);
    let q = "SELECT a FROM t ORDER BY a";
    for conn in [&pager, &versioned] {
        run(conn, "CREATE TABLE t (a INTEGER PRIMARY KEY)");
        run(conn, "INSERT INTO t VALUES (1), (2)");
        run(conn, "BEGIN");
        run(conn, "DROP TABLE t");
        run(conn, "CREATE TABLE u (b)");
        run(conn, "INSERT INTO u VALUES (1)");
        run(conn, "ROLLBACK");
        assert_eq!(rows(conn, q).len(), 2);
        assert!(conn.prepare("SELECT b FROM u").is_err());
        run(conn, "BEGIN");
        run(conn, "DROP TABLE t");
        run(conn, "COMMIT");
        assert!(conn.prepare(q).is_err());
        run(conn, "CREATE TABLE v (c)");
        run(conn, "INSERT INTO v VALUES ('after')");
    }
    let q = "SELECT c FROM v";
    assert_eq!(rows(&versioned, q), rows(&pager, q));
}
