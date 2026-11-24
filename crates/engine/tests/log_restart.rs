//! RESTART, TRUNCATE and the writer's restart before a write over the log's two segments (Amphora's
//! `docs/design/store.md`, decision 45): each starts the log again by a switch, and answers what
//! SQLite documents (sqlite.org/c3ref/wal_checkpoint_v2.html, sqlite.org/pragma.html). The steps
//! below were run in sqlite3 3.54.0 (2026-10-09) as they are, one connection each; what it printed
//! is each case's `sqlite`.

use amphora_engine::{
    ConnectOpts, Connection, Database, DatabaseOpts, OpenOptions, PlatformIO, SqliteDialect,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Which log the steps run on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Log {
    /// A new database's: its second segment has no file yet.
    New,
    /// One that switched before: both files are there.
    Switched,
}

/// The connection a step runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum On {
    W,
    R,
}

struct Case {
    name: &'static str,
    steps: Vec<(On, String)>,
    /// The lines sqlite3 printed.
    sqlite: Vec<&'static str>,
    /// Where decision 45 answers another line: the line's number and what it is here.
    differs: Vec<(usize, &'static str)>,
}

struct Held {
    dir: tempfile::TempDir,
    db: Arc<Database>,
    w: Arc<Connection>,
    r: Arc<Connection>,
}

fn open(path: &Path) -> Arc<Database> {
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    Database::open(
        Arc::new(PlatformIO::new().unwrap()),
        path.to_str().unwrap(),
        options,
    )
    .unwrap()
}

fn connect(db: &Arc<Database>) -> Arc<Connection> {
    let conn = db.connect(ConnectOpts::new()).unwrap();
    conn.execute("PRAGMA wal_autocheckpoint = 0").unwrap();
    conn
}

/// Each row `sql` answers, its columns between bars, as sqlite3 prints them.
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

fn beside(dir: &Path, suffix: &str) -> PathBuf {
    dir.join(format!("t.db{suffix}"))
}

/// The bytes of the log's two files.
fn log_bytes(dir: &Path) -> [u64; 2] {
    ["-wal", "-wal2"]
        .map(|suffix| std::fs::metadata(beside(dir, suffix)).map_or(0, |file| file.len()))
}

/// A database whose log holds the three frames sqlite3's holds as the steps begin.
fn held(log: Log) -> Held {
    let dir = tempfile::tempdir().unwrap();
    let db = open(&beside(dir.path(), ""));
    let w = connect(&db);
    if log == Log::Switched {
        w.execute("CREATE TABLE before(x)").unwrap();
        assert!(db.switch_log(None).unwrap());
        for _ in 0..2 {
            rows(&w, "PRAGMA wal_checkpoint(PASSIVE)");
        }
        assert!(beside(dir.path(), "-wal2").exists());
    }
    w.execute("CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT)")
        .unwrap();
    w.execute("INSERT INTO t VALUES(1, 'one')").unwrap();
    let r = connect(&db);
    Held { dir, db, w, r }
}

fn checkpoint(mode: &str) -> (On, String) {
    (On::W, format!("PRAGMA wal_checkpoint({mode})"))
}

fn insert(row: u32) -> (On, String) {
    (On::W, format!("INSERT INTO t VALUES({row}, 'row')"))
}

fn count(on: On) -> (On, String) {
    (on, "SELECT count(*) FROM t".to_owned())
}

fn read(sql: &str) -> (On, String) {
    (On::R, sql.to_owned())
}

fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case {
            name: "RESTART, no read",
            steps: vec![
                checkpoint("RESTART"),
                insert(2),
                checkpoint("PASSIVE"),
                checkpoint("FULL"),
                checkpoint("RESTART"),
                insert(3),
                insert(4),
                checkpoint("RESTART"),
                count(On::W),
            ],
            sqlite: vec!["0|3|3", "0|1|1", "0|1|1", "0|1|1", "0|2|2", "4"],
            differs: vec![],
        },
        Case {
            name: "TRUNCATE, no read",
            steps: vec![
                checkpoint("TRUNCATE"),
                insert(2),
                checkpoint("PASSIVE"),
                checkpoint("TRUNCATE"),
                checkpoint("TRUNCATE"),
                insert(3),
                checkpoint("TRUNCATE"),
                count(On::W),
            ],
            sqlite: vec!["0|0|0", "0|1|1", "0|0|0", "0|0|0", "0|0|0", "3"],
            differs: vec![],
        },
        Case {
            name: "the writer's restart",
            steps: vec![
                checkpoint("PASSIVE"),
                insert(2),
                checkpoint("PASSIVE"),
                insert(3),
                insert(4),
                checkpoint("PASSIVE"),
                count(On::W),
            ],
            sqlite: vec!["0|3|3", "0|1|1", "0|2|2", "4"],
            differs: vec![],
        },
    ];
    for mode in ["FULL", "RESTART", "TRUNCATE"] {
        let cut = mode == "TRUNCATE";
        cases.push(Case {
            name: "a read of an older snapshot",
            steps: vec![
                read("BEGIN"),
                count(On::R),
                insert(2),
                checkpoint(mode),
                insert(3),
                count(On::R),
                read("COMMIT"),
                checkpoint(mode),
                insert(4),
                checkpoint("PASSIVE"),
                count(On::W),
            ],
            sqlite: vec![
                "1",
                "1|4|3",
                "1",
                if cut { "0|0|0" } else { "0|5|5" },
                "0|1|1",
                "4",
            ],
            differs: vec![],
        });
        // The write between the two calls starts the log again beside the read (decision 45), where
        // SQLite's appends to it: the second call finds the one frame of that write.
        cases.push(Case {
            name: "a read of the newest snapshot",
            steps: vec![
                read("BEGIN"),
                count(On::R),
                checkpoint(mode),
                insert(2),
                count(On::R),
                read("COMMIT"),
                checkpoint(mode),
                insert(3),
                checkpoint("PASSIVE"),
                count(On::W),
            ],
            sqlite: vec![
                "1",
                if mode == "FULL" { "0|3|3" } else { "1|3|3" },
                "1",
                if cut { "0|0|0" } else { "0|4|4" },
                "0|1|1",
                "3",
            ],
            differs: if cut { vec![] } else { vec![(3, "0|1|1")] },
        });
        cases.push(Case {
            name: "a read of the file alone",
            steps: vec![
                checkpoint("FULL"),
                read("BEGIN"),
                count(On::R),
                checkpoint(mode),
                insert(2),
                count(On::R),
                read("COMMIT"),
                checkpoint(mode),
                count(On::W),
            ],
            sqlite: vec![
                "0|3|3",
                "1",
                if cut { "0|0|0" } else { "0|3|3" },
                "1",
                if cut { "0|0|0" } else { "0|1|1" },
                "2",
            ],
            differs: vec![],
        });
        cases.push(Case {
            name: "a writer at work",
            steps: vec![
                read("BEGIN IMMEDIATE"),
                read("INSERT INTO t VALUES(2, 'row')"),
                checkpoint(mode),
                read("COMMIT"),
                checkpoint(mode),
                count(On::W),
            ],
            sqlite: vec!["1|3|3", if cut { "0|0|0" } else { "0|4|4" }, "2"],
            differs: vec![],
        });
    }
    cases
}

/// Do (decision 45, AC-T1.12.3): each case's steps, on a new log and on one that switched before.
/// Expect: what sqlite3 3.54.0 printed, but where the case says decision 45 answers another line; a
/// TRUNCATE that answers 0 leaves both of the log's files at 0 bytes.
#[test]
fn d45_restart_and_truncate_answer_as_sqlite() {
    let mut wrong = Vec::new();
    for log in [Log::New, Log::Switched] {
        for case in cases() {
            let held = held(log);
            let mut printed = Vec::new();
            for (on, sql) in &case.steps {
                let conn = match on {
                    On::W => &held.w,
                    On::R => &held.r,
                };
                let answer = rows(conn, sql);
                if sql.contains("TRUNCATE") && answer == ["0|0|0"] {
                    let left = log_bytes(held.dir.path());
                    if left != [0, 0] {
                        wrong.push(format!("{log:?}, {}: {sql} left {left:?}", case.name));
                    }
                }
                printed.extend(answer);
            }
            let mut expected = case.sqlite.clone();
            for (line, here) in case.differs {
                expected[line] = here;
            }
            if printed != expected {
                wrong.push(format!(
                    "{log:?}, {}, {:?}: {printed:?}, not {expected:?}",
                    case.name,
                    case.steps
                        .iter()
                        .map(|(_, sql)| sql.as_str())
                        .collect::<Vec<_>>()
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Do (decision 45, On disk): after each step of a RESTART, a TRUNCATE and a writer's restart, on a
/// new log and on one that switched before, copy the database's file and the log's two as a process
/// that ends there leaves them, and open the copy. Expect: each copy opens, whole, with every row
/// committed by then.
#[test]
fn d45_the_files_left_at_each_step_of_a_restart_hold_every_row() {
    for log in [Log::New, Log::Switched] {
        for mode in ["RESTART", "TRUNCATE", "PASSIVE"] {
            let held = held(log);
            let mut committed = 1;
            for step in 0..12 {
                if step % 3 == 2 {
                    rows(&held.w, &format!("PRAGMA wal_checkpoint({mode})"));
                } else {
                    committed += 1;
                    held.w
                        .execute(format!("INSERT INTO t VALUES({committed}, 'row')"))
                        .unwrap();
                }
                let copied = tempfile::tempdir().unwrap();
                for suffix in ["", "-wal", "-wal2"] {
                    let from = beside(held.dir.path(), suffix);
                    if from.exists() {
                        std::fs::copy(from, beside(copied.path(), suffix)).unwrap();
                    }
                }
                let reopened = open(&beside(copied.path(), ""));
                let conn = connect(&reopened);
                let at = format!("{log:?}, {mode}, step {step}");
                assert_eq!(
                    rows(&conn, "SELECT count(*) FROM t"),
                    [committed.to_string()],
                    "{at}"
                );
                assert_eq!(rows(&conn, "PRAGMA integrity_check"), ["ok"], "{at}");
            }
        }
    }
}

/// Do (decision 45): hold a read of the newest snapshot, in the log; ten times, write a row and
/// checkpoint (PASSIVE); end the read; write and checkpoint twice more. Expect: no write is refused,
/// the read answers its own snapshot throughout, and the log starts again beside it once (its
/// segment is the one the read holds) and again once the read ends.
#[test]
fn d45_a_read_in_the_log_holds_up_no_write_and_no_restart() {
    for log in [Log::New, Log::Switched] {
        let held = held(log);
        held.r.execute("BEGIN").unwrap();
        assert_eq!(rows(&held.r, "SELECT count(*) FROM t"), ["1"]);
        assert_eq!(rows(&held.w, "PRAGMA wal_checkpoint(PASSIVE)"), ["0|3|3"]);
        let first = held.db.wal_segment();
        for row in 2..12 {
            held.w
                .execute(format!("INSERT INTO t VALUES({row}, 'row')"))
                .unwrap();
            rows(&held.w, "PRAGMA wal_checkpoint(PASSIVE)");
            assert_eq!(rows(&held.r, "SELECT count(*) FROM t"), ["1"], "{log:?}");
            assert_ne!(
                held.db.wal_segment(),
                first,
                "{log:?}: the log didn't start again beside the read"
            );
        }
        let (grown, _) = held.db.wal_backfill();
        assert!(grown >= 10, "{log:?}: {grown} frames");
        held.r.execute("COMMIT").unwrap();
        for row in 12..14 {
            rows(&held.w, "PRAGMA wal_checkpoint(PASSIVE)");
            rows(&held.w, "PRAGMA wal_checkpoint(PASSIVE)");
            held.w
                .execute(format!("INSERT INTO t VALUES({row}, 'row')"))
                .unwrap();
        }
        assert!(held.db.wal_backfill().0 < grown, "{log:?}");
        assert_eq!(rows(&held.r, "SELECT count(*) FROM t"), ["13"], "{log:?}");
        assert_eq!(rows(&held.r, "PRAGMA integrity_check"), ["ok"], "{log:?}");
    }
}

/// Do (decision 45's rule 1): end a process after a switch, the segment it closed not copied (its
/// files, copied as it left them), and open them: checkpoint (FULL, RESTART, TRUNCATE) or VACUUM, with
/// no read. Expect: the whole log in the file, the closed segment's frames and the current one's, as
/// "It then checkpoints all frames in the log file" has it (wal_checkpoint_v2): the file alone holds
/// every row, and TRUNCATE and VACUUM leave both of the log's files at 0 bytes.
#[test]
fn d45_a_checkpoint_of_the_whole_log_copies_both_segments() {
    for (sql, answer) in [
        ("PRAGMA wal_checkpoint(FULL)", vec!["0|4|4"]),
        ("PRAGMA wal_checkpoint(RESTART)", vec!["0|4|4"]),
        ("PRAGMA wal_checkpoint(TRUNCATE)", vec!["0|0|0"]),
        ("VACUUM", vec![]),
    ] {
        let held = held(Log::New);
        assert!(held.db.switch_log(None).unwrap());
        for row in 2..6 {
            held.w
                .execute(format!("INSERT INTO t VALUES({row}, 'row')"))
                .unwrap();
        }
        let left = tempfile::tempdir().unwrap();
        for suffix in ["", "-wal", "-wal2"] {
            std::fs::copy(beside(held.dir.path(), suffix), beside(left.path(), suffix)).unwrap();
        }
        let db = open(&beside(left.path(), ""));
        let conn = connect(&db);
        assert!(!db.wal_closing_copied(), "{sql}");
        assert_eq!(rows(&conn, sql), answer, "{sql}");
        assert_eq!(rows(&conn, "SELECT count(*) FROM t"), ["5"], "{sql}");
        if sql.contains("TRUNCATE") || sql == "VACUUM" {
            assert_eq!(log_bytes(left.path()), [0, 0], "{sql}");
        }
        let alone = tempfile::tempdir().unwrap();
        std::fs::copy(beside(left.path(), ""), beside(alone.path(), "")).unwrap();
        let file = open(&beside(alone.path(), ""));
        let file = connect(&file);
        assert_eq!(rows(&file, "SELECT count(*) FROM t"), ["5"], "{sql}");
        assert_eq!(rows(&file, "PRAGMA integrity_check"), ["ok"], "{sql}");
        conn.execute("INSERT INTO t VALUES(6, 'row')").unwrap();
        assert_eq!(rows(&conn, "SELECT count(*) FROM t"), ["6"], "{sql}");
    }
}
