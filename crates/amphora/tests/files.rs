//! A store's files (docs/design/store.md, decision 12): a new database's files are written in place
//! on a file system that would copy them (btrfs's no-copy-on-write flag), and each page is
//! checksummed, so a page corrupted on disk is refused rather than read (the guarantee btrfs's own
//! data checksums gave, which the flag turns off).

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros
    )
)]

use std::io::{Read, Seek, SeekFrom, Write};

use amphora::turso::Db;
use turso_core::Value;

/// A directory on the build's own file system (not a tmpfs `/tmp`, which takes no such flag).
fn lab() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

/// A new database's file and log carry the no-copy-on-write flag wherever the file system takes it
/// (a probe file says whether it does); where it doesn't, nothing changes.
#[cfg(target_os = "linux")]
#[test]
fn a_new_databases_files_are_written_in_place_where_the_file_system_would_copy_them() {
    use rustix::fs::{IFlags, ioctl_getflags, ioctl_setflags};
    let dir = lab();
    let probe = std::fs::File::create(dir.path().join("probe")).unwrap();
    let takes = ioctl_getflags(&probe)
        .and_then(|f| ioctl_setflags(&probe, f | IFlags::NOCOW))
        .is_ok()
        && ioctl_getflags(&probe).is_ok_and(|f| f.contains(IFlags::NOCOW));
    let path = dir.path().join("t.db");
    let db = Db::open(&path).unwrap();
    db.exec("CREATE TABLE t (n INTEGER)").unwrap();
    let mut wal = path.clone().into_os_string();
    wal.push("-wal");
    for file in [path.clone(), wal.into()] {
        let flags = ioctl_getflags(std::fs::File::open(&file).unwrap()).unwrap();
        assert_eq!(
            flags.contains(IFlags::NOCOW),
            takes,
            "{}: the file system takes the flag: {takes}",
            file.display()
        );
    }
}

/// A page of the database file corrupted on disk is refused with an error when read, never handed
/// back as data.
#[test]
fn a_page_corrupted_on_disk_is_refused_not_read() {
    let dir = lab();
    let path = dir.path().join("t.db");
    {
        let mut db = Db::open(&path).unwrap();
        db.exec("CREATE TABLE t (n INTEGER, text TEXT)").unwrap();
        db.transaction(|db| {
            for n in 0..500 {
                db.query(
                    "INSERT INTO t (n, text) VALUES (?1, ?2)",
                    [
                        Value::from_i64(n),
                        Value::from_text(format!("row {n} of five hundred")),
                    ],
                    |_| Ok(true),
                )?;
            }
            Ok(())
        })
        .unwrap();
        // Everything copied from the log into the file, the file flushed, the log started again.
        assert!(db.restart_log().unwrap());
    }
    // The log goes (its frames, still valid on disk until the next write, would be read in place
    // of the file's pages): the file holds every page.
    let mut wal = path.clone().into_os_string();
    wal.push("-wal");
    std::fs::remove_file(&wal).unwrap();
    // The database keeps room for each page's checksum: the header's reserved-space byte (offset
    // 20 of SQLite's format) says 8.
    let header = std::fs::read(&path).unwrap();
    assert_eq!(header[20], 8, "the pages keep no room for a checksum");
    // A byte in the middle of the table's root page (page 2, the first table made) flipped.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    assert!(
        header.len() > 2 * 4096,
        "the table took more than its root page"
    );
    let at = 4096 + 2048;
    let mut byte = [0u8; 1];
    file.seek(SeekFrom::Start(at)).unwrap();
    file.read_exact(&mut byte).unwrap();
    file.seek(SeekFrom::Start(at)).unwrap();
    file.write_all(&[byte[0] ^ 0xff]).unwrap();
    file.sync_all().unwrap();
    drop(file);

    let mut db = Db::open(&path).unwrap();
    let mut sum = 0i64;
    let read = db.query("SELECT n FROM t", [], |r| {
        sum += r.get::<i64>(0).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(true)
    });
    let err = read.expect_err("a corrupted page was read as data");
    assert!(
        format!("{err:#}").to_lowercase().contains("checksum"),
        "refused, but not for its checksum: {err:#}"
    );
}
