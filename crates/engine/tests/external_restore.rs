//! Regression tests for quiescent external file restore (`Database::reload_wal_after_external_restore`),
//! covering warmed private connections, shared page pools with equal-cookie schema changes, and
//! two-segment (`-wal` + `-wal2`) WAL recovery (Spec P1-4).
#![cfg(feature = "conn_raw_api")]

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, Numeric, OpenOptions, PlatformIO,
    SqliteDialect, Value,
};
use std::path::Path;
use std::sync::Arc;

fn open_db(path: &Path, share_pages: bool) -> Arc<Database> {
    let io = Arc::new(PlatformIO::new().unwrap());
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io, path.to_str().unwrap(), options).unwrap();
    if share_pages {
        db.share_pages(256).unwrap();
    }
    db
}

fn ints(conn: &Arc<Connection>, sql: &str) -> Vec<i64> {
    let mut stmt = conn.prepare(sql).unwrap();
    stmt.run_collect_rows()
        .unwrap()
        .into_iter()
        .map(|row| match &row[0] {
            Value::Numeric(Numeric::Integer(v)) => *v,
            other => panic!("expected integer, got {other:?}"),
        })
        .collect()
}

fn replace_db_files(donor: &Path, target: &Path, atomic_rename: bool) {
    let donor_str = donor.to_str().unwrap();
    let target_str = target.to_str().unwrap();
    for suffix in ["", "-wal", "-wal2"] {
        let src = format!("{donor_str}{suffix}");
        let dst = format!("{target_str}{suffix}");
        if Path::new(&src).exists() {
            if atomic_rename {
                let staged = format!("{dst}.staged");
                std::fs::copy(&src, &staged).unwrap();
                std::fs::rename(&staged, &dst).unwrap();
            } else {
                std::fs::copy(&src, &dst).unwrap();
            }
        } else if Path::new(&dst).exists() {
            std::fs::remove_file(&dst).unwrap();
        }
    }
}

/// Fixture 1: A warmed private connection (without shared pages) must observe restored rows
/// matching a cold reopen, both for in-place overwrite and atomic-rename inode replacement.
#[test]
fn p1_external_restore_invalidates_warmed_private_connection() {
    for atomic_rename in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let target_path = dir.path().join("target.db");
        let donor_path = dir.path().join("donor.db");

        // Build target with [11] and warm a private connection.
        let target_db = open_db(&target_path, false);
        let warmed = target_db.connect(ConnectOpts::new()).unwrap();
        warmed.execute("PRAGMA wal_autocheckpoint = 0").unwrap();
        warmed.execute("CREATE TABLE t(v INTEGER)").unwrap();
        warmed.execute("INSERT INTO t VALUES (11)").unwrap();
        warmed.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        assert_eq!(ints(&warmed, "SELECT v FROM t ORDER BY v"), vec![11]);

        // Build donor with [22] (checkpointed into main DB file).
        {
            let donor_db = open_db(&donor_path, false);
            let donor_conn = donor_db.connect(ConnectOpts::new()).unwrap();
            donor_conn.execute("PRAGMA wal_autocheckpoint = 0").unwrap();
            donor_conn.execute("CREATE TABLE t(v INTEGER)").unwrap();
            donor_conn.execute("INSERT INTO t VALUES (22)").unwrap();
            donor_conn
                .execute("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
            assert_eq!(ints(&donor_conn, "SELECT v FROM t ORDER BY v"), vec![22]);
        }

        // Quiescent external replace + reload.
        replace_db_files(&donor_path, &target_path, atomic_rename);
        target_db.reload_wal_after_external_restore().unwrap();

        assert_eq!(
            ints(&warmed, "SELECT v FROM t ORDER BY v"),
            vec![22],
            "warmed private connection returned stale data (atomic_rename={atomic_rename})"
        );
        let cold_db = open_db(&target_path, false);
        let cold = cold_db.connect(ConnectOpts::new()).unwrap();
        assert_eq!(ints(&cold, "SELECT v FROM t ORDER BY v"), vec![22]);
    }
}

/// Fixture 2: With shared pages enabled, an external restore that replaces the schema at the SAME
/// `schema_cookie` (different column name and row value) must purge shared page keys and publish
/// the restored catalog to both warmed and newly connected sessions.
#[test]
fn p1_external_restore_purges_shared_pages_and_republishes_schema_at_equal_cookie() {
    let dir = tempfile::tempdir().unwrap();
    let target_path = dir.path().join("target.db");
    let donor_path = dir.path().join("donor.db");

    let target_db = open_db(&target_path, true);
    let warmed = target_db.connect(ConnectOpts::new()).unwrap();
    warmed.execute("PRAGMA wal_autocheckpoint = 0").unwrap();
    warmed.execute("CREATE TABLE t(old_col INTEGER)").unwrap();
    warmed.execute("INSERT INTO t VALUES (11)").unwrap();
    warmed.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    // Warm shared page cache and connection schema cache.
    assert_eq!(ints(&warmed, "SELECT old_col FROM t"), vec![11]);

    {
        let donor_db = open_db(&donor_path, false);
        let donor_conn = donor_db.connect(ConnectOpts::new()).unwrap();
        donor_conn.execute("PRAGMA wal_autocheckpoint = 0").unwrap();
        donor_conn
            .execute("CREATE TABLE t(new_col INTEGER)")
            .unwrap();
        donor_conn.execute("INSERT INTO t VALUES (22)").unwrap();
        donor_conn
            .execute("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        assert_eq!(ints(&donor_conn, "SELECT new_col FROM t"), vec![22]);
    }

    replace_db_files(&donor_path, &target_path, true);
    target_db.reload_wal_after_external_restore().unwrap();

    // Both the previously warmed connection and a fresh connection must see `new_col` = [22] and
    // reject `old_col`.
    assert!(
        warmed.prepare("SELECT old_col FROM t").is_err(),
        "warmed connection still exposed pre-restore column old_col"
    );
    assert_eq!(ints(&warmed, "SELECT new_col FROM t"), vec![22]);

    let fresh = target_db.connect(ConnectOpts::new()).unwrap();
    assert!(
        fresh.prepare("SELECT old_col FROM t").is_err(),
        "fresh connection still exposed pre-restore column old_col"
    );
    assert_eq!(ints(&fresh, "SELECT new_col FROM t"), vec![22]);
}

/// Fixture 3: External restore of a two-WAL (`-wal` + `-wal2`) database must recover BOTH segments
/// and return `[22, 33]`, matching both the donor and a cold reopen.
#[test]
fn p1_external_restore_recovers_two_wal_segments() {
    let dir = tempfile::tempdir().unwrap();
    let target_path = dir.path().join("target.db");
    let source_path = dir.path().join("source.db");
    let donor_path = dir.path().join("donor.db");

    // Prepare target with [11] and warm a connection (with shared pages enabled).
    let target_db = open_db(&target_path, true);
    let warmed = target_db.connect(ConnectOpts::new()).unwrap();
    warmed.execute("PRAGMA wal_autocheckpoint = 0").unwrap();
    warmed.execute("CREATE TABLE t(v INTEGER)").unwrap();
    warmed.execute("INSERT INTO t VALUES (11)").unwrap();
    assert_eq!(ints(&warmed, "SELECT v FROM t ORDER BY v"), vec![11]);

    // Build a two-WAL donor image:
    // - main DB file has empty table `t` (checkpointed)
    // - `-wal` (closing segment, seq N) has row 22
    // - `-wal2` (current segment, seq N+1) has row 33
    {
        let source_db = open_db(&source_path, false);
        let src = source_db.connect(ConnectOpts::new()).unwrap();
        src.execute("PRAGMA wal_autocheckpoint = 0").unwrap();
        src.execute("CREATE TABLE t(v INTEGER)").unwrap();
        src.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();

        // Commit 22 into `-wal`, snapshot DB + `-wal` into donor.
        src.execute("INSERT INTO t VALUES (22)").unwrap();
        std::fs::copy(&source_path, &donor_path).unwrap();
        std::fs::copy(
            format!("{}-wal", source_path.display()),
            format!("{}-wal", donor_path.display()),
        )
        .unwrap();

        // Advance checkpoint sequence via TRUNCATE, commit 33 into the new `-wal`, and save that
        // newer segment beside donor as `-wal2`.
        src.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        src.execute("INSERT INTO t VALUES (33)").unwrap();
        std::fs::copy(
            format!("{}-wal", source_path.display()),
            format!("{}-wal2", donor_path.display()),
        )
        .unwrap();
    }

    // Confirm cold open of donor sees [22, 33].
    {
        let donor_check = open_db(&donor_path, false);
        let c = donor_check.connect(ConnectOpts::new()).unwrap();
        assert_eq!(ints(&c, "SELECT v FROM t ORDER BY v"), vec![22, 33]);
    }

    // Externally restore target from the two-WAL donor and reload.
    replace_db_files(&donor_path, &target_path, false);
    target_db.reload_wal_after_external_restore().unwrap();

    assert_eq!(
        ints(&warmed, "SELECT v FROM t ORDER BY v"),
        vec![22, 33],
        "warmed connection missed rows across -wal and -wal2 after reload"
    );
    let fresh = target_db.connect(ConnectOpts::new()).unwrap();
    assert_eq!(ints(&fresh, "SELECT v FROM t ORDER BY v"), vec![22, 33]);

    let cold_db = open_db(&target_path, false);
    let cold = cold_db.connect(ConnectOpts::new()).unwrap();
    assert_eq!(ints(&cold, "SELECT v FROM t ORDER BY v"), vec![22, 33]);
}
