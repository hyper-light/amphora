//! A read the log's owner ends while a page of it is in flight (`Database::end_closing_reads`;
//! Amphora's `docs/design/store.md`, decision 45, "A read ended for a write that needs the room"):
//! by the time its bytes are read, the segment or the file holds another generation's, and the read
//! takes none of them in, for itself or for the reads that share its pages. The page's read is held
//! where the test says, so the order is the test's and not a thread's.

use amphora_engine::io::{FileId, FileSyncType};
use amphora_engine::storage::log_segments::SegmentId;
use amphora_engine::{
    Buffer, CheckpointMode, Clock, Completion, CompletionError, ConnectOpts, Connection, Database,
    DatabaseOpts, File, LimboError, LogFormat, MonotonicInstant, OpenFlags, OpenOptions, PagePath,
    PlatformIO, SqliteDialect, WallClockInstant, IO,
};
use std::path::Path;
use std::sync::{Arc, Mutex};

type Hook = Box<dyn FnOnce() + Send>;

/// Where within a file's read a hook runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum At {
    /// Before its bytes are read: they are what the hook left there.
    BeforeBytes,
    /// Once they were read, before they are given: they are what was there before the hook.
    AfterBytes,
}

/// What runs within the next read of the file named, once.
#[derive(Default)]
struct Within(Mutex<Option<(&'static str, At, Hook)>>);

impl Within {
    fn arm(&self, file: &'static str, at: At, hook: impl FnOnce() + Send + 'static) {
        *self.0.lock().unwrap() = Some((file, at, Box::new(hook)));
    }

    fn armed(&self) -> bool {
        self.0.lock().unwrap().is_some()
    }

    fn reach(&self, path: &str, at: At) {
        let hook = {
            let mut armed = self.0.lock().unwrap();
            match &*armed {
                Some((file, when, _)) if path.ends_with(file) && *when == at => armed.take(),
                _ => None,
            }
        };
        if let Some((_, _, hook)) = hook {
            hook();
        }
    }
}

struct Hooked {
    inner: Arc<PlatformIO>,
    within: Arc<Within>,
}

struct HookedFile {
    inner: Arc<dyn File>,
    path: String,
    within: Arc<Within>,
}

impl File for HookedFile {
    fn lock_file(&self, exclusive: bool) -> amphora_engine::Result<()> {
        self.inner.lock_file(exclusive)
    }

    fn unlock_file(&self) -> amphora_engine::Result<()> {
        self.inner.unlock_file()
    }

    fn pread(&self, pos: u64, c: Completion) -> amphora_engine::Result<Completion> {
        self.within.reach(&self.path, At::BeforeBytes);
        self.inner.pread(pos, c)
    }

    fn pread_inline(&self, pos: u64, dest: &mut [u8]) -> amphora_engine::Result<Option<usize>> {
        self.within.reach(&self.path, At::BeforeBytes);
        let read = self.inner.pread_inline(pos, dest)?;
        if read.is_some() {
            self.within.reach(&self.path, At::AfterBytes);
        }
        Ok(read)
    }

    fn pwrite(
        &self,
        pos: u64,
        buffer: Arc<Buffer>,
        c: Completion,
    ) -> amphora_engine::Result<Completion> {
        self.inner.pwrite(pos, buffer, c)
    }

    fn pwritev(
        &self,
        pos: u64,
        buffers: Vec<Arc<Buffer>>,
        c: Completion,
    ) -> amphora_engine::Result<Completion> {
        self.inner.pwritev(pos, buffers, c)
    }

    fn sync(&self, c: Completion, sync_type: FileSyncType) -> amphora_engine::Result<Completion> {
        self.inner.sync(c, sync_type)
    }

    fn size(&self) -> amphora_engine::Result<u64> {
        self.inner.size()
    }

    fn truncate(&self, len: u64, c: Completion) -> amphora_engine::Result<Completion> {
        self.inner.truncate(len, c)
    }
}

impl Clock for Hooked {
    fn current_time_monotonic(&self) -> MonotonicInstant {
        self.inner.current_time_monotonic()
    }

    fn current_time_wall_clock(&self) -> WallClockInstant {
        self.inner.current_time_wall_clock()
    }
}

impl IO for Hooked {
    fn open_file(
        &self,
        path: &str,
        flags: OpenFlags,
        direct: bool,
    ) -> amphora_engine::Result<Arc<dyn File>> {
        Ok(Arc::new(HookedFile {
            inner: self.inner.open_file(path, flags, direct)?,
            path: path.to_owned(),
            within: self.within.clone(),
        }))
    }

    fn remove_file(&self, path: &str) -> amphora_engine::Result<()> {
        self.inner.remove_file(path)
    }

    fn step(&self) -> amphora_engine::Result<()> {
        self.inner.step()
    }

    fn file_id(&self, path: &str) -> amphora_engine::Result<FileId> {
        self.inner.file_id(path)
    }
}

/// The database's file.
const FILE: &str = ".db";

/// The file of the log's current segment.
fn log_file(db: &Arc<Database>) -> &'static str {
    match db.wal_segment() {
        SegmentId::First => "-wal",
        SegmentId::Second => "-wal2",
    }
}

fn connect(db: &Arc<Database>, path: PagePath) -> Arc<Connection> {
    let conn = db.connect(ConnectOpts::new()).unwrap();
    conn.execute("PRAGMA wal_autocheckpoint = 0").unwrap();
    conn.get_pager().set_page_path(path);
    conn
}

/// Table b's one row, its one page.
fn b(conn: &Arc<Connection>) -> Result<String, LimboError> {
    let mut stmt = conn.prepare("SELECT v FROM b WHERE id = 1")?;
    let rows = stmt.run_collect_rows()?;
    Ok(rows[0][0].to_string())
}

/// Passes on a connection of their own until the log is copied whole into the file.
fn copy_all(db: &Arc<Database>) {
    let copier = connect(db, PagePath::Today);
    for _ in 0..4 {
        copier
            .checkpoint(CheckpointMode::Passive {
                upper_bound_inclusive: None,
            })
            .unwrap();
        let (log, copied) = db.wal_backfill();
        if log == copied && db.closing_reads() == 0 {
            return;
        }
    }
    panic!("the log was not copied whole: {:?}", db.wal_backfill());
}

fn too_old(answer: &Result<String, LimboError>) -> bool {
    matches!(
        answer,
        Err(LimboError::SnapshotTooOld
            | LimboError::CompletionError(CompletionError::SnapshotTooOld))
    )
}

/// A database of tables a and b, a row and a page each, b's 'file', all of it in the file.
fn made(
    dir: &Path,
    format: LogFormat,
    share: bool,
) -> (Arc<Database>, Arc<Within>, Arc<Connection>) {
    let within = Arc::new(Within::default());
    let io = Arc::new(Hooked {
        inner: Arc::new(PlatformIO::new().unwrap()),
        within: within.clone(),
    });
    let options = OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new());
    let db = Database::open(io, dir.join("ended.db").to_str().unwrap(), options).unwrap();
    db.wal_format(format).unwrap();
    if share {
        db.share_pages(4096).unwrap();
    }
    let writer = connect(&db, PagePath::Today);
    for sql in [
        "CREATE TABLE a(x)",
        "INSERT INTO a VALUES(0)",
        "CREATE TABLE b(id INTEGER PRIMARY KEY, v TEXT)",
        "INSERT INTO b VALUES(1, 'file')",
    ] {
        writer.execute(sql).unwrap();
    }
    copy_all(&db);
    (db, within, writer)
}

/// A read of `db`'s snapshot as it is, on a connection of its own, its page of b not read yet.
fn begun(db: &Arc<Database>, path: PagePath) -> Arc<Connection> {
    let reader = connect(db, path);
    reader.execute("BEGIN").unwrap();
    reader
        .prepare("SELECT x FROM a")
        .unwrap()
        .run_collect_rows()
        .unwrap();
    reader
}

/// After the read `reader` was refused, which ended its transaction: the row as it now is is what
/// it and any other read find.
fn newer_after(db: &Arc<Database>, reader: &Arc<Connection>, newest: &str) {
    assert!(
        reader.get_auto_commit(),
        "the refused read's transaction is open"
    );
    assert_eq!(b(reader).unwrap(), newest, "the read asked again");
    for path in [PagePath::Today, PagePath::Plain] {
        assert_eq!(
            b(&connect(db, path)).unwrap(),
            newest,
            "another read, {path:?}"
        );
    }
}

/// Do (decision 45): b's row set to 'old', its page in the log's current segment alone, in each log
/// format; a read begun, b not read yet; the log switched, so that segment closes; b read, and
/// within the read of its page from that segment, before the bytes: the read ended by the log's
/// owner, the segment copied whole and reset, the log switched back into it, and b's row set 40
/// times over, 'new 1' to 'new 40', over the frame being read. Expect: the read of b refused as a
/// snapshot too old, though its bytes are a whole page of b or a whole record by then; 'new 40' to
/// the same read asked again and to any other.
#[test]
fn d45_a_log_page_in_flight_as_its_read_ends_is_never_taken_in() {
    for format in [LogFormat::Pages, LogFormat::Mixed] {
        for path in [PagePath::Today, PagePath::Plain] {
            let dir = tempfile::tempdir().unwrap();
            let (db, within, writer) = made(dir.path(), format, false);
            writer.execute("UPDATE b SET v = 'old'").unwrap();
            let log = log_file(&db);
            let reader = begun(&db, path);
            assert!(db.switch_log(None).unwrap());
            writer.execute("UPDATE a SET x = 1").unwrap();
            let owner = db.clone();
            within.arm(log, At::BeforeBytes, move || {
                assert_eq!(owner.end_closing_reads(), 1);
                copy_all(&owner);
                assert!(owner.switch_log(None).unwrap());
                for n in 1..=40 {
                    writer
                        .execute(format!("UPDATE b SET v = 'new {n}'"))
                        .unwrap();
                }
            });
            let read = b(&reader);
            assert!(!within.armed(), "b's page was not read from the segment");
            assert!(too_old(&read), "{format:?}, {path:?}: {read:?}");
            newer_after(&db, &reader, "new 40");
        }
    }
}

/// Do (decision 45): b's row 'file', its page in the database's file alone; a read begun, b not
/// read yet; the log switched, so the segment it counts on closes; b's row set to 'new' in the
/// segment now current; b read, and within the read of its page from the file, before the bytes or
/// once they were read: the read ended by the log's owner and both segments copied into the file,
/// over the page being read. On each page path, pages shared among the connections or not. Expect:
/// the read of b refused as a snapshot too old, whichever bytes it read; 'new' to the same read
/// asked again and to any other: none finds in the shared pages what the ended read read.
#[test]
fn d45_a_file_page_in_flight_as_its_read_ends_is_never_taken_in_nor_shared() {
    for path in [PagePath::Today, PagePath::Plain] {
        for share in [false, true] {
            for at in [At::BeforeBytes, At::AfterBytes] {
                // Today's path reads through a completion, which ends within the read itself.
                if path == PagePath::Today && at == At::AfterBytes {
                    continue;
                }
                let dir = tempfile::tempdir().unwrap();
                let (db, within, writer) = made(dir.path(), LogFormat::Mixed, share);
                writer.execute("UPDATE a SET x = 1").unwrap();
                let reader = begun(&db, path);
                assert!(db.switch_log(None).unwrap());
                writer.execute("UPDATE b SET v = 'new'").unwrap();
                let owner = db.clone();
                within.arm(FILE, at, move || {
                    assert_eq!(owner.end_closing_reads(), 1);
                    copy_all(&owner);
                });
                let read = b(&reader);
                let case = format!("{path:?}, shared {share}, {at:?}");
                assert!(
                    !within.armed(),
                    "b's page was not read from the file: {case}"
                );
                assert!(too_old(&read), "{case}: {read:?}");
                newer_after(&db, &reader, "new");
            }
        }
    }
}
