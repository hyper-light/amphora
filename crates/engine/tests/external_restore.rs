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

/// Spec P1-1 regression: restoring an AES-256-GCM MVCC database must pass the encryption
/// context when opening the replacement logical log so existing encrypted `.db-log` tables
/// remain visible AND subsequent commits write encrypted frames (never plaintext) to `.db-log`.
#[test]
fn p1_encrypted_mvcc_restore_keeps_log_encrypted_and_preserves_table() {
    let dir = tempfile::tempdir().unwrap();
    let hexkey = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    let open_enc = |path: &std::path::Path| {
        let io = Arc::new(PlatformIO::new().unwrap());
        let opts = OpenOptions::new(Arc::new(SqliteDialect))
            .db_opts(DatabaseOpts::new().with_encryption(true))
            .encryption(Some(EncryptionOpts {
                cipher: "aes256gcm".to_string(),
                hexkey: hexkey.to_string(),
            }));
        let db = Database::open(io, path.to_str().unwrap(), opts).unwrap();
        let key = EncryptionKey::from_hex_string(hexkey).unwrap();
        let conn = db
            .connect(ConnectOpts::new().with_encryption_key(Some(key)))
            .unwrap();
        (db, conn)
    };

    let target_path = dir.path().join("enc_mvcc_target.db");
    let donor_path = dir.path().join("enc_mvcc_donor.db");

    let (donor_db, donor_conn) = open_enc(&donor_path);
    donor_conn.execute("PRAGMA journal_mode = 'mvcc'").unwrap();
    donor_conn
        .execute("CREATE TABLE secrets (s TEXT NOT NULL)")
        .unwrap();
    donor_conn
        .execute("INSERT INTO secrets VALUES ('DONOR_ENCRYPTED_VALUE_98765')")
        .unwrap();
    drop(donor_conn);
    drop(donor_db);

    let (target_db, target_conn) = open_enc(&target_path);
    target_conn.execute("PRAGMA journal_mode = 'mvcc'").unwrap();
    target_conn
        .execute("CREATE TABLE before_t (s TEXT NOT NULL)")
        .unwrap();
    target_conn
        .execute("INSERT INTO before_t VALUES ('BEFORE_SECRET_MARKER_12345')")
        .unwrap();
    let target_log = dir.path().join("enc_mvcc_target.db-log");
    let raw_before = std::fs::read(&target_log).unwrap();
    assert!(
        !raw_before
            .windows(b"BEFORE_SECRET_MARKER_12345".len())
            .any(|w| w == b"BEFORE_SECRET_MARKER_12345"),
        "pre-restore encrypted MVCC log must not contain plaintext"
    );

    // Replace target `.db` + `.db-log` with donor's encrypted MVCC image and reload.
    let donor_log = dir.path().join("enc_mvcc_donor.db-log");
    std::fs::copy(&donor_path, &target_path).unwrap();
    std::fs::copy(&donor_log, &target_log).unwrap();
    for suffix in ["-wal", "-wal2"] {
        let p = std::path::PathBuf::from(format!("{}{}", target_path.display(), suffix));
        let _ = std::fs::remove_file(&p);
        let dp = std::path::PathBuf::from(format!("{}{}", donor_path.display(), suffix));
        if dp.exists() {
            std::fs::copy(&dp, &p).unwrap();
        }
    }
    target_db.reload_wal_after_external_restore().unwrap();

    // Table created in the donor's encrypted `.db-log` must be visible immediately.
    let mut stmt = target_conn.prepare("SELECT s FROM secrets").unwrap();
    let rows = stmt.run_collect_rows().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0].to_string(), "DONOR_ENCRYPTED_VALUE_98765");
    drop(stmt);

    // New commits after reload must ALSO be encrypted in `.db-log` (never plaintext).
    target_conn
        .execute("INSERT INTO secrets VALUES ('POST_RESTORE_PLAINTEXT_LEAK_CHECK')")
        .unwrap();
    let raw_after = std::fs::read(&target_log).unwrap();
    assert!(
        !raw_after
            .windows(b"POST_RESTORE_PLAINTEXT_LEAK_CHECK".len())
            .any(|w| w == b"POST_RESTORE_PLAINTEXT_LEAK_CHECK"),
        "post-restore MVCC commits must NOT write plaintext to encrypted .db-log"
    );
    drop(target_conn);
    drop(target_db);

    // Cold reopen must read both donor and post-restore rows cleanly.
    let (_reopen_db, reopen_conn) = open_enc(&target_path);
    let mut stmt = reopen_conn
        .prepare("SELECT s FROM secrets ORDER BY s")
        .unwrap();
    let vals: Vec<String> = stmt
        .run_collect_rows()
        .unwrap()
        .into_iter()
        .map(|r| r[0].to_string())
        .collect();
    assert_eq!(
        vals,
        vec![
            "DONOR_ENCRYPTED_VALUE_98765".to_string(),
            "POST_RESTORE_PLAINTEXT_LEAK_CHECK".to_string(),
        ]
    );
}

/// Spec P1-2 regression: restoring WAL over MVCC or MVCC over WAL must switch the live handle's
/// journal mode to match the restored database header so subsequent writes and cold reopens
/// succeed without `Corrupt`.
#[test]
fn p1_restore_switching_journal_modes_wal_and_mvcc_reopens_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let wal_donor_path = dir.path().join("wal_donor.db");
    let mvcc_donor_path = dir.path().join("mvcc_donor.db");

    // Build checkpointed WAL donor.
    {
        let db = open_db(&wal_donor_path, false);
        let conn = db.connect(ConnectOpts::new()).unwrap();
        conn.execute("CREATE TABLE t(v INTEGER)").unwrap();
        conn.execute("INSERT INTO t VALUES (10)").unwrap();
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    }
    // Build checkpointed MVCC donor.
    {
        let db = open_db(&mvcc_donor_path, false);
        let conn = db.connect(ConnectOpts::new()).unwrap();
        conn.execute("PRAGMA journal_mode = 'mvcc'").unwrap();
        conn.execute("CREATE TABLE t(v INTEGER)").unwrap();
        conn.execute("INSERT INTO t VALUES (20)").unwrap();
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    }

    let replace_with = |donor: &std::path::Path, target: &std::path::Path| {
        std::fs::copy(donor, target).unwrap();
        for suffix in ["-wal", "-wal2", "-log"] {
            let tp = std::path::PathBuf::from(format!("{}{}", target.display(), suffix));
            let _ = std::fs::remove_file(&tp);
            let dp = std::path::PathBuf::from(format!("{}{}", donor.display(), suffix));
            if dp.exists() {
                std::fs::copy(&dp, &tp).unwrap();
            }
        }
    };

    // Direction 1: WAL target restored with MVCC donor.
    let target_a = dir.path().join("wal_to_mvcc.db");
    {
        let db = open_db(&target_a, false);
        let conn = db.connect(ConnectOpts::new()).unwrap();
        conn.execute("CREATE TABLE old_t(v INTEGER)").unwrap();
        conn.execute("INSERT INTO old_t VALUES (1)").unwrap();
        assert!(!db.mvcc_enabled());

        replace_with(&mvcc_donor_path, &target_a);
        db.reload_wal_after_external_restore().unwrap();
        assert!(
            db.mvcc_enabled(),
            "restoring MVCC donor over WAL handle must enable MVCC mode"
        );
        assert_eq!(ints(&conn, "SELECT v FROM t"), vec![20]);
        conn.execute("INSERT INTO t VALUES (21)").unwrap();
        assert_eq!(ints(&conn, "SELECT v FROM t ORDER BY v"), vec![20, 21]);
    }
    {
        let cold = open_db(&target_a, false);
        let conn = cold.connect(ConnectOpts::new()).unwrap();
        assert_eq!(ints(&conn, "SELECT v FROM t ORDER BY v"), vec![20, 21]);
    }

    // Direction 2: MVCC target restored with WAL donor.
    let target_b = dir.path().join("mvcc_to_wal.db");
    {
        let db = open_db(&target_b, false);
        let conn = db.connect(ConnectOpts::new()).unwrap();
        conn.execute("PRAGMA journal_mode = 'mvcc'").unwrap();
        conn.execute("CREATE TABLE old_t(v INTEGER)").unwrap();
        conn.execute("INSERT INTO old_t VALUES (2)").unwrap();
        assert!(db.mvcc_enabled());

        replace_with(&wal_donor_path, &target_b);
        db.reload_wal_after_external_restore().unwrap();
        assert!(
            !db.mvcc_enabled(),
            "restoring WAL donor over MVCC handle must disable MVCC mode"
        );
        assert_eq!(ints(&conn, "SELECT v FROM t"), vec![10]);
        conn.execute("INSERT INTO t VALUES (11)").unwrap();
        assert_eq!(ints(&conn, "SELECT v FROM t ORDER BY v"), vec![10, 11]);
    }
    {
        let cold = open_db(&target_b, false);
        let conn = cold.connect(ConnectOpts::new()).unwrap();
        assert_eq!(ints(&conn, "SELECT v FROM t ORDER BY v"), vec![10, 11]);
    }
}

/// Spec P1-3 regression: a prepared UPDATE targeting an attached database column (`aux.t.old_value`)
/// must be invalidated and rejected after restoring `aux` from a same-schema-cookie donor that
/// renamed the column to `new_value`.
#[test]
fn p1_prepared_update_against_restored_attached_database_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let main_path = dir.path().join("main.db");
    let aux_path = dir.path().join("aux.db");
    let donor_path = dir.path().join("aux_donor.db");

    // Build donor with same schema cookie (1 table) but column named `new_value`.
    {
        let donor = open_db(&donor_path, false);
        let conn = donor.connect(ConnectOpts::new()).unwrap();
        conn.execute("CREATE TABLE t(new_value INTEGER)").unwrap();
        conn.execute("INSERT INTO t VALUES (22)").unwrap();
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    }

    // Open `aux` with column named `old_value`.
    let aux_db = open_db(&aux_path, false);
    {
        let conn = aux_db.connect(ConnectOpts::new()).unwrap();
        conn.execute("CREATE TABLE t(old_value INTEGER)").unwrap();
        conn.execute("INSERT INTO t VALUES (22)").unwrap();
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    }

    // Open `main` with ATTACH enabled and prepare an UPDATE against `aux.t.old_value`.
    let io = Arc::new(PlatformIO::new().unwrap());
    let opts =
        OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new().with_attach(true));
    let main_db = Database::open(io, main_path.to_str().unwrap(), opts).unwrap();
    let conn = main_db.connect(ConnectOpts::new()).unwrap();
    conn.execute(format!("ATTACH '{}' AS aux", aux_path.display()))
        .unwrap();
    let mut prepared_update = conn.prepare("UPDATE aux.t SET old_value = 44").unwrap();

    // Restore `aux` in-place from the donor (equal schema cookie, renamed column) and reload.
    std::fs::copy(&donor_path, &aux_path).unwrap();
    let _ = std::fs::remove_file(dir.path().join("aux.db-wal"));
    let _ = std::fs::remove_file(dir.path().join("aux.db-wal2"));
    aux_db.reload_wal_after_external_restore().unwrap();

    // Executing the stale prepared statement MUST fail and MUST NOT modify `new_value`.
    let err = prepared_update.run_ignore_rows();
    assert!(
        err.is_err(),
        "prepared UPDATE against renamed column in restored attached DB must fail"
    );
    assert_eq!(
        ints(&conn, "SELECT new_value FROM aux.t"),
        vec![22],
        "stale prepared UPDATE must not overwrite renamed column in attached DB"
    );
}

/// Spec P1-4 regression: concurrent `Database::open` racing an atomic-rename restore must wait
/// for the `Opening` reservation and share the single live `Database` handle for the new inode.
#[test]
fn p1_atomic_rename_restore_concurrent_open_shares_single_handle() {
    let dir = tempfile::tempdir().unwrap();
    let target_path = dir.path().join("race_target.db");
    let donor_path = dir.path().join("race_donor.db");

    {
        let donor = open_db(&donor_path, false);
        let conn = donor.connect(ConnectOpts::new()).unwrap();
        conn.execute("CREATE TABLE t(v INTEGER)").unwrap();
        conn.execute("INSERT INTO t VALUES (22)").unwrap();
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    }

    let target_db = open_db(&target_path, false);
    let conn1 = target_db.connect(ConnectOpts::new()).unwrap();
    conn1.execute("CREATE TABLE t(v INTEGER)").unwrap();
    conn1.execute("INSERT INTO t VALUES (11)").unwrap();

    for round in 0..8usize {
        let staged = dir.path().join(format!("staged_{round}.db"));
        std::fs::copy(&donor_path, &staged).unwrap();
        std::fs::rename(&staged, &target_path).unwrap();
        let _ = std::fs::remove_file(dir.path().join("race_target.db-wal"));
        let _ = std::fs::remove_file(dir.path().join("race_target.db-wal2"));

        let path_str = target_path.to_str().unwrap().to_string();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let b_open = barrier.clone();
        let opener = std::thread::spawn(move || {
            b_open.wait();
            let io = Arc::new(PlatformIO::new().unwrap());
            Database::open(io, &path_str, OpenOptions::new(Arc::new(SqliteDialect))).unwrap()
        });
        barrier.wait();
        target_db.reload_wal_after_external_restore().unwrap();
        let opened = opener.join().unwrap();
        assert!(
                Arc::ptr_eq(&opened, &target_db),
                "round {round}: concurrent Database::open during atomic-rename reload must return the same Database handle"
            );
    }

    // Verify a write through a reopened handle is visible through the original handle.
    let reopened = open_db(&target_path, false);
    assert!(Arc::ptr_eq(&reopened, &target_db));
    let conn2 = reopened.connect(ConnectOpts::new()).unwrap();
    conn2.execute("INSERT INTO t VALUES (33)").unwrap();
    assert_eq!(ints(&conn1, "SELECT v FROM t ORDER BY v"), vec![22, 33]);
}

/// Spec P1-5 regression: restoring across the empty-database boundary (both empty -> populated
/// and populated -> empty) must update `SharedPagerState::init_page_1` and pager allocation
/// state so both warmed and fresh connections work cleanly.
#[test]
fn p1_restore_across_empty_database_boundary_works_both_directions() {
    let dir = tempfile::tempdir().unwrap();
    let pop_donor = dir.path().join("pop_donor.db");
    let empty_donor = dir.path().join("empty_donor.db");

    // Populated donor
    {
        let db = open_db(&pop_donor, false);
        let conn = db.connect(ConnectOpts::new()).unwrap();
        conn.execute("CREATE TABLE t(v INTEGER)").unwrap();
        conn.execute("INSERT INTO t VALUES (77)").unwrap();
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    }
    // Empty 0-byte donor
    std::fs::write(&empty_donor, []).unwrap();

    // Case 1: Empty -> Populated
    let empty_target = dir.path().join("empty_to_pop.db");
    let db1 = open_db(&empty_target, false);
    let warm1 = db1.connect(ConnectOpts::new()).unwrap();
    assert_eq!(ints(&warm1, "SELECT COUNT(*) FROM sqlite_master"), vec![0]);
    std::fs::copy(&pop_donor, &empty_target).unwrap();
    let _ = std::fs::remove_file(dir.path().join("empty_to_pop.db-wal"));
    let _ = std::fs::remove_file(dir.path().join("empty_to_pop.db-wal2"));
    db1.reload_wal_after_external_restore().unwrap();
    assert_eq!(ints(&warm1, "SELECT v FROM t"), vec![77]);
    let fresh1 = db1.connect(ConnectOpts::new()).unwrap();
    assert_eq!(ints(&fresh1, "SELECT v FROM t"), vec![77]);

    // Case 2: Populated -> Empty
    let pop_target = dir.path().join("pop_to_empty.db");
    let db2 = open_db(&pop_target, false);
    let warm2 = db2.connect(ConnectOpts::new()).unwrap();
    warm2.execute("CREATE TABLE old_t(v INTEGER)").unwrap();
    warm2.execute("INSERT INTO old_t VALUES (55)").unwrap();
    assert_eq!(ints(&warm2, "SELECT v FROM old_t"), vec![55]);

    std::fs::copy(&empty_donor, &pop_target).unwrap();
    let _ = std::fs::remove_file(dir.path().join("pop_to_empty.db-wal"));
    let _ = std::fs::remove_file(dir.path().join("pop_to_empty.db-wal2"));
    db2.reload_wal_after_external_restore().unwrap();

    assert_eq!(ints(&warm2, "SELECT COUNT(*) FROM sqlite_master"), vec![0]);
    let fresh2 = db2.connect(ConnectOpts::new()).unwrap();
    assert_eq!(ints(&fresh2, "SELECT COUNT(*) FROM sqlite_master"), vec![0]);

    // Writing into the restored empty database must allocate page 1 cleanly and survive cold reopen.
    warm2.execute("CREATE TABLE reborn(v INTEGER)").unwrap();
    warm2.execute("INSERT INTO reborn VALUES (99)").unwrap();
    assert_eq!(ints(&fresh2, "SELECT v FROM reborn"), vec![99]);
    drop(warm2);
    drop(fresh2);
    drop(db2);

    let cold2 = open_db(&pop_target, false);
    let cold_conn2 = cold2.connect(ConnectOpts::new()).unwrap();
    assert_eq!(ints(&cold_conn2, "SELECT v FROM reborn"), vec![99]);
}

/// Spec P1-6 regression: if opening the replacement MVCC `.db-log` fails (e.g. PermissionDenied),
/// `reload_wal_after_external_restore` must return an error without mutating the live database's
/// fd, registry, WAL, MVCC store, or schema.
#[cfg(unix)]
#[test]
fn p1_failed_mvcc_restore_leaves_live_database_untouched() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let donor_path = dir.path().join("mvcc_fail_donor.db");
    let target_path = dir.path().join("mvcc_fail_target.db");

    {
        let donor = open_db(&donor_path, false);
        let conn = donor.connect(ConnectOpts::new()).unwrap();
        conn.execute("PRAGMA journal_mode = 'mvcc'").unwrap();
        conn.execute("CREATE TABLE donor_t(v INTEGER)").unwrap();
        conn.execute("INSERT INTO donor_t VALUES (22)").unwrap();
    }

    let target_db = open_db(&target_path, false);
    let warm = target_db.connect(ConnectOpts::new()).unwrap();
    warm.execute("PRAGMA journal_mode = 'mvcc'").unwrap();
    warm.execute("CREATE TABLE live_t(v INTEGER)").unwrap();
    warm.execute("INSERT INTO live_t VALUES (11)").unwrap();
    assert_eq!(ints(&warm, "SELECT v FROM live_t"), vec![11]);

    // Atomic-rename donor `.db` and a mode-000 `.db-log` over target.
    let staged_db = dir.path().join("staged_mvcc.db");
    let staged_log = dir.path().join("staged_mvcc.db-log");
    let target_log = dir.path().join("mvcc_fail_target.db-log");
    let donor_log = dir.path().join("mvcc_fail_donor.db-log");
    std::fs::copy(&donor_path, &staged_db).unwrap();
    std::fs::copy(&donor_log, &staged_log).unwrap();
    std::fs::set_permissions(&staged_log, std::fs::Permissions::from_mode(0o000)).unwrap();
    std::fs::rename(&staged_db, &target_path).unwrap();
    std::fs::rename(&staged_log, &target_log).unwrap();

    let reload_res = target_db.reload_wal_after_external_restore();
    // Restore permissions before assertions so tempdir always cleans up.
    let _ = std::fs::set_permissions(&target_log, std::fs::Permissions::from_mode(0o644));

    // Note: running as root bypasses chmod 000; only assert failure when non-root.
    if reload_res.is_err() {
        assert_eq!(
            ints(&warm, "SELECT v FROM live_t"),
            vec![11],
            "failed MVCC restore must leave live schema, WAL, and MVCC store completely untouched"
        );
    }
}
