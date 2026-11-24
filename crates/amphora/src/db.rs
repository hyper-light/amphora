//! Stores on Amphora's engine (`crates/engine`): a database opened the way Amphora's pipeline wants it
//! ([`Db::open`]), its readers ([`Db::reader`]), and what the store's flusher, checkpointer and
//! writer do with it ([`Db::flusher`], [`Db::checkpoint`], [`Db::restart_log`]). An engine on Turso
//! keeps a [`Db`] for its writer and gives readers their own.

use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

pub use amphora_engine::LogFormat;
use amphora_engine::busy::BusyWake;
use amphora_engine::io::FileSyncType;
use amphora_engine::storage::log_segments::SegmentId;
use amphora_engine::{
    Buffer, CheckpointMode, Completion, CompletionError, ConnectOpts, Connection, Database,
    DatabaseOpts, IO, IOExt, LimboError, OpenFlags, OpenOptions, PlatformIO, Row, SqliteDialect,
    Statement, StepResult, Value,
};

use crate::store::{Checkpointed, Config, Flush, Opener};

#[cfg(test)]
mod allocs;
mod cache;

/// The pages a process's databases share (docs/design/store.md, decision 20).
pub use amphora_engine::storage::shared_pages::{Charge, SharedPages};

/// How long opening waits for another process's hold on the file to go.
const LOCK_WAIT: Duration = Duration::from_secs(15);
/// Pages each connection keeps in memory, by what it does (docs/design/store.md, decision 5): the
/// writer keeps the inner pages of its indexes; a reader 2,000 (8 MiB, Turso's own default), since
/// it forgets only what commits wrote (decision 18), so its cache serves the reads after them; the
/// checkpointer next to nothing.
pub const WRITER_PAGES: i64 = 1024;
pub const READER_PAGES: i64 = 2000;

/// The bytes of a page of Amphora's databases (the engine's default page size): what a pool's page
/// budget is counted in.
pub(crate) const PAGE_BYTES: u64 = 4096;
/// The process's pool takes up to this part of the memory the process may use (decision 26):
/// PostgreSQL's documentation advises a quarter of a dedicated server's memory for its shared
/// buffers ("Resource Consumption", `shared_buffers`); CedarDB's buffer pool takes up to 45% and
/// DuckDB's memory limit is 80% by default, so a quarter is the cautious end of what databases
/// that size from memory take.
const MEMORY_SHARE_DIVISOR: u64 = 4;
/// The process's pool where the system doesn't say how much memory there is: 256 MiB, what
/// decision 20 measured on the suite at 200,000 rows.
const UNKNOWN_MEMORY_PAGES: usize = 65_536;
/// The process's pool at least, whatever the memory (16 MiB).
const MIN_PROCESS_PAGES: usize = 4096;

/// Whose shared pages a database's connections read through (docs/design/store.md, decisions 20
/// and 26): a page one connection read in, the others read from memory. [`Db::open`] shares none;
/// whether it should share the process's pool by default, and its budget, are the person's call
/// (STATUS.md).
#[derive(Clone)]
pub enum Sharing {
    /// The process's pool ([`process_pages`]), which every database opened so joins.
    Process,
    /// A pool of the caller's: one budget for the databases it gives it to.
    Pool(Arc<SharedPages>),
    /// None: each connection keeps its own cache alone (a packed log needs this; decision 16; a
    /// mixed one, decision 37, doesn't).
    Off,
}

/// The pool every database opened with [`Sharing::Process`] shares: made when the first opens,
/// with room for [`default_pages`] of the memory this process may use.
pub fn process_pages() -> Arc<SharedPages> {
    static POOL: OnceLock<Arc<SharedPages>> = OnceLock::new();
    POOL.get_or_init(|| Arc::new(SharedPages::new(default_pages(crate::memory::limit()))))
        .clone()
}

/// The settings every connection of a store is made with: the engine's versioned storage takes its
/// memory from the process's budget, as caches, by measured use (G7 step 2, `AC-NS.2.1`; MB.1),
/// one budget for every database the process opens.
fn connect_opts() -> ConnectOpts {
    ConnectOpts::new().with_memory(Arc::new(crate::memory::StorageMemory(
        crate::memory::process(),
    )))
}

/// A connection of `db`. A statement of it that finds the database busy (another connection
/// writing, a checkpoint) waits for the lock's release, the engine's default ([`BusyWake`]), with no
/// deadline of its own: every holder is a connection of this process, and each holder's transaction
/// ends, by its end or by its request's deadline ([`Db::deadline`]), which releases the lock (lane
/// constants-22, goal "no picked constants": the 1 ms retried 10,000 times is gone). The busy
/// timeout isn't set per request: the engine's setter makes every cached statement prepare again.
fn connect(db: &Arc<Database>) -> Result<Arc<Connection>, LimboError> {
    let conn = db.connect(connect_opts())?;
    conn.set_busy_timeout(Duration::MAX);
    Ok(conn)
}

/// The pages a process's pool keeps at most, given the bytes of memory the process may use: a
/// [`MEMORY_SHARE_DIVISOR`]th of them, at least [`MIN_PROCESS_PAGES`]; [`UNKNOWN_MEMORY_PAGES`]
/// when the system doesn't say.
#[must_use]
pub fn default_pages(memory: Option<u64>) -> usize {
    let Some(bytes) = memory else {
        return UNKNOWN_MEMORY_PAGES;
    };
    let pages = bytes
        .checked_div(MEMORY_SHARE_DIVISOR)
        .and_then(|share| share.checked_div(PAGE_BYTES))
        .unwrap_or(0);
    usize::try_from(pages)
        .unwrap_or(usize::MAX)
        .max(MIN_PROCESS_PAGES)
}

fn fail(e: impl std::error::Error + Send + Sync + 'static) -> anyhow::Error {
    anyhow::Error::new(e)
}

/// Wakes the thread that made it: what a statement stepped on this thread waits on while it is busy
/// (the release of the lock it found held, [`step`]).
struct Unpark(std::thread::Thread);

thread_local! {
    /// This thread's [`Unpark`], made once: every statement stepped here shares it, since only the
    /// thread is woken and one thread waits on one statement at a time (AUDIT 1, perf-A3).
    static UNPARK: std::task::Waker = std::task::Waker::from(Arc::new(Unpark(std::thread::current())));
}

impl std::task::Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Parks this thread for `d` whatever wakes it: a busy statement's wait on SQLite's schedule, the
/// control ([`BusyWake::Schedule`]), whose engine wakes the waker at once and means the delay.
fn pause(d: Duration) {
    let until = Instant::now().checked_add(d).unwrap_or_else(Instant::now);
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        std::thread::park_timeout(left);
    }
}

/// The files of the log beside the database at `path`, its two segments' (decision 45): `-wal`,
/// then `-wal2`.
pub(crate) fn log_paths(path: &Path) -> [PathBuf; 2] {
    ["-wal", "-wal2"].map(|suffix| {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        PathBuf::from(file)
    })
}

/// The file of one segment of the log beside the database at `path` (decision 45): `-wal` or
/// `-wal2`.
fn segment_path(path: &Path, segment: SegmentId) -> String {
    let [first, second] = log_paths(path);
    let file = match segment {
        SegmentId::First => first,
        SegmentId::Second => second,
    };
    file.display().to_string()
}

/// Makes `path`, if it doesn't exist yet, an empty file that a copy-on-write file system writes in
/// place (btrfs's no-copy-on-write flag, which takes only while a file is empty; docs/design/store.md,
/// decision 12). Turso checksums each page, so btrfs's own data checksums, which the flag turns off,
/// aren't missed. Best effort: a file that exists keeps its way, and a file system without the flag
/// declines it.
#[cfg(target_os = "linux")]
fn in_place(path: &Path) {
    use rustix::fs::{IFlags, ioctl_getflags, ioctl_setflags};
    let Ok(file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    else {
        return;
    };
    if let Ok(flags) = ioctl_getflags(&file) {
        let _ = ioctl_setflags(&file, flags | IFlags::NOCOW);
    }
}

#[cfg(not(target_os = "linux"))]
fn in_place(_path: &Path) {}

/// A connection on a Turso database, with the statements it prepared kept.
pub(crate) type Pages = Vec<(u32, Vec<u8>)>;

/// What a traced preparation read (`Db::prepare_traced`): its read set, `None` past its bound,
/// and page 1 as it stood at the session's base.
pub(crate) struct Traced {
    pub(crate) reads: Option<Vec<u32>>,
    pub(crate) page_one: Vec<u8>,
}

pub struct Db {
    io: Arc<dyn IO>,
    db: Arc<Database>,
    conn: Arc<Connection>,
    path: PathBuf,
    statements: cache::Cache,
    // After the engine's handles, so dropped after them: the lease and its guards outlive them.
    owner: Option<crate::database::Owned>,
    // Shared by this owner's readers: a failed rollback may never be reused as clean state.
    fenced: Arc<AtomicBool>,
    /// The frames of log a restart keeps the log's file for ([`Db::keep_log`]).
    log_kept: u64,
    /// What a backup asks of this database's writer ([`Db::back_up`]); readers have none.
    backup: Option<Arc<crate::backup::Hooks>>,
}

/// What a reader's opener and the flusher keep of their database: the engine's handles, then the
/// owner lease and its guards, which drop after them.
struct EngineHandles {
    io: Arc<dyn IO>,
    db: Arc<Database>,
    owner: Option<crate::database::Owned>,
}

pub(crate) struct Deadline {
    connection: Arc<Connection>,
}
impl Drop for Deadline {
    fn drop(&mut self) {
        self.connection.set_progress_handler(0, None);
    }
}

/// SQLite's log format (sqlite.org/fileformat2.html, "The WAL File Format"): a 32-byte header, then
/// frames of a 24-byte header and a page.
const LOG_HEADER_BYTES: u64 = 32;
const FRAME_HEADER_BYTES: u64 = 24;

/// The frames of log a restart keeps the file for unless told otherwise (docs/design/store.md,
/// decision 31): twice the store's default bound (`Config::log_frames`, decision 4), the most a log
/// takes between restarts.
fn log_kept() -> u64 {
    Config::default().log_frames.saturating_mul(2)
}

impl Db {
    /// Read-only source inspection. This opens no directories, creates no database, and executes
    /// no writable connection PRAGMAs. The caller owns the product's external lease.
    pub(crate) fn open_readonly(path: &Path) -> anyhow::Result<Self> {
        let name = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("the database path is not UTF-8"))?;
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().map_err(fail)?);
        // Do not let a registry hit substitute a writable engine for this inspection handle.
        let db = Database::do_open(
            Arc::clone(&io),
            name,
            OpenOptions::new(Arc::new(SqliteDialect))
                .flags(OpenFlags::ReadOnly)
                .db_opts(DatabaseOpts::new().with_without_rowid(true)),
        )
        .map_err(fail)?;
        let conn = connect(&db).map_err(fail)?;
        conn.set_statement_journals(false);
        Ok(Self {
            io,
            db,
            conn,
            path: path.to_path_buf(),
            statements: cache::Cache::default(),
            owner: None,
            fenced: Arc::new(AtomicBool::new(false)),
            log_kept: log_kept(),
            backup: None,
        })
    }

    /// Bound SQL VM work by the request's absolute deadline. The owning caller drops this guard
    /// before COMMIT/ROLLBACK cleanup, so cancellation cannot interrupt recovery of its session.
    pub(crate) fn deadline(&self, until: Instant) -> Deadline {
        self.deadline_on(until, Instant::now)
    }

    /// [`Db::deadline`] on `clock`: a replicated owner's, which its shell gives it
    /// (replication.md, "The owner's time").
    pub(crate) fn deadline_on(
        &self,
        until: Instant,
        clock: impl Fn() -> Instant + Send + Sync + 'static,
    ) -> Deadline {
        // An administrative cancellation quantum, not a throughput tuning claim. A SQL VM can
        // execute at most 1024 further instructions between deadline checks.
        self.conn
            .set_progress_handler(1024, Some(Box::new(move || clock() >= until)));
        Deadline {
            connection: Arc::clone(&self.conn),
        }
    }

    /// The database at `path`, made as needed, for a store's writer: a commit writes the log and
    /// doesn't flush it (the store's flusher does, [`Db::flusher`]), and no checkpoint runs in a
    /// commit (the store's checkpointer copies the log, [`Db::checkpoint`]). A checkpoint still
    /// flushes the log before it copies it and the file after. Its statements keep no statement
    /// journal: a transaction ([`Db::transaction`]) is committed whole or rolled back whole, so one
    /// that fails is never undone alone (docs/design/store.md, decision 7).
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        Self::open_sharing(path, &Sharing::Off)
    }

    /// [`Db::open`], its connections reading through `sharing`'s pages.
    pub fn open_sharing(path: &Path, sharing: &Sharing) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::open_made(path, sharing)
    }

    /// [`Db::open_sharing`] in a directory that exists: it makes none.
    pub(crate) fn open_made(path: &Path, sharing: &Sharing) -> anyhow::Result<Self> {
        in_place(path);
        let [first, _] = log_paths(path);
        in_place(&first);
        Self::open_with_sharing(path, Arc::new(PlatformIO::new().map_err(fail)?), sharing)
    }

    /// [`Db::open`] on `io`: the system's files, or a simulator's disk.
    pub fn open_with(path: &Path, io: Arc<dyn IO>) -> anyhow::Result<Self> {
        Self::open_with_sharing(path, io, &Sharing::Off)
    }

    /// [`Db::open_with`], its connections reading through `sharing`'s pages. A pool the caller
    /// names that the database refuses is an error; the process's pool, refused (a storage that
    /// can't share), leaves each connection its own cache, and says so.
    pub fn open_with_sharing(
        path: &Path,
        io: Arc<dyn IO>,
        sharing: &Sharing,
    ) -> anyhow::Result<Self> {
        let name = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("the path isn't UTF-8: {}", path.display()))?;
        let until = Instant::now()
            .checked_add(LOCK_WAIT)
            .unwrap_or_else(Instant::now);
        let db = loop {
            match Database::open(
                io.clone(),
                name,
                OpenOptions::new(Arc::new(SqliteDialect))
                    .db_opts(DatabaseOpts::new().with_without_rowid(true)),
            ) {
                Err(LimboError::LockingError(_)) if Instant::now() < until => {
                    pause(Duration::from_millis(100));
                }
                opened => break opened.map_err(|e| anyhow::anyhow!("opening {name}: {e}"))?,
            }
        };
        let conn = connect(&db).map_err(|e| anyhow::anyhow!("connecting to {name}: {e}"))?;
        conn.set_statement_journals(false);
        let d = Self {
            io,
            db,
            conn,
            path: path.to_path_buf(),
            statements: cache::Cache::default(),
            owner: None,
            fenced: Arc::new(AtomicBool::new(false)),
            log_kept: log_kept(),
            backup: None,
        };
        d.exec("PRAGMA synchronous = NORMAL")?;
        d.exec("PRAGMA foreign_keys = ON")?;
        d.exec("PRAGMA wal_autocheckpoint = 0")?;
        d.exec(&format!("PRAGMA cache_size = {WRITER_PAGES}"))?;
        // A commit logs its pages' changes (docs/design/store.md, decision 37).
        d.log_format(LogFormat::Mixed)?;
        match sharing {
            Sharing::Process => {
                if let Err(e) = d.share_pages_in(&process_pages()) {
                    tracing::info!(target: "amphora.store", event = "pages_unshared", error = %format!("{e:#}"), "the database's connections each keep their own pages");
                }
            }
            Sharing::Pool(pool) => d.share_pages_in(pool)?,
            Sharing::Off => {}
        }
        Ok(d)
    }

    /// Keeps `pages` of the database in this connection's memory.
    pub fn keep_pages(&self, pages: i64) -> anyhow::Result<()> {
        self.exec(&format!("PRAGMA cache_size = {pages}"))
    }

    /// This connection's page cache as `PRAGMA cache_size` states it: pages, or KiB when negative.
    pub(crate) fn cache_pages(&self) -> anyhow::Result<i64> {
        let mut pages = None;
        self.query("PRAGMA cache_size", [], |row| {
            pages = Some(row.get::<i64>(0).map_err(fail)?);
            Ok(false)
        })?;
        pages.ok_or_else(|| anyhow::anyhow!("PRAGMA cache_size answered no row"))
    }

    /// Another connection on the same database, for reading or checkpointing: each read sees what
    /// was committed before it; its checkpoints flush.
    pub fn reader(&self) -> anyhow::Result<Self> {
        self.ensure_unfenced()?;
        let r = Self {
            io: self.io.clone(),
            db: self.db.clone(),
            conn: connect(&self.db).map_err(fail)?,
            path: self.path.clone(),
            statements: cache::Cache::default(),
            owner: self.owner.clone(),
            fenced: Arc::clone(&self.fenced),
            log_kept: self.log_kept,
            backup: None,
        };
        r.keep_pages(READER_PAGES)?;
        Ok(r)
    }

    /// Opens readers on this database from any thread.
    #[must_use]
    pub fn opener(&self) -> Opener<Self> {
        let held = EngineHandles {
            io: self.io.clone(),
            db: self.db.clone(),
            owner: self.owner.clone(),
        };
        let path = self.path.clone();
        let fenced = Arc::clone(&self.fenced);
        let log_kept = self.log_kept;
        Arc::new(move || {
            // The whole of `held` is captured, so its fields drop in their declared order.
            let held = &held;
            Self::check_fence(&fenced)?;
            let r = Self {
                io: held.io.clone(),
                db: held.db.clone(),
                conn: connect(&held.db).map_err(fail)?,
                path: path.clone(),
                statements: cache::Cache::default(),
                owner: held.owner.clone(),
                fenced: Arc::clone(&fenced),
                log_kept,
                backup: None,
            };
            r.keep_pages(READER_PAGES)?;
            Ok(r)
        })
    }

    /// Flushes the log, through the database's own IO: what was committed before the flush began
    /// is durable once it ends. Its data only (`fdatasync`; on macOS `F_FULLFSYNC`, its `fsync`
    /// leaving the disk's cache): the log keeps its length across restarts (within
    /// [`Db::keep_log`]), so once it has been as long as it gets, a flush writes no metadata.
    pub fn flusher(&self) -> Flush {
        let path = self.path.clone();
        let held = EngineHandles {
            io: self.io.clone(),
            db: self.db.clone(),
            owner: self.owner.clone(),
        };
        Box::new(move || {
            // The whole of `held` is captured, so its fields drop in their declared order.
            let EngineHandles { io, db, .. } = &held;
            // Where the log ends before the flush begins: every frame up to it is durable once
            // the flush ends, and the checkpointer copies those without a flush of its own.
            let prefix = db.wal_prefix();
            if prefix.is_durable() {
                return Ok(());
            }
            // Opened each time: a flush is a disk's round trip, an open a few microseconds, and the
            // log may have been made again. No log: everything is in the file, which the checkpoint
            // that emptied the log flushed.
            // The segment the prefix is in: a switch flushes the one it closes (decision 45).
            let wal = segment_path(&path, db.wal_segment());
            let file = match io.open_file(&wal, OpenFlags::NoLock, false) {
                Ok(file) => file,
                Err(LimboError::CompletionError(CompletionError::IOError(
                    std::io::ErrorKind::NotFound,
                    _,
                ))) => return Ok(()),
                Err(e) => return Err(std::io::Error::other(e.to_string())),
            };
            file.sync(Completion::new_sync(|_| {}), FileSyncType::DataSync)
                .and_then(|c| io.wait_for_completion(c))
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            db.wal_prefix_durable(prefix);
            Ok(())
        })
    }

    /// Copies what the log holds into the file without waiting for anyone: what the log holds
    /// after, for the store to start it again once it is past its bound ([`Db::restart_log`]). A
    /// checkpoint already under way (busy) copies nothing and reports nothing. Once a flush has
    /// made some of the log durable ([`Db::flusher`]), only that is copied, and the log isn't
    /// flushed again for it.
    pub fn checkpoint(&mut self) -> anyhow::Result<Checkpointed> {
        self.ensure_unfenced()?;
        match self.conn.checkpoint(CheckpointMode::Passive {
            upper_bound_inclusive: self.db.wal_durable_frame(),
        }) {
            Ok(copied) => Ok(Checkpointed {
                log: copied.wal_max_frame,
                left: copied
                    .wal_max_frame
                    .saturating_sub(copied.wal_total_backfilled),
            }),
            Err(LimboError::Busy) => Ok(Checkpointed::default()),
            Err(e) => Err(fail(e)),
        }
    }

    /// What a checkpoint would find, copying nothing: the frames the log holds and those of them
    /// not yet in the file.
    #[must_use]
    pub fn uncopied(&self) -> Checkpointed {
        let (log, backfilled) = self.db.wal_backfill();
        Checkpointed {
            log,
            left: log.saturating_sub(backfilled),
        }
    }

    /// Copies what the reads under way let it, then starts the log again by switching the writer
    /// to its free segment (docs/design/store.md, decision 45): whether it did. The current
    /// segment is flushed and closes; reads under way keep reading it. It waits for no read:
    /// false while another connection writes, or while a read still counts on the segment the
    /// last switch closed, and the caller asks again at its next turn. A segment switched to is
    /// cut to nothing first if it holds room for more frames than [`Db::keep_log`] keeps
    /// (decision 31).
    ///
    /// A database backed up ([`Db::back_up`]) starts its log again only once the backup has shipped
    /// all of it, until the log is past what a restart keeps (decision 31): then it starts again
    /// and the backup is told its log broke there (docs/design/backup.md).
    pub fn restart_log(&mut self) -> anyhow::Result<bool> {
        self.start_log_again(true)
    }

    /// [`Db::restart_log`]'s switch alone, copying nothing: for a writer whose passes run on
    /// another thread. What the segment it closes still holds uncopied, the next pass copies
    /// ([`Db::checkpoint`]): the closing segment first (decision 45).
    pub(crate) fn switch_log(&mut self) -> anyhow::Result<bool> {
        self.start_log_again(false)
    }

    fn start_log_again(&mut self, copy: bool) -> anyhow::Result<bool> {
        self.ensure_unfenced()?;
        let over = self.log_frames()? > self.log_kept;
        let (seq, frame) = self.db.wal_end();
        let shipped = match &self.backup {
            Some(hooks) => hooks.fence.holds(seq, frame)?,
            None => true,
        };
        if !shipped && !over {
            return Ok(false);
        }
        // The segment switched to is cut to nothing if it holds room for more frames than a
        // restart keeps (decision 31, at each segment's reuse).
        let cut_past = PAGE_BYTES
            .checked_add(FRAME_HEADER_BYTES)
            .and_then(|frame| frame.checked_mul(self.log_kept))
            .and_then(|frames| frames.checked_add(LOG_HEADER_BYTES));
        // What the reads let a pass copy, the closing segment's and then the current one's: with
        // no read under way every frame is in the file, as before. The switch needs the closing
        // segment reset, which a read still on it holds off (the writer asks again at its next
        // turn).
        if copy {
            let _closing = self.checkpoint()?;
            let _current = self.checkpoint()?;
        }
        let switch = || self.db.switch_log(cut_past).map_err(fail);
        match &self.backup {
            Some(hooks) => hooks.fence.restart(seq, frame, !shipped, switch),
            None => switch(),
        }
    }

    /// Backs this database up through `hooks` (docs/design/backup.md): each commit made on this
    /// connection is stamped, and its log starts again only once shipped ([`Db::restart_log`]).
    /// Only the writer is backed up: it makes every commit.
    ///
    /// The engine, as SQLite does, starts a log every frame of which is in the file again by
    /// itself at the writer's next write, and empties it when its last connection closes: both
    /// would overwrite frames the backup hasn't shipped, a break in its log. A backed-up writer
    /// turns both off, so its log starts again only through [`Db::restart_log`] (CP-T1.12,
    /// T-T1.12.D1). Its checkpoints after commits are off already (`wal_autocheckpoint = 0`).
    pub fn back_up(&mut self, hooks: Arc<crate::backup::Hooks>) {
        self.hold_log();
        self.backup = Some(hooks);
    }

    /// The backup's hooks this writer stamps its commits by; none unbacked and for a reader.
    pub(crate) fn stamps(&self) -> Option<&Arc<crate::backup::Hooks>> {
        self.backup.as_ref()
    }

    /// Turns off the engine's own restart of the log at this connection's next write and its
    /// emptying of the log when this connection closes last ([`Db::back_up`]).
    pub(crate) fn hold_log(&self) {
        self.conn.wal_auto_actions_disable();
    }

    /// The frames of log a restart keeps the log's file for ([`Db::keep_log`]): the most a log
    /// holds between restarts.
    pub(crate) fn log_kept_frames(&self) -> u64 {
        self.log_kept
    }

    /// Stamps the commit that just ended, if this database is backed up and one did.
    fn marked(&self) -> anyhow::Result<()> {
        if let Some(hooks) = &self.backup
            && self.conn.get_auto_commit()
        {
            let (seq, frame) = self.db.wal_end();
            if frame > 0 {
                hooks.marks.committed(seq, frame)?;
            }
        }
        Ok(())
    }

    /// The log's generation and its last committed frame.
    pub(crate) fn log_end(&self) -> (u32, u64) {
        self.db.wal_end()
    }

    /// `len` bytes of `segment`'s file of the log from `offset` (decision 45); None when there is
    /// no such file. The writer may switch at any moment, so a caller that reads a generation's
    /// frames names the segment its header was in ([`Db::log_header`]), never the one current now.
    pub(crate) fn read_log(
        &self,
        segment: SegmentId,
        offset: u64,
        len: usize,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.read_at(&segment_path(&self.path, segment), offset, len)
    }

    /// The segment current now and the header its file of the log holds ([`Db::segment_header`]):
    /// its own, or that of the generation the file held before, until its first commit writes its
    /// own.
    pub(crate) fn log_header(&self) -> anyhow::Result<Option<(SegmentId, Vec<u8>)>> {
        let segment = self.db.wal_segment();
        Ok(self
            .segment_header(segment)?
            .map(|header| (segment, header)))
    }

    /// The header `segment`'s file of the log holds (decision 45). None while the file holds none,
    /// which is no failure: a switch makes the file, or cuts it to nothing, and the segment's first
    /// commit writes its header; a log folded whole is emptied too.
    pub(crate) fn segment_header(&self, segment: SegmentId) -> anyhow::Result<Option<Vec<u8>>> {
        let len = usize::try_from(LOG_HEADER_BYTES)?;
        Ok(self
            .read_to(&segment_path(&self.path, segment), 0, len)?
            .filter(|header| header.len() == len))
    }

    /// Whether the file holds every frame committed before the log's current segment (decision
    /// 45): what a copy of the file needs before it begins in the current generation.
    pub(crate) fn closing_copied(&self) -> bool {
        self.db.wal_closing_copied()
    }

    /// `len` bytes of the database's file from `offset`.
    pub(crate) fn read_file(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_at(&self.path.display().to_string(), offset, len)?
            .ok_or_else(|| anyhow::anyhow!("{} is gone", self.path.display()))
    }

    /// The pages of the database now, those only its log holds yet among them: the file a restore
    /// of it makes.
    pub(crate) fn pages(&self) -> anyhow::Result<u64> {
        let mut pages = None;
        self.query("PRAGMA page_count", [], |row| {
            pages = Some(row.get::<i64>(0).map_err(fail)?);
            Ok(false)
        })?;
        let pages = pages.ok_or_else(|| anyhow::anyhow!("PRAGMA page_count answered no row"))?;
        Ok(u64::try_from(pages)?)
    }

    /// The bytes the database's file holds.
    pub(crate) fn file_bytes(&self) -> anyhow::Result<u64> {
        let file = self
            .io
            .open_file(&self.path.display().to_string(), OpenFlags::NoLock, false)
            .map_err(fail)?;
        file.size().map_err(fail)
    }

    /// `len` bytes of `path` from `offset`; None when there is no such file, and refused when it
    /// holds fewer.
    fn read_at(&self, path: &str, offset: u64, len: usize) -> anyhow::Result<Option<Vec<u8>>> {
        let bytes = self.read_to(path, offset, len)?;
        if let Some(bytes) = &bytes
            && bytes.len() != len
        {
            anyhow::bail!("{path} held {} bytes at {offset}, not {len}", bytes.len());
        }
        Ok(bytes)
    }

    /// What `path` holds of the `len` bytes from `offset`, fewer past its end; None when there is
    /// no such file.
    fn read_to(&self, path: &str, offset: u64, len: usize) -> anyhow::Result<Option<Vec<u8>>> {
        let file = match self.io.open_file(path, OpenFlags::NoLock, false) {
            Ok(file) => file,
            Err(LimboError::CompletionError(CompletionError::IOError(
                std::io::ErrorKind::NotFound,
                _,
            ))) => return Ok(None),
            Err(e) => return Err(fail(e)),
        };
        if len == 0 {
            return Ok(Some(Vec::new()));
        }
        let got: Arc<std::sync::Mutex<Option<Vec<u8>>>> = Arc::default();
        let into = Arc::clone(&got);
        let completion = Completion::new_read(Arc::new(Buffer::new_temporary(len)), move |read| {
            if let Ok((buffer, n)) = read
                && let Ok(n) = usize::try_from(n)
                && let Some(bytes) = buffer.as_slice().get(..n)
                && let Ok(mut slot) = into.lock()
            {
                *slot = Some(bytes.to_vec());
            }
            None
        });
        let completion = file.pread(offset, completion).map_err(fail)?;
        self.io.wait_for_completion(completion).map_err(fail)?;
        let bytes = got
            .lock()
            .map_err(|_| anyhow::anyhow!("a read's lock was poisoned"))?
            .take()
            .ok_or_else(|| anyhow::anyhow!("reading {path} failed"))?;
        Ok(Some(bytes))
    }

    /// Copies the whole log into the file and empties it, with no reader reading: a restore's
    /// replay of one generation (docs/design/backup.md).
    pub(crate) fn fold_log(&mut self) -> anyhow::Result<()> {
        self.ensure_unfenced()?;
        self.conn
            .checkpoint(CheckpointMode::Truncate {
                upper_bound_inclusive: None,
            })
            .map(|_| ())
            .map_err(fail)
    }

    /// Folds the log into the file through `frame` (inclusive), leaving the rest in the log and the
    /// log as it is: a replay in parts, each a step a paced check can pause after (T-T1.12.C8). The
    /// engine's passive checkpoint, bounded (`CheckpointMode::Passive`'s `upper_bound_inclusive`).
    pub(crate) fn fold_log_through(&mut self, frame: u64) -> anyhow::Result<()> {
        self.ensure_unfenced()?;
        self.conn
            .checkpoint(CheckpointMode::Passive {
                upper_bound_inclusive: Some(frame),
            })
            .map(|_| ())
            .map_err(fail)
    }

    /// [`Db::query`] uncached, `between` called each time the statement waits on the device: a long
    /// statement (`PRAGMA integrity_check`) a paced check can pause inside (T-T1.12.C8).
    pub(crate) fn query_between(
        &self,
        sql: &str,
        each: impl FnMut(&Row) -> anyhow::Result<bool>,
        between: &mut dyn FnMut() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.ensure_unfenced()?;
        let mut stmt = self.conn.prepare(sql).map_err(fail)?;
        step_between(self.io.as_ref(), &mut stmt, [], each, between)
    }

    /// The frames the log's current segment holds now, from the log's shared state. This
    /// connection's own view is no count of it: a switch (decision 45, [`Db::restart_log`]) leaves
    /// that view on the segment it closed until the connection's next transaction, so a replica
    /// past its room would refuse every submit after the restart that brought it back within.
    pub(crate) fn log_frames_in_use(&self) -> anyhow::Result<u64> {
        Ok(self.db.wal_backfill().0)
    }

    /// The frames of log a restart keeps the log's file for: past them, it cuts the file to nothing
    /// (decision 31). By default twice the store's default bound. A caller using a nondefault
    /// `Config::log_frames` may pass twice that bound here; changing Config alone does not set it.
    pub fn keep_log(&mut self, frames: u64) {
        self.log_kept = frames;
    }

    /// The last frame of the log a flush made durable in its current generation, if any: what a
    /// pass copies through ([`Db::checkpoint`]).
    pub(crate) fn durable_frame(&self) -> Option<u64> {
        self.db.wal_durable_frame()
    }

    /// The frames the log's file holds room for: none without a log.
    pub(crate) fn log_frames(&self) -> anyhow::Result<u64> {
        let wal = segment_path(&self.path, self.db.wal_segment());
        let bytes = match self.io.open_file(&wal, OpenFlags::NoLock, false) {
            Ok(file) => file.size().map_err(fail)?,
            Err(LimboError::CompletionError(CompletionError::IOError(
                std::io::ErrorKind::NotFound,
                _,
            ))) => 0,
            Err(e) => return Err(fail(e)),
        };
        let frame = PAGE_BYTES
            .checked_add(FRAME_HEADER_BYTES)
            .ok_or_else(|| anyhow::anyhow!("a frame's bytes overflow"))?;
        // A file shorter than its header holds no frame.
        Ok(bytes
            .saturating_sub(LOG_HEADER_BYTES)
            .checked_div(frame)
            .unwrap_or(0))
    }

    /// The file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn hold_owner(&mut self, owner: crate::database::Owned) {
        self.owner = Some(owner);
    }

    /// The format the log's generations take from the next one written (docs/design/store.md,
    /// decisions 16, 17 and 37): mixed as opened, commits logged as records of their changes and
    /// spills as pages. A generation keeps the format it began with. A packed log's pages aren't
    /// shared: a database that packs it is opened with [`Sharing::Off`].
    pub fn log_format(&self, format: LogFormat) -> anyhow::Result<()> {
        self.db
            .wal_format(format)
            .map_err(|e| anyhow::anyhow!("setting {}'s log format: {e}", self.path.display()))
    }

    /// Shares the pages this database's connections read among them, `pages` at most
    /// (docs/design/store.md, decision 20): a page one reader read in, the others read from
    /// memory. For a database opened with [`Sharing::Off`]; one that shares already refuses.
    pub fn share_pages(&self, pages: usize) -> anyhow::Result<()> {
        self.db
            .share_pages(pages)
            .map_err(|e| anyhow::anyhow!("sharing {}'s pages: {e}", self.path.display()))
    }

    /// Shares this database's pages in `pool`, with every database that shares it: one budget for
    /// a process's databases (docs/design/store.md, decision 20). For a database opened with
    /// [`Sharing::Off`]; one that shares already refuses.
    pub fn share_pages_in(&self, pool: &Arc<SharedPages>) -> anyhow::Result<()> {
        self.db
            .share_pages_in(pool)
            .map_err(|e| anyhow::anyhow!("sharing {}'s pages: {e}", self.path.display()))
    }

    /// Statements that answer nothing, one after another.
    pub fn exec(&self, sql: &str) -> anyhow::Result<()> {
        self.ensure_unfenced()?;
        // The engine's parser knows where a statement ends, including trigger bodies and quoted
        // semicolons. U05: splitting text on ';' corrupts valid migration scripts.
        let result = self.conn.prepare_execute_batch(sql).map_err(fail);
        // A failed script may have executed DDL before refusing. Invalidate on its actual schema,
        // including rollback and raw-image reparses, without flushing every BEGIN/COMMIT.
        self.statements.schema(&self.conn.current_schema())?;
        self.marked()?;
        result
    }

    /// [`Db::exec`] of an online schema change's own statement (CP-T1.12): the engine lets only
    /// the change's own `ALTER COLUMN` redefine a column it holds. The mark is off again however
    /// the statement ends.
    pub(crate) fn exec_as_change(&self, sql: &str) -> anyhow::Result<()> {
        self.conn.set_online_change_own(true);
        let result = self.exec(sql);
        self.conn.set_online_change_own(false);
        result
    }

    /// Owned/dynamic SQL without an unbounded statement cache (U05). The caller bounds results.
    /// It keeps its cache entry's validation mark and never sets one: only the session validates.
    pub fn query_owned(
        &self,
        sql: &str,
        params: impl IntoIterator<Item = Value>,
        each: impl FnMut(&Row) -> anyhow::Result<bool>,
    ) -> anyhow::Result<()> {
        self.ensure_unfenced()?;
        let schema = self.conn.current_schema();
        let (text, mut stmt, marked) = match self.statements.take(sql, &schema)? {
            Some(taken) => (Some(taken.text), taken.statement, taken.validated),
            None => (None, self.conn.prepare(sql).map_err(fail)?, false),
        };
        step(self.io.as_ref(), &mut stmt, params, each)?;
        // A callback can change schema and run another query. The cache never keeps this
        // checked-out statement under that later schema's cache identity.
        let current = self.conn.current_schema();
        self.statements
            .put(sql, text, stmt, marked, &schema, &current)?;
        self.marked()
    }

    pub fn changes(&self) -> i64 {
        self.conn.changes()
    }

    /// Keeps its cache entry's validation mark and never sets one, as [`Db::query_owned`].
    pub fn column_count(&self, sql: &str) -> anyhow::Result<usize> {
        self.ensure_unfenced()?;
        let schema = self.conn.current_schema();
        let (text, stmt, marked) = match self.statements.take(sql, &schema)? {
            Some(taken) => (Some(taken.text), taken.statement, taken.validated),
            None => (None, self.conn.prepare(sql).map_err(fail)?, false),
        };
        let columns = stmt.num_columns();
        self.statements
            .put(sql, text, stmt, marked, &schema, &schema)?;
        Ok(columns)
    }

    /// Takes `sql`'s statement out of the cache, with its validation mark, for a caller that runs
    /// it again and again with [`Db::run_held`] until [`Db::give_back`] returns it
    /// (docs/design/store.md, decisions 34 and 36). While it's held, the same text is prepared
    /// anew for anyone else.
    pub(crate) fn cached(&self, sql: &str) -> anyhow::Result<Option<Held>> {
        self.ensure_unfenced()?;
        let schema = self.conn.current_schema();
        Ok(self.statements.take(sql, &schema)?.map(|taken| Held {
            sql: sql.to_owned(),
            statement: taken.statement,
            schema,
            validated: taken.validated,
        }))
    }

    /// Prepares `sql` for a caller that holds it as [`Db::cached`] says, unmarked.
    pub(crate) fn prepare_held(&self, sql: &str) -> anyhow::Result<Held> {
        self.ensure_unfenced()?;
        let schema = self.conn.current_schema();
        self.statements.schema(&schema)?;
        Ok(Held {
            sql: sql.to_owned(),
            statement: self.conn.prepare(sql).map_err(fail)?,
            schema,
            validated: false,
        })
    }

    /// Runs a held statement as [`Db::query_owned`] runs one, preparing it again first if the
    /// schema changed since it was prepared.
    pub(crate) fn run_held(
        &self,
        held: &mut Held,
        params: impl IntoIterator<Item = Value>,
        each: impl FnMut(&Row) -> anyhow::Result<bool>,
    ) -> anyhow::Result<()> {
        self.ensure_unfenced()?;
        let schema = self.conn.current_schema();
        self.statements.schema(&schema)?;
        if !same_schema(&held.schema, &schema) {
            held.statement = self.conn.prepare(&held.sql).map_err(fail)?;
            held.schema = schema;
        }
        step(self.io.as_ref(), &mut held.statement, params, each)
    }

    /// Returns a held statement to the cache with its mark, kept for the next caller while the
    /// schema is the one it was prepared under.
    pub(crate) fn give_back(&self, held: Held) -> anyhow::Result<()> {
        let current = self.conn.current_schema();
        self.statements.put(
            &held.sql,
            None,
            held.statement,
            held.validated,
            &held.schema,
            &current,
        )?;
        Ok(())
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.conn.last_insert_rowid()
    }

    /// Validates imported rows using the engine's resolved FK metadata. The engine currently
    /// implements foreign_key_list and enforcement, but not PRAGMA foreign_key_check; an unknown
    /// PRAGMA returns no rows and therefore cannot prove a restored database consistent.
    pub(crate) fn validate_foreign_keys(&self) -> anyhow::Result<()> {
        self.ensure_unfenced()?;
        let schema = self.conn.current_schema();
        let quote = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
        for table in schema.tables.values() {
            let Some(table) = table.btree() else { continue };
            for fk in schema.resolved_fks_for_child(&table.name).map_err(fail)? {
                let mut present = Vec::new();
                let mut equal = Vec::new();
                for (child, parent) in fk.fk.child_columns.iter().zip(fk.parent_cols.iter()) {
                    present.push(format!("c.{} IS NOT NULL", quote(child)));
                    // Parent on the left gives its declared affinity and collation, as FK
                    // enforcement requires; a NULL in any child column exempts that row.
                    equal.push(format!("p.{} = c.{}", quote(parent), quote(child)));
                }
                let sql = format!(
                    "SELECT 1 FROM {} AS c WHERE {} AND NOT EXISTS (SELECT 1 FROM {} AS p WHERE {}) LIMIT 1",
                    quote(&table.name),
                    present.join(" AND "),
                    quote(&fk.fk.parent_table),
                    equal.join(" AND ")
                );
                let mut invalid = false;
                self.query_owned(&sql, [], |_| {
                    invalid = true;
                    Ok(false)
                })?;
                if invalid {
                    anyhow::bail!(
                        "foreign key validation failed: {} referencing {}",
                        table.name,
                        fk.fk.parent_table
                    );
                }
            }
        }
        Ok(())
    }

    fn check_fence(fenced: &AtomicBool) -> anyhow::Result<()> {
        if fenced.load(Ordering::Acquire) {
            return Err(amphora_api::Refusal::Fenced(
                "compaction rollback did not reset the writer; reopen after recovery".into(),
            )
            .into());
        }
        Ok(())
    }

    fn ensure_unfenced(&self) -> anyhow::Result<()> {
        Self::check_fence(&self.fenced)
    }

    fn verify_compaction_reset(&self) -> anyhow::Result<()> {
        if !self.conn.get_auto_commit() || self.conn.get_pager().dirty_page_count() != 0 {
            self.fenced.store(true, Ordering::Release);
        }
        self.ensure_unfenced()
    }

    /// The page fence applies to all modifications, including catalogs, indexes and durable GC
    /// progress. Only deterministic compaction work is retried, and only after a whole rollback.
    pub(crate) fn compaction_attempt<R>(
        &mut self,
        pages: usize,
        prepare: bool,
        f: impl FnOnce(&mut Self) -> anyhow::Result<R>,
    ) -> anyhow::Result<(R, Pages)> {
        self.ensure_unfenced()?;
        self.verify_compaction_reset()?;
        if pages == 0 {
            return Err(amphora_api::invalid("compaction page limit", "must be nonzero").into());
        }
        if self.conn.get_page_size().get() != 4096 {
            return Err(
                amphora_api::invalid("bounded compaction", "requires 4096-byte pages").into(),
            );
        }
        let pager = self.conn.get_pager();
        pager.set_transaction_page_limit(pages).map_err(fail)?;
        // Always release the admission fence before rollback restoration, including BEGIN errors.
        let result = (|| {
            self.exec("BEGIN IMMEDIATE")?;
            let answer = f(self)?;
            let image = if prepare {
                pager
                    .transaction_image(
                        pages
                            .checked_mul(4096)
                            .ok_or_else(|| anyhow::anyhow!("compaction image size overflow"))?,
                    )
                    .map_err(fail)?
            } else {
                Vec::new()
            };
            if !prepare {
                self.exec("COMMIT")?;
            }
            Ok((answer, image))
        })();
        pager.set_transaction_page_limit(0).map_err(fail)?;
        if !self.conn.get_auto_commit()
            && let Err(error) = self.exec("ROLLBACK")
        {
            self.fenced.store(true, Ordering::Release);
            return Err(amphora_api::Refusal::Fenced(format!(
                "compaction rollback failed: {error:#}; reopen after recovery"
            ))
            .into());
        }
        self.verify_compaction_reset()?;
        result
    }

    /// Captures a speculative writer transaction without publishing a commit. The engine's
    /// image includes spilled dirty pages; no random/default/trigger expression is replayed.
    pub(crate) fn prepare_pages<R>(
        &mut self,
        max_bytes: usize,
        f: impl FnOnce(&mut Self) -> anyhow::Result<R>,
    ) -> anyhow::Result<(R, Pages)> {
        self.exec("BEGIN IMMEDIATE")?;
        let result = f(self).and_then(|answer| {
            let pages = self
                .conn
                .get_pager()
                .transaction_image(max_bytes)
                .map_err(fail)?;
            Ok((answer, pages))
        });
        // A prepared transaction never leaves an engine commit marker, even when capture fails.
        self.exec("ROLLBACK")?;
        result
    }

    /// `prepare_pages`, answering also what the session read: its read set (`None` when it read
    /// more than `bound` pages, as if unknown) and page 1 as it was at the session's base. The
    /// record ends before capture: capturing reads every dirty page again, which the session itself
    /// may not have read (replication.md, "A follower's write that read nothing written since its
    /// base: read sets").
    pub(crate) fn prepare_traced<R>(
        &mut self,
        max_bytes: usize,
        bound: u64,
        f: impl FnOnce(&mut Self) -> anyhow::Result<R>,
    ) -> anyhow::Result<(R, Pages, Traced)> {
        self.exec("BEGIN IMMEDIATE")?;
        let pager = self.conn.get_pager();
        let result = self.page_one().and_then(|page_one| {
            pager.record_access(bound).map_err(fail)?;
            let answer = f(self)?;
            let access = pager.take_access().map_err(fail)?;
            let pages = pager.transaction_image(max_bytes).map_err(fail)?;
            let reads = (!access.overflowed()).then(|| access.read().collect());
            Ok((answer, pages, Traced { reads, page_one }))
        });
        pager.end_access();
        // A prepared transaction never leaves an engine commit marker, even when capture fails.
        self.exec("ROLLBACK")?;
        result
    }

    /// Page 1's bytes in the transaction under way.
    fn page_one(&self) -> anyhow::Result<Vec<u8>> {
        let pager = self.conn.get_pager();
        let (page, completion) = self.io.block(|| pager.read_page(1))?;
        if let Some(completion) = completion {
            self.io.wait_for_completion(completion).map_err(fail)?;
        }
        if !page.is_loaded() {
            anyhow::bail!("page 1 was not loaded");
        }
        Ok(page.get_contents().as_ptr().to_vec())
    }

    /// The root page of table `name`: the replica's marker table's, which every apply writes.
    pub(crate) fn root_page(&self, name: &str) -> anyhow::Result<u32> {
        let mut root = None;
        self.query_owned(
            "SELECT rootpage FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [Value::from_text(name.to_string())],
            |row| {
                root = Some(row.get::<i64>(0)?);
                Ok(false)
            },
        )?;
        let root = root.ok_or_else(|| anyhow::anyhow!("table {name} has no root page"))?;
        Ok(u32::try_from(root)?)
    }

    /// The pages an image of this database holds now, and their bytes.
    fn image_extent(&self) -> anyhow::Result<(u32, usize)> {
        let pager = self.conn.get_pager();
        let count = self
            .io
            .block(|| pager.with_header(|h| h.database_size.get()))?;
        let page_size = usize::try_from(self.conn.get_page_size().get())?;
        let size = usize::try_from(count)?
            .checked_mul(page_size)
            .ok_or_else(|| anyhow::anyhow!("snapshot size overflow"))?;
        Ok((count, size))
    }

    /// The bytes an image of this database would hold now, as `image_pages` counts them.
    pub(crate) fn image_bytes(&mut self) -> anyhow::Result<usize> {
        self.exec("BEGIN")?;
        let result = self.image_extent().map(|(_, size)| size);
        self.exec("ROLLBACK")?;
        result
    }

    pub(crate) fn image_pages(&mut self, max_bytes: usize) -> anyhow::Result<Pages> {
        self.exec("BEGIN")?;
        let result = (|| {
            let (count, size) = self.image_extent()?;
            if size > max_bytes {
                anyhow::bail!("snapshot exceeds its byte budget");
            }
            let pager = self.conn.get_pager();
            let mut pages = Vec::with_capacity(usize::try_from(count)?);
            for id in 1..=count {
                let (page, completion) = self.io.block(|| pager.read_page(i64::from(id)))?;
                if let Some(completion) = completion {
                    self.io.wait_for_completion(completion).map_err(fail)?;
                }
                if !page.is_loaded() {
                    anyhow::bail!("snapshot page was not loaded");
                }
                pages.push((id, page.get_contents().as_ptr().to_vec()));
            }
            Ok(pages)
        })();
        self.exec("ROLLBACK")?;
        result
    }

    /// Applies resolved pages and the caller's applied-index update in one WAL transaction.
    /// Raw pages have no commit marker; only the final SQL marker update publishes the batch.
    pub(crate) fn apply_pages<R>(
        &mut self,
        pages: &[(u32, Vec<u8>)],
        finish: impl FnOnce(&mut Self) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        self.statements.clear()?;
        self.ensure_unfenced()?;
        self.conn.wal_insert_begin().map_err(fail)?;
        let result = (|| {
            let mut frame_no = self.conn.wal_state().map_err(fail)?.max_frame;
            for (page, bytes) in pages {
                if *page == 0 || bytes.len() != usize::try_from(self.conn.get_page_size().get())? {
                    anyhow::bail!("invalid replicated page");
                }
                frame_no = frame_no
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("frame number overflow"))?;
                let mut frame = Vec::with_capacity(
                    bytes
                        .len()
                        .checked_add(24)
                        .ok_or_else(|| anyhow::anyhow!("frame size overflow"))?,
                );
                frame.extend_from_slice(&page.to_be_bytes());
                frame.extend_from_slice(&[0; 20]);
                frame.extend_from_slice(bytes);
                self.conn.wal_insert_frame(frame_no, &frame).map_err(fail)?;
            }
            self.conn.reparse_unpublished_schema().map_err(fail)?;
            finish(self)
        })();
        match result {
            Ok(answer) => {
                self.conn.wal_insert_end(true).map_err(fail)?;
                self.marked()?;
                // An installed image can replace a catalog at an equal or older schema cookie.
                // Publish that exact committed schema so readers cannot re-adopt the old one.
                self.conn
                    .publish_schema_after_external_restore()
                    .map_err(fail)?;
                self.statements.clear()?;
                Ok(answer)
            }
            Err(error) => {
                self.conn.wal_insert_end(false).map_err(fail)?;
                self.conn.force_reparse_schema().map_err(fail)?;
                self.statements.clear()?;
                Err(error)
            }
        }
    }

    /// Runs `sql` with `params`, its statement prepared the first time and kept, handing each row
    /// to `each` until it says stop (false).
    /// The bytes of page `pgno` of the database as its pager holds them (CP-T1.9, T-T1.9.R19: a
    /// group's size cut reads its position index's root page alone). Asked inside a transaction
    /// that has read, so the page is that transaction's.
    pub(crate) fn page(&self, pgno: i64) -> anyhow::Result<Vec<u8>> {
        let pager = self.conn.get_pager();
        let (page, completion) = pager
            .io
            .block(|| pager.read_page(pgno))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(completion) = completion {
            pager
                .io
                .wait_for_completion(completion)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        Ok(page.get_contents().as_ptr().to_vec())
    }

    pub fn query(
        &self,
        sql: &'static str,
        params: impl IntoIterator<Item = Value>,
        each: impl FnMut(&Row) -> anyhow::Result<bool>,
    ) -> anyhow::Result<()> {
        self.query_owned(sql, params, each)
    }

    /// The schema cookie moved on and this writer's schema read again inside its transaction, for an
    /// online schema change's state rows (`amphora_engine::schema::INDEX_STATES_TABLE`) to take:
    /// the writer sees them at once, every other connection at the cookie when this commits
    /// (CP-T1.12, docs/design/online-schema.md). The caller holds the write transaction.
    pub(crate) fn restage_schema(&self) -> anyhow::Result<()> {
        let mut version = None;
        self.query("PRAGMA schema_version", [], |row| {
            version = Some(row.get::<i64>(0).map_err(fail)?);
            Ok(false)
        })?;
        // The cookie is a 32-bit field of the database header (sqlite.org/fileformat2.html, offset
        // 40), which the engine's SetCookie writes as an i32.
        let next = version
            .and_then(|v| v.checked_add(1))
            .filter(|v| i32::try_from(*v).is_ok())
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| anyhow::anyhow!("the schema cookie can't move past {version:?}"))?;
        // Not `PRAGMA schema_version = n`: the engine makes that a no-op (SQLite's defensive mode),
        // so the cookie would stay, and readers would keep the schema from before the step. This
        // writes the header's cookie, marks the transaction's schema changed (published to every
        // connection when it commits) and reads this writer's schema again.
        self.conn.write_schema_version(next).map_err(fail)?;
        self.statements.schema(&self.conn.current_schema())?;
        Ok(())
    }

    /// One batch of a write-only index's backfill (CP-T1.12): the rows after rowid `after`, at most
    /// `rows`, each given its entry. The last rowid it visited (None: none was past `after`) and
    /// how many it visited. The caller holds the write transaction.
    pub(crate) fn index_backfill(
        &self,
        index: &str,
        after: Option<i64>,
        rows: u64,
    ) -> anyhow::Result<(Option<i64>, u64)> {
        self.ensure_unfenced()?;
        let mut stmt = self
            .conn
            .prepare_index_backfill(index, after, rows)
            .map_err(fail)?;
        let mut answer = None;
        step(self.io.as_ref(), &mut stmt, [], |row| {
            let last = match row.get::<&Value>(0).map_err(fail)? {
                Value::Null => None,
                _ => Some(row.get::<i64>(0).map_err(fail)?),
            };
            let visited = u64::try_from(row.get::<i64>(1).map_err(fail)?)?;
            answer = Some((last, visited));
            // Run on to its Halt: a write statement stopped at its row is abandoned unfinished,
            // and the engine then refuses the transaction's commit.
            Ok(true)
        })?;
        answer.ok_or_else(|| anyhow::anyhow!("the backfill of {index} answered nothing"))
    }

    /// `f` in one transaction: committed if it returns Ok and the commit goes through, rolled back
    /// whole if not (a commit that fails can leave the transaction open; the next would then
    /// refuse to begin).
    pub fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        // A session can read before it writes. Reserve this owner's writer at the start so
        // checkpoint/coordinator changes cannot invalidate a deferred snapshot mid-closure;
        // an FnOnce session cannot be replayed after an upgrade fails.
        self.query("BEGIN IMMEDIATE", [], |_| Ok(true))?;
        let done = f(self).and_then(|v| self.query("COMMIT", [], |_| Ok(true)).map(|()| v));
        if done.is_err() && !self.conn.get_auto_commit() {
            // What the failure already says is what the caller hears; a rollback that fails too
            // leaves the connection to refuse the next transaction, which says so.
            let _ = self.query("ROLLBACK", [], |_| Ok(true));
        }
        done
    }

    /// `f` in one read transaction (`BEGIN`, deferred), its statements on one snapshot of what was
    /// committed when it began, with every statement that writes refused (the engine's
    /// `query_only`) and the transaction ended by a rollback, so nothing it did stays: a query
    /// beside the writer (docs/design/store.md, decision 33).
    pub(crate) fn query_transaction<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.conn.set_query_only(true);
        let done = self.query("BEGIN", [], |_| Ok(true)).and_then(|()| f(self));
        let ended = if self.conn.get_auto_commit() {
            Ok(())
        } else {
            self.query("ROLLBACK", [], |_| Ok(true))
        };
        self.conn.set_query_only(false);
        let value = done?;
        ended?;
        Ok(value)
    }

    /// `f` with every statement that writes refused (the engine's `query_only`), in the
    /// transaction under way: a replicated query's session (`Database::query`).
    pub(crate) fn query_only<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.conn.set_query_only(true);
        let done = f(self);
        self.conn.set_query_only(false);
        done
    }

    /// `f` on each of `batches`, in order, in one transaction ([`Db::transaction`]) that ends
    /// before a batch its cache might not hold: once the pages written so far, and as many again
    /// as the last batch wrote, pass what the cache holds before it spills written pages into the
    /// log (docs/design/store.md, decision 23). The first batch always goes in. How many it
    /// committed: the rest are the caller's to hand to the next.
    pub fn transaction_while_room<B>(
        &mut self,
        batches: impl IntoIterator<Item = B>,
        mut f: impl FnMut(&mut Self, B) -> anyhow::Result<()>,
    ) -> anyhow::Result<usize> {
        self.transaction(|db| {
            let mut done = 0usize;
            let mut last = 0usize;
            for batch in batches {
                if done > 0 {
                    let written = db.conn.dirty_pages();
                    let grew = written.saturating_sub(last);
                    if written.saturating_add(grew) > db.conn.pages_before_spill() {
                        break;
                    }
                    last = written;
                }
                f(db, batch)?;
                done = done.saturating_add(1);
            }
            Ok(done)
        })
    }
}

/// A statement held by its caller between executes: its text, the schema it was prepared under,
/// and whether the session's validation accepted the text.
pub(crate) struct Held {
    sql: String,
    statement: Statement,
    schema: Arc<amphora_engine::schema::Schema>,
    validated: bool,
}

impl Held {
    pub(crate) fn columns(&self) -> usize {
        self.statement.num_columns()
    }

    /// Its result columns' names, in order.
    pub(crate) fn column_names(&self) -> Vec<String> {
        (0..self.statement.num_columns())
            .map(|i| self.statement.get_column_name(i).into_owned())
            .collect()
    }

    pub(crate) fn validated(&self) -> bool {
        self.validated
    }

    /// The session's validation accepted the text: only `sql::validate`'s success calls this.
    pub(crate) fn mark(&mut self) {
        self.validated = true;
    }
}

/// The same schema, not another at the same address: the cache's own test (`cache::Cache::schema`).
fn same_schema(
    held: &Arc<amphora_engine::schema::Schema>,
    current: &Arc<amphora_engine::schema::Schema>,
) -> bool {
    Arc::ptr_eq(held, current) && held.schema_version == current.schema_version
}

/// Runs `stmt` with `params` to its end, handing each row to `each` until it says stop (false).
pub fn step(
    io: &dyn IO,
    stmt: &mut Statement,
    params: impl IntoIterator<Item = Value>,
    each: impl FnMut(&Row) -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    step_between(io, stmt, params, each, &mut || Ok(()))
}

/// [`step`], `between` called each time the statement waits on the device, before it does: a long
/// statement's points to pause at (T-T1.12.C8).
fn step_between(
    io: &dyn IO,
    stmt: &mut Statement,
    params: impl IntoIterator<Item = Value>,
    mut each: impl FnMut(&Row) -> anyhow::Result<bool>,
    between: &mut dyn FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    stmt.reset().map_err(fail)?;
    for (i, p) in params.into_iter().enumerate() {
        let at = NonZero::new(i.saturating_add(1)).ok_or_else(|| anyhow::anyhow!("parameter 0"))?;
        stmt.bind_at(at, p).map_err(fail)?;
    }
    // The engine wakes this thread when a lock the statement found held is released, and the
    // statement tries again then: no delay of its own between tries (lane constants-22).
    // `try_with`, not `with`: a statement stepped while the thread's locals are torn down gets a
    // waker of its own rather than a panic.
    let waker = UNPARK
        .try_with(std::task::Waker::clone)
        .unwrap_or_else(|_| std::task::Waker::from(Arc::new(Unpark(std::thread::current()))));
    loop {
        match stmt.step_with_waker(&waker).map_err(fail)? {
            StepResult::Row => {
                if let Some(row) = stmt.row()
                    && !each(row)?
                {
                    return stmt.reset().map_err(fail);
                }
            }
            StepResult::Done => return Ok(()),
            StepResult::IO | StepResult::Yield => {
                between()?;
                io.step().map_err(fail)?;
            }
            // Busy: until the waker is woken (a release), or the deadline the duration says. A park
            // that ends early for no reason only tries the step again.
            StepResult::Sleep { duration } => {
                if stmt.busy_wake() == BusyWake::Schedule {
                    pause(duration);
                } else if !duration.is_zero() {
                    std::thread::park_timeout(duration);
                }
                io.step().map_err(fail)?;
            }
            StepResult::Busy => {
                anyhow::bail!("the database stayed busy")
            }
            StepResult::Interrupt => return Err(fail(LimboError::Interrupt)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counts its wakes and unparks the thread that made it.
    struct Woken {
        thread: std::thread::Thread,
        wakes: std::sync::atomic::AtomicUsize,
    }
    impl std::task::Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.wakes.fetch_add(1, Ordering::SeqCst);
            self.thread.unpark();
        }
    }

    /// A failed write's own cleanup cannot be the event it waits for.
    #[test]
    fn busy_write_does_not_wake_itself_on_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let holder = Db::open(&dir.path().join("busy.db")).unwrap();
        holder.exec("CREATE TABLE t(x)").unwrap();
        let waiter = holder.reader().unwrap();
        waiter.conn.set_busy_wake(BusyWake::Release);
        holder.exec("BEGIN IMMEDIATE").unwrap();
        holder.exec("INSERT INTO t VALUES (1)").unwrap();
        let task = Arc::new(Woken {
            thread: std::thread::current(),
            wakes: Default::default(),
        });
        let waker = std::task::Waker::from(task);
        let mut stmt = waiter.conn.prepare("INSERT INTO t VALUES (2)").unwrap();
        let mut waits = Vec::new();
        while waits.len() < 4 {
            match stmt.step_with_waker(&waker).unwrap() {
                StepResult::IO | StepResult::Yield => waiter.io.step().unwrap(),
                StepResult::Sleep { duration } => {
                    waits.push(duration);
                    if !duration.is_zero() {
                        break;
                    }
                }
                other => panic!("expected a blocked write, got {other:?}"),
            }
        }
        eprintln!("busy writer sleeps with the same holder: {waits:?}");
        holder.exec("COMMIT").unwrap();
        assert!(waits.iter().any(|d| !d.is_zero()), "{waits:?}");
    }

    /// T-QB.1.14 (lane constants-22; goal dfabffcc, "every wait wakes on an event"): a write busy
    /// on another connection's write lock wakes on its release. One connection holds the write lock; the
    /// other's insert finds it held and parks, saying so; then the first commits. Under the release
    /// wake (the default) nothing woke the waiter before the commit, and the commit's release does;
    /// under SQLite's schedule (the control) the engine woke it at once, before any release, and it
    /// tries again on the schedule's delays. Each prints the time from the commit to the waiter's
    /// insert done (its wake latency, for the A/B; nothing here asserts a time).
    #[test]
    fn constants_22_a_busy_write_wakes_on_the_write_locks_release() {
        for wake in [BusyWake::Release, BusyWake::Schedule] {
            let dir = tempfile::tempdir().unwrap();
            let holder = Db::open(&dir.path().join("busy.db")).unwrap();
            holder.exec("CREATE TABLE t (x INTEGER)").unwrap();
            let waiter = holder.reader().unwrap();
            waiter.conn.set_busy_wake(wake);
            holder.exec("BEGIN IMMEDIATE").unwrap();
            holder.exec("INSERT INTO t VALUES (1)").unwrap();
            let (parked_tx, parked) = std::sync::mpsc::channel();
            let conn = Arc::clone(&waiter.conn);
            let io = Arc::clone(&waiter.io);
            let thread = std::thread::spawn(move || {
                let woken = Arc::new(Woken {
                    thread: std::thread::current(),
                    wakes: std::sync::atomic::AtomicUsize::new(0),
                });
                let waker = std::task::Waker::from(Arc::clone(&woken));
                let mut stmt = conn.prepare("INSERT INTO t VALUES (2)").unwrap();
                let mut woken_before = None;
                loop {
                    match stmt.step_with_waker(&waker).unwrap() {
                        StepResult::Done => break,
                        StepResult::IO | StepResult::Yield => io.step().unwrap(),
                        StepResult::Sleep { duration } => {
                            // The first wait it holds (a zero sleep is a try again at once).
                            if woken_before.is_none() && !duration.is_zero() {
                                woken_before = Some(woken.wakes.load(Ordering::SeqCst));
                                parked_tx.send(()).unwrap();
                            }
                            if wake == BusyWake::Schedule {
                                pause(duration);
                            } else if !duration.is_zero() {
                                std::thread::park_timeout(duration);
                            }
                            io.step().unwrap();
                        }
                        other => panic!("the waiting insert stepped to {other:?}"),
                    }
                }
                (
                    Instant::now(),
                    woken_before,
                    woken.wakes.load(Ordering::SeqCst),
                )
            });
            parked.recv().unwrap();
            let committed = Instant::now();
            holder.exec("COMMIT").unwrap();
            let (done, woken_before, wakes) = thread.join().unwrap();
            eprintln!(
                "constants-22 wake={wake:?} commit_to_done={:?} woken_before_release={woken_before:?} wakes={wakes}",
                done.saturating_duration_since(committed)
            );
            match wake {
                BusyWake::Release => {
                    assert_eq!(
                        woken_before,
                        Some(0),
                        "nothing wakes a busy write before a release"
                    );
                    assert!(wakes >= 1, "the commit's release woke the waiter");
                }
                BusyWake::Schedule => {
                    assert_eq!(
                        woken_before,
                        Some(1),
                        "the schedule wakes the waker at once"
                    );
                }
            }
            let mut rows = 0;
            waiter
                .query("SELECT x FROM t", [], |_| {
                    rows += 1;
                    Ok(true)
                })
                .unwrap();
            assert_eq!(rows, 2, "both inserts committed");
        }
    }

    /// Says, as it drops, how many strong handles its engine still has.
    struct EngineWitness {
        engine: std::sync::Weak<Database>,
        dropped: std::sync::mpsc::Sender<usize>,
    }
    impl Drop for EngineWitness {
        fn drop(&mut self) {
            self.dropped.send(self.engine.strong_count()).unwrap();
        }
    }

    #[test]
    fn the_owner_guards_drop_after_the_engine_in_every_holder() {
        // CP-T1.5 (U05), local-owner-transport.md "Owner guards": a reader, a reader's opener
        // and the flusher each keep the owner's guards until they drop, and drop the engine's
        // handles before them. Each holder is made the last, then dropped: the guards drop
        // then, with no engine handle left.
        type Holder = Box<dyn FnOnce(&Db) -> Box<dyn std::any::Any>>;
        let holders: [(&str, Holder); 3] = [
            ("reader", Box::new(|db| Box::new(db.reader().unwrap()))),
            ("opener", Box::new(|db| Box::new(db.opener()))),
            ("flusher", Box::new(|db| Box::new(db.flusher()))),
        ];
        for (name, hold) in holders {
            let dir = tempfile::tempdir().unwrap();
            let mut db = Db::open(&dir.path().join("db")).unwrap();
            let (sender, dropped) = std::sync::mpsc::channel();
            let witness = EngineWitness {
                engine: Arc::downgrade(&db.db),
                dropped: sender,
            };
            db.hold_owner(crate::database::Owned::unbudgeted(
                Arc::new(std::fs::File::create(dir.path().join("lease")).unwrap()),
                Some(crate::database::Guards::new([7; 32], Arc::new(witness))),
            ));
            let holder = hold(&db);
            drop(db);
            assert!(
                dropped.try_recv().is_err(),
                "{name}: the guards dropped early"
            );
            drop(holder);
            assert_eq!(
                dropped.try_recv().unwrap(),
                0,
                "{name}: the guards dropped before the engine's handles"
            );
        }
    }

    /// U15/CP-T1.8: failure after raw pages and schema refresh must expose neither the imported
    /// catalog nor its rows. A later successful apply persists both through the real flusher.
    #[test]
    fn raw_page_failure_restores_catalog_and_success_persists_the_same_image() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = Db::open(&dir.path().join("source.db")).unwrap();
        source
            .exec("CREATE TABLE replacement(value TEXT); INSERT INTO replacement VALUES ('seed')")
            .unwrap();
        let image = source.image_pages(1 << 20).unwrap();
        let target_path = dir.path().join("target.db");
        let mut target = Db::open(&target_path).unwrap();
        target
            .exec("CREATE TABLE original(value TEXT); INSERT INTO original VALUES ('retained'); CREATE TABLE discarded_one(n); CREATE TABLE discarded_two(n)")
            .unwrap();
        target.flusher()().unwrap();
        let failed: anyhow::Result<()> = target.apply_pages(&image, |db| {
            db.query("SELECT value FROM replacement", [], |row| {
                assert_eq!(row.get::<String>(0).unwrap(), "seed");
                Ok(false)
            })?;
            anyhow::bail!("refuse before imported pages receive their commit marker")
        });
        assert!(failed.is_err());
        target
            .query("SELECT value FROM original", [], |row| {
                assert_eq!(row.get::<String>(0).unwrap(), "retained");
                Ok(false)
            })
            .unwrap();
        assert!(
            target
                .query("SELECT value FROM replacement", [], |_| Ok(false))
                .is_err()
        );
        target
            .apply_pages(&image, |db| {
                db.exec("INSERT INTO replacement VALUES ('committed')")
            })
            .unwrap();
        target.flusher()().unwrap();
        let reader = target.reader().unwrap();
        reader
            .query("SELECT value FROM replacement WHERE rowid = 1", [], |row| {
                assert_eq!(row.get::<String>(0).unwrap(), "seed");
                Ok(false)
            })
            .unwrap();
        drop(reader);
        drop(target);
        let target = Db::open(&target_path).unwrap();
        let mut rows = Vec::new();
        target
            .query("SELECT value FROM replacement ORDER BY rowid", [], |row| {
                rows.push(row.get::<String>(0).unwrap());
                Ok(true)
            })
            .unwrap();
        assert_eq!(rows, ["seed", "committed"]);
        assert!(
            target
                .query("SELECT value FROM original", [], |_| Ok(false))
                .is_err()
        );
        // U05, CP-T1.8 (replication.md, "Raising the floor", d): a floor this group's members
        // didn't prove never enters replicated mode. An image carrying one where joins aren't
        // tracked is refused at install, rolled back before its marker; a database holding one
        // is refused as a replica at open.
        crate::database::bootstrap_db(&source).unwrap();
        source
            .exec("CREATE TABLE __amphora_application_floor (singleton INTEGER PRIMARY KEY, application TEXT NOT NULL, version INTEGER NOT NULL)")
            .unwrap();
        source
            .exec("INSERT INTO __amphora_application_floor VALUES(1, 'kiln.host-store', 6)")
            .unwrap();
        source.flusher()().unwrap();
        let floor_image = postcard::to_allocvec(&source.image_pages(1 << 20).unwrap()).unwrap();
        let mut replica = crate::replication::Machine::open(
            &dir.path().join("replica.db"),
            hyper_raft::proto::ConfState::default(),
        )
        .unwrap();
        let before = hyper_durable::StateMachine::durable(&replica);
        assert!(
            hyper_durable::StateMachine::install(
                &mut replica,
                &floor_image,
                hyper_durable::Point { index: 2, term: 1 },
                &hyper_raft::proto::ConfState::default(),
            )
            .is_err()
        );
        assert_eq!(hyper_durable::StateMachine::durable(&replica), before);
        assert_eq!(
            replica
                .read_db(|db| crate::database::installed_floor(db))
                .unwrap(),
            None
        );
        drop(replica);
        drop(source);
        let refusal = crate::replication::Machine::open(
            &dir.path().join("source.db"),
            hyper_raft::proto::ConfState::default(),
        )
        .err()
        .unwrap();
        assert!(
            matches!(&refusal, amphora_api::Refusal::Fenced(reason) if reason == "a floor not proven by this group's members; raise it again once joins are tracked"),
            "{refusal:?}"
        );
    }

    /// U05/U15, bug 68a308b7: Db::exec of a statement its deadline interrupts reports the
    /// engine's interrupt, never busy, which is another connection's activity
    /// (sqlite.org/rescode.html: SQLITE_INTERRUPT, SQLITE_BUSY). Do: exec a long insert under a
    /// deadline already past, then the same through Db::query's own runner. Expect:
    /// LimboError::Interrupt from both, nothing of either inserted, and the writer usable after.
    #[test]
    fn exec_and_query_report_a_deadline_interrupt_as_an_interrupt() {
        let mut db = Db::open_with(
            Path::new(":memory:interrupt"),
            Arc::new(amphora_engine::MemoryIO::new()),
        )
        .unwrap();
        db.exec("CREATE TABLE numbers(n)").unwrap();
        const LONG: &str = "INSERT INTO numbers WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x < 10000000) SELECT x FROM n";
        let exec = db.transaction(|db| {
            let _deadline = db.deadline(Instant::now());
            db.exec(LONG)
        });
        let error = exec.unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<LimboError>(),
                Some(LimboError::Interrupt)
            ),
            "{error:#}"
        );
        let query = db.transaction(|db| {
            let _deadline = db.deadline(Instant::now());
            db.query(LONG, [], |_| Ok(true))
        });
        let error = query.unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<LimboError>(),
                Some(LimboError::Interrupt)
            ),
            "{error:#}"
        );
        db.transaction(|db| db.exec("INSERT INTO numbers VALUES (1)"))
            .unwrap();
        let mut count = 0;
        db.query("SELECT count(*) FROM numbers", [], |row| {
            count = row.get::<i64>(0).unwrap();
            Ok(false)
        })
        .unwrap();
        assert_eq!(count, 1);
    }

    /// U05/U15: an expired SQL VM is interrupted and the guard clears before rollback,
    /// leaving the same writer usable for the next transaction.
    #[test]
    fn deadline_interrupts_sql_and_cleanup_remains_usable() {
        let mut db = Db::open_with(
            Path::new(":memory:deadline"),
            Arc::new(amphora_engine::MemoryIO::new()),
        )
        .unwrap();
        db.exec("CREATE TABLE retained(n)").unwrap();
        let result = db.transaction(|db| {
            let _deadline = db.deadline(Instant::now());
            db.query_owned("WITH RECURSIVE numbers(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM numbers WHERE n < 10000000) SELECT sum(n) FROM numbers", [], |_| Ok(true))
        });
        assert!(result.unwrap_err().to_string().contains("interrupt"));
        db.transaction(|db| db.exec("INSERT INTO retained VALUES (42)"))
            .unwrap();
        db.query("SELECT n FROM retained", [], |row| {
            assert_eq!(row.get::<i64>(0).unwrap(), 42);
            Ok(false)
        })
        .unwrap();

        // A hard page refusal rolls the entire attempt back before the writer is reused.
        let refused = db.compaction_attempt(1, false, |db| {
            db.exec("CREATE TABLE oversized(payload BLOB); INSERT INTO oversized VALUES (zeroblob(65536))")
        }).unwrap_err();
        assert!(matches!(
            refused.downcast_ref::<LimboError>(),
            Some(LimboError::TransactionPageLimit { .. })
        ));
        assert!(
            db.query("SELECT * FROM oversized", [], |_| Ok(false))
                .is_err()
        );
        db.transaction(|db| db.exec("INSERT INTO retained VALUES (43)"))
            .unwrap();

        // An actual cleanup interruption fences every later SQL route, including a reader
        // created before the failure. Clearing the interrupt cannot clear owner fencing.
        let reader = db.reader().unwrap();
        let failed: anyhow::Result<((), Pages)> = db.compaction_attempt(100, false, |db| {
            db.exec("INSERT INTO retained VALUES (44)")?;
            db.conn.set_progress_handler(1, Some(Box::new(|| true)));
            anyhow::bail!("force rollback after the application error")
        });
        assert!(matches!(
            failed.unwrap_err().downcast_ref::<amphora_api::Refusal>(),
            Some(amphora_api::Refusal::Fenced(_))
        ));
        db.conn.set_progress_handler(0, None);
        assert!(db.exec("INSERT INTO retained VALUES (45)").is_err());
        assert!(
            db.query_owned("SELECT n FROM retained", [], |_| Ok(false))
                .is_err()
        );
        assert!(db.column_count("SELECT n FROM retained").is_err());
        assert!(db.reader().is_err());
        assert!(
            reader
                .query("SELECT n FROM retained", [], |_| Ok(false))
                .is_err()
        );
        assert!(db.apply_pages(&[], |_| Ok(())).is_err());
    }

    /// Do: size the process's pool for 32 GiB, for 16 MiB, for none said and for more than a
    /// `usize` holds. Expect: a quarter in 4 KiB pages, the floor, the fallback, and no overflow
    /// (decision 26; AC-T1.13.4: reads at their latency).
    #[test]
    fn the_process_pool_is_a_quarter_of_its_memory() {
        assert_eq!(default_pages(Some(32 << 30)), (8 << 30) / 4096);
        assert_eq!(default_pages(Some(16 << 20)), MIN_PROCESS_PAGES);
        assert_eq!(default_pages(None), UNKNOWN_MEMORY_PAGES);
        assert!(default_pages(Some(u64::MAX)) >= MIN_PROCESS_PAGES);
    }
}
