//! What plain SQLite does to an Amphora database whose log may be two files (Amphora's
//! `docs/design/store.md`, decision 45, "What plain SQLite sees"): it knows `-wal` alone, so it reads
//! and writes nothing of a database whose commits it can't see, and reads one closed cleanly whole.
//! SQLite itself is asked where its shell is here (`sqlite3`; the cases were written against 3.54.0,
//! 2026-10-09). What it goes by is asserted everywhere ([`Files::refused`]).

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, LogFormat, OpenOptions, PlatformIO,
    SqliteDialect,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

const SUFFIXES: [&str; 3] = ["", "-wal", "-wal2"];

fn beside(dir: &Path, suffix: &str) -> PathBuf {
    dir.join(format!("t.db{suffix}"))
}

fn open(dir: &Path) -> (Arc<Database>, Arc<Connection>) {
    let options =
        OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new().with_vacuum(true));
    let io = Arc::new(PlatformIO::new().unwrap());
    let db = Database::open(io, beside(dir, "").to_str().unwrap(), options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    conn.execute("PRAGMA wal_autocheckpoint = 0").unwrap();
    (db, conn)
}

fn rows(conn: &Arc<Connection>, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap();
    let rows = stmt.run_collect_rows().unwrap();
    rows.iter()
        .map(|row| {
            row.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect()
}

fn insert(conn: &Arc<Connection>, from: u32, to: u32) {
    for row in from..=to {
        conn.execute(format!("INSERT INTO t VALUES({row}, 'amphora')"))
            .unwrap();
    }
}

/// The closing segment's pass, and then the current one's.
fn copy(conn: &Arc<Connection>) {
    for _ in 0..2 {
        rows(conn, "PRAGMA wal_checkpoint(PASSIVE)");
    }
}

/// The sqlite3 shell's answer to `sql`: whether it ran it, and what it printed. None without a shell.
fn sqlite3(dir: &Path, sql: &str) -> Option<(bool, String)> {
    let ran = Command::new("sqlite3")
        .args(["-batch", beside(dir, "").to_str().unwrap(), sql])
        .output();
    let ran = match ran {
        Ok(ran) => ran,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => panic!("sqlite3: {e}"),
    };
    let out = String::from_utf8_lossy(&ran.stdout).trim().to_owned();
    let err = String::from_utf8_lossy(&ran.stderr).trim().to_owned();
    Some((ran.status.success(), format!("{out}{err}")))
}

/// A database's files as a process that ended left them, or as someone carried them off.
struct Files {
    dir: tempfile::TempDir,
}

impl Files {
    fn of(dir: &Path, without: &[&str]) -> Self {
        let files = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        for suffix in SUFFIXES {
            if beside(dir, suffix).exists() && !without.contains(&suffix) {
                std::fs::copy(beside(dir, suffix), beside(files.dir.path(), suffix)).unwrap();
            }
        }
        files
    }

    fn bytes(&self) -> Vec<Option<Vec<u8>>> {
        SUFFIXES
            .iter()
            .map(|suffix| std::fs::read(beside(self.dir.path(), suffix)).ok())
            .collect()
    }

    /// Whether SQLite refuses these files, by what it goes by: a log whose header is valid and whose
    /// format isn't 3007000 (`wal.c`, `walIndexRecover`), else page 1's read version, above 2
    /// (`btree.c`, `lockBtree`; fileformat2.html, 1.3.3), of the log's last frame of page 1 where the
    /// log has one.
    fn refused(&self) -> bool {
        let file = std::fs::read(beside(self.dir.path(), "")).unwrap();
        let log = std::fs::read(beside(self.dir.path(), "-wal")).unwrap_or_default();
        let field = |at: usize| u32::from_be_bytes(log[at..at + 4].try_into().unwrap());
        let mut version = file[19];
        if log.len() >= 32 && field(0) & !1 == 0x377f_0682 {
            if field(4) != 3_007_000 {
                return true;
            }
            let page = field(8) as usize;
            for frame in log[32..].chunks_exact(24 + page) {
                let salts = frame[8..16] == log[16..24];
                let first = u32::from_be_bytes(frame[0..4].try_into().unwrap()) == 1;
                if salts && first {
                    version = frame[24 + 19];
                }
            }
        }
        version > 2
    }

    /// SQLite's visit, a read and then a write: what it ran of the two, and what it printed.
    fn visited(&self) -> Vec<String> {
        [
            "SELECT count(*) FROM t",
            "INSERT INTO t VALUES(1000, 'sqlite')",
        ]
        .into_iter()
        .filter_map(|sql| Some((sql, sqlite3(self.dir.path(), sql)?)))
        .filter(|(_, (ran, printed))| {
            *ran || !(printed.contains("file is not a database")
                || printed.contains("unable to open database file"))
        })
        .map(|(sql, (_, printed))| format!("`{sql}`: {printed}"))
        .collect()
    }

    /// SQLite is kept out of these files, by itself where its shell is here and by what it goes by.
    fn refuses(&self, at: &str) {
        let refused = self.refused();
        let ran = self.visited();
        assert!(ran.is_empty(), "{at}: sqlite3 ran {ran:?}");
        assert!(refused, "{at}: nothing in these files keeps SQLite out");
    }

    /// What Amphora finds here: how many rows, and what its integrity check says.
    fn found(&self) -> Result<(String, String), String> {
        let found = std::panic::catch_unwind(|| {
            let (_db, conn) = open(self.dir.path());
            let count = rows(&conn, "SELECT count(*) FROM t").join(",");
            (count, rows(&conn, "PRAGMA integrity_check").join(","))
        });
        found.map_err(|e| match e.downcast_ref::<String>() {
            Some(said) => said.clone(),
            None => "a panic".to_owned(),
        })
    }

    fn holds(&self, rows_there: u32, at: &str) {
        let whole = (rows_there.to_string(), "ok".to_owned());
        assert_eq!(self.found(), Ok(whole), "{at}: the rows committed");
    }
}

/// How the log stands as the process ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Log {
    /// The second segment is current, its commits not in the file; the first's are.
    Second,
    /// The second segment is current; nothing of the first one is in the file yet either.
    SecondUncopied,
    /// The first segment is current again, the second closing with commits not in the file.
    First,
    /// The first segment is current and cut to nothing, no commit in it yet; the second closing with
    /// commits not in the file.
    FirstCut,
}

const LOGS: [Log; 4] = [Log::Second, Log::SecondUncopied, Log::First, Log::FirstCut];
const FORMATS: [LogFormat; 2] = [LogFormat::Mixed, LogFormat::Pages];

/// A database left with its log standing as `log`: its folder, and the rows committed.
fn left(log: Log, format: LogFormat) -> (tempfile::TempDir, Arc<Database>, Arc<Connection>, u32) {
    let dir = tempfile::tempdir().unwrap();
    let (db, conn) = open(dir.path());
    db.wal_format(format).unwrap();
    conn.execute("CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT)")
        .unwrap();
    insert(&conn, 1, 3);
    if log != Log::SecondUncopied {
        copy(&conn);
    }
    assert!(db.switch_log(None).unwrap(), "{log:?}");
    if log != Log::SecondUncopied {
        copy(&conn);
    }
    insert(&conn, 4, 6);
    match log {
        Log::Second | Log::SecondUncopied => (dir, db, conn, 6),
        Log::First => {
            assert!(db.switch_log(None).unwrap(), "{log:?}");
            insert(&conn, 7, 9);
            (dir, db, conn, 9)
        }
        Log::FirstCut => {
            assert!(db.switch_log(Some(0)).unwrap(), "{log:?}");
            assert_eq!(
                std::fs::metadata(beside(dir.path(), "-wal")).unwrap().len(),
                0
            );
            (dir, db, conn, 6)
        }
    }
}

/// Do: end a process with commits only `-wal2` holds, its log standing four ways; then read and write
/// the files it left with sqlite3, and open them with Amphora. Expect: sqlite3 refuses both, and
/// Amphora finds every row it committed, in a whole database. Without the guard sqlite3 answers an
/// older count, takes the write, and folds and deletes `-wal`: Amphora then finds fewer rows.
#[test]
fn d45_sqlite3_never_reads_or_writes_a_database_a_crash_left() {
    let mut wrong = Vec::new();
    for format in FORMATS {
        for log in LOGS {
            let at = format!("{format:?}, {log:?}");
            let (dir, _db, _conn, there) = left(log, format);
            let files = Files::of(dir.path(), &[]);
            let (before, refused) = (files.bytes(), files.refused());
            let ran = files.visited();
            let after = files.bytes();
            let found = files.found();
            // A first segment of SQLite's own format holds only frames older than the second's:
            // SQLite folds it into the file as it closes, as Amphora would.
            let sqlites_own =
                format == LogFormat::Pages && matches!(log, Log::Second | Log::SecondUncopied);
            let changed = after != before && !sqlites_own;
            let whole = (there.to_string(), "ok".to_owned());
            if found.as_ref() != Ok(&whole) || !ran.is_empty() || changed || !refused {
                wrong.push(format!(
                    "{at}: {there} rows committed, Amphora finds {found:?} after sqlite3's visit; \
                     sqlite3 ran {ran:?}; a file it refused changed: {changed}; refused by what it \
                     goes by: {refused}"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Do: carry off a database a process left without `-wal2`, without `-wal`, and without both; read
/// and write it with sqlite3. Expect: refused each time, as a part of the database is all SQLite
/// would see.
#[test]
fn d45_sqlite3_refuses_a_database_carried_off_without_its_log() {
    for format in FORMATS {
        for log in LOGS {
            for without in [&["-wal2"][..], &["-wal"], &["-wal", "-wal2"]] {
                let (dir, _db, _conn, _) = left(log, format);
                let files = Files::of(dir.path(), without);
                if log == Log::SecondUncopied && without == ["-wal", "-wal2"] {
                    // The file holds no page but its first; SQLite's own rule is the same for a
                    // file carried off without its log.
                    assert!(files.refused());
                }
                files.refuses(&format!("{format:?}, {log:?}, without {without:?}"));
            }
        }
    }
}

/// Do: close a database cleanly, its log standing four ways; read and write its file with sqlite3;
/// open it with Amphora, write and switch, and end there. Expect: sqlite3 reads every row and is
/// refused the write (a page it wrote would fail its checksum in Amphora, decision 12: without the
/// guard Amphora no longer opens the database), Amphora finds every row, and sqlite3 is refused once
/// Amphora's commits need `-wal2`.
#[test]
fn d45_sqlite3_reads_a_cleanly_closed_database_whole_and_never_writes_it() {
    for format in FORMATS {
        for log in LOGS {
            let at = format!("{format:?}, {log:?}");
            let (dir, db, conn, there) = left(log, format);
            conn.close().unwrap();
            drop((conn, db));
            let files = Files::of(dir.path(), &[]);
            if let Some(read) = sqlite3(files.dir.path(), "SELECT count(*) FROM t") {
                assert_eq!(read, (true, there.to_string()), "{at}");
                let sql = "INSERT INTO t VALUES(1000, 'sqlite')";
                let (ran, printed) = sqlite3(files.dir.path(), sql).unwrap();
                files.holds(
                    there,
                    &format!("{at}: sqlite3 ran `{sql}`: {ran}, {printed}"),
                );
                assert!(!ran && printed.contains("readonly"), "{at}: {printed}");
            }
            assert!(
                !files.refused(),
                "{at}: a cleanly closed database is SQLite's to read"
            );
            let file = std::fs::read(beside(files.dir.path(), "")).unwrap();
            assert_eq!(file[18..20], [3, 2], "{at}: its write and read versions");
            let log_bytes: Vec<usize> = files.bytes()[1..]
                .iter()
                .map(|file| file.as_ref().map_or(0, Vec::len))
                .collect();
            assert_eq!(log_bytes, [0, 0], "{at}");
            let (db, conn) = open(files.dir.path());
            db.wal_format(format).unwrap();
            assert_eq!(
                rows(&conn, "SELECT count(*) FROM t"),
                [there.to_string()],
                "{at}"
            );
            insert(&conn, 2001, 2003);
            copy(&conn);
            assert!(db.switch_log(None).unwrap(), "{at}");
            insert(&conn, 2004, 2006);
            let left = Files::of(files.dir.path(), &[]);
            left.refuses(&at);
            left.holds(there + 6, &at);
        }
    }
}

/// Do: end a process whose database's log never had a second segment; read and write its files with
/// sqlite3, and open them with Amphora. Expect: sqlite3 sees every row or none, and Amphora finds
/// every row committed, sqlite3's too where it took the write.
#[test]
fn d45_a_database_of_one_segment_is_whole_to_sqlite3_or_refused() {
    for format in FORMATS {
        let dir = tempfile::tempdir().unwrap();
        let (db, conn) = open(dir.path());
        db.wal_format(format).unwrap();
        conn.execute("CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT)")
            .unwrap();
        insert(&conn, 1, 3);
        let files = Files::of(dir.path(), &[]);
        let mut there = 3;
        if let Some((true, count)) = sqlite3(files.dir.path(), "SELECT count(*) FROM t") {
            assert_eq!(count, "3", "{format:?}");
            let wrote = sqlite3(files.dir.path(), "INSERT INTO t VALUES(1000, 'sqlite')");
            assert_eq!(wrote, Some((true, String::new())), "{format:?}");
            there += 1;
        }
        files.holds(there, &format!("{format:?}"));
    }
}

/// Do: leave a WAL database as SQLite leaves one, its log not folded (its own format, its frames of
/// page 1 saying version 2); open it with Amphora; checkpoint (RESTART, TRUNCATE) before any write and
/// after; write past two restarts; end there. Expect: Amphora reads every row, no second segment is
/// written before the file itself refuses SQLite, and at the end sqlite3 is refused and Amphora whole.
#[test]
fn d45_a_database_sqlite3_left_is_marked_before_its_second_segment_is_written() {
    let made = tempfile::tempdir().unwrap();
    let script = "PRAGMA journal_mode=wal; PRAGMA wal_autocheckpoint=0; \
                  CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT); \
                  INSERT INTO t VALUES(1,'sqlite'),(2,'sqlite'),(3,'sqlite');";
    let path = beside(made.path(), "");
    let shell = Command::new("sqlite3")
        .args(["-batch", path.to_str().unwrap()])
        .args([".dbconfig no_ckpt_on_close on", script])
        .output();
    let Ok(shell) = shell else {
        return;
    };
    assert!(shell.status.success(), "{shell:?}");
    assert!(
        std::fs::metadata(beside(made.path(), "-wal"))
            .unwrap()
            .len()
            > 0
    );
    for first in [
        "SELECT 1",
        "PRAGMA wal_checkpoint(RESTART)",
        "PRAGMA wal_checkpoint(TRUNCATE)",
    ] {
        let files = Files::of(made.path(), &[]);
        let dir = files.dir.path();
        {
            let (db, conn) = open(dir);
            assert_eq!(rows(&conn, "SELECT count(*) FROM t"), ["3"], "{first}");
            rows(&conn, first);
            for row in 4..24 {
                insert(&conn, row, row);
                copy(&conn);
                if row % 5 == 0 {
                    rows(&conn, "PRAGMA wal_checkpoint(RESTART)");
                }
                let at = format!("{first}, row {row}");
                let left = Files::of(dir, &[]);
                let second = left.bytes()[2].as_ref().map_or(0, Vec::len);
                assert!(
                    second == 0 || left.refused(),
                    "{at}: `-wal2` holds {second} bytes"
                );
                left.holds(row, &at);
            }
            insert(&conn, 24, 30);
            drop(db);
        }
        let left = Files::of(dir, &[]);
        assert!(
            left.bytes()[2].as_ref().is_some_and(|b| !b.is_empty()),
            "{first}"
        );
        left.refuses(first);
        left.holds(30, first);
    }
}

/// Do: set a log's format and commit past two switches. Expect: every generation is in that format, the
/// second segment's as the first's (lane B30, item 2).
#[test]
fn d45_a_logs_format_is_both_its_segments() {
    for (format, named) in [
        (LogFormat::Mixed, 0x414d_5007),
        (LogFormat::Packed, 0x414d_5004),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (db, conn) = open(dir.path());
        db.wal_format(format).unwrap();
        conn.execute("CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT)")
            .unwrap();
        insert(&conn, 1, 3);
        for switch in 1..=2 {
            copy(&conn);
            assert!(db.switch_log(None).unwrap(), "{format:?}, switch {switch}");
            insert(&conn, 3 * switch + 1, 3 * switch + 3);
        }
        for suffix in ["-wal", "-wal2"] {
            let log = std::fs::read(beside(dir.path(), suffix)).unwrap();
            let of = u32::from_be_bytes(log[4..8].try_into().unwrap());
            assert_eq!(of, named, "{format:?}: {suffix}'s format is {of:#x}");
        }
        let files = Files::of(dir.path(), &[]);
        files.holds(9, &format!("{format:?}"));
    }
}

/// Do: VACUUM INTO a file from a database whose commits need `-wal2`; read the file with sqlite3.
/// Expect: every row: that file is whole, with no log.
#[test]
fn d45_vacuum_into_leaves_a_file_sqlite3_reads() {
    for format in FORMATS {
        let (dir, _db, conn, there) = left(Log::Second, format);
        let into = tempfile::tempdir().unwrap();
        let path = beside(into.path(), "");
        conn.execute(format!("VACUUM INTO '{}'", path.display()))
            .unwrap();
        let files = Files::of(into.path(), &[]);
        assert!(!files.refused(), "{format:?}");
        if let Some(read) = sqlite3(files.dir.path(), "SELECT count(*) FROM t") {
            assert_eq!(read, (true, there.to_string()), "{format:?}");
        }
        files.holds(there, &format!("{format:?}"));
        drop(dir);
    }
}
