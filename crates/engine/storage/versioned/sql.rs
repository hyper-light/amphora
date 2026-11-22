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

/// T-NS.2.11: in a transaction, a table and index made under a savepoint rolled back to and a DROP
/// under a savepoint rolled back to, then COMMIT, on both storages; then a CREATE TABLE ... AS that
/// fails after its tree is made, in a transaction committed after it. Expect: the storage holds the trees it held before (no tree no schema row names), and
/// the table whose DROP was undone keeps its rows.
#[test]
fn ns_2_1_undone_ddl_leaves_no_orphan_tree() {
    let (_pdb, pager) = connect(false);
    let (db, versioned) = connect(true);
    let live = || {
        db.versioned_storage(false, None)
            .unwrap()
            .unwrap()
            .live_trees()
    };
    let mut before = 0;
    for conn in [&pager, &versioned] {
        run(conn, "CREATE TABLE t (a INTEGER PRIMARY KEY)");
        run(conn, "INSERT INTO t VALUES (1)");
        if std::ptr::eq(conn, &versioned) {
            before = live();
        }
        for sql in [
            "BEGIN",
            "SAVEPOINT s",
            "CREATE TABLE w (c)",
            "CREATE INDEX wi ON w (c)",
            "INSERT INTO w VALUES (1)",
            "ROLLBACK TO s",
            "RELEASE s",
            "SAVEPOINT s2",
            "DROP TABLE t",
            "ROLLBACK TO s2",
            "RELEASE s2",
            "COMMIT",
        ] {
            let done = conn.execute(sql);
            assert!(
                done.is_ok(),
                "{}: {sql}: {done:?}",
                std::ptr::eq(conn, &versioned)
            );
        }
        assert_eq!(rows(conn, "SELECT a FROM t").len(), 1);
    }
    assert_eq!(live(), before);
    // A failed statement in a transaction, then COMMIT: on the versioned storage alone, as the
    // pager's next statement after it is refused `SchemaUpdated` (the pager's, noted in STATUS).
    run(&versioned, "BEGIN");
    let failed = versioned.execute("CREATE TABLE u AS SELECT abs(-9223372036854775807 - 1) AS x");
    assert!(failed.is_err(), "{failed:?}");
    let done = versioned.execute("COMMIT");
    assert!(done.is_ok(), "{done:?}");
    assert_eq!(live(), before);
}

/// Do: SAVEPOINTs nested in a transaction, ROLLBACK TO the inner one twice, RELEASE the outer one,
/// COMMIT; and a SAVEPOINT that starts a transaction, rolled back to and released. Expect: the
/// same rows on both storages.
#[test]
fn t_ns_2_6_named_savepoints_answer_alike() {
    let (_pdb, pager) = connect(false);
    let (_vdb, versioned) = connect(true);
    for conn in [&pager, &versioned] {
        run(conn, "CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT)");
        run(conn, "BEGIN");
        run(conn, "INSERT INTO t VALUES (1, 'kept')");
        run(conn, "SAVEPOINT outer_sp");
        run(conn, "INSERT INTO t VALUES (2, 'kept too')");
        run(conn, "SAVEPOINT inner_sp");
        run(conn, "INSERT INTO t VALUES (3, 'undone')");
        run(conn, "UPDATE t SET b = 'undone' WHERE a = 1");
        run(conn, "ROLLBACK TO inner_sp");
        run(conn, "INSERT INTO t VALUES (4, 'undone again')");
        run(conn, "ROLLBACK TO inner_sp");
        run(conn, "RELEASE outer_sp");
        run(conn, "COMMIT");
        run(conn, "SAVEPOINT alone");
        run(conn, "DELETE FROM t WHERE a = 2");
        run(conn, "ROLLBACK TO alone");
        run(conn, "INSERT INTO t VALUES (5, 'last')");
        run(conn, "RELEASE alone");
    }
    let q = "SELECT a, b FROM t ORDER BY a";
    let want = rows(&pager, q);
    assert_eq!(want.len(), 3);
    assert_eq!(rows(&versioned, q), want);
}

fn open_file(path: &std::path::Path) -> (Arc<Database>, Arc<Connection>) {
    let io: Arc<dyn IO> = Arc::new(crate::io::PlatformIO::new().unwrap());
    let db = Database::open_file(io, path.to_str().unwrap(), Arc::new(SqliteDialect)).unwrap();
    let conn = db
        .connect(ConnectOpts::new().with_versioned_storage())
        .unwrap();
    (db, conn)
}

/// Copies `from` (a file or a directory of files) to `to`, as a crash leaves them.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    if from.is_dir() {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    } else if from.exists() {
        std::fs::copy(from, to).unwrap();
    }
}

/// Do (`AC-NS.2.1`, G7 step 2): on a file database on the versioned storage, tables with indexes
/// (a descending NOCASE one, a UNIQUE autoindex, one of two columns), rows written and
/// committed, then the database closed and opened again; and the same database's files copied
/// while it was still open, as a crash leaves them, and the copy opened. Expect: both open with
/// every table and index, read the committed rows through each index in its order, and refuse a
/// duplicate through the UNIQUE index, and check whole (`PRAGMA integrity_check` reads each tree
/// in its order): each tree's order was recorded and the schema read from the storage's
/// `sqlite_schema`.
#[test]
fn ns_2_1_a_file_database_reopens_after_a_close_and_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reopen.db");
    let queries = [
        "SELECT a, b, c FROM t ORDER BY a",
        "SELECT b FROM t INDEXED BY tb ORDER BY b COLLATE NOCASE DESC",
        "SELECT c, a FROM t INDEXED BY tc WHERE c >= 20 ORDER BY c",
        "SELECT k, v FROM w INDEXED BY wkv WHERE k > 'k8' ORDER BY k, v DESC",
        "SELECT name FROM sqlite_schema ORDER BY name",
    ];
    let want = {
        let (db, conn) = open_file(&path);
        for sql in [
            "CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT UNIQUE, c INTEGER)",
            "CREATE INDEX tc ON t (c)",
            "CREATE INDEX tb ON t (b COLLATE NOCASE DESC)",
            "CREATE TABLE w (k TEXT, v INTEGER)",
            "CREATE INDEX wkv ON w (k, v DESC)",
        ] {
            run(&conn, sql);
        }
        for i in 0..300 {
            run(
                &conn,
                &format!(
                    "INSERT INTO t VALUES ({i}, '{}{i}', {})",
                    if i % 2 == 0 { "Pear" } else { "apple" },
                    (i * 37) % 101
                ),
            );
            run(
                &conn,
                &format!("INSERT INTO w VALUES ('k{}', {i})", 999 - i),
            );
        }
        run(&conn, "BEGIN");
        run(&conn, "DELETE FROM t WHERE a % 3 = 0");
        run(&conn, "UPDATE w SET v = -v WHERE v % 5 = 0");
        run(&conn, "COMMIT");
        let want: Vec<_> = queries.iter().map(|q| rows(&conn, q)).collect();
        for w in &want {
            assert!(!w.is_empty());
        }
        drop(conn);
        drop(db);
        want
    };
    let (db, conn) = open_file(&path);
    for (q, w) in queries.iter().zip(&want) {
        assert_eq!(&rows(&conn, q), w, "after a reopen: {q}");
    }
    assert!(conn
        .execute("INSERT INTO t VALUES (1000, 'apple1', 0)")
        .is_err());
    run(&conn, "INSERT INTO t VALUES (1000, 'fig', 50)");
    let want: Vec<_> = queries.iter().map(|q| rows(&conn, q)).collect();
    // The crash: the files copied while the database is open, its close never run.
    let copy = dir.path().join("crashed.db");
    for (from, to) in [
        (path.clone(), copy.clone()),
        (
            std::path::PathBuf::from(format!("{}-wal", path.display())),
            std::path::PathBuf::from(format!("{}-wal", copy.display())),
        ),
        (
            std::path::PathBuf::from(format!("{}-versioned", path.display())),
            std::path::PathBuf::from(format!("{}-versioned", copy.display())),
        ),
    ] {
        copy_tree(&from, &to);
    }
    std::mem::forget(conn);
    std::mem::forget(db);
    let (_db, conn) = open_file(&copy);
    for (q, w) in queries.iter().zip(&want) {
        assert_eq!(&rows(&conn, q), w, "after a crash: {q}");
    }
    assert_eq!(
        rows(&conn, "PRAGMA integrity_check"),
        vec![vec![Value::build_text("ok")]]
    );
}

/// Whether `e` is the versioned storage's refusal of a write that met another open writer's row:
/// its first writer's win, or its commit's validation.
fn conflict(e: &crate::LimboError) -> bool {
    matches!(
        e,
        crate::LimboError::WriteWriteConflict | crate::LimboError::SerializationFailure
    )
}

/// Two connections to one database on the versioned storage, a table of two committed rows.
fn two_writers() -> (Arc<Database>, Arc<Connection>, Arc<Connection>) {
    let (db, a) = connect(true);
    run(
        &a,
        "CREATE TABLE t (id INTEGER PRIMARY KEY, v INTEGER NOT NULL)",
    );
    run(&a, "INSERT INTO t VALUES (1, 0), (2, 0)");
    let b = db
        .connect(ConnectOpts::new().with_versioned_storage())
        .unwrap();
    (db, a, b)
}

/// AC-NS.2.1: do open two write transactions through SQL on the versioned storage at once, each
/// changing and inserting rows of its own, and commit both; expect neither refused (no pager write
/// lock between them: they conflict only on the rows they touch) and both one's rows in the table.
#[test]
fn ac_ns_2_1_writers_of_different_rows_both_commit() {
    let (_db, a, b) = two_writers();
    run(&a, "BEGIN");
    run(&b, "BEGIN");
    run(&a, "UPDATE t SET v = 10 WHERE id = 1");
    run(&b, "UPDATE t SET v = 20 WHERE id = 2");
    run(&a, "INSERT INTO t VALUES (3, 30)");
    run(&b, "INSERT INTO t VALUES (4, 40)");
    run(&a, "COMMIT");
    run(&b, "COMMIT");
    let want: Vec<Vec<Value>> = [(1, 10), (2, 20), (3, 30), (4, 40)]
        .into_iter()
        .map(|(id, v)| vec![Value::from_i64(id), Value::from_i64(v)])
        .collect();
    assert_eq!(rows(&a, "SELECT id, v FROM t ORDER BY id"), want);
    assert_eq!(rows(&b, "SELECT id, v FROM t ORDER BY id"), want);
    // The same through BEGIN IMMEDIATE, which on the pager's storage refuses the second writer.
    run(&a, "BEGIN IMMEDIATE");
    run(&b, "BEGIN IMMEDIATE");
    run(&a, "UPDATE t SET v = 11 WHERE id = 1");
    run(&b, "UPDATE t SET v = 21 WHERE id = 2");
    run(&b, "COMMIT");
    run(&a, "COMMIT");
    assert_eq!(
        rows(&a, "SELECT v FROM t WHERE id IN (1, 2) ORDER BY id"),
        vec![vec![Value::from_i64(11)], vec![Value::from_i64(21)]]
    );
}

/// AC-NS.2.1: do open two write transactions through SQL on the versioned storage at once, both
/// changing one row; expect the second refused with a conflict (at its write or its commit), and
/// after it rolls back, the first's commit to stand alone.
#[test]
fn ac_ns_2_1_writers_of_one_row_one_conflicts() {
    let (_db, a, b) = two_writers();
    run(&a, "BEGIN");
    run(&b, "BEGIN");
    run(&a, "UPDATE t SET v = 10 WHERE id = 1");
    let refused = match b.execute("UPDATE t SET v = 20 WHERE id = 1") {
        Err(e) => {
            assert!(conflict(&e), "the second writer's refusal: {e:?}");
            true
        }
        Ok(()) => false,
    };
    run(&a, "COMMIT");
    if !refused {
        let e = b
            .execute("COMMIT")
            .expect_err("the second writer of one row commits");
        assert!(conflict(&e), "the second writer's refusal at commit: {e:?}");
    }
    if !b.get_auto_commit() {
        run(&b, "ROLLBACK");
    }
    assert_eq!(
        rows(&b, "SELECT id, v FROM t ORDER BY id"),
        vec![
            vec![Value::from_i64(1), Value::from_i64(10)],
            vec![Value::from_i64(2), Value::from_i64(0)],
        ]
    );
}

/// AC-NS.2.1: what writes the pager's header (here a schema change, its cookie) still takes the
/// pager's write lock, one at a time, beside a writer of rows that holds none: do run a row writer
/// open and one connection's CREATE TABLE in an open transaction, then another's; expect the first
/// schema change to pass beside the row writer, the second to be refused busy until it ends, the
/// row writer, begun before the first committed, refused at its commit (lane g7s2-17: it wrote
/// against the older catalog) and its write standing once run again, and every commit after to
/// stand.
#[test]
fn ac_ns_2_1_header_writers_still_take_the_pager_lock() {
    let (db, a, b) = two_writers();
    let c = db
        .connect(ConnectOpts::new().with_versioned_storage())
        .unwrap();
    run(&a, "BEGIN");
    run(&a, "UPDATE t SET v = 10 WHERE id = 1");
    run(&b, "BEGIN");
    run(&b, "CREATE TABLE u (x)");
    run(&c, "BEGIN");
    let e = c
        .execute("CREATE TABLE w (x)")
        .expect_err("a second schema change beside an open one");
    assert!(
        matches!(e, crate::LimboError::Busy | crate::LimboError::BusySnapshot),
        "the second schema change's refusal: {e:?}"
    );
    if !c.get_auto_commit() {
        run(&c, "ROLLBACK");
    }
    run(&b, "COMMIT");
    let e = a
        .execute("COMMIT")
        .expect_err("a writer begun before a schema change commits");
    assert!(
        matches!(e, crate::LimboError::SchemaConflict),
        "the row writer's refusal: {e:?}"
    );
    if !a.get_auto_commit() {
        run(&a, "ROLLBACK");
    }
    run(&a, "UPDATE t SET v = 10 WHERE id = 1");
    run(&c, "CREATE TABLE w (x)");
    assert_eq!(
        rows(&c, "SELECT v FROM t WHERE id = 1"),
        vec![vec![Value::from_i64(10)]]
    );
    assert_eq!(
        rows(
            &c,
            "SELECT name FROM sqlite_schema WHERE name IN ('u', 'w') ORDER BY name"
        ),
        vec![vec![Value::build_text("u")], vec![Value::build_text("w")]]
    );
}

/// The ids with `v` found through the index `tv`, and through the table alone: equal when the two
/// agree.
fn by_index(conn: &Arc<Connection>, v: i64) -> (Vec<Vec<Value>>, Vec<Vec<Value>>) {
    (
        rows(
            conn,
            &format!("SELECT id FROM t INDEXED BY tv WHERE v = {v}"),
        ),
        rows(conn, &format!("SELECT id FROM t NOT INDEXED WHERE v = {v}")),
    )
}

/// AC-NS.2.1 (lane g7s2-17): writers through SQL run at once, so do open a writer that inserts a
/// row, by a statement prepared before and run after another connection's CREATE INDEX on the
/// table commits (and, the other way, one whose insert ran before it); then commit the writer.
/// Expect, both ways: the commit refused with `SchemaConflict` (its statements were compiled
/// against the catalog without the index: committed, its row would be in the table and not the
/// index), nothing of it in either; run again, its row in both.
#[test]
fn ac_ns_2_1_a_writer_against_an_older_schema_is_refused_at_commit() {
    for prepared_before in [true, false] {
        let (_db, a, b) = two_writers();
        let mut insert = a.prepare("INSERT INTO t VALUES (3, 30)").unwrap();
        run(&a, "BEGIN");
        if prepared_before {
            // Its snapshot begins here, before the index.
            assert_eq!(
                rows(&a, "SELECT count(*) FROM t"),
                vec![vec![Value::from_i64(2)]]
            );
        } else {
            insert.run_ignore_rows().unwrap();
        }
        run(&b, "CREATE INDEX tv ON t (v)");
        if prepared_before {
            insert.run_ignore_rows().unwrap();
        }
        let e = a
            .execute("COMMIT")
            .expect_err("a writer against an older schema commits");
        assert!(
            matches!(e, crate::LimboError::SchemaConflict),
            "prepared_before={prepared_before}: the writer's refusal: {e:?}"
        );
        if !a.get_auto_commit() {
            run(&a, "ROLLBACK");
        }
        assert_eq!(
            by_index(&b, 30),
            (vec![], vec![]),
            "prepared_before={prepared_before}"
        );
        assert_eq!(
            rows(&b, "SELECT count(*) FROM t"),
            vec![vec![Value::from_i64(2)]]
        );
        run(&a, "INSERT INTO t VALUES (3, 30)");
        let three = vec![vec![Value::from_i64(3)]];
        assert_eq!(
            by_index(&b, 30),
            (three.clone(), three),
            "prepared_before={prepared_before}"
        );
    }
}

/// The schema cookie (`PRAGMA schema_version`) as `conn` reads it.
fn cookie(conn: &Arc<Connection>) -> Vec<Vec<Value>> {
    rows(conn, "PRAGMA schema_version")
}

/// The names `sqlite_schema` holds, as `conn` reads them.
fn names(conn: &Arc<Connection>) -> Vec<Vec<Value>> {
    rows(conn, "SELECT name FROM sqlite_schema ORDER BY name")
}

/// AC-NS.2.1 (lane g7s2-18): a commit is one across the pager and the versioned storage. Do open
/// a transaction that builds an index of `t` (it reads the table and writes its schema row, its
/// index rows and the pager's header, the cookie), let another connection insert a row of `t` and
/// commit, then commit the index; expect it refused (it read the table the insert changed), and
/// nothing of it anywhere: the cookie and the schema rows as before on its own connection, on the
/// other and on a new one, and no connection knowing the index. Run again, the index stands on
/// every connection, the cookie one past. The pager committed first: its cookie stood, and its
/// schema, the index naming a tree the storage dropped, was every connection's.
#[test]
fn ac_ns_2_1_a_schema_change_the_storage_refuses_leaves_nothing() {
    let (db, a, b) = two_writers();
    let (cookie_before, names_before) = (cookie(&a), names(&a));
    run(&a, "BEGIN");
    run(&a, "CREATE INDEX tv ON t (v)");
    run(&b, "INSERT INTO t VALUES (3, 0)");
    let e = a
        .execute("COMMIT")
        .expect_err("an index built over rows changed beside it commits");
    assert!(
        matches!(
            e,
            crate::LimboError::SerializationFailure | crate::LimboError::WriteWriteConflict
        ),
        "the schema change's refusal: {e:?}"
    );
    if !a.get_auto_commit() {
        run(&a, "ROLLBACK");
    }
    let c = db
        .connect(ConnectOpts::new().with_versioned_storage())
        .unwrap();
    for (who, conn) in [("its own", &a), ("the other", &b), ("a new one", &c)] {
        assert_eq!(cookie(conn), cookie_before, "{who} connection's cookie");
        assert_eq!(names(conn), names_before, "{who} connection's schema rows");
        assert!(
            conn.prepare("SELECT id FROM t INDEXED BY tv WHERE v = 0")
                .is_err(),
            "{who} connection knows the refused index"
        );
    }
    run(&a, "CREATE INDEX tv ON t (v)");
    let all = vec![
        vec![Value::from_i64(1)],
        vec![Value::from_i64(2)],
        vec![Value::from_i64(3)],
    ];
    for (who, conn) in [("its own", &a), ("the other", &b), ("a new one", &c)] {
        assert_ne!(cookie(conn), cookie_before, "{who} connection's cookie");
        let (by_tv, by_table) = by_index(conn, 0);
        assert_eq!(by_tv, all, "{who} connection, through the index");
        assert_eq!(by_table, all, "{who} connection, through the table");
    }
}

/// AC-NS.2.1 (lane g7s2-19): a writer's schema checks read the header its pager's read sees, and
/// that read can begin before its storage snapshot does. Do open a transaction whose first
/// statement reads only the header (`PRAGMA schema_version`: its pager's read begins, no row of
/// the storage read), let another connection's CREATE INDEX on `t` commit, then insert a row of
/// `t` (compiled against the schema without the index, its cookie check against the older header
/// passing, its storage snapshot begun only now, after the index) and commit. Expect the header
/// still the older one, the commit refused with `SchemaConflict` and nothing of it in the table or
/// the index; run again, its row in both. Limited by its snapshot's start (lane g7s2-17), the
/// writer was let through and its row was in the table alone.
#[test]
fn ac_ns_2_1_a_writer_checked_before_a_schema_change_and_begun_after_is_refused() {
    let (_db, a, b) = two_writers();
    run(&a, "BEGIN");
    let before = cookie(&a);
    run(&b, "CREATE INDEX tv ON t (v)");
    assert_eq!(
        cookie(&a),
        before,
        "the writer's header is its pager read's"
    );
    run(&a, "INSERT INTO t VALUES (3, 30)");
    let e = a
        .execute("COMMIT")
        .expect_err("a writer checked against the schema before an index commits");
    assert!(
        matches!(e, crate::LimboError::SchemaConflict),
        "the writer's refusal: {e:?}"
    );
    if !a.get_auto_commit() {
        run(&a, "ROLLBACK");
    }
    assert_eq!(by_index(&b, 30), (vec![], vec![]));
    run(&a, "INSERT INTO t VALUES (3, 30)");
    let three = vec![vec![Value::from_i64(3)]];
    assert_eq!(by_index(&b, 30), (three.clone(), three));
}

/// AC-NS.2.1 (lane g7s2-19): a schema change's cookie can be read a moment before its rows are
/// published (the pager's part commits under the storage's publish lock, before the rows). Do
/// pause another connection's CREATE INDEX there (`Switch::pause_next_commit_with`), then run a
/// statement on a connection whose schema is the older one: it reads the newer cookie, so it
/// reloads the schema; the index's commit goes on only once that reload is about to wait for it
/// (`Switch::tell_next_publish_wait`). Expect the reload to have read the index's row: that
/// connection, another made before the index and a new one all read the same cookie and find the
/// rows through the index. Without the wait, the reload read the older rows, kept them under the
/// newer cookie and gave them to the database, so no connection after knew the index.
#[test]
fn ac_ns_2_1_a_schema_read_beside_a_schema_change_waits_for_its_rows() {
    let (db, a, c) = two_writers();
    let switch = a.versioned_for(crate::MAIN_DB_ID).unwrap();
    let (paused, go_on) = switch.pause_next_commit_with();
    let maker = {
        let db = db.clone();
        std::thread::spawn(move || {
            let b = db
                .connect(ConnectOpts::new().with_versioned_storage())
                .unwrap();
            let made = b.execute("CREATE INDEX tv ON t (v)");
            // A commit that never paused (it failed first) lets the test hear it.
            if let Some(switch) = b.versioned_for(crate::MAIN_DB_ID) {
                switch.clear_commit_pause();
            }
            made
        })
    };
    // The index's cookie is committed, its rows not yet published.
    paused
        .recv()
        .unwrap_or_else(|_| panic!("the index's commit ended without its pause"));
    let told = switch.tell_next_publish_wait();
    let releaser = std::thread::spawn(move || {
        // Told, or the word dropped unheard: either way the index's commit goes on.
        told.recv().ok();
        go_on.send(()).ok();
    });
    assert_eq!(
        rows(&c, "SELECT count(*) FROM t"),
        vec![vec![Value::from_i64(2)]]
    );
    switch.clear_publish_wait();
    releaser.join().unwrap();
    maker.join().unwrap().unwrap();
    let d = db
        .connect(ConnectOpts::new().with_versioned_storage())
        .unwrap();
    let now = cookie(&d);
    let both = vec![vec![Value::from_i64(1)], vec![Value::from_i64(2)]];
    for (who, conn) in [("the reloaded", &c), ("another", &a), ("a new", &d)] {
        assert_eq!(cookie(conn), now, "{who} connection's cookie");
        assert!(
            conn.prepare("SELECT id FROM t INDEXED BY tv WHERE v = 0")
                .is_ok(),
            "{who} connection doesn't know the index"
        );
        assert_eq!(
            by_index(conn, 0),
            (both.clone(), both.clone()),
            "{who} connection"
        );
    }
}
