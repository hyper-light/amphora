//! A stopped database's files, as its user finds them (docs/design/store.md, decision 45, "What
//! plain SQLite sees"; AC-T1.1.1): an owner that ends in order closes them cleanly, so sqlite3 reads
//! every commit in them; one that is killed leaves them refused, whole to Amphora. SQLite itself is
//! asked where its shell is here (`sqlite3`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use amphora::database::{AnswerMax, Database};
use amphora_api::Refusal;

fn until() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

fn write(db: &Database, rows: std::ops::Range<i64>) {
    db.call(until(), AnswerMax::fixed(), move |c| {
        c.execute_batch("CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT)")?;
        for id in rows {
            c.execute_batch(&format!(
                "INSERT INTO notes VALUES ({id}, '{}')",
                "n".repeat(500)
            ))?;
        }
        Ok(())
    })
    .unwrap();
}

fn count(db: &Database) -> i64 {
    db.call(until(), AnswerMax::fixed(), |c| {
        Ok(c.query_row("SELECT count(*) FROM notes", [], |r| {
            r.get::<_, i64>(0usize)
        })?)
    })
    .unwrap()
}

/// The files of the database at `path` as they are now, carried off.
fn carried_off(path: &Path) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let name = path.file_name().unwrap().to_str().unwrap();
    for suffix in ["", "-wal", "-wal2"] {
        let file = path.with_file_name(format!("{name}{suffix}"));
        if file.exists() {
            std::fs::copy(&file, dir.path().join(format!("{name}{suffix}"))).unwrap();
        }
    }
    let path = dir.path().join(name);
    (dir, path)
}

/// What sqlite3's shell says of the files at `path`: the rows it counts, or why it doesn't. None
/// without a shell.
fn sqlite3(path: &Path) -> Option<Result<i64, String>> {
    let (_dir, path) = carried_off(path);
    let ran = Command::new("sqlite3")
        .args(["-batch", path.to_str().unwrap()])
        .arg("SELECT count(*) FROM notes")
        .output();
    let ran = match ran {
        Ok(ran) => ran,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => panic!("sqlite3: {e}"),
    };
    let out = String::from_utf8_lossy(&ran.stdout).trim().to_owned();
    if ran.status.success() {
        return Some(out.parse().map_err(|_| out));
    }
    Some(Err(String::from_utf8_lossy(&ran.stderr).trim().to_owned()))
}

/// SQLite's refusals of a database it is kept out of: page 1's version, or the log's format.
fn refused(path: &Path) -> bool {
    match sqlite3(path) {
        Some(Ok(_)) => false,
        Some(Err(said)) => {
            said.contains("file is not a database") || said.contains("unable to open database file")
        }
        None => true,
    }
}

/// The versions page 1 of the file says, written first, and the bytes the log's two files hold.
fn marks(path: &Path) -> ([u8; 2], [u64; 2]) {
    let file = std::fs::read(path).unwrap();
    let name = path.file_name().unwrap().to_str().unwrap();
    let held = |suffix: &str| {
        std::fs::metadata(path.with_file_name(format!("{name}{suffix}"))).map_or(0, |m| m.len())
    };
    ([file[18], file[19]], [held("-wal"), held("-wal2")])
}

/// Do: three times, open a database, write to it, and end it in order. Expect: sqlite3 refused while it
/// is open, reading every commit once it ended; and the next open finding them all.
#[test]
fn d45_sqlite3_reads_every_commit_of_a_database_that_ended_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.db");
    for life in 0..3 {
        let db = Database::open(&path, until()).unwrap();
        if life > 0 {
            assert_eq!(count(&db), life * 20, "life {life}");
        }
        write(&db, life * 20..(life + 1) * 20);
        assert!(refused(&path), "life {life}: {:?}", sqlite3(&path));
        db.close(until()).unwrap();
        assert_eq!(marks(&path), ([3, 2], [0, 0]), "life {life}");
        if let Some(read) = sqlite3(&path) {
            assert_eq!(read, Ok((life + 1) * 20), "life {life}");
        }
    }
}

/// Do: close a database one of whose handles is held elsewhere, then let that one go. Expect: refused
/// as not in time, the database going on; closed once the last handle is.
#[test]
fn d45_a_database_held_elsewhere_closes_with_its_last_handle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.db");
    let db = Database::open(&path, until()).unwrap();
    let held = db.clone();
    write(&db, 0..5);
    let soon = Instant::now() + Duration::from_millis(50);
    assert!(matches!(db.close(soon), Err(Refusal::Timeout)));
    write(&held, 5..10);
    assert_eq!(count(&held), 10);
    assert!(refused(&path), "{:?}", sqlite3(&path));
    held.close(until()).unwrap();
    assert_eq!(marks(&path), ([3, 2], [0, 0]));
    if let Some(read) = sqlite3(&path) {
        assert_eq!(read, Ok(10));
    }
}

const CHILD: &str = "AMPHORA_SHUTDOWN_CHILD";

/// Do: kill a process with a database open that it wrote to, and open the database again. Expect:
/// sqlite3 refused until it ends in order, and every commit there.
#[test]
fn d45_sqlite3_refuses_a_database_whose_owner_was_killed() {
    if let Some(path) = std::env::var_os(CHILD) {
        let db = Database::open(Path::new(&path), until()).unwrap();
        write(&db, 0..20);
        std::process::abort();
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.db");
    let killed = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "d45_sqlite3_refuses_a_database_whose_owner_was_killed",
        ])
        .env(CHILD, &path)
        .status()
        .unwrap();
    assert!(!killed.success());
    assert!(refused(&path), "{:?}", sqlite3(&path));
    assert_eq!(marks(&path).0, [3, 3]);
    let db = Database::open(&path, until()).unwrap();
    assert_eq!(count(&db), 20);
    db.close(until()).unwrap();
    if let Some(read) = sqlite3(&path) {
        assert_eq!(read, Ok(20));
    }
}
