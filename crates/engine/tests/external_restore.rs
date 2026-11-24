//! Regression tests for quiescent external file restore (`Database::reload_wal_after_external_restore`),
//! covering warmed private connections, shared page pools with equal-cookie schema changes, and
//! two-segment (`-wal` + `-wal2`) WAL recovery (Spec P1-4).
#![cfg(feature = "conn_raw_api")]

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, EncryptionKey, EncryptionOpts, Numeric,
    OpenOptions, PlatformIO, SqliteDialect, Value,
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

/// Spec P1-1: Prepared statements created before an external restore must NOT execute against a
/// stale schema even when the restored schema has the same `schema_cookie`.
#[test]
fn p1_external_restore_invalidates_prepared_update_at_equal_schema_cookie() {
    let dir = tempfile::tempdir().unwrap();
    let target_path = dir.path().join("target.db");
    let donor_path = dir.path().join("donor.db");

    let target_db = open_db(&target_path, true);
    let warmed = target_db.connect(ConnectOpts::new()).unwrap();
    warmed.execute("PRAGMA wal_autocheckpoint = 0").unwrap();
    warmed.execute("CREATE TABLE t(old_value INTEGER)").unwrap();
    warmed.execute("INSERT INTO t VALUES (11)").unwrap();
    warmed.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();

    // Prepare UPDATE before the restore.
    let mut prepared_update = warmed.prepare("UPDATE t SET old_value = 44").unwrap();

    // Build donor at the exact same schema version with `old_value` renamed to `new_value` = 22.
    {
        let donor_db = open_db(&donor_path, false);
        let donor_conn = donor_db.connect(ConnectOpts::new()).unwrap();
        donor_conn.execute("PRAGMA wal_autocheckpoint = 0").unwrap();
        donor_conn
            .execute("CREATE TABLE t(new_value INTEGER)")
            .unwrap();
        donor_conn.execute("INSERT INTO t VALUES (22)").unwrap();
        donor_conn
            .execute("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
    }

    replace_db_files(&donor_path, &target_path, true);
    target_db.reload_wal_after_external_restore().unwrap();

    // Stepping the previously prepared UPDATE must reprepare against the restored catalog and fail
    // because `old_value` no longer exists, leaving `new_value` untouched at 22.
    assert!(
        prepared_update.run_collect_rows().is_err(),
        "prepared UPDATE succeeded through stale schema after external restore"
    );
    assert_eq!(ints(&warmed, "SELECT new_value FROM t"), vec![22]);
}

/// Spec P1-2: Atomic-rename restore must re-key `DATABASE_MANAGER` to the replacement inode so
/// opening the restored path returns the SAME `Database` handle and commits are visible across
/// handles.
#[test]
fn p1_external_restore_atomic_rename_updates_registry_identity() {
    let dir = tempfile::tempdir().unwrap();
    let target_path = dir.path().join("target.db");
    let donor_path = dir.path().join("donor.db");

    let target_db = open_db(&target_path, true);
    let orig_conn = target_db.connect(ConnectOpts::new()).unwrap();
    orig_conn.execute("CREATE TABLE t(v INTEGER)").unwrap();
    orig_conn.execute("INSERT INTO t VALUES (11)").unwrap();
    orig_conn
        .execute("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();

    {
        let donor_db = open_db(&donor_path, false);
        let donor_conn = donor_db.connect(ConnectOpts::new()).unwrap();
        donor_conn.execute("CREATE TABLE t(v INTEGER)").unwrap();
        donor_conn.execute("INSERT INTO t VALUES (22)").unwrap();
        donor_conn
            .execute("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
    }

    replace_db_files(&donor_path, &target_path, true);
    target_db.reload_wal_after_external_restore().unwrap();

    // Opening the restored path must reuse the existing handle in the process registry.
    let reopened_db = open_db(&target_path, false);
    assert!(
        Arc::ptr_eq(&target_db, &reopened_db),
        "atomic-rename reload left a stale registry key and created a second Database handle"
    );

    let reopened_conn = reopened_db.connect(ConnectOpts::new()).unwrap();
    reopened_conn.execute("INSERT INTO t VALUES (33)").unwrap();
    assert_eq!(
        ints(&reopened_conn, "SELECT v FROM t ORDER BY v"),
        vec![22, 33]
    );
    assert_eq!(ints(&orig_conn, "SELECT v FROM t ORDER BY v"), vec![22, 33]);
}

/// Spec P1-3: In-place reload must keep the existing file descriptor and NEVER drop the Unix
/// advisory database lock held against other processes.
#[cfg(unix)]
#[test]
fn p1_external_restore_in_place_preserves_unix_database_lock() {
    let probe_lock_held_by_us = |path: &Path| -> bool {
        let status = std::process::Command::new("python3")
            .arg("-c")
            .arg(
                "import fcntl, os, sys\nfd = os.open(sys.argv[1], os.O_RDWR)\ntry:\n    fcntl.lockf(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n    sys.exit(0)\nsys.exit(1)\n",
            )
            .arg(path)
            .status()
            .expect("spawn python3 lock probe");
        status.success()
    };

    let dir = tempfile::tempdir().unwrap();
    let target_path = dir.path().join("target.db");
    let donor_path = dir.path().join("donor.db");

    let target_db = open_db(&target_path, true);
    let warmed = target_db.connect(ConnectOpts::new()).unwrap();
    warmed.execute("CREATE TABLE t(v INTEGER)").unwrap();
    warmed.execute("INSERT INTO t VALUES (11)").unwrap();
    warmed.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    assert!(
        probe_lock_held_by_us(&target_path),
        "database lock should reject another process before reload"
    );

    {
        let donor_db = open_db(&donor_path, false);
        let donor_conn = donor_db.connect(ConnectOpts::new()).unwrap();
        donor_conn.execute("CREATE TABLE t(v INTEGER)").unwrap();
        donor_conn.execute("INSERT INTO t VALUES (22)").unwrap();
        donor_conn
            .execute("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
    }

    // 1. External-process in-place overwrite (`cp`) + reload: lock stays held continuously.
    assert!(std::process::Command::new("cp")
        .arg(&donor_path)
        .arg(&target_path)
        .status()
        .expect("cp donor to target")
        .success());
    for suffix in ["-wal", "-wal2"] {
        let p = format!("{}{suffix}", target_path.display());
        if Path::new(&p).exists() {
            std::fs::remove_file(&p).unwrap();
        }
    }
    assert!(
        probe_lock_held_by_us(&target_path),
        "external in-place overwrite should not disturb our advisory lock before reload"
    );
    target_db.reload_wal_after_external_restore().unwrap();
    assert_eq!(ints(&warmed, "SELECT v FROM t ORDER BY v"), vec![22]);
    assert!(
        probe_lock_held_by_us(&target_path),
        "in-place reload dropped the Unix advisory lock on target.db"
    );

    // 2. Same-process in-place overwrite (`std::fs::copy`, which closes a second fd to the same
    //    inode and clears POSIX `F_SETLK`) + reload: reload must restore the advisory lock.
    replace_db_files(&donor_path, &target_path, false);
    target_db.reload_wal_after_external_restore().unwrap();
    assert_eq!(ints(&warmed, "SELECT v FROM t ORDER BY v"), vec![22]);
    assert!(
        probe_lock_held_by_us(&target_path),
        "in-place reload failed to re-establish Unix advisory lock after same-process overwrite"
    );
}

/// Spec P2-1: Restoring valid 1 KiB, 8 KiB, and AES-256-GCM donor images into a 4 KiB target must
/// succeed on both warmed and fresh connections, while a corrupt image must fail BEFORE mutating
/// the live WAL or database state.
#[test]
fn p2_external_restore_handles_page_sizes_encryption_and_rejects_corrupt_atomically() {
    // 1. Different page sizes: 1 KiB and 8 KiB donors restored into 4 KiB targets.
    for donor_page_size in [1024usize, 8192] {
        let dir = tempfile::tempdir().unwrap();
        let target_path = dir.path().join("target.db");
        let donor_path = dir.path().join("donor.db");

        let target_db = open_db(&target_path, true);
        let warmed = target_db.connect(ConnectOpts::new()).unwrap();
        warmed.execute("CREATE TABLE t(v INTEGER)").unwrap();
        warmed.execute("INSERT INTO t VALUES (11)").unwrap();
        warmed.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        assert_eq!(ints(&warmed, "SELECT v FROM t"), vec![11]);

        {
            let donor_db = open_db(&donor_path, false);
            let donor_conn = donor_db.connect(ConnectOpts::new()).unwrap();
            donor_conn
                .execute(format!("PRAGMA page_size = {donor_page_size}"))
                .unwrap();
            donor_conn.execute("CREATE TABLE t(v INTEGER)").unwrap();
            donor_conn.execute("INSERT INTO t VALUES (22)").unwrap();
            donor_conn
                .execute("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
        }

        replace_db_files(&donor_path, &target_path, true);
        target_db
            .reload_wal_after_external_restore()
            .unwrap_or_else(|e| panic!("failed to reload {donor_page_size}-byte donor: {e}"));

        assert_eq!(
            ints(&warmed, "SELECT v FROM t"),
            vec![22],
            "warmed connection failed after {donor_page_size}-byte page-size restore"
        );
        let fresh = target_db.connect(ConnectOpts::new()).unwrap();
        assert_eq!(
            ints(&fresh, "SELECT v FROM t"),
            vec![22],
            "fresh connection failed after {donor_page_size}-byte page-size restore"
        );
    }

    // 2. AES-256-GCM encrypted target + donor using the same key (tested via both
    //    `Database::reload_wal_after_external_restore` and `Connection::reload_wal_after_external_restore`).
    {
        let dir = tempfile::tempdir().unwrap();
        let target_path = dir.path().join("enc_target.db");
        let donor_path = dir.path().join("enc_donor.db");
        let hexkey = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let enc_opts = EncryptionOpts {
            cipher: "aes256gcm".to_string(),
            hexkey: hexkey.to_string(),
        };
        let key = EncryptionKey::from_hex_string(hexkey).unwrap();

        let open_enc = |path: &Path| {
            let io = Arc::new(PlatformIO::new().unwrap());
            let options = OpenOptions::new(Arc::new(SqliteDialect))
                .db_opts(DatabaseOpts::new().with_encryption(true))
                .encryption(Some(enc_opts.clone()));
            Database::open(io, path.to_str().unwrap(), options).unwrap()
        };

        let target_db = open_enc(&target_path);
        let warmed = target_db
            .connect(ConnectOpts::new().with_encryption_key(Some(key.clone())))
            .unwrap();
        warmed.execute("CREATE TABLE t(v INTEGER)").unwrap();
        warmed.execute("INSERT INTO t VALUES (11)").unwrap();
        warmed.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        assert_eq!(ints(&warmed, "SELECT v FROM t"), vec![11]);

        {
            let donor_db = open_enc(&donor_path);
            let donor_conn = donor_db
                .connect(ConnectOpts::new().with_encryption_key(Some(key.clone())))
                .unwrap();
            donor_conn.execute("CREATE TABLE t(v INTEGER)").unwrap();
            donor_conn.execute("INSERT INTO t VALUES (22)").unwrap();
            donor_conn
                .execute("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
        }

        replace_db_files(&donor_path, &target_path, true);
        warmed.reload_wal_after_external_restore().unwrap();
        assert_eq!(ints(&warmed, "SELECT v FROM t"), vec![22]);

        let fresh = target_db
            .connect(ConnectOpts::new().with_encryption_key(Some(key)))
            .unwrap();
        assert_eq!(ints(&fresh, "SELECT v FROM t"), vec![22]);

        // Also verify `Database::reload_wal_after_external_restore()` works directly.
        target_db.reload_wal_after_external_restore().unwrap();
        assert_eq!(ints(&warmed, "SELECT v FROM t"), vec![22]);
    }

    // 3. Corrupt candidate image must fail during staging without clobbering the live WAL or
    //    warmed connection state.
    {
        let dir = tempfile::tempdir().unwrap();
        let target_path = dir.path().join("live.db");

        let target_db = open_db(&target_path, true);
        let warmed = target_db.connect(ConnectOpts::new()).unwrap();
        warmed.execute("PRAGMA wal_autocheckpoint = 0").unwrap();
        warmed.execute("CREATE TABLE t(v INTEGER)").unwrap();
        warmed.execute("INSERT INTO t VALUES (11)").unwrap();
        assert_eq!(ints(&warmed, "SELECT v FROM t"), vec![11]);

        // Atomic-rename a corrupt image into place while live handle still holds the original fd
        // and WAL; reload must reject it before mutating live WAL/fd/pool/registry state.
        let staged = dir.path().join("corrupt.staged");
        std::fs::write(&staged, vec![0xFFu8; 4096]).unwrap();
        std::fs::rename(&staged, &target_path).unwrap();

        assert!(
            target_db.reload_wal_after_external_restore().is_err(),
            "corrupt image must be rejected during staging"
        );
        // Live state (WAL + original file descriptor) was NOT replaced, so warmed queries still
        // see [11].
        assert_eq!(ints(&warmed, "SELECT v FROM t"), vec![11]);
    }
}
