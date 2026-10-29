//! Stores on Amphora's engine (`crates/engine`): a database opened the way Amphora's pipeline wants it
//! ([`Db::open`]), its readers ([`Db::reader`]), and what the store's flusher, checkpointer and
//! writer do with it ([`Db::flusher`], [`Db::checkpoint`], [`Db::restart_log`]). An engine on Turso
//! keeps a [`Db`] for its writer and gives readers their own.

use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use amphora_engine::io::FileSyncType;
use amphora_engine::{
    CheckpointMode, Completion, CompletionError, ConnectOpts, Connection, Database, DatabaseOpts,
    IO, IOExt, LimboError, OpenFlags, OpenOptions, PlatformIO, Row, SqliteDialect, Statement,
    StepResult, Value,
};

use crate::store::{Checkpointed, Flush, Opener};

mod cache;

/// The pages a process's databases share (docs/design/store.md, decision 20).
pub use amphora_engine::storage::shared_pages::SharedPages;

/// How long opening waits for another process's hold on the file to go.
const LOCK_WAIT: Duration = Duration::from_secs(15);
/// A statement that finds the database busy (another connection writing, a checkpoint) tries again
/// after this, this many times.
const BUSY_WAIT: Duration = Duration::from_millis(1);
const BUSY_TRIES: u32 = 10_000;
/// Pages each connection keeps in memory, by what it does (docs/design/store.md, decision 5): the
/// writer keeps the inner pages of its indexes; a reader 2,000 (8 MiB, Turso's own default), since
/// it forgets only what commits wrote (decision 18), so its cache serves the reads after them; the
/// checkpointer next to nothing.
pub const WRITER_PAGES: i64 = 1024;
pub const READER_PAGES: i64 = 2000;

/// The bytes of a page of Amphora's databases (the engine's default page size): what a pool's page
/// budget is counted in.
const PAGE_BYTES: u64 = 4096;
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
    /// None: each connection keeps its own cache alone (a packed log needs this; decision 16).
    Off,
}

/// The pool every database opened with [`Sharing::Process`] shares: made when the first opens,
/// with room for [`default_pages`] of the memory this process may use.
pub fn process_pages() -> Arc<SharedPages> {
    static POOL: OnceLock<Arc<SharedPages>> = OnceLock::new();
    POOL.get_or_init(|| Arc::new(SharedPages::new(default_pages(crate::memory::limit()))))
        .clone()
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

fn fail(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

/// Parks this thread for `d` (a park ends early for no reason sometimes: the rest is parked again).
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

/// The log beside the database at `path`.
fn wal_path(path: &Path) -> PathBuf {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    PathBuf::from(wal)
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

pub struct Db {
    io: Arc<dyn IO>,
    db: Arc<Database>,
    conn: Arc<Connection>,
    path: PathBuf,
    statements: cache::Cache,
    owner: Option<Arc<std::fs::File>>,
}

impl Db {
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
        in_place(path);
        in_place(&wal_path(path));
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
        let conn = db
            .connect(ConnectOpts::new())
            .map_err(|e| anyhow::anyhow!("connecting to {name}: {e}"))?;
        conn.set_statement_journals(false);
        let d = Self {
            io,
            db,
            conn,
            path: path.to_path_buf(),
            statements: cache::Cache::default(),
            owner: None,
        };
        d.exec("PRAGMA synchronous = NORMAL")?;
        d.exec("PRAGMA foreign_keys = ON")?;
        d.exec("PRAGMA wal_autocheckpoint = 0")?;
        d.exec(&format!("PRAGMA cache_size = {WRITER_PAGES}"))?;
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

    /// Another connection on the same database, for reading or checkpointing: each read sees what
    /// was committed before it; its checkpoints flush.
    pub fn reader(&self) -> anyhow::Result<Self> {
        let r = Self {
            io: self.io.clone(),
            db: self.db.clone(),
            conn: self.db.connect(ConnectOpts::new()).map_err(fail)?,
            path: self.path.clone(),
            statements: cache::Cache::default(),
            owner: self.owner.clone(),
        };
        r.keep_pages(READER_PAGES)?;
        Ok(r)
    }

    /// Opens readers on this database from any thread.
    #[must_use]
    pub fn opener(&self) -> Opener<Self> {
        let (io, db, path) = (self.io.clone(), self.db.clone(), self.path.clone());
        let owner = self.owner.clone();
        Arc::new(move || {
            let r = Self {
                io: io.clone(),
                db: db.clone(),
                conn: db.connect(ConnectOpts::new()).map_err(fail)?,
                path: path.clone(),
                statements: cache::Cache::default(),
                owner: owner.clone(),
            };
            r.keep_pages(READER_PAGES)?;
            Ok(r)
        })
    }

    /// Flushes the log, through the database's own IO: what was committed before the flush began
    /// is durable once it ends. Its data only (`fdatasync`; on macOS `F_FULLFSYNC`, its `fsync`
    /// leaving the disk's cache): the log keeps its length across restarts, so once it has been
    /// as long as it gets, a flush writes no metadata.
    pub fn flusher(&self) -> Flush {
        let wal = format!("{}-wal", self.path.display());
        let (io, db) = (self.io.clone(), self.db.clone());
        let owner = self.owner.clone();
        Box::new(move || {
            let _lease = &owner;
            // Where the log ends before the flush begins: every frame up to it is durable once
            // the flush ends, and the checkpointer copies those without a flush of its own.
            let (generation, frame) = db.wal_end();
            // Opened each time: a flush is a disk's round trip, an open a few microseconds, and the
            // log may have been made again. No log: everything is in the file, which the checkpoint
            // that emptied the log flushed.
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
            db.wal_durable(generation, frame);
            Ok(())
        })
    }

    /// Copies what the log holds into the file without waiting for anyone: what the log holds
    /// after, for the store to start it again once it is past its bound ([`Db::restart_log`]). A
    /// checkpoint already under way (busy) copies nothing and reports nothing. Once a flush has
    /// made some of the log durable ([`Db::flusher`]), only that is copied, and the log isn't
    /// flushed again for it.
    pub fn checkpoint(&mut self) -> anyhow::Result<Checkpointed> {
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

    /// With no reader reading (the store holds them back), copies what remains, flushes the file,
    /// and starts the log from its beginning, its file kept: whether it did.
    pub fn restart_log(&mut self) -> anyhow::Result<bool> {
        match self.conn.checkpoint(CheckpointMode::Restart) {
            Ok(_) => Ok(true),
            Err(LimboError::Busy) => Ok(false),
            Err(e) => Err(fail(e)),
        }
    }

    /// The file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn hold_owner(&mut self, owner: Arc<std::fs::File>) {
        self.owner = Some(owner);
    }

    /// Whether the log's generations from the next one written are packed (docs/design/store.md,
    /// decisions 16 and 17): not unless this asks, while the bench weighs them (`AMPHORA_PACK_LOG`).
    /// A generation keeps the format it began with. A packed log's pages aren't shared: a database
    /// that packs it is opened with [`Sharing::Off`].
    pub fn pack_log(&self, pack: bool) -> anyhow::Result<()> {
        self.db
            .wal_pack(pack)
            .map_err(|e| anyhow::anyhow!("packing {}'s log: {e}", self.path.display()))
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
        // The engine's parser knows where a statement ends, including trigger bodies and quoted
        // semicolons. U05: splitting text on ';' corrupts valid migration scripts.
        self.conn.prepare_execute_batch(sql).map_err(fail)
    }

    /// Owned/dynamic SQL without an unbounded statement cache (U05). The caller bounds results.
    pub fn query_owned(
        &self,
        sql: &str,
        params: impl IntoIterator<Item = Value>,
        each: impl FnMut(&Row) -> anyhow::Result<bool>,
    ) -> anyhow::Result<()> {
        let mut stmt = match self.statements.take(sql)? {
            Some(stmt) => stmt,
            None => self.conn.prepare(sql).map_err(fail)?,
        };
        step(self.io.as_ref(), &mut stmt, params, each)?;
        self.statements.put(sql, stmt)?;
        Ok(())
    }

    pub fn changes(&self) -> i64 {
        self.conn.changes()
    }

    pub fn column_count(&self, sql: &str) -> anyhow::Result<usize> {
        let stmt = match self.statements.take(sql)? {
            Some(stmt) => stmt,
            None => self.conn.prepare(sql).map_err(fail)?,
        };
        let columns = stmt.num_columns();
        self.statements.put(sql, stmt)?;
        Ok(columns)
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.conn.last_insert_rowid()
    }

    /// Validates imported rows using the engine's resolved FK metadata. The engine currently
    /// implements foreign_key_list and enforcement, but not PRAGMA foreign_key_check; an unknown
    /// PRAGMA returns no rows and therefore cannot prove a restored database consistent.
    pub(crate) fn validate_foreign_keys(&self) -> anyhow::Result<()> {
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

    pub(crate) fn image_pages(&mut self, max_bytes: usize) -> anyhow::Result<Pages> {
        self.exec("BEGIN")?;
        let result = (|| {
            let pager = self.conn.get_pager();
            let count = self
                .io
                .block(|| pager.with_header(|h| h.database_size.get()))?;
            let page_size = usize::try_from(self.conn.get_page_size().get())?;
            let size = usize::try_from(count)?
                .checked_mul(page_size)
                .ok_or_else(|| anyhow::anyhow!("snapshot size overflow"))?;
            if size > max_bytes {
                anyhow::bail!("snapshot exceeds its byte budget");
            }
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
    pub fn query(
        &self,
        sql: &'static str,
        params: impl IntoIterator<Item = Value>,
        each: impl FnMut(&Row) -> anyhow::Result<bool>,
    ) -> anyhow::Result<()> {
        self.query_owned(sql, params, each)
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

/// Runs `stmt` with `params` to its end, handing each row to `each` until it says stop (false).
pub fn step(
    io: &dyn IO,
    stmt: &mut Statement,
    params: impl IntoIterator<Item = Value>,
    mut each: impl FnMut(&Row) -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    stmt.reset().map_err(fail)?;
    for (i, p) in params.into_iter().enumerate() {
        let at = NonZero::new(i.saturating_add(1)).ok_or_else(|| anyhow::anyhow!("parameter 0"))?;
        stmt.bind_at(at, p).map_err(fail)?;
    }
    let mut busy = 0u32;
    loop {
        match stmt.step().map_err(fail)? {
            StepResult::Row => {
                if let Some(row) = stmt.row()
                    && !each(row)?
                {
                    return stmt.reset().map_err(fail);
                }
            }
            StepResult::Done => return Ok(()),
            StepResult::IO | StepResult::Yield => io.step().map_err(fail)?,
            StepResult::Sleep { duration } => {
                pause(duration);
                io.step().map_err(fail)?;
            }
            StepResult::Busy => {
                busy = busy.saturating_add(1);
                if busy > BUSY_TRIES {
                    anyhow::bail!("the database stayed busy");
                }
                pause(BUSY_WAIT);
            }
            StepResult::Interrupt => anyhow::bail!("a statement was interrupted"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
